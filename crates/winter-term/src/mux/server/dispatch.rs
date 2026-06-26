//! The server's state machine: which clients are connected, which sessions
//! each is attached to and what size it wants them, and what every request
//! and session event does to that state. Free of sockets and threads, so it
//! is driven directly by the tests.

use std::collections::HashMap;

use portable_pty::CommandBuilder;

use super::super::persist::SessionDef;
use super::super::protocol::{self, ClientMessage, Geometry, ServerMessage, MUX_PROTOCOL_VERSION};
use super::super::session::{SessionEvent, SessionExit, SessionId, SessionManager, SessionOutput};
use super::connection::Outbox;
use super::size::sync_session_size;

// ========================================================================
// Constants
// ========================================================================

/// Geometry used when creating a session whose size the attaching client
/// has not reported yet (the client resizes right after attaching).
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// The shell a command runs through when `$SHELL` is unset.
const FALLBACK_SHELL: &str = "sh";

// ========================================================================
// Data Structures
// ========================================================================

/// Every connected client and every session, and what one does to the other.
pub(super) struct Dispatcher {
    clients: HashMap<u64, Client>,
    manager: SessionManager,
    /// Set when a session was created or ended, so the event loop rewrites
    /// the respawn recipes; cleared by [`Dispatcher::take_sessions_changed`].
    sessions_changed: bool,
}

/// One connected client's state.
pub(super) struct Client {
    attachments: Vec<SessionId>,
    /// Set when the client should be dropped once the current event is
    /// handled: it fell too far behind, or it cannot speak the protocol.
    doomed: bool,
    outbox: Outbox,
    /// Geometry the client asked for per session, the input to size
    /// arbitration: the session sizes to the smallest of these.
    wanted: HashMap<SessionId, Geometry>,
}

// ========================================================================
// Client
// ========================================================================

impl Client {
    /// A client attached to nothing, whose frames go to `outbox`.
    pub(super) fn new(outbox: Outbox) -> Self {
        Client {
            attachments: Vec::new(),
            doomed: false,
            outbox,
            wanted: HashMap::new(),
        }
    }

    /// Queue a message for delivery, dooming the client when it has fallen
    /// too far behind to keep buffering.
    pub(super) fn enqueue(&mut self, msg: &ServerMessage) {
        if !self.doomed && !self.outbox.push(protocol::encode(msg)) {
            self.doomed = true;
        }
    }

    /// Whether this client is attached to `id`.
    pub(super) fn is_attached(&self, id: &SessionId) -> bool {
        self.attachments.contains(id)
    }

    /// The geometry this client asked `id` to have, if it asked.
    pub(super) fn wanted(&self, id: &SessionId) -> Option<Geometry> {
        self.wanted.get(id).copied()
    }

    /// Record the geometry this client wants `id` to have.
    pub(super) fn want(&mut self, id: SessionId, geometry: Geometry) {
        self.wanted.insert(id, geometry);
    }

    fn attach(&mut self, id: &SessionId) {
        if !self.is_attached(id) {
            self.attachments.push(id.clone());
        }
    }

    /// Forget `id`: it has ended.
    fn forget(&mut self, id: &SessionId) {
        self.attachments.retain(|attached| attached != id);
        self.wanted.remove(id);
    }
}

// ========================================================================
// Dispatcher
// ========================================================================

impl Dispatcher {
    // --------------------------------------------------------------------
    // Event loop entry points
    // --------------------------------------------------------------------

    /// A dispatcher over `manager`'s sessions, with no clients yet.
    pub(super) fn new(manager: SessionManager) -> Self {
        Dispatcher {
            clients: HashMap::new(),
            manager,
            sessions_changed: false,
        }
    }

    /// Start tracking a newly connected client.
    pub(super) fn add_client(&mut self, cid: u64, outbox: Outbox) {
        self.clients.insert(cid, Client::new(outbox));
    }

    /// Stop tracking a client. Its geometry constraints vanish with it, so
    /// the sessions it shrank may grow back toward the remaining clients.
    pub(super) fn remove_client(&mut self, cid: u64) {
        if let Some(client) = self.clients.remove(&cid) {
            for id in client.wanted.keys() {
                sync_session_size(&mut self.manager, &mut self.clients, id);
            }
        }
    }

    /// Drop every client marked doomed while handling the last event.
    pub(super) fn reap_doomed(&mut self) {
        let doomed: Vec<u64> = self
            .clients
            .iter()
            .filter(|(_, client)| client.doomed)
            .map(|(&cid, _)| cid)
            .collect();
        for cid in doomed {
            self.remove_client(cid);
        }
    }

    /// Carry out one frame from client `cid`, or tell it why its frame
    /// could not be read.
    pub(super) fn handle_frame(&mut self, cid: u64, frame: Result<ClientMessage, String>) {
        match frame {
            Ok(msg) => self.handle_message(cid, msg),
            Err(reason) => self.reply_error(cid, format!("unreadable request: {reason}")),
        }
    }

    /// React to a session's program writing output or exiting. Reports from
    /// an incarnation that is no longer current (a killed session whose
    /// reader is still winding down) are dropped: the name may already
    /// belong to a new session.
    pub(super) fn handle_session_event(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Exit(SessionExit { code, id }) => {
                if self.manager.remove(&id) {
                    self.end_session(&id, code);
                }
            }
            SessionEvent::Output(SessionOutput { bytes, id }) => {
                if !self.manager.is_current(&id) {
                    return;
                }
                self.manager.record_output(&id, &bytes);
                let msg = ServerMessage::Output {
                    session: id.name.clone(),
                    bytes,
                };
                for client in self.clients.values_mut() {
                    if client.is_attached(&id) {
                        client.enqueue(&msg);
                    }
                }
            }
        }
    }

    /// Recreate `defs` (scrollback does not survive: each respawn is a
    /// brand-new PTY). A def that names no valid session, or whose command
    /// can no longer spawn, is logged and dropped rather than aborting the
    /// rest of the batch.
    pub(super) fn respawn(&mut self, defs: Vec<SessionDef>) {
        for def in defs {
            if let Err(e) = protocol::validate_session_name(&def.name) {
                eprintln!("winter mux: not respawning: {e}");
                self.sessions_changed = true;
                continue;
            }
            let builder = build_spawn_command(def.command.as_deref(), def.cwd.as_deref());
            if let Err(e) = self.manager.create_with(
                &def.name,
                DEFAULT_COLS,
                DEFAULT_ROWS,
                builder,
                def.command.as_deref(),
                def.cwd.as_deref(),
            ) {
                eprintln!("winter mux: failed to respawn '{}': {e}", def.name);
                self.sessions_changed = true;
            }
        }
    }

    /// Whether the set of sessions changed since the last call.
    pub(super) fn take_sessions_changed(&mut self) -> bool {
        std::mem::take(&mut self.sessions_changed)
    }

    /// Every live session's respawn recipe.
    pub(super) fn session_defs(&self) -> Vec<SessionDef> {
        self.manager.session_defs()
    }

    // --------------------------------------------------------------------
    // Requests
    // --------------------------------------------------------------------

    fn handle_message(&mut self, cid: u64, msg: ClientMessage) {
        match msg {
            ClientMessage::Attach { replay, session } => self.attach(cid, &session, replay),
            ClientMessage::Detach => self.detach(cid),
            ClientMessage::Hello { version } => self.greet(cid, version),
            ClientMessage::Input { bytes, session } => self.input(cid, &session, &bytes),
            ClientMessage::Kill { session } => self.kill(cid, &session),
            ClientMessage::ListSessions => self.list(cid),
            ClientMessage::Resize {
                cell_px,
                cols,
                rows,
                session,
            } => self.resize(
                cid,
                &session,
                Geometry {
                    cell_px,
                    cols,
                    rows,
                },
            ),
            ClientMessage::Spawn {
                command,
                cols,
                cwd,
                rows,
                session,
            } => self.spawn(
                cid,
                &session,
                cols,
                rows,
                cwd.as_deref(),
                command.as_deref(),
            ),
        }
    }

    /// Answer a handshake: the server's version, or an error and a closed
    /// connection for a client speaking another one.
    fn greet(&mut self, cid: u64, version: u16) {
        if version == MUX_PROTOCOL_VERSION {
            self.reply(
                cid,
                &ServerMessage::Hello {
                    version: MUX_PROTOCOL_VERSION,
                },
            );
            return;
        }
        self.reply_error(
            cid,
            format!(
                "mux protocol version {version} is not supported; \
                 this server speaks version {MUX_PROTOCOL_VERSION}"
            ),
        );
        if let Some(client) = self.clients.get_mut(&cid) {
            client.doomed = true;
        }
    }

    /// Attach to `name`, creating it with the default shell if absent.
    fn attach(&mut self, cid: u64, name: &str, replay: bool) {
        if let Err(e) = protocol::validate_session_name(name) {
            return self.reply_error(cid, e);
        }
        let id = match self.manager.current(name) {
            Some(id) => id,
            None => match self.manager.create(name, DEFAULT_COLS, DEFAULT_ROWS) {
                Ok(id) => {
                    self.sessions_changed = true;
                    id
                }
                Err(e) => return self.reply_error(cid, e.to_string()),
            },
        };
        let replies = attach_replies(&self.manager, name, replay);
        if let Some(client) = self.clients.get_mut(&cid) {
            client.attach(&id);
            for msg in &replies {
                client.enqueue(msg);
            }
        }
    }

    fn detach(&mut self, cid: u64) {
        let Some(client) = self.clients.get_mut(&cid) else {
            return;
        };
        let affected: Vec<SessionId> = client.wanted.keys().cloned().collect();
        client.attachments.clear();
        client.wanted.clear();
        // The detached client no longer constrains these sessions;
        // re-arbitrate so they can grow back.
        for id in &affected {
            sync_session_size(&mut self.manager, &mut self.clients, id);
        }
    }

    /// Input is the injection vector: only a client attached to the session
    /// may feed it bytes. (Kill stays open to unattached clients because
    /// `winter mux kill` and the palette's kill command connect without
    /// attaching by design.)
    fn input(&mut self, cid: u64, name: &str, bytes: &[u8]) {
        match self.attached_session(cid, name) {
            Some(_) => {
                let _ = self.manager.write(name, bytes);
            }
            None => self.reply_error(cid, format!("not attached to session '{name}'")),
        }
    }

    /// Record the client's wish, then arbitrate: the session sizes to the
    /// smallest attached geometry, and every attached client learns the
    /// result (see [`sync_session_size`]).
    fn resize(&mut self, cid: u64, name: &str, geometry: Geometry) {
        let Some(id) = self.attached_session(cid, name) else {
            return self.reply_error(cid, format!("not attached to session '{name}'"));
        };
        if let Some(client) = self.clients.get_mut(&cid) {
            client.want(id.clone(), geometry);
        }
        sync_session_size(&mut self.manager, &mut self.clients, &id);
    }

    /// Create `name` running `command` (the default shell when `None`) in
    /// `cwd`, then attach the creator to it. An existing name is an error,
    /// not a clobber.
    fn spawn(
        &mut self,
        cid: u64,
        name: &str,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        command: Option<&str>,
    ) {
        if let Err(e) = protocol::validate_session_name(name) {
            return self.reply_error(cid, e);
        }
        if self.manager.has(name) {
            return self.reply_error(cid, format!("session '{name}' already exists"));
        }
        let builder = build_spawn_command(command, cwd);
        let id = match self
            .manager
            .create_with(name, cols, rows, builder, command, cwd)
        {
            Ok(id) => id,
            Err(e) => return self.reply_error(cid, e.to_string()),
        };
        self.sessions_changed = true;
        let replies = attach_replies(&self.manager, name, true);
        if let Some(client) = self.clients.get_mut(&cid) {
            client.attach(&id);
            client.want(
                id.clone(),
                Geometry {
                    cell_px: None,
                    cols,
                    rows,
                },
            );
            for msg in &replies {
                client.enqueue(msg);
            }
        }
        // Another attached client may constrain the new session smaller
        // than its spawn geometry.
        sync_session_size(&mut self.manager, &mut self.clients, &id);
    }

    fn list(&mut self, cid: u64) {
        let mut sessions = self.manager.session_info();
        for info in &mut sessions {
            info.attach_count = self.manager.current(&info.name).map_or(0, |id| {
                self.clients.values().filter(|c| c.is_attached(&id)).count()
            });
        }
        self.reply(cid, &ServerMessage::SessionList { sessions });
    }

    /// End `name` at a client's request. Its attached clients hear of it
    /// now: the program's own exit report will arrive for an incarnation no
    /// longer current, and be dropped.
    fn kill(&mut self, cid: u64, name: &str) {
        match self.manager.kill(name) {
            Some(id) => self.end_session(&id, None),
            None => self.reply_error(cid, format!("no session named '{name}'")),
        }
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// The live session `name`, when client `cid` is attached to it.
    fn attached_session(&self, cid: u64, name: &str) -> Option<SessionId> {
        let id = self.manager.current(name)?;
        let client = self.clients.get(&cid)?;
        client.is_attached(&id).then_some(id)
    }

    /// Tell every client attached to `id` that it ended, and detach them,
    /// so a later session reusing the name starts with no one attached.
    fn end_session(&mut self, id: &SessionId, code: Option<i32>) {
        self.sessions_changed = true;
        let msg = ServerMessage::Exit {
            session: id.name.clone(),
            code,
        };
        for client in self.clients.values_mut() {
            if client.is_attached(id) {
                client.enqueue(&msg);
            }
            client.forget(id);
        }
    }

    fn reply(&mut self, cid: u64, msg: &ServerMessage) {
        if let Some(client) = self.clients.get_mut(&cid) {
            client.enqueue(msg);
        }
    }

    fn reply_error(&mut self, cid: u64, message: String) {
        self.reply(cid, &ServerMessage::Error { message });
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// The messages a client receives right after attaching: confirmation,
/// then, when `replay` asks for it, the session's buffered output so a
/// fresh client rebuilds its screen and scrollback before live output
/// resumes.
fn attach_replies(manager: &SessionManager, name: &str, replay: bool) -> Vec<ServerMessage> {
    let (cols, rows) = manager
        .geometry(name)
        .unwrap_or((DEFAULT_COLS, DEFAULT_ROWS));
    let mut replies = vec![ServerMessage::Attached {
        session: name.to_string(),
        cols,
        rows,
    }];
    if !replay {
        return replies;
    }
    if let Some(bytes) = manager.scrollback(name) {
        if !bytes.is_empty() {
            replies.push(ServerMessage::Scrollback {
                session: name.to_string(),
                bytes,
            });
        }
    }
    replies
}

/// The command a Spawn runs: the given command line through the user's
/// shell (so it can carry arguments, pipes, and env), or the default shell
/// when none was given; `cwd` overrides the working directory.
fn build_spawn_command(command: Option<&str>, cwd: Option<&str>) -> CommandBuilder {
    let mut builder = match command {
        Some(cmd) if !cmd.trim().is_empty() => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| FALLBACK_SHELL.to_string());
            let mut b = CommandBuilder::new(shell);
            b.arg("-c");
            b.arg(cmd);
            b
        }
        _ => CommandBuilder::new_default_prog(),
    };
    if let Some(dir) = cwd {
        builder.cwd(dir);
    }
    builder
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::{mpsc, Arc};

    use super::*;
    use crate::mux::server::connection::CLIENT_OUTBOX_LIMIT;
    use crate::mux::session::{EventSink, SCROLLBACK_CAPACITY_BYTES};

    /// A dispatcher whose sessions report into a channel nobody reads.
    fn dispatcher() -> Dispatcher {
        let sink: EventSink = Arc::new(|_| {});
        Dispatcher::new(SessionManager::new(sink))
    }

    /// Connect client `cid`, returning what the server sends it.
    fn connect(dispatcher: &mut Dispatcher, cid: u64) -> mpsc::Receiver<Vec<u8>> {
        let (frames, queued) = mpsc::channel();
        dispatcher.add_client(cid, Outbox::new(frames));
        queued
    }

    /// The next message the server sent a client.
    fn next(queued: &mpsc::Receiver<Vec<u8>>) -> ServerMessage {
        let frame = queued.try_recv().expect("a message was sent");
        protocol::decode(&frame).expect("the frame decodes")
    }

    /// Every message the server sent a client so far.
    fn drain(queued: &mpsc::Receiver<Vec<u8>>) -> Vec<ServerMessage> {
        queued
            .try_iter()
            .map(|frame| protocol::decode(&frame).expect("the frame decodes"))
            .collect()
    }

    fn attach(name: &str) -> ClientMessage {
        ClientMessage::Attach {
            replay: true,
            session: name.into(),
        }
    }

    fn resize(name: &str, cols: u16, rows: u16) -> ClientMessage {
        ClientMessage::Resize {
            cell_px: None,
            cols,
            rows,
            session: name.into(),
        }
    }

    fn spawn(name: &str, cols: u16, rows: u16, command: Option<&str>) -> ClientMessage {
        ClientMessage::Spawn {
            command: command.map(str::to_string),
            cols,
            cwd: None,
            rows,
            session: name.into(),
        }
    }

    #[test]
    fn test_a_full_scrollback_replay_fits_the_outbox() {
        // Regression: bytes were JSON number arrays, up to four wire bytes
        // per byte, so a full ring of non-ASCII output encoded past the cap
        // and the client was dropped the moment it attached.
        let frame = protocol::encode(&ServerMessage::Scrollback {
            session: "x".repeat(64),
            bytes: vec![0xE2; SCROLLBACK_CAPACITY_BYTES],
        });
        assert!(frame.len() * 2 < CLIENT_OUTBOX_LIMIT, "{}", frame.len());
    }

    #[test]
    fn test_attach_replays_buffered_output_after_confirmation() {
        let mut d = dispatcher();
        let id = d.manager.create("replay", 80, 24).unwrap();
        d.manager.record_output(&id, b"missed output\n");
        let replies = attach_replies(&d.manager, "replay", true);
        assert_eq!(replies.len(), 2);
        assert!(
            matches!(&replies[0], ServerMessage::Attached { session, .. } if session == "replay")
        );
        assert!(
            matches!(&replies[1], ServerMessage::Scrollback { session, bytes }
                if session == "replay" && bytes == b"missed output\n")
        );
        assert_eq!(
            attach_replies(&d.manager, "replay", false).len(),
            1,
            "a reattaching client already has the history"
        );
    }

    #[test]
    fn test_attach_confirmation_reports_real_session_geometry() {
        // Regression: the confirmation (and every listing) hard-coded the
        // create-time default geometry, so clients believed a 120x50 PTY
        // was 80x24.
        let mut d = dispatcher();
        d.manager.create("geo", 120, 50).unwrap();
        let replies = attach_replies(&d.manager, "geo", true);
        assert!(matches!(
            &replies[0],
            ServerMessage::Attached {
                cols: 120,
                rows: 50,
                ..
            }
        ));
    }

    #[test]
    fn test_an_attach_that_creates_its_session_persists_it() {
        // Regression: only Spawn and Kill rewrote the respawn recipes, so a
        // session created by attaching to a new name did not survive a
        // server restart.
        let mut d = dispatcher();
        let _queued = connect(&mut d, 1);
        d.handle_frame(1, Ok(attach("fresh")));
        assert!(d.manager.has("fresh"));
        assert!(d.take_sessions_changed());
        d.handle_frame(1, Ok(attach("fresh")));
        assert!(!d.take_sessions_changed(), "reattaching changes nothing");
    }

    #[test]
    fn test_a_late_exit_from_a_killed_session_spares_its_successor() {
        // Regression: the killed program's exit arrived after the name had
        // been reused, and the server killed the new session by name.
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(1, Ok(spawn("a", 80, 24, Some("sleep 30"))));
        let old = d.manager.current("a").unwrap();
        d.handle_frame(
            1,
            Ok(ClientMessage::Kill {
                session: "a".into(),
            }),
        );
        d.handle_frame(1, Ok(spawn("a", 80, 24, Some("sleep 30"))));
        drain(&queued);

        d.handle_session_event(SessionEvent::Output(SessionOutput {
            bytes: b"stale".to_vec(),
            id: old.clone(),
        }));
        d.handle_session_event(SessionEvent::Exit(SessionExit {
            code: Some(0),
            id: old,
        }));
        assert!(d.manager.has("a"), "the new session survives");
        assert!(
            drain(&queued).is_empty(),
            "nothing stale reaches the client"
        );
        assert_eq!(d.manager.scrollback("a"), Some(Vec::new()));
    }

    #[test]
    fn test_killing_a_session_detaches_its_clients() {
        // Regression: a killed session's clients stayed attached by name, so
        // a later session reusing the name took their input unasked.
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(1, Ok(attach("a")));
        drain(&queued);
        d.handle_frame(
            1,
            Ok(ClientMessage::Kill {
                session: "a".into(),
            }),
        );
        assert!(matches!(next(&queued), ServerMessage::Exit { ref session, .. } if session == "a"));

        d.manager.create("a", 80, 24).unwrap();
        d.handle_frame(
            1,
            Ok(ClientMessage::Input {
                bytes: b"injected".to_vec(),
                session: "a".into(),
            }),
        );
        assert!(matches!(next(&queued), ServerMessage::Error { .. }));
    }

    #[test]
    fn test_a_session_exit_detaches_and_is_persisted() {
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(1, Ok(attach("a")));
        drain(&queued);
        d.take_sessions_changed();
        let id = d.manager.current("a").unwrap();
        d.handle_session_event(SessionEvent::Exit(SessionExit {
            code: Some(3),
            id: id.clone(),
        }));
        assert!(matches!(
            next(&queued),
            ServerMessage::Exit { code: Some(3), .. }
        ));
        assert!(!d.manager.has("a"));
        assert!(d.take_sessions_changed());
        assert!(!d.clients[&1].is_attached(&id));
    }

    #[test]
    fn test_the_handshake_answers_a_matching_version_and_drops_another() {
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(
            1,
            Ok(ClientMessage::Hello {
                version: MUX_PROTOCOL_VERSION,
            }),
        );
        assert!(matches!(
            next(&queued),
            ServerMessage::Hello { version } if version == MUX_PROTOCOL_VERSION
        ));

        d.handle_frame(
            1,
            Ok(ClientMessage::Hello {
                version: MUX_PROTOCOL_VERSION + 1,
            }),
        );
        assert!(matches!(next(&queued), ServerMessage::Error { .. }));
        d.reap_doomed();
        assert!(d.clients.is_empty(), "a client that cannot talk is dropped");
    }

    #[test]
    fn test_an_unreadable_frame_is_answered_with_an_error() {
        // Regression: a frame that did not decode was dropped silently, so
        // a client sending a request this server does not know waited for
        // a reply that never came.
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(1, Err("unknown variant `Teleport`".into()));
        assert!(
            matches!(next(&queued), ServerMessage::Error { ref message } if message.contains("Teleport"))
        );
    }

    #[test]
    fn test_a_session_name_with_shell_syntax_is_refused() {
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(1, Ok(attach("x; rm -rf ~")));
        d.handle_frame(1, Ok(spawn("-oProxyCommand=x", 80, 24, None)));
        assert!(d.manager.session_names().is_empty());
        let replies = drain(&queued);
        assert_eq!(replies.len(), 2);
        assert!(replies
            .iter()
            .all(|msg| matches!(msg, ServerMessage::Error { .. })));
    }

    #[test]
    fn test_respawn_recreates_valid_sessions_and_skips_bad_names() {
        // Regression: a server restart discarded every `mux new` session;
        // respawning must recreate each one under its saved name/command.
        let mut d = dispatcher();
        d.respawn(vec![
            SessionDef {
                name: "work".into(),
                command: Some("sleep 5".into()),
                cwd: None,
            },
            SessionDef {
                name: "bad name".into(),
                command: None,
                cwd: None,
            },
        ]);
        assert_eq!(d.manager.session_names(), vec!["work".to_string()]);
        assert!(d.manager.session_info()[0].command.ends_with("sleep 5"));
        assert!(
            d.take_sessions_changed(),
            "the skipped recipe is dropped from the file"
        );
    }

    #[test]
    fn test_resize_arbitration_sizes_to_the_smallest_client() {
        // Regression: any attached client's resize went straight to the PTY
        // (last writer wins), so two clients at different sizes fought and
        // the loser rendered a stream wrapped for a width it didn't have,
        // with no way to learn the session's real geometry.
        let mut d = dispatcher();
        let q1 = connect(&mut d, 1);
        let q2 = connect(&mut d, 2);
        d.handle_frame(1, Ok(attach("s")));
        d.handle_frame(2, Ok(attach("s")));
        drain(&q1);
        drain(&q2);

        d.handle_frame(1, Ok(resize("s", 120, 40)));
        assert_eq!(d.manager.geometry("s"), Some((120, 40)));
        d.handle_frame(2, Ok(resize("s", 80, 24)));
        assert_eq!(d.manager.geometry("s"), Some((80, 24)), "the smallest wins");
        // The minimum is per-axis: 100x50 against 120x40 is 100x40.
        d.handle_frame(2, Ok(resize("s", 100, 50)));
        assert_eq!(d.manager.geometry("s"), Some((100, 40)));

        // Every attached client heard each arbitration result, in order.
        for queued in [&q1, &q2] {
            let sizes: Vec<(u16, u16)> = drain(queued)
                .into_iter()
                .filter_map(|msg| match msg {
                    ServerMessage::Resized { cols, rows, .. } => Some((cols, rows)),
                    _ => None,
                })
                .collect();
            assert_eq!(sizes, vec![(120, 40), (80, 24), (100, 40)]);
        }
    }

    #[test]
    fn test_detaching_or_disconnecting_lets_the_session_grow_back() {
        let mut d = dispatcher();
        let _q1 = connect(&mut d, 1);
        let q2 = connect(&mut d, 2);
        let _q3 = connect(&mut d, 3);
        for cid in [1, 2, 3] {
            d.handle_frame(cid, Ok(attach("s")));
        }
        d.handle_frame(1, Ok(resize("s", 80, 24)));
        d.handle_frame(3, Ok(resize("s", 100, 30)));
        d.handle_frame(2, Ok(resize("s", 120, 40)));
        assert_eq!(d.manager.geometry("s"), Some((80, 24)));
        drain(&q2);

        // The small client detaching releases its constraint; the remaining
        // clients learn the session grew.
        d.handle_frame(1, Ok(ClientMessage::Detach));
        assert_eq!(d.manager.geometry("s"), Some((100, 30)));
        d.remove_client(3);
        assert_eq!(d.manager.geometry("s"), Some((120, 40)));
        let sizes: Vec<(u16, u16)> = drain(&q2)
            .into_iter()
            .filter_map(|msg| match msg {
                ServerMessage::Resized { cols, rows, .. } => Some((cols, rows)),
                _ => None,
            })
            .collect();
        assert_eq!(sizes, vec![(100, 30), (120, 40)]);
    }

    #[test]
    fn test_spawn_creates_attaches_and_refuses_duplicates() {
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.handle_frame(1, Ok(spawn("dev", 100, 30, Some("sleep 5"))));

        assert_eq!(d.manager.geometry("dev"), Some((100, 30)));
        let info = d.manager.session_info();
        assert!(info[0].command.ends_with("sleep 5"), "{}", info[0].command);
        assert!(
            d.attached_session(1, "dev").is_some(),
            "the creator is attached"
        );
        assert!(matches!(
            next(&queued),
            ServerMessage::Attached {
                cols: 100,
                rows: 30,
                ..
            }
        ));

        d.handle_frame(1, Ok(spawn("dev", 80, 24, None)));
        assert!(matches!(next(&queued), ServerMessage::Error { .. }));
    }

    #[test]
    fn test_list_sessions_reports_the_real_attach_count() {
        let mut d = dispatcher();
        let q1 = connect(&mut d, 1);
        let _q2 = connect(&mut d, 2);
        d.handle_frame(1, Ok(attach("shared")));
        d.handle_frame(2, Ok(attach("shared")));
        drain(&q1);

        d.handle_frame(1, Ok(ClientMessage::ListSessions));
        let ServerMessage::SessionList { sessions } = next(&q1) else {
            panic!("expected a SessionList");
        };
        assert_eq!(sessions[0].attach_count, 2);
    }

    #[test]
    fn test_input_from_unattached_client_is_refused() {
        // Input injection is gated on attachment: a connected client that
        // never attached to the session must not be able to feed it bytes.
        let mut d = dispatcher();
        let queued = connect(&mut d, 1);
        d.manager.create("s", 80, 24).unwrap();
        d.handle_frame(
            1,
            Ok(ClientMessage::Input {
                bytes: b"injected".to_vec(),
                session: "s".into(),
            }),
        );
        assert!(!d.clients[&1].doomed);
        assert!(matches!(next(&queued), ServerMessage::Error { .. }));
    }

    #[test]
    fn test_a_client_past_its_outbox_cap_is_dropped() {
        let mut d = dispatcher();
        let _queued = connect(&mut d, 1);
        let huge = ServerMessage::Error {
            message: "x".repeat(CLIENT_OUTBOX_LIMIT),
        };
        d.reply(1, &huge);
        assert!(d.clients[&1].doomed);
        d.reap_doomed();
        assert!(d.clients.is_empty());
    }
}

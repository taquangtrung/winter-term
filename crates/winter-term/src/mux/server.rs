//! Mux server: listens on a Unix domain socket, manages PTY sessions,
//! and routes output to connected clients.
//!
//! Usage:
//!
//! ```text
//! Winter mux serve
//! Winter mux attach default
//! ```

use std::collections::HashMap;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::thread;
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use super::client::MuxClient;
use super::persist::{self, SessionDef, SessionDefs};
use super::protocol::{self, ClientMessage, FrameBuffer, ServerMessage};
use super::session::SessionManager;
use portable_pty::CommandBuilder;

// ========================================================================
// Constants
// ========================================================================

/// Geometry used when creating a session whose size the attaching client
/// has not reported yet (the client resizes right after attaching).
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// Per-client cap on queued outgoing frames. A client this far behind is
/// dropped rather than buffered without bound: the server's memory is not
/// traded for the illusion of a live connection.
const CLIENT_OUTBOX_LIMIT: usize = 4 << 20;

/// How long [`is_socket_live`] waits for a server to answer its probe before
/// declaring the socket stale. Paid once at server startup, and only when a
/// socket file is already there, so it can be generous enough to survive a
/// loaded machine without being felt in the common case.
const LIVENESS_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(500);

// ========================================================================
// Data Structures
// ========================================================================

/// Per-client send buffer. Sockets are nonblocking, so a direct write
/// fails (or returns short) whenever the peer is not keeping up; without
/// this buffer those frames were silently dropped, permanently losing
/// screen bytes. Frames queue here and flushing resumes across partial
/// writes.
struct Outbox {
    buf: Vec<u8>,
    sent: usize,
}

struct Client {
    attachments: Vec<String>,
    /// Set when the client should be detached at the end of the loop
    /// iteration: its connection broke, or it fell too far behind.
    doomed: bool,
    frames: FrameBuffer,
    outbox: Outbox,
    stream: UnixStream,
    /// Geometry the client asked for per session, the input to size
    /// arbitration: the session sizes to the smallest of these.
    wanted: HashMap<String, (u16, u16)>,
}

/// The mux server: owns every PTY session and routes output to clients.
pub struct MuxServer {
    path: String,
}

// ========================================================================
// Implementation
// ========================================================================

impl Outbox {
    fn new() -> Self {
        Outbox {
            buf: Vec::new(),
            sent: 0,
        }
    }

    /// Bytes queued but not yet accepted by the socket.
    fn pending(&self) -> usize {
        self.buf.len() - self.sent
    }

    /// Queue an encoded frame; `false` when doing so would exceed
    /// [`CLIENT_OUTBOX_LIMIT`]: the client is too slow to keep up.
    fn push(&mut self, frame: &[u8]) -> bool {
        if self.pending() + frame.len() > CLIENT_OUTBOX_LIMIT {
            return false;
        }
        self.buf.extend_from_slice(frame);
        true
    }

    /// Drop the already-sent prefix so `buf` cannot grow without bound
    /// under continuous backpressure.
    fn compact(&mut self) {
        if self.sent > 0 {
            self.buf.drain(..self.sent);
            self.sent = 0;
        }
    }

    /// Write as much as the socket currently accepts, resuming from where
    /// the last partial write stopped. `Ok` means "flushed or paused on a
    /// full socket", not necessarily "buffer empty".
    fn flush(&mut self, stream: &mut UnixStream) -> std::io::Result<()> {
        while self.sent < self.buf.len() {
            match stream.write(&self.buf[self.sent..]) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "socket accepted 0 bytes",
                    ))
                }
                Ok(n) => self.sent += n,
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    self.compact();
                    return Ok(());
                }
                Err(e) => return Err(e),
            }
        }
        self.buf.clear();
        self.sent = 0;
        Ok(())
    }
}

impl Client {
    /// Queue a message for delivery; `false` when the outbox cap was hit.
    fn enqueue(&mut self, msg: &ServerMessage) -> bool {
        let encoded = protocol::encode(msg);
        self.outbox.push(&encoded)
    }
}

impl MuxServer {
    /// A server that will listen on the socket at `path`.
    pub fn new(path: &str) -> Self {
        MuxServer {
            path: path.to_string(),
        }
    }

    /// Serve connections until the process is asked to stop.
    pub fn run(self) -> anyhow::Result<()> {
        let listener = bind_socket(&self.path)?;
        listener.set_nonblocking(true)?;

        let (output_tx, output_rx) = mpsc::channel::<ServerMessage>();
        let mut manager = SessionManager::new(output_tx);
        respawn_persisted_sessions(&mut manager);

        let mut clients: HashMap<u64, Client> = HashMap::new();
        let mut next_client_id: u64 = 1;

        loop {
            // Tracks whether this iteration did anything, so the poll sleep
            // below fires whenever there's nothing to react to, not only
            // when zero clients are connected: with a client attached but
            // idle, every read below returns `WouldBlock`, and without this
            // the loop would still spin unthrottled at 100% CPU forever.
            let mut did_work = false;

            match listener.accept() {
                Ok((stream, _)) => {
                    did_work = true;
                    stream.set_nonblocking(true).ok();
                    clients.insert(
                        next_client_id,
                        Client {
                            attachments: Vec::new(),
                            doomed: false,
                            frames: FrameBuffer::new(),
                            outbox: Outbox::new(),
                            stream,
                            wanted: HashMap::new(),
                        },
                    );
                    next_client_id += 1;
                }
                Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                Err(_) => {}
            }

            // Deliver queued frames first: replies and output enqueued last
            // iteration flush as soon as the socket accepts them.
            for client in clients.values_mut() {
                if client.doomed {
                    continue;
                }
                let before = client.outbox.pending();
                if client.outbox.flush(&mut client.stream).is_err() {
                    did_work = true;
                    client.doomed = true;
                } else if client.outbox.pending() < before {
                    did_work = true;
                }
            }

            let mut pending_messages: Vec<(u64, ClientMessage)> = Vec::new();
            for (&cid, client) in &mut clients {
                if client.doomed {
                    continue;
                }
                let mut buf = [0u8; 4096];
                match client.stream.read(&mut buf) {
                    Ok(0) => {
                        did_work = true;
                        client.doomed = true;
                        continue;
                    }
                    Ok(n) => {
                        did_work = true;
                        client.frames.extend(&buf[..n]);
                        for msg in client.frames.drain::<ClientMessage>() {
                            pending_messages.push((cid, msg));
                        }
                    }
                    Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
                    Err(_) => {
                        did_work = true;
                        client.doomed = true;
                    }
                }
            }

            let mut sessions_changed = false;
            for (cid, msg) in pending_messages {
                if matches!(
                    msg,
                    ClientMessage::Spawn { .. } | ClientMessage::Kill { .. }
                ) {
                    sessions_changed = true;
                }
                Self::handle_message(&mut manager, cid, &mut clients, msg);
            }
            if sessions_changed {
                persist_session_defs(&manager);
            }

            while let Ok(msg) = output_rx.try_recv() {
                did_work = true;
                let sessions = match &msg {
                    ServerMessage::Output { session, .. } => vec![session.clone()],
                    ServerMessage::Exit { session, .. } => vec![session.clone()],
                    _ => Vec::new(),
                };
                if let ServerMessage::Output { session, bytes } = &msg {
                    manager.record_output(session, bytes);
                }
                for client in clients.values_mut() {
                    if client.doomed {
                        continue;
                    }
                    if sessions.is_empty()
                        || client.attachments.iter().any(|s| sessions.contains(s))
                    {
                        // Queue rather than write: a slow client's socket
                        // would turn a direct write into a dropped frame.
                        if !client.enqueue(&msg) {
                            client.doomed = true;
                        }
                    }
                }
                if let ServerMessage::Exit { session, .. } = &msg {
                    manager.kill(session);
                    persist_session_defs(&manager);
                }
            }

            // A dropped client's geometry constraints vanish with it; the
            // sessions it shrank may grow back toward the remaining clients.
            let orphaned: Vec<String> = clients
                .values()
                .filter(|c| c.doomed)
                .flat_map(|c| c.wanted.keys().cloned())
                .collect();
            clients.retain(|_, client| !client.doomed);
            for session in orphaned {
                sync_session_size(&mut manager, &mut clients, &session);
            }

            if !did_work {
                thread::sleep(std::time::Duration::from_millis(10));
            }
        }
    }

    fn handle_message(
        manager: &mut SessionManager,
        cid: u64,
        clients: &mut HashMap<u64, Client>,
        msg: ClientMessage,
    ) {
        match msg {
            ClientMessage::Attach { session } => {
                if !manager.has(&session) {
                    if let Err(e) = manager.create(&session, DEFAULT_COLS, DEFAULT_ROWS) {
                        if let Some(client) = clients.get_mut(&cid) {
                            reply(
                                client,
                                &ServerMessage::Error {
                                    message: e.to_string(),
                                },
                            );
                        }
                        return;
                    }
                }
                if let Some(client) = clients.get_mut(&cid) {
                    if !client.attachments.contains(&session) {
                        client.attachments.push(session.clone());
                    }
                    for msg in attach_replies(manager, &session) {
                        reply(client, &msg);
                    }
                }
            }
            ClientMessage::Detach => {
                if let Some(client) = clients.get_mut(&cid) {
                    let affected: Vec<String> = client.wanted.keys().cloned().collect();
                    client.attachments.clear();
                    client.wanted.clear();
                    // The detached client no longer constrains these
                    // sessions; re-arbitrate so they can grow back.
                    for session in affected {
                        sync_session_size(manager, clients, &session);
                    }
                }
            }
            ClientMessage::Input { session, bytes } => {
                // Input is the injection vector: only a client that
                // attached to the session may feed it bytes. (Kill stays
                // open to unattached clients because `winter mux kill` and
                // the palette's kill command connect without attaching by
                // design.)
                if is_attached(clients, cid, &session) {
                    let _ = manager.write(&session, &bytes);
                } else if let Some(client) = clients.get_mut(&cid) {
                    reply(
                        client,
                        &ServerMessage::Error {
                            message: format!("not attached to session '{session}'"),
                        },
                    );
                }
            }
            ClientMessage::Resize {
                session,
                cols,
                rows,
            } => {
                if is_attached(clients, cid, &session) {
                    // Record the client's wish, then arbitrate: the session
                    // sizes to the smallest attached geometry, and every
                    // attached client learns the result (see
                    // `sync_session_size`).
                    if let Some(client) = clients.get_mut(&cid) {
                        client.wanted.insert(session.clone(), (cols, rows));
                    }
                    sync_session_size(manager, clients, &session);
                } else if let Some(client) = clients.get_mut(&cid) {
                    reply(
                        client,
                        &ServerMessage::Error {
                            message: format!("not attached to session '{session}'"),
                        },
                    );
                }
            }
            ClientMessage::Spawn {
                session,
                cols,
                rows,
                cwd,
                command,
            } => {
                if manager.has(&session) {
                    if let Some(client) = clients.get_mut(&cid) {
                        reply(
                            client,
                            &ServerMessage::Error {
                                message: format!("session '{session}' already exists"),
                            },
                        );
                    }
                    return;
                }
                let builder = build_spawn_command(command.as_deref(), cwd.as_deref());
                if let Err(e) = manager.create_with(
                    &session,
                    cols,
                    rows,
                    builder,
                    command.as_deref(),
                    cwd.as_deref(),
                ) {
                    if let Some(client) = clients.get_mut(&cid) {
                        reply(
                            client,
                            &ServerMessage::Error {
                                message: e.to_string(),
                            },
                        );
                    }
                    return;
                }
                if let Some(client) = clients.get_mut(&cid) {
                    if !client.attachments.contains(&session) {
                        client.attachments.push(session.clone());
                    }
                    client.wanted.insert(session.clone(), (cols, rows));
                    for msg in attach_replies(manager, &session) {
                        reply(client, &msg);
                    }
                }
                // Another attached client may constrain the new session
                // smaller than its spawn geometry.
                sync_session_size(manager, clients, &session);
            }
            ClientMessage::ListSessions => {
                let mut sessions = manager.session_info();
                for info in &mut sessions {
                    info.attach_count = attach_count(clients, &info.name);
                }
                let msg = ServerMessage::SessionList { sessions };
                if let Some(client) = clients.get_mut(&cid) {
                    reply(client, &msg);
                }
            }
            ClientMessage::Kill { session } => {
                manager.kill(&session);
            }
        }
    }
}

/// Queue a message for delivery to one client, marking it doomed when it
/// has fallen too far behind to keep buffering.
fn reply(client: &mut Client, msg: &ServerMessage) {
    if !client.enqueue(msg) {
        client.doomed = true;
    }
}

fn is_attached(clients: &HashMap<u64, Client>, cid: u64, session: &str) -> bool {
    clients
        .get(&cid)
        .is_some_and(|c| c.attachments.iter().any(|s| s == session))
}

/// How many clients are currently attached to `session`, for listings.
fn attach_count(clients: &HashMap<u64, Client>, session: &str) -> usize {
    clients
        .values()
        .filter(|c| c.attachments.iter().any(|s| s == session))
        .count()
}

/// The smallest geometry among the clients that reported one for `session`.
/// Clients that attached but never resized are unconstrained: they don't
/// shrink the session, and `None` means no client constrains it, so the
/// session keeps its current size (the pre-arbitration behavior for
/// attach-only clients like the CLI).
fn effective_size(clients: &HashMap<u64, Client>, session: &str) -> Option<(u16, u16)> {
    let mut size: Option<(u16, u16)> = None;
    for client in clients.values() {
        let Some(&(cols, rows)) = client.wanted.get(session) else {
            continue;
        };
        size = Some(match size {
            None => (cols, rows),
            Some((c, r)) => (c.min(cols), r.min(rows)),
        });
    }
    size
}

/// Re-arbitrate `session`: when the effective size changed, resize the PTY
/// and tell every attached client the session's real geometry, so a pane
/// whose layout is larger than the session letterboxes its grid instead of
/// rendering a stream wrapped for a width it doesn't have.
fn sync_session_size(
    manager: &mut SessionManager,
    clients: &mut HashMap<u64, Client>,
    session: &str,
) {
    let Some((cols, rows)) = effective_size(clients, session) else {
        return;
    };
    if manager.geometry(session) == Some((cols, rows)) {
        return;
    }
    if manager.resize(session, cols, rows).is_err() {
        return;
    }
    let msg = ServerMessage::Resized {
        session: session.to_string(),
        cols,
        rows,
    };
    for client in clients.values_mut() {
        if client.doomed {
            continue;
        }
        if client.attachments.iter().any(|s| s == session) && !client.enqueue(&msg) {
            client.doomed = true;
        }
    }
}

/// The command a Spawn runs: the given command line through the user's
/// shell (so it can carry arguments, pipes, and env), or the default shell
/// when none was given; `cwd` overrides the working directory.
fn build_spawn_command(command: Option<&str>, cwd: Option<&str>) -> CommandBuilder {
    let mut builder = match command {
        Some(cmd) if !cmd.trim().is_empty() => {
            let shell = std::env::var("SHELL").unwrap_or_else(|_| "sh".to_string());
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

/// Write every live session's respawn recipe to the defs file, dropping
/// whichever ones no longer exist (killed explicitly, or exited on
/// their own) so a restart does not bring them back.
fn persist_session_defs(manager: &SessionManager) {
    persist::save_defs(&SessionDefs {
        sessions: manager.session_defs(),
    });
}

/// Recreate every session from the last persisted defs file, so `mux new`
/// sessions survive a server restart.
fn respawn_persisted_sessions(manager: &mut SessionManager) {
    respawn_defs(manager, persist::load_defs().sessions);
}

/// Recreate `defs` in `manager` (scrollback does not survive: each respawn
/// is a brand-new PTY). A def whose command can no longer spawn is logged
/// and dropped rather than aborting the rest of the batch.
fn respawn_defs(manager: &mut SessionManager, defs: Vec<SessionDef>) {
    for def in defs {
        let builder = build_spawn_command(def.command.as_deref(), def.cwd.as_deref());
        if let Err(e) = manager.create_with(
            &def.name,
            DEFAULT_COLS,
            DEFAULT_ROWS,
            builder,
            def.command.as_deref(),
            def.cwd.as_deref(),
        ) {
            eprintln!("winter mux: failed to respawn '{}': {e}", def.name);
        }
    }
}

/// Bind the server socket at `path`, replacing any stale socket file a
/// crashed server left behind, and restrict it to this user: the fallback
/// location when `$XDG_RUNTIME_DIR` is unset is `/tmp`, where without the
/// restriction any local user could attach to (and inject input into)
/// every session.
///
/// Fails rather than displacing a server that is still serving. A Unix socket
/// path can be unlinked out from under a live listener, so unlinking
/// unconditionally let a second `winter mux serve` silently take over the path
/// and strand the first server's sessions: still running, still holding their
/// PTYs, but unreachable by any client.
pub fn bind_socket(path: &str) -> anyhow::Result<UnixListener> {
    // Bind first, and only investigate if the path is taken. A plain
    // `bind` is atomic and needs no liveness guess, so the ordinary case
    // (nothing there) never consults the probe at all.
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(e) if e.kind() == std::io::ErrorKind::AddrInUse => {
            if is_socket_live(path) {
                anyhow::bail!("a winter mux server is already listening on {path}");
            }
            // Nothing is answering, so the file is a crashed server's
            // leftover: clear it and take the path.
            std::fs::remove_file(path)?;
            UnixListener::bind(path)?
        }
        Err(e) => return Err(e.into()),
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(listener)
}

/// The messages a client receives right after attaching: confirmation,
/// then the session's buffered output so a fresh client rebuilds its
/// screen and scrollback before live output resumes.
fn attach_replies(manager: &SessionManager, session: &str) -> Vec<ServerMessage> {
    let (cols, rows) = manager
        .geometry(session)
        .unwrap_or((DEFAULT_COLS, DEFAULT_ROWS));
    let mut replies = vec![ServerMessage::Attached {
        session: session.to_string(),
        cols,
        rows,
    }];
    if let Some(bytes) = manager.scrollback(session) {
        if !bytes.is_empty() {
            replies.push(ServerMessage::Scrollback {
                session: session.to_string(),
                bytes,
            });
        }
    }
    replies
}

impl Drop for MuxServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Whether a server is currently *serving* on `path`.
///
/// The file existing proves nothing: a crashed server leaves its socket
/// behind. Connecting proves almost nothing either, because a connect to a
/// socket whose listener has just closed can still succeed while the kernel
/// tears the endpoint down; probing with a bare connect made this flap, which
/// would turn a legitimate restart after a crash into "already listening".
///
/// So liveness is confirmed by an actual protocol exchange: ask for the
/// session list and require a reply. Only a running server answers.
fn is_socket_live(path: &str) -> bool {
    if !std::path::Path::new(path).exists() {
        return false;
    }
    match MuxClient::connect(path) {
        Ok(mut client) => client.query_sessions(LIVENESS_PROBE_TIMEOUT).is_ok(),
        Err(_) => false,
    }
}

/// The socket path used when the user names no other.
pub fn default_socket_path() -> String {
    match crate::paths::runtime_dir() {
        Some(dir) => format!("{dir}/winter-mux.sock"),
        None => "/tmp/winter-mux.sock".to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bind_refuses_to_displace_a_live_server() {
        // Regression: `bind_socket` unlinked the path unconditionally, so a
        // second `winter mux serve` stole it and stranded the first server's
        // sessions: still running, still holding their PTYs, unreachable.
        let path = std::env::temp_dir()
            .join(format!("winter-mux-bind-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);

        let first = bind_socket(&path).expect("first bind succeeds");
        // Liveness is confirmed by a protocol reply, not a bare connect, so
        // the stand-in server has to actually answer one.
        let responder = std::thread::spawn(move || {
            if let Ok((mut conn, _)) = first.accept() {
                let mut buf = [0u8; 1024];
                let _ = conn.read(&mut buf);
                let reply = protocol::encode(&ServerMessage::SessionList {
                    sessions: Vec::new(),
                });
                let _ = conn.write_all(&reply);
            }
        });

        let displaced = bind_socket(&path);
        assert!(
            displaced.is_err(),
            "a second bind must not displace a serving mux"
        );

        responder.join().expect("responder thread panicked");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_bind_replaces_a_stale_socket_file() {
        // The crashed-server case the unconditional remove existed to handle
        // must keep working: nothing is listening, so the path is free.
        let path = std::env::temp_dir()
            .join(format!("winter-mux-stale-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);

        drop(bind_socket(&path).expect("first bind succeeds"));
        assert!(
            std::path::Path::new(&path).exists(),
            "dropping the listener leaves the socket file behind"
        );
        if let Err(e) = bind_socket(&path) {
            panic!("a stale socket file must not block a fresh server: {e}");
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_default_socket_path_is_deterministic() {
        let p1 = default_socket_path();
        let p2 = default_socket_path();
        assert_eq!(p1, p2);
    }

    #[test]
    fn test_server_new_creates_instance() {
        let server = MuxServer::new("/tmp/test-winter-mux.sock");
        assert_eq!(server.path, "/tmp/test-winter-mux.sock");
    }

    #[test]
    fn test_respawn_defs_recreates_sessions_with_their_recipe() {
        // Regression: a server restart discarded every `mux new` session;
        // respawning must recreate each one under its saved name/command.
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        respawn_defs(
            &mut manager,
            vec![SessionDef {
                name: "work".into(),
                command: Some("sleep 5".into()),
                cwd: None,
            }],
        );
        assert!(manager.has("work"));
        assert!(manager.session_info()[0].command.ends_with("sleep 5"));
    }

    #[test]
    fn test_attach_replays_buffered_output_after_confirmation() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        manager.create("replay", 80, 24).unwrap();
        manager.record_output("replay", b"missed output\n");
        let replies = attach_replies(&manager, "replay");
        assert_eq!(replies.len(), 2);
        assert!(
            matches!(&replies[0], ServerMessage::Attached { session, .. } if session == "replay")
        );
        assert!(
            matches!(&replies[1], ServerMessage::Scrollback { session, bytes }
                if session == "replay" && bytes == b"missed output\n")
        );
    }

    #[test]
    fn test_attach_without_buffered_output_confirms_only() {
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        manager.create("fresh", 80, 24).unwrap();
        let replies = attach_replies(&manager, "fresh");
        assert_eq!(replies.len(), 1);
        assert!(matches!(&replies[0], ServerMessage::Attached { .. }));
    }

    #[test]
    fn test_attach_confirmation_reports_real_session_geometry() {
        // Regression: the confirmation (and every listing) hard-coded the
        // create-time default geometry, so clients believed a 120x50 PTY
        // was 80x24.
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        manager.create("geo", 120, 50).unwrap();
        let replies = attach_replies(&manager, "geo");
        assert!(matches!(
            &replies[0],
            ServerMessage::Attached {
                cols: 120,
                rows: 50,
                ..
            }
        ));
        manager.resize("geo", 132, 43).unwrap();
        let replies = attach_replies(&manager, "geo");
        assert!(matches!(
            &replies[0],
            ServerMessage::Attached {
                cols: 132,
                rows: 43,
                ..
            }
        ));
    }

    #[test]
    fn test_outbox_queues_and_delivers_frames_in_order() {
        let path = std::env::temp_dir()
            .join(format!("winter-mux-outbox-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let mut peer = UnixStream::connect(&path).unwrap();
        let (mut stream, _) = listener.accept().unwrap();

        let mut outbox = Outbox::new();
        assert!(outbox.push(&protocol::encode(&ServerMessage::Error {
            message: "first".into()
        })));
        assert!(outbox.push(&protocol::encode(&ServerMessage::Error {
            message: "second".into()
        })));
        assert!(outbox.pending() > 0);

        outbox.flush(&mut stream).unwrap();
        assert_eq!(outbox.pending(), 0);

        for expected in ["first", "second"] {
            let mut len_bytes = [0u8; 4];
            peer.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            peer.read_exact(&mut body).unwrap();
            framed.extend(body);
            let msg: ServerMessage = protocol::decode(&framed).unwrap();
            match msg {
                ServerMessage::Error { message } => assert_eq!(message, expected),
                other => panic!("wrong message: {other:?}"),
            }
        }

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_outbox_refuses_frames_beyond_the_cap() {
        let mut outbox = Outbox::new();
        let huge = vec![0u8; CLIENT_OUTBOX_LIMIT + 1];
        assert!(
            !outbox.push(&huge),
            "a frame that overflows the cap must be refused, not buffered"
        );
        assert_eq!(outbox.pending(), 0);
    }

    #[cfg(unix)]
    #[test]
    fn test_bind_socket_restricts_permissions_to_the_owner() {
        use std::os::unix::fs::PermissionsExt;
        let path = std::env::temp_dir()
            .join(format!("winter-mux-bind-test-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _listener = bind_socket(&path).unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_file(&path);
    }

    /// A `Client` wired to a readable peer socket, with `wanted` geometry
    /// entries preloaded. Returns `(client, peer, socket_path)`.
    fn connected_client(tag: &str, wanted: &[(&str, u16, u16)]) -> (Client, UnixStream, String) {
        let path = std::env::temp_dir()
            .join(format!("winter-mux-{tag}-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let peer = UnixStream::connect(&path).unwrap();
        let stream = listener.accept().unwrap().0;
        drop(listener);
        (
            Client {
                attachments: Vec::new(),
                doomed: false,
                frames: FrameBuffer::new(),
                outbox: Outbox::new(),
                stream,
                wanted: wanted
                    .iter()
                    .map(|(s, c, r)| (s.to_string(), (*c, *r)))
                    .collect(),
            },
            peer,
            path,
        )
    }

    /// Flush `client`'s outbox and read one decoded frame from its peer.
    fn delivered_frame(client: &mut Client, peer: &mut UnixStream) -> ServerMessage {
        client.outbox.flush(&mut client.stream).unwrap();
        let mut len_bytes = [0u8; 4];
        peer.read_exact(&mut len_bytes).unwrap();
        let mut framed = len_bytes.to_vec();
        let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
        peer.read_exact(&mut body).unwrap();
        framed.extend(body);
        protocol::decode(&framed).unwrap()
    }

    #[test]
    fn test_effective_size_ignores_unconstrained_clients() {
        let (c1, _p1, path1) = connected_client("eff1", &[("s", 120, 40)]);
        let (c2, _p2, path2) = connected_client("eff2", &[]); // attached, never resized
        let (c3, _p3, path3) = connected_client("eff3", &[("s", 80, 24), ("t", 50, 10)]);
        let mut clients = HashMap::new();
        clients.insert(1u64, c1);
        clients.insert(2u64, c2);
        clients.insert(3u64, c3);

        assert_eq!(effective_size(&clients, "s"), Some((80, 24)));
        assert_eq!(effective_size(&clients, "t"), Some((50, 10)));
        assert_eq!(effective_size(&clients, "missing"), None);

        for p in [path1, path2, path3] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn test_resize_arbitration_sizes_to_the_smallest_client() {
        // Regression: any attached client's resize went straight to the PTY
        // (last writer wins), so two clients at different sizes fought and
        // the loser rendered a stream wrapped for a width it didn't have,
        // with no way to learn the session's real geometry.
        let (c1, mut p1, path1) = connected_client("arb1", &[]);
        let (c2, mut p2, path2) = connected_client("arb2", &[]);
        let mut clients = HashMap::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        manager.create("s", 80, 24).unwrap();
        {
            let c1 = Client {
                attachments: vec!["s".into()],
                ..c1
            };
            let c2 = Client {
                attachments: vec!["s".into()],
                ..c2
            };
            clients.insert(1u64, c1);
            clients.insert(2u64, c2);
        }

        MuxServer::handle_message(
            &mut manager,
            1,
            &mut clients,
            ClientMessage::Resize {
                session: "s".into(),
                cols: 120,
                rows: 40,
            },
        );
        assert_eq!(manager.geometry("s"), Some((120, 40)));

        MuxServer::handle_message(
            &mut manager,
            2,
            &mut clients,
            ClientMessage::Resize {
                session: "s".into(),
                cols: 80,
                rows: 24,
            },
        );
        assert_eq!(
            manager.geometry("s"),
            Some((80, 24)),
            "the smallest client wins"
        );

        // The minimum is per-axis: 100x50 against 120x40 is 100x40.
        MuxServer::handle_message(
            &mut manager,
            2,
            &mut clients,
            ClientMessage::Resize {
                session: "s".into(),
                cols: 100,
                rows: 50,
            },
        );
        assert_eq!(manager.geometry("s"), Some((100, 40)));

        // Every attached client heard each arbitration result, in order.
        for expected in [(120, 40), (80, 24), (100, 40)] {
            let msg = delivered_frame(clients.get_mut(&1).unwrap(), &mut p1);
            assert!(
                matches!(msg, ServerMessage::Resized { cols, rows, .. } if (cols, rows) == expected)
            );
        }
        for expected in [(120, 40), (80, 24), (100, 40)] {
            let msg = delivered_frame(clients.get_mut(&2).unwrap(), &mut p2);
            assert!(
                matches!(msg, ServerMessage::Resized { cols, rows, .. } if (cols, rows) == expected)
            );
        }

        for p in [path1, path2] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn test_detaching_lets_the_session_grow_back() {
        let (c1, _p1, path1) = connected_client("det1", &[]);
        let (c2, mut p2, path2) = connected_client("det2", &[]);
        let mut clients = HashMap::new();
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        manager.create("s", 80, 24).unwrap();
        clients.insert(
            1u64,
            Client {
                attachments: vec!["s".into()],
                ..c1
            },
        );
        clients.insert(
            2u64,
            Client {
                attachments: vec!["s".into()],
                ..c2
            },
        );

        MuxServer::handle_message(
            &mut manager,
            1,
            &mut clients,
            ClientMessage::Resize {
                session: "s".into(),
                cols: 80,
                rows: 24,
            },
        );
        MuxServer::handle_message(
            &mut manager,
            2,
            &mut clients,
            ClientMessage::Resize {
                session: "s".into(),
                cols: 120,
                rows: 40,
            },
        );
        assert_eq!(manager.geometry("s"), Some((80, 24)));

        // The small client detaching releases its constraint; the remaining
        // client learns the session grew.
        MuxServer::handle_message(&mut manager, 1, &mut clients, ClientMessage::Detach);
        assert_eq!(manager.geometry("s"), Some((120, 40)));
        let msg = delivered_frame(clients.get_mut(&2).unwrap(), &mut p2);
        assert!(matches!(
            msg,
            ServerMessage::Resized {
                cols: 120,
                rows: 40,
                ..
            }
        ));

        for p in [path1, path2] {
            let _ = std::fs::remove_file(p);
        }
    }

    #[test]
    fn test_spawn_creates_attaches_and_refuses_duplicates() {
        let (c1, mut p1, path) = connected_client("spawn", &[]);
        let mut clients = HashMap::new();
        clients.insert(1u64, c1);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);

        MuxServer::handle_message(
            &mut manager,
            1,
            &mut clients,
            ClientMessage::Spawn {
                session: "dev".into(),
                cols: 100,
                rows: 30,
                cwd: None,
                command: Some("sleep 5".into()),
            },
        );

        assert!(manager.has("dev"));
        assert_eq!(manager.geometry("dev"), Some((100, 30)));
        let info = manager.session_info();
        assert!(info[0].command.ends_with("sleep 5"), "{}", info[0].command);
        assert!(is_attached(&clients, 1, "dev"), "the creator is attached");
        let msg = delivered_frame(clients.get_mut(&1).unwrap(), &mut p1);
        assert!(matches!(
            msg,
            ServerMessage::Attached {
                cols: 100,
                rows: 30,
                ..
            }
        ));

        // Spawning over an existing name is an error, not a clobber.
        MuxServer::handle_message(
            &mut manager,
            1,
            &mut clients,
            ClientMessage::Spawn {
                session: "dev".into(),
                cols: 80,
                rows: 24,
                cwd: None,
                command: None,
            },
        );
        let msg = delivered_frame(clients.get_mut(&1).unwrap(), &mut p1);
        assert!(matches!(msg, ServerMessage::Error { .. }));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_list_sessions_reports_the_real_attach_count() {
        let (c1, mut p1, path1) = connected_client("list1", &[]);
        let (c2, _p2, path2) = connected_client("list2", &[]);
        let mut clients = HashMap::new();
        clients.insert(1u64, c1);
        clients.insert(2u64, c2);
        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);

        for cid in [1u64, 2] {
            MuxServer::handle_message(
                &mut manager,
                cid,
                &mut clients,
                ClientMessage::Attach {
                    session: "shared".into(),
                },
            );
        }
        // Drain each client's attach confirmation before the ListSessions
        // reply, or the assertion below would decode the wrong frame.
        delivered_frame(clients.get_mut(&1).unwrap(), &mut p1);

        MuxServer::handle_message(&mut manager, 1, &mut clients, ClientMessage::ListSessions);
        let msg = delivered_frame(clients.get_mut(&1).unwrap(), &mut p1);
        let ServerMessage::SessionList { sessions } = msg else {
            panic!("expected a SessionList, got {msg:?}");
        };
        assert_eq!(sessions[0].attach_count, 2);

        for path in [path1, path2] {
            let _ = std::fs::remove_file(path);
        }
    }

    #[test]
    fn test_input_from_unattached_client_is_refused() {
        // Input injection is gated on attachment: a connected client that
        // never attached to the session must not be able to feed it bytes.
        let path = std::env::temp_dir()
            .join(format!("winter-mux-gate-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let mut peer = UnixStream::connect(&path).unwrap();
        let (stream, _) = listener.accept().unwrap();

        let (tx, _rx) = std::sync::mpsc::channel();
        let mut manager = SessionManager::new(tx);
        let mut clients = HashMap::new();
        clients.insert(
            1u64,
            Client {
                attachments: Vec::new(),
                doomed: false,
                frames: FrameBuffer::new(),
                outbox: Outbox::new(),
                stream,
                wanted: HashMap::new(),
            },
        );

        MuxServer::handle_message(
            &mut manager,
            1,
            &mut clients,
            ClientMessage::Input {
                session: "s".into(),
                bytes: b"injected".to_vec(),
            },
        );

        let client = clients.get_mut(&1).unwrap();
        assert!(!client.doomed);
        client.outbox.flush(&mut client.stream).unwrap();
        drop(clients);

        // The refusal arrives as an Error frame rather than silence.
        let mut len_bytes = [0u8; 4];
        peer.read_exact(&mut len_bytes).unwrap();
        let mut framed = len_bytes.to_vec();
        let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
        peer.read_exact(&mut body).unwrap();
        framed.extend(body);
        let msg: ServerMessage = protocol::decode(&framed).unwrap();
        assert!(matches!(msg, ServerMessage::Error { .. }));

        let _ = std::fs::remove_file(&path);
    }
}

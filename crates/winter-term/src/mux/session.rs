//! Session manager: owns PTY children, reads their output, and routes I/O.
//!
//! Every session is identified by a [`SessionId`]: its name plus the
//! generation it was created in. A name can be killed and reused while the
//! old program's reader thread is still winding down, so everything that
//! thread reports carries the id it was started under, and a report for a
//! generation that is no longer current is the old program's, not the new.

use std::collections::HashMap;
use std::io::Read;
use std::sync::{mpsc, Arc};
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use super::persist::SessionDef;
use super::protocol::{CellPixels, Geometry, SessionInfo};
use crate::terminal::session::spawn_pty_writer;

// ========================================================================
// Constants
// ========================================================================

/// Retained PTY output per session, bounded so a long-lived server's
/// memory scales with session count rather than uptime; ~1 MiB covers a
/// typical session's useful history.
pub(super) const SCROLLBACK_CAPACITY_BYTES: usize = 1 << 20;

/// How much PTY output one read takes.
const READ_CHUNK_BYTES: usize = 4096;

/// The listing label of a session started without a command of its own.
const DEFAULT_SHELL_LABEL: &str = "(default shell)";

// ========================================================================
// Data Structures
// ========================================================================

/// Where a session's reader thread reports what its program did.
pub type EventSink = Arc<dyn Fn(SessionEvent) + Send + Sync>;

/// Owns every live PTY session and publishes what they write.
pub struct SessionManager {
    /// The generation the next session is created in.
    next_generation: u64,
    sessions: HashMap<String, Session>,
    sink: EventSink,
}

/// One incarnation of a named session.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct SessionId {
    /// Distinguishes this incarnation from earlier ones under the same name.
    pub generation: u64,
    /// The session's name.
    pub name: String,
}

/// What a session's program did, as its reader thread reports it.
#[derive(Debug)]
pub enum SessionEvent {
    /// The program exited.
    Exit(SessionExit),
    /// The program wrote output.
    Output(SessionOutput),
}

/// A session's program exiting.
#[derive(Debug)]
pub struct SessionExit {
    /// Exit status, absent when the process was signalled.
    pub code: Option<i32>,
    /// The session it ran in.
    pub id: SessionId,
}

/// A batch of a session's output.
#[derive(Debug)]
pub struct SessionOutput {
    /// The bytes read from the PTY.
    pub bytes: Vec<u8>,
    /// The session that wrote them.
    pub id: SessionId,
}

struct Session {
    /// The cell size in pixels the PTY was last sized with, if any client
    /// reported one.
    cell_px: Option<CellPixels>,
    /// The command line the session runs, for listings.
    command: String,
    /// Current PTY geometry, tracked so listings and attach confirmations
    /// report reality rather than the create-time default.
    cols: u16,
    /// Creation time, Unix-epoch seconds.
    created: u64,
    /// Working directory the session was spawned in, for restart survival.
    cwd: Option<String>,
    /// Which incarnation of the name this is.
    generation: u64,
    history: ScrollbackRing,
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Unwrapped command line the session runs, for restart survival, as
    /// given by the caller, before shell-wrapping, unlike `command`.
    raw_command: Option<String>,
    rows: u16,
    /// Signals the session's program; dropping the PTY alone does not end it,
    /// since the reader thread holds a copy of the master.
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
    /// Input for the program, written by a thread of its own (see
    /// [`spawn_pty_writer`]): the server has one thread for every client and
    /// session, and one program not reading must not stall them all.
    writer: mpsc::Sender<Vec<u8>>,
}

/// Bounded append-only log of a session's PTY output, kept so clients
/// attaching later can replay what they missed.
struct ScrollbackRing {
    buf: Vec<u8>,
    capacity: usize,
    /// Set once the head has been evicted: the buffer then starts
    /// mid-stream rather than at the session's first byte.
    evicted: bool,
}

// ========================================================================
// SessionManager
// ========================================================================

impl SessionManager {
    /// A manager that reports session output and exits to `sink`.
    pub fn new(sink: EventSink) -> Self {
        SessionManager {
            next_generation: 1,
            sessions: HashMap::new(),
            sink,
        }
    }

    /// Spawn a session under this name running the default shell; an
    /// existing name is left as it is and its id returned.
    pub fn create(&mut self, name: &str, cols: u16, rows: u16) -> anyhow::Result<SessionId> {
        self.create_with(
            name,
            cols,
            rows,
            CommandBuilder::new_default_prog(),
            None,
            None,
        )
    }

    /// Like [`create`](Self::create), but the session runs `command` instead
    /// of the default shell. `raw_command`/`cwd` are the unwrapped strings
    /// behind `command`, retained for restart respawn recipes.
    pub fn create_with(
        &mut self,
        name: &str,
        cols: u16,
        rows: u16,
        command: CommandBuilder,
        raw_command: Option<&str>,
        cwd: Option<&str>,
    ) -> anyhow::Result<SessionId> {
        if let Some(id) = self.current(name) {
            return Ok(id);
        }

        let command_label = command
            .get_argv()
            .iter()
            .map(|a| a.to_string_lossy())
            .collect::<Vec<_>>()
            .join(" ");

        let pty_system = NativePtySystem::default();
        let pair = pty_system.openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })?;

        let child = pair.slave.spawn_command(command)?;
        let killer = child.clone_killer();

        let reader = pair.master.try_clone_reader()?;
        let writer = spawn_pty_writer(pair.master.take_writer()?)?;
        let id = SessionId {
            generation: self.next_generation,
            name: name.to_string(),
        };
        let sink = Arc::clone(&self.sink);
        let reader_id = id.clone();
        thread::spawn(move || pump_session_output(reader, child, reader_id, sink));

        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.sessions.insert(
            name.to_string(),
            Session {
                cell_px: None,
                command: if command_label.is_empty() {
                    DEFAULT_SHELL_LABEL.to_string()
                } else {
                    command_label
                },
                cols,
                created,
                cwd: cwd.map(str::to_string),
                generation: id.generation,
                history: ScrollbackRing::new(SCROLLBACK_CAPACITY_BYTES),
                master: pair.master,
                raw_command: raw_command.map(str::to_string),
                rows,
                killer,
                writer,
            },
        );
        self.next_generation += 1;
        Ok(id)
    }

    /// The id of the live session under `name`, if any.
    pub fn current(&self, name: &str) -> Option<SessionId> {
        self.sessions.get(name).map(|session| SessionId {
            generation: session.generation,
            name: name.to_string(),
        })
    }

    /// Whether `id` is the live incarnation of its name: `false` for one
    /// killed or exited, even when the name has been reused since.
    pub fn is_current(&self, id: &SessionId) -> bool {
        self.sessions
            .get(&id.name)
            .is_some_and(|session| session.generation == id.generation)
    }

    /// Forward bytes to a session's PTY.
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(session) = self.sessions.get_mut(name) {
            session
                .writer
                .send(bytes.to_vec())
                .map_err(|_| anyhow::anyhow!("session {name} no longer takes input"))?;
        }
        Ok(())
    }

    /// Appends live PTY output to the session's retained history so it
    /// can be replayed to clients that attach later. Output from an earlier
    /// incarnation of the name is not this session's and is dropped.
    pub fn record_output(&mut self, id: &SessionId, bytes: &[u8]) {
        if let Some(session) = self.sessions.get_mut(&id.name) {
            if session.generation == id.generation {
                session.history.push(bytes);
            }
        }
    }

    /// The session's retained output, oldest first, for replay on attach;
    /// `None` when the session does not exist.
    pub fn scrollback(&self, name: &str) -> Option<Vec<u8>> {
        self.sessions
            .get(name)
            .map(|s| s.history.snapshot().to_vec())
    }

    /// Set a session's PTY geometry.
    pub fn resize(&mut self, name: &str, geometry: Geometry) -> anyhow::Result<()> {
        let Geometry {
            cell_px,
            cols,
            rows,
        } = geometry;
        if let Some(session) = self.sessions.get_mut(name) {
            session.master.resize(PtySize {
                rows,
                cols,
                pixel_width: cell_px.map_or(0, |cell| cell.pixel_width(cols)),
                pixel_height: cell_px.map_or(0, |cell| cell.pixel_height(rows)),
            })?;
            session.cell_px = cell_px;
            session.cols = cols;
            session.rows = rows;
        }
        Ok(())
    }

    /// The cell size in pixels the session's PTY was last sized with, or
    /// `None` when no client reported one (or the session does not exist).
    pub fn cell_px(&self, name: &str) -> Option<CellPixels> {
        self.sessions.get(name).and_then(|s| s.cell_px)
    }

    /// The session's current PTY geometry, for attach confirmations.
    /// `None` when the session does not exist.
    pub fn geometry(&self, name: &str) -> Option<(u16, u16)> {
        self.sessions.get(name).map(|s| (s.cols, s.rows))
    }

    /// Real geometry and creation time per session, sorted by name so
    /// listings are stable across queries (HashMap iteration is not).
    pub fn session_info(&self) -> Vec<SessionInfo> {
        let mut info: Vec<SessionInfo> = self
            .sessions
            .iter()
            .map(|(name, s)| SessionInfo {
                // The manager has no notion of attached clients; the
                // server fills this in from its own client registry.
                attach_count: 0,
                name: name.clone(),
                cols: s.cols,
                rows: s.rows,
                created: s.created,
                command: s.command.clone(),
            })
            .collect();
        info.sort_by(|a, b| a.name.cmp(&b.name));
        info
    }

    /// Respawn recipe per session, sorted by name for the same stability
    /// reason as [`session_info`](Self::session_info).
    pub fn session_defs(&self) -> Vec<SessionDef> {
        let mut defs: Vec<SessionDef> = self
            .sessions
            .iter()
            .map(|(name, s)| SessionDef {
                name: name.clone(),
                command: s.raw_command.clone(),
                cwd: s.cwd.clone(),
            })
            .collect();
        defs.sort_by(|a, b| a.name.cmp(&b.name));
        defs
    }

    /// Terminate a session: its program is signalled (dropping the PTY alone
    /// leaves it running, unseen, behind the reader's copy of the master),
    /// and the session is dropped. The reader thread reaps the program when
    /// its output ends. Returns the id the session had, `None` when there
    /// was none by that name.
    pub fn kill(&mut self, name: &str) -> Option<SessionId> {
        let id = self.current(name)?;
        if let Some(mut session) = self.sessions.remove(name) {
            let _ = session.killer.kill();
        }
        Some(id)
    }

    /// Forget a session whose program has exited. `false`, and nothing
    /// done, when `id` is not the live incarnation of its name.
    pub fn remove(&mut self, id: &SessionId) -> bool {
        if !self.is_current(id) {
            return false;
        }
        self.sessions.remove(&id.name);
        true
    }

    /// The names of every live session.
    pub fn session_names(&self) -> Vec<String> {
        self.sessions.keys().cloned().collect()
    }

    /// Whether a session by this name is live.
    pub fn has(&self, name: &str) -> bool {
        self.sessions.contains_key(name)
    }
}

impl Drop for SessionManager {
    /// End every program: the reader threads hold copies of the masters, so
    /// dropping the PTYs alone would leave them running with no one to see.
    fn drop(&mut self) {
        for session in self.sessions.values_mut() {
            let _ = session.killer.kill();
        }
    }
}

// ========================================================================
// ScrollbackRing
// ========================================================================

impl ScrollbackRing {
    fn new(capacity: usize) -> Self {
        ScrollbackRing {
            buf: Vec::new(),
            capacity,
            evicted: false,
        }
    }

    /// Appends output, evicting the oldest bytes once capacity is exceeded.
    fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
        if self.buf.len() > self.capacity {
            let excess = self.buf.len() - self.capacity;
            self.buf.drain(..excess);
            self.evicted = true;
        }
    }

    /// The bytes safe to replay to a fresh client: after eviction the
    /// head usually starts mid-line (or mid-escape-sequence), so replay
    /// begins after the first newline instead. With no newline at all the
    /// whole buffer is returned: a torn head is less harmful than an
    /// empty replay.
    fn snapshot(&self) -> &[u8] {
        if self.evicted {
            if let Some(nl) = self.buf.iter().position(|&b| b == b'\n') {
                return &self.buf[nl + 1..];
            }
        }
        &self.buf
    }
}

// ========================================================================
// Reader thread
// ========================================================================

/// Report a session's output to `sink` until its PTY closes, then reap the
/// program and report how it exited. EOF, or EIO (how Linux ends a PTY whose
/// program closed it), both mean the child is gone; `wait` reaps it, so no
/// zombie is left, and reports the real code instead of a hardcoded 0.
fn pump_session_output(
    mut reader: Box<dyn Read + Send>,
    mut child: Box<dyn portable_pty::Child + Send + Sync>,
    id: SessionId,
    sink: EventSink,
) {
    let mut buf = [0u8; READ_CHUNK_BYTES];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => sink(SessionEvent::Output(SessionOutput {
                bytes: buf[..n].to_vec(),
                id: id.clone(),
            })),
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break,
        }
    }
    let code = child.wait().ok().map(|status| status.exit_code() as i32);
    sink(SessionEvent::Exit(SessionExit { code, id }));
}
// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A sink feeding a channel the test reads from.
    fn channel_sink() -> (EventSink, mpsc::Receiver<SessionEvent>) {
        let (tx, rx) = mpsc::channel();
        let sink: EventSink = Arc::new(move |event| {
            let _ = tx.send(event);
        });
        (sink, rx)
    }

    #[test]
    fn test_a_reused_name_gets_a_new_generation_and_ignores_the_old_one() {
        // Regression: sessions were keyed by name alone, so output and the
        // exit of a killed program still winding down landed on the new
        // session that reused its name, and that exit killed it.
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        let old = mgr.create("a", 80, 24).unwrap();
        assert_eq!(mgr.kill("a"), Some(old.clone()));
        let new = mgr.create("a", 80, 24).unwrap();
        assert_ne!(old, new);
        assert!(!mgr.is_current(&old));
        assert!(mgr.is_current(&new));

        mgr.record_output(&old, b"stale");
        assert_eq!(mgr.scrollback("a"), Some(Vec::new()));
        assert!(
            !mgr.remove(&old),
            "the old exit must not end the new session"
        );
        assert!(mgr.has("a"));
        assert!(mgr.remove(&new));
        assert!(!mgr.has("a"));
    }

    fn grid(cols: u16, rows: u16) -> Geometry {
        Geometry {
            cell_px: None,
            cols,
            rows,
        }
    }

    #[test]
    fn test_create_and_list_sessions() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create("test", 80, 24).unwrap();
        assert!(mgr.has("test"));
        assert_eq!(mgr.session_names(), vec!["test"]);
    }

    #[test]
    fn test_create_duplicate_is_ok() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create("dup", 80, 24).unwrap();
        mgr.create("dup", 80, 24).unwrap();
        assert_eq!(mgr.session_names().len(), 1);
    }

    #[test]
    fn test_kill_removes_session() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create("temp", 80, 24).unwrap();
        mgr.kill("temp");
        assert!(!mgr.has("temp"));
    }

    #[test]
    fn test_killing_a_session_ends_its_program() {
        // Dropping the PTY alone left the program running, unseen, behind the
        // reader thread's copy of the master.
        let (sink, rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        let mut sleep = CommandBuilder::new("sleep");
        sleep.arg("30");
        mgr.create_with("doomed", 80, 24, sleep, None, None)
            .unwrap();
        mgr.kill("doomed");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let exited = std::iter::from_fn(|| {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            rx.recv_timeout(left).ok()
        })
        .any(|msg| matches!(msg, SessionEvent::Exit(SessionExit { ref id, .. }) if id.name == "doomed"));
        assert!(exited, "the program exited within the deadline");
    }

    #[test]
    fn test_write_to_nonexistent_is_ok() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.write("missing", b"hello").unwrap();
    }

    #[test]
    fn test_resize_nonexistent_is_ok() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.resize("missing", grid(100, 50)).unwrap();
    }

    #[test]
    fn test_session_exit_reports_the_real_exit_code() {
        // Regression: the reader thread discarded the spawned child's handle,
        // so it could never `wait()` on it, and every exit was reported as a
        // hardcoded `Some(0)` regardless of how the shell actually exited.
        let (sink, rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create("exit-code-test", 80, 24).unwrap();
        mgr.write("exit-code-test", b"exit 42\n").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut exit_code = None;
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(SessionEvent::Exit(SessionExit { code, .. })) => {
                    exit_code = Some(code);
                    break;
                }
                Ok(_) => continue,
                Err(_) => continue,
            }
        }
        assert_eq!(
            exit_code,
            Some(Some(42)),
            "shell's real exit code must be reported, not a hardcoded 0"
        );
    }

    #[test]
    fn test_ring_keeps_everything_until_capacity() {
        let mut ring = ScrollbackRing::new(1024);
        ring.push(b"hello\nworld");
        assert_eq!(ring.snapshot(), b"hello\nworld");
    }

    #[test]
    fn test_ring_evicts_oldest_bytes_beyond_capacity() {
        // Two 100-byte writes into a 150-byte ring keep only the newest
        // 150 bytes; the snapshot then starts after the surviving newline,
        // i.e. at exactly the second write.
        let mut ring = ScrollbackRing::new(150);
        let first: Vec<u8> = std::iter::repeat_n(b'a', 99).chain(Some(b'\n')).collect();
        let second = vec![b'x'; 100];
        ring.push(&first);
        ring.push(&second);
        assert_eq!(ring.snapshot(), second.as_slice());
    }

    #[test]
    fn test_ring_snapshot_drops_torn_first_line_after_eviction() {
        // After eviction the buffer begins mid-line; replay must start at
        // the next line boundary or the client renders a garbage first line.
        let mut ring = ScrollbackRing::new(16);
        ring.push(b"AAAA\ngood\nBBBB");
        ring.push(b"CCCC");
        assert_eq!(ring.snapshot(), b"good\nBBBBCCCC");
    }

    #[test]
    fn test_record_output_accumulates_session_history() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        let id = mgr.create("hist", 80, 24).unwrap();
        mgr.record_output(&id, b"echo one\n");
        mgr.record_output(&id, b"echo two\n");
        assert_eq!(
            mgr.scrollback("hist"),
            Some(b"echo one\necho two\n".to_vec())
        );
    }

    #[test]
    fn test_scrollback_unknown_session_returns_none() {
        let (sink, _rx) = channel_sink();
        let mgr = SessionManager::new(sink);
        assert_eq!(mgr.scrollback("missing"), None);
    }

    #[test]
    fn test_create_with_runs_the_given_command() {
        // Regression: every session spawned the default shell, so a mux
        // server could not host a named long-running command at all.
        let (sink, rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        let mut cmd = CommandBuilder::new("true");
        cmd.arg("--help");
        mgr.create_with("runner", 90, 30, cmd, Some("true --help"), None)
            .unwrap();

        assert_eq!(mgr.session_info()[0].command, "true --help");
        assert_eq!(mgr.geometry("runner"), Some((90, 30)));

        // `true --help` exits immediately; the reader thread must report it.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut exited = false;
        while std::time::Instant::now() < deadline {
            if matches!(
                rx.recv_timeout(std::time::Duration::from_millis(100)),
                Ok(SessionEvent::Exit(_))
            ) {
                exited = true;
                break;
            }
        }
        assert!(exited, "the custom command must actually run");
    }

    #[test]
    fn test_session_defs_reflects_raw_command_and_cwd() {
        // Regression: cwd/raw_command were discarded after spawn, so a
        // restart's respawn recipe had nothing to work with.
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create_with(
            "runner",
            90,
            30,
            CommandBuilder::new("true"),
            Some("true --help"),
            Some("/tmp"),
        )
        .unwrap();

        let defs = mgr.session_defs();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].name, "runner");
        assert_eq!(defs[0].command.as_deref(), Some("true --help"));
        assert_eq!(defs[0].cwd.as_deref(), Some("/tmp"));
    }

    #[test]
    fn test_geometry_tracks_create_and_resize() {
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create("geo", 100, 40).unwrap();
        assert_eq!(mgr.geometry("geo"), Some((100, 40)));
        mgr.resize("geo", grid(120, 50)).unwrap();
        assert_eq!(mgr.geometry("geo"), Some((120, 50)));
        assert_eq!(mgr.geometry("missing"), None);
    }

    #[test]
    fn test_session_info_reports_real_geometry_and_creation_time() {
        // Regression: listings hard-coded 80x24 and created=0 for every
        // session, so the palette showed placeholder data no matter what
        // the PTY actually was.
        let (sink, _rx) = channel_sink();
        let mut mgr = SessionManager::new(sink);
        mgr.create("beta", 80, 24).unwrap();
        mgr.create("alpha", 132, 43).unwrap();
        let info = mgr.session_info();
        assert_eq!(info.len(), 2);
        assert_eq!(
            info[0].name, "alpha",
            "listings are name-sorted, not HashMap order"
        );
        assert_eq!((info[0].cols, info[0].rows), (132, 43));
        assert!(info[0].created > 0);
        assert_eq!((info[1].cols, info[1].rows), (80, 24));
    }
}

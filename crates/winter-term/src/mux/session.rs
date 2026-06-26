//! Session manager: owns PTY children, reads their output, and routes I/O.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;

use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};

use super::persist::SessionDef;
use super::protocol::{ServerMessage, SessionInfo};

// ========================================================================
// Constants
// ========================================================================

/// Retained PTY output per session, bounded so a long-lived server's
/// memory scales with session count rather than uptime; ~1 MiB covers a
/// typical session's useful history.
const SCROLLBACK_CAPACITY_BYTES: usize = 1 << 20;

// ========================================================================
// Data Structures
// ========================================================================

/// Bounded append-only log of a session's PTY output, kept so clients
/// attaching later can replay what they missed.
struct ScrollbackRing {
    buf: Vec<u8>,
    capacity: usize,
    /// Set once the head has been evicted: the buffer then starts
    /// mid-stream rather than at the session's first byte.
    evicted: bool,
}

struct Session {
    /// The command line the session runs, for listings.
    command: String,
    /// Current PTY geometry, tracked so listings and attach confirmations
    /// report reality rather than the create-time default.
    cols: u16,
    /// Creation time, Unix-epoch seconds.
    created: u64,
    /// Working directory the session was spawned in, for restart survival.
    cwd: Option<String>,
    history: ScrollbackRing,
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// Unwrapped command line the session runs, for restart survival, as
    /// given by the caller, before shell-wrapping, unlike `command`.
    raw_command: Option<String>,
    rows: u16,
    writer: Box<dyn Write + Send>,
}

/// Owns every live PTY session and publishes what they write.
pub struct SessionManager {
    next_id: u64,
    sessions: HashMap<String, Session>,
    output_tx: mpsc::Sender<ServerMessage>,
}

// ========================================================================
// Implementation
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

impl SessionManager {
    /// A manager that publishes session output on the given channel.
    pub fn new(output_tx: mpsc::Sender<ServerMessage>) -> Self {
        SessionManager {
            next_id: 1,
            sessions: HashMap::new(),
            output_tx,
        }
    }

    /// Spawn a session under this name; an existing name is an error.
    pub fn create(&mut self, name: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
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
    ) -> anyhow::Result<()> {
        if self.sessions.contains_key(name) {
            return Ok(());
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

        let mut child = pair.slave.spawn_command(command)?;

        let reader = pair.master.try_clone_reader()?;
        let writer = pair.master.take_writer()?;
        let tx = self.output_tx.clone();
        let session_name = name.to_string();

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            let mut reader = reader;
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => {
                        // EOF means the child already exited; `wait` reaps it
                        // and reports the real code instead of a hardcoded 0.
                        let code = child.wait().ok().map(|status| status.exit_code() as i32);
                        let _ = tx.send(ServerMessage::Exit {
                            session: session_name.clone(),
                            code,
                        });
                        break;
                    }
                    Ok(n) => {
                        let _ = tx.send(ServerMessage::Output {
                            session: session_name.clone(),
                            bytes: buf[..n].to_vec(),
                        });
                    }
                    Err(_) => {
                        let _ = tx.send(ServerMessage::Exit {
                            session: session_name.clone(),
                            code: None,
                        });
                        break;
                    }
                }
            }
        });

        let created = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.sessions.insert(
            name.to_string(),
            Session {
                command: if command_label.is_empty() {
                    "(default shell)".to_string()
                } else {
                    command_label
                },
                cols,
                created,
                cwd: cwd.map(str::to_string),
                history: ScrollbackRing::new(SCROLLBACK_CAPACITY_BYTES),
                master: pair.master,
                raw_command: raw_command.map(str::to_string),
                rows,
                writer,
            },
        );
        self.next_id += 1;
        Ok(())
    }

    /// Forward bytes to a session's PTY.
    pub fn write(&mut self, name: &str, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(session) = self.sessions.get_mut(name) {
            session.writer.write_all(bytes)?;
            session.writer.flush()?;
        }
        Ok(())
    }

    /// Appends live PTY output to the session's retained history so it
    /// can be replayed to clients that attach later.
    pub fn record_output(&mut self, name: &str, bytes: &[u8]) {
        if let Some(session) = self.sessions.get_mut(name) {
            session.history.push(bytes);
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
    pub fn resize(&mut self, name: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        if let Some(session) = self.sessions.get_mut(name) {
            session.master.resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })?;
            session.cols = cols;
            session.rows = rows;
        }
        Ok(())
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

    /// Terminate a session and drop its PTY.
    pub fn kill(&mut self, name: &str) {
        self.sessions.remove(name);
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_create_and_list_sessions() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("test", 80, 24).unwrap();
        assert!(mgr.has("test"));
        assert_eq!(mgr.session_names(), vec!["test"]);
    }

    #[test]
    fn test_create_duplicate_is_ok() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("dup", 80, 24).unwrap();
        mgr.create("dup", 80, 24).unwrap();
        assert_eq!(mgr.session_names().len(), 1);
    }

    #[test]
    fn test_kill_removes_session() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("temp", 80, 24).unwrap();
        mgr.kill("temp");
        assert!(!mgr.has("temp"));
    }

    #[test]
    fn test_write_to_nonexistent_is_ok() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.write("missing", b"hello").unwrap();
    }

    #[test]
    fn test_resize_nonexistent_is_ok() {
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.resize("missing", 100, 50).unwrap();
    }

    #[test]
    fn test_session_exit_reports_the_real_exit_code() {
        // Regression: the reader thread discarded the spawned child's handle,
        // so it could never `wait()` on it, and every exit was reported as a
        // hardcoded `Some(0)` regardless of how the shell actually exited.
        let (tx, rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("exit-code-test", 80, 24).unwrap();
        mgr.write("exit-code-test", b"exit 42\n").unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        let mut exit_code = None;
        while std::time::Instant::now() < deadline {
            match rx.recv_timeout(std::time::Duration::from_millis(200)) {
                Ok(ServerMessage::Exit { code, .. }) => {
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
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("hist", 80, 24).unwrap();
        mgr.record_output("hist", b"echo one\n");
        mgr.record_output("hist", b"echo two\n");
        assert_eq!(
            mgr.scrollback("hist"),
            Some(b"echo one\necho two\n".to_vec())
        );
    }

    #[test]
    fn test_scrollback_unknown_session_returns_none() {
        let (tx, _rx) = mpsc::channel();
        let mgr = SessionManager::new(tx);
        assert_eq!(mgr.scrollback("missing"), None);
    }

    #[test]
    fn test_create_with_runs_the_given_command() {
        // Regression: every session spawned the default shell, so a mux
        // server could not host a named long-running command at all.
        let (tx, rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
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
                Ok(ServerMessage::Exit { .. })
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
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
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
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
        mgr.create("geo", 100, 40).unwrap();
        assert_eq!(mgr.geometry("geo"), Some((100, 40)));
        mgr.resize("geo", 120, 50).unwrap();
        assert_eq!(mgr.geometry("geo"), Some((120, 50)));
        assert_eq!(mgr.geometry("missing"), None);
    }

    #[test]
    fn test_session_info_reports_real_geometry_and_creation_time() {
        // Regression: listings hard-coded 80x24 and created=0 for every
        // session, so the palette showed placeholder data no matter what
        // the PTY actually was.
        let (tx, _rx) = mpsc::channel();
        let mut mgr = SessionManager::new(tx);
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

//! Mux client: connects to the mux server over a Unix socket.

use std::collections::VecDeque;
use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};
#[cfg(windows)]
use uds_windows::UnixStream;

use super::protocol::{self, ClientMessage, FrameBuffer, ServerMessage, SessionInfo};
use super::transport::{self, MuxTransport};

// ========================================================================
// Constants
// ========================================================================

/// Pause between polls while waiting for a server reply, which the server
/// sends as soon as it has handled the request.
const POLL_INTERVAL: Duration = Duration::from_millis(10);

/// The most unsent bytes the client queues while the server is not reading.
/// Input travels as base64, four bytes on the wire for every three pasted,
/// so this takes a paste of over twenty megabytes; a server further
/// behind than that is stuck, and the connection is treated as broken rather
/// than queueing without end.
const MAX_OUTBOX_BYTES: usize = 32 * 1024 * 1024;

// ========================================================================
// Data Structures
// ========================================================================

/// A connection to a mux server over a Unix domain socket.
pub struct MuxClient {
    /// Set when the server closed its end of the connection: every further
    /// read returns EOF, so "nothing available right now" and "never
    /// again" must be distinguishable (see [`MuxClient::eof`]).
    eof: bool,
    frames: FrameBuffer,
    /// Encoded frames the socket has not taken yet. The socket does not
    /// block, so a write can stop mid-frame; what is left waits here, whole
    /// frames in order, and goes out on the next send or poll, so the server
    /// never sees half a frame followed by the start of another.
    outbox: Vec<u8>,
    pending: VecDeque<ServerMessage>,
    stream: UnixStream,
}

// ========================================================================
// MuxClient
// ========================================================================

impl MuxClient {
    /// Connect to the server listening on the socket at `path`, opening
    /// with the protocol handshake.
    pub fn connect(path: &str) -> anyhow::Result<Self> {
        let stream = UnixStream::connect(path)?;
        stream.set_nonblocking(true)?;
        let mut client = MuxClient {
            eof: false,
            frames: FrameBuffer::new(),
            outbox: Vec::new(),
            pending: VecDeque::new(),
            stream,
        };
        // A server that accepts and closes at once refuses the handshake;
        // that is a clean close for `recv` to report, not a failed connect.
        client
            .outbox
            .extend_from_slice(&protocol::encode(&transport::hello()));
        client.flush_noting_hang_up()?;
        Ok(client)
    }

    /// Attach to a session, which the server creates if it is absent.
    /// `replay` asks for the session's buffered output; a client that
    /// already shows it (one reconnecting) asks for none.
    pub fn attach(&mut self, session: &str, replay: bool) -> anyhow::Result<()> {
        self.send(&ClientMessage::Attach {
            replay,
            session: session.to_string(),
        })
    }

    /// Detach from the current session without killing it.
    pub fn detach(&mut self) -> anyhow::Result<()> {
        self.send(&ClientMessage::Detach)
    }

    /// Ask for the session list; the reply arrives as a later message.
    pub fn list_sessions(&mut self) -> anyhow::Result<()> {
        self.send(&ClientMessage::ListSessions)
    }

    /// Terminate a session and the process it is running.
    pub fn kill(&mut self, session: &str) -> anyhow::Result<()> {
        self.send(&ClientMessage::Kill {
            session: session.to_string(),
        })
    }

    /// Create a session running `command` (the default shell when `None`)
    /// and attach to it.
    pub fn spawn(
        &mut self,
        session: &str,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        command: Option<&str>,
    ) -> anyhow::Result<()> {
        self.send(&ClientMessage::Spawn {
            session: session.to_string(),
            cols,
            rows,
            cwd: cwd.map(str::to_string),
            command: command.map(str::to_string),
        })
    }

    /// One buffered `ServerMessage` per call, oldest first. A single
    /// `read()` can contain several frames (or only part of one): they're
    /// reassembled into `frames`/`pending` here so a caller looping on this
    /// until `Ok(None)` still sees every message, in order, exactly once.
    /// A handshake reply naming a version this client does not speak is an
    /// error.
    pub fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        let msg = self.read_message()?;
        if let Some(msg) = &msg {
            transport::check_handshake(msg)?;
        }
        Ok(msg)
    }

    fn read_message(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        self.flush_noting_hang_up()?;
        if let Some(msg) = self.pending.pop_front() {
            return Ok(Some(msg));
        }
        let mut buf = [0u8; 8192];
        match self.stream.read(&mut buf) {
            Ok(0) => {
                // The server closed its end: record it so callers can tell
                // a clean shutdown apart from a merely idle socket.
                self.eof = true;
                Ok(None)
            }
            Ok(n) => {
                self.frames.extend(&buf[..n]);
                self.pending.extend(self.frames.drain::<ServerMessage>());
                Ok(self.pending.pop_front())
            }
            Err(ref e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            // A server that closes with our handshake still unread resets
            // the connection rather than ending it: the same hang-up.
            Err(ref e) if is_hang_up(e) => {
                self.eof = true;
                Ok(None)
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Whether the server has closed its end of the connection (clean
    /// shutdown). Buffered messages may still be drained via
    /// [`recv`](Self::recv) after this returns `true`.
    pub fn eof(&self) -> bool {
        self.eof
    }

    /// Send `ListSessions` and poll until the reply arrives or `timeout`
    /// elapses.
    ///
    /// The server services its sockets on a poll cycle, so the reply is
    /// never available to the first nonblocking read: a caller that sends
    /// the request and then calls [`recv`](Self::recv) once races the
    /// server, sees nothing, and falls back to placeholder data. This
    /// polls with a deadline instead (the same shape as the CLI's
    /// `drain_mux_messages`). It runs on the calling thread, so keep the
    /// timeout small.
    pub fn query_sessions(&mut self, timeout: Duration) -> anyhow::Result<Vec<SessionInfo>> {
        self.list_sessions()?;
        let deadline = Instant::now() + timeout;
        loop {
            while let Some(msg) = self.recv()? {
                if let ServerMessage::SessionList { sessions } = msg {
                    return Ok(sessions);
                }
            }
            if Instant::now() >= deadline {
                anyhow::bail!("timed out waiting for the session list");
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    /// Send `Spawn` and poll until the server confirms the new session,
    /// reports an error, or `timeout` elapses: the same deadline-polling
    /// rationale as [`query_sessions`](Self::query_sessions). `Ok` returns
    /// the confirmed geometry.
    pub fn spawn_confirmed(
        &mut self,
        session: &str,
        cols: u16,
        rows: u16,
        cwd: Option<&str>,
        command: Option<&str>,
        timeout: Duration,
    ) -> Result<(u16, u16), String> {
        self.spawn(session, cols, rows, cwd, command)
            .map_err(|e| e.to_string())?;
        let deadline = Instant::now() + timeout;
        loop {
            while let Some(msg) = self.recv().map_err(|e| e.to_string())? {
                match msg {
                    ServerMessage::Attached { cols, rows, .. } => return Ok((cols, rows)),
                    ServerMessage::Error { message } => return Err(message),
                    _ => {}
                }
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for the session to start".to_string());
            }
            std::thread::sleep(POLL_INTERVAL);
        }
    }

    fn queue(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        let encoded = protocol::encode(msg);
        if self.outbox.len() + encoded.len() > MAX_OUTBOX_BYTES {
            anyhow::bail!("the mux server is not reading");
        }
        self.outbox.extend_from_slice(&encoded);
        Ok(self.flush_outbox()?)
    }

    /// Flush the outbox, recording a server that hung up as EOF. A closed
    /// server refuses what is still queued, often the handshake; that is the
    /// same clean close a read reports, and whatever the server sent before
    /// closing can still be read.
    fn flush_noting_hang_up(&mut self) -> std::io::Result<()> {
        match self.flush_outbox() {
            Err(e) if is_hang_up(&e) => {
                self.eof = true;
                self.outbox.clear();
                Ok(())
            }
            result => result,
        }
    }

    /// Write as much of the outbox as the socket takes now, keeping the rest.
    fn flush_outbox(&mut self) -> std::io::Result<()> {
        while !self.outbox.is_empty() {
            match self.stream.write(&self.outbox) {
                Ok(0) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::WriteZero,
                        "the mux server closed the connection",
                    ))
                }
                Ok(written) => {
                    self.outbox.drain(..written);
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => break,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }
}

/// Whether a read or write failed because the server closed the
/// connection, rather than for some other reason.
fn is_hang_up(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionReset
            | std::io::ErrorKind::NotConnected
            | std::io::ErrorKind::WriteZero
    )
}

impl MuxTransport for MuxClient {
    fn eof(&self) -> bool {
        MuxClient::eof(self)
    }

    fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        MuxClient::recv(self)
    }

    fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        self.queue(msg)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    use std::os::unix::net::UnixListener;
    #[cfg(windows)]
    use uds_windows::UnixListener;

    #[test]
    fn test_connect_to_missing_socket_fails() {
        assert!(MuxClient::connect("/tmp/winter-mux-nonexistent-test.sock").is_err());
    }

    /// A throwaway socket path unique to this test process.
    fn test_socket_path(tag: &str) -> String {
        std::env::temp_dir()
            .join(format!(
                "winter-mux-client-{tag}-{}.sock",
                std::process::id()
            ))
            .to_string_lossy()
            .to_string()
    }

    /// Read one length-prefixed frame from a blocking stream.
    fn read_frame(conn: &mut impl Read) -> Vec<u8> {
        let mut len_bytes = [0u8; 4];
        conn.read_exact(&mut len_bytes).unwrap();
        let mut framed = len_bytes.to_vec();
        let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
        conn.read_exact(&mut body).unwrap();
        framed.extend(body);
        framed
    }

    #[test]
    fn test_query_sessions_waits_for_the_delayed_reply() {
        // Regression: the server answers on its ~10 ms poll cycle, so the
        // reply is never there for an immediate one-shot read: a query
        // must keep polling until it arrives, or every listing falls back
        // to placeholder data.
        let path = test_socket_path("query");
        let listener = UnixListener::bind(&path).unwrap();

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _request = read_frame(&mut conn);
            // Reply late enough that a single immediate recv has already
            // returned WouldBlock and given up.
            std::thread::sleep(POLL_INTERVAL * 4);
            let reply = protocol::encode(&ServerMessage::SessionList {
                sessions: vec![SessionInfo {
                    attach_count: 0,
                    name: "work".into(),
                    cols: 120,
                    rows: 40,
                    created: 0,
                    command: "cargo watch".into(),
                }],
            });
            conn.write_all(&reply).unwrap();
        });

        let mut client = MuxClient::connect(&path).unwrap();
        let sessions = client.query_sessions(Duration::from_millis(500)).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "work");
        assert_eq!(sessions[0].cols, 120);
        assert_eq!(sessions[0].rows, 40);

        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_query_sessions_times_out_when_nothing_answers() {
        let path = test_socket_path("query-timeout");
        let listener = UnixListener::bind(&path).unwrap();
        let _server = std::thread::spawn(move || {
            // Accept, consume the request, and never reply.
            let (mut conn, _) = listener.accept().unwrap();
            let _request = read_frame(&mut conn);
        });

        let mut client = MuxClient::connect(&path).unwrap();
        assert!(
            client.query_sessions(Duration::from_millis(60)).is_err(),
            "a server that never answers must surface as an error, not an empty list"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_recv_records_eof_after_server_closes() {
        // Regression: EOF (`Ok(0)`) and an idle socket (`WouldBlock`) both
        // returned `Ok(None)`, so a cleanly shut-down server was
        // indistinguishable from a quiet one.
        let path = test_socket_path("eof");
        let listener = UnixListener::bind(&path).unwrap();
        // The server holds the connection until the client has looked: closed
        // before `connect` returns, the hello itself hits the hang-up and
        // `connect` records the close, which is right but not what this tests.
        let (close_tx, close_rx) = std::sync::mpsc::channel::<()>();
        let server = std::thread::spawn(move || {
            let (conn, _) = listener.accept().unwrap();
            let _ = close_rx.recv();
            drop(conn);
        });

        let mut client = MuxClient::connect(&path).unwrap();
        assert!(!client.eof());
        close_tx.send(()).unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !client.eof() && Instant::now() < deadline {
            let _ = client.recv().unwrap();
            std::thread::sleep(Duration::from_millis(1));
        }
        assert!(
            client.eof(),
            "a closed server must be observable, not just idle"
        );

        server.join().unwrap();
        let _ = std::fs::remove_file(&path);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_send_the_socket_cannot_take_now_arrives_later_as_one_whole_frame() {
        // A paste larger than the socket buffer, while the server is busy:
        // the old `write_all` on the non-blocking socket gave up mid-frame.
        let (client_end, mut server_end) = UnixStream::pair().expect("pair");
        client_end.set_nonblocking(true).expect("nonblocking");
        let mut client = MuxClient {
            eof: false,
            frames: FrameBuffer::new(),
            outbox: Vec::new(),
            pending: VecDeque::new(),
            stream: client_end,
        };
        let paste = vec![b'p'; 1 << 20];
        client
            .send_input("main", &paste)
            .expect("queued, not failed");
        assert!(
            !client.outbox.is_empty(),
            "more than the socket took at once"
        );

        let reader = std::thread::spawn(move || {
            let mut frames = FrameBuffer::new();
            let mut buf = [0u8; 65536];
            loop {
                let n = server_end.read(&mut buf).expect("read");
                frames.extend(&buf[..n]);
                if let Some(msg) = frames.drain::<ClientMessage>().pop() {
                    return msg;
                }
            }
        });
        while !client.outbox.is_empty() {
            client.flush_outbox().expect("flushed");
            std::thread::sleep(Duration::from_millis(1));
        }
        match reader.join().expect("reader") {
            ClientMessage::Input { session, bytes } => {
                assert_eq!(session, "main");
                assert_eq!(bytes.len(), paste.len());
            }
            other => panic!("expected the input frame, got {other:?}"),
        }
    }
}

//! Resilience: automatic reconnection with exponential backoff.
//!
//! Wraps a mux transport — local Unix socket or SSH bridge — and
//! reconnects transparently when the connection drops.

use std::time::{Duration, Instant};

use super::client::MuxClient;
use super::protocol::ServerMessage;
use super::remote::RemoteClient;

// ========================================================================
// Data Structures
// ========================================================================

/// Where a resilient client's transport connects: the local mux server's
/// Unix socket, or an SSH bridge to a mux server on a remote host.
enum Endpoint {
    Local { path: String },
    Remote { host: String },
}

/// The live connection for an [`Endpoint`]: a Unix-socket client, or the
/// ssh child tunneling one.
enum Transport {
    Local(MuxClient),
    Remote(RemoteClient),
}

pub struct ResilientClient {
    backoff: Duration,
    connected: bool,
    endpoint: Endpoint,
    inner: Option<Transport>,
    /// The last geometry this pane asked for, re-sent after a reconnect so
    /// the server's size arbitration sees this client's constraint again
    /// (a fresh connection starts with none).
    last_resize: Option<(u16, u16)>,
    last_attempt: Option<Instant>,
    max_backoff: Duration,
    retries: u32,
    session: String,
}

// ========================================================================
// Constants
// ========================================================================

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_RETRIES: u32 = 50;

// ========================================================================
// Implementation
// ========================================================================

impl Transport {
    /// Connect per `endpoint` and attach to `session`. The remote endpoint
    /// attaches through the proxy invocation on the far side.
    fn connect(endpoint: &Endpoint, session: &str) -> anyhow::Result<Self> {
        match endpoint {
            Endpoint::Local { path } => {
                let mut client = MuxClient::connect(path)?;
                client.attach(session)?;
                Ok(Transport::Local(client))
            }
            Endpoint::Remote { host } => {
                let client = RemoteClient::connect(host, Some(session))?;
                Ok(Transport::Remote(client))
            }
        }
    }

    fn eof(&self) -> bool {
        match self {
            Transport::Local(client) => client.eof(),
            Transport::Remote(client) => client.eof(),
        }
    }

    fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        match self {
            Transport::Local(client) => client.recv(),
            Transport::Remote(client) => client.recv(),
        }
    }

    fn resize(&mut self, session: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        match self {
            Transport::Local(client) => client.resize(session, cols, rows),
            Transport::Remote(client) => client.resize(session, cols, rows),
        }
    }

    fn send_input(&mut self, session: &str, bytes: &[u8]) -> anyhow::Result<()> {
        match self {
            Transport::Local(client) => client.send_input(session, bytes),
            Transport::Remote(client) => client.send_input(session, bytes),
        }
    }
}

impl ResilientClient {
    pub fn new(path: &str, session: &str) -> Self {
        Self::build(
            Endpoint::Local {
                path: path.to_string(),
            },
            session,
        )
    }

    /// Attach to `session` on a remote mux server reached over SSH at
    /// `host` (via `winter mux proxy` on the far side). Same reconnect
    /// behavior as the local transport.
    pub fn new_remote(host: &str, session: &str) -> Self {
        Self::build(
            Endpoint::Remote {
                host: host.to_string(),
            },
            session,
        )
    }

    fn build(endpoint: Endpoint, session: &str) -> Self {
        let inner = Transport::connect(&endpoint, session).ok();
        let connected = inner.is_some();
        ResilientClient {
            backoff: INITIAL_BACKOFF,
            connected,
            endpoint,
            inner,
            last_resize: None,
            last_attempt: None,
            max_backoff: MAX_BACKOFF,
            retries: 0,
            session: session.to_string(),
        }
    }

    pub fn send_input(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        if let Some(ref mut client) = self.inner {
            let result = client.send_input(&self.session, bytes);
            if result.is_err() {
                self.connected = false;
            }
            return result;
        }
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.last_resize = Some((cols, rows));
        if let Some(ref mut client) = self.inner {
            let result = client.resize(&self.session, cols, rows);
            if result.is_err() {
                self.connected = false;
            }
            return result;
        }
        Ok(())
    }

    pub fn recv(&mut self) -> Option<ServerMessage> {
        if let Some(ref mut client) = self.inner {
            match client.recv() {
                Ok(Some(msg)) => {
                    self.retries = 0;
                    self.backoff = INITIAL_BACKOFF;
                    return Some(msg);
                }
                Ok(None) => {
                    // A clean server shutdown is an EOF, not an error: the
                    // stream stays readable and every read returns zero,
                    // so without checking `eof` the client would sit
                    // "connected" to a dead server forever and never
                    // reconnect — the exact scenario this layer exists for.
                    if !client.eof() {
                        return None;
                    }
                    self.connected = false;
                }
                Err(_) => {
                    self.connected = false;
                }
            }
        }

        self.try_reconnect();
        None
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    fn try_reconnect(&mut self) {
        if self.retries >= MAX_RETRIES {
            return;
        }
        if let Some(last) = self.last_attempt {
            if last.elapsed() < self.backoff {
                return;
            }
        }
        self.last_attempt = Some(Instant::now());
        self.retries += 1;

        if let Ok(mut client) = Transport::connect(&self.endpoint, &self.session) {
            if let Some((cols, rows)) = self.last_resize {
                let _ = client.resize(&self.session, cols, rows);
            }
            self.inner = Some(client);
            self.connected = true;
            self.retries = 0;
            self.backoff = INITIAL_BACKOFF;
        }

        self.backoff = (self.backoff * 2).min(self.max_backoff);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resilient_client_starts_disconnected_for_bad_path() {
        let client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(!client.is_connected());
    }

    #[test]
    fn test_resilient_client_send_succeeds_gracefully() {
        let mut client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(client.send_input(b"hello").is_ok());
    }

    #[test]
    fn test_resilient_client_resize_succeeds_gracefully() {
        let mut client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(client.resize(80, 24).is_ok());
    }

    #[test]
    fn test_resilient_client_recv_returns_none() {
        let mut client = ResilientClient::new("/tmp/nonexistent.sock", "default");
        assert!(client.recv().is_none());
    }

    #[test]
    fn test_resilient_remote_client_starts_disconnected_for_unspawnable_bridge() {
        // An argv the OS rejects (interior NUL byte) makes the bridge spawn
        // fail outright; the remote endpoint must degrade to the same
        // disconnected-but-usable state as a missing local socket.
        let mut client = ResilientClient::new_remote("bad\0host", "s");
        assert!(!client.is_connected());
        assert!(client.send_input(b"x").is_ok());
        assert!(client.recv().is_none());
    }

    #[test]
    fn test_recv_reconnects_after_clean_server_shutdown() {
        // Regression: the server closing its socket read as "idle"
        // (EOF → Ok(None)), so the resilient client never marked itself
        // disconnected and never reconnected — a pane stayed dead after a
        // server restart despite the resilience layer. A restarted server
        // must be reattached to transparently.
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path = std::env::temp_dir()
            .join(format!("winter-mux-resilience-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let server = std::thread::spawn(move || {
            // First connection: read the attach, then close without a
            // word — a clean server shutdown.
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn);
            drop(conn);
            // Second connection: the reconnect. Confirm the attach and
            // deliver one output frame so recovery is observable.
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn);
            for frame in [
                crate::mux::protocol::encode(&ServerMessage::Attached {
                    session: "s".into(),
                    cols: 80,
                    rows: 24,
                }),
                crate::mux::protocol::encode(&ServerMessage::Output {
                    session: "s".into(),
                    bytes: b"recovered".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            // Hold the connection open until the test has finished reading.
            std::thread::sleep(Duration::from_secs(5));
        });

        let mut client = ResilientClient::new(&path, "s");
        assert!(client.is_connected());

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut recovered = false;
        while Instant::now() < deadline {
            if let Some(ServerMessage::Output { bytes, .. }) = client.recv() {
                assert_eq!(bytes, b"recovered".to_vec());
                recovered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            recovered,
            "the client must reattach and receive output after the server restarts"
        );
        assert!(client.is_connected());

        let _ = std::fs::remove_file(&path);
        drop(server);
    }
}

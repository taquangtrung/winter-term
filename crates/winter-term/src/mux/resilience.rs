//! Resilience: automatic reconnection with exponential backoff.
//!
//! Wraps a mux transport, local Unix socket or SSH bridge, and
//! reconnects transparently when the connection drops.

use std::time::{Duration, Instant};

use super::client::MuxClient;
use super::protocol::{Geometry, ServerMessage};
use super::remote::RemoteClient;
use super::transport::MuxTransport;

// ========================================================================
// Constants
// ========================================================================

const INITIAL_BACKOFF: Duration = Duration::from_millis(100);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const MAX_RETRIES: u32 = 50;

// ========================================================================
// Data Structures
// ========================================================================

/// A mux client that reconnects on its own when the server goes away.
pub struct ResilientClient {
    backoff: Duration,
    connected: bool,
    endpoint: Endpoint,
    inner: Option<Box<dyn MuxTransport + Send>>,
    /// The last geometry this pane asked for, re-sent after a reconnect so
    /// the server's size arbitration sees this client's constraint again
    /// (a fresh connection starts with none).
    last_resize: Option<Geometry>,
    last_attempt: Option<Instant>,
    max_backoff: Duration,
    retries: u32,
    session: String,
}

/// Where a resilient client's transport connects: the local mux server's
/// Unix socket, or an SSH bridge to a mux server on a remote host.
enum Endpoint {
    Local { path: String },
    Remote { host: String },
}

// ========================================================================
// Endpoint
// ========================================================================

impl Endpoint {
    /// Connect and attach to `session`, replaying its buffered output when
    /// `replay` is set. The remote endpoint attaches through the proxy
    /// invocation on the far side.
    fn connect(&self, session: &str, replay: bool) -> anyhow::Result<Box<dyn MuxTransport + Send>> {
        match self {
            Endpoint::Local { path } => {
                let mut client = MuxClient::connect(path)?;
                client.attach(session, replay)?;
                Ok(Box::new(client))
            }
            Endpoint::Remote { host } => {
                Ok(Box::new(RemoteClient::connect(host, session, replay)?))
            }
        }
    }
}

// ========================================================================
// ResilientClient
// ========================================================================

impl ResilientClient {
    /// A client for one session on the server at `path`, connected on first use.
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

    /// The host a remote client reaches over ssh, or `None` for the local
    /// server.
    pub fn remote_host(&self) -> Option<&str> {
        match &self.endpoint {
            Endpoint::Local { .. } => None,
            Endpoint::Remote { host } => Some(host),
        }
    }

    fn build(endpoint: Endpoint, session: &str) -> Self {
        let inner = endpoint.connect(session, true).ok();
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

    /// Forward bytes, reconnecting first when the connection is down.
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

    /// Report geometry, reconnecting first when the connection is down.
    pub fn resize(&mut self, geometry: Geometry) -> anyhow::Result<()> {
        self.last_resize = Some(geometry);
        if let Some(ref mut client) = self.inner {
            let result = client.resize(&self.session, geometry);
            if result.is_err() {
                self.connected = false;
            }
            return result;
        }
        Ok(())
    }

    /// The next message from the server, or `None` when none is pending.
    /// A message arriving is what proves a connection works, so only then
    /// does the retry budget and backoff start over.
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
                    // reconnect: the exact scenario this layer exists for.
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

    /// Whether a live connection to the server is currently held.
    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Connect again once the backoff has passed. Success here proves
    /// little: an ssh bridge spawns fine for a host that cannot be reached,
    /// and dies a moment later. So the retry count and backoff keep growing
    /// across attempts, and only a message from the server
    /// ([`recv`](Self::recv)) resets them; otherwise an unreachable host
    /// would be redialed every few hundred milliseconds forever.
    fn try_reconnect(&mut self) {
        if self.retries >= MAX_RETRIES {
            return;
        }
        if let Some(last) = self.last_attempt {
            if last.elapsed() < self.backoff {
                return;
            }
            self.backoff = (self.backoff * 2).min(self.max_backoff);
        }
        self.last_attempt = Some(Instant::now());
        self.retries += 1;

        // The pane already shows the session's history, so the new
        // connection asks for no replay of it.
        if let Ok(mut client) = self.endpoint.connect(&self.session, false) {
            if let Some(geometry) = self.last_resize {
                let _ = client.resize(&self.session, geometry);
            }
            self.inner = Some(client);
            self.connected = true;
        }
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
        assert!(client
            .resize(Geometry {
                cell_px: None,
                cols: 80,
                rows: 24,
            })
            .is_ok());
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
    fn test_a_connection_that_dies_before_a_message_keeps_backing_off() {
        // Regression: a reconnect that merely connected reset the backoff
        // and the retry count. An ssh bridge to an unreachable host always
        // "connects", so it was respawned every ~200 ms forever. A server
        // that accepts and hangs up at once stands in for it here.
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        const ACCEPTS: usize = 64;
        let path = std::env::temp_dir()
            .join(format!("winter-mux-backoff-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        std::thread::spawn(move || {
            for _ in 0..ACCEPTS {
                let Ok((conn, _)) = listener.accept() else {
                    return;
                };
                drop(conn);
            }
        });

        let mut client = ResilientClient::new(&path, "s");
        let deadline = Instant::now() + Duration::from_millis(1500);
        while Instant::now() < deadline {
            assert!(client.recv().is_none());
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(client.retries >= 2, "retries: {}", client.retries);
        assert!(
            client.backoff > INITIAL_BACKOFF * 2,
            "backoff: {:?}",
            client.backoff
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_recv_reconnects_after_clean_server_shutdown() {
        // Regression: the server closing its socket read as "idle"
        // (EOF → Ok(None)), so the resilient client never marked itself
        // disconnected and never reconnected: a pane stayed dead after a
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
            // word: a clean server shutdown.
            // Read both the handshake and the attach before hanging up, or
            // the client's attach races the close and the first connect
            // fails instead of succeeding and then losing the server.
            let (mut conn, _) = listener.accept().unwrap();
            let _hello = read_frame(&mut conn);
            let _attach = read_frame(&mut conn);
            drop(conn);
            // Second connection: the reconnect. Confirm the attach and
            // deliver one output frame so recovery is observable.
            let (mut conn, _) = listener.accept().unwrap();
            let _hello = read_frame(&mut conn);
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

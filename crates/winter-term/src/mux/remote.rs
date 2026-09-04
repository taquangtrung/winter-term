//! Remote domain support: connects to a remote mux server over SSH.
//!
//! Uses `ssh -W` (or a direct TCP forward) to tunnel the Unix socket
//! through an SSH connection to a remote Winter-mux server.

use std::collections::VecDeque;
use std::io::{Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use super::protocol::{self, ClientMessage, FrameBuffer, ServerMessage};

// ========================================================================
// Constants
// ========================================================================

/// Bytes moved per read by the background pipe pump.
const RECV_BUFFER_BYTES: usize = 8192;

// ========================================================================
// Data Structures
// ========================================================================

/// A mux client whose transport is an SSH tunnel to another host.
pub struct RemoteClient {
    child: Child,
    /// Set by the reader thread once the bridge's stdout closes: the
    /// tunnel is gone and no further frames can arrive.
    eof: Arc<AtomicBool>,
    frames: FrameBuffer,
    pending: VecDeque<ServerMessage>,
    /// Raw bytes pumped off the ssh pipe by the reader thread.
    rx: mpsc::Receiver<Vec<u8>>,
    stdin: ChildStdin,
    _reader: std::thread::JoinHandle<()>,
}

// ========================================================================
// Implementation
// ========================================================================

impl RemoteClient {
    /// Connect to a remote mux server via SSH.
    ///
    /// Spawns `ssh host winter mux proxy <session>` which bridges
    /// stdin/stdout to the remote Unix socket. This avoids needing to
    /// expose a TCP port.
    pub fn connect(host: &str, session: Option<&str>) -> anyhow::Result<Self> {
        let session = session.unwrap_or("default");
        let mut child = Command::new("ssh")
            .arg(host)
            .arg("winter")
            .arg("mux")
            .arg("proxy")
            .arg(session)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            _ => anyhow::bail!("the ssh bridge did not expose piped stdio"),
        };

        // The ssh pipe is blocking and has no portable nonblocking mode,
        // so a reader thread pumps it into a channel — the same shape as
        // the local PTY transport — keeping `recv` nonblocking and EOF
        // observable.
        let (tx, rx) = mpsc::channel();
        let eof = Arc::new(AtomicBool::new(false));
        let reader_eof = Arc::clone(&eof);
        let reader = std::thread::spawn(move || pump_bridge_to_channel(stdout, tx, reader_eof));

        Ok(RemoteClient {
            child,
            eof,
            frames: FrameBuffer::new(),
            pending: VecDeque::new(),
            rx,
            stdin,
            _reader: reader,
        })
    }

    /// Send one request over the tunnel.
    pub fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        let encoded = protocol::encode(msg);
        self.stdin.write_all(&encoded)?;
        self.stdin.flush()?;
        Ok(())
    }

    /// Forward bytes to a remote session's PTY.
    pub fn send_input(&mut self, session: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.send(&ClientMessage::Input {
            session: session.to_string(),
            bytes: bytes.to_vec(),
        })
    }

    /// Report this client's geometry to the remote session.
    pub fn resize(&mut self, session: &str, cols: u16, rows: u16) -> anyhow::Result<()> {
        self.send(&ClientMessage::Resize {
            session: session.to_string(),
            cols,
            rows,
        })
    }

    /// One buffered `ServerMessage` per call, oldest first. Never blocks:
    /// raw bytes are reassembled from the reader thread's channel into
    /// `frames`/`pending`, since a single read can contain several frames
    /// or only part of one.
    pub fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        if let Some(msg) = self.pending.pop_front() {
            return Ok(Some(msg));
        }
        while let Ok(chunk) = self.rx.try_recv() {
            self.frames.extend(&chunk);
        }
        self.pending.extend(self.frames.drain::<ServerMessage>());
        Ok(self.pending.pop_front())
    }

    /// Whether the ssh bridge has closed (remote exit, network drop, or
    /// the local ssh failing). Buffered messages may still be drained via
    /// [`recv`](Self::recv) after this returns `true`.
    pub fn eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }
}

/// Pump the bridge's stdout into `tx` until the pipe closes, then flag
/// `eof` so callers can tell a dead tunnel from a quiet one.
fn pump_bridge_to_channel(
    mut stdout: ChildStdout,
    tx: mpsc::Sender<Vec<u8>>,
    eof: Arc<AtomicBool>,
) {
    let mut buf = [0u8; RECV_BUFFER_BYTES];
    loop {
        match stdout.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
        }
    }
    eof.store(true, Ordering::Release);
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_connect_returns_instance() {
        let result = RemoteClient::connect("nonexistent.host.invalid", None);
        assert!(result.is_ok());
        let mut client = result.unwrap();
        let _ = client.send(&super::protocol::ClientMessage::ListSessions);
    }

    #[test]
    fn test_eof_is_observable_once_the_bridge_dies() {
        // Regression: an exited ssh child read as an idle pipe (`Ok(0)`
        // mapped to `Ok(None)` like WouldBlock), so a caller could not
        // tell a dead tunnel from a quiet one and would poll it forever.
        let mut client = RemoteClient::connect("nonexistent.host.invalid", None).unwrap();
        assert!(!client.eof());
        let deadline = Instant::now() + Duration::from_secs(10);
        while !client.eof() && Instant::now() < deadline {
            let _ = client.recv().unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(
            client.eof(),
            "a dead ssh bridge must be observable, not just idle"
        );
    }
}

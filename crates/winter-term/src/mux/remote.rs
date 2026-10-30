//! Remote domain support: connects to a remote mux server over SSH.
//!
//! Runs `ssh -- <host> winter mux proxy <session>`, whose far end bridges
//! the ssh channel to the remote server's Unix socket, so no TCP port is
//! exposed. The host is passed after `--` so it can never be read as an ssh
//! option, and the session name is validated before it becomes a word on
//! the remote shell's command line.

use std::collections::VecDeque;
use std::io::Read;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use super::protocol::{self, ClientMessage, FrameBuffer, ServerMessage};
use super::transport::{self, MuxTransport};
use crate::terminal::session::spawn_pty_writer;

// ========================================================================
// Constants
// ========================================================================

/// Bytes moved per read by the background pipe pump.
const RECV_BUFFER_BYTES: usize = 8192;

/// The program that carries the bridge.
const SSH_PROGRAM: &str = "ssh";

/// Ends ssh's options, so the host after it is never read as one.
const SSH_END_OF_OPTIONS: &str = "--";

/// What the far side runs: the proxy subcommand of `winter`.
const REMOTE_PROXY_ARGS: &[&str] = &["winter", "mux", "proxy"];

/// The proxy flag asking the far server not to replay scrollback.
pub const NO_REPLAY_FLAG: &str = "--no-replay";

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
    /// Frames for the bridge's stdin, written by a thread of their own so a
    /// stalled ssh never blocks the caller (the UI thread, for a pane).
    writer: mpsc::Sender<Vec<u8>>,
    _reader: std::thread::JoinHandle<()>,
}

// ========================================================================
// RemoteClient
// ========================================================================

impl RemoteClient {
    /// Connect to `session` on the mux server at `host` over ssh. `replay`
    /// asks for the session's buffered output, as
    /// [`MuxClient::attach`](super::client::MuxClient::attach) does.
    pub fn connect(host: &str, session: &str, replay: bool) -> anyhow::Result<Self> {
        let mut child = bridge_command(host, session, replay)?
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (stdin, stdout) = match (child.stdin.take(), child.stdout.take()) {
            (Some(stdin), Some(stdout)) => (stdin, stdout),
            _ => anyhow::bail!("the ssh bridge did not expose piped stdio"),
        };

        // The ssh pipe is blocking and has no portable nonblocking mode,
        // so a reader thread pumps it into a channel: the same shape as
        // the local PTY transport, keeping `recv` nonblocking and EOF
        // observable.
        let (tx, rx) = mpsc::channel();
        let eof = Arc::new(AtomicBool::new(false));
        let reader_eof = Arc::clone(&eof);
        let reader = std::thread::spawn(move || pump_bridge_to_channel(stdout, tx, reader_eof));

        let mut client = RemoteClient {
            child,
            eof,
            frames: FrameBuffer::new(),
            pending: VecDeque::new(),
            rx,
            writer: spawn_pty_writer(Box::new(stdin))?,
            _reader: reader,
        };
        client.send(&transport::hello())?;
        Ok(client)
    }

    /// Send one request over the tunnel.
    pub fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        self.writer
            .send(protocol::encode(msg))
            .map_err(|_| anyhow::anyhow!("the ssh bridge no longer takes input"))
    }

    /// One buffered `ServerMessage` per call, oldest first. Never blocks:
    /// raw bytes are reassembled from the reader thread's channel into
    /// `frames`/`pending`, since a single read can contain several frames
    /// or only part of one. A handshake reply naming a version this client
    /// does not speak is an error.
    pub fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        if self.pending.is_empty() {
            while let Ok(chunk) = self.rx.try_recv() {
                self.frames.extend(&chunk);
            }
            self.pending.extend(self.frames.drain::<ServerMessage>());
        }
        let msg = self.pending.pop_front();
        if let Some(msg) = &msg {
            transport::check_handshake(msg)?;
        }
        Ok(msg)
    }

    /// Whether the ssh bridge has closed (remote exit, network drop, or
    /// the local ssh failing). Buffered messages may still be drained via
    /// [`recv`](Self::recv) after this returns `true`.
    pub fn eof(&self) -> bool {
        self.eof.load(Ordering::Acquire)
    }
}

impl MuxTransport for RemoteClient {
    fn eof(&self) -> bool {
        RemoteClient::eof(self)
    }

    fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>> {
        RemoteClient::recv(self)
    }

    fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()> {
        RemoteClient::send(self, msg)
    }
}

impl Drop for RemoteClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

// ========================================================================
// Bridge
// ========================================================================

/// The ssh invocation bridging to `session` on `host`, refused when the
/// host could be read as an option or the session name is not one.
fn bridge_command(host: &str, session: &str, replay: bool) -> anyhow::Result<Command> {
    if host.is_empty() || host.starts_with('-') {
        anyhow::bail!("'{host}' is not a host name");
    }
    protocol::validate_session_name(session).map_err(anyhow::Error::msg)?;
    let mut command = Command::new(SSH_PROGRAM);
    command
        .arg(SSH_END_OF_OPTIONS)
        .arg(host)
        .args(REMOTE_PROXY_ARGS)
        .arg(session);
    if !replay {
        command.arg(NO_REPLAY_FLAG);
    }
    Ok(command)
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_the_bridge_keeps_host_and_session_out_of_option_and_shell_syntax() {
        let command = bridge_command("example.org", "work", false).unwrap();
        let args: Vec<_> = command.get_args().collect();
        assert_eq!(
            args,
            [
                "--",
                "example.org",
                "winter",
                "mux",
                "proxy",
                "work",
                NO_REPLAY_FLAG
            ]
        );
        assert!(bridge_command("-oProxyCommand=touch /tmp/x", "work", true).is_err());
        assert!(bridge_command("example.org", "w; rm -rf ~", true).is_err());
    }

    #[test]
    fn test_connect_returns_instance() {
        let result = RemoteClient::connect("nonexistent.host.invalid", "default", true);
        assert!(result.is_ok());
        let mut client = result.unwrap();
        let _ = client.send(&ClientMessage::ListSessions);
    }

    #[test]
    fn test_eof_is_observable_once_the_bridge_dies() {
        // Regression: an exited ssh child read as an idle pipe (`Ok(0)`
        // mapped to `Ok(None)` like WouldBlock), so a caller could not
        // tell a dead tunnel from a quiet one and would poll it forever.
        let mut client =
            RemoteClient::connect("nonexistent.host.invalid", "default", true).unwrap();
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

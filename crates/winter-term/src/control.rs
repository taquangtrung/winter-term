//! Control channel: a tiny IPC socket the windowed app listens on so an
//! external `winter --reload` invocation can ask a running instance to save
//! its session, relaunch itself, and exit. Reuses the mux subsystem's
//! length-prefixed JSON framing ([`crate::mux::protocol`]) rather than
//! inventing a second wire format, but is otherwise unrelated to mux: this
//! socket carries one-shot control requests, not multiplexed PTY sessions.

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use serde::{Deserialize, Serialize};

use crate::mux::protocol::{decode, encode};

// ========================================================================
// Constants
// ========================================================================

/// How long the listener waits for a connected peer to send its message and
/// close, before giving up. Without it, a connection that never closes
/// wedges `read_to_end` forever, dropping every later `--reload` request.
const READ_TIMEOUT: Duration = Duration::from_secs(5);

// ========================================================================
// Data Structures
// ========================================================================

/// A message sent over the control channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum ControlMessage {
    /// Save the session, relaunch a fresh instance, and exit.
    Reload,
}

// ========================================================================
// Socket path
// ========================================================================

/// The control-channel socket path. Distinct from the mux server's socket
/// ([`crate::mux::server::default_socket_path`]) so the two never collide.
pub fn socket_path() -> String {
    match crate::paths::runtime_dir() {
        Some(dir) => format!("{dir}/winter-control.sock"),
        None => "/tmp/winter-control.sock".to_string(),
    }
}

// ========================================================================
// Listener (GUI-side)
// ========================================================================

/// Start listening on the control socket in a background thread, returning
/// a receiver a poller can drain for incoming messages. Returns `None` when
/// another instance already owns the socket, so only the first-launched
/// instance accepts control messages; a `--reload` request always targets
/// that one.
pub(crate) fn spawn_listener() -> Option<mpsc::Receiver<ControlMessage>> {
    let listener = bind_fresh(&socket_path())?;
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || loop {
        let Ok((stream, _)) = listener.accept() else {
            continue;
        };
        let Some(msg) = read_message(stream) else {
            continue;
        };
        if tx.send(msg).is_err() {
            break;
        }
    });
    Some(rx)
}

/// Bind the socket, clearing a stale file left by a crashed instance but
/// leaving a live listener alone: if connecting to the existing path
/// succeeds, another instance already owns it.
fn bind_fresh(path: &str) -> Option<UnixListener> {
    match UnixListener::bind(path) {
        Ok(listener) => Some(listener),
        Err(_) if UnixStream::connect(path).is_ok() => None,
        Err(_) => {
            let _ = std::fs::remove_file(path);
            UnixListener::bind(path).ok()
        }
    }
}

fn read_message(stream: UnixStream) -> Option<ControlMessage> {
    read_message_with_timeout(stream, READ_TIMEOUT)
}

fn read_message_with_timeout(mut stream: UnixStream, timeout: Duration) -> Option<ControlMessage> {
    stream.set_read_timeout(Some(timeout)).ok()?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).ok()?;
    decode(&buf)
}

// ========================================================================
// Client (CLI-side)
// ========================================================================

/// Connect to a running instance's control socket and ask it to reload.
pub fn request_reload(path: &str) -> anyhow::Result<()> {
    let mut stream = UnixStream::connect(path)?;
    stream.write_all(&encode(&ControlMessage::Reload))?;
    Ok(())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_control_message_round_trips_through_mux_framing() {
        let encoded = encode(&ControlMessage::Reload);
        let decoded: ControlMessage = decode(&encoded).expect("decode");
        assert!(matches!(decoded, ControlMessage::Reload));
    }

    #[test]
    fn test_bind_fresh_skips_a_socket_a_live_listener_already_owns() {
        let path = std::env::temp_dir()
            .join(format!(
                "winter-control-test-{:?}.sock",
                thread::current().id()
            ))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);

        let first = bind_fresh(&path).expect("first bind should succeed");
        assert!(
            bind_fresh(&path).is_none(),
            "a live listener must not be displaced"
        );

        drop(first);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_read_message_gives_up_on_a_stalled_connection_instead_of_hanging_forever() {
        // Regression: `read_to_end` had no timeout, so a connection that
        // never closes (a hung or misbehaving client) wedged the listener
        // thread forever, silently dropping every later `--reload` request.
        let path = std::env::temp_dir()
            .join(format!(
                "winter-control-stall-test-{:?}.sock",
                thread::current().id()
            ))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");

        // Held open and never written to or closed, simulating a stalled peer.
        let _client = UnixStream::connect(&path).expect("connect");
        let (accepted, _) = listener.accept().expect("accept");

        let start = std::time::Instant::now();
        let result = read_message_with_timeout(accepted, Duration::from_millis(50));
        assert!(result.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "must give up quickly instead of hanging forever"
        );

        let _ = std::fs::remove_file(&path);
    }
}

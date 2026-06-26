//! Control channel: tiny IPC sockets the windowed app listens on so an
//! external `winter --reload` invocation can ask a running instance to save
//! its session, relaunch itself, and exit, and a `winter --edit-wait` in one
//! of its panes can have it edit a file. Every window listens on a socket of
//! its own, which the `GIT_EDITOR` of its panes names, so a commit started in
//! one window is edited in that window; the first window launched also owns
//! the shared socket `--reload` targets. Reuses the mux subsystem's
//! length-prefixed JSON framing ([`crate::mux::protocol`]) rather than
//! inventing a second wire format, but is otherwise unrelated to mux: this
//! socket carries one-shot control requests, not multiplexed PTY sessions.

use std::io::{Read, Write};
#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
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

/// The largest control request read, in bytes: a request names a file at
/// most, so anything longer is not one.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

/// How long the listener waits after a failed `accept` before trying again.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// The byte answering a waiting `--edit-wait`: the edit stands.
const ANSWER_ACCEPTED: u8 = 1;

/// The byte answering a waiting `--edit-wait`: the edit was discarded.
const ANSWER_REJECTED: u8 = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// A message sent over the control channel.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) enum ControlMessage {
    /// Open this file in the app's own editor and answer once it is closed.
    /// The sender is `winter --edit-wait`, standing in for `$EDITOR` under a
    /// program (`git commit`) that blocks until the editor exits.
    Edit {
        /// The file to edit.
        path: PathBuf,
    },
    /// Save the session, relaunch a fresh instance, and exit.
    Reload,
}

/// What [`spawn_listener`] started: the requests the control sockets deliver,
/// and this window's own socket file.
#[derive(Debug)]
pub(crate) struct ControlListener {
    /// `None` when no socket could be bound, so no request ever arrives.
    pub(crate) requests: Option<mpsc::Receiver<ControlRequest>>,
    /// `None` when this window's own socket could not be bound.
    pub(crate) socket: Option<SocketFile>,
}

/// A [`ControlMessage`] with the connection it arrived on, kept open so a
/// message that waits for an answer can give one.
#[derive(Debug)]
pub(crate) struct ControlRequest {
    pub(crate) message: ControlMessage,
    pub(crate) stream: UnixStream,
}

/// Answers a waiting `winter --edit-wait` when dropped, so the program blocked
/// on the editor continues however the editing tool came to an end: closed,
/// replaced, or torn down with the app.
#[derive(Debug)]
pub(crate) struct EditWaiter {
    /// Whether the edit stands; `false` tells the waiter to abort what it was
    /// editing for.
    accepted: bool,
    stream: UnixStream,
}

/// A socket file this window bound, removed when the window closes so
/// closed windows leave none behind.
#[derive(Debug)]
pub(crate) struct SocketFile {
    path: String,
}

// ========================================================================
// Socket path
// ========================================================================

/// The shared control-channel socket path, owned by the first window
/// launched. Distinct from the mux server's socket
/// ([`crate::mux::server::default_socket_path`]) so the two never collide.
pub fn socket_path() -> String {
    private_socket_path("winter-control.sock")
}

/// This window's own control socket path. Named after the process ID, which
/// a reload keeps on Unix (it `exec`s in place), so the panes it hands over
/// still reach it.
pub(crate) fn instance_socket_path() -> String {
    private_socket_path(&format!("winter-control-{}.sock", std::process::id()))
}

fn private_socket_path(name: &str) -> String {
    let dir = crate::paths::private_dir();
    format!(
        "{}/{name}",
        dir.as_deref().unwrap_or(crate::paths::UNUSABLE_DIR)
    )
}

// ========================================================================
// Listener (GUI-side)
// ========================================================================

/// Start listening on this window's own control socket, and on the shared
/// one unless another instance already owns it, each in a background thread
/// feeding one receiver a poller can drain. Only the first-launched instance
/// owns the shared socket, so a `--reload` request always targets that one.
pub(crate) fn spawn_listener() -> ControlListener {
    let own_path = instance_socket_path();
    let own = bind_fresh(&own_path);
    let socket = own.is_some().then_some(SocketFile { path: own_path });
    let (tx, rx) = mpsc::channel();
    let mut listening = false;
    for listener in [bind_fresh(&socket_path()), own].into_iter().flatten() {
        spawn_accept_loop(listener, tx.clone());
        listening = true;
    }
    ControlListener {
        requests: listening.then_some(rx),
        socket,
    }
}

/// Accept connections on `listener` in a background thread, sending each
/// request read to `tx` until the receiver is gone.
fn spawn_accept_loop(listener: UnixListener, tx: mpsc::Sender<ControlRequest>) {
    thread::spawn(move || loop {
        let Ok((stream, _)) = listener.accept() else {
            // Out of file descriptors, most likely: retrying at once would
            // spin a core until one frees up.
            thread::sleep(ACCEPT_RETRY_DELAY);
            continue;
        };
        let Some(request) = read_request(stream) else {
            continue;
        };
        if tx.send(request).is_err() {
            break;
        }
    });
}

/// Bind the socket, clearing a stale file left by a crashed instance but
/// leaving a live listener alone: if connecting to the existing path
/// succeeds, another instance already owns it.
fn bind_fresh(path: &str) -> Option<UnixListener> {
    let listener = match UnixListener::bind(path) {
        Ok(listener) => listener,
        Err(_) if UnixStream::connect(path).is_ok() => return None,
        Err(_) => {
            let _ = std::fs::remove_file(path);
            UnixListener::bind(path).ok()?
        }
    };
    // This user's alone: whoever can connect can ask this window to reload or
    // to open files.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).ok()?;
    }
    Some(listener)
}

fn read_request(stream: UnixStream) -> Option<ControlRequest> {
    read_request_with_timeout(stream, READ_TIMEOUT)
}

fn read_request_with_timeout(mut stream: UnixStream, timeout: Duration) -> Option<ControlRequest> {
    stream.set_read_timeout(Some(timeout)).ok()?;
    let mut buf = Vec::new();
    // A request is a path at most; a sender writing more is not one.
    (&mut stream)
        .take(MAX_REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() > MAX_REQUEST_BYTES {
        return None;
    }
    let message = decode(&buf)?;
    // The deadline was for the request; the answer is written, never read.
    stream.set_read_timeout(None).ok()?;
    Some(ControlRequest { message, stream })
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

/// Ask a running instance to edit `file` and wait until its editing tool
/// closes. Whether the edit was kept comes back; an error means no instance
/// could be reached or it went away without answering.
///
/// A relative `file` is resolved here, against this process's directory:
/// the instance runs somewhere else, and `git` names its message file
/// relative to the repository it runs in (`.git/COMMIT_EDITMSG`).
pub fn request_edit(path: &str, file: PathBuf) -> anyhow::Result<bool> {
    let file = std::path::absolute(&file)?;
    let mut stream = UnixStream::connect(path)?;
    stream.write_all(&encode(&ControlMessage::Edit { path: file }))?;
    stream.shutdown(std::net::Shutdown::Write)?;
    let mut answer = Vec::new();
    stream.read_to_end(&mut answer)?;
    anyhow::ensure!(!answer.is_empty(), "the app closed without answering");
    Ok(answer == [ANSWER_ACCEPTED])
}

// ========================================================================
// EditWaiter
// ========================================================================

impl EditWaiter {
    pub(crate) fn new(stream: UnixStream) -> Self {
        Self {
            accepted: true,
            stream,
        }
    }

    /// Answer that the edit was thrown away, so the waiter exits with failure.
    pub(crate) fn reject(&mut self) {
        self.accepted = false;
    }
}

impl Drop for EditWaiter {
    fn drop(&mut self) {
        let answer = match self.accepted {
            true => ANSWER_ACCEPTED,
            false => ANSWER_REJECTED,
        };
        let _ = self.stream.write_all(&[answer]);
    }
}

// ========================================================================
// SocketFile
// ========================================================================

impl Drop for SocketFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
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
        let result = read_request_with_timeout(accepted, Duration::from_millis(50));
        assert!(result.is_none());
        assert!(
            start.elapsed() < Duration::from_secs(2),
            "must give up quickly instead of hanging forever"
        );

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_edit_request_waits_for_the_answer_and_reports_a_rejection() {
        let path = std::env::temp_dir()
            .join(format!(
                "winter-control-edit-test-{:?}.sock",
                thread::current().id()
            ))
            .to_string_lossy()
            .into_owned();
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).expect("bind");

        let client_path = path.clone();
        let client = thread::spawn(move || request_edit(&client_path, PathBuf::from("msg")));

        // Regression: the relative name went over as given, and the instance
        // opened it against its own directory instead of the caller's.
        let expected = std::env::current_dir().expect("cwd").join("msg");
        let (accepted, _) = listener.accept().expect("accept");
        let request = read_request(accepted).expect("request");
        assert!(matches!(
            request.message,
            ControlMessage::Edit { ref path } if path == &expected
        ));
        assert!(!client.is_finished(), "the client waits for the answer");

        let mut waiter = EditWaiter::new(request.stream);
        waiter.reject();
        drop(waiter);
        assert!(!client.join().expect("joins").expect("answered"));

        let _ = std::fs::remove_file(&path);
    }
}

//! Mux server: listens on a Unix domain socket, manages PTY sessions,
//! and routes output to connected clients.
//!
//! Usage:
//!
//! ```text
//! Winter mux serve
//! Winter mux attach default
//! ```
//!
//! Nothing here polls. A listener thread accepts connections, each client
//! gets a reader and a writer thread ([`connection`]), and each session a
//! reader thread ([`SessionManager`]); all of them report into one inbox,
//! and the event loop in [`MuxServer::run`] blocks on it, handing each
//! event to the [`dispatch`] state machine. Session sizing lives in
//! [`size`].

mod connection;
mod dispatch;
mod size;

#[cfg(unix)]
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::{Arc, mpsc};
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use uds_windows::{UnixListener, UnixStream};

use self::dispatch::Dispatcher;
use super::client::{MuxClient, Probe};
use super::persist::{self, SessionDefs};
use super::protocol::ClientMessage;
use super::session::{EventSink, SessionEvent, SessionManager};

// ========================================================================
// Constants
// ========================================================================

/// How long [`is_socket_live`] waits for a server to answer its probe before
/// declaring the socket stale. Paid once at server startup, and only when a
/// socket file is already there, so it can be generous enough to survive a
/// loaded machine without being felt in the common case.
const LIVENESS_PROBE_TIMEOUT: Duration = Duration::from_millis(500);

/// How long the listener waits after a failed `accept` before trying again.
/// Out of file descriptors, most likely: retrying at once would spin a core
/// until one frees up.
const ACCEPT_RETRY_DELAY: Duration = Duration::from_millis(100);

/// The socket's file name inside the private runtime directory.
const SOCKET_FILE_NAME: &str = "winter-mux.sock";

/// What the socket's path takes on to name the file holding the serving
/// process's id.
const PID_FILE_SUFFIX: &str = ".pid";

// ========================================================================
// Data Structures
// ========================================================================

/// The mux server: owns every PTY session and routes output to clients.
pub struct MuxServer {
    path: String,
}

/// Everything the event loop reacts to, from whichever thread saw it.
enum Event {
    /// A client's connection ended (or was found unusable).
    Closed(u64),
    /// The listener accepted a new connection.
    Connected(UnixStream),
    /// A client sent a frame.
    Frame(ClientFrame),
    /// A session's program wrote output or exited.
    Session(SessionEvent),
}

/// One frame read from a client: the request, or why it did not decode.
struct ClientFrame {
    cid: u64,
    frame: Result<ClientMessage, String>,
}

// ========================================================================
// MuxServer
// ========================================================================

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
        // So a window of another version, which cannot speak with this
        // server, can still end it (see [`server_pid`]).
        std::fs::write(pid_file(&self.path), std::process::id().to_string())?;
        let (events, inbox) = mpsc::channel::<Event>();

        let session_events = events.clone();
        let sink: EventSink = Arc::new(move |event| {
            let _ = session_events.send(Event::Session(event));
        });
        let mut dispatcher = Dispatcher::new(SessionManager::new(sink));
        dispatcher.respawn(persist::load_defs().sessions);
        persist_if_changed(&mut dispatcher);

        let accepted = events.clone();
        thread::spawn(move || accept_connections(listener, accepted));

        let mut next_client_id: u64 = 1;
        // `events` stays alive here, so the inbox never runs dry: the loop
        // blocks until something happens and ends only with the process.
        for event in inbox {
            match event {
                Event::Closed(cid) => dispatcher.remove_client(cid),
                Event::Connected(stream) => {
                    match connection::open(next_client_id, stream, events.clone()) {
                        Ok(outbox) => dispatcher.add_client(next_client_id, outbox),
                        Err(e) => eprintln!("winter mux: could not serve a connection: {e}"),
                    }
                    next_client_id += 1;
                }
                Event::Frame(ClientFrame { cid, frame }) => dispatcher.handle_frame(cid, frame),
                Event::Session(event) => dispatcher.handle_session_event(event),
            }
            dispatcher.reap_doomed();
            persist_if_changed(&mut dispatcher);
        }
        Ok(())
    }
}

impl Drop for MuxServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(pid_file(&self.path));
    }
}

// ========================================================================
// Listener
// ========================================================================

/// Hand every accepted connection to the event loop, until it is gone.
fn accept_connections(listener: UnixListener, events: mpsc::Sender<Event>) {
    loop {
        match listener.accept() {
            Ok((stream, _)) => {
                if events.send(Event::Connected(stream)).is_err() {
                    return;
                }
            }
            Err(_) => thread::sleep(ACCEPT_RETRY_DELAY),
        }
    }
}

/// Write every live session's respawn recipe to the defs file when the set
/// of sessions changed, dropping whichever ones no longer exist (killed
/// explicitly, or exited on their own) so a restart does not bring them back.
fn persist_if_changed(dispatcher: &mut Dispatcher) {
    if dispatcher.take_sessions_changed() {
        persist::save_defs(&SessionDefs {
            sessions: dispatcher.session_defs(),
        });
    }
}

/// Bind the server socket at `path`, replacing any stale socket file a
/// crashed server left behind, and restrict it to this user: whoever can
/// connect can attach to (and inject input into) every session.
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

/// Whether a server is currently *serving* on `path`.
///
/// The file existing proves nothing: a crashed server leaves its socket
/// behind. Connecting proves almost nothing either, because a connect to a
/// socket whose listener has just closed can still succeed while the kernel
/// tears the endpoint down; probing with a bare connect made this flap, which
/// would turn a legitimate restart after a crash into "already listening".
///
/// So liveness is confirmed by an actual protocol exchange: the handshake's
/// answer. Only a running server answers, and one of another version that
/// refuses ours is as live as one that does not: displacing it would strand
/// its sessions.
fn is_socket_live(path: &str) -> bool {
    if !std::path::Path::new(path).exists() {
        return false;
    }
    MuxClient::probe(path, LIVENESS_PROBE_TIMEOUT) != Probe::Absent
}

/// The process id of the server serving on the socket at `path`, as it
/// wrote it on starting; `None` for a server too old to write it. Ask only
/// while a server answers there, since a server that crashed leaves the
/// file behind.
pub fn server_pid(path: &str) -> Option<u32> {
    std::fs::read_to_string(pid_file(path))
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// The file beside the socket at `path` that holds its server's process id.
fn pid_file(path: &str) -> String {
    format!("{path}{PID_FILE_SUFFIX}")
}

/// The socket path used when the user names no other.
pub fn default_socket_path() -> String {
    let dir = winter_base::paths::private_dir();
    format!(
        "{}/{SOCKET_FILE_NAME}",
        dir.as_deref().unwrap_or(winter_base::paths::UNUSABLE_DIR)
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};

    use super::*;
    use crate::protocol::{self, ServerMessage};

    /// A socket at a fresh temporary path named after `name`.
    fn socket_path(name: &str) -> String {
        let path = std::env::temp_dir()
            .join(format!("winter-mux-{name}-{}.sock", std::process::id()))
            .to_string_lossy()
            .to_string();
        let _ = std::fs::remove_file(&path);
        path
    }

    /// Serve `count` connections on `listener`, answering each handshake
    /// with `reply`, as a server of this version or another would.
    fn answer(
        listener: UnixListener,
        reply: ServerMessage,
        count: usize,
    ) -> thread::JoinHandle<()> {
        thread::spawn(move || {
            for _ in 0..count {
                if let Ok((mut conn, _)) = listener.accept() {
                    let mut buf = [0u8; 1024];
                    let _ = conn.read(&mut buf);
                    let _ = conn.write_all(&protocol::encode(&reply));
                }
            }
        })
    }

    #[test]
    fn test_bind_refuses_to_displace_a_live_server() {
        // Regression: `bind_socket` unlinked the path unconditionally, so a
        // second `winter mux serve` stole it and stranded the first server's
        // sessions: still running, still holding their PTYs, unreachable.
        let path = socket_path("bind");
        let first = bind_socket(&path).expect("first bind succeeds");
        // Liveness is confirmed by a protocol reply, not a bare connect, so
        // the stand-in server has to actually answer one.
        let hello = ServerMessage::Hello {
            version: protocol::MUX_PROTOCOL_VERSION,
        };
        let responder = answer(first, hello, 1);

        let displaced = bind_socket(&path);
        assert!(
            displaced.is_err(),
            "a second bind must not displace a serving mux"
        );
        responder.join().expect("responder thread panicked");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_a_server_of_another_version_is_found_mismatched_and_never_displaced() {
        let path = socket_path("other");
        let first = bind_socket(&path).expect("first bind succeeds");
        let message = "mux protocol version 1 is not supported".to_string();
        let responder = answer(first, ServerMessage::Error { message }, 2);

        let probed = MuxClient::probe(&path, LIVENESS_PROBE_TIMEOUT);
        let displaced = bind_socket(&path);
        responder.join().expect("responder thread panicked");
        let _ = std::fs::remove_file(&path);
        assert!(
            matches!(&probed, Probe::Mismatched(why) if why.contains("not supported")),
            "{probed:?}"
        );
        assert!(
            displaced.is_err(),
            "a server refusing our version still holds its sessions"
        );
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
}

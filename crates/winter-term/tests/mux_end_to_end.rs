//! End-to-end multiplexer test: a real [`MuxServer`] on a real Unix socket,
//! driven by a real [`MuxClient`].
//!
//! The unit tests around the mux either drive the server's message handlers
//! directly or stand a hand-written socket in for one side. Neither exercises
//! the actual composition: bind, accept, frame, spawn a PTY, pump its output
//! back through the socket, and reassemble it on the client. This does.

#![cfg(unix)]

use std::time::{Duration, Instant};

use winter_app::mux::client::MuxClient;
use winter_app::mux::protocol::ServerMessage;
use winter_app::mux::server::MuxServer;

// ========================================================================
// Constants
// ========================================================================

/// Long enough to cover PTY spawn plus a shell writing one line on a loaded
/// CI runner, short enough that a genuine hang fails the test rather than
/// stalling the suite.
const REPLY_TIMEOUT: Duration = Duration::from_secs(20);

/// The server's accept loop polls; give it room to notice a new connection.
const POLL_INTERVAL: Duration = Duration::from_millis(20);

// ========================================================================
// Harness
// ========================================================================

/// A socket path and state directory unique to this test process, so a
/// concurrent run (or a real `winter mux serve` on this machine) cannot
/// collide with it.
struct TestPaths {
    socket: String,
    state_dir: std::path::PathBuf,
}

impl TestPaths {
    fn new() -> Self {
        let unique = format!("winter-mux-e2e-{}", std::process::id());
        let state_dir = std::env::temp_dir().join(&unique);
        std::fs::create_dir_all(&state_dir).expect("create state dir");
        Self {
            socket: std::env::temp_dir()
                .join(format!("{unique}.sock"))
                .to_string_lossy()
                .to_string(),
            state_dir,
        }
    }
}

impl Drop for TestPaths {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_dir_all(&self.state_dir);
    }
}

/// Poll `client` until a message satisfying `want` arrives, or fail.
fn wait_for(
    client: &mut MuxClient,
    what: &str,
    mut want: impl FnMut(&ServerMessage) -> bool,
) -> ServerMessage {
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        while let Some(msg) = client.recv().expect("client read failed") {
            if want(&msg) {
                return msg;
            }
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what} after {REPLY_TIMEOUT:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[test]
fn test_session_spawns_and_streams_its_output_back_over_the_socket() {
    let paths = TestPaths::new();

    // Point session persistence at a scratch directory before the server
    // starts. Without this the server would load (and respawn) whatever
    // sessions the developer running the suite happens to have persisted.
    std::env::set_var("XDG_STATE_HOME", &paths.state_dir);

    let socket = paths.socket.clone();
    // The server's run loop is infinite by design; the thread is left to be
    // reaped with the test process rather than given a shutdown path that
    // exists only for tests.
    std::thread::spawn(move || {
        let _ = MuxServer::new(&socket).run();
    });

    let mut client = {
        let deadline = Instant::now() + REPLY_TIMEOUT;
        loop {
            match MuxClient::connect(&paths.socket) {
                Ok(client) => break client,
                Err(e) => {
                    assert!(Instant::now() < deadline, "server never came up: {e}");
                    std::thread::sleep(POLL_INTERVAL);
                }
            }
        }
    };

    let (cols, rows) = client
        .spawn_confirmed(
            "e2e",
            100,
            30,
            None,
            // Prints, then stays alive: a command that exits immediately
            // takes its session down with it, and the listing below would
            // race the teardown.
            Some("echo winter-e2e-marker; sleep 30"),
            REPLY_TIMEOUT,
        )
        .expect("session should start");
    assert_eq!(
        (cols, rows),
        (100, 30),
        "confirmed geometry is what we asked for"
    );

    let mut seen = Vec::new();
    wait_for(&mut client, "the session's output", |msg| match msg {
        ServerMessage::Output { bytes, .. } | ServerMessage::Scrollback { bytes, .. } => {
            seen.extend_from_slice(bytes);
            String::from_utf8_lossy(&seen).contains("winter-e2e-marker")
        }
        _ => false,
    });

    let sessions = client
        .query_sessions(REPLY_TIMEOUT)
        .expect("listing should succeed");
    let listed = sessions
        .iter()
        .find(|s| s.name == "e2e")
        .expect("the spawned session should be listed");
    assert_eq!((listed.cols, listed.rows), (100, 30));
    assert!(
        listed.attach_count >= 1,
        "spawning attaches, so the count should include us"
    );

    // Kill, then confirm the session is really gone before the test process
    // falls off the end: an unapplied kill would leave the session's PTY child
    // orphaned past the end of the suite. `kill` drops the session rather than
    // reporting an `Exit`, so the listing is what confirms it.
    client.kill("e2e").expect("kill should send");
    let deadline = Instant::now() + REPLY_TIMEOUT;
    loop {
        let remaining = client
            .query_sessions(REPLY_TIMEOUT)
            .expect("listing should succeed");
        if !remaining.iter().any(|s| s.name == "e2e") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "session survived the kill after {REPLY_TIMEOUT:?}"
        );
        std::thread::sleep(POLL_INTERVAL);
    }
}

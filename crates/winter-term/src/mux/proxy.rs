//! Mux proxy: bridges stdin/stdout to the local mux server's socket so a
//! remote client can speak the mux protocol through an SSH pipe.

use std::io::{Read, Write};
use std::net::Shutdown;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
#[cfg(windows)]
use uds_windows::UnixStream;

use super::protocol::{self, ClientMessage};
use super::server;

// ========================================================================
// Constants
// ========================================================================

/// Bytes moved per read in either pump direction.
const PROXY_BUFFER_BYTES: usize = 8192;

// ========================================================================
// Implementation
// ========================================================================

/// Bridge the local mux server to this process's stdin/stdout, attaching
/// to `session` on the remote client's behalf.
pub fn run(session: &str) -> anyhow::Result<()> {
    let mut stream = connect_and_attach(&server::default_socket_path(), session)?;

    // The socket→stdout pump runs on its own thread so both directions
    // stream concurrently; stdin→socket stays on the main thread.
    let mut reader = stream.try_clone()?;
    let pump = std::thread::spawn(move || pump_socket_to_stdout(&mut reader));

    let stdin = std::io::stdin();
    let mut input = stdin.lock();
    let mut buf = [0u8; PROXY_BUFFER_BYTES];
    loop {
        match input.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                if stream.write_all(&buf[..n]).is_err() {
                    break;
                }
                let _ = stream.flush();
            }
        }
    }

    // stdin EOF means the remote client is gone: closing the write half
    // makes the server drop this connection, which ends the stdout pump
    // instead of leaving it blocked on a dead socket.
    let _ = stream.shutdown(Shutdown::Write);
    let _ = pump.join();
    Ok(())
}

/// Connect to the mux server and attach to `session`. The server treats a
/// repeat attach as a no-op, so a client that sends its own `Attach` over
/// the bridge is unaffected.
fn connect_and_attach(path: &str, session: &str) -> anyhow::Result<UnixStream> {
    let mut stream = UnixStream::connect(path)?;
    let attach = protocol::encode(&ClientMessage::Attach {
        session: session.to_string(),
    });
    stream.write_all(&attach)?;
    stream.flush()?;
    Ok(stream)
}

fn pump_socket_to_stdout(reader: &mut UnixStream) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let mut buf = [0u8; PROXY_BUFFER_BYTES];
    loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => {
                if out.write_all(&buf[..n]).is_err() {
                    return;
                }
                let _ = out.flush();
            }
        }
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
    fn test_connect_and_attach_sends_attach_frame_to_server() {
        let path =
            std::env::temp_dir().join(format!("winter-mux-proxy-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();

        let socket_path = path.to_string_lossy().to_string();
        let handle = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut json = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut json).unwrap();
            let mut framed = len_bytes.to_vec();
            framed.extend(json);
            protocol::decode::<ClientMessage>(&framed)
        });

        let _stream = connect_and_attach(&socket_path, "remote-session").unwrap();
        let msg = handle.join().unwrap().expect("frame must decode");
        assert!(matches!(
            msg,
            ClientMessage::Attach { ref session } if session == "remote-session"
        ));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_connect_and_attach_reports_missing_server() {
        assert!(connect_and_attach("/tmp/winter-mux-proxy-missing-test.sock", "s").is_err());
    }
}

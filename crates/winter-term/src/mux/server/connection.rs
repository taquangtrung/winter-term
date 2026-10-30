//! Client connections: each socket gets a reader thread, which decodes the
//! client's frames into the server's inbox, and a writer thread, which
//! drains a bounded [`Outbox`] onto the socket. Both block on their own
//! socket, so a slow or silent client stalls neither the event loop nor any
//! other client.

use std::io::{self, Read, Write};
use std::net::Shutdown;
#[cfg(unix)]
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;
use std::time::Duration;
#[cfg(windows)]
use uds_windows::UnixStream;

use super::super::protocol::{ClientMessage, FrameBuffer};
use super::{ClientFrame, Event};

// ========================================================================
// Constants
// ========================================================================

/// Per-client cap on queued outgoing bytes. A client this far behind is
/// dropped rather than buffered without bound: the server's memory is not
/// traded for the illusion of a live connection. It must hold a full
/// scrollback replay with room to spare (see the dispatcher's tests).
pub(super) const CLIENT_OUTBOX_LIMIT: usize = 4 << 20;

/// How long one write to a client may block before the client is taken to
/// be gone. Without it a peer that stopped reading would hold its writer
/// thread forever.
const CLIENT_WRITE_TIMEOUT: Duration = Duration::from_secs(10);

/// How much one read from a client takes.
const READ_CHUNK_BYTES: usize = 8192;

// ========================================================================
// Data Structures
// ========================================================================

/// The queue of encoded frames waiting for one client's writer thread.
/// Dropping it lets the writer finish what is queued, then close the
/// connection.
pub(super) struct Outbox {
    frames: mpsc::Sender<Vec<u8>>,
    /// Bytes queued and not yet written, shared with the writer thread.
    pending: Arc<AtomicUsize>,
}

// ========================================================================
// Outbox
// ========================================================================

impl Outbox {
    /// An outbox feeding `frames`.
    pub(super) fn new(frames: mpsc::Sender<Vec<u8>>) -> Self {
        Outbox {
            frames,
            pending: Arc::new(AtomicUsize::new(0)),
        }
    }

    /// Queue an encoded frame; `false` when doing so would exceed
    /// [`CLIENT_OUTBOX_LIMIT`] (the client is too slow to keep up) or the
    /// writer is gone.
    pub(super) fn push(&self, frame: Vec<u8>) -> bool {
        let len = frame.len();
        if self.pending.load(Ordering::Acquire) + len > CLIENT_OUTBOX_LIMIT {
            return false;
        }
        self.pending.fetch_add(len, Ordering::AcqRel);
        self.frames.send(frame).is_ok()
    }
}

// ========================================================================
// Connection threads
// ========================================================================

/// Start serving client `cid` on `stream`: its frames arrive in `events`,
/// and what is pushed to the returned outbox is written back to it.
pub(super) fn open(
    cid: u64,
    stream: UnixStream,
    events: mpsc::Sender<Event>,
) -> io::Result<Outbox> {
    let reader = stream.try_clone()?;
    stream.set_write_timeout(Some(CLIENT_WRITE_TIMEOUT))?;
    let (frames, queued) = mpsc::channel();
    let outbox = Outbox::new(frames);
    let pending = Arc::clone(&outbox.pending);
    thread::spawn(move || write_frames(stream, queued, pending));
    thread::spawn(move || read_frames(cid, reader, events));
    Ok(outbox)
}

/// Decode `stream` into `events` until it ends or declares a frame too
/// large to be real, then report the connection closed.
fn read_frames(cid: u64, mut stream: UnixStream, events: mpsc::Sender<Event>) {
    let mut frames = FrameBuffer::new();
    let mut buf = [0u8; READ_CHUNK_BYTES];
    loop {
        let read = match stream.read(&mut buf) {
            Ok(0) => break,
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        frames.extend(&buf[..read]);
        for frame in frames.drain_results::<ClientMessage>() {
            if events
                .send(Event::Frame(ClientFrame { cid, frame }))
                .is_err()
            {
                return;
            }
        }
        if frames.is_broken() {
            break;
        }
    }
    let _ = events.send(Event::Closed(cid));
}

/// Write queued frames to `stream` in order until the outbox is dropped or
/// the client stops taking them, then close the connection both ways, which
/// also ends its reader thread.
fn write_frames(
    mut stream: UnixStream,
    queued: mpsc::Receiver<Vec<u8>>,
    pending: Arc<AtomicUsize>,
) {
    for frame in queued {
        if stream.write_all(&frame).is_err() {
            break;
        }
        pending.fetch_sub(frame.len(), Ordering::AcqRel);
    }
    let _ = stream.shutdown(Shutdown::Both);
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;
    use crate::mux::protocol::{self, ServerMessage};

    /// Long enough for a thread to be scheduled on a loaded machine.
    const EVENT_TIMEOUT: Duration = Duration::from_secs(5);

    fn read_one_frame(peer: &mut UnixStream) -> Vec<u8> {
        let mut len_bytes = [0u8; 4];
        peer.read_exact(&mut len_bytes).unwrap();
        let mut framed = len_bytes.to_vec();
        let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
        peer.read_exact(&mut body).unwrap();
        framed.extend(body);
        framed
    }

    #[test]
    fn test_a_connection_delivers_both_ways_and_reports_its_end() {
        let (server_end, mut peer) = UnixStream::pair().unwrap();
        let (events, inbox) = mpsc::channel();
        let outbox = open(7, server_end, events).unwrap();

        assert!(outbox.push(protocol::encode(&ServerMessage::Error {
            message: "first".into(),
        })));
        let first: ServerMessage = protocol::decode(&read_one_frame(&mut peer)).unwrap();
        assert!(matches!(first, ServerMessage::Error { ref message } if message == "first"));

        // A frame that decodes and one that does not both arrive, in order;
        // the bad one carries its reason instead of vanishing.
        peer.write_all(&protocol::encode(&ClientMessage::Detach))
            .unwrap();
        let junk = br#"{"Teleport":{}}"#;
        peer.write_all(&(junk.len() as u32).to_be_bytes()).unwrap();
        peer.write_all(junk).unwrap();
        drop(peer);

        let deadline = Instant::now() + EVENT_TIMEOUT;
        let mut seen = Vec::new();
        while Instant::now() < deadline {
            match inbox.recv_timeout(EVENT_TIMEOUT) {
                Ok(Event::Closed(7)) => break,
                Ok(Event::Frame(ClientFrame { cid: 7, frame })) => seen.push(frame),
                Ok(_) | Err(_) => panic!("unexpected event or timeout"),
            }
        }
        assert_eq!(seen.len(), 2);
        assert!(matches!(seen[0], Ok(ClientMessage::Detach)));
        assert!(seen[1].is_err());
    }

    #[test]
    fn test_outbox_refuses_frames_beyond_the_cap() {
        let (frames, _queued) = mpsc::channel();
        let outbox = Outbox::new(frames);
        assert!(outbox.push(vec![0u8; CLIENT_OUTBOX_LIMIT]));
        assert!(
            !outbox.push(vec![0u8; 1]),
            "a frame that overflows the cap must be refused, not buffered"
        );
    }
}

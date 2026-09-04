//! Wire protocol for the mux client-server connection.
//!
//! Messages are length-prefixed JSON frames: 4-byte big-endian length, then
//! UTF-8 JSON. This keeps the protocol debuggable and language-agnostic.

use serde::{Deserialize, Serialize};

// ========================================================================
// Constants
// ========================================================================

const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = SECS_PER_MINUTE * 60;
const SECS_PER_DAY: u64 = SECS_PER_HOUR * 24;

// ========================================================================
// Data Structures
// ========================================================================

/// A request sent from an attached client to the mux server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Attach to an existing session (or create if absent).
    Attach {
        /// Name of the session to attach to.
        session: String,
    },
    /// Detach without killing the session.
    Detach,
    /// Send bytes to the PTY.
    Input {
        /// Session the bytes are for.
        session: String,
        /// Raw bytes to write to the PTY.
        bytes: Vec<u8>,
    },
    /// Resize the PTY.
    Resize {
        /// Session being resized.
        session: String,
        /// This client's column count.
        cols: u16,
        /// This client's row count.
        rows: u16,
    },
    /// List active sessions.
    ListSessions,
    /// Kill a session.
    Kill {
        /// Name of the session to terminate.
        session: String,
    },
    /// Create a session running `command` (a missing/empty command means
    /// the default shell) in `cwd`, then attach to it. Errors when a
    /// session with this name already exists.
    Spawn {
        /// Name for the new session.
        session: String,
        /// Initial column count.
        cols: u16,
        /// Initial row count.
        rows: u16,
        /// Directory to start in.
        cwd: Option<String>,
        /// Command to run; `None` means the default shell.
        command: Option<String>,
    },
}

/// A message the mux server sends out to its attached clients.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ServerMessage {
    /// PTY output batch.
    Output {
        /// Session that produced the bytes.
        session: String,
        /// Raw bytes read from the PTY.
        bytes: Vec<u8>,
    },
    /// Session list response.
    SessionList {
        /// One entry per live session.
        sessions: Vec<SessionInfo>,
    },
    /// Attach confirmed.
    Attached {
        /// Session now attached.
        session: String,
        /// The session's current column count.
        cols: u16,
        /// The session's current row count.
        rows: u16,
    },
    /// Session output buffered before this client attached; sent once
    /// right after `Attached` so a fresh client rebuilds its screen and
    /// scrollback before live output resumes.
    Scrollback {
        /// Session the buffered output belongs to.
        session: String,
        /// Output produced before this client attached.
        bytes: Vec<u8>,
    },
    /// Session exited.
    Exit {
        /// Session that ended.
        session: String,
        /// Exit status, absent when the process was signalled.
        code: Option<i32>,
    },
    /// The session's PTY geometry changed (server-arbitrated). Sent to every
    /// attached client whenever the effective size — the smallest geometry
    /// among attached clients — changes, so a pane whose layout is larger
    /// than the session can letterbox its grid instead of rendering a
    /// stream wrapped for a width it doesn't have.
    Resized {
        /// Session whose geometry changed.
        session: String,
        /// The new effective column count.
        cols: u16,
        /// The new effective row count.
        rows: u16,
    },
    /// The server could not carry out a request.
    Error {
        /// What went wrong, for display to the user.
        message: String,
    },
}

/// One session's summary, as reported in a session listing.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Clients currently attached to the session.
    #[serde(default)]
    pub attach_count: usize,
    /// The session's column count.
    pub cols: u16,
    /// Creation time, as seconds since the Unix epoch.
    pub created: u64,
    /// The session's name.
    pub name: String,
    /// The session's row count.
    pub rows: u16,
    /// The command line the session runs, for listings.
    pub command: String,
}

/// Accumulates bytes read from a connection and drains complete
/// length-prefixed frames as they arrive, carrying any partial frame (or
/// extra complete ones from a single read) over to the next call.
#[derive(Default)]
pub struct FrameBuffer {
    buf: Vec<u8>,
}

// ========================================================================
// Frame encoding
// ========================================================================

/// Frame a message as a four-byte big-endian length followed by JSON.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let json = serde_json::to_vec(msg).unwrap_or_default();
    let len = json.len() as u32;
    let mut out = len.to_be_bytes().to_vec();
    out.extend_from_slice(&json);
    out
}

/// Decode one framed message, or `None` when the bytes are not valid.
pub fn decode<T: serde::de::DeserializeOwned>(buf: &[u8]) -> Option<T> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    if buf.len() < 4 + len {
        return None;
    }
    serde_json::from_slice(&buf[4..4 + len]).ok()
}

/// Total length of the frame at the front of the buffer, prefix included.
pub fn frame_len(buf: &[u8]) -> Option<usize> {
    if buf.len() < 4 {
        return None;
    }
    let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
    Some(4 + len)
}

// ========================================================================
// FrameBuffer
// ========================================================================

impl FrameBuffer {
    /// An empty buffer, ready to accumulate frames read off a socket.
    pub fn new() -> Self {
        Self::default()
    }

    /// Append newly read bytes.
    pub fn extend(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Decode and remove every complete frame currently buffered, in
    /// arrival order. A frame that fails to deserialize as `T` is dropped
    /// rather than stalling the buffer on a message the caller can't use.
    pub fn drain<T: serde::de::DeserializeOwned>(&mut self) -> Vec<T> {
        let mut out = Vec::new();
        while let Some(total) = frame_len(&self.buf) {
            if self.buf.len() < total {
                break;
            }
            if let Some(msg) = decode::<T>(&self.buf[..total]) {
                out.push(msg);
            }
            self.buf.drain(..total);
        }
        out
    }
}

// ========================================================================
// Display formatting
// ========================================================================

/// A session's age as a short human-readable duration (`"45s"`, `"12m"`,
/// `"3h"`, `"2d"`), for `winter mux list` and the mux palette. `now` is
/// passed in rather than read internally so listings stay pure/testable.
pub fn format_uptime(created: u64, now: u64) -> String {
    let secs = now.saturating_sub(created);
    if secs < SECS_PER_MINUTE {
        format!("{secs}s")
    } else if secs < SECS_PER_HOUR {
        format!("{}m", secs / SECS_PER_MINUTE)
    } else if secs < SECS_PER_DAY {
        format!("{}h", secs / SECS_PER_HOUR)
    } else {
        format!("{}d", secs / SECS_PER_DAY)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_uptime_picks_the_largest_whole_unit() {
        assert_eq!(format_uptime(0, 45), "45s");
        assert_eq!(format_uptime(0, 60), "1m");
        assert_eq!(format_uptime(0, 3599), "59m");
        assert_eq!(format_uptime(0, 3600), "1h");
        assert_eq!(format_uptime(0, 86399), "23h");
        assert_eq!(format_uptime(0, 86400), "1d");
    }

    #[test]
    fn test_format_uptime_never_underflows_when_created_is_in_the_future() {
        // Clock skew between the server's `created` stamp and a caller's
        // `now` must not panic on unsigned subtraction.
        assert_eq!(format_uptime(100, 50), "0s");
    }

    #[test]
    fn test_encode_decode_client_attach() {
        let msg = ClientMessage::Attach {
            session: "default".into(),
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Attach { session } => assert_eq!(session, "default"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_server_output() {
        let msg = ServerMessage::Output {
            session: "s1".into(),
            bytes: vec![72, 101, 108, 108, 111],
        };
        let encoded = encode(&msg);
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Output { bytes, .. } => assert_eq!(bytes, b"Hello"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_frame_len_incomplete() {
        assert!(frame_len(&[0, 0]).is_none());
    }

    #[test]
    fn test_decode_incomplete() {
        assert!(decode::<ClientMessage>(&[0, 0, 0, 5, 123]).is_none());
    }

    #[test]
    fn test_session_info_round_trip() {
        let info = SessionInfo {
            attach_count: 2,
            name: "work".into(),
            cols: 120,
            rows: 40,
            created: 1700000000,
            command: "cargo watch".into(),
        };
        let encoded = encode(&info);
        let decoded: SessionInfo = decode(&encoded).unwrap();
        assert_eq!(decoded.name, "work");
        assert_eq!(decoded.cols, 120);
        assert_eq!(decoded.attach_count, 2);
    }

    #[test]
    fn test_encode_decode_client_resize() {
        let msg = ClientMessage::Resize {
            session: "default".into(),
            cols: 200,
            rows: 50,
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Resize { cols, rows, .. } => {
                assert_eq!(cols, 200);
                assert_eq!(rows, 50);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_server_resized() {
        let msg = ServerMessage::Resized {
            session: "s1".into(),
            cols: 80,
            rows: 24,
        };
        let encoded = encode(&msg);
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Resized {
                session,
                cols,
                rows,
            } => {
                assert_eq!(session, "s1");
                assert_eq!((cols, rows), (80, 24));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_client_spawn() {
        let msg = ClientMessage::Spawn {
            session: "dev".into(),
            cols: 120,
            rows: 40,
            cwd: Some("/tmp".into()),
            command: Some("cargo watch -x test".into()),
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Spawn {
                session,
                cols,
                rows,
                cwd,
                command,
            } => {
                assert_eq!(session, "dev");
                assert_eq!((cols, rows), (120, 40));
                assert_eq!(cwd.as_deref(), Some("/tmp"));
                assert_eq!(command.as_deref(), Some("cargo watch -x test"));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_server_exit() {
        let msg = ServerMessage::Exit {
            session: "s1".into(),
            code: Some(0),
        };
        let encoded = encode(&msg);
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Exit { code, .. } => assert_eq!(code, Some(0)),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_encode_decode_server_scrollback() {
        let msg = ServerMessage::Scrollback {
            session: "s1".into(),
            bytes: b"replay".to_vec(),
        };
        let encoded = encode(&msg);
        let decoded: ServerMessage = decode(&encoded).unwrap();
        match decoded {
            ServerMessage::Scrollback { bytes, .. } => assert_eq!(bytes, b"replay"),
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_frame_buffer_reassembles_a_frame_split_across_reads() {
        // Regression: a single `stream.read()` into a fixed buffer has no
        // notion of a frame boundary, so a frame split across two reads (a
        // batch of PTY output larger than the read buffer, or arriving in
        // two TCP/pipe segments) used to be silently dropped instead of
        // reassembled.
        let encoded = encode(&ClientMessage::Detach);
        let (first_half, second_half) = encoded.split_at(encoded.len() - 2);

        let mut fb = FrameBuffer::new();
        fb.extend(first_half);
        assert!(
            fb.drain::<ClientMessage>().is_empty(),
            "an incomplete frame must not decode yet"
        );

        fb.extend(second_half);
        let msgs = fb.drain::<ClientMessage>();
        assert_eq!(msgs.len(), 1);
        assert!(matches!(msgs[0], ClientMessage::Detach));
    }

    #[test]
    fn test_frame_buffer_drains_every_frame_from_one_read() {
        // Regression: decoding only ever looked at the first frame in a
        // read's bytes, so a read that happened to contain two or more
        // complete frames back-to-back silently lost every frame after the
        // first.
        let mut combined = encode(&ClientMessage::ListSessions);
        combined.extend(encode(&ClientMessage::Detach));
        combined.extend(encode(&ClientMessage::Kill {
            session: "s1".into(),
        }));

        let mut fb = FrameBuffer::new();
        fb.extend(&combined);
        let msgs = fb.drain::<ClientMessage>();

        assert_eq!(msgs.len(), 3);
        assert!(matches!(msgs[0], ClientMessage::ListSessions));
        assert!(matches!(msgs[1], ClientMessage::Detach));
        assert!(matches!(&msgs[2], ClientMessage::Kill { session } if session == "s1"));
    }

    #[test]
    fn test_frame_buffer_keeps_a_trailing_partial_frame_for_next_time() {
        let mut combined = encode(&ClientMessage::Detach);
        let full_frame_len = combined.len();
        combined.extend(encode(&ClientMessage::ListSessions));
        combined.truncate(full_frame_len + 2); // second frame arrives partially

        let mut fb = FrameBuffer::new();
        fb.extend(&combined);
        let msgs = fb.drain::<ClientMessage>();

        assert_eq!(msgs.len(), 1, "only the complete first frame decodes");
        assert!(matches!(msgs[0], ClientMessage::Detach));

        fb.extend(&encode(&ClientMessage::ListSessions)[2..]);
        let rest = fb.drain::<ClientMessage>();
        assert_eq!(
            rest.len(),
            1,
            "the carried-over partial frame completes and decodes"
        );
        assert!(matches!(rest[0], ClientMessage::ListSessions));
    }
}

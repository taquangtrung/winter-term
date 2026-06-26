//! Wire protocol for the mux client-server connection.
//!
//! Messages are length-prefixed JSON frames: 4-byte big-endian length, then
//! UTF-8 JSON. This keeps the protocol debuggable and language-agnostic.
//! Byte payloads (PTY input and output) travel as base64 strings, four wire
//! bytes for every three carried, rather than as JSON number arrays, which
//! cost up to four for every one.
//!
//! A client opens with [`ClientMessage::Hello`] naming the
//! [`MUX_PROTOCOL_VERSION`] it speaks; the server answers with its own, or
//! with a [`ServerMessage::Error`] and a closed connection when the two
//! cannot talk.

use serde::{Deserialize, Serialize};

// ========================================================================
// Constants
// ========================================================================

/// The mux wire version this build speaks. Version 1 was the unversioned
/// protocol that carried bytes as JSON number arrays; 2 added the handshake
/// and base64 byte payloads.
pub const MUX_PROTOCOL_VERSION: u16 = 2;

/// The largest frame a peer may declare. The biggest real message, a
/// session's scrollback replay, is capped well below this; a larger length
/// prefix is a broken or hostile peer, and buffering toward it would let
/// four bytes ask for four gigabytes.
const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// The longest session name accepted, in bytes.
const MAX_SESSION_NAME_BYTES: usize = 64;

/// Punctuation a session name may hold beside ASCII letters and digits.
/// Everything else is refused, so a name is safe as a file name, as a shell
/// word on the far side of `ssh`, and as a command-line argument.
const SESSION_NAME_PUNCTUATION: &[char] = &['-', '.', '_'];

const SECS_PER_MINUTE: u64 = 60;
const SECS_PER_HOUR: u64 = SECS_PER_MINUTE * 60;
const SECS_PER_DAY: u64 = SECS_PER_HOUR * 24;

// ========================================================================
// Data Structures
// ========================================================================

/// The size of one terminal cell in whole pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CellPixels {
    /// Cell height, in pixels.
    pub height: u16,
    /// Cell width, in pixels.
    pub width: u16,
}

/// A terminal's geometry: its grid, and its cell size when known.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Geometry {
    /// Cell size in pixels, or `None` when the client did not say.
    pub cell_px: Option<CellPixels>,
    /// Column count.
    pub cols: u16,
    /// Row count.
    pub rows: u16,
}

/// A request sent from an attached client to the mux server.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Attach to an existing session (or create if absent).
    Attach {
        /// Name of the session to attach to.
        session: String,
        /// Whether the session's buffered output is replayed after the
        /// confirmation. A client reattaching after a dropped connection
        /// already holds that history and asks for none, or it would draw
        /// it a second time. Absent from older clients, which always replay.
        #[serde(default = "replay_by_default")]
        replay: bool,
    },
    /// Detach without killing the session.
    Detach,
    /// The opening message of a connection: the protocol version the client
    /// speaks. The server answers with [`ServerMessage::Hello`].
    Hello {
        /// The client's [`MUX_PROTOCOL_VERSION`].
        version: u16,
    },
    /// Send bytes to the PTY.
    Input {
        /// Session the bytes are for.
        session: String,
        /// Raw bytes to write to the PTY.
        #[serde(with = "base64_bytes")]
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
        /// This client's cell size in pixels, when it knows it, so the PTY
        /// reports a true pixel size to programs that size images by it.
        /// Absent from older clients, which leaves the pixel size unknown.
        #[serde(default)]
        cell_px: Option<CellPixels>,
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
    /// The answer to [`ClientMessage::Hello`]: the version the server
    /// speaks. A client seeing one it does not speak stops.
    Hello {
        /// The server's [`MUX_PROTOCOL_VERSION`].
        version: u16,
    },
    /// PTY output batch.
    Output {
        /// Session that produced the bytes.
        session: String,
        /// Raw bytes read from the PTY.
        #[serde(with = "base64_bytes")]
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
        #[serde(with = "base64_bytes")]
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
    /// attached client whenever the effective size: the smallest geometry
    /// among attached clients: changes, so a pane whose layout is larger
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
    /// Set once the peer declared a frame past [`MAX_FRAME_BYTES`]: nothing
    /// it sends after can be trusted to line up, so it is all discarded and
    /// the connection is for the caller to drop.
    broken: bool,
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

    /// Append newly read bytes. A buffer that is [broken](Self::is_broken)
    /// takes no more.
    pub fn extend(&mut self, data: &[u8]) {
        if !self.broken {
            self.buf.extend_from_slice(data);
        }
    }

    /// Whether the peer declared a frame too large to be real, so the
    /// connection should be dropped.
    pub fn is_broken(&self) -> bool {
        self.broken
    }

    /// Decode and remove every complete frame currently buffered, in
    /// arrival order. A frame that fails to deserialize as `T` is dropped
    /// rather than stalling the buffer on a message the caller can't use.
    pub fn drain<T: serde::de::DeserializeOwned>(&mut self) -> Vec<T> {
        self.drain_results().into_iter().flatten().collect()
    }

    /// Like [`drain`](Self::drain), but a frame that does not deserialize
    /// as `T` comes back as the reason it did not, so the receiver can tell
    /// its peer instead of dropping the request without a word.
    pub fn drain_results<T: serde::de::DeserializeOwned>(&mut self) -> Vec<Result<T, String>> {
        let mut out = Vec::new();
        while let Some(total) = frame_len(&self.buf) {
            if total > MAX_FRAME_BYTES + 4 {
                self.broken = true;
                self.buf = Vec::new();
                break;
            }
            if self.buf.len() < total {
                break;
            }
            out.push(serde_json::from_slice(&self.buf[4..total]).map_err(|e| e.to_string()));
            self.buf.drain(..total);
        }
        out
    }
}

// ========================================================================
// CellPixels
// ========================================================================

impl CellPixels {
    /// A cell `width` x `height` pixels wide and tall, rounded to whole
    /// pixels.
    pub fn from_px(width: f32, height: f32) -> Self {
        Self {
            height: to_pixels(height),
            width: to_pixels(width),
        }
    }

    /// The pixel width of `cols` cells, saturating at what a PTY can hold.
    pub fn pixel_width(self, cols: u16) -> u16 {
        saturate(u32::from(cols) * u32::from(self.width))
    }

    /// The pixel height of `rows` cells, saturating at what a PTY can hold.
    pub fn pixel_height(self, rows: u16) -> u16 {
        saturate(u32::from(rows) * u32::from(self.height))
    }
}

// ========================================================================
// Session names
// ========================================================================

/// Check that `name` can name a session: 1 to [`MAX_SESSION_NAME_BYTES`]
/// bytes of ASCII letters, digits, and [`SESSION_NAME_PUNCTUATION`], not
/// starting with `-` (which a command line would read as an option).
/// The error says what is wrong, for display to the user.
pub fn validate_session_name(name: &str) -> Result<(), String> {
    if name.is_empty() || name.len() > MAX_SESSION_NAME_BYTES {
        return Err(format!(
            "a session name takes 1 to {MAX_SESSION_NAME_BYTES} characters"
        ));
    }
    if name.starts_with('-') {
        return Err(format!("session name '{name}' must not start with '-'"));
    }
    let allowed = |c: char| c.is_ascii_alphanumeric() || SESSION_NAME_PUNCTUATION.contains(&c);
    if !name.chars().all(allowed) {
        return Err(format!(
            "session name '{name}' may hold only letters, digits, '-', '.', and '_'"
        ));
    }
    Ok(())
}

/// What an [`ClientMessage::Attach`] from a client that predates the
/// `replay` field asks for: the replay every attach used to get.
fn replay_by_default() -> bool {
    true
}

// ========================================================================
// Byte payloads
// ========================================================================

/// Serde adapter for byte payloads: written as a base64 string, read as one
/// or as the JSON number array version 1 peers still send.
mod base64_bytes {
    use base64::prelude::BASE64_STANDARD;
    use base64::Engine;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    /// The two forms a byte payload arrives in.
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Encoded {
        Base64(String),
        Numbers(Vec<u8>),
    }

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&BASE64_STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        match Encoded::deserialize(deserializer)? {
            Encoded::Base64(text) => BASE64_STANDARD.decode(text).map_err(D::Error::custom),
            Encoded::Numbers(bytes) => Ok(bytes),
        }
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

fn to_pixels(px: f32) -> u16 {
    px.round().clamp(0.0, f32::from(u16::MAX)) as u16
}

fn saturate(px: u32) -> u16 {
    u16::try_from(px).unwrap_or(u16::MAX)
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
            replay: false,
            session: "default".into(),
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Attach { replay, session } => {
                assert_eq!(session, "default");
                assert!(!replay);
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_an_attach_from_an_older_client_asks_for_the_replay() {
        let old = br#"{"Attach":{"session":"s"}}"#;
        let mut frame = (old.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(old);
        match decode::<ClientMessage>(&frame) {
            Some(ClientMessage::Attach { replay, .. }) => assert!(replay),
            other => panic!("decoded {other:?}"),
        }
    }

    #[test]
    fn test_bytes_travel_as_base64_and_number_arrays_still_decode() {
        // Regression: bytes went out as a JSON number array, up to four wire
        // bytes per byte, so a full scrollback replay outgrew the outbox cap.
        let encoded = encode(&ServerMessage::Output {
            session: "s".into(),
            bytes: vec![0xFF; 3],
        });
        let json = std::str::from_utf8(&encoded[4..]).unwrap();
        assert!(json.contains(r#""bytes":"////""#), "{json}");

        let old = br#"{"Input":{"session":"s","bytes":[104,105]}}"#;
        let mut frame = (old.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(old);
        match decode::<ClientMessage>(&frame) {
            Some(ClientMessage::Input { bytes, .. }) => assert_eq!(bytes, b"hi"),
            other => panic!("decoded {other:?}"),
        }
    }

    #[test]
    fn test_drain_results_reports_a_frame_that_does_not_decode() {
        let junk = br#"{"Teleport":{}}"#;
        let mut frames = FrameBuffer::new();
        frames.extend(&(junk.len() as u32).to_be_bytes());
        frames.extend(junk);
        frames.extend(&encode(&ClientMessage::Detach));
        let results = frames.drain_results::<ClientMessage>();
        assert_eq!(results.len(), 2);
        assert!(results[0].is_err(), "the unknown message is reported");
        assert!(matches!(results[1], Ok(ClientMessage::Detach)));
    }

    #[test]
    fn test_session_names_refuse_shell_and_option_syntax() {
        for good in ["default", "work.2", "a_b-c"] {
            assert_eq!(validate_session_name(good), Ok(()), "{good}");
        }
        let too_long = "x".repeat(MAX_SESSION_NAME_BYTES + 1);
        for bad in [
            "",
            "-oProxyCommand=x",
            "a b",
            "x;rm",
            "$(id)",
            "a/b",
            &too_long,
        ] {
            assert!(validate_session_name(bad).is_err(), "{bad:?}");
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
        let cell = CellPixels {
            height: 18,
            width: 9,
        };
        let msg = ClientMessage::Resize {
            session: "default".into(),
            cols: 200,
            rows: 50,
            cell_px: Some(cell),
        };
        let encoded = encode(&msg);
        let decoded: ClientMessage = decode(&encoded).unwrap();
        match decoded {
            ClientMessage::Resize {
                cols,
                rows,
                cell_px,
                ..
            } => {
                assert_eq!(cols, 200);
                assert_eq!(rows, 50);
                assert_eq!(cell_px, Some(cell));
            }
            _ => panic!("wrong variant"),
        }
    }

    #[test]
    fn test_a_resize_from_an_older_client_still_decodes() {
        // Peers of different versions share a session; one that predates the
        // cell size field must not be refused.
        let old = br#"{"Resize":{"session":"s","cols":80,"rows":24}}"#;
        let mut frame = (old.len() as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(old);
        match decode::<ClientMessage>(&frame) {
            Some(ClientMessage::Resize { cell_px, .. }) => assert_eq!(cell_px, None),
            other => panic!("decoded {other:?}"),
        }
    }

    #[test]
    fn test_pixel_sizes_saturate_instead_of_wrapping() {
        let cell = CellPixels::from_px(9.4, 18.6);
        assert_eq!((cell.width, cell.height), (9, 19));
        assert_eq!(cell.pixel_width(80), 720);
        assert_eq!(cell.pixel_height(u16::MAX), u16::MAX);
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

    #[test]
    fn test_a_frame_too_large_to_be_real_breaks_the_buffer_instead_of_growing_it() {
        let mut frames = FrameBuffer::new();
        frames.extend(&[0xFF, 0xFF, 0xFF, 0xFF]);
        assert!(frames.drain::<ClientMessage>().is_empty());
        assert!(frames.is_broken());
        frames.extend(&[0u8; 4096]);
        assert!(frames.buf.is_empty(), "nothing more is buffered");
    }
}

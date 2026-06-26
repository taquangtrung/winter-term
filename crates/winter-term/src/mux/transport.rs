//! What every mux connection offers its user, whichever way it reaches the
//! server: a local Unix socket ([`MuxClient`](super::client::MuxClient)) or
//! an ssh bridge ([`RemoteClient`](super::remote::RemoteClient)).

use super::protocol::{ClientMessage, Geometry, ServerMessage, MUX_PROTOCOL_VERSION};

// ========================================================================
// Traits
// ========================================================================

/// A connection to a mux server.
pub trait MuxTransport {
    /// Whether the server end has closed: further reads return nothing,
    /// ever. Buffered messages may still be drained after this is `true`.
    fn eof(&self) -> bool;

    /// The next message from the server, oldest first, or `None` when none
    /// has arrived yet. An error means the connection is unusable.
    fn recv(&mut self) -> anyhow::Result<Option<ServerMessage>>;

    /// Send a request to the server.
    fn send(&mut self, msg: &ClientMessage) -> anyhow::Result<()>;

    /// Forward bytes to a session's PTY.
    fn send_input(&mut self, session: &str, bytes: &[u8]) -> anyhow::Result<()> {
        self.send(&ClientMessage::Input {
            session: session.to_string(),
            bytes: bytes.to_vec(),
        })
    }

    /// Report this client's geometry; the server arbitrates the session's size.
    fn resize(&mut self, session: &str, geometry: Geometry) -> anyhow::Result<()> {
        self.send(&ClientMessage::Resize {
            session: session.to_string(),
            cols: geometry.cols,
            rows: geometry.rows,
            cell_px: geometry.cell_px,
        })
    }
}

// ========================================================================
// Handshake
// ========================================================================

/// The message a connection opens with.
pub(super) fn hello() -> ClientMessage {
    ClientMessage::Hello {
        version: MUX_PROTOCOL_VERSION,
    }
}

/// Fail on a handshake reply naming a version this client does not speak;
/// every other message passes.
pub(super) fn check_handshake(msg: &ServerMessage) -> anyhow::Result<()> {
    if let ServerMessage::Hello { version } = msg {
        anyhow::ensure!(
            *version == MUX_PROTOCOL_VERSION,
            "the mux server speaks protocol version {version}; \
             this client speaks version {MUX_PROTOCOL_VERSION}"
        );
    }
    Ok(())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_only_a_mismatched_handshake_fails() {
        let other = ServerMessage::Hello {
            version: MUX_PROTOCOL_VERSION + 1,
        };
        assert!(check_handshake(&other).is_err());
        let same = ServerMessage::Hello {
            version: MUX_PROTOCOL_VERSION,
        };
        assert!(check_handshake(&same).is_ok());
        let output = ServerMessage::Error {
            message: String::new(),
        };
        assert!(check_handshake(&output).is_ok());
    }
}

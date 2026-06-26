//! Terminal capability advertisement, returned in reply to a `caps` query.

use serde::{Deserialize, Serialize};

use crate::tier::TrustTier;
use crate::Version;

// ============================================================================
// Data Structures
// ============================================================================

/// What a terminal can render, sent back to a querying tool so it can choose a
/// representation before emitting. Travels as JSON on the tool's stdin, not as
/// an OSC escape, so it is serialized directly rather than through this crate's escape codec.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct Caps {
    /// Whether the terminal supports live blocks (`open`/`patch`/`close`), as
    /// opposed to one-shot `emit` only.
    pub live: bool,
    /// MIME types the terminal can render, richest first.
    pub mime: Vec<String>,
    /// Whether the terminal will read a payload from a file referenced by the
    /// escape instead of inline base64, for payloads too large for the PTY.
    pub side_channel: bool,
    /// Trust tiers the terminal's policy will actually grant. A tier absent
    /// here is clamped away (see [`TrustTier::clamp_to`]), so a tool should not
    /// bother requesting it.
    pub tiers: Vec<TrustTier>,
    /// The TBP protocol version the terminal speaks.
    #[serde(rename = "v")]
    pub version: Version,
}

// ============================================================================
// Constants
// ============================================================================

const SUPPORTED_MIME: &[&str] = &[
    "text/html",
    "image/svg+xml",
    "text/markdown",
    "text/csv",
    "image/png",
    "image/jpeg",
    "image/gif",
    "application/json",
    "text/plain",
];

// ============================================================================
// Caps
// ============================================================================

impl Caps {
    /// The capabilities this terminal advertises to querying tools.
    pub fn winter_default() -> Self {
        Self {
            live: true,
            mime: SUPPORTED_MIME.iter().map(|s| (*s).to_string()).collect(),
            side_channel: true,
            tiers: vec![
                TrustTier::Trusted,
                TrustTier::Restricted,
                TrustTier::Isolated,
            ],
            version: crate::PROTOCOL_VERSION,
        }
    }

    /// Serialize as JSON, ready to write to a tool's stdin.
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("Caps is always serializable")
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use crate::PROTOCOL_VERSION;

    use super::*;

    #[test]
    fn test_version_serializes_as_bare_v_field() {
        let caps = Caps {
            live: true,
            mime: vec!["text/plain".to_string()],
            side_channel: false,
            tiers: vec![TrustTier::Restricted],
            version: PROTOCOL_VERSION,
        };
        let json = serde_json::to_value(&caps).unwrap();
        assert_eq!(json["v"], serde_json::json!(PROTOCOL_VERSION.0));
    }

    #[test]
    fn test_winter_default_has_supported_mime() {
        let caps = Caps::winter_default();
        assert!(caps.mime.contains(&"text/html".to_string()));
        assert!(caps.mime.contains(&"text/plain".to_string()));
        assert!(caps.live);
        assert!(caps.side_channel);
    }

    #[test]
    fn test_winter_default_serializes_to_json() {
        let caps = Caps::winter_default();
        let json = caps.to_json();
        assert!(json.contains("\"v\":1"), "{json}");
        assert!(json.contains("text/html"), "{json}");
    }
}

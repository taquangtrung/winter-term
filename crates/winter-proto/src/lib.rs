//! Terminal Block Protocol (TBP) v1: wire types and codec.
//!
//! TBP is an OSC escape sequence carrying a MIME bundle, directly inspired by
//! Jupyter's `display_data` message. An editor emits a block as one escape; the
//! terminal selects the richest MIME representation it can render and falls back
//! toward `text/plain`. Terminals that do not understand TBP ignore the escape.
//!
//! The wire form is documented normatively in
//! `docs/terminal-block-protocol-spec.md`; this crate is its reference codec.
//! See [`encode`] and [`decode`] for the byte-stream entry points.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod bundle;
mod caps;
mod message;
mod tier;
mod wire;

pub use bundle::{BlockMeta, MimeBundle, TEXT_PLAIN};
pub use caps::Caps;
pub use message::{EmitBlock, Message, OpenBlock, PatchBlock};
pub use tier::TrustTier;
pub use wire::{decode, decode_with_sidechannel, encode, encode_emit_file, ProtoError};

// ============================================================================
// Constants
// ============================================================================

/// Protocol version this crate emits and is the highest it accepts.
pub const PROTOCOL_VERSION: Version = Version(1);

/// Private-use OSC number that frames every TBP message. Provisional; the final
/// number is coordinated with other terminals before 1.0.
pub const OSC_NUMBER: u32 = 9001;

/// How many low bits of a [`BlockId::process_scoped`] id count blocks
/// within one process; the bits above hold the process id. Twenty leave
/// room for a million blocks per process, and keep a 32-bit process id's
/// ids below 2^53, so they survive a trip through a JavaScript number.
pub const BLOCK_SEQUENCE_BITS: u32 = 20;

// ============================================================================
// Data Structures
// ============================================================================

/// TBP protocol version. Wire form is a bare integer.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct Version(pub u16);

/// Identifier correlating live-block updates (`open`/`patch`/`close`) and
/// distinguishing concurrent blocks within one session.
#[derive(
    Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, serde::Deserialize, serde::Serialize,
)]
#[serde(transparent)]
pub struct BlockId(pub u64);

// ============================================================================
// BlockId
// ============================================================================

impl BlockId {
    /// The id of the `sequence`th block emitted by process `pid`. Ids are
    /// per session, and every program writing to one shares it, so a plain
    /// counter from 1 in each would collide: a later program's `open` for
    /// a block an earlier one left open is refused, and its patches land on
    /// the earlier block. Mixing in the process id keeps them apart.
    /// `sequence` wraps after 2^[`BLOCK_SEQUENCE_BITS`] blocks.
    pub fn process_scoped(pid: u32, sequence: u64) -> Self {
        let sequence_mask = (1u64 << BLOCK_SEQUENCE_BITS) - 1;
        BlockId((u64::from(pid) << BLOCK_SEQUENCE_BITS) | (sequence & sequence_mask))
    }
}

// ============================================================================
// Version
// ============================================================================

impl Version {
    /// Whether a bundle declaring this version can be rendered by this crate.
    /// A terminal accepts any version at or below the one it implements.
    pub fn is_supported(self) -> bool {
        self.0 <= PROTOCOL_VERSION.0
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest integer a JavaScript number holds exactly.
    const MAX_SAFE_JS_INTEGER: u64 = (1 << 53) - 1;

    #[test]
    fn test_process_scoped_ids_differ_across_processes_and_stay_js_safe() {
        assert_ne!(
            BlockId::process_scoped(100, 1),
            BlockId::process_scoped(101, 1)
        );
        assert_eq!(
            BlockId::process_scoped(7, 1 << BLOCK_SEQUENCE_BITS),
            BlockId::process_scoped(7, 0),
            "the sequence wraps inside its bits instead of spilling into the pid"
        );
        assert!(BlockId::process_scoped(u32::MAX, u64::MAX).0 <= MAX_SAFE_JS_INTEGER);
    }

    #[test]
    fn test_current_version_is_supported() {
        assert!(PROTOCOL_VERSION.is_supported());
    }

    #[test]
    fn test_future_version_is_unsupported() {
        let future = Version(PROTOCOL_VERSION.0 + 1);
        assert!(!future.is_supported());
    }
}

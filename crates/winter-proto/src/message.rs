//! TBP messages: the verbs an editor sends to the terminal.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::bundle::MimeBundle;
use crate::tier::TrustTier;
use crate::BlockId;

// ============================================================================
// Data Structures
// ============================================================================

/// A single TBP message, decoded from one OSC escape.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Message {
    /// Query terminal capabilities. The reply travels out of band (see
    /// [`crate::Caps`]), not as another `Message`.
    Caps,
    /// Close a live block.
    Close(BlockId),
    /// Emit a one-shot block.
    Emit(EmitBlock),
    /// Open a live block for subsequent incremental updates.
    Open(OpenBlock),
    /// Apply an incremental update to a live block.
    Patch(PatchBlock),
}

/// Payload of [`Message::Emit`]: a complete block rendered once.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
pub struct EmitBlock {
    /// The block's alternative representations.
    pub bundle: MimeBundle,
    /// Emitter-chosen identifier, unique within the emitting process.
    pub id: BlockId,
    /// The tier the emitter *requested*. Attacker-controlled: clamp it with
    /// [`TrustTier::clamp_to`] before granting any capability from it.
    pub trust: TrustTier,
}

/// Payload of [`Message::Open`]: the initial state of a live block.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
pub struct OpenBlock {
    /// Identifier later `patch` and `close` messages refer back to.
    pub id: BlockId,
    /// The single MIME type this block's spec is written in; a live block
    /// carries one representation rather than a bundle of alternatives.
    pub mime: String,
    /// The block's initial content, before any patch is folded in.
    pub spec: Value,
}

/// Payload of [`Message::Patch`]: an RFC 6902 JSON patch applied to a live block.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[non_exhaustive]
pub struct PatchBlock {
    /// Which open block to update.
    pub id: BlockId,
    /// An RFC 6902 patch document applied to the block's current spec.
    pub patch: Value,
}

// ============================================================================
// EmitBlock
// ============================================================================

impl EmitBlock {
    /// Block `id`, showing `bundle`, at the tier `trust` asks for.
    pub fn new(id: BlockId, trust: TrustTier, bundle: MimeBundle) -> Self {
        Self { bundle, id, trust }
    }
}

// ============================================================================
// OpenBlock
// ============================================================================

impl OpenBlock {
    /// Live block `id`, starting from `spec` written in `mime`.
    pub fn new(id: BlockId, mime: impl Into<String>, spec: Value) -> Self {
        Self {
            id,
            mime: mime.into(),
            spec,
        }
    }
}

// ============================================================================
// PatchBlock
// ============================================================================

impl PatchBlock {
    /// `patch`, an RFC 6902 document, applied to live block `id`.
    pub fn new(id: BlockId, patch: Value) -> Self {
        Self { id, patch }
    }
}

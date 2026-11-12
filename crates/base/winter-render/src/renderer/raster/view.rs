//! The images the rasterizers hand back: pixels, and where they go.

// ========================================================================
// Data Structures
// ========================================================================

/// A rasterized dropdown overlay: its pixels and the surface position to place
/// them at (top-left, already offset to include the shadow margin).
pub(crate) struct DropdownImage {
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

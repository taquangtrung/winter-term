//! The images the rasterizers hand back: pixels, and where they go.

use glyphon::{Color, TextBounds};

// ========================================================================
// Data Structures
// ========================================================================

/// One shaped text run of the top tabbar (a tab title, a menu title, a glyph),
/// plus where to place and clip it. Built fresh each frame and kept alive until
/// after `glyphon` prepares the text pass.
pub(crate) struct TabbarText {
    pub(crate) bounds: TextBounds,
    pub(crate) buffer: glyphon::Buffer,
    pub(crate) color: Color,
    pub(crate) left: f32,
    pub(crate) top: f32,
}
/// A rasterized top-tabbar strip: its pixels and their dimensions. Unlike a
/// dropdown it carries no position, because the strip always sits at the
/// surface's top-left corner.
pub(crate) struct StripImage {
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
}

/// A rasterized dropdown overlay: its pixels and the surface position to place
/// them at (top-left, already offset to include the shadow margin).
pub(crate) struct DropdownImage {
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}

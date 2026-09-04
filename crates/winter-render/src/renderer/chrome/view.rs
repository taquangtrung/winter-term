//! View types the chrome rasterizers are handed.

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
/// A rasterized dropdown overlay: its pixels and the surface position to place
/// them at (top-left, already offset to include the shadow margin).
pub(crate) struct DropdownImage {
    pub(crate) height: u32,
    pub(crate) rgba: Vec<u8>,
    pub(crate) width: u32,
    pub(crate) x: f32,
    pub(crate) y: f32,
}
/// One entry shown in the command palette results list.
pub struct PaletteItem {
    /// Command id dispatched when this entry is chosen.
    pub action: String,
    /// Human-readable entry text shown in the list.
    pub label: String,
    /// Char indices in `label` that matched the query, used to highlight them.
    pub match_positions: Vec<usize>,
    /// Keyboard shortcut hint shown on the right (e.g. `"Ctrl-Shift-T"`).
    pub shortcut: String,
}
/// The command palette state the renderer needs to draw its overlay.
pub struct PaletteView {
    /// Text shown when the filtered list is empty.
    pub empty_message: String,
    /// The entries to draw, already filtered and ranked by the caller.
    pub items: Vec<PaletteItem>,
    /// Draw an underline under highlighted match characters.
    pub match_underline: bool,
    /// The filter text as typed.
    pub query: String,
    /// Index into `items` of the highlighted entry.
    pub selected: usize,
}
/// The which-key hint overlay state shown when the user pauses mid-prefix.
#[derive(Clone, Debug)]
pub struct WhichKeyView {
    /// The continuations available from the pending prefix, as
    /// `(key, description)`.
    pub items: Vec<(String, String)>,
    /// The prefix already typed, shown as the popup's heading.
    pub title: String,
}

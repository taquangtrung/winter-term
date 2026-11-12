//! The fixed regions of the window around the panes: each pane's own tab
//! strip ([`pane_strip`]). The title bar is misty's, drawn by the app.

pub(in crate::renderer) mod pane_strip;

// ========================================================================
// Constants
// ========================================================================

/// Corner radius of every pill on the pane strips: none,
/// so their tabs and button highlights are square.
const SQUARE_CORNER: f32 = 0.0;

/// Flat pixels the close button (× glyph and its hover pill) is shifted
/// right of its horizontally-centered position within its close slot.
const TAB_CLOSE_SHIFT_RIGHT_PX: f32 = 3.0;

/// Flat pixels the tab title label is shifted up from its vertically-
/// centered position on the tab shape.
const TAB_LABEL_SHIFT_UP_PX: f32 = 1.0;

//! The title bar's styles, which the app's settings name, and the pixel
//! rectangle the app hands the renderer to anchor things to. The title bar
//! and the pane strips themselves are misty's.

// ========================================================================
// Data Structures
// ========================================================================

/// Which menubar presentation to draw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuStyle {
    /// A `File / Edit / View / …` menubar in the title bar; each title opens its menu.
    Classic,
    /// A single `☰` button in the title bar that opens one dropdown of commands.
    #[default]
    Modern,
}

/// Which edge of the title bar carries the minimize/maximize/close buttons.
/// The hamburger button (modern style only) sits on the opposite edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlsSide {
    /// Window controls hug the left edge; hamburger (if any) on the right.
    #[default]
    Left,
    /// Window controls hug the right edge; hamburger (if any) on the left.
    Right,
}

/// A pixel rectangle in surface coordinates (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Height in pixels.
    pub h: f32,
    /// Width in pixels.
    pub w: f32,
    /// X origin in pixels.
    pub x: f32,
    /// Y origin in pixels.
    pub y: f32,
}

// ========================================================================
// Region
// ========================================================================

impl Region {
    /// Whether the point `(px, py)` is inside this region.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

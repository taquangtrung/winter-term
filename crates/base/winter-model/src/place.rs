//! Where a view puts the cursor's screen row: the placements `zt`, `zz`,
//! `zb`, and `C-l` name, and the rows on screen `H`, `M`, and `L` go to.

// ========================================================================
// Data Structures
// ========================================================================

/// Where a placement (`zt`, `zz`, `zb`, `C-l`) puts the cursor's screen row
/// in the pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Place {
    /// On the pane's last screen row.
    Bottom,
    /// Centered within the portion of the pane not covered by the editor-bottom picker.
    /// The uncovered portion is ~60% of the pane height at the top, so its center
    /// is at ~30% from the top.
    CenterInUncovered,
    /// On its middle one.
    Middle,
    /// On its first.
    Top,
}

/// The rows a view shows on its top, middle, and bottom screen rows, which
/// `H`, `M`, and `L` go to: a row wrapped over several screen rows is one
/// of them however many it takes. Or those screen rows themselves, counted
/// from the pane's first (see `RowWindow::shown_at` in `winter-model`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Shown {
    /// The row on the pane's last screen row with something on it.
    pub bottom: usize,
    /// The row on the middle one of those.
    pub middle: usize,
    /// The row on the first.
    pub top: usize,
}

// ========================================================================
// Functions
// ========================================================================

/// `margin` screen rows, held to under half a pane `screen_rows` tall, so
/// the margins above and below the cursor leave its own row between them.
pub fn clamp_margin(margin: usize, screen_rows: usize) -> usize {
    margin.min(screen_rows.saturating_sub(1) / 2)
}

// ========================================================================
// Place
// ========================================================================

impl Place {
    /// How many screen rows of a pane `screen_rows` tall are above the
    /// cursor's once it is placed, `margin` of them kept at the edge it goes
    /// to.
    pub fn rows_above(self, screen_rows: usize, margin: usize) -> usize {
        let screen_rows = screen_rows.max(1);
        let margin = clamp_margin(margin, screen_rows);
        match self {
            Place::Bottom => screen_rows - 1 - margin,
            Place::CenterInUncovered => {
                let picker_rows = (screen_rows as f32 * 0.40).round() as usize;
                let uncovered_rows = screen_rows.saturating_sub(picker_rows).max(1);
                uncovered_rows / 2
            }
            Place::Middle => screen_rows / 2,
            Place::Top => margin,
        }
    }
}

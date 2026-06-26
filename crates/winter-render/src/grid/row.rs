//! One row's worth of grid state: the per-row layout facts a soft wrap
//! leaves behind, and a history row that carries its cells with them.

use super::Cell;

// ========================================================================
// Data Structures
// ========================================================================

/// A row of the scrollback history: its cells and how it ended. Kept as one
/// value so the history can never hold cells for a row whose wrap facts went
/// missing, which is what parallel per-field lists invite.
#[derive(Clone, Debug)]
pub(crate) struct Row {
    pub(crate) cells: Vec<Cell>,
    pub(crate) meta: RowMeta,
}

/// How a row ended and how its continuation starts, recorded at print time
/// and read back when the logical line is joined into text or reflowed.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RowMeta {
    /// `Some(k)` when the row soft-wrapped at a word boundary, consuming the
    /// space at column `k` and moving everything after it onto the next row.
    /// Joining the logical line contributes that space back instead of
    /// gluing the words together.
    pub(crate) break_col: Option<usize>,
    /// Hanging indent columns at the start of this (continuation) row.
    pub(crate) indent: usize,
    /// Whether the row filled the width and auto-wrapped into the next one,
    /// rather than ending at an explicit newline.
    pub(crate) wrapped: bool,
}

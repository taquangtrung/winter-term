//! Quick-select: assigns single-letter labels to visible block boundaries so
//! the user can jump to a block with one keystroke.

use crate::model::layout::TabId;

use crate::app::blocks::block_at_row;
use crate::app::{App, QuickLabel};

// ========================================================================
// Constants
// ========================================================================

const QUICK_SELECT_LABELS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
    'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

// ========================================================================
// App: quick-select
// ========================================================================

impl App {
    pub(crate) fn enter_quick_select(&mut self, focused: TabId) {
        self.quick_select = self.generate_quick_labels(focused);
        self.dirty = true;
    }

    /// A label on the first row of each block that starts inside the
    /// focused pane's view.
    fn generate_quick_labels(&self, focused: TabId) -> Option<Vec<QuickLabel>> {
        let terminal = self.open_tabs.terminal(&focused)?;
        let grid = terminal.grid();
        let vis_start = grid.to_absolute_row(0);
        let vis_end = vis_start + grid.rows();

        let mut starts: Vec<usize> = terminal
            .block_rows()
            .iter()
            .copied()
            .filter(|row| (vis_start..vis_end).contains(row))
            .collect();
        // Blocks a `clear` pulled onto one row share a single label.
        starts.dedup();
        let labels: Vec<QuickLabel> = starts
            .into_iter()
            .zip(QUICK_SELECT_LABELS.iter())
            .map(|(abs_row, &label)| QuickLabel {
                col: 0,
                label,
                row: abs_row - vis_start,
            })
            .collect();
        (!labels.is_empty()).then_some(labels)
    }

    /// Scroll the focused pane so the block `label` marks starts at the top
    /// of the view.
    pub(crate) fn quick_jump(&mut self, focused: TabId, label: char) {
        let Some(labels) = self.quick_select.take() else {
            return;
        };
        let Some(target) = labels.iter().find(|ql| ql.label == label) else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        let abs_target = terminal.grid().to_absolute_row(target.row);
        let starts = terminal.block_rows();
        let Some(start) = block_at_row(starts, abs_target).map(|idx| starts[idx]) else {
            return;
        };
        terminal.grid_mut().scroll_to_absolute_row(start);
        self.dirty = true;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_quick_select_labels_constants() {
        assert!(!QUICK_SELECT_LABELS.is_empty());
        assert_eq!(QUICK_SELECT_LABELS[0], 'a');
    }
}

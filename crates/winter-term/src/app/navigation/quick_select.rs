//! Quick-select: assigns single-letter labels to visible block boundaries so
//! the user can jump to a block with one keystroke.

use crate::model::layout::TabId;

use crate::app::{App, QuickLabel};

// ========================================================================
// Constants
// ========================================================================

const QUICK_SELECT_LABELS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
    'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

// ========================================================================
// Free functions
// ========================================================================

/// The nearest block start offset at or before `target`, or `None` if
/// `target` precedes every block. `offsets` must be ascending, as returned
/// by `Scrollback::block_row_offsets`.
fn block_containing(offsets: &[usize], target: usize) -> Option<usize> {
    offsets.iter().rev().find(|&&row| row <= target).copied()
}

// ========================================================================
// App: quick-select
// ========================================================================

impl App {
    pub(crate) fn enter_quick_select(&mut self, focused: TabId) {
        self.quick_select = self.generate_quick_labels(focused);
        self.dirty = true;
    }

    fn generate_quick_labels(&self, focused: TabId) -> Option<Vec<QuickLabel>> {
        let terminal = self.open_tabs.terminal(&focused)?;
        let grid = terminal.grid();
        let scrollback = terminal.scrollback();
        let cols = grid.cols();
        let rows = grid.rows();
        let offsets = scrollback.block_row_offsets(cols, grid.word_wrap());
        let scroll_offset = grid.scroll_offset();
        let sb_len = grid.scrollback_len();

        let vis_start = sb_len.saturating_sub(scroll_offset);
        let vis_end = vis_start + rows;

        let mut labels = Vec::new();
        for &abs_row in &offsets {
            if abs_row >= vis_start && abs_row < vis_end {
                if let Some(&label) = QUICK_SELECT_LABELS.get(labels.len()) {
                    let grid_row = abs_row - vis_start;
                    labels.push(QuickLabel {
                        row: grid_row,
                        col: 0,
                        label,
                    });
                }
            }
        }

        if labels.is_empty() {
            None
        } else {
            Some(labels)
        }
    }

    pub(crate) fn quick_jump(&mut self, focused: TabId, label: char) {
        let labels = match self.quick_select.take() {
            Some(l) => l,
            None => return,
        };
        let target = match labels.iter().find(|ql| ql.label == label) {
            Some(ql) => *ql,
            None => return,
        };

        let terminal = match self.open_tabs.terminal(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = terminal.grid().cols();
        let offsets = terminal
            .scrollback()
            .block_row_offsets(cols, terminal.grid().word_wrap());
        let scroll_offset = terminal.grid().scroll_offset();
        let sb_len = terminal.grid().scrollback_len();

        let vis_start = sb_len.saturating_sub(scroll_offset);
        let abs_target = vis_start + target.row;

        let Some(block_start) = block_containing(&offsets, abs_target) else {
            return;
        };

        let diff = abs_target.saturating_sub(block_start);
        if block_start >= sb_len {
            let live_row = block_start - sb_len;
            let current_row = terminal.grid().cursor().0;
            if live_row > current_row {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal
                        .grid_mut()
                        .scroll_up_history(live_row - current_row);
                }
            }
        } else if block_start > 0 || diff > 0 {
            let target_scroll = sb_len.saturating_sub(block_start);
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.grid_mut().set_scroll_offset(target_scroll);
            }
        }

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

    #[test]
    fn test_block_containing_finds_the_nearest_block_not_the_first() {
        // Regression: `.find()` over an ascending list whose first element
        // is always 0 previously matched immediately, so every quick-select
        // jump landed on the oldest block regardless of the label picked.
        let offsets = [0, 10, 25, 40];
        assert_eq!(block_containing(&offsets, 5), Some(0));
        assert_eq!(block_containing(&offsets, 10), Some(10));
        assert_eq!(block_containing(&offsets, 39), Some(25));
        assert_eq!(block_containing(&offsets, 100), Some(40));
        assert_eq!(block_containing(&[], 5), None);
    }
}

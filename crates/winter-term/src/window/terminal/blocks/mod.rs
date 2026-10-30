//! Blocks in the scrollback: fold, yank, focus, and navigate operations.
//! Drawing them natively lives in [`native`], and the plain text a structured
//! block is shown as in [`text`].

pub(crate) mod native;
pub(crate) mod text;

use crate::model::input;
use crate::model::layout::TabId;
use crate::model::mode::Mode;

use crate::window::App;

// ========================================================================
// App: block operations
// ========================================================================

impl App {
    pub(in crate::window) fn yank_block_source(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let text = self
            .block_in_view(focused)
            .and_then(|idx| terminal.scrollback().blocks().get(idx))
            .map(|block| block.plain_text())
            .filter(|text| !text.is_empty());
        // Borrows of `pane` end here, freeing `self` for the clipboard handle.
        let Some(text) = text else {
            return;
        };
        if let Some(clipboard) = self.input.clipboard() {
            let copied = clipboard.set_text(&text).is_ok();
            if copied {
                self.set_notice("Copied to clipboard");
            }
        }
    }

    /// Export the plain-text content of the focused block to the system clipboard.
    pub(in crate::window) fn export_focused_block_text(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let text = self
            .block_in_view(focused)
            .and_then(|idx| terminal.scrollback().blocks().get(idx))
            .map(|block| {
                let mut content = format!("$ {}\n", block.command);
                content.push_str(&block.plain_text());
                content
            });
        let Some(text) = text else {
            self.set_error("No block content to export");
            return;
        };
        if let Some(clipboard) = self.input.clipboard() {
            if clipboard.set_text(&text).is_ok() {
                self.set_notice("Copied block text to clipboard");
            } else {
                self.set_error("Could not copy to clipboard");
            }
        }
    }

    /// Export the SVG graphic in the focused block (if any) to a temporary file and open it.
    pub(in crate::window) fn export_focused_block_svg(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let svg = self
            .block_in_view(focused)
            .and_then(|idx| terminal.scrollback().blocks().get(idx))
            .and_then(|block| block.svg_content());
        let Some(svg) = svg else {
            self.set_error("Focused block has no SVG graphic");
            return;
        };
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!("winter-block-{timestamp}.svg"));
        if let Err(e) = std::fs::write(&path, svg) {
            self.set_error(format!("Could not write SVG file: {e}"));
            return;
        }
        match ::open::that_detached(&path) {
            Ok(()) => self.set_notice(format!("Opened {}", path.display())),
            Err(e) => self.set_error(format!("Could not open SVG file: {e}")),
        }
    }

    pub(in crate::window) fn toggle_fold(&mut self, focused: TabId) {
        let Some(idx) = self.block_in_view(focused) else {
            return;
        };

        let Some(tab) = self.open_tabs.terminal_tab_mut(&focused) else {
            return;
        };
        if !tab.folded_blocks.remove(&idx) {
            tab.folded_blocks.insert(idx);
        }
        self.dirty = true;
    }

    pub(in crate::window) fn is_block_folded(&self, tab_id: TabId, block_index: usize) -> bool {
        self.open_tabs
            .terminal_tab(&tab_id)
            .is_some_and(|tab| tab.folded_blocks.contains(&block_index))
    }

    /// Scroll the focused pane so the next or previous block's first row is
    /// at the top of the view. Past the newest block the view returns to the
    /// live bottom; before the oldest it stays on the oldest.
    pub(in crate::window) fn focus_block(&mut self, nav: input::BlockNav, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        let top = terminal.grid().to_absolute_row(0);
        let starts = terminal.block_rows();
        let target = match nav {
            input::BlockNav::Next => starts.iter().copied().find(|&start| start > top),
            input::BlockNav::Previous => starts
                .iter()
                .copied()
                .rev()
                .find(|&start| start < top)
                .or_else(|| starts.first().copied()),
        };
        let grid = terminal.grid_mut();
        match target {
            Some(start) => grid.scroll_to_absolute_row(start),
            None => grid.set_scroll_offset(0),
        }
        self.dirty = true;
    }

    /// The block the focused pane's view is on: the one its Normal-mode
    /// cursor sits in; else, when the view is scrolled back, the one at its
    /// top row; else, at the live bottom, the newest block with output (the
    /// command that just finished, not the prompt waiting below it).
    pub(in crate::window) fn block_in_view(&self, focused: TabId) -> Option<usize> {
        let terminal = self.open_tabs.terminal(&focused)?;
        let grid = terminal.grid();
        let in_normal = self
            .open_tabs
            .mode(&focused)
            .is_some_and(|mode| matches!(mode, Mode::Normal | Mode::Visual));
        let view_row = match self.nav_cursor(focused).filter(|_| in_normal) {
            Some((row, _)) => Some(row),
            None => (grid.scroll_offset() > 0).then_some(0),
        };
        match view_row {
            Some(row) => block_at_row(terminal.block_rows(), grid.to_absolute_row(row)),
            None => terminal
                .scrollback()
                .blocks()
                .iter()
                .rposition(|block| !block.output.is_empty()),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The index of the block absolute row `abs_row` belongs to: the newest one
/// starting at or above it, given each block's start row (see
/// [`Terminal::block_rows`](crate::terminal::session::Terminal::block_rows)). `None` above the first block.
pub(in crate::window) fn block_at_row(starts: &[usize], abs_row: usize) -> Option<usize> {
    starts.iter().rposition(|&start| start <= abs_row)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_block_at_row_finds_the_newest_block_at_or_above_the_row() {
        // Regression: `.find()` over an ascending list whose first element
        // is always 0 matched at once, so every lookup landed on the oldest
        // block whatever row was asked about.
        let starts = [0, 10, 25, 40];
        assert_eq!(block_at_row(&starts, 5), Some(0));
        assert_eq!(block_at_row(&starts, 10), Some(1));
        assert_eq!(block_at_row(&starts, 39), Some(2));
        assert_eq!(block_at_row(&starts, 100), Some(3));
        assert_eq!(block_at_row(&[3], 1), None, "above the first block");
        assert_eq!(
            block_at_row(&[0, 7, 7], 8),
            Some(2),
            "a shared row is the newest's"
        );
    }
}

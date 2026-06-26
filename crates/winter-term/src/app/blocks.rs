//! Block fold / yank / focus / navigate operations.

use crate::model::input;
use crate::model::layout::PaneId;

use super::App;

// ========================================================================
// App: block operations
// ========================================================================

impl App {
    pub(crate) fn yank_block_source(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let blocks = pane.scrollback().blocks();
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let text = offsets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &row)| row <= current)
            .and_then(|(idx, _)| blocks.get(idx))
            .map(|block| block.plain_text())
            .filter(|text| !text.is_empty());
        // Borrows of `pane` end here, freeing `self` for the clipboard handle.
        let Some(text) = text else {
            return;
        };
        if let Some(clipboard) = self.clipboard() {
            let copied = clipboard.set_text(&text).is_ok();
            if copied {
                self.set_notice("Copied to clipboard");
            }
        }
    }

    /// Export the plain-text content of the focused block to the system clipboard.
    pub(crate) fn export_focused_block_text(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let blocks = pane.scrollback().blocks();
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let text = offsets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &row)| row <= current)
            .and_then(|(idx, _)| blocks.get(idx))
            .map(|block| {
                let mut content = format!("$ {}\n", block.command);
                content.push_str(&block.plain_text());
                content
            });
        let Some(text) = text else {
            self.set_error("No block content to export");
            return;
        };
        if let Some(clipboard) = self.clipboard() {
            if clipboard.set_text(&text).is_ok() {
                self.set_notice("Copied block text to clipboard");
            } else {
                self.set_error("Could not copy to clipboard");
            }
        }
    }

    /// Export the SVG graphic in the focused block (if any) to a temporary file and open it.
    pub(crate) fn export_focused_block_svg(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let blocks = pane.scrollback().blocks();
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let svg = offsets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &row)| row <= current)
            .and_then(|(idx, _)| blocks.get(idx))
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
        match ::open::that(&path) {
            Ok(()) => self.set_notice(format!("Opened {}", path.display())),
            Err(e) => self.set_error(format!("Could not open SVG file: {e}")),
        }
    }

    pub(crate) fn toggle_fold(&mut self, focused: PaneId) {
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        let current = pane.grid().scroll_offset();
        let block_idx = offsets
            .iter()
            .enumerate()
            .rev()
            .find(|(_, &row)| row <= current)
            .map(|(i, _)| i);
        let Some(idx) = block_idx else {
            return;
        };

        let folded = self.folded_blocks.entry(focused).or_default();
        if folded.contains(&idx) {
            folded.remove(&idx);
            self.webview_mgr.unfold_block(focused, idx);
        } else {
            folded.insert(idx);
            self.webview_mgr.fold_block(focused, idx);
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }

    pub(crate) fn is_block_folded(&self, pane_id: PaneId, block_index: usize) -> bool {
        self.folded_blocks
            .get(&pane_id)
            .is_some_and(|set| set.contains(&block_index))
    }

    pub(crate) fn focus_block(&mut self, nav: input::BlockNav, focused: PaneId) {
        let pane = match self.panes.get(&focused) {
            Some(p) => p,
            None => return,
        };
        let cols = pane.grid().cols();
        let offsets = pane.scrollback().block_row_offsets(cols);
        if offsets.is_empty() {
            return;
        }
        let current_offset = pane.grid().scroll_offset();
        let target_row = match nav {
            input::BlockNav::Next => offsets
                .iter()
                .find(|&&row| row > current_offset)
                .copied()
                .unwrap_or_else(|| offsets.last().copied().unwrap_or(0)),
            input::BlockNav::Previous => offsets
                .iter()
                .rev()
                .find(|&&row| row < current_offset)
                .copied()
                .unwrap_or(0),
        };
        let diff = target_row.abs_diff(current_offset);
        let grid = self.panes.get_mut(&focused).unwrap().grid_mut();
        if target_row > current_offset {
            grid.scroll_up_history(diff);
        } else {
            grid.scroll_down_history(diff);
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
    fn test_export_focused_block_text_with_no_pane() {
        let mut app = App::new();
        app.export_focused_block_text(PaneId(999));
        assert!(app.notice.is_none());
    }

    #[test]
    fn test_is_block_folded_defaults_to_false() {
        let app = App::new();
        assert!(!app.is_block_folded(PaneId(1), 0));
    }
}

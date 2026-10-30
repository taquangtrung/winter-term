//! `f`/`t` and their labelled overlay: every visible match of a character
//! labelled with a key, jumped to with one more keystroke.

use crate::model::input::FindChar;
use crate::model::layout::TabId;
use crate::model::mode::Mode;

use crate::window::{App, FindLabel};

use super::vim;

// ========================================================================
// Constants
// ========================================================================

/// Label keys for the `f`/`t` jump overlay, home-row first so the common cases sit
/// under the fingers. Lowercase only: the overlay is dismissed by anything else,
/// so uppercase keys stay free.
const FIND_LABELS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
    'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

// ========================================================================
// App: find-char
// ========================================================================

impl App {
    /// `f`/`F`/`t`/`T`: show the easymotion-style jump overlay for the character
    /// just typed. Every landing spot on the visible screen in the search direction
    /// gets a lowercase label; a single candidate is jumped to straight away (no
    /// overlay to read), and none leaves the cursor put.
    ///
    /// Returns `true` when the overlay is now up, so the caller can park the
    /// keymap on [`PendingPrefix::FindLabel`](crate::model::input::PendingPrefix::FindLabel) to catch the label key.
    pub(in crate::window) fn find_char_overlay(&mut self, find: FindChar, focused: TabId) -> bool {
        self.chrome.labels.find = None;
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return false;
        };
        let grid = terminal.grid();
        let (row, col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let row = row.min(grid.rows().saturating_sub(1));
        let targets = vim::find_char_targets(grid, grid.rows(), row, col, find);

        match targets.len() {
            0 => false,
            1 => {
                self.move_nav_cursor_to(focused, targets[0]);
                false
            }
            _ => {
                self.chrome.labels.find = Some(
                    targets
                        .iter()
                        .zip(FIND_LABELS)
                        .map(|(&(row, col), &label)| FindLabel { col, label, row })
                        .collect(),
                );
                self.dirty = true;
                true
            }
        }
    }

    /// Jump to the labelled spot the user picked from the `f`/`t` overlay, and take
    /// the overlay down. An unknown label just dismisses it.
    pub(in crate::window) fn find_jump(&mut self, focused: TabId, label: char) {
        let target = self
            .chrome.labels
            .find
            .take()
            .and_then(|labels| labels.iter().find(|l| l.label == label).copied());
        if let Some(target) = target {
            // An easymotion-style label jump is one of vim's jumplist jumps.
            self.push_jump(focused);
            self.move_nav_cursor_to(focused, (target.row, target.col));
        }
        self.dirty = true;
    }

    /// Take down the `f`/`t` overlay without moving.
    pub(in crate::window) fn clear_find_labels(&mut self) {
        if self.chrome.labels.find.take().is_some() {
            self.dirty = true;
        }
    }

    /// Move the traversal cursor to a char-search (`f`/`F`/`t`/`T`) target on the
    /// current line. A miss leaves the cursor put. Extends the Visual selection
    /// when the focused pane is in Visual mode, mirroring [`Self::move_nav_cursor`].
    pub(in crate::window) fn find_char_move(&mut self, find: FindChar, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (row, col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let row = row.min(grid.rows().saturating_sub(1));
        let line = vim::line_chars(grid, row);
        let Some(target) = vim::find_char(&line, col, find) else {
            return;
        };
        let col = target.min(vim::nav_line_end(grid, row));

        self.set_nav_cursor(focused, (row, col));
        if self.open_tabs.mode(&focused) == Some(Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }
}

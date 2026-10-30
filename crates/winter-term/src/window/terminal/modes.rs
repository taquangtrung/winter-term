//! A terminal tab's modes: switching between Insert, Normal, and Visual, and
//! what entering or leaving each does to the cursor, the search, and the
//! status bar.

use crate::model::layout::TabId;
use crate::model::mode::Mode;

use crate::window::App;

// ========================================================================
// App: modes
// ========================================================================

impl App {
    /// Move `focused` into `new_mode`, running the bookkeeping each side of the
    /// transition owes: what the old mode must hand back, then what the new one
    /// needs set up.
    pub(in crate::window) fn switch_mode(&mut self, new_mode: Mode, focused: TabId) {
        let old_mode = self.open_tabs.mode(&focused).unwrap_or_default();
        self.open_tabs.set_mode(&focused, new_mode);
        // A pending `f`/`t` overlay belongs to the mode it was raised in.
        self.clear_find_labels();
        self.leave_mode(old_mode, new_mode, focused);
        self.enter_mode(old_mode, new_mode, focused);
        self.dirty = true;
    }

    /// Bookkeeping owed to the mode being left: the jumplist entry, the `gv`
    /// selection, and the live search.
    fn leave_mode(&mut self, old_mode: Mode, new_mode: Mode, focused: TabId) {
        // Leaving Normal altogether (back to Insert, into a block) is one of
        // vim's jumplist jumps: the browsing position is recorded so `C-o`
        // after the next Escape returns to it. Recorded before the nav cursor is
        // cleared in `enter_mode`.
        if old_mode == Mode::Normal && new_mode != Mode::Normal {
            self.push_jump(focused);
        }
        // Leaving Visual: remember the selection for `gv`, then clear it and the
        // anchor.
        if old_mode == Mode::Visual {
            self.remember_visual(focused);
            self.selection.visual_anchor = None;
            self.selection.span = None;
        }
        // Leaving Normal (browsing matches with `/`, `n`/`N`) without going
        // through `SearchCancel` (e.g. `i`/`a`/`o` back to Insert once done, or
        // focusing a block) still ends the search.
        if old_mode == Mode::Normal && new_mode != Mode::Normal {
            self.dismiss_search();
        }
    }

    /// Bookkeeping owed to the mode being entered: seeding or clearing the nav
    /// cursor, and handing the prompt back to the shell on the way into Insert.
    fn enter_mode(&mut self, old_mode: Mode, new_mode: Mode, focused: TabId) {
        match new_mode {
            // Entering Normal afresh (from Insert/Block) seeds the nav cursor at
            // the prompt; returning from Visual keeps it put.
            Mode::Normal => {
                if old_mode != Mode::Normal && old_mode != Mode::Visual {
                    self.init_nav_cursor(focused);
                }
            }
            Mode::Insert => {
                if new_mode == Mode::Insert {
                    self.align_prompt_to_nav_cursor(focused);
                }
                self.clear_nav_cursor(focused);
            }
            Mode::Visual => {}
        }
    }
}

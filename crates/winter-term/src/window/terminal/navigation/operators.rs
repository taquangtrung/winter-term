//! Vim operators over the scrollback and the prompt: yanking lines, motions,
//! and text objects, selecting them, and deleting, changing, or surrounding
//! them on the shell's prompt.

use crate::model::input::{CursorMove, TextObject, TextObjectSpec, VisualKind};
use crate::model::layout::TabId;
use crate::model::mode::{Mode, ModeEvent};
use crate::model::textedit::modal::shared;

use crate::window::terminal::prompt_edit::{
    prompt_add_surround_bytes, prompt_change_surround_bytes, prompt_delete_surround_bytes,
    PromptDelete,
};
use crate::window::{App, Selection};

use super::vim;

// ========================================================================
// Functions
// ========================================================================

/// The cell one step before `(row, col)` in a grid `cols` wide: the column to
/// its left, or the end of the row above when it sits in the first one. What
/// an exclusive backward motion yanks up to, since the cursor's own cell is
/// not taken.
fn cell_before(cols: usize, (row, col): (usize, usize)) -> (usize, usize) {
    match (col, row) {
        (0, 0) => (0, 0),
        (0, _) => (row - 1, cols.saturating_sub(1)),
        _ => (row, col - 1),
    }
}

// ========================================================================
// App: operators
// ========================================================================

impl App {
    /// `M-i`: select the paragraph the cursor sits in, linewise: vim's `vip`
    /// in one chord. A paragraph is the run of non-blank lines around the cursor;
    /// on a blank line it's the run of blank lines instead, as `ip` does.
    ///
    /// The run is found within the visible rows: the Visual anchor and nav cursor
    /// are viewport coordinates, so a paragraph continuing above or below the
    /// screen is selected up to the screen's edge.
    pub(in crate::window) fn select_paragraph(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let rows = grid.rows();
        if rows == 0 {
            return;
        }
        let (cursor_row, _) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let row = cursor_row.min(rows - 1);

        let blank = |r: usize| vim::line_chars(grid, r).iter().all(|c| c.is_whitespace());
        let want_blank = blank(row);
        let mut start = row;
        while start > 0 && blank(start - 1) == want_blank {
            start -= 1;
        }
        let mut end = row;
        while end + 1 < rows && blank(end + 1) == want_blank {
            end += 1;
        }

        let anchor = (grid.to_absolute_row(start), 0);
        self.open_tabs
            .set_mode(&focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.selection.visual_kind = VisualKind::Line;
        self.selection.visual_anchor = Some(anchor);
        self.set_nav_cursor(focused, (end, 0));
        self.update_visual_selection(focused);
        self.dirty = true;
    }

    /// Select the specified text object (`iw`, `a"`, `i(`, etc.) around or inside the cursor.
    /// `yy`: copy the whole line the cursor is on.
    pub(in crate::window) fn yank_line(&mut self, focused: TabId, register: Option<char>) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (row, _) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let abs_row = grid.to_absolute_row(row);
        let last_col = grid.cols().saturating_sub(1);
        self.yank_absolute_span(focused, (abs_row, 0), (abs_row, last_col), register);
    }

    /// `y{motion}`: copy the text between the cursor and where the motion
    /// lands. A backward motion yanks up to the cursor without taking the cell
    /// it sits on, the way Vim's own exclusive motions do.
    pub(in crate::window) fn yank_motion(
        &mut self,
        focused: TabId,
        motion: CursorMove,
        register: Option<char>,
    ) {
        let cursor = self.nav_cursor(focused);
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        // The word motions take the grid mutably: at the viewport's edge they
        // scroll to reach the line they land on, exactly as the same motion
        // does when it moves the cursor.
        let grid = terminal.grid_mut();
        let rows = grid.rows();
        let (row, col) = cursor.unwrap_or_else(|| grid.cursor());
        let landing = match motion {
            CursorMove::WordEnd => vim::motion_word_end(grid, rows, row, col, false),
            CursorMove::WordEndBig => vim::motion_word_end(grid, rows, row, col, true),
            CursorMove::WordBack => vim::motion_word_back(grid, row, col, false),
            CursorMove::WordBackBig => vim::motion_word_back(grid, row, col, true),
            CursorMove::LineEnd => (row, vim::nav_line_end(grid, row)),
            CursorMove::LineStart => (row, 0),
            // Only the motions the yank operator offers reach here.
            _ => return,
        };
        let cols = grid.cols();
        let (start, end) = if landing < (row, col) {
            (landing, cell_before(cols, (row, col)))
        } else {
            ((row, col), landing)
        };
        if end < start {
            return;
        }
        let start = (grid.to_absolute_row(start.0), start.1);
        let end = (grid.to_absolute_row(end.0), end.1);
        self.yank_absolute_span(focused, start, end, register);
    }

    /// `yi{object}`/`ya{object}`: copy the text object under the cursor.
    pub(in crate::window) fn yank_text_object(
        &mut self,
        focused: TabId,
        spec: TextObjectSpec,
        register: Option<char>,
    ) {
        let cursor = self.nav_cursor(focused);
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (row, col) = cursor.unwrap_or_else(|| grid.cursor());
        let abs_row = grid.to_absolute_row(row);
        let Some((start, end)) =
            vim::text_object_span(grid, abs_row, col, spec.around, spec.object)
        else {
            return;
        };
        if end < start {
            return;
        }
        self.yank_absolute_span(focused, start, end, register);
    }

    /// Copy the text between two absolute positions into `register`, or the
    /// clipboard when there is none, and leave it lit for a moment so what the
    /// yank took is visible rather than only reported.
    pub(in crate::window) fn yank_absolute_span(
        &mut self,
        focused: TabId,
        start: (usize, usize),
        end: (usize, usize),
        register: Option<char>,
    ) {
        let Some(grid) = self.selection_grid(focused) else {
            return;
        };
        let last_col = grid.cols().saturating_sub(1);
        self.selection.span = Some(Selection {
            block: false,
            end_col: end.1.min(last_col),
            end_row: end.0,
            tab: focused,
            start_col: start.1.min(last_col),
            start_row: start.0,
        });
        let Some(text) = self.selected_text() else {
            self.selection.span = None;
            return;
        };
        self.set_auto_mark(focused, '[', start);
        self.set_auto_mark(focused, ']', end);
        match register {
            // The clipboard registers and the unnamed one mean the clipboard,
            // which is what a yank with no register named goes to as well.
            Some(reg) if !matches!(reg, '+' | '*' | '"') => {
                shared::set_register_text(reg, text);
                self.set_notice(format!("Yanked into register \"{reg}"));
            }
            _ => self.copy_selection(),
        }
        self.flash_yank();
    }

    pub(in crate::window) fn select_text_object(
        &mut self,
        focused: TabId,
        around: bool,
        object: TextObject,
    ) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (row, col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let abs_row = grid.to_absolute_row(row);

        let Some(((abs_r1, c1), (abs_r2, c2))) =
            vim::text_object_span(grid, abs_row, col, around, object)
        else {
            return;
        };

        if abs_r1 > abs_r2 || (abs_r1 == abs_r2 && c1 > c2) {
            self.jump_to(focused, (abs_r1, c1));
            return;
        }

        self.open_tabs
            .set_mode(&focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.selection.visual_kind = VisualKind::Char;

        self.reveal_position(focused, (abs_r2, c2));
        if let Some(terminal) = self.open_tabs.terminal(&focused) {
            let grid = terminal.grid();
            let top = grid.scrollback_len() - grid.scroll_offset().min(grid.scrollback_len());
            let view_r2 = abs_r2
                .saturating_sub(top)
                .min(grid.rows().saturating_sub(1));
            self.selection.visual_anchor = Some((abs_r1, c1));
            self.set_nav_cursor(focused, (view_r2, c2));
            self.update_visual_selection(focused);
            self.dirty = true;
        }
    }

    /// Delete the specified text object on the editable prompt line.
    pub(in crate::window) fn delete_text_object(
        &mut self,
        focused: TabId,
        around: bool,
        object: TextObject,
    ) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot delete: not on the editable prompt line");
            return;
        }
        let abs_row = grid.to_absolute_row(prompt_row);
        let Some(((abs_r1, c1), (abs_r2, c2))) =
            vim::text_object_span(grid, abs_row, nav_col, around, object)
        else {
            return;
        };
        if abs_r1 != abs_row || abs_r2 != abs_row || c1 > c2 {
            return;
        }
        self.delete_on_prompt(
            PromptDelete::Range {
                start_col: c1,
                end_col: c2,
            },
            focused,
        );
    }

    /// Change the specified text object on the editable prompt line (delete and enter Insert mode).
    pub(in crate::window) fn change_text_object(
        &mut self,
        focused: TabId,
        around: bool,
        object: TextObject,
    ) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot change: not on the editable prompt line");
            return;
        }
        let abs_row = grid.to_absolute_row(prompt_row);
        let Some(((abs_r1, c1), (abs_r2, c2))) =
            vim::text_object_span(grid, abs_row, nav_col, around, object)
        else {
            return;
        };
        if abs_r1 != abs_row || abs_r2 != abs_row || c1 > c2 {
            return;
        }
        self.delete_on_prompt(
            PromptDelete::Range {
                start_col: c1,
                end_col: c2,
            },
            focused,
        );
        self.open_tabs.set_mode(&focused, Mode::Insert);
        self.selection.span = None;
        self.selection.visual_anchor = None;
        self.dirty = true;
    }

    /// Delete the surrounding delimiter pair on the editable prompt line.
    pub(in crate::window) fn delete_surround(&mut self, focused: TabId, target: char) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot delete surround: not on the editable prompt line");
            return;
        }
        let abs_row = grid.to_absolute_row(prompt_row);
        let Some(((abs_r1, c1), (abs_r2, c2))) =
            vim::surround_pair_positions(grid, abs_row, nav_col, target)
        else {
            return;
        };
        if abs_r1 != abs_row || abs_r2 != abs_row || c1 >= c2 {
            return;
        }
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(
            terminal,
            &[nav_col, c1, c2],
        )) {
            return;
        }
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.desync();
        }
        let bytes = prompt_delete_surround_bytes(pty_col, c1, c2, nav_col);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.term.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Change the surrounding delimiter pair `target` to `replacement` on the editable prompt line.
    pub(in crate::window) fn change_surround(
        &mut self,
        focused: TabId,
        target: char,
        replacement: char,
    ) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot change surround: not on the editable prompt line");
            return;
        }
        let abs_row = grid.to_absolute_row(prompt_row);
        let Some(((abs_r1, c1), (abs_r2, c2))) =
            vim::surround_pair_positions(grid, abs_row, nav_col, target)
        else {
            return;
        };
        if abs_r1 != abs_row || abs_r2 != abs_row || c1 >= c2 {
            return;
        }
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(
            terminal,
            &[nav_col, c1, c2],
        )) {
            return;
        }
        let Some((open, close, _)) = vim::surround_pair_chars(replacement) else {
            return;
        };
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.desync();
        }
        let bytes = prompt_change_surround_bytes(pty_col, c1, open, c2, close, nav_col);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.term.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Wrap a text object on the editable prompt line with the given delimiter.
    pub(in crate::window) fn surround_text_object(
        &mut self,
        focused: TabId,
        around: bool,
        object: TextObject,
        delimiter: char,
    ) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot surround: not on the editable prompt line");
            return;
        }
        let abs_row = grid.to_absolute_row(prompt_row);
        let Some(((abs_r1, c1), (abs_r2, c2))) =
            vim::text_object_span(grid, abs_row, nav_col, around, object)
        else {
            return;
        };
        if abs_r1 != abs_row || abs_r2 != abs_row || c1 > c2 {
            return;
        }
        let Some((open, close, _)) = vim::surround_pair_chars(delimiter) else {
            return;
        };
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.desync();
        }
        let bytes = prompt_add_surround_bytes(pty_col, c1, c2, open, close, nav_col);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.term.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Delete the current Visual selection if it sits on the editable prompt line.
    pub(in crate::window) fn delete_selection(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, _) = grid.cursor();
        let abs_prompt_row = grid.to_absolute_row(prompt_row);

        let Some(sel) = self.selection.span.as_ref().filter(|s| s.tab == focused) else {
            return;
        };
        let (sr, sc, er, ec) = (
            sel.start_row.min(sel.end_row),
            sel.start_col.min(sel.end_col),
            sel.start_row.max(sel.end_row),
            sel.start_col.max(sel.end_col),
        );
        if sr != abs_prompt_row || er != abs_prompt_row {
            self.set_error("Cannot delete: selection not on the editable prompt line");
            return;
        }
        let (c1, c2) = if sel.block {
            (sc, ec)
        } else if sel.start_row == sel.end_row {
            (
                sel.start_col.min(sel.end_col),
                sel.start_col.max(sel.end_col),
            )
        } else {
            (0, grid.cols().saturating_sub(1))
        };
        self.open_tabs.set_mode(&focused, Mode::Normal);
        self.selection.visual_anchor = None;
        self.selection.span = None;
        self.delete_on_prompt(
            PromptDelete::Range {
                start_col: c1,
                end_col: c2,
            },
            focused,
        );
    }
}

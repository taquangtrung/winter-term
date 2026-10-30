//! Vim-style navigation over a terminal's scrollback: the normal-mode cursor
//! and its motions live here; the rest in the submodules:
//!
//! - [`history`]: the jump and change lists, marks, and `.` repeat.
//! - [`operators`]: yank, delete, change, and surround over text objects.
//! - [`find`]: `f`/`t` and their labelled overlay.
//! - [`search`]: in-pane text search.
//! - [`quick_select`]: one-key jumps to visible block boundaries.
//! - [`open`]: opening what the cursor points at.
//! - [`reading`]: sentence bands and rainbow parens.
//! - [`swoop`]: fuzzy line search over the scrollback.
//! - [`export`]: the scrollback as text, ANSI, or HTML.
//! - [`vim`]: word and line motion helpers over the grid.

pub(crate) mod export;
pub(crate) mod find;
pub(crate) mod history;
pub(crate) mod open;
pub(crate) mod operators;
mod quick_select;
pub(crate) mod reading;
pub(crate) mod search;
pub(crate) mod swoop;
pub(crate) mod vim;

use std::collections::HashMap;

use crate::model::input::{self, CursorMove};
use crate::model::layout::TabId;
use crate::model::mode::Mode;

use crate::window::App;

use self::history::{InsertSession, LastChange};
use crate::model::vim::jumps::{ChangeList, JumpList};
use crate::window::TerminalState;

pub(in crate::window) use open::resolve_existing_path;
pub(in crate::window) use search::SearchState;

// ========================================================================
// Constants
// ========================================================================

/// How many lines `gg` and `G` ask a full-screen app to scroll: its buffer's
/// length is the app's to know, so this is simply further than a conversation
/// is likely to reach.
const ALT_SCREEN_JUMP_LINES: isize = 200;

// ========================================================================
// Data Structures
// ========================================================================

/// Vim bookkeeping that survives across commands: each tab's own, and the
/// registers every tab shares.
#[derive(Default)]
pub(in crate::window) struct VimState {
    /// Each tab's own: closing a tab is one removal here.
    pub(in crate::window) tabs: HashMap<TabId, VimTab>,
}

/// One tab's vim bookkeeping: the jumplist, the changelist, dot-repeat, and
/// marks.
#[derive(Default)]
pub(in crate::window) struct VimTab {
    /// The vim-style changelist backing `g;`/`g,` (see [`ChangeList`]).
    pub(in crate::window) change_list: ChangeList,
    /// The Insert-mode typing run in progress (see [`InsertSession`]).
    pub(in crate::window) insert_session: Option<InsertSession>,
    /// The vim-style jumplist backing `C-o`/`C-i` (see [`JumpList`]).
    pub(in crate::window) jump_list: JumpList,
    /// The most recent change, replayed by `.` (see [`LastChange`]).
    pub(in crate::window) last_change: Option<LastChange>,
    /// Named marks, as `mark_char -> (abs_row, col)`.
    pub(in crate::window) marks: HashMap<char, (usize, usize)>,
}

impl VimState {
    /// Tab `tab`'s bookkeeping, if it has any yet.
    pub(in crate::window) fn tab(&self, tab: TabId) -> Option<&VimTab> {
        self.tabs.get(&tab)
    }

    /// Tab `tab`'s bookkeeping, started empty if it has none yet.
    pub(in crate::window) fn tab_mut(&mut self, tab: TabId) -> &mut VimTab {
        self.tabs.entry(tab).or_default()
    }
}

/// The motions whose origin belongs on the jumplist: whole-viewport jumps and
/// paragraph/bracket hops, but not ordinary character/word/line moves, which
/// would bury every useful jump under noise.
fn pushes_jump(mv: CursorMove) -> bool {
    matches!(
        mv,
        CursorMove::Top
            | CursorMove::Bottom
            | CursorMove::ScreenTop
            | CursorMove::ScreenMiddle
            | CursorMove::ScreenBottom
            | CursorMove::ParagraphBack
            | CursorMove::ParagraphForward
            | CursorMove::MatchingBracket
    )
}

/// The viewport row a `j`/`k` motion lands on when every rich block's reserved
/// band in `spans` (viewport rows `[start, end)`) counts as a single stop.
///
/// Entering a band parks the cursor on its first row, where the cursor draws
/// as the block-as-cursor outline instead of a 1-cell block in blank space;
/// leaving it jumps to the first row past the band. A block therefore costs
/// one stop in each direction instead of one keypress per blank row behind
/// it, and the cursor never sits unseen inside a band it cannot annotate.
fn vertical_band_stop(spans: &[(usize, usize)], row: usize, down: bool) -> usize {
    if down {
        // Already inside a band: leave at its end (the furthest, should bands
        // ever overlap).
        if let Some(&(_, end)) = spans
            .iter()
            .filter(|&&(start, end)| row >= start && row < end)
            .max_by_key(|&&(_, end)| end)
        {
            return end;
        }
        // About to enter one: park on its first row.
        if let Some(&(start, _)) = spans.iter().find(|&&(start, _)| start == row + 1) {
            return start;
        }
        row + 1
    } else {
        // The row above is inside a band: park on its first row (the earliest,
        // should bands ever overlap).
        if let Some(&(start, _)) = spans
            .iter()
            .filter(|&&(start, end)| row > start && row <= end)
            .min_by_key(|&&(start, _)| start)
        {
            return start;
        }
        row.saturating_sub(1)
    }
}

// ========================================================================
// App: normal-mode cursor
// ========================================================================

impl App {
    /// Terminal tab `tab`'s Normal/Visual traversal cursor, if it has one.
    /// Because the cursor is kept with its tab, this never returns a position
    /// belonging to a different one: the previous bug where switching panes
    /// leaked the source pane's cursor into the destination (landing it before
    /// the first typeable column) cannot occur.
    pub(in crate::window) fn nav_cursor(&self, tab: TabId) -> Option<(usize, usize)> {
        self.open_tabs.nav_cursor(&tab)
    }

    /// The traversal cursor as an absolute `(row, col)`, which is how the
    /// Visual anchor and the `gv` snapshot both address text.
    pub(in crate::window) fn absolute_nav_cursor(&self, terminal: TabId) -> Option<(usize, usize)> {
        let (row, col) = self.nav_cursor(terminal)?;
        Some((
            self.open_tabs
                .terminal(&terminal)?
                .grid()
                .to_absolute_row(row),
            col,
        ))
    }

    /// Record terminal tab `tab`'s traversal cursor.
    pub(in crate::window) fn set_nav_cursor(&mut self, tab: TabId, pos: (usize, usize)) {
        self.open_tabs.set_nav_cursor(&tab, Some(pos));
    }

    /// Drop terminal tab `tab`'s traversal cursor (called when it leaves Normal).
    pub(in crate::window) fn clear_nav_cursor(&mut self, tab: TabId) {
        self.open_tabs.set_nav_cursor(&tab, None);
    }

    /// Scroll `pos` into view, centering it when it's off screen, leaving the
    /// viewport alone when it isn't (Vim's minimal scrolling), and park the
    /// Normal-mode cursor on it. Shared by search jumps and the jumplist so
    /// later motions and `n`/`N` resume from the landed spot.
    pub(in crate::window) fn reveal_position(
        &mut self,
        focused: TabId,
        (abs_row, col): (usize, usize),
    ) {
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        let grid = terminal.grid_mut();
        let rows = grid.rows();
        let scrollback = grid.scrollback_len();
        let top = scrollback - grid.scroll_offset().min(scrollback);
        if abs_row < top || abs_row >= top + rows {
            // `offset = view_row + scrollback - abs_row` puts `abs_row` on
            // viewport row `view_row`; each end clamps itself (saturating at the
            // newest line, `set_scroll_offset` at the oldest).
            grid.set_scroll_offset((rows / 2 + scrollback).saturating_sub(abs_row));
        }
        let top = grid.scrollback_len() - grid.scroll_offset();
        let view_row = abs_row.saturating_sub(top).min(rows.saturating_sub(1));
        self.set_nav_cursor(focused, (view_row, col));
    }

    /// Scroll `pos` into view such that `abs_row` is vertically centered in the pane,
    /// and park the Normal-mode cursor on it.
    pub(in crate::window) fn reveal_position_centered(
        &mut self,
        focused: TabId,
        (abs_row, col): (usize, usize),
    ) {
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        let grid = terminal.grid_mut();
        let rows = grid.rows();
        let scrollback = grid.scrollback_len();
        grid.set_scroll_offset((rows / 2 + scrollback).saturating_sub(abs_row));
        let top = grid.scrollback_len() - grid.scroll_offset();
        let view_row = abs_row.saturating_sub(top).min(rows.saturating_sub(1));
        self.set_nav_cursor(focused, (view_row, col));
    }

    /// Park the traversal cursor on the cell the pointer is selecting, so the
    /// block cursor follows the mouse through clicks and selection drags.
    /// Insert-mode panes are skipped: they hide the traversal cursor (and
    /// re-derive it from the shell cursor via [`Self::init_nav_cursor`] when
    /// the pane next enters Normal), so a stale click position must not linger
    /// in `nav_cursors`.
    pub(in crate::window) fn track_nav_cursor_to_mouse(
        &mut self,
        tab: TabId,
        row: usize,
        col: usize,
    ) {
        if self
            .open_tabs
            .mode(&tab)
            .is_some_and(|m| matches!(m, Mode::Normal | Mode::Visual))
        {
            self.set_nav_cursor(tab, (row, col));
        }
    }

    /// Place the traversal cursor at the focused pane's terminal cursor, where
    /// the prompt sits, when entering Normal mode. The shell cursor marks the
    /// insertion point (one past the last typed character); Normal mode adopts
    /// that exact column so the cursor does not appear to jump when switching
    /// modes: only its shape changes (e.g. bar → block).
    pub(in crate::window) fn init_nav_cursor(&mut self, focused: TabId) {
        if let Some(terminal) = self.open_tabs.terminal(&focused) {
            let grid = terminal.grid();
            let (cursor_row, cursor_col) = grid.cursor();
            let row = cursor_row.min(grid.rows().saturating_sub(1));
            let col = cursor_col.min(grid.cols().saturating_sub(1));
            self.set_nav_cursor(focused, (row, col));
        }
    }

    /// Move the traversal cursor within the focused pane. Moves past a viewport
    /// edge scroll the grid's history instead, so the cursor reaches the whole
    /// buffer. Full-page moves scroll and snap the cursor to the new page's top
    /// or bottom row, like vim's `C-f`/`C-b`; half-page moves scroll and
    /// keep the cursor on its current screen row, like `C-d`/`C-u`.
    /// A rich block's reserved band counts as one stop for `j`/`k` (see
    /// [`vertical_band_stop`]).
    pub(in crate::window) fn move_nav_cursor(&mut self, mv: input::CursorMove, focused: TabId) {
        use CursorMove;

        // A jump-class motion records its origin for `C-o` before moving.
        if pushes_jump(mv) {
            self.push_jump(focused);
        }

        // The pane's block bands, gathered before the mutable pane borrow so
        // the two borrows of `self` don't overlap.
        let bands = self.term.block_bands(focused);
        // Read the stored cursor before mutably borrowing the terminal, since
        // both live in the same tab.
        let stored = self.nav_cursor(focused);
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return;
        };
        let grid = terminal.grid_mut();
        let rows = grid.rows();
        let cols = grid.cols();
        let (mut row, mut col) = stored.unwrap_or_else(|| grid.cursor());
        row = row.min(rows.saturating_sub(1));
        col = col.min(cols.saturating_sub(1));
        // The bands in viewport terms, clipped to the rows on screen and as
        // `[start, end)` spans (see [`vertical_band_stop`]): only these can be
        // motion stops or carry the block-as-cursor outline.
        let spans: Vec<(usize, usize)> = bands
            .iter()
            .filter_map(|&(abs_row, band_rows)| {
                let top = grid.to_viewport_row(abs_row);
                let bottom = top + band_rows as isize;
                let (start, end) = (top.max(0), bottom.min(rows as isize));
                (start < end).then_some((start as usize, end as usize))
            })
            .collect();

        // How far the motion wants the view scrolled (positive is up). The
        // alternate screen has no history to scroll, so this is asked of the
        // full-screen app instead once the cursor is placed.
        let mut app_scroll: isize = 0;

        match mv {
            CursorMove::Left => col = col.saturating_sub(1),
            CursorMove::Right => col += 1,
            CursorMove::Up => {
                if row > 0 {
                    row = vertical_band_stop(&spans, row, false);
                } else {
                    grid.scroll_up_history(1);
                    app_scroll = 1;
                }
            }
            CursorMove::Down => {
                if row < grid.last_content_row() {
                    row = vertical_band_stop(&spans, row, true);
                } else {
                    grid.scroll_down_history(1);
                    app_scroll = -1;
                }
            }
            CursorMove::LineStart => col = 0,
            CursorMove::LineEnd => col = cols,
            CursorMove::FirstNonBlank => col = vim::first_non_blank(&vim::line_chars(grid, row)),
            CursorMove::LastNonBlank => col = vim::last_non_blank(&vim::line_chars(grid, row)),
            CursorMove::WordForward => {
                (row, col) = vim::motion_word_forward(grid, rows, row, col, false)
            }
            CursorMove::WordForwardBig => {
                (row, col) = vim::motion_word_forward(grid, rows, row, col, true)
            }
            CursorMove::WordBack => (row, col) = vim::motion_word_back(grid, row, col, false),
            CursorMove::WordBackBig => (row, col) = vim::motion_word_back(grid, row, col, true),
            CursorMove::WordEnd => (row, col) = vim::motion_word_end(grid, rows, row, col, false),
            CursorMove::WordEndBig => (row, col) = vim::motion_word_end(grid, rows, row, col, true),
            CursorMove::WordEndBack => {
                (row, col) = vim::motion_word_end_back(grid, row, col, false)
            }
            CursorMove::WordEndBackBig => {
                (row, col) = vim::motion_word_end_back(grid, row, col, true)
            }
            CursorMove::ParagraphBack => {
                row = vim::motion_paragraph(grid, rows, row, false);
                col = 0;
            }
            CursorMove::ParagraphForward => {
                row = vim::motion_paragraph(grid, rows, row, true);
                col = 0;
            }
            CursorMove::ScreenTop => row = 0,
            CursorMove::ScreenMiddle => row = (rows / 2).min(grid.last_content_row()),
            CursorMove::ScreenBottom => row = grid.last_content_row(),
            CursorMove::MatchingBracket => {
                if let Some((r, c)) = vim::motion_matching_bracket(grid, rows, row, col) {
                    row = r;
                    col = c;
                }
            }
            // `zt`/`zz`/`zb`: scroll so the cursor's own line lands on the given
            // viewport row, keeping the cursor on that same line of the buffer.
            CursorMove::LineToTop | CursorMove::LineToCenter | CursorMove::LineToBottom => {
                let want = match mv {
                    CursorMove::LineToTop => 0,
                    CursorMove::LineToCenter => rows / 2,
                    _ => rows.saturating_sub(1),
                };
                let abs = grid.to_absolute_row(row);
                let scrollback = grid.scrollback_len();
                grid.set_scroll_offset((want + scrollback).saturating_sub(abs));
                let top = grid.scrollback_len() - grid.scroll_offset();
                row = abs.saturating_sub(top).min(rows.saturating_sub(1));
            }
            CursorMove::Top => {
                grid.set_scroll_offset(grid.scrollback_len());
                row = 0;
                col = 0;
                app_scroll = ALT_SCREEN_JUMP_LINES;
            }
            CursorMove::Bottom => {
                grid.set_scroll_offset(0);
                row = grid.last_content_row();
                app_scroll = -ALT_SCREEN_JUMP_LINES;
            }
            CursorMove::PageUp => {
                grid.scroll_up_history(rows);
                row = 0;
                app_scroll = rows as isize;
            }
            CursorMove::PageDown => {
                grid.scroll_down_history(rows);
                row = rows.saturating_sub(1);
                app_scroll = -(rows as isize);
            }
            CursorMove::HalfPageUp => {
                grid.scroll_up_history(rows / 2);
                app_scroll = (rows / 2) as isize;
            }
            CursorMove::HalfPageDown => {
                grid.scroll_down_history(rows / 2);
                app_scroll = -((rows / 2) as isize);
            }
        }

        // At the live bottom, the cursor can never sit below the last printed
        // row. Page/half-page motions only change the scroll offset above and
        // leave `row` untouched, so returning from scrolled-back history to a
        // pane with just a few lines of content would otherwise strand the
        // cursor past the last prompt line.
        if grid.scroll_offset() == 0 {
            row = row.min(grid.last_content_row());
        }

        // Respect each line's real end: never sit on the blank padding past the
        // last printed character (snapping to a shorter line on vertical moves).
        // The prompt row extends to the shell cursor so typed trailing whitespace
        // stays reachable.
        col = col.min(vim::nav_line_end(grid, row));
        self.set_nav_cursor(focused, (row, col));
        self.scroll_app_view(focused, app_scroll);
        self.dirty = true;
    }

    /// `gp`: Jump the navigation cursor directly to the active prompt row.
    pub(in crate::window) fn jump_to_prompt(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        self.push_jump(focused);
        self.set_nav_cursor(focused, (prompt_row, pty_col));
        if self.open_tabs.mode(&focused) == Some(Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }

    /// `gP`: Jump the navigation cursor to the previous prompt / command start.
    pub(in crate::window) fn jump_to_previous_prompt(&mut self, focused: TabId) {
        self.push_jump(focused);
        self.focus_block(input::BlockNav::Previous, focused);
    }

    /// Park the traversal cursor on a viewport cell, extending the Visual selection
    /// when the pane is in Visual mode: the tail shared by the char-search jumps.
    fn move_nav_cursor_to(&mut self, focused: TabId, (row, col): (usize, usize)) {
        self.set_nav_cursor(focused, (row, col));
        if self.open_tabs.mode(&focused) == Some(Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }

}

// ========================================================================
// TerminalState
// ========================================================================

impl TerminalState {
    /// The reserved bands of every rich block in `tab_id`, as `(abs_row,
    /// rows)` pairs in anchor order, absolute so the bands keep naming their
    /// rows under scrolling.
    ///
    /// These spans are what the pane's grid reserved for a block, so they are
    /// where a vertical motion stops once (see [`vertical_band_stop`]) and
    /// what the block-as-cursor outline is drawn around.
    pub(in crate::window) fn block_bands(&self, tab_id: TabId) -> Vec<(usize, usize)> {
        let mut bands: Vec<(usize, usize)> = self
            .image_blocks
            .iter()
            .filter(|block| block.tab_id == tab_id)
            .map(|block| (block.abs_row, block.max_rows))
            .collect();
        bands.sort_unstable_by_key(|&(abs_row, _)| abs_row);
        bands
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

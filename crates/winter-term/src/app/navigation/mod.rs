//! Navigation submodules: vim-style cursor motions, text search, and
//! quick-select block jumping.

pub(crate) mod export;
mod open;
mod quick_select;
pub(crate) mod reading;
pub(crate) mod search;
pub(crate) mod swoop;
mod vim;

use std::collections::HashMap;

use crate::model::input::{self, CursorMove, FindChar, TextObject, VisualKind};
use crate::model::layout::PaneId;
use crate::model::mode::{Mode, ModeEvent};

use super::prompt_edit::PromptDelete;
use super::App;
use super::FindLabel;

pub(crate) use search::SearchState;

/// Label keys for the `f`/`t` jump overlay, home-row first so the common cases sit
/// under the fingers. Lowercase only: the overlay is dismissed by anything else,
/// so uppercase keys stay free.
const FIND_LABELS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
    'z', 'x', 'c', 'v', 'b', 'n', 'm',
];

/// How many jump origins a pane's [`JumpList`] keeps; older entries fall off
/// the front. Matches the addon-length scale vim itself uses.
const MAX_JUMP_LIST_LEN: usize = 100;

// ========================================================================
// Data Structures
// ========================================================================

/// Per-pane vim bookkeeping that survives across commands: the jumplist,
/// the changelist, dot-repeat, marks, and registers.
#[derive(Default)]
pub(crate) struct VimState {
    /// Per-pane vim-style changelists backing `g;`/`g,` (see
    /// [`ChangeList`]).
    pub(crate) change_lists: HashMap<PaneId, ChangeList>,
    /// The Insert-mode typing run in progress per pane (see
    /// [`InsertSession`]).
    pub(crate) insert_sessions: HashMap<PaneId, InsertSession>,
    /// Per-pane vim-style jumplists backing `Ctrl+O`/`Ctrl+I` (see
    /// [`JumpList`]).
    pub(crate) jump_lists: HashMap<PaneId, JumpList>,
    /// The most recent change per pane, replayed by `.` (see
    /// [`LastChange`]).
    pub(crate) last_changes: HashMap<PaneId, LastChange>,
    /// Per-pane named marks (`(PaneId, mark_char) -> (abs_row, col)`).
    pub(crate) marks: HashMap<(PaneId, char), (usize, usize)>,
    /// Vim named registers (`"{a-z}`, `"{0-9}`, `"+`, `"*`).
    pub(crate) registers: HashMap<char, String>,
}

/// A per-pane, vim-style jumplist: the absolute positions (row, col) the
/// cursor was at before each "long" jump — `gg`/`G`, `H`/`M`/`L`, `{`/`}`,
/// `%`, a confirmed search, an `f`/`t` label jump, leaving Normal for Insert —
/// oldest first. `Ctrl+O` walks back through them ([`App::jump_older`]),
/// `Ctrl+I`/`Tab` forward again ([`App::jump_newer`]).
#[derive(Default)]
pub(crate) struct JumpList {
    /// Jump origins, oldest first, capped at [`MAX_JUMP_LIST_LEN`].
    entries: Vec<(usize, usize)>,
    /// How far back from the newest entry the cursor currently sits; 0 means
    /// "at the live position, no jump navigation active" — the state every
    /// push resets to.
    index: usize,
    /// The live position [`JumpList::older`] stepped away from, so
    /// [`JumpList::newer`] can return to it after walking all the way back —
    /// vim appends the current position the first time `Ctrl+O` is pressed.
    return_pos: Option<(usize, usize)>,
}

impl JumpList {
    /// Record `origin` (absolute) as the newest jump origin. Stepping back
    /// first forks away everything the cursor had stepped back past, exactly
    /// like an undo history: a new jump from the past discards the abandoned
    /// future.
    pub(crate) fn push(&mut self, origin: (usize, usize)) {
        if self.index > 0 {
            let keep = self.entries.len() - self.index;
            self.entries.truncate(keep);
            self.index = 0;
            self.return_pos = None;
        }
        self.entries.push(origin);
        if self.entries.len() > MAX_JUMP_LIST_LEN {
            self.entries.remove(0);
        }
    }

    /// The origin `Ctrl+O` lands on next, or `None` when already at the oldest
    /// entry. `live` is the current non-jumplist position, remembered so the
    /// matching `Ctrl+I` can come back to it.
    fn older(&mut self, live: (usize, usize)) -> Option<(usize, usize)> {
        if self.entries.is_empty() {
            return None;
        }
        if self.index == 0 {
            self.return_pos = Some(live);
        }
        if self.index < self.entries.len() {
            self.index += 1;
        }
        self.entries.get(self.entries.len() - self.index).copied()
    }

    /// The position `Ctrl+I` lands on next: the next entry forward, or the
    /// remembered live position once the walk is exhausted. `None` when
    /// already at the live position.
    fn newer(&mut self) -> Option<(usize, usize)> {
        if self.index == 0 {
            return None;
        }
        self.index -= 1;
        if self.index == 0 {
            self.return_pos.take()
        } else {
            self.entries.get(self.entries.len() - self.index).copied()
        }
    }
}

/// How many change positions a pane's [`ChangeList`] keeps; older entries
/// fall off the front, matching vim's own changelist length.
const MAX_CHANGE_LIST_LEN: usize = 100;

/// A per-pane, vim-style changelist: the absolute positions where recorded
/// changes began — Normal-mode edits (`x`, `dw`, `cs`, puts) and Insert-mode
/// typing sessions — oldest first. `g;` walks back through them
/// ([`App::change_older`]), `g,` forward again ([`App::change_newer`]).
///
/// Wraps [`JumpList`]'s walk state (index and return position) but pushes
/// append-only: a changelist is a log of historical facts, so making a new
/// change while stepped back into the past keeps the entries ahead, unlike
/// the jumplist's undo-history forking. Consecutive duplicate positions
/// collapse so repeated edits at one spot (and `.`'s own replays) don't fill
/// the log with noise.
#[derive(Default)]
pub(crate) struct ChangeList(JumpList);

impl ChangeList {
    /// Record `pos` as the newest change position, collapsing it when it
    /// matches the previous one and resetting the walk to the live position.
    fn push(&mut self, pos: (usize, usize)) {
        if self.0.entries.last() == Some(&pos) {
            return;
        }
        self.0.entries.push(pos);
        if self.0.entries.len() > MAX_CHANGE_LIST_LEN {
            self.0.entries.remove(0);
        }
        self.0.index = 0;
        self.0.return_pos = None;
    }

    /// The position `g;` lands on next — [`JumpList::older`]'s walk, shared
    /// with the jumplist so the two lists step identically.
    fn older(&mut self, live: (usize, usize)) -> Option<(usize, usize)> {
        self.0.older(live)
    }

    /// The position `g,` lands on next — [`JumpList::newer`]'s walk.
    fn newer(&mut self) -> Option<(usize, usize)> {
        self.0.newer()
    }
}

/// One replayable unit for `.` — the pane's most recent change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum LastChange {
    /// A Normal-mode edit or put, replayed by re-dispatching the action so it
    /// recomputes its alignment and extent from the cursor's current position.
    Action(input::Action),
    /// An Insert-mode typing run, replayed by re-sending the exact bytes that
    /// were originally forwarded to the shell.
    Typed(Vec<u8>),
}

/// The Insert-mode visit in progress: the raw bytes forwarded so far (the
/// `.`-repeat run, accumulated by the key handler) and the absolute position
/// the session began at — the changelist entry a non-empty run pushes when
/// the pane leaves Insert. `anchor` is `None` for a pane that never entered
/// Insert through an `i`/`a`/`o` action (it started there).
#[derive(Default)]
pub(crate) struct InsertSession {
    pub(crate) anchor: Option<(usize, usize)>,
    pub(crate) run: Vec<u8>,
}

/// The Normal-mode dispatches that count as a vim "change" for `.` and the
/// changelist: every prompt-line edit and every put. Insert-mode typing is
/// tracked as a session instead (see [`App::track_change`]) — its `Edit`
/// actions belong to that session, not to `.` on their own.
fn is_change_action(action: &input::Action) -> bool {
    matches!(
        action,
        input::Action::DeleteCharForward
            | input::Action::DeleteLine
            | input::Action::DeleteSelection
            | input::Action::DeleteTextObject(..)
            | input::Action::DeleteToLineEnd
            | input::Action::DeleteToLineStart
            | input::Action::DeleteWordBack
            | input::Action::DeleteWordForward
            | input::Action::ChangeLine
            | input::Action::ChangeToLineEnd
            | input::Action::ChangeToLineStart
            | input::Action::ChangeWordBack
            | input::Action::ChangeWordForward
            | input::Action::ChangeTextObject(..)
            | input::Action::SubstituteChar
            | input::Action::ReplaceChar(..)
            | input::Action::ToggleCaseChar
            | input::Action::ChangeSearchMatch { .. }
            | input::Action::DeleteSearchMatch { .. }
            | input::Action::ChangeSurround { .. }
            | input::Action::DeleteSurround(..)
            | input::Action::SurroundTextObject { .. }
            | input::Action::Paste
            | input::Action::PasteRegister { .. }
    )
}

/// The motions whose origin belongs on the jumplist: whole-viewport jumps and
/// paragraph/bracket hops — but not ordinary character/word/line moves, which
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

// ========================================================================
// App: normal-mode cursor
// ========================================================================

impl App {
    /// The focused pane's Normal/Visual traversal cursor, if it has one. Because
    /// the cursor is stored per pane ([`App::nav_cursors`]), this never returns a
    /// position belonging to a different pane — the previous bug where switching
    /// panes leaked the source pane's cursor into the destination (landing it
    /// before the first typeable column) cannot occur.
    pub(crate) fn nav_cursor(&self, pane: PaneId) -> Option<(usize, usize)> {
        self.nav_cursors.get(&pane).copied()
    }

    /// Record the focused pane's traversal cursor.
    pub(crate) fn set_nav_cursor(&mut self, pane: PaneId, pos: (usize, usize)) {
        self.nav_cursors.insert(pane, pos);
    }

    /// Drop the focused pane's traversal cursor (called when it leaves Normal).
    pub(crate) fn clear_nav_cursor(&mut self, pane: PaneId) {
        self.nav_cursors.remove(&pane);
    }

    /// Record the cursor's current position (absolute) as the origin of a
    /// jump, so `Ctrl+O` can come back — called before jump-class motions
    /// land, before confirmed search steps, and when Normal mode ends.
    pub(crate) fn push_jump(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let Some((row, col)) = self.nav_cursor(focused) else {
            return;
        };
        let origin = (pane.grid().to_absolute_row(row), col);
        self.vim.jump_lists.entry(focused).or_default().push(origin);
    }

    /// `Ctrl+O`: step the cursor back to the previous recorded jump origin,
    /// revealing it if it scrolled off screen. Extends the Visual selection
    /// when the pane is in Visual mode, like any other cursor move.
    pub(crate) fn jump_older(&mut self, focused: PaneId) {
        let live = self.live_jump_position(focused);
        let target =
            live.and_then(|live| self.vim.jump_lists.entry(focused).or_default().older(live));
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// `Ctrl+I`/`Tab`: step the cursor forward through the jumplist, back
    /// toward the position the walk started from.
    pub(crate) fn jump_newer(&mut self, focused: PaneId) {
        let target = self.vim.jump_lists.entry(focused).or_default().newer();
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// `g;`: step the cursor back to where the previous recorded change
    /// began, revealing it if it scrolled off screen. Extends the Visual
    /// selection when the pane is in Visual mode, like any other cursor move.
    pub(crate) fn change_older(&mut self, focused: PaneId) {
        let live = self.live_jump_position(focused);
        let target = live.and_then(|live| {
            self.vim
                .change_lists
                .entry(focused)
                .or_default()
                .older(live)
        });
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// `g,`: step the cursor forward through the changelist, back toward the
    /// position the walk started from.
    pub(crate) fn change_newer(&mut self, focused: PaneId) {
        let target = self.vim.change_lists.entry(focused).or_default().newer();
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// Record `action`'s dispatch for `.` and the changelist. Called at the
    /// top of [`App::handle_action`] for every action: change-class
    /// Normal-mode edits become the pane's last change and push their start
    /// position; `i`/`a`/`o` open a typing session whose forwarded bytes
    /// accumulate (in the key handler) until the pane leaves Insert, when a
    /// non-empty run becomes the last change instead.
    pub(crate) fn track_change(&mut self, action: &input::Action, focused: PaneId) {
        match action {
            input::Action::EnterInsert(_)
            | input::Action::ChangeLine
            | input::Action::ChangeToLineEnd
            | input::Action::ChangeToLineStart
            | input::Action::ChangeWordBack
            | input::Action::ChangeWordForward
            | input::Action::ChangeTextObject(..)
            | input::Action::ChangeSearchMatch { .. }
            | input::Action::SubstituteChar => {
                let anchor = self.live_jump_position(focused);
                self.vim.insert_sessions.insert(
                    focused,
                    InsertSession {
                        anchor,
                        run: Vec::new(),
                    },
                );
            }
            input::Action::SwitchMode(new_mode) => {
                let was_insert = self.modes.get(&focused) == Some(&Mode::Insert);
                if was_insert && *new_mode != Mode::Insert {
                    self.finish_insert_session(focused);
                }
            }
            _ if is_change_action(action)
                && self.panes.get(&focused).is_some_and(|p| p.is_at_prompt()) =>
            {
                self.vim
                    .last_changes
                    .insert(focused, LastChange::Action(action.clone()));
                if let Some(pos) = self.live_jump_position(focused) {
                    self.vim.change_lists.entry(focused).or_default().push(pos);
                }
            }
            _ => {}
        }
    }

    /// Close the pane's Insert-mode typing session: a non-empty run becomes
    /// the pane's last change (replayed by `.`), and the session's anchor
    /// joins the changelist so `g;` returns to where the typing began.
    fn finish_insert_session(&mut self, focused: PaneId) {
        let Some(session) = self.vim.insert_sessions.remove(&focused) else {
            return;
        };
        if session.run.is_empty() {
            return;
        }
        if let Some(anchor) = session.anchor {
            self.vim
                .change_lists
                .entry(focused)
                .or_default()
                .push(anchor);
        }
        self.vim
            .last_changes
            .insert(focused, LastChange::Typed(session.run));
    }

    /// `.`: replay the pane's most recent change where the cursor sits now.
    /// A Normal-mode edit re-dispatches its action (recomputing its effect
    /// from the current position); an Insert-mode typing run re-sends its
    /// bytes verbatim — straight to the PTY, so the prompt shadow is desynced
    /// to keep later undo honest about a line it no longer models.
    pub(crate) fn repeat_last_change(&mut self, focused: PaneId) {
        let Some(change) = self.vim.last_changes.get(&focused).cloned() else {
            return;
        };
        match change {
            LastChange::Action(action) => self.handle_action(action, focused),
            LastChange::Typed(run) => {
                if self.panes.get(&focused).is_some_and(|p| p.is_at_prompt()) {
                    if let Some(pos) = self.live_jump_position(focused) {
                        self.vim.change_lists.entry(focused).or_default().push(pos);
                    }
                    if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
                        shadow.desync();
                    }
                }
                self.handle_action(input::Action::SendBytes(run), focused);
            }
        }
    }

    /// `m{a-z}`: record the current absolute cursor position in a named mark.
    pub(crate) fn set_mark(&mut self, focused: PaneId, mark: char) {
        if !mark.is_ascii_lowercase() {
            return;
        }
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let Some((row, col)) = self.nav_cursor(focused) else {
            return;
        };
        let abs_row = pane.grid().to_absolute_row(row);
        self.vim.marks.insert((focused, mark), (abs_row, col));
    }

    /// `` `{a-z} `` / `'{a-z}`: jump to a named mark recorded by [`Self::set_mark`].
    /// `exact` lands on the exact column; `false` lands on the line's first non-blank.
    /// Records the jump origin beforehand so `Ctrl+O` returns to the pre-jump location.
    pub(crate) fn goto_mark(&mut self, focused: PaneId, mark: char, exact: bool) {
        let Some(&(abs_row, col)) = self.vim.marks.get(&(focused, mark)) else {
            return;
        };
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
        let max_abs = (grid.scrollback_len() + grid.rows()).saturating_sub(1);
        let clamped_abs = abs_row.min(max_abs);
        let target_col = if exact {
            col
        } else {
            let chars = vim::absolute_row_chars(grid, clamped_abs);
            vim::first_non_blank(&chars)
        };
        self.push_jump(focused);
        self.jump_to(focused, (clamped_abs, target_col));
    }

    /// The cursor's current position in jump coordinates (absolute), when the
    /// pane and its nav cursor both exist.
    fn live_jump_position(&self, focused: PaneId) -> Option<(usize, usize)> {
        let pane = self.panes.get(&focused)?;
        let (row, col) = self.nav_cursor(focused)?;
        Some((pane.grid().to_absolute_row(row), col))
    }

    /// Land the cursor on an absolute position: scroll it into view (centering
    /// when off screen), park the nav cursor on it, and extend the Visual
    /// selection when the pane is in Visual mode.
    fn jump_to(&mut self, focused: PaneId, (abs_row, col): (usize, usize)) {
        self.reveal_position(focused, (abs_row, col));
        if self.modes.get(&focused) == Some(&Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }

    /// Scroll `pos` into view — centering it when it's off screen, leaving the
    /// viewport alone when it isn't (Vim's minimal scrolling) — and park the
    /// Normal-mode cursor on it. Shared by search jumps and the jumplist so
    /// later motions and `n`/`N` resume from the landed spot.
    pub(crate) fn reveal_position(&mut self, focused: PaneId, (abs_row, col): (usize, usize)) {
        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        let grid = pane.grid_mut();
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

    /// Park the traversal cursor on the cell the pointer is selecting, so the
    /// block cursor follows the mouse through clicks and selection drags.
    /// Insert-mode panes are skipped: they hide the traversal cursor (and
    /// re-derive it from the shell cursor via [`Self::init_nav_cursor`] when
    /// the pane next enters Normal), so a stale click position must not linger
    /// in `nav_cursors`.
    pub(crate) fn track_nav_cursor_to_mouse(&mut self, pane: PaneId, row: usize, col: usize) {
        if self
            .modes
            .get(&pane)
            .is_some_and(|m| matches!(m, Mode::Normal | Mode::Visual))
        {
            self.set_nav_cursor(pane, (row, col));
        }
    }

    /// Place the traversal cursor at the focused pane's terminal cursor, where
    /// the prompt sits, when entering Normal mode. The shell cursor marks the
    /// insertion point (one past the last typed character); Normal mode adopts
    /// that exact column so the cursor does not appear to jump when switching
    /// modes — only its shape changes (e.g. bar → block).
    pub(crate) fn init_nav_cursor(&mut self, focused: PaneId) {
        if let Some(pane) = self.panes.get(&focused) {
            let grid = pane.grid();
            let (cursor_row, cursor_col) = grid.cursor();
            let row = cursor_row.min(grid.rows().saturating_sub(1));
            let col = cursor_col.min(grid.cols().saturating_sub(1));
            self.set_nav_cursor(focused, (row, col));
        }
    }

    /// Move the traversal cursor within the focused pane. Moves past a viewport
    /// edge scroll the grid's history instead, so the cursor reaches the whole
    /// buffer. Full-page moves scroll and snap the cursor to the new page's top
    /// or bottom row, like vim's `Ctrl-F`/`Ctrl-B`; half-page moves scroll and
    /// keep the cursor on its current screen row, like `Ctrl-D`/`Ctrl-U`.
    pub(crate) fn move_nav_cursor(&mut self, mv: input::CursorMove, focused: PaneId) {
        use CursorMove;

        // A jump-class motion records its origin for `Ctrl+O` before moving.
        if pushes_jump(mv) {
            self.push_jump(focused);
        }

        // Read the stored cursor before mutably borrowing the pane so the two
        // borrows of `self` don't overlap (the method form borrows all of `self`).
        let stored = self.nav_cursors.get(&focused).copied();
        let Some(pane) = self.panes.get_mut(&focused) else {
            return;
        };
        let grid = pane.grid_mut();
        let rows = grid.rows();
        let cols = grid.cols();
        let (mut row, mut col) = stored.unwrap_or_else(|| grid.cursor());
        row = row.min(rows.saturating_sub(1));
        col = col.min(cols.saturating_sub(1));

        match mv {
            CursorMove::Left => col = col.saturating_sub(1),
            CursorMove::Right => col += 1,
            CursorMove::Up => {
                if row > 0 {
                    row -= 1;
                } else {
                    grid.scroll_up_history(1);
                }
            }
            CursorMove::Down => {
                if row < grid.last_content_row() {
                    row += 1;
                } else {
                    grid.scroll_down_history(1);
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
            }
            CursorMove::Bottom => {
                grid.set_scroll_offset(0);
                row = grid.last_content_row();
            }
            CursorMove::PageUp => {
                grid.scroll_up_history(rows);
                row = 0;
            }
            CursorMove::PageDown => {
                grid.scroll_down_history(rows);
                row = rows.saturating_sub(1);
            }
            CursorMove::HalfPageUp => grid.scroll_up_history(rows / 2),
            CursorMove::HalfPageDown => grid.scroll_down_history(rows / 2),
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
        self.dirty = true;
    }

    /// `gp`: Jump the navigation cursor directly to the active prompt row.
    pub(crate) fn jump_to_prompt(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = pane.grid().cursor();
        self.push_jump(focused);
        self.set_nav_cursor(focused, (prompt_row, pty_col));
        if self.modes.get(&focused) == Some(&Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }

    /// `gP`: Jump the navigation cursor to the previous prompt / command start.
    pub(crate) fn jump_to_previous_prompt(&mut self, focused: PaneId) {
        self.push_jump(focused);
        self.focus_block(input::BlockNav::Previous, focused);
    }

    /// `Alt-i`: select the paragraph the cursor sits in, linewise — vim's `vip`
    /// in one chord. A paragraph is the run of non-blank lines around the cursor;
    /// on a blank line it's the run of blank lines instead, as `ip` does.
    ///
    /// The run is found within the visible rows: the Visual anchor and nav cursor
    /// are viewport coordinates, so a paragraph continuing above or below the
    /// screen is selected up to the screen's edge.
    pub(crate) fn select_paragraph(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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

        self.modes
            .insert(focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.selection.visual_kind = VisualKind::Line;
        self.selection.visual_anchor = Some((start, 0));
        self.set_nav_cursor(focused, (end, 0));
        self.update_visual_selection(focused);
        self.dirty = true;
    }

    /// Select the specified text object (`iw`, `a"`, `i(`, etc.) around or inside the cursor.
    pub(crate) fn select_text_object(&mut self, focused: PaneId, around: bool, object: TextObject) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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

        self.modes
            .insert(focused, Mode::Normal.apply(ModeEvent::EnterVisual));
        self.selection.visual_kind = VisualKind::Char;

        self.reveal_position(focused, (abs_r2, c2));
        if let Some(pane) = self.panes.get(&focused) {
            let grid = pane.grid();
            let top = grid.scrollback_len() - grid.scroll_offset().min(grid.scrollback_len());
            let view_r1 = abs_r1
                .saturating_sub(top)
                .min(grid.rows().saturating_sub(1));
            let view_r2 = abs_r2
                .saturating_sub(top)
                .min(grid.rows().saturating_sub(1));
            self.selection.visual_anchor = Some((view_r1, c1));
            self.set_nav_cursor(focused, (view_r2, c2));
            self.update_visual_selection(focused);
            self.dirty = true;
        }
    }

    /// Delete the specified text object on the editable prompt line.
    pub(crate) fn delete_text_object(&mut self, focused: PaneId, around: bool, object: TextObject) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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
    pub(crate) fn change_text_object(&mut self, focused: PaneId, around: bool, object: TextObject) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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
        self.modes.insert(focused, Mode::Insert);
        self.selection.span = None;
        self.selection.visual_anchor = None;
        self.dirty = true;
    }

    /// Delete the surrounding delimiter pair on the editable prompt line.
    pub(crate) fn delete_surround(&mut self, focused: PaneId, target: char) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.desync();
        }
        let bytes = super::prompt_edit::prompt_delete_surround_bytes(pty_col, c1, c2, nav_col);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Change the surrounding delimiter pair `target` to `replacement` on the editable prompt line.
    pub(crate) fn change_surround(&mut self, focused: PaneId, target: char, replacement: char) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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
        let Some((open, close, _)) = vim::surround_pair_chars(replacement) else {
            return;
        };
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.desync();
        }
        let bytes =
            super::prompt_edit::prompt_change_surround_bytes(pty_col, c1, open, c2, close, nav_col);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Wrap a text object on the editable prompt line with the given delimiter.
    pub(crate) fn surround_text_object(
        &mut self,
        focused: PaneId,
        around: bool,
        object: TextObject,
        delimiter: char,
    ) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
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
        if let Some(shadow) = self.prompt_shadows.get_mut(&focused) {
            shadow.desync();
        }
        let bytes =
            super::prompt_edit::prompt_add_surround_bytes(pty_col, c1, c2, open, close, nav_col);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
        self.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Delete the current Visual selection if it sits on the editable prompt line.
    pub(crate) fn delete_selection(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
        let (prompt_row, _) = grid.cursor();
        let abs_prompt_row = grid.to_absolute_row(prompt_row);

        let Some(sel) = self.selection.span.as_ref().filter(|s| s.pane == focused) else {
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
        self.modes.insert(focused, Mode::Normal);
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

    /// `f`/`F`/`t`/`T`: show the easymotion-style jump overlay for the character
    /// just typed. Every landing spot on the visible screen in the search direction
    /// gets a lowercase label; a single candidate is jumped to straight away (no
    /// overlay to read), and none leaves the cursor put.
    ///
    /// Returns `true` when the overlay is now up, so the caller can park the
    /// keymap on [`input::PendingPrefix::FindLabel`] to catch the label key.
    pub(crate) fn find_char_overlay(&mut self, find: FindChar, focused: PaneId) -> bool {
        self.find_labels = None;
        let Some(pane) = self.panes.get(&focused) else {
            return false;
        };
        let grid = pane.grid();
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
                self.find_labels = Some(
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
    pub(crate) fn find_jump(&mut self, focused: PaneId, label: char) {
        let target = self
            .find_labels
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
    pub(crate) fn clear_find_labels(&mut self) {
        if self.find_labels.take().is_some() {
            self.dirty = true;
        }
    }

    /// Park the traversal cursor on a viewport cell, extending the Visual selection
    /// when the pane is in Visual mode — the tail shared by the char-search jumps.
    fn move_nav_cursor_to(&mut self, focused: PaneId, (row, col): (usize, usize)) {
        self.set_nav_cursor(focused, (row, col));
        if self.modes.get(&focused) == Some(&Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }

    /// Move the traversal cursor to a char-search (`f`/`F`/`t`/`T`) target on the
    /// current line. A miss leaves the cursor put. Extends the Visual selection
    /// when the focused pane is in Visual mode, mirroring [`Self::move_nav_cursor`].
    pub(crate) fn find_char_move(&mut self, find: FindChar, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let grid = pane.grid();
        let (row, col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let row = row.min(grid.rows().saturating_sub(1));
        let line = vim::line_chars(grid, row);
        let Some(target) = vim::find_char(&line, col, find) else {
            return;
        };
        let col = target.min(vim::nav_line_end(grid, row));

        self.set_nav_cursor(focused, (row, col));
        if self.modes.get(&focused) == Some(&Mode::Visual) {
            self.update_visual_selection(focused);
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

    /// A pane with enough scrollback (a `seq` run wider than the viewport) to
    /// exercise full-page motions against real history rather than the
    /// live-bottom edge case.
    fn pane_with_scrollback() -> crate::terminal::pane::Pane {
        let mut pane = crate::terminal::pane::Pane::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("bash"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        pane.write(b"seq 1 200\n");
        for _ in 0..100 {
            pane.drain_output();
            if pane.grid().scrollback_len() > pane.grid().rows() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            pane.grid().scrollback_len() > pane.grid().rows(),
            "fixture needs more than a page of scrollback"
        );
        pane
    }

    /// A pane whose visible grid holds `lines`, one per row from the top, written
    /// straight into the grid (no shell, no timing).
    fn pane_with_lines(lines: &[&str]) -> crate::terminal::pane::Pane {
        let cols = lines
            .iter()
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(20)
            .max(20)
            + 5;
        let rows = lines.len().max(8);
        let mut pane = crate::terminal::pane::Pane::with_command(
            cols,
            rows,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        let grid = pane.grid_mut();
        for (row, line) in lines.iter().enumerate() {
            grid.move_to(row, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        grid.move_to(0, 0);
        pane
    }

    /// Row `row`'s visible text with trailing blanks trimmed, for asserting on
    /// what the fixture's `cat` echoed back into the grid.
    fn row_text(app: &App, pane: PaneId, row: usize) -> String {
        let grid = app.panes[&pane].grid();
        (0..grid.cols())
            .map(|col| grid.visible_cell(row, col).map(|c| c.ch).unwrap_or(' '))
            .collect::<String>()
            .trim_end()
            .to_string()
    }

    /// `["alpha", "beta", "", "gamma", ...blank]` — one paragraph of two lines, a
    /// blank separator, then a one-line paragraph.
    fn app_with_paragraphs(cursor_row: usize) -> (App, PaneId) {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = app.tab().panes()[0];
        app.panes
            .insert(id, pane_with_lines(&["alpha", "beta", "", "gamma"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (cursor_row, 0));
        (app, id)
    }

    #[test]
    fn test_select_paragraph_covers_the_run_of_lines_around_the_cursor() {
        // `Alt-i` from the second line of the first paragraph selects both its
        // lines linewise and stops at the blank separator — vim's `vip`.
        let (mut app, id) = app_with_paragraphs(1);

        app.handle_action(input::Action::SelectParagraph, id);

        assert_eq!(app.modes.get(&id), Some(&Mode::Visual));
        assert_eq!(
            app.selection.visual_kind,
            VisualKind::Line,
            "paragraph selection is linewise"
        );
        let sel = app.selection.span.as_ref().expect("a selection");
        assert_eq!((sel.start_row, sel.end_row), (0, 1));
        assert_eq!(app.selected_text().as_deref(), Some("alpha\nbeta"));
        assert_eq!(
            app.nav_cursor(id),
            Some((1, 0)),
            "cursor ends on the last line"
        );
    }

    #[test]
    fn test_select_paragraph_from_a_one_line_paragraph_selects_only_that_line() {
        let (mut app, id) = app_with_paragraphs(3);

        app.handle_action(input::Action::SelectParagraph, id);

        let sel = app.selection.span.as_ref().expect("a selection");
        assert_eq!((sel.start_row, sel.end_row), (3, 3));
        assert_eq!(app.selected_text().as_deref(), Some("gamma"));
    }

    #[test]
    fn test_select_paragraph_on_a_blank_line_selects_the_blank_run() {
        // Vim's `ip` on a blank line takes the blank run instead; here that's the
        // single separator row.
        let (mut app, id) = app_with_paragraphs(2);

        app.handle_action(input::Action::SelectParagraph, id);

        let sel = app.selection.span.as_ref().expect("a selection");
        assert_eq!((sel.start_row, sel.end_row), (2, 2));
    }

    /// A pane whose grid has `lines` printed one per line-feed, so that with more
    /// lines than rows the earlier ones end up in scrollback.
    fn pane_with_history(lines: &[&str]) -> crate::terminal::pane::Pane {
        let mut pane = crate::terminal::pane::Pane::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        {
            let grid = pane.grid_mut();
            for (i, line) in lines.iter().enumerate() {
                for ch in line.chars() {
                    grid.print(ch);
                }
                if i + 1 < lines.len() {
                    grid.line_feed();
                    grid.move_to_column(0);
                }
            }
        }
        pane
    }

    #[test]
    fn test_find_overlay_labels_every_candidate_and_jumps_to_the_picked_one() {
        // `f a` on "banana" has three landing spots, so the overlay goes up with a
        // lowercase label on each; the label key jumps and takes it down.
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["banana"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        app.handle_action(
            input::Action::FindChar(FindChar {
                ch: 'a',
                forward: true,
                till: false,
            }),
            id,
        );

        let labels = app.find_labels.clone().expect("overlay is up");
        assert_eq!(
            labels.iter().map(|l| (l.row, l.col)).collect::<Vec<_>>(),
            vec![(0, 1), (0, 3), (0, 5)]
        );
        assert!(
            labels.iter().all(|l| l.label.is_ascii_lowercase()),
            "labels are lowercase"
        );
        assert_eq!(app.pending, input::PendingPrefix::FindLabel);
        assert_eq!(app.nav_cursor(id), Some((0, 0)), "nothing moves yet");

        let second = labels[1].label;
        app.handle_action(input::Action::FindJump(second), id);

        assert_eq!(app.nav_cursor(id), Some((0, 3)));
        assert!(
            app.find_labels.is_none(),
            "overlay taken down after the jump"
        );
    }

    #[test]
    fn test_find_overlay_is_skipped_for_a_single_candidate() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["abc"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        app.handle_action(
            input::Action::FindChar(FindChar {
                ch: 'c',
                forward: true,
                till: false,
            }),
            id,
        );

        assert!(app.find_labels.is_none(), "one candidate needs no overlay");
        assert_eq!(app.nav_cursor(id), Some((0, 2)), "it just jumps");
        assert_eq!(app.pending, input::PendingPrefix::None);
    }

    #[test]
    fn test_find_overlay_spans_the_visible_rows_and_respects_till() {
        // Easymotion-style: candidates come from the whole screen in the search
        // direction, and `t` lands one cell short of each target.
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["--a--", "--a--"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        app.handle_action(
            input::Action::FindChar(FindChar {
                ch: 'a',
                forward: true,
                till: true,
            }),
            id,
        );

        let labels = app.find_labels.clone().expect("overlay is up");
        assert_eq!(
            labels.iter().map(|l| (l.row, l.col)).collect::<Vec<_>>(),
            vec![(0, 1), (1, 1)],
            "one landing spot per row, each one cell before the 'a'"
        );
    }

    #[test]
    fn test_find_overlay_jump_extends_a_visual_selection() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["banana"]));
        app.modes.insert(id, Mode::Visual);
        app.selection.visual_anchor = Some((0, 0));
        app.set_nav_cursor(id, (0, 0));
        app.update_visual_selection(id);

        app.handle_action(
            input::Action::FindChar(FindChar {
                ch: 'a',
                forward: true,
                till: false,
            }),
            id,
        );
        let label = app.find_labels.clone().expect("overlay is up")[2].label;
        app.handle_action(input::Action::FindJump(label), id);

        assert_eq!(app.nav_cursor(id), Some((0, 5)));
        assert_eq!(app.selected_text().as_deref(), Some("banana"));
    }

    #[test]
    fn test_find_overlay_cancel_leaves_the_cursor_alone() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["banana"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        app.handle_action(
            input::Action::FindChar(FindChar {
                ch: 'a',
                forward: true,
                till: false,
            }),
            id,
        );
        app.handle_action(input::Action::FindCancel, id);

        assert!(app.find_labels.is_none());
        assert_eq!(app.nav_cursor(id), Some((0, 0)));
    }

    #[test]
    fn test_matching_bracket_reaches_a_partner_in_scrollback() {
        // Regression: `%` searched only the visible rows, so a partner scrolled off
        // the top was unreachable. It now scans the whole buffer and scrolls.
        let pane = pane_with_history(&["f(x", "a", "b", "c", "d", ")end"]);
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane);
        assert!(
            app.panes[&id].grid().scrollback_len() > 0,
            "fixture needs the opening bracket above the viewport"
        );
        // The `)` sits on the last visible row, at column 0.
        let last = app.panes[&id].grid().last_content_row();
        app.set_nav_cursor(id, (last, 0));

        app.move_nav_cursor(input::CursorMove::MatchingBracket, id);

        let (row, col) = app.nav_cursor(id).unwrap();
        let grid = app.panes[&id].grid();
        assert!(
            grid.scroll_offset() > 0,
            "the view scrolled up to the partner"
        );
        assert_eq!(
            grid.visible_cell(row, col).map(|c| c.ch),
            Some('('),
            "the cursor sits on the opening bracket"
        );
    }

    #[test]
    fn test_word_end_back_wraps_onto_the_previous_line_in_scrollback() {
        // `ge` at the start of the top visible line steps to the previous line's
        // last word end, scrolling history in like `b` does.
        let pane = pane_with_history(&["aa", "bb", "cc", "dd", "ee"]);
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane);
        assert_eq!(app.panes[&id].grid().scrollback_len(), 1, "one line above");
        app.set_nav_cursor(id, (0, 0));

        app.move_nav_cursor(input::CursorMove::WordEndBack, id);

        let (row, col) = app.nav_cursor(id).unwrap();
        let grid = app.panes[&id].grid();
        assert_eq!(grid.scroll_offset(), 1, "scrolled one line into history");
        assert_eq!(row, 0);
        assert_eq!(
            grid.visible_cell(row, col).map(|c| c.ch),
            Some('a'),
            "landed on the end of the previous line's word"
        );
    }

    #[test]
    fn test_paragraph_motion_scrolls_past_the_viewport_edge() {
        // Regression: `{`/`}` only scanned the visible rows and clamped to the
        // screen's first/last row, so they stopped dead at the viewport edge
        // instead of scrolling into scrollback like `k`/`j` do.
        let mut pane = crate::terminal::pane::Pane::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        // Six paragraphs of "text / blank", pushed through a 4-row grid so most
        // of them end up in scrollback.
        {
            let grid = pane.grid_mut();
            for i in 0..6 {
                for ch in format!("para{i}").chars() {
                    grid.print(ch);
                }
                grid.line_feed();
                grid.move_to_column(0);
                grid.line_feed();
                grid.move_to_column(0);
            }
        }
        assert!(
            pane.grid().scrollback_len() > 0,
            "fixture needs paragraphs above the viewport"
        );

        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane);
        // Start at the top of the visible screen: everything earlier is off screen.
        app.set_nav_cursor(id, (0, 0));
        let abs_before = app.panes[&id].grid().to_absolute_row(0);

        app.move_nav_cursor(input::CursorMove::ParagraphBack, id);

        let (row, _) = app.nav_cursor(id).unwrap();
        let abs_after = app.panes[&id].grid().to_absolute_row(row);
        assert!(
            abs_after < abs_before,
            "backward paragraph should reach a boundary above the viewport \
             (was at absolute row {abs_before}, landed on {abs_after})"
        );
        assert!(
            app.panes[&id].grid().scroll_offset() > 0,
            "and scroll the view up to show it"
        );

        // Forward again returns down the buffer, past the viewport it scrolled to.
        app.move_nav_cursor(input::CursorMove::ParagraphForward, id);
        let (row_fwd, _) = app.nav_cursor(id).unwrap();
        let abs_fwd = app.panes[&id].grid().to_absolute_row(row_fwd);
        assert!(abs_fwd > abs_after, "forward paragraph moves back down");
    }

    #[test]
    fn test_paragraph_motions_stop_on_the_blank_separators() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(
            id,
            pane_with_lines(&["alpha", "beta", "", "gamma", "delta"]),
        );
        app.set_nav_cursor(id, (0, 0));

        app.move_nav_cursor(input::CursorMove::ParagraphForward, id);
        assert_eq!(
            app.nav_cursor(id).unwrap().0,
            2,
            "forward paragraph lands on the blank row"
        );

        app.set_nav_cursor(id, (4, 0));
        app.move_nav_cursor(input::CursorMove::ParagraphBack, id);
        assert_eq!(
            app.nav_cursor(id).unwrap().0,
            2,
            "backward paragraph lands on the blank row"
        );
    }

    #[test]
    fn test_screen_motions_jump_to_top_middle_and_bottom_of_the_viewport() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        // Fill every row so `L`/`M` aren't clamped by the content's last row.
        let lines: Vec<String> = (0..8).map(|i| format!("line{i}")).collect();
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        app.panes.insert(id, pane_with_lines(&refs));
        let rows = app.panes[&id].grid().rows();
        app.set_nav_cursor(id, (3, 2));

        app.move_nav_cursor(input::CursorMove::ScreenTop, id);
        assert_eq!(app.nav_cursor(id).unwrap().0, 0);
        app.move_nav_cursor(input::CursorMove::ScreenBottom, id);
        assert_eq!(app.nav_cursor(id).unwrap().0, rows - 1);
        app.move_nav_cursor(input::CursorMove::ScreenMiddle, id);
        assert_eq!(app.nav_cursor(id).unwrap().0, rows / 2);
    }

    #[test]
    fn test_matching_bracket_hops_between_partners_counting_nesting() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["f(g(x))"]));

        // From the outer `(` at col 1 to its partner at col 6, and back.
        app.set_nav_cursor(id, (0, 1));
        app.move_nav_cursor(input::CursorMove::MatchingBracket, id);
        assert_eq!(app.nav_cursor(id), Some((0, 6)));
        app.move_nav_cursor(input::CursorMove::MatchingBracket, id);
        assert_eq!(app.nav_cursor(id), Some((0, 1)));

        // Vim starts at the first bracket at or right of the cursor: from col 0
        // that's the outer `(` again.
        app.set_nav_cursor(id, (0, 0));
        app.move_nav_cursor(input::CursorMove::MatchingBracket, id);
        assert_eq!(app.nav_cursor(id), Some((0, 6)));
    }

    #[test]
    fn test_last_non_blank_and_word_end_back_motions() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let id = PaneId(1);
        app.panes.insert(id, pane_with_lines(&["foo bar baz"]));

        app.set_nav_cursor(id, (0, 0));
        app.move_nav_cursor(input::CursorMove::LastNonBlank, id);
        assert_eq!(app.nav_cursor(id), Some((0, 10)), "g_ lands on the final z");

        // `ge` from inside "baz" lands on the end of "bar".
        app.set_nav_cursor(id, (0, 9));
        app.move_nav_cursor(input::CursorMove::WordEndBack, id);
        assert_eq!(app.nav_cursor(id), Some((0, 6)));
    }

    #[test]
    fn test_line_to_top_scrolls_the_cursor_line_to_the_first_row() {
        // `zt` keeps the cursor on its buffer line and scrolls the view so that
        // line sits at the top. Started from inside history: at the live bottom
        // the view can't scroll further down, so a line near the bottom cannot be
        // lifted to the top (a terminal has no blank space past the last row).
        let mut pane = pane_with_scrollback();
        pane.grid_mut().scroll_up_history(20);
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane);
        app.set_nav_cursor(id, (5, 0));
        let abs_before = app.panes[&id].grid().to_absolute_row(5);

        app.move_nav_cursor(input::CursorMove::LineToTop, id);

        let (row, _) = app.nav_cursor(id).unwrap();
        let grid = app.panes[&id].grid();
        assert_eq!(row, 0, "the cursor's line is now the top row");
        assert_eq!(
            grid.to_absolute_row(row),
            abs_before,
            "and it is still the same buffer line"
        );
    }

    #[test]
    fn test_page_up_snaps_cursor_to_top_of_page() {
        // Editor convention: PageUp (vim's `Ctrl-F`/`Ctrl-B` analog) scrolls a
        // full page and repositions the cursor to the new page's top row,
        // rather than leaving it on whatever screen row it was on before.
        let pane = pane_with_scrollback();
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane);
        app.set_nav_cursor(id, (5, 0));

        app.move_nav_cursor(input::CursorMove::PageUp, id);

        let (row, _) = app.nav_cursor(id).expect("cursor still tracked");
        assert_eq!(
            row, 0,
            "PageUp should land the cursor on the new page's top row"
        );
    }

    #[test]
    fn test_page_down_snaps_cursor_to_bottom_of_page() {
        // Same convention in the other direction: paging down through
        // history (not yet back at the live bottom) should land the cursor
        // on the new page's bottom row, not wherever it happened to be.
        let pane = pane_with_scrollback();
        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane);

        app.move_nav_cursor(input::CursorMove::Top, id);
        app.move_nav_cursor(input::CursorMove::PageDown, id);

        let rows = app.panes[&id].grid().rows();
        let (row, _) = app.nav_cursor(id).expect("cursor still tracked");
        assert_eq!(
            row,
            rows - 1,
            "PageDown should land the cursor on the new page's bottom row"
        );
    }

    #[test]
    fn test_page_down_from_history_does_not_strand_cursor_past_last_prompt() {
        // Regression: PageDown/PageUp only changed the grid's scroll offset
        // and left `row` untouched, so returning from scrolled-back history
        // to a pane holding just a few lines of content could leave the
        // traversal cursor stranded below the last printed row. `Down`'s own
        // clamp only stops further increments, so it could never pull an
        // already-too-large row back up.
        let mut pane = crate::terminal::pane::Pane::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("bash"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        pane.write(b"echo hi\n");
        std::thread::sleep(std::time::Duration::from_millis(100));
        pane.drain_output();
        let last_row = pane.grid().last_content_row();
        let bottom_row = pane.grid().rows() - 1;
        assert!(
            bottom_row > last_row,
            "fixture needs blank rows below the last prompt to reproduce the bug"
        );

        let mut app = App::new();
        let id = PaneId(1);
        app.panes.insert(id, pane);
        // Simulate a cursor stranded past the content, as returning from a
        // scrolled-back page would leave it.
        app.set_nav_cursor(id, (bottom_row, 0));

        app.move_nav_cursor(input::CursorMove::PageDown, id);

        let (row, _) = app.nav_cursor(id).expect("cursor still tracked");
        assert!(
            row <= last_row,
            "cursor at row {row} sits past the last prompt line (row {last_row})"
        );
    }

    #[test]
    fn test_mouse_tracking_parks_nav_cursor_in_normal_and_visual_modes() {
        // A click (or drag) parks the traversal cursor on the pointer's cell so
        // the block cursor follows the mouse while selecting.
        let (mut app, id) = app_with_paragraphs(0);

        app.track_nav_cursor_to_mouse(id, 2, 4);
        assert_eq!(app.nav_cursor(id), Some((2, 4)));

        app.modes.insert(id, Mode::Visual);
        app.track_nav_cursor_to_mouse(id, 3, 1);
        assert_eq!(app.nav_cursor(id), Some((3, 1)));
    }

    #[test]
    fn test_mouse_tracking_leaves_nav_cursor_alone_in_insert_mode() {
        // Insert mode hides the traversal cursor and re-derives it from the
        // shell cursor on entering Normal, so a click must not overwrite the
        // stored position.
        let (mut app, id) = app_with_paragraphs(1);
        app.modes.insert(id, Mode::Insert);

        app.track_nav_cursor_to_mouse(id, 2, 4);

        assert_eq!(
            app.nav_cursor(id),
            Some((1, 0)),
            "Insert-mode click must not move the traversal cursor"
        );
    }

    #[test]
    fn test_count_moves_the_cursor_multiple_rows() {
        // `3j` lands three rows down in one stroke; the count is spent on the
        // motion (a count that resolved to Ignore, or moved once, fails this).
        let (mut app, id) = app_with_paragraphs(0);

        app.handle_action(
            input::Action::MoveCursorN {
                count: 3,
                mv: input::CursorMove::Down,
            },
            id,
        );

        assert_eq!(
            app.nav_cursor(id),
            Some((3, 0)),
            "3j from row 0 lands on row 3"
        );
    }

    #[test]
    fn test_changelist_appends_instead_of_forking() {
        // Unlike the jumplist (whose new jump from the past discards the
        // abandoned future), a changelist is a log: a change made while
        // stepped back becomes the newest entry while the older ones stay
        // reachable behind it.
        let mut changes = ChangeList::default();
        changes.push((1, 0));
        changes.push((2, 0));
        assert_eq!(changes.older((9, 9)), Some((2, 0)));
        changes.push((3, 0));
        assert_eq!(
            changes.older((9, 9)),
            Some((3, 0)),
            "a change made while stepped back becomes the newest entry"
        );
        assert_eq!(
            changes.older((9, 9)),
            Some((2, 0)),
            "older entries survive the push — the changelist is a log, not a fork"
        );
    }

    #[test]
    fn test_changelist_walk_returns_to_the_live_position_last() {
        let mut changes = ChangeList::default();
        changes.push((1, 0));
        changes.push((2, 0));
        assert_eq!(changes.older((9, 9)), Some((2, 0)));
        assert_eq!(changes.older((9, 9)), Some((1, 0)));
        assert_eq!(changes.newer(), Some((2, 0)));
        assert_eq!(
            changes.newer(),
            Some((9, 9)),
            "g, walks all the way back to where it started"
        );
        assert_eq!(
            changes.newer(),
            None,
            "at the live position, g, has nowhere to go"
        );
    }

    #[test]
    fn test_g_semicolon_returns_to_where_a_change_began() {
        // An edit records where it began; `g;` lands the cursor back there
        // after it moved on, and `g,` walks forward to the live spot again.
        let (mut app, id) = app_with_paragraphs(0);
        let (prompt_row, prompt_col) = app.panes[&id].grid().cursor();
        app.set_nav_cursor(id, (prompt_row, prompt_col));

        app.handle_action(input::Action::DeleteCharForward, id);

        app.handle_action(input::Action::MoveCursor(input::CursorMove::Down), id);
        app.handle_action(input::Action::ChangeOlder, id);
        assert_eq!(
            app.nav_cursor(id),
            Some((prompt_row, prompt_col)),
            "g; lands where the delete began"
        );
        app.handle_action(input::Action::ChangeNewer, id);
        assert_eq!(
            app.nav_cursor(id),
            Some((prompt_row + 1, 0)),
            "g, returns to the position the walk started from"
        );
    }

    #[test]
    fn test_dot_redispatches_the_last_normal_mode_change() {
        // `x` then `.`: the repeat re-runs the delete at the cursor, which a
        // no-op `.` (no recorded change, or a stale one) would leave behind.
        let (mut app, id) = app_with_paragraphs(0);
        let (prompt_row, prompt_col) = app.panes[&id].grid().cursor();
        app.set_nav_cursor(id, (prompt_row, prompt_col));

        app.handle_action(input::Action::DeleteCharForward, id);
        assert!(matches!(
            app.vim.last_changes.get(&id),
            Some(LastChange::Action(input::Action::DeleteCharForward))
        ));

        // `delete_on_prompt` flags the nav cursor for resync on success; reset
        // and re-check it to prove the repeat really re-dispatched the delete.
        app.nav_resync_pending = false;
        app.handle_action(input::Action::RepeatLastChange, id);
        assert!(
            app.nav_resync_pending,
            "`.` must re-run the recorded delete, not just remember it"
        );
    }

    #[test]
    fn test_dot_replays_the_last_insert_typing_run() {
        // Typing "hi" in Insert and leaving for Normal records the run as the
        // pane's last change; `.` re-sends it, so the echo pane shows it twice.
        // A blank fixture row so the echo is the only text to assert on.
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes.insert(id, pane_with_lines(&[""]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        app.handle_action(input::Action::EnterInsert(input::InsertAt::Cursor), id);
        app.handle_action(input::Action::SendBytes(b"hi".to_vec()), id);
        // The key handler accumulates forwarded bytes into the session run.
        app.vim
            .insert_sessions
            .entry(id)
            .or_default()
            .run
            .extend_from_slice(b"hi");
        app.handle_action(input::Action::SwitchMode(Mode::Normal), id);
        assert_eq!(
            app.vim.last_changes.get(&id),
            Some(&LastChange::Typed(b"hi".to_vec()))
        );

        app.handle_action(input::Action::RepeatLastChange, id);
        // `cat` echoes both writes back; pump until both arrive.
        for _ in 0..50 {
            if app.panes.get_mut(&id).unwrap().drain_output() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let _ = app.panes.get_mut(&id).unwrap().drain_output();
                if row_text(&app, id, 0) == "hihi" {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            row_text(&app, id, 0),
            "hihi",
            "`.` must re-send the recorded typing run"
        );
    }

    #[test]
    fn test_dot_without_a_recorded_change_is_a_no_op() {
        let (mut app, id) = app_with_paragraphs(0);
        let before = app.nav_cursor(id);

        app.handle_action(input::Action::RepeatLastChange, id);

        assert_eq!(app.nav_cursor(id), before);
        assert!(!app.vim.last_changes.contains_key(&id));
    }

    #[test]
    fn test_dot_session_through_the_real_resolver_and_key_path() {
        // Drives the same pipeline the keyboard handler runs — `resolve_with`
        // for each key, `record_insert_key` for forwarded bytes, `handle_action`
        // for dispatch — so the Insert session opens, accumulates, and closes
        // through production code, not hand-built actions. Entry chord
        // (Ctrl+Shift+Space) leaves Insert deterministically, without the
        // bare-Escape foreground-process branch.
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes.insert(id, pane_with_lines(&[""]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));
        let keymap = input::WindowKeymap::default();
        let mut pending = input::PendingPrefix::None;
        let mut dispatch = |app: &mut App, key: input::Key| {
            let mode = app.modes.get(&id).copied().unwrap_or_default();
            let action = input::resolve_with(mode, &key, &mut pending, &keymap, 0, None, false);
            if mode == Mode::Insert && crate::app::forwarded_to_pty(&action) {
                let at_prompt = app.panes.get(&id).is_some_and(|p| p.is_at_prompt());
                app.record_insert_key(id, &key, &action, at_prompt);
            }
            app.handle_action(action, id);
        };

        dispatch(
            &mut app,
            input::Key {
                alt: false,
                code: input::KeyCode::Char('i'),
                ctrl: false,
                shift: false,
            },
        );
        for ch in ['h', 'i'] {
            dispatch(
                &mut app,
                input::Key {
                    alt: false,
                    code: input::KeyCode::Char(ch),
                    ctrl: false,
                    shift: false,
                },
            );
        }
        dispatch(
            &mut app,
            input::Key {
                alt: false,
                code: input::KeyCode::Space,
                ctrl: true,
                shift: true,
            },
        );

        assert_eq!(
            app.modes.get(&id),
            Some(&Mode::Normal),
            "entry chord left Insert"
        );
        assert_eq!(
            app.vim.last_changes.get(&id),
            Some(&LastChange::Typed(b"hi".to_vec())),
            "the real key path accumulated the typed run and finalized it on leaving Insert"
        );

        app.handle_action(input::Action::RepeatLastChange, id);
        for _ in 0..50 {
            if app.panes.get_mut(&id).unwrap().drain_output() {
                std::thread::sleep(std::time::Duration::from_millis(10));
                let _ = app.panes.get_mut(&id).unwrap().drain_output();
                if row_text(&app, id, 0) == "hihi" {
                    break;
                }
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(
            row_text(&app, id, 0),
            "hihi",
            "`.` re-sends the run recorded through the real path"
        );
    }

    #[test]
    fn test_jumplist_returns_to_the_position_before_the_jump() {
        // `gg` records its origin, so `Ctrl+O` comes back to it and `Ctrl+I`
        // walks forward again — in absolute terms, so the round trip holds
        // even once the view has scrolled.
        let (mut app, id) = app_with_paragraphs(2);
        let abs_before = app.panes[&id].grid().to_absolute_row(2);

        app.handle_action(input::Action::MoveCursor(input::CursorMove::Top), id);
        assert_eq!(
            app.nav_cursor(id),
            Some((0, 0)),
            "gg lands on the oldest row"
        );

        app.handle_action(input::Action::JumpOlder, id);
        let (row, _) = app.nav_cursor(id).expect("cursor restored");
        assert_eq!(
            app.panes[&id].grid().to_absolute_row(row),
            abs_before,
            "Ctrl+O returns to the pre-jump line"
        );

        app.handle_action(input::Action::JumpNewer, id);
        assert_eq!(
            app.nav_cursor(id),
            Some((0, 0)),
            "Ctrl+I walks forward to the post-jump position"
        );
    }

    #[test]
    fn test_jumplist_forks_when_a_new_jump_leaves_the_past() {
        // Stepping back then jumping again discards the abandoned entries, so
        // the next Ctrl+O lands on the new jump — not on a forked-away future.
        let mut jumps = JumpList::default();
        jumps.push((1, 0));
        jumps.push((2, 0));
        assert_eq!(jumps.older((9, 9)), Some((2, 0)));
        jumps.push((3, 0));
        assert_eq!(
            jumps.older((8, 8)),
            Some((3, 0)),
            "the entry stepped back past is forked away, not kept"
        );
        assert_eq!(
            jumps.newer(),
            Some((8, 8)),
            "walking all the way forward returns to the live position"
        );
        assert_eq!(jumps.newer(), None, "already at the live position");
    }

    #[test]
    fn test_leaving_normal_records_a_jump_origin() {
        // Pressing `i` from a browsed position is itself a jump in vim's
        // jumplist: after the next Escape (which re-derives the cursor at the
        // prompt), Ctrl+O returns to where the browsing left off.
        let (mut app, id) = app_with_paragraphs(2);
        let abs_before = app.panes[&id].grid().to_absolute_row(2);

        app.handle_action(input::Action::SwitchMode(Mode::Insert), id);
        // The next Escape re-enters Normal and re-derives the cursor at the
        // prompt — the state Ctrl+O is actually pressed from.
        app.handle_action(input::Action::SwitchMode(Mode::Normal), id);

        app.handle_action(input::Action::JumpOlder, id);
        let (row, _) = app.nav_cursor(id).expect("cursor restored");
        assert_eq!(
            app.panes[&id].grid().to_absolute_row(row),
            abs_before,
            "the pre-Insert browsing position is on the jumplist"
        );
    }

    #[test]
    fn test_visual_o_swaps_the_cursor_to_the_anchor() {
        // The cursor block moves to the selection's other end while the
        // highlighted span is unchanged — it always runs anchor..cursor in
        // either order.
        let (mut app, id) = app_with_paragraphs(2);
        app.modes.insert(id, Mode::Visual);
        app.selection.visual_anchor = Some((0, 2));
        app.set_nav_cursor(id, (2, 1));
        app.update_visual_selection(id);
        // `update_visual_selection` stores anchor->cursor order, and the swap
        // flips which end is which — compare the covered span (normalized, the
        // same way the renderer resolves it) instead of the raw field order.
        let span_before = app
            .selection
            .span
            .as_ref()
            .map(|s| (s.start_row.min(s.end_row), s.start_row.max(s.end_row)));

        app.handle_action(input::Action::SwapVisualEnds, id);

        assert_eq!(
            app.nav_cursor(id),
            Some((0, 2)),
            "cursor sits on the anchor"
        );
        assert_eq!(
            app.selection.visual_anchor,
            Some((2, 1)),
            "anchor moved to the old cursor"
        );
        assert_eq!(
            app.selection
                .span
                .as_ref()
                .map(|s| (s.start_row.min(s.end_row), s.start_row.max(s.end_row))),
            span_before,
            "the selection span itself is unchanged"
        );
    }

    #[test]
    fn test_gv_restores_the_last_visual_selection() {
        // Every exit from Visual snapshots the selection; `gv` brings back the
        // same span, the same kind (linewise here), and the same cursor end.
        let (mut app, id) = app_with_paragraphs(0);
        app.handle_action(input::Action::EnterVisual(input::VisualKind::Line), id);
        app.handle_action(
            input::Action::MoveCursorN {
                count: 2,
                mv: input::CursorMove::Down,
            },
            id,
        );
        let span = app
            .selection
            .span
            .as_ref()
            .map(|s| (s.start_row, s.end_row))
            .expect("selection live");

        // Leaving Visual (same `V` again) clears it; `gv` restores it.
        app.handle_action(input::Action::EnterVisual(input::VisualKind::Line), id);
        assert!(
            app.selection.span.is_none(),
            "exiting Visual cleared the selection"
        );

        app.handle_action(input::Action::RestoreVisual, id);

        assert_eq!(
            app.modes.get(&id),
            Some(&Mode::Visual),
            "gv re-enters Visual"
        );
        assert_eq!(
            app.selection.visual_kind,
            VisualKind::Line,
            "the linewise kind is preserved"
        );
        assert_eq!(
            app.selection
                .span
                .as_ref()
                .map(|s| (s.start_row, s.end_row)),
            Some(span),
            "the same span is selected again"
        );
    }

    #[test]
    fn test_mark_set_and_goto_exact_and_first_non_blank() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes
            .insert(id, pane_with_lines(&["alpha", "   beta", "gamma"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (1, 4));

        // Set mark 'a' at row 1, col 4
        app.handle_action(input::Action::SetMark('a'), id);

        // Move elsewhere
        app.set_nav_cursor(id, (0, 0));

        // Goto exact (`) lands on col 4
        app.handle_action(input::Action::GotoMark(input::GotoMark::new('a', true)), id);
        assert_eq!(app.nav_cursor(id), Some((1, 4)));

        // Move elsewhere
        app.set_nav_cursor(id, (0, 0));

        // Goto first non-blank (') lands on col 3 (start of "beta")
        app.handle_action(
            input::Action::GotoMark(input::GotoMark::new('a', false)),
            id,
        );
        assert_eq!(app.nav_cursor(id), Some((1, 3)));
    }

    #[test]
    fn test_goto_mark_records_jump_origin_for_ctrl_o() {
        let (mut app, id) = app_with_paragraphs(0);
        app.set_nav_cursor(id, (3, 0));
        app.handle_action(input::Action::SetMark('z'), id);

        app.set_nav_cursor(id, (0, 2));

        // Jumping to mark 'z' records the origin (0, 2)
        app.handle_action(input::Action::GotoMark(input::GotoMark::new('z', true)), id);
        assert_eq!(app.nav_cursor(id), Some((3, 0)));

        // Ctrl+O returns to (0, 2)
        app.handle_action(input::Action::JumpOlder, id);
        assert_eq!(app.nav_cursor(id), Some((0, 2)));
    }

    #[test]
    fn test_goto_mark_in_visual_mode_extends_selection() {
        let (mut app, id) = app_with_paragraphs(0);
        app.set_nav_cursor(id, (3, 0));
        app.handle_action(input::Action::SetMark('b'), id);

        app.set_nav_cursor(id, (0, 0));
        app.handle_action(input::Action::EnterVisual(input::VisualKind::Char), id);

        app.handle_action(input::Action::GotoMark(input::GotoMark::new('b', true)), id);
        assert_eq!(app.nav_cursor(id), Some((3, 0)));
        assert!(
            app.selection.span.is_some(),
            "selection extends to the mark"
        );
        let sel = app.selection.span.as_ref().unwrap();
        assert_eq!((sel.start_row, sel.end_row), (0, 3));
    }

    #[test]
    fn test_select_text_object_word_in_normal_enters_visual() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes.insert(id, pane_with_lines(&["hello world foo"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 7)); // on 'o' in 'world'

        app.handle_action(
            input::Action::SelectTextObject(input::TextObjectSpec::new(
                false,
                input::TextObject::Word,
            )),
            id,
        );

        assert_eq!(app.modes.get(&id), Some(&Mode::Visual));
        assert_eq!(app.selection.visual_kind, VisualKind::Char);
        assert_eq!(app.selected_text().as_deref(), Some("world"));
    }

    #[test]
    fn test_select_text_object_quotes() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes
            .insert(id, pane_with_lines(&["let s = \"hello world\";"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 11)); // inside quotes

        // Inner quotes: `i"`
        app.handle_action(
            input::Action::SelectTextObject(input::TextObjectSpec::new(
                false,
                input::TextObject::Quotes('"'),
            )),
            id,
        );
        assert_eq!(app.selected_text().as_deref(), Some("hello world"));

        // Around quotes: `a"`
        app.handle_action(
            input::Action::SelectTextObject(input::TextObjectSpec::new(
                true,
                input::TextObject::Quotes('"'),
            )),
            id,
        );
        assert_eq!(app.selected_text().as_deref(), Some("\"hello world\""));
    }

    #[test]
    fn test_blockwise_visual_selection_and_yank() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes
            .insert(id, pane_with_lines(&["abcdef", "123456", "ghijkl"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 1)); // 'b'

        // Enter blockwise visual
        app.handle_action(input::Action::EnterVisual(input::VisualKind::Block), id);
        assert_eq!(app.modes.get(&id), Some(&Mode::Visual));
        assert_eq!(app.selection.visual_kind, VisualKind::Block);

        // Move down 2 rows, right 2 cols (cols 1..=3 on rows 0..=2)
        app.set_nav_cursor(id, (2, 3));
        app.update_visual_selection(id);

        let sel = app.selection.span.as_ref().expect("block selection active");
        assert!(sel.block, "selection is marked block");
        assert_eq!(app.selected_text().as_deref(), Some("bcd\n234\nhij"));
    }

    #[test]
    fn test_named_register_yank_and_paste_in_app() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes
            .insert(id, pane_with_lines(&["echo \"hello\"", ""]));
        app.modes.insert(id, Mode::Visual);
        app.selection.visual_anchor = Some((0, 5));
        app.set_nav_cursor(id, (0, 11));
        app.update_visual_selection(id);

        // Yank into register 'a'
        app.handle_action(input::Action::YankSelectionRegister('a'), id);
        assert_eq!(
            app.vim.registers.get(&'a').map(String::as_str),
            Some("\"hello\"")
        );
        assert_eq!(app.modes.get(&id), Some(&Mode::Normal));
    }

    #[test]
    fn test_buffer_swoop_opens_navigates_and_confirms_with_jump_history() {
        use winit::keyboard::{Key, NamedKey, PhysicalKey};

        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes.insert(
            id,
            pane_with_lines(&["alpha line", "beta target", "gamma last"]),
        );
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        // Open Swoop
        app.open_swoop(id);
        let palette = app.palette.as_ref().expect("palette is active");
        assert_eq!(palette.mode, crate::model::palette::PaletteMode::Swoop);
        assert_eq!(palette.entries.len(), 3);

        // Press Down Arrow to select "beta target" (row 1)
        let key_down = Key::Named(NamedKey::ArrowDown);
        let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        let mut pal = app.palette.take().unwrap();
        app.handle_palette_input(&mut pal, &key_down, &phys, id);
        app.palette = Some(pal);

        // Check live preview jumped nav cursor to row 1
        assert_eq!(app.nav_cursor(id), Some((1, 0)));

        // Press Enter to confirm
        let key_enter = Key::Named(NamedKey::Enter);
        let mut pal = app.palette.take().unwrap();
        app.handle_palette_input(&mut pal, &key_enter, &phys, id);
        // Palette closed
        assert!(app.palette.is_none());
        assert_eq!(app.nav_cursor(id), Some((1, 0)));

        // Jump list has origin (0, 0)
        let jl = app.vim.jump_lists.get_mut(&id).expect("jumplist entry");
        assert_eq!(jl.older((1, 0)), Some((0, 0)));
    }

    #[test]
    fn test_buffer_swoop_cancel_restores_initial_cursor() {
        use winit::keyboard::{Key, NamedKey, PhysicalKey};

        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes.insert(
            id,
            pane_with_lines(&["alpha line", "beta target", "gamma last"]),
        );
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 2));

        // Open Swoop
        app.open_swoop(id);

        // Press Down Arrow
        let key_down = Key::Named(NamedKey::ArrowDown);
        let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        let mut pal = app.palette.take().unwrap();
        app.handle_palette_input(&mut pal, &key_down, &phys, id);
        app.palette = Some(pal);

        // Preview is at row 1
        assert_eq!(app.nav_cursor(id), Some((1, 0)));

        // Press Escape to cancel
        let key_esc = Key::Named(NamedKey::Escape);
        let mut pal = app.palette.take().unwrap();
        app.handle_palette_input(&mut pal, &key_esc, &phys, id);

        // Palette closed and cursor restored to (0, 2)
        assert!(app.palette.is_none());
        assert_eq!(app.nav_cursor(id), Some((0, 2)));
    }

    #[test]
    fn test_change_operator_deletes_and_enters_insert() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes
            .insert(id, pane_with_lines(&["echo \"hello world\""]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 6)); // on 'h'

        // `ci"` (Change inside quotes)
        app.handle_action(
            input::Action::ChangeTextObject(input::TextObjectSpec::new(
                false,
                input::TextObject::Quotes('"'),
            )),
            id,
        );

        // Pane should transition to Mode::Insert
        assert_eq!(app.modes.get(&id), Some(&Mode::Insert));

        // Return to Normal mode and test `s` (substitute)
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 2));
        app.handle_action(input::Action::SubstituteChar, id);
        assert_eq!(app.modes.get(&id), Some(&Mode::Insert));

        // Return to Normal mode and test `S` (change whole line)
        app.modes.insert(id, Mode::Normal);
        app.handle_action(input::Action::ChangeLine, id);
        assert_eq!(app.modes.get(&id), Some(&Mode::Insert));
    }

    #[test]
    fn test_replace_char_and_toggle_case_on_prompt() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        app.panes.insert(id, pane_with_lines(&["Hello World"]));
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0)); // 'H'

        // `rX` (Replace char with 'X')
        app.handle_action(input::Action::ReplaceChar('X'), id);
        // Stays in Mode::Normal
        assert_eq!(app.modes.get(&id), Some(&Mode::Normal));

        // `~` (Toggle case)
        app.handle_action(input::Action::ToggleCaseChar, id);
        // Cursor moved to column 1 and stays in Normal mode
        assert_eq!(app.nav_cursor(id), Some((0, 1)));
        assert_eq!(app.modes.get(&id), Some(&Mode::Normal));
    }

    #[test]
    fn test_jump_to_prompt_gp() {
        let mut app = App::new();
        let id = app.tab().panes()[0];
        let mut pane = pane_with_lines(&["first line", "second line", "prompt line"]);
        pane.grid_mut().move_to_row(2);
        pane.grid_mut().move_to_column(5);
        app.panes.insert(id, pane);
        app.modes.insert(id, Mode::Normal);
        app.set_nav_cursor(id, (0, 0));

        // Jump to prompt with `gp`
        app.handle_action(input::Action::JumpToPrompt, id);
        assert_eq!(app.nav_cursor(id), Some((2, 5)));
    }
}

//! Where the cursor has been and what it changed: the jump list and change
//! list walked back and forth, marks set and gone to, and `.` repeating the
//! last change.

use crate::model::input;
use crate::model::layout::TabId;
use crate::model::mode::Mode;

use crate::window::App;

use super::vim;

// ========================================================================
// Data Structures
// ========================================================================

/// One replayable unit for `.`: the pane's most recent change.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum LastChange {
    /// A Normal-mode edit or put, replayed by re-dispatching the action so it
    /// recomputes its alignment and extent from the cursor's current position.
    Action(input::Action),
    /// An Insert-mode typing run, replayed by re-sending the exact bytes that
    /// were originally forwarded to the shell.
    Typed(Vec<u8>),
}

/// The Insert-mode visit in progress: the raw bytes forwarded so far (the
/// `.`-repeat run, accumulated by the key handler) and the absolute position
/// the session began at: the changelist entry a non-empty run pushes when
/// the pane leaves Insert. `anchor` is `None` for a pane that never entered
/// Insert through an `i`/`a`/`o` action (it started there).
#[derive(Default)]
pub(in crate::window) struct InsertSession {
    pub(in crate::window) anchor: Option<(usize, usize)>,
    pub(in crate::window) run: Vec<u8>,
}

// ========================================================================
// Functions
// ========================================================================

/// The Normal-mode dispatches that count as a vim "change" for `.` and the
/// changelist: every prompt-line edit and every put. Insert-mode typing is
/// tracked as a session instead (see [`App::track_change`]): its `Edit`
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

// ========================================================================
// App: jumps, changes, marks, and repeat
// ========================================================================

impl App {
    /// Record the cursor's current position (absolute) as the origin of a
    /// jump, so `C-o` can come back: called before jump-class motions
    /// land, before confirmed search steps, and when Normal mode ends.
    pub(in crate::window) fn push_jump(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let Some((row, col)) = self.nav_cursor(focused) else {
            return;
        };
        let origin = (terminal.grid().to_absolute_row(row), col);
        self.vim.tab_mut(focused).jump_list.push(origin);
        // Where the jump started is also Vim's own `` ` ``/`'` mark, which is
        // how you get back without walking the whole jumplist.
        self.set_auto_mark(focused, '\'', origin);
    }

    /// Write one of the marks Winter keeps without being asked, the ones Vim
    /// names with punctuation rather than a letter. They live in the same
    /// table `m` writes to, so a jump to one is the jump to any other; only
    /// their names keep them apart from the letters a reader owns.
    pub(in crate::window) fn set_auto_mark(
        &mut self,
        focused: TabId,
        name: char,
        pos: (usize, usize),
    ) {
        self.vim.tab_mut(focused).marks.insert(name, pos);
    }

    /// `C-o`: step the cursor back to the previous recorded jump origin,
    /// revealing it if it scrolled off screen. Extends the Visual selection
    /// when the pane is in Visual mode, like any other cursor move.
    pub(in crate::window) fn jump_older(&mut self, focused: TabId) {
        let live = self.live_jump_position(focused);
        let target = live.and_then(|live| self.vim.tab_mut(focused).jump_list.older(live));
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// `C-i`/`Tab`: step the cursor forward through the jumplist, back
    /// toward the position the walk started from.
    pub(in crate::window) fn jump_newer(&mut self, focused: TabId) {
        let target = self.vim.tab_mut(focused).jump_list.newer();
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// `g;`: step the cursor back to where the previous recorded change
    /// began, revealing it if it scrolled off screen. Extends the Visual
    /// selection when the pane is in Visual mode, like any other cursor move.
    pub(in crate::window) fn change_older(&mut self, focused: TabId) {
        let live = self.live_jump_position(focused);
        let target = live.and_then(|live| self.vim.tab_mut(focused).change_list.older(live));
        if let Some(target) = target {
            self.jump_to(focused, target);
        }
    }

    /// `g,`: step the cursor forward through the changelist, back toward the
    /// position the walk started from.
    pub(in crate::window) fn change_newer(&mut self, focused: TabId) {
        let target = self.vim.tab_mut(focused).change_list.newer();
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
    pub(in crate::window) fn track_change(&mut self, action: &input::Action, focused: TabId) {
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
                self.vim.tab_mut(focused).insert_session = Some(InsertSession {
                    anchor,
                    run: Vec::new(),
                });
            }
            input::Action::SwitchMode(new_mode) => {
                let was_insert = self.open_tabs.mode(&focused) == Some(Mode::Insert);
                if was_insert && *new_mode != Mode::Insert {
                    self.finish_insert_session(focused);
                }
            }
            _ if is_change_action(action)
                && self
                    .open_tabs
                    .terminal(&focused)
                    .is_some_and(|p| p.is_at_prompt()) =>
            {
                self.vim.tab_mut(focused).last_change = Some(LastChange::Action(action.clone()));
                if let Some(pos) = self.live_jump_position(focused) {
                    self.vim.tab_mut(focused).change_list.push(pos);
                    self.set_auto_mark(focused, '.', pos);
                }
            }
            _ => {}
        }
    }

    /// Close the terminal's Insert-mode typing session: a non-empty run becomes
    /// the pane's last change (replayed by `.`), and the session's anchor
    /// joins the changelist so `g;` returns to where the typing began.
    fn finish_insert_session(&mut self, focused: TabId) {
        let Some(session) = self.vim.tab_mut(focused).insert_session.take() else {
            return;
        };
        if session.run.is_empty() {
            return;
        }
        if let Some(anchor) = session.anchor {
            self.vim.tab_mut(focused).change_list.push(anchor);
            // Where the typing began is both the last change and the place
            // Insert was left, which Vim marks `.` and `^`.
            self.set_auto_mark(focused, '.', anchor);
            self.set_auto_mark(focused, '^', anchor);
        }
        self.vim.tab_mut(focused).last_change = Some(LastChange::Typed(session.run));
    }

    /// `.`: replay the pane's most recent change where the cursor sits now.
    /// A Normal-mode edit re-dispatches its action (recomputing its effect
    /// from the current position); an Insert-mode typing run re-sends its
    /// bytes verbatim: straight to the PTY, so the prompt shadow is desynced
    /// to keep later undo honest about a line it no longer models.
    pub(in crate::window) fn repeat_last_change(&mut self, focused: TabId) {
        let Some(change) = self
            .vim
            .tab(focused)
            .and_then(|vim| vim.last_change.clone())
        else {
            return;
        };
        match change {
            LastChange::Action(action) => self.handle_action(action, focused),
            LastChange::Typed(run) => {
                if self
                    .open_tabs
                    .terminal(&focused)
                    .is_some_and(|p| p.is_at_prompt())
                {
                    if let Some(pos) = self.live_jump_position(focused) {
                        self.vim.tab_mut(focused).change_list.push(pos);
                    }
                    if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
                        shadow.desync();
                    }
                }
                self.handle_action(input::Action::SendBytes(run), focused);
            }
        }
    }

    /// `m{a-z}`: record the current absolute cursor position in a named mark.
    pub(in crate::window) fn set_mark(&mut self, focused: TabId, mark: char) {
        if !mark.is_ascii_lowercase() {
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let Some((row, col)) = self.nav_cursor(focused) else {
            return;
        };
        let abs_row = terminal.grid().to_absolute_row(row);
        self.vim.tab_mut(focused).marks.insert(mark, (abs_row, col));
    }

    /// `` `{a-z} `` / `'{a-z}`: jump to a named mark recorded by [`Self::set_mark`].
    /// `exact` lands on the exact column; `false` lands on the line's first non-blank.
    /// Records the jump origin beforehand so `C-o` returns to the pre-jump location.
    pub(in crate::window) fn goto_mark(&mut self, focused: TabId, mark: char, exact: bool) {
        let Some(&(abs_row, col)) = self.vim.tab(focused).and_then(|vim| vim.marks.get(&mark))
        else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
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
    fn live_jump_position(&self, focused: TabId) -> Option<(usize, usize)> {
        let terminal = self.open_tabs.terminal(&focused)?;
        let (row, col) = self.nav_cursor(focused)?;
        Some((terminal.grid().to_absolute_row(row), col))
    }

    /// Land the cursor on an absolute position: scroll it into view (centering
    /// when off screen), park the nav cursor on it, and extend the Visual
    /// selection when the pane is in Visual mode.
    pub(super) fn jump_to(&mut self, focused: TabId, (abs_row, col): (usize, usize)) {
        self.reveal_position(focused, (abs_row, col));
        if self.open_tabs.mode(&focused) == Some(Mode::Visual) {
            self.update_visual_selection(focused);
        }
        self.dirty = true;
    }
}

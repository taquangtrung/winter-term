//! Vim edits applied to the shell's prompt: the line editor's cursor brought
//! to the Vim cursor, deletes, changes, replaces, and case toggles sent as the
//! keystrokes the shell understands, and declined where they cannot land.

use crate::config::PromptEditBindings;
use crate::model::input::{CursorMove, EditAction};
use crate::model::layout::TabId;
use crate::model::mode::Mode;

use crate::terminal::session::Terminal;
use crate::window::terminal::prompt_edit::{
    self, edit_action_bytes, prompt_delete_bytes, rebuild_line_bytes, PromptDelete, READLINE_UNDO,
};
use crate::window::App;

// ========================================================================
// App: editing the prompt
// ========================================================================

impl App {
    /// Bring the prompt back into view and walk the shell's cursor over to the
    /// nav cursor's column, so typing resumes where the user was looking.
    ///
    /// Every route into Insert (`i`/`a`/`o`, Visual's `i`, the menu/palette, a
    /// click) lands here, so they all scroll and align alike.
    pub(in crate::window) fn align_prompt_to_nav_cursor(&mut self, focused: TabId) {
        // Typing goes to the prompt, so a pane scrolled up into history snaps
        // back to the live bottom first.
        let was_scrolled = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|terminal| terminal.grid().scroll_offset() > 0);
        if was_scrolled {
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.grid_mut().set_scroll_offset(0);
            }
        }
        // Aligning the shell cursor to the nav cursor's column only makes sense
        // when that cursor addressed the live screen; from scrolled-back history
        // it named a scrollback row, not a column of the prompt.
        let aligned = (!was_scrolled)
            .then(|| {
                self.nav_cursor(focused).and_then(|(nav_row, nav_col)| {
                    let terminal = self.open_tabs.terminal(&focused)?;
                    let (prompt_row, pty_col) = terminal.grid().cursor();
                    (nav_row == prompt_row).then_some((nav_col, pty_col))
                })
            })
            .flatten();
        let Some((nav_col, pty_col)) = aligned else {
            return;
        };
        // Keep the shadow cursor aligned with the shell cursor so
        // post-navigation inserts model correctly.
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.sync_cursor(nav_col, pty_col);
        }
        let (seq, steps) = if nav_col >= pty_col {
            (b"\x1b[C" as &[u8], nav_col - pty_col)
        } else {
            (b"\x1b[D" as &[u8], pty_col - nav_col)
        };
        let mut bytes = Vec::new();
        for _ in 0..steps {
            bytes.extend_from_slice(seq);
        }
        if !bytes.is_empty() {
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.write(&bytes);
            }
        }
    }

    /// Whether Vim operators may edit the shell's prompt line right now.
    ///
    /// False when the user has told us their shell is not in emacs mode: the
    /// operators are realized as readline chords (`C-a`, `C-k`, ...), and
    /// a shell in vi mode has those bound to something else entirely, so
    /// "delete a word" would run whatever `C-w` means there instead. A shell
    /// in vi mode already provides its own Vim line editing, so declining here
    /// leaves the user with working editing rather than none.
    pub(in crate::window) fn prompt_editing_enabled(&self) -> bool {
        self.config.prompt_edit_bindings == PromptEditBindings::Emacs
    }

    /// Report that a prompt operator was declined because prompt editing is
    /// off, so the keypress does not just vanish.
    pub(in crate::window) fn decline_prompt_edit(&mut self) {
        self.set_error("Prompt editing is off (prompt-edit-bindings); your shell's own editor handles the line");
    }

    /// Whether any of `targets`, the grid columns a Normal-mode prompt edit
    /// would touch, falls inside the shell's own prompt decoration - left of
    /// where OSC 133's `B` mark said the command line begins. Such an edit
    /// cannot be translated: readline's buffer starts at the command text, so
    /// the arrow-key alignment would overshoot it and the following Delete
    /// keypresses would eat characters the operator never targeted.
    pub(in crate::window) fn prompt_edit_hits_decoration(
        terminal: &Terminal,
        targets: &[usize],
    ) -> bool {
        terminal
            .prompt_line_start()
            .is_some_and(|start| targets.iter().any(|&col| col < start))
    }

    /// Refuse an edit that [`Self::prompt_edit_hits_decoration`] flags, so the
    /// keypress does not just vanish. Returns whether it refused.
    pub(in crate::window) fn decline_decoration_edit(&mut self, blocked: bool) -> bool {
        if blocked {
            self.set_error("Cannot edit: that is the shell's own prompt, not the command line");
        }
        blocked
    }

    /// Apply a Vim delete operator to the last prompt by sending the shell the
    /// equivalent readline edit. Only the live prompt line (the row holding the
    /// shell cursor) is editable; deletes aimed at scrollback history are ignored.
    pub(in crate::window) fn delete_on_prompt(&mut self, op: PromptDelete, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            // The cursor is on scrollback history, not the live prompt: only the
            // prompt line is editable, so report the attempt instead of silently
            // dropping it.
            self.set_error("Cannot delete: not on the editable prompt line");
            return;
        }
        let mut targets = vec![nav_col];
        if let PromptDelete::Range { start_col, .. } = &op {
            targets.push(*start_col);
        }
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(terminal, &targets)) {
            return;
        }
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.record_normal_delete(op, nav_col, pty_col);
        }
        let bytes = prompt_delete_bytes(op, pty_col, nav_col);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.term.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Apply a Vim change operator on the prompt line: deletes the target span and enters Insert mode.
    pub(in crate::window) fn change_on_prompt(&mut self, op: PromptDelete, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, _) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot change: not on the editable prompt line");
            return;
        }
        self.delete_on_prompt(op, focused);
        self.open_tabs.set_mode(&focused, Mode::Insert);
        self.selection.span = None;
        self.selection.visual_anchor = None;
        self.dirty = true;
    }

    /// Replace the character under the cursor on the editable prompt line without entering Insert mode.
    pub(in crate::window) fn replace_char_on_prompt(&mut self, ch: char, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot replace: not on the editable prompt line");
            return;
        }
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(terminal, &[nav_col])) {
            return;
        }
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.desync();
        }
        let bytes = prompt_edit::prompt_replace_char_bytes(pty_col, nav_col, ch);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.term.nav_resync_pending = true;
        self.dirty = true;
    }

    /// Toggle the ASCII case of the character under the cursor and advance the cursor right.
    pub(in crate::window) fn toggle_case_on_prompt(&mut self, focused: TabId) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let (prompt_row, pty_col) = terminal.grid().cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot toggle case: not on the editable prompt line");
            return;
        }
        let ch = terminal.grid().cell(prompt_row, nav_col).map(|c| c.ch);
        if self.decline_decoration_edit(Self::prompt_edit_hits_decoration(terminal, &[nav_col])) {
            return;
        }
        if let Some(c) = ch {
            let toggled = if c.is_ascii_uppercase() {
                c.to_ascii_lowercase()
            } else if c.is_ascii_lowercase() {
                c.to_ascii_uppercase()
            } else {
                self.move_nav_cursor(CursorMove::Right, focused);
                return;
            };
            if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
                shadow.desync();
            }
            let bytes = prompt_edit::prompt_toggle_case_bytes(pty_col, nav_col, toggled);
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.write(&bytes);
            }
            self.move_nav_cursor(CursorMove::Right, focused);
            self.term.nav_resync_pending = true;
            self.dirty = true;
        }
    }

    /// Perform a configurable Insert-mode line edit (e.g. `C-DEL` to
    /// delete the word before the cursor): fold it into the undo shadow and send
    /// the equivalent readline keystrokes to the shell.
    pub(in crate::window) fn edit_on_prompt(&mut self, action: EditAction, focused: TabId) {
        // The chord is forwarded either way: the user pressed it, and the shell
        // is entitled to interpret it. Only the shadow is skipped, because the
        // model of the line would be wrong under other bindings.
        let at_prompt = self.prompt_editing_enabled()
            && self
                .open_tabs
                .terminal(&focused)
                .is_some_and(|terminal| terminal.is_at_prompt());
        if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
            shadow.apply_edit_action(action, at_prompt);
        }
        let bytes = edit_action_bytes(action);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.dirty = true;
    }

    /// Apply a prompt undo (`undo == true`, `C-/`) or redo (`C-\`).
    ///
    /// Undo is delegated to the shell's own undo (`C-_`), the WezTerm approach:
    /// a single readline/ZLE operation, so the line repaints once instead of
    /// flashing through a multi-keystroke rebuild (which syntax-highlight /
    /// autosuggest plugins re-render at every step). The shadow pointer is advanced
    /// in step so a following redo has the right base.
    ///
    /// Shells have no portable redo, so redo is Winter's own: it rebuilds the
    /// recorded target line in one bracketed paste (see [`rebuild_line_bytes`]),
    /// which is robust to whatever state the shell's undo left the line in and
    /// still repaints once. A no-op off the prompt.
    pub(in crate::window) fn prompt_history_apply(&mut self, focused: TabId, undo: bool) {
        if !self.prompt_editing_enabled() {
            self.decline_prompt_edit();
            return;
        }
        let at_prompt = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|terminal| terminal.is_at_prompt());
        if !at_prompt {
            return;
        }
        if undo {
            if let Some(shadow) = self.open_tabs.prompt_shadow_mut(&focused) {
                shadow.step_back();
            }
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                terminal.write(&[READLINE_UNDO]);
            }
            self.dirty = true;
            return;
        }
        let Some(target) = self
            .open_tabs
            .prompt_shadow_mut(&focused)
            .and_then(|shadow| shadow.redo_target())
        else {
            return;
        };
        let bracketed = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|terminal| terminal.bracketed_paste());
        let bytes = rebuild_line_bytes(&target, bracketed);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
        self.term.nav_resync_pending = true;
        self.dirty = true;
    }
}

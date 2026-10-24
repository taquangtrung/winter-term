//! Insert and Replace: the Emacs keys of typing, and writing over text.

use crate::model::editor::Notice;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::textedit::buffer::{TextBuffer, WordCase};
use crate::model::textedit::kill::Merge;
use crate::model::vim::words::prev_word_start;

use super::{
    completion_candidates, current, is_word_char, Completion, EditMode, EditOutcome, Session,
    Yanked, HALF_PAGE, PAGE_OVERLAP,
};

// ========================================================================
// Session: Insert and Replace
// ========================================================================

impl Session<'_> {
    /// Insert mode: an Emacs-style field, since that is what the shell's own
    /// line is, with the chords that edit one.
    pub(super) fn on_insert_key(&mut self, key: &Key) -> EditOutcome {
        if self.ed.insert_only {
            if let Some(outcome) = self.on_field_key(key) {
                return outcome;
            }
            self.group_field_undo(key);
        }
        // Typing is no reason to lose the window chords: `M-h` moves focus,
        // it does not type an `h`. The Emacs Alt chords below leave those, and
        // `M-l` with them, alone.
        if key.alt {
            return match key.ctrl {
                true => EditOutcome::Ignored,
                false => self.on_insert_alt_key(key),
            };
        }
        if key.ctrl {
            return self.on_insert_ctrl_key(key);
        }
        match key.glyph() {
            KeyCode::Escape if self.ed.insert_only => EditOutcome::Cancel,
            KeyCode::Escape => self.leave_insert(),
            // Text whose lines stand for something cannot take a new one, so
            // `Enter` there is the answer being given.
            KeyCode::Enter if self.buffer().is_fixed() => EditOutcome::Submit,
            KeyCode::Enter => {
                self.buffer_mut().insert_newline();
                EditOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.buffer_mut().backspace();
                EditOutcome::Consumed
            }
            KeyCode::Delete => {
                self.buffer_mut().delete_under(1);
                EditOutcome::Consumed
            }
            KeyCode::Tab => {
                self.buffer_mut().insert_char('\t');
                EditOutcome::Consumed
            }
            KeyCode::Space => {
                self.buffer_mut().insert_char(' ');
                EditOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer_mut().insert_char(c);
                EditOutcome::Consumed
            }
            KeyCode::Left => {
                self.buffer_mut().move_left(1);
                EditOutcome::Consumed
            }
            KeyCode::Right => {
                self.buffer_mut().move_right(1, true);
                EditOutcome::Consumed
            }
            KeyCode::Up => {
                self.buffer_mut().move_up(1, true);
                EditOutcome::Consumed
            }
            KeyCode::Down => {
                self.buffer_mut().move_down(1, true);
                EditOutcome::Consumed
            }
            KeyCode::Home => {
                self.buffer_mut().move_line_start();
                EditOutcome::Consumed
            }
            KeyCode::End => {
                self.buffer_mut().move_line_end(true);
                EditOutcome::Consumed
            }
            _ => EditOutcome::Ignored,
        }
    }

    /// The Emacs keys a field that is only ever typed into adds to Insert
    /// mode's: `C-g` withdraws the answer, `C-/` and `C-_` undo, `C-\` redoes,
    /// `M-l` lowercases a word and `M-\` closes up the spaces around the
    /// caret. A field holds the keyboard over the window, so `M-l` is free
    /// there as it is not in a file. `None` for every other key.
    fn on_field_key(&mut self, key: &Key) -> Option<EditOutcome> {
        let ctrl = key.ctrl && !key.alt && !key.sup;
        let KeyCode::Char(c) = key.glyph() else {
            return None;
        };
        match c {
            'g' if ctrl && !key.shift => return Some(EditOutcome::Cancel),
            '/' | '_' if ctrl => {
                self.buffer_mut().undo();
            }
            '\\' if ctrl => {
                self.buffer_mut().redo();
            }
            'l' if key.mods() == Mods::ALT => {
                self.buffer_mut().change_word_case(WordCase::Lower);
            }
            '\\' if key.mods() == Mods::ALT => {
                self.buffer_mut().delete_horizontal_space();
            }
            _ => return None,
        }
        self.ed.typing_run = false;
        Some(EditOutcome::Consumed)
    }

    /// Open a new undo step in a field unless `key` carries on a run of
    /// typing: each word typed, and each other edit, walks back on its own,
    /// where the field's one Insert session would otherwise walk back whole.
    fn group_field_undo(&mut self, key: &Key) {
        let typed = !key.ctrl && !key.alt && !key.sup;
        let character = match key.typed().glyph() {
            KeyCode::Char(c) if typed => Some(c),
            _ => None,
        };
        if character.is_none_or(|c| c == ' ') || !self.ed.typing_run {
            self.buffer_mut().snapshot();
        }
        self.ed.typing_run = character.is_some();
    }

    /// The Ctrl chords of Insert mode: the Emacs ones for moving by character
    /// and line, for killing and yanking, and for editing around the cursor.
    pub(super) fn on_insert_ctrl_key(&mut self, key: &Key) -> EditOutcome {
        if key.mods() != Mods::CTRL {
            return EditOutcome::Ignored;
        }
        // `C-DEL` kills the word behind the cursor, as `M-DEL` does: the chord
        // most editors outside Emacs give it, and the one the prompt line's
        // own `editing` block binds.
        if key.glyph() == KeyCode::Backspace {
            self.kill(false, |buffer| buffer.kill_emacs_word(false));
            return EditOutcome::Consumed;
        }
        let KeyCode::Char(c) = key.glyph() else {
            return EditOutcome::Ignored;
        };
        match c {
            's' => return EditOutcome::Save,
            'a' => self.buffer_mut().move_line_start(),
            'e' => self.buffer_mut().move_line_end(true),
            'b' => self.buffer_mut().move_left(1),
            'f' => self.buffer_mut().move_right(1, true),
            'n' => self.buffer_mut().move_down(1, true),
            'p' => self.buffer_mut().move_up(1, true),
            'd' => self.buffer_mut().delete_forward(),
            'h' => self.buffer_mut().backspace(),
            'k' => self.kill(true, TextBuffer::kill_to_line_end),
            'o' => self.buffer_mut().open_line_here(),
            't' => self.buffer_mut().transpose_chars(),
            'y' => self.yank_kill(),
            'v' => {
                let editor = self.insert_editor();
                self.buffer_mut().move_down(editor, true);
            }
            'l' => self.center_on_cursor(),
            'u' => self.kill(false, |buffer| buffer.take_to(0, false)),
            'w' => {
                let chars = self.buffer().chars();
                let to = prev_word_start(&chars, self.buffer().col(), false).unwrap_or(0);
                self.kill(false, |buffer| buffer.take_to(to, false));
            }
            _ => return EditOutcome::Ignored,
        }
        EditOutcome::Consumed
    }

    /// The Alt chords of Insert mode: the Emacs word and buffer motions and the
    /// edits that work a word at a time. The letters the window keeps (`h`
    /// `j` `k` `l` and the digits) are not among them.
    pub(super) fn on_insert_alt_key(&mut self, key: &Key) -> EditOutcome {
        match (key.mods(), key.glyph()) {
            (Mods::ALT, KeyCode::Char('b')) => self.buffer_mut().move_emacs_word(false),
            (Mods::ALT, KeyCode::Char('f')) => self.buffer_mut().move_emacs_word(true),
            (Mods::ALT, KeyCode::Char('d')) => {
                self.kill(true, |buffer| buffer.kill_emacs_word(true))
            }
            (Mods::ALT, KeyCode::Backspace) => {
                self.kill(false, |buffer| buffer.kill_emacs_word(false))
            }
            (Mods::ALT, KeyCode::Char('y')) => self.yank_pop(),
            (Mods::ALT, KeyCode::Char('/')) => self.complete(),
            (Mods::ALT, KeyCode::Char('t')) => self.buffer_mut().transpose_words(),
            (Mods::ALT, KeyCode::Char('u')) => self.buffer_mut().change_word_case(WordCase::Upper),
            (Mods::ALT, KeyCode::Char('c')) => {
                self.buffer_mut().change_word_case(WordCase::Capitalize)
            }
            (Mods::ALT, KeyCode::Char('m')) => self.buffer_mut().move_first_non_blank(),
            (Mods::ALT, KeyCode::Char('v')) => {
                let editor = self.insert_editor();
                self.buffer_mut().move_up(editor, true);
            }
            (Mods::ALT_SHIFT, KeyCode::Char('<')) => self.buffer_mut().move_text_start(),
            (Mods::ALT_SHIFT, KeyCode::Char('>')) => self.buffer_mut().move_text_end(),
            _ => return EditOutcome::Ignored,
        }
        EditOutcome::Consumed
    }

    /// How far a screen-wise Emacs scroll moves: a page, less the lines that
    /// stay in view to keep the place.
    pub(super) fn insert_editor(&self) -> usize {
        self.view.viewport.saturating_sub(PAGE_OVERLAP).max(1)
    }

    /// Scroll so the cursor's line sits in the middle of the view (`C-l`).
    pub(super) fn center_on_cursor(&mut self) {
        let row = self.buffer().row();
        *self.view.scroll = row.saturating_sub(self.view.viewport / HALF_PAGE);
    }

    /// Run an Emacs kill, and keep what it took on the kill ring: joined onto
    /// the last kill when the key before was one too, after it for a kill
    /// forward and before it for a kill back, so the run yanks back whole.
    pub(super) fn kill(&mut self, forward: bool, edit: impl FnOnce(&mut TextBuffer)) {
        let before = self.buffer().revision();
        edit(self.buffer_mut());
        // A kill with nothing to take keeps the run going, as in Emacs.
        self.ed.kill_chain = self.chained;
        if self.buffer().revision() == before {
            return;
        }
        let merge = match (self.chained, forward) {
            (false, _) => Merge::New,
            (true, true) => Merge::Append,
            (true, false) => Merge::Prepend,
        };
        let taken = self.buffer().register().clone();
        self.ed.kills.kill(taken, merge);
        // The unnamed register holds the whole run, so `p` and `C-y` agree.
        if let Some(run) = self.ed.kills.yank() {
            self.buffer_mut().set_register(run);
        }
        self.ed.kill_chain = true;
    }

    /// `C-y`: put the newest kill back at the caret, or the unnamed register
    /// where nothing has been killed yet.
    pub(super) fn yank_kill(&mut self) {
        if let Some(newest) = self.ed.kills.yank() {
            self.buffer_mut().set_register(newest);
        }
        self.put_yank();
    }

    /// Yank the register at the caret, remembering where it went for `M-y`.
    pub(super) fn put_yank(&mut self) {
        let start = (self.buffer().row(), self.buffer().col());
        self.buffer_mut().yank();
        let end = (self.buffer().row(), self.buffer().col());
        if end != start {
            self.ed.yanked = Some(Yanked { end, start });
        }
    }

    /// `M-y` straight after a yank: swap what it put in for the kill before.
    pub(super) fn yank_pop(&mut self) {
        let Some(yanked) = self.yanked else {
            self.ed.notice = Some(Notice::error("the last command was not a yank"));
            return;
        };
        let Some(older) = self.ed.kills.yank_pop() else {
            return;
        };
        self.buffer_mut()
            .take_between(yanked.start, yanked.end, false);
        self.buffer_mut()
            .move_to_typing(yanked.start.0, yanked.start.1);
        self.buffer_mut().set_register(older);
        self.put_yank();
    }

    /// `M-/`: complete the word before the caret from the words around it,
    /// nearest first, then from the other open texts. Pressed again it puts
    /// in the next candidate, and after the last, the word as it was typed.
    pub(super) fn complete(&mut self) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let continuing = self
            .completing
            .take()
            .filter(|done| done.row == row && done.start + current(done).chars().count() == col);
        let mut completion = match continuing {
            Some(completion) => completion,
            None => {
                let chars = self.buffer().chars();
                let mut start = col.min(chars.len());
                while start > 0 && is_word_char(chars[start - 1]) {
                    start -= 1;
                }
                let typed: String = chars[start..col.min(chars.len())].iter().collect();
                if typed.is_empty() {
                    return;
                }
                let candidates = completion_candidates(
                    self.buffer().lines(),
                    (row, start),
                    &typed,
                    self.view.others,
                );
                if candidates.is_empty() {
                    self.ed.notice = Some(Notice::error(format!("nothing completes `{typed}`")));
                    return;
                }
                Completion {
                    candidates,
                    index: None,
                    row,
                    start,
                    typed,
                }
            }
        };
        completion.index = match completion.index {
            None => Some(0),
            Some(at) if at + 1 < completion.candidates.len() => Some(at + 1),
            Some(_) => None,
        };
        let kept = self.buffer().register().clone();
        self.buffer_mut()
            .take_between((row, completion.start), (row, col), false);
        self.buffer_mut().move_to_typing(row, completion.start);
        for c in current(&completion).chars() {
            self.buffer_mut().insert_char(c);
        }
        self.buffer_mut().set_register(kept);
        self.ed.completion = Some(completion);
    }

    /// Replace mode (`R`): every character typed writes over the one under
    /// the cursor rather than pushing it along.
    pub(super) fn on_replace_key(&mut self, key: &Key) -> EditOutcome {
        if key.alt || key.ctrl {
            return EditOutcome::Ignored;
        }
        match key.glyph() {
            KeyCode::Escape => self.leave_insert(),
            KeyCode::Backspace => {
                self.buffer_mut().move_left(1);
                EditOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer_mut().replace_char_over(c);
                EditOutcome::Consumed
            }
            _ => self.on_insert_key(key),
        }
    }

    /// Back to Normal, with the cursor stepped onto the character it was
    /// typing past, which is where Vim leaves it.
    pub(super) fn leave_insert(&mut self) -> EditOutcome {
        self.ed.mode = EditMode::Normal;
        self.buffer_mut().move_left(1);
        self.buffer_mut().clamp(false);
        EditOutcome::Consumed
    }

    /// Start typing, with an undo step open so the whole run walks back at
    /// once rather than a character at a time.
    pub(super) fn enter_insert(&mut self) -> EditOutcome {
        self.buffer_mut().snapshot();
        self.ed.mode = EditMode::Insert;
        EditOutcome::Consumed
    }
}

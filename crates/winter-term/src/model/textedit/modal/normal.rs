//! Normal mode: the keys that move, edit in place, and open commands.

use crate::model::editor::Notice;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::vim::grammar::{operator_of, push_count_digit, OperatorKind};
use crate::model::vim::nav::VimKey;

use super::{
    find_wait, BracketWait, EditMode, EditOutcome, MarkJump, Pending, Session, READ_ONLY_MESSAGE,
};

// ========================================================================
// Session: Normal mode
// ========================================================================

impl Session<'_> {
    /// Normal mode: motions, operators, and the commands that change text.
    pub(super) fn on_normal_key(&mut self, key: &Key) -> EditOutcome {
        if let Some(pending) = self.ed.pending {
            return self.resolve_pending(pending, key);
        }
        if self.take_digit(key) {
            return EditOutcome::Consumed;
        }
        let code = match key.chord() {
            (Mods::NONE, code) => code,
            (Mods::CTRL, _) => return self.on_ctrl_key(key),
            // An Alt chord belongs to the window: `M-p` moves focus rather
            // than pasting. Only the two buffer ends the shared layer claims
            // stay here, and no other chord is read as its plain key.
            _ => return self.shared_motion(key),
        };
        // A count or a register typed and then thought better of: `Esc` drops
        // it rather than closing a file over a half-typed command.
        if code == KeyCode::Escape && (self.ed.count.is_some() || self.ed.register.is_some()) {
            self.ed.count = None;
            self.ed.register = None;
            return EditOutcome::Consumed;
        }
        if self.ed.read_only {
            if let KeyCode::Char(
                'i' | 'a' | 'I' | 'A' | 'o' | 'O' | 'R' | 's' | 'S' | 'x' | 'X' | 'C' | 'D' | 'r'
                | '~' | 'J' | 'p' | 'P' | 'u' | '.' | 'd' | 'c' | '>' | '<',
            ) = code
            {
                self.ed.notice = Some(Notice::error(READ_ONLY_MESSAGE));
                return EditOutcome::Consumed;
            }
        }
        match code {
            KeyCode::Char('i') => self.enter_insert(),
            KeyCode::Char('a') => {
                self.buffer_mut().move_right(1, true);
                self.enter_insert()
            }
            KeyCode::Char('I') => {
                self.buffer_mut().move_first_non_blank();
                self.enter_insert()
            }
            KeyCode::Char('A') => {
                self.buffer_mut().move_line_end(true);
                self.enter_insert()
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                let below = code == KeyCode::Char('o');
                self.buffer_mut().snapshot();
                self.buffer_mut().open_line(below);
                self.ed.mode = EditMode::Insert;
                EditOutcome::Consumed
            }
            KeyCode::Char('R') => {
                self.buffer_mut().snapshot();
                self.ed.mode = EditMode::Replace;
                EditOutcome::Consumed
            }
            KeyCode::Char('v') => self.start_visual(false),
            KeyCode::Char('V') => self.start_visual(true),
            KeyCode::Char('x') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().delete_under(count);
                self.store_take()
            }
            KeyCode::Char('X') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                let (row, col) = (self.buffer().row(), self.buffer().col());
                self.buffer_mut()
                    .take_between((row, col.saturating_sub(count)), (row, col), false);
                self.store_take()
            }
            KeyCode::Char('s') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().delete_under(count);
                self.ed.mode = EditMode::Insert;
                EditOutcome::Consumed
            }
            KeyCode::Char('S') => {
                let operator = self.operator_of(OperatorKind::Change);
                self.operate_on_lines(operator)
            }
            KeyCode::Char('C') => {
                let operator = self.operator_of(OperatorKind::Change);
                self.line_tail(operator)
            }
            KeyCode::Char('D') => {
                let operator = self.operator_of(OperatorKind::Delete);
                self.line_tail(operator)
            }
            KeyCode::Char('r') => {
                self.ed.pending = Some(Pending::Replace);
                EditOutcome::Consumed
            }
            KeyCode::Char('~') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().toggle_case(count);
                EditOutcome::Consumed
            }
            KeyCode::Char('J') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().join_lines(count);
                EditOutcome::Consumed
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                let after = code == KeyCode::Char('p');
                self.paste(after)
            }
            KeyCode::Char('u') => {
                if !self.buffer_mut().undo() {
                    self.ed.notice = Some(Notice::error("nothing to undo"));
                }
                // An undo is not a change to be done again: `.` still means
                // whatever changed the text last.
                self.ed.revision = self.buffer().revision();
                EditOutcome::Consumed
            }
            KeyCode::Char('.') => self.repeat_change(),
            KeyCode::Char('d' | 'c' | 'y' | '>' | '<') => match operator_of(key) {
                Some(kind) => self.start_operator(kind),
                None => EditOutcome::Consumed,
            },
            KeyCode::Char('q') => {
                self.ed.pending = Some(Pending::RecordMacro);
                EditOutcome::Consumed
            }
            KeyCode::Char('@') => {
                self.ed.pending = Some(Pending::ReplayMacro);
                EditOutcome::Consumed
            }
            KeyCode::Char('"') => {
                self.ed.pending = Some(Pending::Register);
                EditOutcome::Consumed
            }
            KeyCode::Char('m') => {
                self.ed.pending = Some(Pending::Mark);
                EditOutcome::Consumed
            }
            KeyCode::Char('\'') => {
                self.ed.pending = Some(Pending::Jump(MarkJump { exact: false }));
                EditOutcome::Consumed
            }
            KeyCode::Char('`') => {
                self.ed.pending = Some(Pending::Jump(MarkJump { exact: true }));
                EditOutcome::Consumed
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.ed.pending = Some(Pending::Find(find_wait(key)));
                EditOutcome::Consumed
            }
            KeyCode::Char(';') | KeyCode::Char(',') => {
                let reverse = code == KeyCode::Char(',');
                self.repeat_find(reverse)
            }
            KeyCode::Char('/') | KeyCode::Char('?') => EditOutcome::Search {
                forward: code == KeyCode::Char('/'),
            },
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let reverse = code == KeyCode::Char('N');
                self.search_step(reverse)
            }
            KeyCode::Char('*') | KeyCode::Char('#') => {
                let forward = code == KeyCode::Char('*');
                self.search_word(forward)
            }
            KeyCode::Char('g') => {
                self.ed.pending = Some(Pending::Goto);
                EditOutcome::Consumed
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let forward = code == KeyCode::Char(']');
                self.ed.pending = Some(Pending::Bracket(BracketWait { forward }));
                EditOutcome::Consumed
            }
            KeyCode::Char('z') => {
                self.ed.pending = Some(Pending::Scroll);
                EditOutcome::Consumed
            }
            KeyCode::Char('Z') => {
                self.ed.pending = Some(Pending::Leave);
                EditOutcome::Consumed
            }
            _ => self.shared_motion(key),
        }
    }

    /// The Ctrl chords: saving, redo, the number keys, and the paging motions.
    pub(super) fn on_ctrl_key(&mut self, key: &Key) -> EditOutcome {
        // Only the bare Ctrl chords are these; `C-S-d` is the window's.
        let (Mods::CTRL, code) = key.chord() else {
            return self.shared_motion(key);
        };
        if self.ed.read_only && matches!(code, KeyCode::Char('s' | 'r' | 'a' | 'x')) {
            self.ed.notice = Some(Notice::error(READ_ONLY_MESSAGE));
            return EditOutcome::Consumed;
        }
        match code {
            KeyCode::Char('s') => EditOutcome::Save,
            KeyCode::Char('r') => {
                if !self.buffer_mut().redo() {
                    self.ed.notice = Some(Notice::error("nothing to redo"));
                }
                self.ed.revision = self.buffer().revision();
                EditOutcome::Consumed
            }
            KeyCode::Char('a') | KeyCode::Char('x') => {
                let count = self.take_count() as i64;
                let by = match code == KeyCode::Char('a') {
                    true => count,
                    false => -count,
                };
                self.buffer_mut().snapshot();
                if !self.buffer_mut().add_to_number(by) {
                    self.ed.notice = Some(Notice::error("no number here"));
                }
                EditOutcome::Consumed
            }
            KeyCode::Char('o') => EditOutcome::Escalate,
            _ => self.shared_motion(key),
        }
    }

    /// A key the engine does not claim, offered to the shared Vim layer so a
    /// motion means here what it means over every other surface.
    pub(super) fn shared_motion(&mut self, key: &Key) -> EditOutcome {
        let typed = self.ed.count.take();
        match self.ed.nav.key(key) {
            VimKey::Motion(motion) => {
                self.jump_for(motion);
                self.apply_motion(motion, typed)
            }
            VimKey::Pending => EditOutcome::Consumed,
            VimKey::Unhandled => EditOutcome::Ignored,
        }
    }

    /// A digit typed before a command, which belongs to the command that
    /// follows it. A leading zero is the motion to column one instead.
    pub(super) fn take_digit(&mut self, key: &Key) -> bool {
        match push_count_digit(key, self.ed.count) {
            Some(count) => {
                self.ed.count = Some(count);
                true
            }
            None => false,
        }
    }
}

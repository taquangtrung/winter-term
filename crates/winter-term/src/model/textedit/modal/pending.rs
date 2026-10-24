//! The key a part-typed command was waiting on.

use crate::model::editor::Notice;
use crate::model::input::{Key, KeyCode};
use crate::model::vim::grammar::{
    is_register_name, is_settable_mark, mark_name, table_entry, BLACK_HOLE_REGISTER, GOTO_MOTIONS,
    JUMP_MARK, LINE_PLACEMENTS,
};
use crate::model::vim::motion::{CursorMove, FindChar};

use super::{
    EditOutcome, FindWait, MarkJump, Pending, Reach, Session, EDITOR_REGISTERS, GOTO_BUFFERS,
    HALF_PAGE, LEAVE,
};

// ========================================================================
// Session: pending commands
// ========================================================================

impl Session<'_> {
    /// The second key of a command: the character `r` writes, the motion an
    /// operator reaches over, the object it takes, or the letter naming a
    /// register or a mark.
    pub(super) fn resolve_pending(&mut self, pending: Pending, key: &Key) -> EditOutcome {
        self.ed.pending = None;
        if key.glyph() == KeyCode::Escape {
            self.ed.operator = None;
            self.ed.count = None;
            return EditOutcome::Consumed;
        }
        match pending {
            // `]b` and `[b` step through the open files, the way they step
            // through the blocks of a shell's output.
            Pending::Bracket(wait) => match key.glyph() {
                KeyCode::Char('b') => EditOutcome::StepBuffer {
                    forward: wait.forward,
                },
                _ => EditOutcome::Consumed,
            },
            Pending::Find(wait) => self.resolve_find(wait, key),
            Pending::Goto => self.resolve_goto(key),
            Pending::Jump(jump) => self.jump_to_mark(jump, key),
            Pending::Leave => match table_entry(LEAVE, key) {
                Some(save) => EditOutcome::Quit { save },
                None => EditOutcome::Consumed,
            },
            Pending::Mark => {
                if let KeyCode::Char(c) = key.glyph() {
                    if is_settable_mark(c) {
                        let at = (self.buffer().row(), self.buffer().col());
                        self.view.marks.named.insert(c, at);
                    }
                }
                EditOutcome::Consumed
            }
            Pending::Motion => self.resolve_motion(key),
            Pending::Object(wait) => self.resolve_object(wait, key),
            Pending::Register => {
                if let KeyCode::Char(c) = key.glyph() {
                    if !is_register_name(c) && !EDITOR_REGISTERS.contains(&c) {
                        return EditOutcome::Consumed;
                    }
                    self.ed.register = Some(c);
                    if c == BLACK_HOLE_REGISTER {
                        self.ed.held = Some(self.buffer().register().clone());
                    }
                }
                EditOutcome::Consumed
            }
            Pending::Replace => self.resolve_replace(key),
            Pending::Scroll => {
                let at = match table_entry(LINE_PLACEMENTS, key) {
                    Some(CursorMove::LineToCenter) => self.view.viewport / HALF_PAGE,
                    Some(CursorMove::LineToTop) => 0,
                    Some(CursorMove::LineToBottom) => self.view.viewport.saturating_sub(1),
                    Some(_) | None => return EditOutcome::Consumed,
                };
                *self.view.scroll = self.buffer().row().saturating_sub(at);
                EditOutcome::Consumed
            }
        }
    }

    /// `r`: write one character over the one under the cursor, or over every
    /// character of a selection.
    pub(super) fn resolve_replace(&mut self, key: &Key) -> EditOutcome {
        let c = match key.glyph() {
            KeyCode::Char(c) => c,
            KeyCode::Space => ' ',
            _ => return EditOutcome::Consumed,
        };
        self.buffer_mut().snapshot();
        match self.selection() {
            Some(selection) => {
                self.fill_selection(selection, c);
                self.leave_visual()
            }
            None => {
                self.buffer_mut().replace_char(c);
                EditOutcome::Consumed
            }
        }
    }

    /// The `g` commands the engine carries: the shared [`GOTO_MOTIONS`] and
    /// [`GOTO_BUFFERS`].
    pub(super) fn resolve_goto(&mut self, key: &Key) -> EditOutcome {
        if key.glyph() == KeyCode::Char(GOTO_BUFFERS.0) {
            self.ed.operator = None;
            return EditOutcome::ListBuffers;
        }
        let Some(motion) = table_entry(GOTO_MOTIONS, key) else {
            self.ed.operator = None;
            return EditOutcome::Consumed;
        };
        let typed = self.ed.count.take();
        self.jump_for(motion);
        match self.ed.operator.take() {
            Some(operator) => self.operate_over(operator, motion, typed),
            None => {
                self.apply_motion(motion, typed);
                EditOutcome::Consumed
            }
        }
    }

    /// The character a char search was waiting for, which now stands as the
    /// search `;` and `,` repeat.
    pub(super) fn resolve_find(&mut self, wait: FindWait, key: &Key) -> EditOutcome {
        let ch = match key.glyph() {
            KeyCode::Char(ch) => ch,
            KeyCode::Space => ' ',
            _ => {
                self.ed.operator = None;
                return EditOutcome::Consumed;
            }
        };
        let find = FindChar {
            ch,
            forward: wait.forward,
            till: wait.till,
        };
        self.ed.find = Some(find);
        self.run_find(find)
    }

    /// The last char search again (`;`), or the other way (`,`).
    pub(super) fn repeat_find(&mut self, reverse: bool) -> EditOutcome {
        let Some(find) = self.ed.find else {
            return EditOutcome::Consumed;
        };
        let find = match reverse {
            true => find.reversed(),
            false => find,
        };
        self.run_find(find)
    }

    /// Move to where a char search lands, or act over it where an operator is
    /// waiting on one.
    pub(super) fn run_find(&mut self, find: FindChar) -> EditOutcome {
        let count = self.take_count();
        let operator = self.ed.operator.take();
        let mut probe = self.buffer().clone();
        if !probe.move_to_char(find, count) {
            self.ed.notice = Some(Notice::error(format!("no `{}` on this line", find.ch)));
            return EditOutcome::Consumed;
        }
        let to = (probe.row(), probe.col());
        match operator {
            Some(operator) => self.operate(
                operator,
                Reach {
                    // A char search takes the cell it stops on, which is what
                    // makes `df,` reach through the comma and `dt,` up to it.
                    inclusive: true,
                    linewise: false,
                    to,
                },
            ),
            None => {
                self.buffer_mut().move_to(to.0, to.1);
                EditOutcome::Consumed
            }
        }
    }

    /// Go to a mark (`'a`, `` `a ``), or back to where the last jump started
    /// (`''`).
    pub(super) fn jump_to_mark(&mut self, jump: MarkJump, key: &Key) -> EditOutcome {
        let KeyCode::Char(c) = key.glyph() else {
            return EditOutcome::Consumed;
        };
        let at = match mark_name(c) {
            Some(JUMP_MARK) => self.view.marks.jump,
            Some(mark) => self.view.marks.named.get(&mark).copied(),
            None => return EditOutcome::Consumed,
        };
        let Some((row, col)) = at else {
            self.ed.notice = Some(Notice::error(format!("mark `{c}` is not set")));
            return EditOutcome::Consumed;
        };
        self.push_jump();
        match jump.exact {
            true => self.buffer_mut().move_to(row, col),
            false => self.buffer_mut().move_to_line(row),
        }
        EditOutcome::Consumed
    }
}

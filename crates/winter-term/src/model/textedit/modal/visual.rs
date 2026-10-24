//! Visual mode: acting on a selection.

use crate::model::editor::Notice;
use crate::model::input::{Key, KeyCode};
use crate::model::vim::grammar::{operator_of, OperatorKind};

use super::{
    find_wait, Anchor, EditMode, EditOutcome, ObjectWait, Pending, Selection, Session,
    READ_ONLY_MESSAGE,
};

// ========================================================================
// Session: Visual mode
// ========================================================================

impl Session<'_> {
    /// Start selecting from where the cursor is (`v`/`V`).
    pub(super) fn start_visual(&mut self, linewise: bool) -> EditOutcome {
        self.ed.start_visual(self.view.buffer, linewise);
        EditOutcome::Consumed
    }

    /// Stop selecting, leaving the cursor where it is.
    pub(super) fn leave_visual(&mut self) -> EditOutcome {
        self.ed.anchor = None;
        self.ed.mode = EditMode::Normal;
        self.buffer_mut().clamp(false);
        EditOutcome::Consumed
    }

    pub(super) fn selection(&self) -> Option<Selection> {
        self.ed.selection(self.view.buffer)
    }

    /// Visual mode: the motions move one end of the selection, and an
    /// operator acts on everything between the two.
    pub(super) fn on_visual_key(&mut self, key: &Key) -> EditOutcome {
        if let Some(pending) = self.ed.pending {
            return self.resolve_pending(pending, key);
        }
        if key.alt {
            return self.shared_motion(key);
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        if self.take_digit(key) {
            return EditOutcome::Consumed;
        }
        let Some(selection) = self.selection() else {
            return self.leave_visual();
        };
        if self.ed.read_only {
            if let KeyCode::Char('d' | 'x' | 'c' | 's' | '>' | '<' | 'J' | '~' | 'p' | 'P' | 'r') =
                key.glyph()
            {
                self.ed.notice = Some(Notice::error(READ_ONLY_MESSAGE));
                return EditOutcome::Consumed;
            }
        }
        match key.glyph() {
            KeyCode::Escape | KeyCode::Char('q') => self.leave_visual(),
            KeyCode::Char('v') => match selection.linewise {
                true => self.start_visual(false),
                false => self.leave_visual(),
            },
            KeyCode::Char('V') => match selection.linewise {
                true => self.leave_visual(),
                false => self.start_visual(true),
            },
            KeyCode::Char('o') => {
                let anchor = self.ed.anchor.take();
                if let Some(anchor) = anchor {
                    self.ed.anchor = Some(Anchor {
                        col: self.buffer().col(),
                        linewise: anchor.linewise,
                        row: self.buffer().row(),
                    });
                    self.buffer_mut().move_to(anchor.row, anchor.col);
                }
                EditOutcome::Consumed
            }
            // Visual mode's own spellings of two of them.
            KeyCode::Char('x') => self.operate_on_selection(selection, OperatorKind::Delete),
            KeyCode::Char('s') => self.operate_on_selection(selection, OperatorKind::Change),
            KeyCode::Char('d' | 'c' | 'y' | '>' | '<') => match operator_of(key) {
                Some(kind) => self.operate_on_selection(selection, kind),
                None => EditOutcome::Consumed,
            },
            KeyCode::Char('J') => {
                self.buffer_mut().snapshot();
                self.buffer_mut().move_to(selection.start.0, 0);
                let rows = selection.end.0 - selection.start.0;
                self.buffer_mut().join_lines(rows.max(1));
                self.leave_visual()
            }
            KeyCode::Char('~') => {
                self.buffer_mut().snapshot();
                self.flip_selection(selection);
                self.leave_visual()
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                // What the selection held is not what goes back in its place,
                // so the register survives the delete that makes room.
                let register = self.buffer().register().clone();
                self.buffer_mut().snapshot();
                self.cut_selection(selection, false);
                self.buffer_mut().set_register(register);
                let linewise = self.buffer().register().linewise;
                self.buffer_mut().paste(linewise && selection.linewise);
                self.leave_visual()
            }
            KeyCode::Char('r') => {
                self.ed.pending = Some(Pending::Replace);
                EditOutcome::Consumed
            }
            KeyCode::Char('i') | KeyCode::Char('a') => {
                let around = key.glyph() == KeyCode::Char('a');
                self.ed.pending = Some(Pending::Object(ObjectWait { around }));
                EditOutcome::Consumed
            }
            KeyCode::Char('"') => {
                self.ed.pending = Some(Pending::Register);
                EditOutcome::Consumed
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.ed.pending = Some(Pending::Find(find_wait(key)));
                EditOutcome::Consumed
            }
            KeyCode::Char('g') => {
                self.ed.pending = Some(Pending::Goto);
                EditOutcome::Consumed
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let reverse = key.glyph() == KeyCode::Char('N');
                self.search_step(reverse)
            }
            _ => self.shared_motion(key),
        }
    }

    /// Act on what is selected, and stop selecting: an operator is the end of
    /// a Visual run whichever one it is.
    pub(super) fn operate_on_selection(
        &mut self,
        selection: Selection,
        kind: OperatorKind,
    ) -> EditOutcome {
        let register = self.ed.register.take();
        self.buffer_mut().snapshot();
        match kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                self.buffer_mut().move_to(selection.start.0, 0);
                let rows = selection.end.0 - selection.start.0 + 1;
                self.buffer_mut()
                    .shift_lines(rows, kind == OperatorKind::Indent);
                self.leave_visual()
            }
            OperatorKind::Yank => {
                self.cut_selection(selection, true);
                let outcome = self.store_register(register, true);
                self.leave_visual();
                outcome
            }
            OperatorKind::Delete => {
                self.cut_selection(selection, false);
                let outcome = self.store_register(register, false);
                self.leave_visual();
                outcome
            }
            OperatorKind::Change => {
                match selection.linewise {
                    true => {
                        self.buffer_mut().move_to(selection.start.0, 0);
                        self.buffer_mut()
                            .clear_lines(selection.end.0 - selection.start.0 + 1);
                    }
                    false => self.cut_selection(selection, false),
                }
                let outcome = self.store_register(register, false);
                self.ed.anchor = None;
                self.ed.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// Take the selected text into the register, removing it unless `keep`.
    pub(super) fn cut_selection(&mut self, selection: Selection, keep: bool) {
        if selection.linewise {
            self.buffer_mut()
                .take_rows(selection.start.0, selection.end.0, keep);
            return;
        }
        // The cursor's own cell is inside the selection, so the range runs one
        // past it: a Visual selection is inclusive at both ends.
        let end = (selection.end.0, selection.end.1 + 1);
        self.buffer_mut().take_between(selection.start, end, keep);
    }

    /// Flip the case of every character selected (`~`).
    pub(super) fn flip_selection(&mut self, selection: Selection) {
        for row in selection.start.0..=selection.end.0 {
            let (from, to) = self.selected_on(selection, row);
            if from >= to {
                continue;
            }
            self.buffer_mut().move_to(row, from);
            self.buffer_mut().toggle_case(to - from);
        }
        self.buffer_mut()
            .move_to(selection.start.0, selection.start.1);
    }

    /// Write one character over every character selected (`r`).
    pub(super) fn fill_selection(&mut self, selection: Selection, c: char) {
        for row in selection.start.0..=selection.end.0 {
            let (from, to) = self.selected_on(selection, row);
            for col in from..to {
                self.buffer_mut().move_to(row, col);
                self.buffer_mut().replace_char(c);
            }
        }
        self.buffer_mut()
            .move_to(selection.start.0, selection.start.1);
    }

    /// The half-open range of columns `selection` covers on `row`.
    pub(super) fn selected_on(&self, selection: Selection, row: usize) -> (usize, usize) {
        let len = self.buffer().line_len_of(row);
        let from = match row == selection.start.0 && !selection.linewise {
            true => selection.start.1,
            false => 0,
        };
        let to = match row == selection.end.0 && !selection.linewise {
            true => (selection.end.1 + 1).min(len),
            false => len,
        };
        (from, to)
    }
}

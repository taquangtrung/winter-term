//! Visual mode: acting on a selection.

use crate::editor::Notice;
use crate::input::{Key, KeyCode, Mods};
use crate::textedit::buffer::Register;
use crate::vim::grammar::{operator_of, OperatorKind};

use super::{
    find_wait, Anchor, BlockInsert, EditMode, EditOutcome, ObjectWait, Pending, Selection, Session,
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

    /// Start selecting a block from where the cursor is (`C-v`).
    pub(super) fn start_visual_block(&mut self) -> EditOutcome {
        self.ed.anchor = Some(Anchor {
            block: true,
            col: self.buffer().col(),
            linewise: false,
            row: self.buffer().row(),
        });
        self.ed.mode = EditMode::Visual;
        EditOutcome::Consumed
    }

    /// Make the selection whole lines or a block from the same anchor, as
    /// `v`, `V`, and `C-v` do over a selection of another kind.
    fn switch_visual(&mut self, linewise: bool, block: bool) -> EditOutcome {
        if let Some(anchor) = self.ed.anchor {
            self.ed.anchor = Some(Anchor {
                block,
                linewise,
                ..anchor
            });
        }
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
        if self.take_digit(key) {
            return EditOutcome::Consumed;
        }
        let code = match key.chord() {
            (Mods::NONE, code) => code,
            (Mods::CTRL, KeyCode::Char('v')) => {
                return match self.selection().is_some_and(|selection| selection.block) {
                    true => self.leave_visual(),
                    false => self.switch_visual(false, true),
                };
            }
            (Mods::CTRL, _) => return self.on_ctrl_key(key),
            // An Alt chord is the window's, as in Normal mode, and no other
            // chord is read as its plain key.
            _ => return self.shared_motion(key),
        };
        let Some(selection) = self.selection() else {
            return self.leave_visual();
        };
        if self.ed.read_only {
            if let KeyCode::Char(
                'd' | 'x' | 'c' | 's' | '>' | '<' | 'J' | '~' | 'p' | 'P' | 'r' | 'I' | 'A',
            ) = code
            {
                self.ed.notice = Some(Notice::error(READ_ONLY_MESSAGE));
                return EditOutcome::Consumed;
            }
        }
        match code {
            KeyCode::Escape | KeyCode::Char('q') => self.leave_visual(),
            KeyCode::Char('v') if selection.block => self.switch_visual(false, false),
            KeyCode::Char('V') if selection.block => self.switch_visual(true, false),
            KeyCode::Char('v') => match selection.linewise {
                true => self.start_visual(false),
                false => self.leave_visual(),
            },
            KeyCode::Char('V') => match selection.linewise {
                true => self.leave_visual(),
                false => self.start_visual(true),
            },
            // Typing in front of a block, or after it, on every row of it.
            KeyCode::Char('I') if selection.block => {
                self.buffer_mut().snapshot();
                self.insert_in_block(selection, selection.start.1, false)
            }
            KeyCode::Char('A') if selection.block => {
                self.buffer_mut().snapshot();
                self.insert_in_block(selection, selection.end.1 + 1, true)
            }
            KeyCode::Char('o') => {
                let anchor = self.ed.anchor.take();
                if let Some(anchor) = anchor {
                    self.ed.anchor = Some(Anchor {
                        block: anchor.block,
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
            KeyCode::Char('Y') => self.operate_on_selection(selection, OperatorKind::Yank),
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
                let around = code == KeyCode::Char('a');
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
                let reverse = code == KeyCode::Char('N');
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
            OperatorKind::Change if selection.block => {
                self.cut_selection(selection, false);
                let outcome = self.store_register(register, false);
                self.insert_in_block(selection, selection.start.1, false);
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
        if selection.block {
            self.cut_block(selection, keep);
            return;
        }
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

    /// Take a block's columns into the register, a row of them to a line,
    /// removing them unless `keep`.
    fn cut_block(&mut self, selection: Selection, keep: bool) {
        let mut taken = Vec::new();
        for row in selection.start.0..=selection.end.0 {
            let (from, to) = self.selected_on(selection, row);
            let chars: Vec<char> = self.buffer().lines()[row].chars().collect();
            taken.push(chars[from..to].iter().collect::<String>());
            if !keep && from < to {
                let rest: String = chars[..from].iter().chain(&chars[to..]).collect();
                self.buffer_mut().replace_line(row, rest, false);
            }
        }
        self.buffer_mut().set_register(Register {
            linewise: false,
            text: taken.join("\n"),
        });
        self.buffer_mut()
            .move_to(selection.start.0, selection.start.1);
    }

    /// Start typing at `col` of a block's first row, for the block's other
    /// rows to take once Insert mode is left (see `finish_block_insert`).
    /// With `pad`, a row too short to reach `col` is filled out to it with
    /// spaces first; without, it is left alone. The undo step is the
    /// caller's to open.
    fn insert_in_block(&mut self, selection: Selection, col: usize, pad: bool) -> EditOutcome {
        let first = selection.start.0;
        let len = self.buffer().line_len_of(first);
        if pad && len < col {
            let mut line = self.buffer().lines()[first].clone();
            line.extend(std::iter::repeat_n(' ', col - len));
            self.buffer_mut().replace_line(first, line, false);
        }
        self.buffer_mut().move_to_typing(first, col);
        let col = self.buffer().col();
        self.ed.block_insert = Some(BlockInsert {
            col,
            first,
            last: selection.end.0,
            len_before: self.buffer().line_len_of(first),
            lines_before: self.buffer().lines().len(),
            pad,
        });
        self.ed.anchor = None;
        self.ed.mode = EditMode::Insert;
        EditOutcome::Consumed
    }

    /// Copy what was typed into a block's first row to the block's other
    /// rows, as Insert mode is left. Typing that broke the line, moved off
    /// it, or took more away than it put in is not copied: there is no one
    /// run of text to repeat.
    pub(super) fn finish_block_insert(&mut self) {
        let Some(block) = self.ed.block_insert.take() else {
            return;
        };
        let stayed =
            self.buffer().row() == block.first && self.buffer().lines().len() == block.lines_before;
        let typed = self
            .buffer()
            .line_len_of(block.first)
            .checked_sub(block.len_before);
        let Some(typed) = typed.filter(|&typed| stayed && typed > 0) else {
            return;
        };
        let text: Vec<char> = self.buffer().lines()[block.first]
            .chars()
            .skip(block.col)
            .take(typed)
            .collect();
        for row in block.first + 1..=block.last {
            let mut chars: Vec<char> = self.buffer().lines()[row].chars().collect();
            if chars.len() < block.col {
                if !block.pad {
                    continue;
                }
                chars.resize(block.col, ' ');
            }
            let line: String = chars[..block.col]
                .iter()
                .chain(&text)
                .chain(&chars[block.col..])
                .collect();
            self.buffer_mut().replace_line(row, line, false);
        }
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
        if selection.block {
            return (selection.start.1.min(len), (selection.end.1 + 1).min(len));
        }
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

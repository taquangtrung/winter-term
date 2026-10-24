//! Operators, and the motions and text objects that say how far they reach.

use crate::model::editor::Notice;
use crate::model::input::{Key, KeyCode, TextObject};
use crate::model::textedit::buffer::Register;
use crate::model::textedit::kill::Merge;
use crate::model::vim::grammar::{OperatorKind, MAX_COUNT};
use crate::model::vim::motion::CursorMove;
use crate::model::vim::nav::bare_motion;
use crate::model::vim::objects::{
    bracket_object, paragraph_object, quote_object, sentence_object, word_object, Span,
};
use crate::model::vim::words::next_word_start;

use super::{
    doubled_key, find_wait, motion_kind, move_cursor, order, Anchor, EditMode, EditOutcome,
    ObjectWait, Operator, Pending, Reach, Session, NUMBERED_REGISTERS,
};

// ========================================================================
// Session: operators
// ========================================================================

impl Session<'_> {
    /// An operator with the count and register typed before it.
    pub(super) fn operator_of(&mut self, kind: OperatorKind) -> Operator {
        Operator {
            count: self.take_count(),
            kind,
            register: self.ed.register.take(),
        }
    }

    /// Open an operator, which now waits for the motion or object saying how
    /// far it reaches.
    pub(super) fn start_operator(&mut self, kind: OperatorKind) -> EditOutcome {
        self.ed.operator = Some(self.operator_of(kind));
        self.ed.pending = Some(Pending::Motion);
        EditOutcome::Consumed
    }

    /// The key after an operator: its own key doubles it over whole lines, an
    /// `i` or `a` opens a text object, and anything else is its motion.
    pub(super) fn resolve_motion(&mut self, key: &Key) -> EditOutcome {
        let Some(operator) = self.ed.operator else {
            return EditOutcome::Consumed;
        };
        if self.take_digit(key) {
            self.ed.pending = Some(Pending::Motion);
            return EditOutcome::Consumed;
        }
        match key.glyph() {
            KeyCode::Char('i') | KeyCode::Char('a') => {
                let around = key.glyph() == KeyCode::Char('a');
                self.ed.pending = Some(Pending::Object(ObjectWait { around }));
                return EditOutcome::Consumed;
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.ed.pending = Some(Pending::Find(find_wait(key)));
                return EditOutcome::Consumed;
            }
            KeyCode::Char('g') => {
                self.ed.pending = Some(Pending::Goto);
                return EditOutcome::Consumed;
            }
            _ => {}
        }
        self.ed.operator = None;
        // A count either side of the operator counts: `2d3w` is six words.
        let typed = match (self.ed.count.take(), operator.count) {
            (Some(count), one) => Some(count.saturating_mul(one).min(MAX_COUNT)),
            (None, one) if one > 1 => Some(one),
            (None, _) => None,
        };
        if key.glyph() == doubled_key(operator.kind) {
            let count = typed.unwrap_or(1);
            return self.operate_on_lines(Operator { count, ..operator });
        }
        let Some(motion) = bare_motion(key) else {
            return EditOutcome::Consumed;
        };
        self.operate_over(operator, motion, typed)
    }

    /// Act over what a motion covers from the cursor.
    pub(super) fn operate_over(
        &mut self,
        operator: Operator,
        motion: CursorMove,
        typed: Option<usize>,
    ) -> EditOutcome {
        let count = typed.unwrap_or(1);
        let mut probe = self.buffer().clone();
        move_cursor(
            &mut probe,
            motion,
            typed,
            self.view.viewport.max(1),
            *self.view.scroll,
        );
        let (linewise, mut inclusive) = motion_kind(motion);
        let mut to = (probe.row(), probe.col());
        // `cw` stops at the end of the word rather than the start of the next,
        // so that changing a word does not swallow the space after it.
        let word_motion = match motion {
            CursorMove::WordForward => Some(false),
            CursorMove::WordForwardBig => Some(true),
            _ => None,
        };
        if let (OperatorKind::Change, Some(big)) = (operator.kind, word_motion) {
            let mut word = self.buffer().clone();
            word.move_word_end(count, big);
            to = (word.row(), word.col());
            inclusive = true;
        } else if let Some(big) = word_motion {
            // Vim's other `w` rule: from the last word on a line an operator
            // reaches to the line's end, not into the next line, and not one
            // short of the last character.
            let here = (self.buffer().row(), self.buffer().col());
            let last_word =
                count == 1 && next_word_start(&self.buffer().chars(), here.1, big).is_none();
            if last_word || to.0 != here.0 {
                to = (here.0, self.buffer().line_len());
                inclusive = false;
            }
        }
        self.operate(
            operator,
            Reach {
                inclusive,
                linewise,
                to,
            },
        )
    }

    /// Act over everything between the cursor and where a motion or an object
    /// ended.
    pub(super) fn operate(&mut self, operator: Operator, reach: Reach) -> EditOutcome {
        self.buffer_mut().snapshot();
        let here = (self.buffer().row(), self.buffer().col());
        if reach.linewise {
            let (first, last) = order(here.0, reach.to.0);
            return self.operate_on_rows(operator, first, last);
        }
        let (start, end) = match here <= reach.to {
            true => (here, reach.to),
            false => (reach.to, here),
        };
        let end = match reach.inclusive {
            true => (end.0, end.1 + 1),
            false => end,
        };
        match operator.kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                let (first, last) = order(here.0, reach.to.0);
                self.operate_on_rows(operator, first, last)
            }
            OperatorKind::Yank => {
                self.buffer_mut().take_between(start, end, true);
                self.store_register(operator.register, true)
            }
            OperatorKind::Delete => {
                self.buffer_mut().take_between(start, end, false);
                self.store_register(operator.register, false)
            }
            OperatorKind::Change => {
                self.buffer_mut().take_between(start, end, false);
                let outcome = self.store_register(operator.register, false);
                self.ed.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// The doubled form: `dd`, `yy`, `cc`, `>>`, `<<`.
    pub(super) fn operate_on_lines(&mut self, operator: Operator) -> EditOutcome {
        self.buffer_mut().snapshot();
        let first = self.buffer().row();
        let last = (first + operator.count.max(1) - 1).min(self.buffer().lines().len() - 1);
        self.operate_on_rows(operator, first, last)
    }

    /// Act on whole rows, which is what every linewise operator comes down to.
    pub(super) fn operate_on_rows(
        &mut self,
        operator: Operator,
        first: usize,
        last: usize,
    ) -> EditOutcome {
        match operator.kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                self.buffer_mut().move_to(first, 0);
                self.buffer_mut()
                    .shift_lines(last - first + 1, operator.kind == OperatorKind::Indent);
                EditOutcome::Consumed
            }
            OperatorKind::Yank => {
                self.buffer_mut().take_rows(first, last, true);
                self.store_register(operator.register, true)
            }
            OperatorKind::Delete => {
                self.buffer_mut().take_rows(first, last, false);
                self.store_register(operator.register, false)
            }
            OperatorKind::Change => {
                self.buffer_mut().move_to(first, 0);
                self.buffer_mut().clear_lines(last - first + 1);
                let outcome = self.store_register(operator.register, false);
                self.ed.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// `C` and `D`: change or delete to the end of the line.
    pub(super) fn line_tail(&mut self, operator: Operator) -> EditOutcome {
        let to = (self.buffer().row(), self.buffer().line_len());
        self.operate(
            operator,
            Reach {
                inclusive: false,
                linewise: false,
                to,
            },
        )
    }

    /// Put what a yank or a delete took where the register prefix asked for,
    /// the way Vim files it. The clipboard registers leave the engine, so a
    /// yank there reaches every other window; the black hole keeps nothing,
    /// not even in the unnamed register; a capital appends to its register.
    /// With no prefix, a yank also goes to `0`, a delete of lines or across
    /// them to `1` (shifting the older ones up to `9`), a smaller one to `-`,
    /// and either to the kill ring, so `C-y` reaches it.
    pub(super) fn store_register(&mut self, register: Option<char>, yank: bool) -> EditOutcome {
        let taken = self.buffer().register().clone();
        match register {
            Some('_') => {
                if let Some(held) = self.ed.held.take() {
                    self.buffer_mut().set_register(held);
                }
            }
            Some('+') | Some('*') => return EditOutcome::Yank(taken.text),
            Some(c) if c.is_ascii_uppercase() => {
                let lower = c.to_ascii_lowercase();
                let joined = match self.ed.registers.remove(&lower) {
                    Some(old) if old.linewise || taken.linewise => Register {
                        linewise: true,
                        text: format!("{}\n{}", old.text, taken.text),
                    },
                    Some(old) => Register {
                        linewise: false,
                        text: format!("{}{}", old.text, taken.text),
                    },
                    None => taken,
                };
                self.ed.registers.insert(lower, joined);
            }
            Some(c) => {
                self.ed.registers.insert(c, taken);
            }
            None => {
                if yank {
                    self.ed.registers.insert('0', taken.clone());
                } else if taken.linewise || taken.text.contains('\n') {
                    for pair in NUMBERED_REGISTERS.windows(2).rev() {
                        if let Some(older) = self.ed.registers.remove(&pair[0]) {
                            self.ed.registers.insert(pair[1], older);
                        }
                    }
                    self.ed
                        .registers
                        .insert(NUMBERED_REGISTERS[0], taken.clone());
                } else {
                    self.ed.registers.insert('-', taken.clone());
                }
                self.ed.kills.kill(taken, Merge::New);
            }
        }
        EditOutcome::Consumed
    }

    /// Store what a bare `x` or `X` took, which has no operator to carry the
    /// register prefix for it.
    pub(super) fn store_take(&mut self) -> EditOutcome {
        let register = self.ed.register.take();
        self.store_register(register, false)
    }

    /// Put a register's text back (`p`/`P`), from the clipboard where the
    /// prefix named it, which the host has to read for us.
    pub(super) fn paste(&mut self, after: bool) -> EditOutcome {
        let count = self.take_count();
        match self.ed.register.take() {
            Some('+') | Some('*') => return EditOutcome::Paste,
            Some('_') => return EditOutcome::Consumed,
            Some(c) => match self.ed.registers.get(&c.to_ascii_lowercase()).cloned() {
                Some(register) => self.buffer_mut().set_register(register),
                None => {
                    self.ed.notice = Some(Notice::error(format!("register `{c}` is empty")));
                    return EditOutcome::Consumed;
                }
            },
            None => {}
        }
        self.buffer_mut().snapshot();
        for _ in 0..count {
            self.buffer_mut().paste(after);
        }
        EditOutcome::Consumed
    }

    /// Put text read out of the clipboard in at the cursor: after the
    /// character under it in Normal mode, and at the caret while typing, which
    /// it then steps past the way typing it would.
    pub(super) fn paste_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        // Text that ends in a newline is whole lines, however it was copied.
        let linewise = text.ends_with('\n');
        let kept = self.buffer().register().clone();
        self.buffer_mut().set_register(Register {
            linewise,
            text: text.trim_end_matches('\n').to_string(),
        });
        self.buffer_mut().snapshot();
        // Whole lines go in below the cursor's whichever the mode; text goes in
        // at the caret while typing, and after the character under it else.
        let at_caret = self.ed.is_typing() && (!linewise || self.buffer().is_fixed());
        self.buffer_mut().paste(!at_caret);
        if at_caret {
            self.buffer_mut().move_right(1, true);
        }
        self.buffer_mut().set_register(kept);
    }
}

// ========================================================================
// Session: text objects
// ========================================================================

impl Session<'_> {
    /// The object key after an `i` or an `a`: it either says how far a waiting
    /// operator reaches, or what a Visual selection now covers.
    pub(super) fn resolve_object(&mut self, wait: ObjectWait, key: &Key) -> EditOutcome {
        let operator = self.ed.operator.take();
        let Some(object) = TextObject::of_key(key.glyph()) else {
            return EditOutcome::Consumed;
        };
        let Some((span, linewise)) = self.object_span(object, wait.around) else {
            self.ed.notice = Some(Notice::error("no object here"));
            return EditOutcome::Consumed;
        };
        if let Some(operator) = operator {
            self.buffer_mut().move_to(span.start.0, span.start.1);
            return self.operate(
                operator,
                Reach {
                    inclusive: true,
                    linewise,
                    to: span.end,
                },
            );
        }
        // In Visual mode the object becomes the selection.
        self.ed.anchor = Some(Anchor {
            col: span.start.1,
            linewise: linewise || self.ed.anchor.is_some_and(|anchor| anchor.linewise),
            row: span.start.0,
        });
        self.buffer_mut().move_to(span.end.0, span.end.1);
        EditOutcome::Consumed
    }

    /// Where an object at the cursor starts and ends, and whether it covers
    /// whole lines.
    pub(super) fn object_span(&self, object: TextObject, around: bool) -> Option<(Span, bool)> {
        let row = self.buffer().row();
        let col = self.buffer().col();
        let line = self.buffer().chars();
        let over_line = |range: Option<(usize, usize)>| {
            range.map(|(start, end)| {
                (
                    Span {
                        end: (row, end),
                        start: (row, start),
                    },
                    false,
                )
            })
        };
        match object {
            TextObject::Word => over_line(word_object(&line, col, false, around)),
            TextObject::WordBig => over_line(word_object(&line, col, true, around)),
            TextObject::Quotes(quote) => over_line(quote_object(&line, col, quote, around)),
            TextObject::Sentence => over_line(sentence_object(&line, col, around)),
            TextObject::Brackets(open, close) => {
                bracket_object(self.buffer(), (row, col), open, close, around)
                    .map(|span| (span, false))
            }
            TextObject::Paragraph => paragraph_object(self.buffer(), row, around).map(|(a, b)| {
                (
                    Span {
                        end: (b, 0),
                        start: (a, 0),
                    },
                    true,
                )
            }),
        }
    }
}

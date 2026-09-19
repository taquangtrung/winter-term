//! What a key does to the file: the Vim grammar the editor answers to in its
//! four modes, and the operators that reach over a motion or a text object.

use crate::model::input::{Key, KeyCode, TextObject};
use crate::model::page::{PageOutcome, PickRequest, PromptMode, PromptRequest};
use crate::model::vim::motion::{CursorMove, FindChar};
use crate::model::vim::nav::{bare_motion, VimKey};
use crate::model::vim::objects::{
    bracket_object, paragraph_object, quote_object, sentence_object, word_object, Span,
};
use crate::model::vim::words::prev_word_start;

use super::buffer::Register;
use super::search::Search;
use super::{Anchor, EditMode, EditorPage, ASK_BUFFER, ASK_SEARCH, ASK_SEARCH_BACK, HALF_PAGE};

// ========================================================================
// Data Structures
// ========================================================================

/// A command part-way through being typed, waiting on the key that says what
/// it acts on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Pending {
    /// `f`/`F`/`t`/`T`, awaiting the character to look for.
    Find(FindWait),
    /// `[` or `]`, awaiting the key that says which list to step through.
    Bracket(BracketWait),
    /// `g`, awaiting the key that says which of its commands is meant.
    Goto,
    /// `'` or `` ` ``, awaiting the mark to go to.
    Jump(MarkJump),
    /// `Z`, awaiting the second key that says save or discard.
    Leave,
    /// `m`, awaiting the letter the mark is named by.
    Mark,
    /// An operator, awaiting the motion that says how far it reaches.
    Motion,
    /// An operator's `i` or `a`, awaiting the object it takes.
    Object(ObjectWait),
    /// `"`, awaiting the register the next command reads or writes.
    Register,
    /// `r`, awaiting the character to write.
    Replace,
    /// `z`, awaiting the key that says where the cursor's line goes.
    Scroll,
}

/// A char search waiting for its target.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct FindWait {
    /// True for `f`/`t`, false for `F`/`T`.
    pub forward: bool,
    /// True for `t`/`T`, which stop one short of the target.
    pub till: bool,
}

/// A `[` or `]` waiting for what it steps through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct BracketWait {
    /// True after `]`, which steps on rather than back.
    pub forward: bool,
}

/// A jump to a mark waiting for the letter naming it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MarkJump {
    /// True after `` ` ``, which restores the column as well as the line.
    pub exact: bool,
}

/// A text object waiting for the key that names it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ObjectWait {
    /// True after `a`, false after `i`.
    pub around: bool,
}

/// What an operator does to the text it reaches over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Operator {
    /// How many times over, from the count typed before it. One where none
    /// was typed, which is what tells `G` from `20G`.
    pub count: usize,
    /// What it does to the text it reaches over.
    pub kind: OperatorKind,
    /// The register it reads or writes, from a `"` prefix.
    pub register: Option<char>,
}

/// The operators the editor carries.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum OperatorKind {
    /// `c`: take the text and start typing in its place.
    Change,
    /// `d`: take the text out.
    Delete,
    /// `>`: push the lines it covers one level right.
    Indent,
    /// `<`: pull them one level back.
    Outdent,
    /// `y`: copy the text, leaving it where it is.
    Yank,
}

/// How far an operator reaches, and how the text there is taken.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Reach {
    /// Whether the cell landed on goes with it, as `e` and `$` do.
    inclusive: bool,
    /// Whether it takes whole lines, as `j` and `G` do.
    linewise: bool,
    /// Where the motion or object ended.
    to: (usize, usize),
}

/// The text a Visual selection covers, ordered from its first point.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Selection {
    /// The last point selected, taken with the selection.
    pub end: (usize, usize),
    /// Whether whole lines are selected.
    pub linewise: bool,
    /// The first point selected.
    pub start: (usize, usize),
}

// ========================================================================
// EditorPage: dispatch
// ========================================================================

impl EditorPage {
    /// One key, in whichever mode the editor is in, with what it changes kept
    /// for `.` to do again.
    pub(super) fn dispatch(&mut self, key: &Key) -> PageOutcome {
        if !self.replaying {
            self.record.push(key.clone());
        }
        let outcome = match self.mode {
            EditMode::Insert => self.on_insert_key(key),
            EditMode::Normal => self.on_normal_key(key),
            EditMode::Replace => self.on_replace_key(key),
            EditMode::Visual => self.on_visual_key(key),
        };
        self.close_command();
        outcome
    }

    /// A command is over once nothing is part-typed and the keys are commands
    /// again. What it changed, if anything, is what `.` now does again.
    fn close_command(&mut self) {
        if self.mode != EditMode::Normal || self.pending.is_some() || self.count.is_some() {
            return;
        }
        if self.buffer().revision() != self.revision {
            if !self.replaying {
                self.last_change = self.record.clone();
            }
            self.revision = self.buffer().revision();
        }
        self.record.clear();
    }

    /// Type the last command that changed the text again (`.`).
    fn repeat_change(&mut self) -> PageOutcome {
        if self.last_change.is_empty() {
            self.message = Some("nothing to repeat".to_string());
            return PageOutcome::Consumed;
        }
        let keys = self.last_change.clone();
        self.replaying = true;
        for key in &keys {
            self.dispatch(key);
        }
        self.replaying = false;
        self.revision = self.buffer().revision();
        PageOutcome::Consumed
    }
}

// ========================================================================
// EditorPage: Normal mode
// ========================================================================

impl EditorPage {
    /// Normal mode: motions, operators, and the commands that change text.
    fn on_normal_key(&mut self, key: &Key) -> PageOutcome {
        if let Some(pending) = self.pending {
            return self.resolve_pending(pending, key);
        }
        // An Alt chord belongs to the window: `Alt-p` moves focus rather than
        // pasting. Only the two buffer ends the shared layer claims stay here.
        if key.alt {
            return self.shared_motion(key);
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        if self.take_digit(key) {
            return PageOutcome::Consumed;
        }
        // A count or a register typed and then thought better of: `Esc` drops
        // it rather than closing a file over a half-typed command.
        if key.code == KeyCode::Escape && (self.count.is_some() || self.register.is_some()) {
            self.count = None;
            self.register = None;
            return PageOutcome::Consumed;
        }
        match key.code {
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
                let below = key.code == KeyCode::Char('o');
                self.buffer_mut().snapshot();
                self.buffer_mut().open_line(below);
                self.mode = EditMode::Insert;
                PageOutcome::Consumed
            }
            KeyCode::Char('R') => {
                self.buffer_mut().snapshot();
                self.mode = EditMode::Replace;
                PageOutcome::Consumed
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
                self.mode = EditMode::Insert;
                PageOutcome::Consumed
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
                self.pending = Some(Pending::Replace);
                PageOutcome::Consumed
            }
            KeyCode::Char('~') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().toggle_case(count);
                PageOutcome::Consumed
            }
            KeyCode::Char('J') => {
                let count = self.take_count();
                self.buffer_mut().snapshot();
                self.buffer_mut().join_lines(count);
                PageOutcome::Consumed
            }
            KeyCode::Char('p') | KeyCode::Char('P') => {
                let after = key.code == KeyCode::Char('p');
                self.paste(after)
            }
            KeyCode::Char('u') => {
                if !self.buffer_mut().undo() {
                    self.message = Some("nothing to undo".to_string());
                }
                // An undo is not a change to be done again: `.` still means
                // whatever changed the text last.
                self.revision = self.buffer().revision();
                PageOutcome::Consumed
            }
            KeyCode::Char('.') => self.repeat_change(),
            KeyCode::Char('d') => self.start_operator(OperatorKind::Delete),
            KeyCode::Char('c') => self.start_operator(OperatorKind::Change),
            KeyCode::Char('y') => self.start_operator(OperatorKind::Yank),
            KeyCode::Char('>') => self.start_operator(OperatorKind::Indent),
            KeyCode::Char('<') => self.start_operator(OperatorKind::Outdent),
            KeyCode::Char('"') => {
                self.pending = Some(Pending::Register);
                PageOutcome::Consumed
            }
            KeyCode::Char('m') => {
                self.pending = Some(Pending::Mark);
                PageOutcome::Consumed
            }
            KeyCode::Char('\'') => {
                self.pending = Some(Pending::Jump(MarkJump { exact: false }));
                PageOutcome::Consumed
            }
            KeyCode::Char('`') => {
                self.pending = Some(Pending::Jump(MarkJump { exact: true }));
                PageOutcome::Consumed
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.pending = Some(Pending::Find(find_wait(key)));
                PageOutcome::Consumed
            }
            KeyCode::Char(';') | KeyCode::Char(',') => {
                let reverse = key.code == KeyCode::Char(',');
                self.repeat_find(reverse)
            }
            KeyCode::Char('/') | KeyCode::Char('?') => {
                let forward = key.code == KeyCode::Char('/');
                PageOutcome::Prompt(PromptRequest {
                    initial: String::new(),
                    label: if forward {
                        "/".to_string()
                    } else {
                        "?".to_string()
                    },
                    mode: PromptMode::Text,
                    tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
                })
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let reverse = key.code == KeyCode::Char('N');
                self.search_step(reverse)
            }
            KeyCode::Char('*') | KeyCode::Char('#') => {
                let forward = key.code == KeyCode::Char('*');
                self.search_word(forward)
            }
            KeyCode::Char('g') => {
                self.pending = Some(Pending::Goto);
                PageOutcome::Consumed
            }
            KeyCode::Char('[') | KeyCode::Char(']') => {
                let forward = key.code == KeyCode::Char(']');
                self.pending = Some(Pending::Bracket(BracketWait { forward }));
                PageOutcome::Consumed
            }
            KeyCode::Char('z') => {
                self.pending = Some(Pending::Scroll);
                PageOutcome::Consumed
            }
            KeyCode::Char('Z') => {
                self.pending = Some(Pending::Leave);
                PageOutcome::Consumed
            }
            KeyCode::Char('q') | KeyCode::Escape => self.leave(),
            _ => self.shared_motion(key),
        }
    }

    /// The Ctrl chords: saving, redo, the number keys, and the paging motions.
    fn on_ctrl_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            KeyCode::Char('s') => self.save(),
            KeyCode::Char('r') => {
                if !self.buffer_mut().redo() {
                    self.message = Some("nothing to redo".to_string());
                }
                self.revision = self.buffer().revision();
                PageOutcome::Consumed
            }
            KeyCode::Char('a') | KeyCode::Char('x') => {
                let count = self.take_count() as i64;
                let by = match key.code == KeyCode::Char('a') {
                    true => count,
                    false => -count,
                };
                self.buffer_mut().snapshot();
                if !self.buffer_mut().add_to_number(by) {
                    self.message = Some("no number here".to_string());
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('o') => self.escalate(),
            _ => self.shared_motion(key),
        }
    }

    /// A key the editor does not claim, offered to the shared Vim layer so a
    /// motion means here what it means over every other surface.
    fn shared_motion(&mut self, key: &Key) -> PageOutcome {
        let typed = self.count.take();
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.jump_for(motion);
                self.apply_motion(motion, typed);
                PageOutcome::Consumed
            }
            VimKey::Pending => PageOutcome::Consumed,
            VimKey::Unhandled => PageOutcome::Ignored,
        }
    }

    /// A digit typed before a command, which belongs to the command that
    /// follows it. A leading zero is the motion to column one instead.
    fn take_digit(&mut self, key: &Key) -> bool {
        let KeyCode::Char(c) = key.code else {
            return false;
        };
        if !c.is_ascii_digit() || (c == '0' && self.count.is_none()) {
            return false;
        }
        let digit = c as usize - '0' as usize;
        self.count = Some(self.count.unwrap_or(0) * 10 + digit);
        true
    }
}

// ========================================================================
// EditorPage: Insert and Replace
// ========================================================================

impl EditorPage {
    /// Insert mode: an Emacs-style field, since that is what the shell's own
    /// line is, with the chords that edit one.
    fn on_insert_key(&mut self, key: &Key) -> PageOutcome {
        // Typing is no reason to lose the window chords: `Alt-h` moves focus,
        // it does not type an `h`.
        if key.alt {
            return PageOutcome::Ignored;
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('s') => self.save(),
                KeyCode::Char('a') => {
                    self.buffer_mut().move_line_start();
                    PageOutcome::Consumed
                }
                KeyCode::Char('e') => {
                    self.buffer_mut().move_line_end(true);
                    PageOutcome::Consumed
                }
                KeyCode::Char('b') => {
                    self.buffer_mut().move_left(1);
                    PageOutcome::Consumed
                }
                KeyCode::Char('f') => {
                    self.buffer_mut().move_right(1, true);
                    PageOutcome::Consumed
                }
                KeyCode::Char('k') => {
                    let end = self.buffer().line_len();
                    self.buffer_mut().take_to(end, false);
                    PageOutcome::Consumed
                }
                KeyCode::Char('u') => {
                    self.buffer_mut().take_to(0, false);
                    PageOutcome::Consumed
                }
                KeyCode::Char('w') => {
                    let chars = self.buffer().chars();
                    let to = prev_word_start(&chars, self.buffer().col(), false).unwrap_or(0);
                    self.buffer_mut().take_to(to, false);
                    PageOutcome::Consumed
                }
                _ => PageOutcome::Ignored,
            };
        }
        match key.code {
            KeyCode::Escape => self.leave_insert(),
            KeyCode::Enter => {
                self.buffer_mut().insert_newline();
                PageOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.buffer_mut().backspace();
                PageOutcome::Consumed
            }
            KeyCode::Delete => {
                self.buffer_mut().delete_under(1);
                PageOutcome::Consumed
            }
            KeyCode::Tab => {
                self.buffer_mut().insert_char('\t');
                PageOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer_mut().insert_char(c);
                PageOutcome::Consumed
            }
            KeyCode::Left => {
                self.buffer_mut().move_left(1);
                PageOutcome::Consumed
            }
            KeyCode::Right => {
                self.buffer_mut().move_right(1, true);
                PageOutcome::Consumed
            }
            KeyCode::Up => {
                self.buffer_mut().move_up(1, true);
                PageOutcome::Consumed
            }
            KeyCode::Down => {
                self.buffer_mut().move_down(1, true);
                PageOutcome::Consumed
            }
            KeyCode::Home => {
                self.buffer_mut().move_line_start();
                PageOutcome::Consumed
            }
            KeyCode::End => {
                self.buffer_mut().move_line_end(true);
                PageOutcome::Consumed
            }
            _ => PageOutcome::Ignored,
        }
    }

    /// Replace mode (`R`): every character typed writes over the one under
    /// the cursor rather than pushing it along.
    fn on_replace_key(&mut self, key: &Key) -> PageOutcome {
        if key.alt || key.ctrl {
            return PageOutcome::Ignored;
        }
        match key.code {
            KeyCode::Escape => self.leave_insert(),
            KeyCode::Enter => {
                self.buffer_mut().insert_newline();
                PageOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.buffer_mut().move_left(1);
                PageOutcome::Consumed
            }
            KeyCode::Char(c) => {
                self.buffer_mut().replace_char_over(c);
                PageOutcome::Consumed
            }
            _ => self.on_insert_key(key),
        }
    }

    /// Back to Normal, with the cursor stepped onto the character it was
    /// typing past, which is where Vim leaves it.
    fn leave_insert(&mut self) -> PageOutcome {
        self.mode = EditMode::Normal;
        self.buffer_mut().move_left(1);
        self.buffer_mut().clamp(false);
        PageOutcome::Consumed
    }

    /// Start typing, with an undo step open so the whole run walks back at
    /// once rather than a character at a time.
    fn enter_insert(&mut self) -> PageOutcome {
        self.buffer_mut().snapshot();
        self.mode = EditMode::Insert;
        PageOutcome::Consumed
    }
}

// ========================================================================
// EditorPage: Visual mode
// ========================================================================

impl EditorPage {
    /// Start selecting from where the cursor is (`v`/`V`).
    pub(super) fn start_visual(&mut self, linewise: bool) -> PageOutcome {
        self.anchor = Some(Anchor {
            col: self.buffer().col(),
            linewise,
            row: self.buffer().row(),
        });
        self.mode = EditMode::Visual;
        PageOutcome::Consumed
    }

    /// Stop selecting, leaving the cursor where it is.
    fn leave_visual(&mut self) -> PageOutcome {
        self.anchor = None;
        self.mode = EditMode::Normal;
        self.buffer_mut().clamp(false);
        PageOutcome::Consumed
    }

    /// The text the selection covers, ordered from its first point.
    pub(super) fn selection(&self) -> Option<Selection> {
        let anchor = self.anchor?;
        let here = (self.buffer().row(), self.buffer().col());
        let there = (anchor.row, anchor.col);
        let (start, end) = match here < there {
            true => (here, there),
            false => (there, here),
        };
        Some(Selection {
            end,
            linewise: anchor.linewise,
            start,
        })
    }

    /// Visual mode: the motions move one end of the selection, and an
    /// operator acts on everything between the two.
    fn on_visual_key(&mut self, key: &Key) -> PageOutcome {
        if let Some(pending) = self.pending {
            return self.resolve_pending(pending, key);
        }
        if key.alt {
            return self.shared_motion(key);
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        if self.take_digit(key) {
            return PageOutcome::Consumed;
        }
        let Some(selection) = self.selection() else {
            return self.leave_visual();
        };
        match key.code {
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
                let anchor = self.anchor.take();
                if let Some(anchor) = anchor {
                    self.anchor = Some(Anchor {
                        col: self.buffer().col(),
                        linewise: anchor.linewise,
                        row: self.buffer().row(),
                    });
                    self.buffer_mut().move_to(anchor.row, anchor.col);
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('d') | KeyCode::Char('x') => {
                self.operate_on_selection(selection, OperatorKind::Delete)
            }
            KeyCode::Char('c') | KeyCode::Char('s') => {
                self.operate_on_selection(selection, OperatorKind::Change)
            }
            KeyCode::Char('y') => self.operate_on_selection(selection, OperatorKind::Yank),
            KeyCode::Char('>') => self.operate_on_selection(selection, OperatorKind::Indent),
            KeyCode::Char('<') => self.operate_on_selection(selection, OperatorKind::Outdent),
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
                self.pending = Some(Pending::Replace);
                PageOutcome::Consumed
            }
            KeyCode::Char('i') | KeyCode::Char('a') => {
                let around = key.code == KeyCode::Char('a');
                self.pending = Some(Pending::Object(ObjectWait { around }));
                PageOutcome::Consumed
            }
            KeyCode::Char('"') => {
                self.pending = Some(Pending::Register);
                PageOutcome::Consumed
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.pending = Some(Pending::Find(find_wait(key)));
                PageOutcome::Consumed
            }
            KeyCode::Char('g') => {
                self.pending = Some(Pending::Goto);
                PageOutcome::Consumed
            }
            KeyCode::Char('n') | KeyCode::Char('N') => {
                let reverse = key.code == KeyCode::Char('N');
                self.search_step(reverse)
            }
            _ => self.shared_motion(key),
        }
    }

    /// Act on what is selected, and stop selecting: an operator is the end of
    /// a Visual run whichever one it is.
    fn operate_on_selection(&mut self, selection: Selection, kind: OperatorKind) -> PageOutcome {
        let register = self.register.take();
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
                let outcome = self.store_register(register);
                self.leave_visual();
                outcome
            }
            OperatorKind::Delete => {
                self.cut_selection(selection, false);
                let outcome = self.store_register(register);
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
                let outcome = self.store_register(register);
                self.anchor = None;
                self.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// Take the selected text into the register, removing it unless `keep`.
    fn cut_selection(&mut self, selection: Selection, keep: bool) {
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
    fn flip_selection(&mut self, selection: Selection) {
        for row in selection.start.0..=selection.end.0 {
            let len = self.buffer().line_len_of(row);
            let from = match row == selection.start.0 && !selection.linewise {
                true => selection.start.1,
                false => 0,
            };
            let to = match row == selection.end.0 && !selection.linewise {
                true => (selection.end.1 + 1).min(len),
                false => len,
            };
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
    fn fill_selection(&mut self, selection: Selection, c: char) {
        for row in selection.start.0..=selection.end.0 {
            let len = self.buffer().line_len_of(row);
            let from = match row == selection.start.0 && !selection.linewise {
                true => selection.start.1,
                false => 0,
            };
            let to = match row == selection.end.0 && !selection.linewise {
                true => (selection.end.1 + 1).min(len),
                false => len,
            };
            for col in from..to {
                self.buffer_mut().move_to(row, col);
                self.buffer_mut().replace_char(c);
            }
        }
        self.buffer_mut()
            .move_to(selection.start.0, selection.start.1);
    }
}

// ========================================================================
// EditorPage: pending commands
// ========================================================================

impl EditorPage {
    /// The second key of a command: the character `r` writes, the motion an
    /// operator reaches over, the object it takes, or the letter naming a
    /// register or a mark.
    fn resolve_pending(&mut self, pending: Pending, key: &Key) -> PageOutcome {
        self.pending = None;
        if key.code == KeyCode::Escape {
            self.operator = None;
            self.count = None;
            return PageOutcome::Consumed;
        }
        match pending {
            Pending::Bracket(wait) => {
                // `]b` and `[b` step through the open buffers, the way they
                // step through the blocks of a shell's output.
                if key.code == KeyCode::Char('b') {
                    return self.step_buffer(wait.forward);
                }
                PageOutcome::Consumed
            }
            Pending::Find(wait) => self.resolve_find(wait, key),
            Pending::Goto => self.resolve_goto(key),
            Pending::Jump(jump) => self.jump_to_mark(jump, key),
            Pending::Leave => match key.code {
                KeyCode::Char('Z') => {
                    self.write();
                    PageOutcome::Close
                }
                KeyCode::Char('Q') => PageOutcome::Close,
                _ => PageOutcome::Consumed,
            },
            Pending::Mark => {
                if let KeyCode::Char(c) = key.code {
                    let at = (self.buffer().row(), self.buffer().col());
                    self.doc_mut().marks.insert(c, at);
                }
                PageOutcome::Consumed
            }
            Pending::Motion => self.resolve_motion(key),
            Pending::Object(wait) => self.resolve_object(wait, key),
            Pending::Register => {
                if let KeyCode::Char(c) = key.code {
                    self.register = Some(c);
                }
                PageOutcome::Consumed
            }
            Pending::Replace => self.resolve_replace(key),
            Pending::Scroll => {
                let at = match key.code {
                    KeyCode::Char('z') => self.viewport / HALF_PAGE,
                    KeyCode::Char('t') => 0,
                    KeyCode::Char('b') => self.viewport.saturating_sub(1),
                    _ => return PageOutcome::Consumed,
                };
                self.doc_mut().scroll = self.buffer().row().saturating_sub(at);
                PageOutcome::Consumed
            }
        }
    }

    /// `r`: write one character over the one under the cursor, or over every
    /// character of a selection.
    fn resolve_replace(&mut self, key: &Key) -> PageOutcome {
        let KeyCode::Char(c) = key.code else {
            return PageOutcome::Consumed;
        };
        self.buffer_mut().snapshot();
        match self.selection() {
            Some(selection) => {
                self.fill_selection(selection, c);
                self.leave_visual()
            }
            None => {
                self.buffer_mut().replace_char(c);
                PageOutcome::Consumed
            }
        }
    }

    /// The `g` commands the editor carries: `gg`, `ge`, `gE`, `g_`, and the
    /// list of open buffers.
    fn resolve_goto(&mut self, key: &Key) -> PageOutcome {
        if key.code == KeyCode::Char('b') {
            self.operator = None;
            return self.list_buffers();
        }
        let motion = match key.code {
            KeyCode::Char('g') => CursorMove::Top,
            KeyCode::Char('e') => CursorMove::WordEndBack,
            KeyCode::Char('E') => CursorMove::WordEndBackBig,
            KeyCode::Char('_') => CursorMove::LastNonBlank,
            _ => {
                self.operator = None;
                return PageOutcome::Consumed;
            }
        };
        let typed = self.count.take();
        self.jump_for(motion);
        match self.operator.take() {
            Some(operator) => self.operate_over(operator, motion, typed),
            None => {
                self.apply_motion(motion, typed);
                PageOutcome::Consumed
            }
        }
    }

    /// The character a char search was waiting for, which now stands as the
    /// search `;` and `,` repeat.
    fn resolve_find(&mut self, wait: FindWait, key: &Key) -> PageOutcome {
        let KeyCode::Char(ch) = key.code else {
            self.operator = None;
            return PageOutcome::Consumed;
        };
        let find = FindChar {
            ch,
            forward: wait.forward,
            till: wait.till,
        };
        self.find = Some(find);
        self.run_find(find)
    }

    /// The last char search again (`;`), or the other way (`,`).
    fn repeat_find(&mut self, reverse: bool) -> PageOutcome {
        let Some(find) = self.find else {
            return PageOutcome::Consumed;
        };
        let find = match reverse {
            true => find.reversed(),
            false => find,
        };
        self.run_find(find)
    }

    /// Move to where a char search lands, or act over it where an operator is
    /// waiting on one.
    fn run_find(&mut self, find: FindChar) -> PageOutcome {
        let count = self.take_count();
        let operator = self.operator.take();
        let mut probe = self.buffer().clone();
        if !probe.move_to_char(find, count) {
            self.message = Some(format!("no `{}` on this line", find.ch));
            return PageOutcome::Consumed;
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
                PageOutcome::Consumed
            }
        }
    }

    /// Go to a mark (`'a`, `` `a ``), or back to where the last jump started
    /// (`''`).
    fn jump_to_mark(&mut self, jump: MarkJump, key: &Key) -> PageOutcome {
        let KeyCode::Char(c) = key.code else {
            return PageOutcome::Consumed;
        };
        let at = match c {
            '\'' | '`' => self.doc().jump,
            _ => self.doc().marks.get(&c).copied(),
        };
        let Some((row, col)) = at else {
            self.message = Some(format!("mark `{c}` is not set"));
            return PageOutcome::Consumed;
        };
        self.push_jump();
        match jump.exact {
            true => self.buffer_mut().move_to(row, col),
            false => self.buffer_mut().move_to_line(row),
        }
        PageOutcome::Consumed
    }
}

// ========================================================================
// EditorPage: operators
// ========================================================================

impl EditorPage {
    /// An operator with the count and register typed before it.
    fn operator_of(&mut self, kind: OperatorKind) -> Operator {
        Operator {
            count: self.take_count(),
            kind,
            register: self.register.take(),
        }
    }

    /// Open an operator, which now waits for the motion or object saying how
    /// far it reaches.
    fn start_operator(&mut self, kind: OperatorKind) -> PageOutcome {
        self.operator = Some(self.operator_of(kind));
        self.pending = Some(Pending::Motion);
        PageOutcome::Consumed
    }

    /// The key after an operator: its own key doubles it over whole lines, an
    /// `i` or `a` opens a text object, and anything else is its motion.
    fn resolve_motion(&mut self, key: &Key) -> PageOutcome {
        let Some(operator) = self.operator else {
            return PageOutcome::Consumed;
        };
        if self.take_digit(key) {
            self.pending = Some(Pending::Motion);
            return PageOutcome::Consumed;
        }
        match key.code {
            KeyCode::Char('i') | KeyCode::Char('a') => {
                let around = key.code == KeyCode::Char('a');
                self.pending = Some(Pending::Object(ObjectWait { around }));
                return PageOutcome::Consumed;
            }
            KeyCode::Char('f') | KeyCode::Char('F') | KeyCode::Char('t') | KeyCode::Char('T') => {
                self.pending = Some(Pending::Find(find_wait(key)));
                return PageOutcome::Consumed;
            }
            KeyCode::Char('g') => {
                self.pending = Some(Pending::Goto);
                return PageOutcome::Consumed;
            }
            _ => {}
        }
        self.operator = None;
        // A count either side of the operator counts: `2d3w` is six words.
        let typed = match (self.count.take(), operator.count) {
            (Some(count), one) => Some(count * one),
            (None, one) if one > 1 => Some(one),
            (None, _) => None,
        };
        if key.code == doubled_key(operator.kind) {
            let count = typed.unwrap_or(1);
            return self.operate_on_lines(Operator { count, ..operator });
        }
        let Some(motion) = bare_motion(key) else {
            return PageOutcome::Consumed;
        };
        self.operate_over(operator, motion, typed)
    }

    /// Act over what a motion covers from the cursor.
    fn operate_over(
        &mut self,
        operator: Operator,
        motion: CursorMove,
        typed: Option<usize>,
    ) -> PageOutcome {
        let count = typed.unwrap_or(1);
        let mut probe = self.buffer().clone();
        super::move_cursor(
            &mut probe,
            motion,
            typed,
            self.viewport.max(1),
            self.doc().scroll,
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
    fn operate(&mut self, operator: Operator, reach: Reach) -> PageOutcome {
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
                self.store_register(operator.register)
            }
            OperatorKind::Delete => {
                self.buffer_mut().take_between(start, end, false);
                self.store_register(operator.register)
            }
            OperatorKind::Change => {
                self.buffer_mut().take_between(start, end, false);
                let outcome = self.store_register(operator.register);
                self.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// The doubled form: `dd`, `yy`, `cc`, `>>`, `<<`.
    fn operate_on_lines(&mut self, operator: Operator) -> PageOutcome {
        self.buffer_mut().snapshot();
        let first = self.buffer().row();
        let last = (first + operator.count.max(1) - 1).min(self.buffer().lines().len() - 1);
        self.operate_on_rows(operator, first, last)
    }

    /// Act on whole rows, which is what every linewise operator comes down to.
    fn operate_on_rows(&mut self, operator: Operator, first: usize, last: usize) -> PageOutcome {
        match operator.kind {
            OperatorKind::Indent | OperatorKind::Outdent => {
                self.buffer_mut().move_to(first, 0);
                self.buffer_mut()
                    .shift_lines(last - first + 1, operator.kind == OperatorKind::Indent);
                PageOutcome::Consumed
            }
            OperatorKind::Yank => {
                self.buffer_mut().take_rows(first, last, true);
                self.store_register(operator.register)
            }
            OperatorKind::Delete => {
                self.buffer_mut().take_rows(first, last, false);
                self.store_register(operator.register)
            }
            OperatorKind::Change => {
                self.buffer_mut().move_to(first, 0);
                self.buffer_mut().clear_lines(last - first + 1);
                let outcome = self.store_register(operator.register);
                self.mode = EditMode::Insert;
                outcome
            }
        }
    }

    /// `C` and `D`: change or delete to the end of the line.
    fn line_tail(&mut self, operator: Operator) -> PageOutcome {
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

    /// Put what was taken where the register prefix asked for. The clipboard
    /// registers leave the page, so a yank there reaches every other window.
    fn store_register(&mut self, register: Option<char>) -> PageOutcome {
        let taken = self.buffer().register().clone();
        match register {
            None => PageOutcome::Consumed,
            Some('+') | Some('*') => PageOutcome::Yank(taken.text),
            Some(c) => {
                self.registers.insert(c, taken);
                PageOutcome::Consumed
            }
        }
    }

    /// Store what a bare `x` or `X` took, which has no operator to carry the
    /// register prefix for it.
    fn store_take(&mut self) -> PageOutcome {
        let register = self.register.take();
        self.store_register(register)
    }

    /// Put a register's text back (`p`/`P`), from the clipboard where the
    /// prefix named it, which the host has to read for us.
    fn paste(&mut self, after: bool) -> PageOutcome {
        let count = self.take_count();
        match self.register.take() {
            Some('+') | Some('*') => return PageOutcome::Paste,
            Some(c) => match self.registers.get(&c).cloned() {
                Some(register) => self.buffer_mut().set_register(register),
                None => {
                    self.message = Some(format!("register `{c}` is empty"));
                    return PageOutcome::Consumed;
                }
            },
            None => {}
        }
        self.buffer_mut().snapshot();
        for _ in 0..count {
            self.buffer_mut().paste(after);
        }
        PageOutcome::Consumed
    }

    /// Put text the host read out of the clipboard in at the cursor.
    pub(super) fn paste_text(&mut self, text: String) -> PageOutcome {
        if text.is_empty() {
            return PageOutcome::Consumed;
        }
        // Text that ends in a newline is whole lines, however it was copied.
        let linewise = text.ends_with('\n');
        let kept = self.buffer().register().clone();
        self.buffer_mut().set_register(Register {
            linewise,
            text: text.trim_end_matches('\n').to_string(),
        });
        self.buffer_mut().snapshot();
        self.buffer_mut().paste(true);
        self.buffer_mut().set_register(kept);
        PageOutcome::Consumed
    }
}

// ========================================================================
// EditorPage: text objects
// ========================================================================

impl EditorPage {
    /// The object key after an `i` or an `a`: it either says how far a waiting
    /// operator reaches, or what a Visual selection now covers.
    fn resolve_object(&mut self, wait: ObjectWait, key: &Key) -> PageOutcome {
        let operator = self.operator.take();
        let Some(object) = TextObject::of_key(key.code) else {
            return PageOutcome::Consumed;
        };
        let Some((span, linewise)) = self.object_span(object, wait.around) else {
            self.message = Some("no object here".to_string());
            return PageOutcome::Consumed;
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
        self.anchor = Some(Anchor {
            col: span.start.1,
            linewise: linewise || self.anchor.is_some_and(|anchor| anchor.linewise),
            row: span.start.0,
        });
        self.buffer_mut().move_to(span.end.0, span.end.1);
        PageOutcome::Consumed
    }

    /// Where an object at the cursor starts and ends, and whether it covers
    /// whole lines.
    fn object_span(&self, object: TextObject, around: bool) -> Option<(Span, bool)> {
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

// ========================================================================
// EditorPage: buffers
// ========================================================================

impl EditorPage {
    /// On to the next open buffer, or back to the one before (`]b`/`[b`).
    fn step_buffer(&mut self, forward: bool) -> PageOutcome {
        let count = self.buffer_count();
        if count < 2 {
            self.message = Some("only this file is open".to_string());
            return PageOutcome::Consumed;
        }
        let at = self.buffer_at();
        let next = match forward {
            true => (at + 1) % count,
            false => (at + count - 1) % count,
        };
        self.show_buffer(next);
        PageOutcome::Consumed
    }

    /// The open buffers, as a list to choose from (`gb`).
    fn list_buffers(&mut self) -> PageOutcome {
        PageOutcome::Pick(PickRequest {
            items: self.buffer_list(),
            label: "Open buffers".to_string(),
            tag: ASK_BUFFER,
        })
    }
}

// ========================================================================
// EditorPage: search
// ========================================================================

impl EditorPage {
    /// Look for `pattern` from the cursor, and stay where it was found.
    pub(super) fn start_search(&mut self, pattern: String, forward: bool) -> PageOutcome {
        if pattern.is_empty() {
            return PageOutcome::Consumed;
        }
        self.search = Some(Search::new(pattern, forward));
        self.search_step(false)
    }

    /// The next match of the last search (`n`), or the one the other way
    /// (`N`).
    fn search_step(&mut self, reverse: bool) -> PageOutcome {
        let Some(search) = self.search.clone() else {
            self.message = Some("nothing has been searched for".to_string());
            return PageOutcome::Consumed;
        };
        let forward = search.forward != reverse;
        let at = (self.buffer().row(), self.buffer().col());
        match search.next(self.buffer().lines(), at, forward) {
            Some((row, col)) => {
                self.push_jump();
                self.buffer_mut().move_to(row, col);
            }
            None => self.message = Some(format!("`{}` is not in the file", search.pattern)),
        }
        PageOutcome::Consumed
    }

    /// Look for the word under the cursor (`*`/`#`).
    fn search_word(&mut self, forward: bool) -> PageOutcome {
        let line = self.buffer().chars();
        let Some((start, end)) = word_object(&line, self.buffer().col(), false, false) else {
            return PageOutcome::Consumed;
        };
        let word: String = line[start..=end].iter().collect();
        if word.trim().is_empty() {
            return PageOutcome::Consumed;
        }
        self.search = Some(Search::new(word, forward));
        self.search_step(false)
    }
}

// ========================================================================
// EditorPage: motions
// ========================================================================

impl EditorPage {
    /// Move the cursor as `motion` says, in the terms a file gives them.
    pub(super) fn apply_motion(&mut self, motion: CursorMove, typed: Option<usize>) {
        match motion {
            // The scroll placements move the window rather than the cursor,
            // so they are the page's to answer, not the text's.
            CursorMove::LineToTop => {
                let row = self.buffer().row();
                self.doc_mut().scroll = row;
            }
            CursorMove::LineToCenter => {
                let row = self
                    .buffer()
                    .row()
                    .saturating_sub(self.viewport / HALF_PAGE);
                self.doc_mut().scroll = row;
            }
            CursorMove::LineToBottom => {
                let row = self
                    .buffer()
                    .row()
                    .saturating_sub(self.viewport.saturating_sub(1));
                self.doc_mut().scroll = row;
            }
            _ => {
                let (page, scroll) = (self.viewport.max(1), self.doc().scroll);
                super::move_cursor(self.buffer_mut(), motion, typed, page, scroll);
            }
        }
    }

    /// Remember where a jump started, so `''` can go back to it.
    pub(super) fn push_jump(&mut self) {
        self.doc_mut().jump = Some((self.buffer().row(), self.buffer().col()));
    }

    /// Remember the cursor ahead of the motions that carry the eye a long way.
    fn jump_for(&mut self, motion: CursorMove) {
        let jumps = matches!(
            motion,
            CursorMove::Bottom
                | CursorMove::MatchingBracket
                | CursorMove::PageDown
                | CursorMove::PageUp
                | CursorMove::ParagraphBack
                | CursorMove::ParagraphForward
                | CursorMove::Top
        );
        if jumps {
            self.push_jump();
        }
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// The direction and the stopping rule a char search key names.
fn find_wait(key: &Key) -> FindWait {
    FindWait {
        forward: matches!(key.code, KeyCode::Char('f') | KeyCode::Char('t')),
        till: matches!(key.code, KeyCode::Char('t') | KeyCode::Char('T')),
    }
}

/// What an operator is called where the key hints name it.
pub(super) fn name_of_kind(operator: Operator) -> &'static str {
    match operator.kind {
        OperatorKind::Change => "change",
        OperatorKind::Delete => "delete",
        OperatorKind::Indent => "indent",
        OperatorKind::Outdent => "outdent",
        OperatorKind::Yank => "yank",
    }
}

/// The key that doubles an operator over whole lines.
fn doubled_key(kind: OperatorKind) -> KeyCode {
    match kind {
        OperatorKind::Change => KeyCode::Char('c'),
        OperatorKind::Delete => KeyCode::Char('d'),
        OperatorKind::Indent => KeyCode::Char('>'),
        OperatorKind::Outdent => KeyCode::Char('<'),
        OperatorKind::Yank => KeyCode::Char('y'),
    }
}

/// Whether a motion takes whole lines, and whether it takes the cell it lands
/// on with it. Vim's two classifications, which are what tell `dj` from `dw`
/// and `de` from `db`.
fn motion_kind(motion: CursorMove) -> (bool, bool) {
    use CursorMove as M;
    match motion {
        M::Bottom
        | M::Down
        | M::HalfPageDown
        | M::HalfPageUp
        | M::PageDown
        | M::PageUp
        | M::ScreenBottom
        | M::ScreenMiddle
        | M::ScreenTop
        | M::Top
        | M::Up => (true, false),
        M::LineEnd | M::LastNonBlank | M::MatchingBracket | M::WordEnd | M::WordEndBig => {
            (false, true)
        }
        M::FirstNonBlank
        | M::Left
        | M::LineStart
        | M::LineToBottom
        | M::LineToCenter
        | M::LineToTop
        | M::ParagraphBack
        | M::ParagraphForward
        | M::Right
        | M::WordBack
        | M::WordBackBig
        | M::WordEndBack
        | M::WordEndBackBig
        | M::WordForward
        | M::WordForwardBig => (false, false),
    }
}

/// Two rows in the order they come in the file.
fn order(one: usize, other: usize) -> (usize, usize) {
    match one <= other {
        true => (one, other),
        false => (other, one),
    }
}

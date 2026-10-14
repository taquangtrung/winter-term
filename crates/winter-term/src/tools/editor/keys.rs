//! What a key does in the editor: the chords that belong to the page rather
//! than to the text (stepping between buffers, finishing a commit message),
//! then the modal keys every text shares, and what the page does with what
//! those ask of it.

use crate::model::input::{Key, KeyCode};
use crate::model::page::{PageOutcome, PickRequest, PromptMode, PromptRequest};
use crate::model::textedit::modal::EditOutcome;

use super::comment::{self, Marker};
use super::commit;
use super::rebase;
use super::{EditorPage, ASK_BUFFER, ASK_SEARCH, ASK_SEARCH_BACK};

// ========================================================================
// EditorPage: dispatch
// ========================================================================

impl EditorPage {
    /// One key: the page's own chords first, then the modal keys.
    pub(super) fn dispatch(&mut self, key: &Key) -> PageOutcome {
        // Stepping between buffers, and recalling a commit message, are no part
        // of a text command, so they are taken ahead of the modal keys: the
        // chord means the same whether a file is being read or typed into.
        if key.ctrl && key.alt {
            match key.code {
                KeyCode::Char(']') => {
                    self.recent_buffer(true);
                    return PageOutcome::Consumed;
                }
                KeyCode::Char('[') => {
                    self.recent_buffer(false);
                    return PageOutcome::Consumed;
                }
                KeyCode::Char('p') => {
                    self.recall_message(true);
                    return PageOutcome::Consumed;
                }
                KeyCode::Char('n') => {
                    self.recall_message(false);
                    return PageOutcome::Consumed;
                }
                _ => {}
            }
        }
        // Commenting is the same chord in every mode: the line, or the lines
        // a selection covers.
        if key.ctrl && !key.alt && key.code == KeyCode::Char('/') {
            return self.toggle_comment();
        }
        if let Some(outcome) = self.message_chord(key) {
            return outcome;
        }
        if let Some(outcome) = self.todo_chord(key) {
            return outcome;
        }
        let outcome = self.with_modal(|modal, view| modal.on_key(view, key));
        self.carry_out(outcome)
    }

    /// `Ctrl-Shift-C`, or `C-c C-c`, and `C-c C-k` in a commit message: save
    /// and close to commit, or empty it and close to cancel. `None` for every
    /// other key, which is left to the mode, and a `Ctrl-C` not followed by
    /// either is forgotten.
    fn message_chord(&mut self, key: &Key) -> Option<PageOutcome> {
        if !commit::is_message_file(&self.doc().path) {
            return None;
        }
        // Shift turns the letter into the capital, as it does for the Git
        // page's own `Ctrl-Shift-C`.
        let commit_chord = matches!(key.code, KeyCode::Char('C' | 'c'));
        if key.ctrl && key.shift && !key.alt && commit_chord {
            self.message_chord = false;
            self.write();
            return Some(PageOutcome::Close);
        }
        let started = std::mem::take(&mut self.message_chord);
        match (started, key.ctrl, key.code) {
            (false, true, KeyCode::Char('c')) => {
                self.message_chord = true;
                Some(PageOutcome::Consumed)
            }
            (true, true, KeyCode::Char('c')) => {
                self.write();
                Some(PageOutcome::Close)
            }
            (true, true, KeyCode::Char('k')) => self.cancel_message(),
            _ => None,
        }
    }

    /// The `C-c` chords of a rebase plan: a letter sets the action of the
    /// line under the cursor and steps on to the next, `j` and `k` move the
    /// line down or up, `C-c` finishes and `C-k` aborts the rebase. `None` for
    /// every other key, as for a commit message.
    fn todo_chord(&mut self, key: &Key) -> Option<PageOutcome> {
        if !rebase::is_rebase_todo(&self.doc().path) {
            return None;
        }
        let started = std::mem::take(&mut self.message_chord);
        if !started {
            if key.ctrl && key.code == KeyCode::Char('c') {
                self.message_chord = true;
                return Some(PageOutcome::Consumed);
            }
            return None;
        }
        let row = self.buffer().row();
        match (key.ctrl, key.code) {
            (true, KeyCode::Char('c')) => return Some(self.finish_todo()),
            (true, KeyCode::Char('k')) => {
                // An empty plan is how git is told to abort: it stops at
                // "nothing to do" and leaves the branch as it was.
                let rows = self.buffer().lines().len();
                self.buffer_mut().replace_rows(0, rows, vec![String::new()]);
                self.write();
                return Some(PageOutcome::Close);
            }
            (false, KeyCode::Char('j')) => self.move_todo_line(row, row + 1),
            (false, KeyCode::Char('k')) if row > 0 => self.move_todo_line(row, row - 1),
            (false, KeyCode::Char(c)) => {
                let line = self.buffer().lines()[row].clone();
                match rebase::action_of(c).and_then(|action| rebase::with_action(&line, action)) {
                    Some(changed) => {
                        self.buffer_mut().replace_rows(row, 1, vec![changed]);
                        self.buffer_mut().move_to_line(row + 1);
                    }
                    None => self.modal.set_message("not a commit line".to_string()),
                }
            }
            _ => {}
        }
        Some(PageOutcome::Consumed)
    }

    /// Swap plan lines `from` and `to`, the cursor going with the moved one.
    fn move_todo_line(&mut self, from: usize, to: usize) {
        let lines = self.buffer().lines();
        if to >= lines.len() {
            return;
        }
        let first = from.min(to);
        let swapped = vec![lines[first + 1].clone(), lines[first].clone()];
        self.buffer_mut().replace_rows(first, 2, swapped);
        self.buffer_mut().move_to_line(to);
    }

    /// Write the plan and close, so git carries it out, unless git would
    /// refuse it, which is said instead.
    fn finish_todo(&mut self) -> PageOutcome {
        if let Some(problem) = rebase::problem(self.buffer().lines()) {
            self.modal.set_message(problem);
            return PageOutcome::Consumed;
        }
        self.write();
        PageOutcome::Close
    }

    /// Comment out the cursor's line, or every line a selection covers, or
    /// comment them back in, with the file's own comment marker (`Ctrl-/`).
    /// The cursor stays on the character it was on.
    fn toggle_comment(&mut self) -> PageOutcome {
        let marker = self.doc().syntax.and_then(|syntax| {
            match (syntax.line_comment(), syntax.block()) {
                (Some(open), _) => Some(Marker::Line(open)),
                (None, Some((open, close))) => Some(Marker::Block(open, close)),
                (None, None) => None,
            }
        });
        let Some(marker) = marker else {
            self.modal
                .set_message("no comment marker known for this file".to_string());
            return PageOutcome::Consumed;
        };
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (row, row),
        };
        let before = self.buffer().lines()[first..=last].to_vec();
        let after = comment::toggle(&before, marker);
        if after == before {
            return PageOutcome::Consumed;
        }
        let shift = after[row - first].chars().count() as isize
            - before[row - first].chars().count() as isize;
        let col = (col as isize + shift).max(0) as usize;
        self.buffer_mut().replace_rows(first, last - first + 1, after);
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(row, col),
            false => self.buffer_mut().move_to(row, col),
        }
        self.modal.clear_selection();
        PageOutcome::Consumed
    }

    /// Do what a modal key asked of the page.
    fn carry_out(&mut self, outcome: EditOutcome) -> PageOutcome {
        match outcome {
            // A file is never a field that is only typed into, and `Enter`
            // breaks its lines, so these two do not reach a page.
            EditOutcome::Cancel | EditOutcome::Submit => PageOutcome::Consumed,
            EditOutcome::Consumed => PageOutcome::Consumed,
            EditOutcome::Escalate => self.escalate(),
            EditOutcome::Ignored => PageOutcome::Ignored,
            EditOutcome::ListBuffers => self.list_buffers(),
            EditOutcome::Paste => PageOutcome::Paste,
            EditOutcome::Quit { save: true } if rebase::is_rebase_todo(&self.doc().path) => {
                self.finish_todo()
            }
            EditOutcome::Quit { save: true } => {
                self.write();
                PageOutcome::Close
            }
            EditOutcome::Quit { save: false } => match self.cancel_message() {
                Some(outcome) => outcome,
                None => {
                    self.reject_waiter_if_closing();
                    PageOutcome::Close
                }
            },
            EditOutcome::Save => self.save(),
            EditOutcome::Search { forward } => PageOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: if forward {
                    "/".to_string()
                } else {
                    "?".to_string()
                },
                mode: PromptMode::Text,
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
            }),
            EditOutcome::StepBuffer { forward } => self.step_buffer(forward),
            EditOutcome::Yank(text) => PageOutcome::Yank(text),
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
            self.modal.set_message("only this file is open".to_string());
            return PageOutcome::Consumed;
        }
        let at = self.buffer_at();
        let next = match forward {
            true => (at + 1) % count,
            false => (at + count - 1) % count,
        };
        self.select_buffer(next);
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

    /// Look for `pattern` from the cursor, and stay where it was found.
    pub(super) fn start_search(&mut self, pattern: String, forward: bool) -> PageOutcome {
        self.with_modal(|modal, view| modal.start_search(view, pattern, forward));
        PageOutcome::Consumed
    }

    /// Put text the host read out of the clipboard in at the cursor.
    pub(super) fn paste_text(&mut self, text: String) -> PageOutcome {
        self.with_modal(|modal, view| modal.paste_text(view, text));
        PageOutcome::Consumed
    }
}

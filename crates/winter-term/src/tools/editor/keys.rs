//! What a key does in the editor: the chords that belong to the tool rather
//! than to the text (stepping between buffers, finishing a commit message),
//! then the modal keys every text shares, and what the tool does with what
//! those ask of it.

use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::Key;
use crate::model::textedit::modal::{EditMode, EditOutcome};
use crate::model::tool::{
    menu_groups, Notice, PickRequest, PromptMode, PromptRequest, ToolMenuItem, ToolOutcome,
};
use crate::model::vim::nav::VimNav;

use super::comment;
use super::commit;
use super::rebase;
use super::{EditorTool, ASK_BUFFER, ASK_SEARCH, ASK_SEARCH_BACK};

// ========================================================================
// Constants
// ========================================================================

/// The tool's own chords in any file. Stepping between buffers is no part
/// of a text command, so it is taken ahead of the modal keys: the chord means
/// the same whether a file is being read or typed into. Commenting is the
/// same chord in every mode: the line, or the lines a selection covers.
const EDITOR_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::RecentBuffer(false),
        keys: &["C-M-["],
        label: "Back through the buffers by recent use",
    },
    Binding {
        action: Act::RecentBuffer(true),
        keys: &["C-M-]"],
        label: "Forward through the buffers by recent use",
    },
    Binding {
        action: Act::Comment,
        keys: &["C-/"],
        label: "Comment or uncomment the line or selection",
    },
    // `C-s` is the modal keys' own; `C-S-s` saves too, ahead of the window's
    // Grep on the same chord, since a hand reaching to save from an editor
    // reaches for either.
    Binding {
        action: Act::Save,
        keys: &["C-S-s"],
        label: "Write the file",
    },
];

/// The menu's entries typed as Vim keys, the clipboard ones through the `+`
/// register so they reach other windows rather than an internal register.
const MENU_COPY: (&str, &str) = ("\" + y", "Copy");
const MENU_CUT: (&str, &str) = ("\" + d", "Cut");
const MENU_PASTE: (&str, &str) = ("\" + p", "Paste");
const MENU_UNDO: (&str, &str) = ("u", "Undo");
const MENU_REDO: (&str, &str) = ("C-r", "Redo");
const MENU_SEARCH: (&str, &str) = ("/", "Find");
const MENU_ESCALATE: (&str, &str) = ("C-o", "Open in $EDITOR");

/// The menu's names for the tool's own chords, shorter than the help list's.
const MENU_COMMENT: &str = "Toggle Comment";
const MENU_SAVE: &str = "Save";

/// A commit message's chords: save and close to commit, or empty it and close
/// to cancel, and earlier messages to start from.
const MESSAGE_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Finish,
        keys: &["C-c C-c", "C-S-c"],
        label: "Commit with this message",
    },
    Binding {
        action: Act::Cancel,
        keys: &["C-c C-k"],
        label: "Cancel the commit",
    },
    Binding {
        action: Act::Recall(true),
        keys: &["C-M-p"],
        label: "An earlier commit message",
    },
    Binding {
        action: Act::Recall(false),
        keys: &["C-M-n"],
        label: "A later commit message",
    },
];

/// A rebase plan's chords: a letter sets the action of the line under the
/// cursor and steps on to the next, `j` and `k` move the line down or up.
const TODO_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Finish,
        keys: &["C-c C-c"],
        label: "Carry out the rebase",
    },
    Binding {
        action: Act::Cancel,
        keys: &["C-c C-k"],
        label: "Abort the rebase",
    },
    Binding {
        action: Act::SetAction("pick"),
        keys: &["C-c p"],
        label: "Pick the commit",
    },
    Binding {
        action: Act::SetAction("reword"),
        keys: &["C-c r"],
        label: "Reword the commit",
    },
    Binding {
        action: Act::SetAction("edit"),
        keys: &["C-c e"],
        label: "Stop to edit the commit",
    },
    Binding {
        action: Act::SetAction("squash"),
        keys: &["C-c s"],
        label: "Squash into the commit before",
    },
    Binding {
        action: Act::SetAction("fixup"),
        keys: &["C-c f"],
        label: "Fix up the commit before",
    },
    Binding {
        action: Act::SetAction("drop"),
        keys: &["C-c d"],
        label: "Drop the commit",
    },
    Binding {
        action: Act::MoveLine(true),
        keys: &["C-c j"],
        label: "Move the commit down",
    },
    Binding {
        action: Act::MoveLine(false),
        keys: &["C-c k"],
        label: "Move the commit up",
    },
];

/// A rebase plan's bare keys in Normal mode, as magit's plan buffer has them:
/// git's own one-letter name for each action sets it on the line under the
/// cursor and steps on to the next. They stand in for the Vim keys of the
/// same letters only here, where a plan is read and reordered far more than
/// it is typed into; Insert mode still types them.
const TODO_LETTER_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::SetAction("pick"),
        keys: &["p"],
        label: "Pick the commit",
    },
    Binding {
        action: Act::SetAction("reword"),
        keys: &["r"],
        label: "Reword the commit",
    },
    Binding {
        action: Act::SetAction("edit"),
        keys: &["e"],
        label: "Stop to edit the commit",
    },
    Binding {
        action: Act::SetAction("squash"),
        keys: &["s"],
        label: "Squash into the commit before",
    },
    Binding {
        action: Act::SetAction("fixup"),
        keys: &["f"],
        label: "Fix up the commit before",
    },
    Binding {
        action: Act::SetAction("drop"),
        keys: &["d"],
        label: "Drop the commit",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a chord of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    /// Close, leaving nothing for git to act on.
    Cancel,
    /// Comment the line or selection, or uncomment it.
    Comment,
    /// Write and close, for git to carry on.
    Finish,
    /// Move the plan line down when set, else up.
    MoveLine(bool),
    /// Step back through the buffers by recent use, forward when set.
    RecentBuffer(bool),
    /// An earlier commit message when set, else a later one.
    Recall(bool),
    /// Write the file, as `C-s` does.
    Save,
    /// Give the plan line this action.
    SetAction(&'static str),
}

// ========================================================================
// EditorTool: dispatch
// ========================================================================

impl EditorTool {
    /// One key: the tool's own chords first, then the modal keys.
    pub(super) fn dispatch(&mut self, key: &Key) -> ToolOutcome {
        // A buffer standing in for a file not yet read takes no edit, which
        // a save would write over the file: any key but a step to another
        // buffer reads it, asking for the password where sudo wants one.
        if self.doc().unread {
            return match table::find(EDITOR_KEYS, key) {
                Some(act @ Act::RecentBuffer(_)) => self.on_act(act),
                _ => self.read_in(),
            };
        }
        if let Some(outcome) = self.file_chord(key) {
            return outcome;
        }
        if let Some(act) = table::find(EDITOR_KEYS, key) {
            return self.on_act(act);
        }
        let outcome = self.with_modal(|modal, view| modal.on_key(view, key));
        self.carry_out(outcome)
    }

    /// The right-click menu, for the mode the text is in: what a selection
    /// can have done to it, what Normal mode offers, and in Insert mode only
    /// the chords that do not type. A commit message or a rebase plan adds
    /// what finishes it, and a plan in Normal mode the action of its line.
    pub(super) fn menu_items(&self) -> Vec<ToolMenuItem> {
        let vim = |(spec, label): (&str, &str)| {
            table::keys(spec).map(|keys| ToolMenuItem::typed(keys, label))
        };
        let own = |act: Act, label: &str| {
            table::menu_typed(EDITOR_KEYS, act).map(|(keys, _)| ToolMenuItem::typed(keys, label))
        };
        let file = |act: Act| {
            let (keys, label) = table::menu_typed(self.file_keys()?, act)?;
            Some(ToolMenuItem::typed(keys, label))
        };
        let writable = !self.modal.is_read_only();
        let finish: Vec<ToolMenuItem> = [file(Act::Finish), file(Act::Cancel)]
            .into_iter()
            .flatten()
            .collect();
        let mode = self.modal.mode();
        let letters: Vec<ToolMenuItem> =
            match mode == EditMode::Normal && rebase::is_rebase_todo(&self.doc().path) {
                true => TODO_LETTER_KEYS
                    .iter()
                    .filter_map(|binding| table::menu_typed(TODO_LETTER_KEYS, binding.action))
                    .map(|(keys, label)| ToolMenuItem::typed(keys, label))
                    .collect(),
                false => Vec::new(),
            };
        let comment = own(Act::Comment, MENU_COMMENT).filter(|_| writable);
        let save = own(Act::Save, MENU_SAVE).filter(|_| writable);
        let flat = |items: Vec<Option<ToolMenuItem>>| items.into_iter().flatten().collect();
        match mode {
            EditMode::Visual => menu_groups([
                flat(vec![
                    vim(MENU_COPY),
                    vim(MENU_CUT).filter(|_| writable),
                ]),
                flat(vec![comment]),
                letters,
            ]),
            EditMode::Normal => menu_groups([
                flat(vec![vim(MENU_PASTE).filter(|_| writable)]),
                flat(vec![
                    vim(MENU_UNDO).filter(|_| writable),
                    vim(MENU_REDO).filter(|_| writable),
                ]),
                flat(vec![comment, vim(MENU_SEARCH)]),
                letters,
                flat(vec![save, vim(MENU_ESCALATE)]),
                finish,
            ]),
            EditMode::Insert | EditMode::Replace => {
                menu_groups([flat(vec![comment, save]), finish])
            }
        }
    }

    /// Every chord the editor answers for the file it is showing, and the
    /// motions its Vim keys share with every tool.
    pub(super) fn key_help(&self) -> Vec<KeyHelp> {
        let mut rows = self.file_keys().map(table::help).unwrap_or_default();
        if rebase::is_rebase_todo(&self.doc().path) {
            rows.extend(table::help(TODO_LETTER_KEYS));
        }
        rows.extend(table::help(EDITOR_KEYS));
        rows.extend(VimNav::help());
        rows
    }

    /// The chords of the kind of file being edited, for a commit message or
    /// a rebase plan.
    fn file_keys(&self) -> Option<&'static [Binding<Act>]> {
        let path = &self.doc().path;
        if commit::is_message_file(path) {
            return Some(MESSAGE_KEYS);
        }
        rebase::is_rebase_todo(path).then_some(TODO_KEYS)
    }

    /// A chord of a commit message or a rebase plan. `None` for every other
    /// key, which is left to the rest; a `C-c` not followed by a key that
    /// completes it is forgotten, and the key with it.
    fn file_chord(&mut self, key: &Key) -> Option<ToolOutcome> {
        let keys = self.file_keys()?;
        let typed: Vec<Key> = self.chord_start.take().into_iter().chain([*key]).collect();
        match table::lookup(keys, &typed) {
            TableMatch::Exact(act) => Some(self.on_act(act)),
            TableMatch::Prefix => {
                self.chord_start = Some(*key);
                Some(ToolOutcome::Consumed)
            }
            TableMatch::Miss if typed.len() > 1 => Some(ToolOutcome::Consumed),
            TableMatch::Miss => self.todo_letter(key),
        }
    }

    /// A rebase plan's bare action key, while Normal mode has nothing
    /// part-typed for it to finish, or over a Visual selection. `None`
    /// anywhere else.
    fn todo_letter(&mut self, key: &Key) -> Option<ToolOutcome> {
        let ready = self.modal.is_idle() || self.modal.mode() == EditMode::Visual;
        if !rebase::is_rebase_todo(&self.doc().path) || !ready {
            return None;
        }
        let act = table::find(TODO_LETTER_KEYS, key)?;
        Some(self.on_act(act))
    }

    /// Do what a chord of the editor's own asks.
    fn on_act(&mut self, act: Act) -> ToolOutcome {
        let row = self.buffer().row();
        match act {
            Act::Cancel if rebase::is_rebase_todo(&self.doc().path) => {
                // An empty plan is how git is told to abort: it stops at
                // "nothing to do" and leaves the branch as it was.
                let rows = self.buffer().lines().len();
                self.buffer_mut().replace_rows(0, rows, vec![String::new()]);
                self.write();
                ToolOutcome::Close
            }
            Act::Cancel => self.cancel_message().unwrap_or(ToolOutcome::Consumed),
            Act::Comment => self.toggle_comment(),
            Act::Save => self.carry_out(EditOutcome::Save),
            Act::Finish if rebase::is_rebase_todo(&self.doc().path) => self.finish_todo(),
            Act::Finish => {
                self.write();
                ToolOutcome::Close
            }
            Act::MoveLine(true) => {
                self.move_todo_line(row, row + 1);
                ToolOutcome::Consumed
            }
            Act::MoveLine(false) => {
                if row > 0 {
                    self.move_todo_line(row, row - 1);
                }
                ToolOutcome::Consumed
            }
            Act::RecentBuffer(forward) => {
                self.recent_buffer(forward);
                ToolOutcome::Consumed
            }
            Act::Recall(back) => self.recall_message(back),
            Act::SetAction(action) => self.set_todo_action(action),
        }
    }

    /// Give `action` to the plan line under the cursor, or to every commit
    /// line a Visual selection covers, and step on past them. The lines that
    /// are not commits (a comment, a blank) are left as they are; one undo
    /// takes the whole change back.
    fn set_todo_action(&mut self, action: &str) -> ToolOutcome {
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (self.buffer().row(), self.buffer().row()),
        };
        let lines = self.buffer().lines()[first..=last].to_vec();
        let changed: Vec<Option<String>> = lines
            .iter()
            .map(|line| rebase::with_action(line, action))
            .collect();
        if changed.iter().all(Option::is_none) {
            self.modal.report(Notice::error("not a commit line"));
            return ToolOutcome::Consumed;
        }
        let rows = lines
            .into_iter()
            .zip(changed)
            .map(|(line, changed)| changed.unwrap_or(line))
            .collect();
        self.buffer_mut()
            .replace_rows(first, last - first + 1, rows);
        self.leave_to_normal();
        self.buffer_mut().move_to_line(last + 1);
        ToolOutcome::Consumed
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
    fn finish_todo(&mut self) -> ToolOutcome {
        if let Some(problem) = rebase::problem(self.buffer().lines()) {
            self.modal.report(Notice::error(problem));
            return ToolOutcome::Consumed;
        }
        self.write();
        ToolOutcome::Close
    }

    /// Comment out the cursor's line, or every line a selection covers, or
    /// comment them back in, with the file's own comment marker (`C-/`).
    /// The cursor stays on the character it was on.
    fn toggle_comment(&mut self) -> ToolOutcome {
        let marker = self
            .doc()
            .grammar
            .and_then(|grammar| comment::marker_of(grammar.name()));
        let Some(marker) = marker else {
            self.modal
                .report(Notice::error("no comment marker known for this file"));
            return ToolOutcome::Consumed;
        };
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let (first, last) = match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (row, row),
        };
        let before = self.buffer().lines()[first..=last].to_vec();
        let after = comment::toggle(&before, marker);
        if after == before {
            return ToolOutcome::Consumed;
        }
        let shift = after[row - first].chars().count() as isize
            - before[row - first].chars().count() as isize;
        let col = (col as isize + shift).max(0) as usize;
        self.buffer_mut()
            .replace_rows(first, last - first + 1, after);
        match self.modal.is_typing() {
            true => self.buffer_mut().move_to_typing(row, col),
            false => self.buffer_mut().move_to(row, col),
        }
        self.modal.clear_selection();
        ToolOutcome::Consumed
    }

    /// Do what a modal key asked of the tool.
    fn carry_out(&mut self, outcome: EditOutcome) -> ToolOutcome {
        match outcome {
            // A file is never a field that is only typed into, and `Enter`
            // breaks its lines, so these two do not reach a tool.
            EditOutcome::Cancel | EditOutcome::Submit => ToolOutcome::Consumed,
            EditOutcome::Consumed => ToolOutcome::Consumed,
            EditOutcome::Escalate => self.escalate(),
            EditOutcome::Ignored => ToolOutcome::Ignored,
            EditOutcome::ListBuffers => self.list_buffers(),
            EditOutcome::Paste => ToolOutcome::Paste,
            EditOutcome::Quit { save: true } if rebase::is_rebase_todo(&self.doc().path) => {
                self.finish_todo()
            }
            // A save that has to ask first (as root, for a password) keeps
            // the buffer open, since closing would drop the edits it holds.
            EditOutcome::Quit { save: true } => match self.write() {
                ToolOutcome::Consumed => ToolOutcome::Close,
                asked => asked,
            },
            EditOutcome::Quit { save: false } => match self.cancel_message() {
                Some(outcome) => outcome,
                None => {
                    self.reject_waiter_if_closing();
                    ToolOutcome::Close
                }
            },
            EditOutcome::Save => self.save(),
            EditOutcome::Search { forward } => ToolOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: if forward {
                    "/".to_string()
                } else {
                    "?".to_string()
                },
                mode: PromptMode::Search { reverse: !forward },
                tag: if forward { ASK_SEARCH } else { ASK_SEARCH_BACK },
            }),
            EditOutcome::StepBuffer { forward } => self.step_buffer(forward),
            EditOutcome::Yank(text) => ToolOutcome::Yank(text),
        }
    }
}

// ========================================================================
// EditorTool: buffers
// ========================================================================

impl EditorTool {
    /// On to the next open buffer, or back to the one before (`]b`/`[b`).
    fn step_buffer(&mut self, forward: bool) -> ToolOutcome {
        let count = self.buffer_count();
        if count < 2 {
            self.modal.report(Notice::error("only this file is open"));
            return ToolOutcome::Consumed;
        }
        let at = self.buffer_at();
        let next = match forward {
            true => (at + 1) % count,
            false => (at + count - 1) % count,
        };
        self.select_buffer(next);
        ToolOutcome::Consumed
    }

    /// The open buffers, as a list to choose from (`gb`).
    fn list_buffers(&mut self) -> ToolOutcome {
        let (items, icons) = self.buffer_list().into_iter().unzip();
        ToolOutcome::Pick(PickRequest {
            icons,
            items,
            label: "Open buffers".to_string(),
            tag: ASK_BUFFER,
        })
    }

    /// Look for `pattern` from the cursor, and stay where it was found.
    pub(super) fn start_search(&mut self, pattern: String, forward: bool) -> ToolOutcome {
        self.with_modal(|modal, view| modal.start_search(view, pattern, forward));
        ToolOutcome::Consumed
    }

    /// Put text the host read out of the clipboard in at the cursor.
    pub(super) fn paste_text(&mut self, text: String) -> ToolOutcome {
        self.with_modal(|modal, view| modal.paste_text(view, text));
        ToolOutcome::Consumed
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_key_of_the_editor_is_spelled_right() {
        for keys in [EDITOR_KEYS, MESSAGE_KEYS, TODO_KEYS] {
            assert_eq!(table::invalid_specs(keys), Vec::<&str>::new());
        }
    }
}

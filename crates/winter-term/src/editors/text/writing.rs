//! What the editor does for prose: the Markdown and LaTeX commands, on the
//! buffer being edited, over the text [`markdown`] and [`latex`] read.
//!
//! [`markdown`]: super::markdown
//! [`latex`]: super::latex

use std::path::{Path, PathBuf};

use crate::model::editor::{
    CommandOutput, CommandRequest, EditorOutcome, JobRequest, Notice, PromptMode, PromptRequest,
};
use crate::model::input::{Key, KeyCode};

use super::{latex, markdown, TextEditor, ASK_ENV_NEW, ASK_ENV_RENAME};

// ========================================================================
// Constants
// ========================================================================

/// The extensions of a Markdown file.
const MARKDOWN_EXTENSIONS: [&str; 5] = ["md", "markdown", "mdown", "mkd", "mdx"];

/// The extensions of a LaTeX file.
const LATEX_EXTENSIONS: [&str; 4] = ["tex", "ltx", "sty", "cls"];

/// The program a LaTeX document's words are counted with, as magic-vscode
/// counts them: by role, the text apart from the headings, and following
/// every `\input` and `\include` into the files it names.
const TEXCOUNT: &str = "texcount";

/// The tag a word count of a LaTeX document is answered under.
pub(super) const TAG_TEXCOUNT: &str = "texcount";

// ========================================================================
// Data Structures
// ========================================================================

/// What kind of prose a file is written in, for the commands that know it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Prose {
    /// A LaTeX document.
    Latex,
    /// A Markdown document.
    Markdown,
}

// ========================================================================
// Functions
// ========================================================================

/// The kind of prose `path` is written in, by its extension.
pub(super) fn prose_of(path: &Path) -> Option<Prose> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    if MARKDOWN_EXTENSIONS.contains(&extension.as_str()) {
        return Some(Prose::Markdown);
    }
    LATEX_EXTENSIONS
        .contains(&extension.as_str())
        .then_some(Prose::Latex)
}

/// What `texcount`'s brief answer counts, worded: it opens with the words
/// of the text, of the headings, and of the captions, joined by `+`.
fn texcount_summary(stdout: &str) -> Option<String> {
    let line = stdout.lines().map(str::trim).find(|line| !line.is_empty())?;
    let counts: Vec<&str> = line.split_whitespace().next()?.split('+').collect();
    let [text, headings, captions] = counts.as_slice() else {
        return None;
    };
    Some(format!(
        "{text} words of text, {headings} in headings, {captions} in captions"
    ))
}

// ========================================================================
// TextEditor: prose
// ========================================================================

impl TextEditor {
    /// The kind of prose the buffer is written in.
    pub(super) fn prose(&self) -> Option<Prose> {
        prose_of(&self.doc().path)
    }

    /// Move to the next heading or the one before: in Markdown at any level
    /// or, where `same_level` asks, at the cursor's own; in LaTeX the next
    /// sectioning command.
    pub(super) fn step_heading(&mut self, forward: bool, same_level: bool) -> EditorOutcome {
        let row = self.buffer().row();
        let lines = self.buffer().lines();
        let found = match self.prose() {
            Some(Prose::Markdown) => {
                let level = markdown::heading_over(lines, row).map(|heading| heading.level);
                markdown::step_heading(lines, row, forward, |heading| {
                    !same_level || Some(heading.level) == level
                })
            }
            Some(Prose::Latex) => latex::step_section(lines, row, forward),
            None => None,
        };
        self.land_or_say(found, "no more headings that way")
    }

    /// Move up to the heading over the section the cursor is in.
    pub(super) fn heading_up(&mut self) -> EditorOutcome {
        let found = markdown::parent_heading(self.buffer().lines(), self.buffer().row());
        self.land_or_say(found, "no heading above this one")
    }

    /// Promote the heading or list item on the cursor's line (or every line
    /// a selection covers), or demote it when `deeper`; with `subtree`, the
    /// heading and every heading under it.
    pub(super) fn shift_heading(&mut self, deeper: bool, subtree: bool) -> EditorOutcome {
        let row = self.buffer().row();
        let lines = self.buffer().lines().to_vec();
        if subtree {
            let Some(heading) = markdown::heading_over(&lines, row) else {
                return self.say("no heading over the cursor");
            };
            let end = markdown::subtree_end(&lines, heading);
            let Some(shifted) = markdown::shifted_subtree(&lines[heading.row..end], deeper) else {
                return self.say("a heading in it cannot go further");
            };
            self.buffer_mut()
                .replace_rows(heading.row, end - heading.row, shifted);
            self.buffer_mut().move_to(row, 0);
            return EditorOutcome::Consumed;
        }
        let (first, last) = self.selected_rows();
        let shifted: Vec<Option<String>> = lines[first..=last]
            .iter()
            .map(|line| markdown::shifted(line, deeper))
            .collect();
        if shifted.iter().all(Option::is_none) {
            return self.say("not a heading or a list item that can go further");
        }
        let rows = lines[first..=last]
            .iter()
            .zip(shifted)
            .map(|(line, shifted)| shifted.unwrap_or_else(|| line.clone()))
            .collect();
        self.buffer_mut().replace_rows(first, last - first + 1, rows);
        self.leave_to_normal();
        self.buffer_mut().move_to(row, 0);
        EditorOutcome::Consumed
    }

    /// Wrap the selection, or the word under the cursor, in Markdown's
    /// `mark` on both sides, or take the mark off where it already wraps it.
    pub(super) fn toggle_mark(&mut self, mark: &str) -> EditorOutcome {
        self.toggle_around(|line, start, end| markdown::toggled_wrap(line, start, end, mark))
    }

    /// Wrap the selection, or the word under the cursor, in the LaTeX font
    /// command `command`, or take it off where it already wraps it.
    pub(super) fn toggle_font(&mut self, command: &str) -> EditorOutcome {
        self.toggle_around(|line, start, end| latex::toggled_command(line, start, end, command))
    }

    /// Change the characters a one-line selection, or the word under the
    /// cursor, covers by `toggle`, and select what they became.
    fn toggle_around(
        &mut self,
        toggle: impl Fn(&str, usize, usize) -> (String, usize, usize),
    ) -> EditorOutcome {
        let row = self.buffer().row();
        let range = match self.selection() {
            Some(selection) if selection.start.0 == selection.end.0 && !selection.linewise => {
                Some((selection.start.1, selection.end.1 + 1))
            }
            Some(_) => return self.say("select within one line"),
            None => markdown::word_at(&self.buffer().lines()[row], self.buffer().col()),
        };
        let Some((start, end)) = range else {
            return self.say("no word under the cursor");
        };
        let line = self.buffer().lines()[row].clone();
        let (changed, start, _) = toggle(&line, start, end);
        self.buffer_mut().replace_rows(row, 1, vec![changed]);
        self.leave_to_normal();
        self.buffer_mut().move_to(row, start);
        EditorOutcome::Consumed
    }

    /// Tick or clear the task box on the cursor's line, or every line a
    /// selection covers, giving a plain list item one.
    pub(super) fn toggle_task(&mut self) -> EditorOutcome {
        let (first, last) = self.selected_rows();
        let lines = self.buffer().lines()[first..=last].to_vec();
        let toggled: Vec<Option<String>> = lines.iter().map(|line| markdown::toggled_task(line)).collect();
        if toggled.iter().all(Option::is_none) {
            return self.say("not a list item");
        }
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let rows = lines
            .into_iter()
            .zip(toggled)
            .map(|(line, toggled)| toggled.unwrap_or(line))
            .collect();
        self.buffer_mut().replace_rows(first, last - first + 1, rows);
        self.leave_to_normal();
        self.buffer_mut().move_to(row, col);
        EditorOutcome::Consumed
    }

    /// Number the ordered list the cursor is in again, from its first item.
    pub(super) fn renumber_list(&mut self) -> EditorOutcome {
        let row = self.buffer().row();
        let Some((first, rows)) = markdown::renumbered(self.buffer().lines(), row) else {
            return self.say("not in an ordered list");
        };
        let count = rows.len();
        self.buffer_mut().replace_rows(first, count, rows);
        self.buffer_mut().move_to(row, 0);
        EditorOutcome::Consumed
    }

    /// Start the next item under the cursor's list item, or a heading of the
    /// same level under its heading, and type into it.
    pub(super) fn new_item(&mut self) -> EditorOutcome {
        let row = self.buffer().row();
        let line = self.buffer().lines()[row].clone();
        let prefix = match markdown::heading_level(&line) {
            Some(level) => format!("{} ", "#".repeat(level)),
            None => match markdown::next_item_prefix(&line) {
                Some(prefix) => prefix,
                None => return self.say("not a heading or a list item"),
            },
        };
        self.leave_to_normal();
        self.buffer_mut().replace_rows(row + 1, 0, vec![prefix]);
        self.buffer_mut().move_to(row + 1, 0);
        self.type_key(KeyCode::Char('A'));
        EditorOutcome::Consumed
    }

    /// Say how many words the buffer, or the lines a selection covers, hold;
    /// a LaTeX document is counted by `texcount` instead.
    pub(super) fn word_count(&mut self) -> EditorOutcome {
        if self.prose() == Some(Prose::Latex) {
            return self.count_latex_words();
        }
        let (first, last) = match self.selection() {
            Some(_) => self.selected_rows(),
            None => (0, self.buffer().lines().len().saturating_sub(1)),
        };
        let words = markdown::word_count(&self.buffer().lines()[first..=last]);
        let what = match self.selection().is_some() {
            true => "in the selection",
            false => "in the file",
        };
        self.modal
            .report(Notice::info(format!("{words} words {what}")));
        EditorOutcome::Consumed
    }

    /// Ask `texcount` for the words of the document as it is on disk.
    fn count_latex_words(&mut self) -> EditorOutcome {
        let path = self.doc().path.clone();
        if crate::remote::parse(&path).is_some() || !path.is_file() {
            return self.say("word count: not a file on this machine");
        }
        let cwd = path.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("."));
        EditorOutcome::Job(JobRequest::Command(CommandRequest {
            id: 0,
            args: vec![
                "-merge".to_string(),
                "-brief".to_string(),
                "-total".to_string(),
                path.to_string_lossy().into_owned(),
            ],
            cwd,
            env: Vec::new(),
            program: TEXCOUNT.to_string(),
            stdin: None,
            tag: TAG_TEXCOUNT,
        }))
    }

    /// What `texcount` answered.
    pub(super) fn latex_words_counted(&mut self, output: &CommandOutput) -> EditorOutcome {
        let notice = match (output.succeeded(), texcount_summary(&output.stdout)) {
            (true, Some(summary)) => Notice::info(summary),
            (true, None) => Notice::error("texcount said nothing"),
            (false, _) => Notice::error(format!("texcount: {}", output.failure())),
        };
        self.modal.report(notice);
        EditorOutcome::Consumed
    }

    /// Ask for the name of an environment to start at the cursor.
    pub(super) fn ask_new_environment(&mut self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "New environment: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_ENV_NEW,
        })
    }

    /// Start the environment `name` on the cursor's line if it is blank, else
    /// under it, and type inside it.
    pub(super) fn new_environment(&mut self, name: &str) -> EditorOutcome {
        let name = name.trim();
        if name.is_empty() {
            return EditorOutcome::Consumed;
        }
        let row = self.buffer().row();
        let line = self.buffer().lines()[row].clone();
        let indent: String = line.chars().take_while(|c| c.is_whitespace()).collect();
        let lines = latex::new_environment(name, &indent);
        self.leave_to_normal();
        let at = match line.trim().is_empty() {
            true => {
                self.buffer_mut().replace_rows(row, 1, lines);
                row
            }
            false => {
                self.buffer_mut().replace_rows(row + 1, 0, lines);
                row + 1
            }
        };
        self.buffer_mut().move_to(at + 1, 0);
        self.type_key(KeyCode::Char('A'));
        EditorOutcome::Consumed
    }

    /// Ask what to rename the environment the cursor is in to, starting from
    /// its own name.
    pub(super) fn ask_rename_environment(&mut self) -> EditorOutcome {
        let Some(env) = latex::enclosing(self.buffer().lines(), self.buffer().row()) else {
            return self.say("the cursor is in no environment");
        };
        EditorOutcome::Prompt(PromptRequest {
            initial: env.name.clone(),
            label: format!("Rename {} to: ", env.name),
            mode: PromptMode::Text,
            tag: ASK_ENV_RENAME,
        })
    }

    /// Rename the environment the cursor is in to `name`, at both ends.
    pub(super) fn rename_environment(&mut self, name: &str) -> EditorOutcome {
        let name = name.trim();
        let lines = self.buffer().lines().to_vec();
        let Some(env) = latex::enclosing(&lines, self.buffer().row()) else {
            return EditorOutcome::Consumed;
        };
        if name.is_empty() || name == env.name {
            return EditorOutcome::Consumed;
        }
        let changed = latex::renamed(&lines, &env, name);
        self.replace_lines(&lines, changed)
    }

    /// Turn the display the cursor is in from `\[...\]` into an `equation`
    /// environment, or back.
    pub(super) fn toggle_display(&mut self) -> EditorOutcome {
        let lines = self.buffer().lines().to_vec();
        match latex::toggled_display(&lines, self.buffer().row()) {
            Some(changed) => self.replace_lines(&lines, changed),
            None => self.say("the cursor is in no \\[...\\] display or equation"),
        }
    }

    /// Put `changed` in place of the rows of `lines` it names, as one change.
    fn replace_lines(&mut self, lines: &[String], changed: Vec<(usize, String)>) -> EditorOutcome {
        let Some(first) = changed.iter().map(|(row, _)| *row).min() else {
            return EditorOutcome::Consumed;
        };
        let last = changed.iter().map(|(row, _)| *row).max().unwrap_or(first);
        let mut rows = lines[first..=last].to_vec();
        for (row, line) in changed {
            rows[row - first] = line;
        }
        let (row, col) = (self.buffer().row(), self.buffer().col());
        self.buffer_mut().replace_rows(first, last - first + 1, rows);
        self.buffer_mut().move_to(row, col);
        EditorOutcome::Consumed
    }

    /// The first and last rows a selection covers, or the cursor's row.
    fn selected_rows(&self) -> (usize, usize) {
        match self.selection() {
            Some(selection) => (selection.start.0, selection.end.0),
            None => (self.buffer().row(), self.buffer().row()),
        }
    }

    /// Move to `row`, or say `otherwise` where there is none.
    fn land_or_say(&mut self, row: Option<usize>, otherwise: &str) -> EditorOutcome {
        match row {
            Some(row) => {
                self.leave_to_normal();
                self.buffer_mut().move_to(row, 0);
                EditorOutcome::Consumed
            }
            None => self.say(otherwise),
        }
    }

    /// Say `text` in the status bar as a refusal.
    fn say(&mut self, text: &str) -> EditorOutcome {
        self.modal.report(Notice::error(text));
        EditorOutcome::Consumed
    }

    /// Send the modal keys one bare key, as if it were typed.
    fn type_key(&mut self, code: KeyCode) {
        let key = Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        };
        self.with_modal(|modal, view| modal.on_key(view, &key));
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_prose_is_known_by_its_extension() {
        assert_eq!(prose_of(Path::new("a/README.md")), Some(Prose::Markdown));
        assert_eq!(prose_of(Path::new("paper.TEX")), Some(Prose::Latex));
        assert_eq!(prose_of(Path::new("main.rs")), None);
        assert_eq!(
            texcount_summary("\n120+8+2 (3/1/0/0) Total\n").as_deref(),
            Some("120 words of text, 8 in headings, 2 in captions")
        );
        assert_eq!(texcount_summary("no counts here"), None);
    }
}

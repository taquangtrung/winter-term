//! Looking the word under the cursor up in the dictionary (`S-M-t`), as
//! magic-vscode translates it: its entry opens in a card at the cursor.

use crate::editors::dictionary::{self, Lookup};
use crate::model::editor::{EditorOutcome, Notice};

use super::TextEditor;

// ========================================================================
// TextEditor: the dictionary
// ========================================================================

impl TextEditor {
    /// Look the word under the cursor, or the text selected on one line,
    /// up in the dictionary, and show its entry.
    pub(super) fn look_up_word(&mut self) -> EditorOutcome {
        let row = self.buffer().row();
        let line: Vec<char> = self.line_of(row).chars().collect();
        let selected = self
            .selection()
            .filter(|selection| !selection.linewise && selection.start.0 == selection.end.0)
            .map(|selection| line[selection.start.1..(selection.end.1 + 1).min(line.len())].iter().collect::<String>());
        let word = selected.or_else(|| {
            let col = self.buffer().col().min(line.len());
            let is_letter = |c: &char| c.is_alphabetic() || *c == '\'' || *c == '-';
            let start = (0..col).rev().take_while(|at| is_letter(&line[*at])).last().unwrap_or(col);
            let end = (col..line.len()).take_while(|at| is_letter(&line[*at])).last().map_or(col, |at| at + 1);
            (start < end).then(|| line[start..end].iter().collect())
        });
        let Some(word) = word.map(|word| word.trim_matches(['\'', '-']).to_string()).filter(|word| !word.is_empty()) else {
            self.modal.report(Notice::info("no word under the cursor to look up").unlogged());
            return EditorOutcome::Consumed;
        };
        match dictionary::look_up(&word) {
            Lookup::Found(entry) => {
                let mut text = format!("{}  ({})", entry.headword, entry.dictionary);
                for line in entry.lines {
                    text.push('\n');
                    text.push_str(&line);
                }
                self.show_card(text);
            }
            Lookup::Loading => self
                .modal
                .report(Notice::info("the dictionary is still being read: ask again in a moment").unlogged()),
            Lookup::NotFound => self.modal.report(Notice::info(format!("{word}: not in the dictionary")).unlogged()),
            Lookup::Missing(why) => self.modal.report(Notice::error(why)),
        }
        EditorOutcome::Consumed
    }
}

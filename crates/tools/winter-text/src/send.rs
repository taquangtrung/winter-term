//! Code sent to the terminal beside, to run there: the selection, or else
//! the cursor's line, after which the cursor steps on to the next line
//! holding anything, so a script is walked through a line at a time, as
//! Emacs sends a region to its inferior process and vim-slime to a REPL.
//! Which terminal is the window's to choose.

use winter_model::editor::{EditorOutcome, Notice};
use winter_model::textedit::modal::Selection;

use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// What is said where there is nothing to send.
const NOTHING_TO_SEND: &str = "nothing to send: the line is blank";

// ========================================================================
// TextEditor: sending to a terminal
// ========================================================================

impl TextEditor {
    /// Send the selection, or the cursor's line and step past it, to the
    /// terminal beside.
    pub(super) fn send_to_terminal(&mut self) -> EditorOutcome {
        let text = match self.selection() {
            Some(selection) => {
                let text = selected_text(self.buffer().lines(), selection);
                self.leave_to_normal();
                text
            }
            None => {
                let row = self.buffer().row();
                let lines = self.buffer().lines();
                let text = lines[row].clone();
                let next = (row + 1..lines.len()).find(|at| !lines[*at].trim().is_empty());
                if let Some(next) = next {
                    self.buffer_mut().move_to_line(next);
                }
                text
            }
        };
        if text.trim().is_empty() {
            self.modal.report(Notice::info(NOTHING_TO_SEND).unlogged());
            return EditorOutcome::Consumed;
        }
        EditorOutcome::SendToTerminal(text)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The text `selection` covers in `lines`: whole lines, a block's columns
/// on each of its rows, or the characters from its first point to its last,
/// both taken.
fn selected_text(lines: &[String], selection: Selection) -> String {
    let (first, last) = (selection.start.0, selection.end.0.min(lines.len() - 1));
    let columns = |line: &str, from: usize, to: Option<usize>| -> String {
        let chars = line.chars().skip(from);
        match to {
            Some(to) => chars.take((to + 1).saturating_sub(from)).collect(),
            None => chars.collect(),
        }
    };
    let rows = first..=last;
    let picked: Vec<String> = match (selection.linewise, selection.block) {
        (true, _) => rows.map(|row| lines[row].clone()).collect(),
        (false, true) => rows
            .map(|row| columns(&lines[row], selection.start.1, Some(selection.end.1)))
            .collect(),
        (false, false) => rows
            .map(|row| {
                let from = if row == first { selection.start.1 } else { 0 };
                let to = (row == last).then_some(selection.end.1);
                columns(&lines[row], from, to)
            })
            .collect(),
    };
    picked.join("\n")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    fn selection(
        start: (usize, usize),
        end: (usize, usize),
        linewise: bool,
        block: bool,
    ) -> Selection {
        Selection {
            block,
            end,
            linewise,
            start,
        }
    }

    #[test]
    fn test_a_line_is_sent_and_the_cursor_steps_past_blank_lines_to_the_next() {
        let path = std::env::temp_dir().join(format!("winter-send-{}.py", std::process::id()));
        std::fs::write(&path, "x = 1\n\n   \ny = 2\n").expect("file");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        let EditorOutcome::SendToTerminal(sent) = editor.send_to_terminal() else {
            panic!("sent");
        };
        assert_eq!(sent, "x = 1");
        assert_eq!(editor.buffer().row(), 3, "past the blank lines");
        let EditorOutcome::SendToTerminal(sent) = editor.send_to_terminal() else {
            panic!("sent");
        };
        assert_eq!(sent, "y = 2");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_selection_sends_what_it_covers_its_last_character_taken() {
        let text = lines("x = 1\ny = x + 2\nprint(y)");
        let chars = selection((0, 4), (1, 4), false, false);
        assert_eq!(selected_text(&text, chars), "1\ny = x");
        let whole = selection((1, 3), (2, 0), true, false);
        assert_eq!(selected_text(&text, whole), "y = x + 2\nprint(y)");
        let block = selection((0, 0), (1, 0), false, true);
        assert_eq!(selected_text(&text, block), "x\ny");
    }
}

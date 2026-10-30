//! The shape of a program's text, for moving by it: the definition the
//! cursor is in (a function, a type, a module), from the line its grammar
//! names it on to the bracket that closes it, or the last line indented
//! under it; and the blocks the indentation makes, stepped between as
//! folds are.

use std::time::Duration;

use crate::model::editor::{EditorOutcome, Notice};

use crate::editors::highlight;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The brackets a definition's body opens with, and closes with.
const OPENERS: [(char, char); 3] = [('{', '}'), ('(', ')'), ('[', ']')];

/// How many lines past a definition's name its body may open, for a
/// signature written over several lines.
const BODY_LOOKAHEAD: usize = 12;

// ========================================================================
// TextEditor: definitions
// ========================================================================

impl TextEditor {
    /// The lines the grammar names a definition on, top to bottom.
    fn definition_rows(&mut self) -> Vec<usize> {
        let total = self.buffer().lines().len();
        let doc = self.doc_mut();
        if doc.grammar.is_none() {
            return Vec::new();
        }
        doc.color_upto = total;
        doc.learn_states(Duration::MAX);
        let doc = self.doc();
        doc.buffer
            .lines()
            .iter()
            .enumerate()
            .filter(|(row, line)| doc.states.get(*row).is_some_and(|state| highlight::defines(line, state)))
            .map(|(row, _)| row)
            .collect()
    }

    /// The first and last lines of the innermost definition holding line
    /// `row`.
    pub(super) fn enclosing_definition(&mut self, row: usize) -> Option<(usize, usize)> {
        let starts = self.definition_rows();
        let lines = self.buffer().lines();
        starts
            .iter()
            .rev()
            .filter(|start| **start <= row)
            .map(|start| (*start, end_of(lines, *start)))
            .find(|(_, end)| *end >= row)
    }

    /// To the first line of the definition the cursor is in, or of the one
    /// above it where the cursor is already there; to its last line when
    /// `end` is set.
    pub(super) fn goto_definition_edge(&mut self, end: bool) -> EditorOutcome {
        let row = self.buffer().row();
        let found = match end {
            true => self.enclosing_definition(row).map(|(_, last)| last),
            false => self.enclosing_definition(row).map(|(first, _)| first).and_then(|first| match first == row {
                // Already on its first line: the definition holding this one.
                true => row.checked_sub(1).and_then(|above| self.enclosing_definition(above)).map(|(up, _)| up),
                false => Some(first),
            }),
        };
        match found {
            Some(target) => {
                self.remember_jump();
                self.buffer_mut().move_to_line(target);
            }
            None => self.modal.report(Notice::info("not inside a definition").unlogged()),
        }
        EditorOutcome::Consumed
    }

    /// Select the whole of the definition the cursor is in, by lines; a
    /// second press while it is selected takes the one holding it.
    pub(super) fn select_definition(&mut self) -> EditorOutcome {
        let row = self.buffer().row();
        let selected = self.selection().filter(|selection| selection.linewise);
        let found = match selected {
            Some(selection) => {
                let (first, last) = (selection.start.0, selection.end.0);
                let mut at = first;
                let mut wider = None;
                while let Some(above) = at.checked_sub(1) {
                    match self.enclosing_definition(above) {
                        Some((start, end)) if end >= last => {
                            wider = Some((start, end));
                            break;
                        }
                        Some((start, _)) => at = start,
                        None => break,
                    }
                }
                wider
            }
            None => self.enclosing_definition(row),
        };
        let Some((first, last)) = found else {
            self.modal.report(Notice::info("not inside a definition").unlogged());
            return EditorOutcome::Consumed;
        };
        self.modal.clear_selection();
        self.buffer_mut().move_to_line(first);
        let at = self.at.min(self.docs.len() - 1);
        self.modal.start_visual(&self.docs[at].buffer, true);
        self.buffer_mut().move_to_line(last);
        EditorOutcome::Consumed
    }

    /// On to the start of the next block the indentation makes, or back to
    /// the start of the one before: a line indented deeper than the one
    /// above it, as an editor folds there.
    pub(super) fn step_block(&mut self, forward: bool) -> EditorOutcome {
        let lines = self.buffer().lines();
        let starts = block_starts(lines);
        let row = self.buffer().row();
        let next = match forward {
            true => starts.iter().find(|start| **start > row),
            false => starts.iter().rev().find(|start| **start < row),
        };
        match next.copied() {
            Some(target) => self.buffer_mut().move_to_line(target),
            None => self.modal.report(Notice::info("no block further on").unlogged()),
        }
        EditorOutcome::Consumed
    }

    /// Remember where the cursor is as a mark `''` goes back to.
    fn remember_jump(&mut self) {
        let here = (self.buffer().row(), self.buffer().col());
        self.doc_mut().marks.jump = Some(here);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The last line of the definition named on line `start`: where the bracket
/// its body opens with closes, or, for a body set off by indentation alone
/// (Python, YAML), the last line indented deeper than `start`.
fn end_of(lines: &[String], start: usize) -> usize {
    let indent = indent_of(&lines[start]);
    // The body's bracket opens on the name's line or soon after it, before
    // a line back at the definition's own indentation.
    let mut depth = 0usize;
    let mut opened = false;
    for (row, line) in lines.iter().enumerate().skip(start) {
        if row > start + BODY_LOOKAHEAD && !opened {
            break;
        }
        for c in code_chars(line) {
            if OPENERS.iter().any(|(open, _)| *open == c) {
                if c == '{' || opened {
                    depth += 1;
                    opened = opened || c == '{';
                }
            } else if OPENERS.iter().any(|(_, close)| *close == c) && opened {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return row;
                }
            }
        }
        let code: String = code_chars(line).collect();
        // A line ending in `;` before any body is a declaration alone, and
        // one ending in `:` opens a body set off by indentation.
        if !opened && code.trim_end().ends_with(';') {
            return row;
        }
        if !opened && code.trim_end().ends_with(':') {
            break;
        }
        if !opened && row > start && !line.trim().is_empty() && indent_of(line) <= indent {
            break;
        }
    }
    let mut last = start;
    for (row, line) in lines.iter().enumerate().skip(start + 1) {
        if line.trim().is_empty() {
            continue;
        }
        if indent_of(line) <= indent {
            break;
        }
        last = row;
    }
    last
}

/// The characters of `line` that are code: before a line comment, and
/// outside strings.
fn code_chars(line: &str) -> impl Iterator<Item = char> + '_ {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut previous = ' ';
    let mut comment = false;
    line.chars().filter(move |&c| {
        if comment {
            return false;
        }
        let keep = match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == open {
                    quote = None;
                }
                false
            }
            None => match c {
                '"' | '`' => {
                    quote = Some(c);
                    false
                }
                '/' if previous == '/' => {
                    comment = true;
                    false
                }
                '#' if previous == ' ' || previous == '\t' => {
                    comment = true;
                    false
                }
                _ => true,
            },
        };
        previous = c;
        keep
    })
}

/// The lines indented deeper than the line above them that holds text: where
/// a block the indentation makes starts, as a fold would.
fn block_starts(lines: &[String]) -> Vec<usize> {
    let mut starts = Vec::new();
    let mut above: Option<usize> = None;
    for (row, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let indent = indent_of(line);
        if above.is_some_and(|above| indent > above) {
            // The block is the line that opens it, which is the one above.
            if let Some(opener) = (0..row).rev().find(|at| !lines[*at].trim().is_empty()) {
                starts.push(opener);
            }
        }
        above = Some(indent);
    }
    starts.dedup();
    starts
}

/// How far `line` is indented, in columns, a tab counting as four.
fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { 4 } else { 1 })
        .sum()
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

    #[test]
    fn test_a_braced_body_ends_where_its_bracket_closes_strings_and_comments_aside() {
        let text = lines("fn f(\n    a: u8,\n) -> u8 {\n    let s = \"}\"; // }\n    a\n}\nfn g() {}");
        assert_eq!(end_of(&text, 0), 5);
        assert_eq!(end_of(&text, 6), 6, "a body on one line");
        let declaration = lines("fn f(a: u8);\nfn g() {}");
        assert_eq!(end_of(&declaration, 0), 0, "a declaration has no body");
    }

    #[test]
    fn test_an_indented_body_ends_at_the_last_line_indented_under_it() {
        let text = lines("def f(x):\n    if x:\n        d = {\n        }\n    return 2\n\ndef g():\n    pass");
        assert_eq!(end_of(&text, 0), 4);
        assert_eq!(end_of(&text, 6), 7);
    }

    #[test]
    fn test_a_block_starts_on_the_line_that_opens_a_deeper_indent() {
        let text = lines("a\n  b\n  c\n    d\ne\n  f");
        assert_eq!(block_starts(&text), [0, 2, 4]);
    }
}

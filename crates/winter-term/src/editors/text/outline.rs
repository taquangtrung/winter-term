//! The outline of a file: its headings in Markdown, its sectioning commands
//! in LaTeX, and in a program what its grammar names as defined (functions,
//! types, modules), each indented by how deep it sits, to be gone through
//! the way the jump list is.

use crate::model::editor::JumpTarget;

use crate::editors::highlight;
use super::writing::Prose;
use super::{latex, markdown, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// What an entry is indented by for each level it sits below the top.
const LEVEL_INDENT: &str = "  ";

/// How many characters of a definition's line an entry shows.
const ENTRY_CHARS: usize = 100;

/// The columns a tab counts for, when a line's indent says how deep it is.
const TAB_COLUMNS: usize = 4;

// ========================================================================
// TextEditor: outline
// ========================================================================

impl TextEditor {
    /// The outline of the buffer being edited, top to bottom.
    pub(super) fn outline(&mut self) -> Vec<JumpTarget> {
        match self.prose() {
            Some(Prose::Markdown) => markdown_outline(self.buffer().lines()),
            Some(Prose::Latex) => latex_outline(self.buffer().lines()),
            None => self.definition_outline(),
        }
    }

    /// The lines the file's grammar names a definition on, indented by how
    /// far the line is: a method under its `impl`, a nested class under the
    /// one holding it.
    fn definition_outline(&mut self) -> Vec<JumpTarget> {
        let total = self.buffer().lines().len();
        let doc = self.doc_mut();
        if doc.grammar.is_none() {
            return Vec::new();
        }
        doc.color_upto = total;
        doc.learn_states(std::time::Duration::MAX);
        let doc = self.doc();
        let lines = doc.buffer.lines();
        let indents: Vec<usize> = lines.iter().map(|line| indent_of(line)).collect();
        let step = indents.iter().copied().filter(|width| *width > 0).min().unwrap_or(1);
        lines
            .iter()
            .enumerate()
            .filter(|(row, line)| {
                doc.states
                    .get(*row)
                    .is_some_and(|state| highlight::defines(line, state))
            })
            .map(|(row, line)| entry(row, indents[row] / step, line.trim()))
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A Markdown file's headings, each indented below the shallowest.
fn markdown_outline(lines: &[String]) -> Vec<JumpTarget> {
    let headings = markdown::headings(lines);
    let top = headings.iter().map(|heading| heading.level).min().unwrap_or(1);
    headings
        .into_iter()
        .map(|heading| {
            let text = lines[heading.row].trim_start_matches('#').trim();
            entry(heading.row, heading.level - top, text)
        })
        .collect()
}

/// A LaTeX document's sectioning commands, each indented below the
/// shallowest, named by its title.
fn latex_outline(lines: &[String]) -> Vec<JumpTarget> {
    let sections: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter_map(|(row, line)| latex::section_depth(line).map(|depth| (row, depth)))
        .collect();
    let top = sections.iter().map(|(_, depth)| *depth).min().unwrap_or(0);
    sections
        .into_iter()
        .map(|(row, depth)| entry(row, depth - top, &title_of(&lines[row])))
        .collect()
}

/// What a sectioning command's braces hold: its title, or the whole line
/// where the braces do not close on it.
fn title_of(line: &str) -> String {
    let Some(open) = line.find('{') else {
        return line.trim().to_string();
    };
    let mut depth = 0usize;
    for (at, c) in line[open..].char_indices() {
        match c {
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return line[open + 1..open + at].to_string();
                }
            }
            _ => {}
        }
    }
    line.trim().to_string()
}

/// One entry: line `row`, `level` deep, shown as `text`.
fn entry(row: usize, level: usize, text: &str) -> JumpTarget {
    let text: String = text.chars().take(ENTRY_CHARS).collect();
    JumpTarget::new(
        row,
        format!("{:>5}  {}{text}", row + 1, LEVEL_INDENT.repeat(level)),
    )
}

/// How far `line` is indented, in columns, a tab counting for
/// [`TAB_COLUMNS`].
fn indent_of(line: &str) -> usize {
    line.chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { TAB_COLUMNS } else { 1 })
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

    fn labels(entries: &[JumpTarget]) -> Vec<String> {
        entries.iter().map(|entry| entry.label.clone()).collect()
    }

    #[test]
    fn test_markdown_headings_are_indented_below_the_shallowest() {
        let outline = markdown_outline(&lines("## A\ntext\n### B\n## C"));
        assert_eq!(labels(&outline), ["    1  A", "    3    B", "    4  C"]);
        assert_eq!(outline[1].target, 2);
    }

    #[test]
    fn test_latex_sections_are_named_by_their_titles() {
        let doc = lines("\\section{Intro}\n\\subsection*{The {nested} part}\n\\section{End");
        assert_eq!(
            labels(&latex_outline(&doc)),
            ["    1  Intro", "    2    The {nested} part", "    3  \\section{End"]
        );
    }
}

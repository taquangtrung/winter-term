//! The outline of a file, each entry indented by how deep it sits, to be
//! gone through the way the jump list is: what a language server serving
//! the file says it defines, asked for whenever typing pauses; else an
//! outline written for the file's family (Markdown's headings, LaTeX's
//! sections, KDL's blocks, JSON's and YAML's keys, and the like); else what a program's
//! grammar names as defined (functions, types, modules). A family with no
//! outline of its own gets none guessed from its braces or indentation.

use std::time::{Duration, Instant};

use serde_json::Value;
use winter_lsp::protocol;
use winter_model::editor::{EditorOutcome, JumpTarget};

use super::writing::Prose;
use super::{latex, markdown, TextEditor};

mod html;
mod ini;
mod json;
mod kdl;
mod nginx;
mod rst;
mod ssh;
mod swift;
mod xml;
mod yaml;

// ========================================================================
// Constants
// ========================================================================

/// What an entry is indented by for each level it sits below the top.
const LEVEL_INDENT: &str = "  ";

/// How many characters of a definition's line an entry shows.
const ENTRY_CHARS: usize = 100;

/// The columns a tab counts for, when a line's indent says how deep it is.
const TAB_COLUMNS: usize = 4;

/// The languages with an outline of their own, as settings name them (a
/// grammar's name, lowercased, where Winter has no name of its own).
const HTML: &str = "html";
const INI: &str = "ini";
const JSON: &str = "json";
const KDL: &str = "kdl";
const NGINX: &str = "nginx";
const RESTRUCTURED_TEXT: &str = "restructuredtext";
const SSH_CONFIG: &str = "ssh config";
const SSHD_CONFIG: &str = "sshd config";
const SWIFT: &str = "swift";
const XML: &str = "xml";
const YAML: &str = "yaml";

/// How long typing pauses before the symbols are asked for again.
const SYMBOL_PAUSE: Duration = Duration::from_millis(300);

/// What a server says it can do, for document symbols.
const PROVIDER: &str = "documentSymbolProvider";

// ========================================================================
// Data Structures
// ========================================================================

/// What the language server says one document defines, and when to ask it
/// again.
#[derive(Debug, Default)]
pub(super) struct Symbols {
    /// The revision of the text last asked about, so it is asked once.
    asked: Option<usize>,
    /// When to ask, once typing has paused.
    due: Option<Instant>,
    /// The outline last answered, and the revision of the text it is of.
    held: Option<(usize, Vec<JumpTarget>)>,
}

// ========================================================================
// TextEditor: outline
// ========================================================================

impl TextEditor {
    /// The outline of the buffer being edited, top to bottom.
    pub(super) fn outline(&mut self) -> Vec<JumpTarget> {
        match self.prose() {
            Some(Prose::Markdown) => markdown_outline(self.buffer().lines()),
            Some(Prose::Latex) => latex_outline(self.buffer().lines()),
            None => match self.served_outline() {
                Some(served) => served,
                None => {
                    let lines = self.buffer().lines();
                    match self.language().as_str() {
                        HTML => html::outline(lines),
                        INI => ini::outline(lines),
                        JSON => json::outline(lines),
                        KDL => kdl::outline(lines),
                        NGINX => nginx::outline(lines),
                        RESTRUCTURED_TEXT => rst::outline(lines),
                        SSH_CONFIG | SSHD_CONFIG => ssh::outline(lines),
                        SWIFT => swift::outline(lines),
                        XML => xml::outline(lines),
                        YAML => yaml::outline(lines),
                        _ => self.definition_outline(),
                    }
                }
            },
        }
    }

    /// When the symbols are next to be asked for.
    pub(super) fn symbols_next_tick(&self) -> Option<Instant> {
        self.doc().symbols.due
    }

    /// Ask for the symbols where the text has changed since they were last
    /// asked about and typing has paused, from a server that has them.
    pub(super) fn symbols_tick(&mut self, now: Instant) {
        let revision = self.buffer().revision();
        if self.doc().symbols.asked == Some(revision) || self.prose().is_some() {
            return;
        }
        let Some(port) = self
            .serving_port()
            .filter(|port| port.provides(PROVIDER))
            .cloned()
        else {
            return;
        };
        match self.doc().symbols.due {
            None => {
                self.doc_mut().symbols.due = Some(now + SYMBOL_PAUSE);
                return;
            }
            Some(due) if due > now => return,
            Some(_) => {}
        }
        let symbols = &mut self.doc_mut().symbols;
        symbols.due = None;
        symbols.asked = Some(revision);
        let params = protocol::document_params(&self.doc().path);
        if let Some(id) = port.request("textDocument/documentSymbol", params) {
            self.wait_for_symbols(port.id(), id, revision);
        }
    }

    /// Take the symbols a server answered about the text at `revision`.
    pub(super) fn take_symbols(&mut self, revision: usize, value: &Value) -> EditorOutcome {
        // Lines a server counted in text changed since would land wrong.
        if revision != self.buffer().revision() {
            return EditorOutcome::Consumed;
        }
        let outline = symbol_outline(self.buffer().lines(), &protocol::document_symbols(value));
        self.doc_mut().symbols.held = Some((revision, outline));
        EditorOutcome::Consumed
    }

    /// The outline the server gave for the text as it is, where it gave
    /// one with anything in it.
    fn served_outline(&self) -> Option<Vec<JumpTarget>> {
        let (revision, outline) = self.doc().symbols.held.as_ref()?;
        (*revision == self.buffer().revision() && !outline.is_empty()).then(|| outline.clone())
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
        let step = indents
            .iter()
            .copied()
            .filter(|width| *width > 0)
            .min()
            .unwrap_or(1);
        lines
            .iter()
            .enumerate()
            .filter(|(row, line)| {
                doc.states
                    .get(*row)
                    .is_some_and(|state| winter_syntax::defines(line, state))
            })
            .map(|(row, line)| entry(row, indents[row] / step, line.trim()))
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The lines `symbols` name, each indented by how deep its symbol sits,
/// one entry to a line.
fn symbol_outline(lines: &[String], symbols: &[protocol::Symbol]) -> Vec<JumpTarget> {
    let mut outline: Vec<JumpTarget> = Vec::new();
    for symbol in symbols {
        if symbol.at.line as usize >= lines.len() {
            continue;
        }
        let (row, _) = protocol::place_of(lines, symbol.at);
        if outline.last().is_some_and(|last| last.target == row) {
            continue;
        }
        outline.push(entry(row, symbol.depth, lines[row].trim()));
    }
    outline
}

/// A Markdown file's headings, each indented below the shallowest.
fn markdown_outline(lines: &[String]) -> Vec<JumpTarget> {
    let headings = markdown::headings(lines);
    let top = headings
        .iter()
        .map(|heading| heading.level)
        .min()
        .unwrap_or(1);
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

    pub(super) fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    pub(super) fn labels(entries: &[JumpTarget]) -> Vec<String> {
        entries.iter().map(|entry| entry.label.clone()).collect()
    }

    #[test]
    fn test_a_served_outline_nests_by_depth_and_gives_a_line_once() {
        let at = |line: u32, depth: usize| protocol::Symbol {
            at: protocol::Position { character: 4, line },
            depth,
        };
        let doc = lines("struct S {\n    a: u8, b: u8,\n}\nfn f() {}");
        let outline = symbol_outline(&doc, &[at(0, 0), at(1, 1), at(1, 1), at(3, 0), at(9, 0)]);
        assert_eq!(
            labels(&outline),
            [
                "    1  struct S {",
                "    2    a: u8, b: u8,",
                "    4  fn f() {}"
            ]
        );
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
            [
                "    1  Intro",
                "    2    The {nested} part",
                "    3  \\section{End"
            ]
        );
    }
}

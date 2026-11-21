//! Searching an editor's rows, and the jumps a search or a link lands on.

use std::path::PathBuf;

pub use crate::search::{SearchReport, match_spans};

use crate::paint::{IconKind, StyleRun};

// ========================================================================
// Data Structures
// ========================================================================

/// One place the jump list can go: a line, a row, an entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JumpTarget {
    /// What the place is, drawn before its label: a listing's file or folder.
    /// `None` for a line of text, which is not a thing with a type.
    pub icon: Option<IconKind>,
    /// How the list shows it.
    pub label: String,
    /// The editor's own index for it, as a [`JumpStep`] names it.
    pub target: usize,
}

/// What an editor could color of the jump targets a list shows: the syntax
/// colors of each target's text past its line number.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct TargetStyles {
    /// Whether some targets asked for were not reached in the time given,
    /// so asking again (the next frame) colors more of them.
    pub pending: bool,
    /// The targets colored, in no particular order.
    pub styles: Vec<TargetStyle>,
}

/// One jump target's colors.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TargetStyle {
    /// The colors of its text past its line number, as runs.
    pub runs: Vec<StyleRun>,
    /// The target, as [`JumpTarget::target`] names it.
    pub target: usize,
}

/// One step through the jump list or an outline, offered to the editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JumpStep {
    /// The jump called off: back to where it started.
    Cancel,
    /// The move to the target made for good, where it started kept as the
    /// place to jump back to.
    Confirm(usize),
    /// The target shown while the list is still being gone through.
    Preview(usize),
}

/// A search over a directory tree for the lines holding one of a list of
/// words whole, in its own case: what a list of comment keywords means.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    /// Where the walk starts.
    pub root: PathBuf,
    /// The words, one of which each line reported holds.
    pub words: Vec<String>,
}

/// What a search found: every matching line.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResult {
    /// The matching lines, in the order the walk found them.
    pub hits: Vec<SearchHit>,
}

/// One matching line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    /// Which line of the file it is, counting from one.
    pub line: usize,
    /// The file holding it.
    pub path: PathBuf,
    /// The line itself, with trailing whitespace dropped.
    pub text: String,
}

// ========================================================================
// Functions
// ========================================================================

/// The index of the next label holding `query`, looking one past `from` and
/// wrapping once so the row under the cursor is tried last. Matching is
/// [`match_spans`]'s, which keeps an editor's search free of a pattern engine.
pub fn find_match(
    labels: &[impl AsRef<str>],
    query: &str,
    from: usize,
    forward: bool,
) -> Option<usize> {
    let total = labels.len();
    if query.is_empty() || total == 0 {
        return None;
    }
    (1..=total)
        .map(|step| {
            if forward {
                (from + step) % total
            } else {
                (from + total - step) % total
            }
        })
        .find(|&index| contains_match(labels[index].as_ref(), query))
}

/// What a search over rows comes to with the cursor on row `at`: the rows
/// holding `query` are the matches, since a row is what `n` steps to.
pub fn row_search_report(
    labels: &[impl AsRef<str>],
    query: &str,
    at: usize,
    reverse: bool,
) -> SearchReport {
    let mut report = SearchReport {
        query: query.to_string(),
        reverse,
        ..SearchReport::default()
    };
    for (row, label) in labels.iter().enumerate() {
        if contains_match(label.as_ref(), query) {
            report.total += 1;
            if row == at {
                report.index = report.total;
            }
        }
    }
    report
}

/// Whether `query` occurs anywhere in `text`, by [`match_spans`]'s rule.
pub fn contains_match(text: &str, query: &str) -> bool {
    !match_spans(text, query).is_empty()
}

// ========================================================================
// JumpTarget
// ========================================================================

impl JumpTarget {
    /// A place called `label`, drawn without an icon.
    pub fn new(target: usize, label: String) -> Self {
        Self {
            icon: None,
            label,
            target,
        }
    }

    /// The same place, drawn with `icon` before its label.
    pub fn with_icon(self, icon: IconKind) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }
}

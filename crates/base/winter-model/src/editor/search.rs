//! Searching an editor's rows, and the jumps a search or a link lands on.

use std::path::PathBuf;

use regex::{Regex, RegexBuilder};

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

/// A text search over a directory tree, run without a pattern engine: the
/// query is literal text, read the way [`SearchPattern`] says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    /// How a text query is read, past finding it as it is: case, regular
    /// expressions, whole words, and which files are looked in.
    pub options: SearchOptions,
    /// How the query is matched against a line.
    pub pattern: SearchPattern,
    /// The text every reported line holds, or the words one of which it holds.
    pub query: String,
    /// Where the walk starts.
    pub root: PathBuf,
}

/// How a [`SearchPattern::Text`] query is read, as the Grep editor's toggles
/// set it: the default finds the text as it is, in any case, in any file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchOptions {
    /// Whether case counts: off, `foo` finds `Foo`.
    pub case_sensitive: bool,
    /// Only the files whose name this glob matches (`*.rs`), or every file.
    pub glob: Option<String>,
    /// Whether the text of PDF files is searched, page by page, rather than
    /// text files: each hit's line is then the page it is on.
    pub pdf: bool,
    /// Whether the query is a regular expression rather than text.
    pub regex: bool,
    /// Whether what the query matches must stand as a whole word.
    pub whole_word: bool,
}

/// How a [`SearchRequest`]'s query is matched against a line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchPattern {
    /// The query as one run of text, found anywhere in the line, whatever the
    /// case: what a person typing a search means.
    Text,
    /// The query as words split on whitespace, any one of which the line holds
    /// as a whole word in exactly that case: `TODO` finds `// TODO: x` and
    /// `TODO(me)`, never `todos` or `TODOS`. What a list of comment keywords
    /// means.
    Words,
}

impl SearchRequest {
    /// Look under `root` for lines `query` matches, read as `pattern` says.
    pub fn new(pattern: SearchPattern, query: impl Into<String>, root: PathBuf) -> Self {
        Self {
            options: SearchOptions::default(),
            pattern,
            query: query.into(),
            root,
        }
    }

    /// The same search, reading its query as `options` say.
    pub fn with_options(self, options: SearchOptions) -> Self {
        Self { options, ..self }
    }

    /// The words a [`SearchPattern::Words`] query stands for, without blanks.
    pub fn split_words(&self) -> Vec<&str> {
        self.query.split_whitespace().collect()
    }
}

/// What a search found. A walk that hit its cap reports what it had rather
/// than failing, since a first page of matches is still worth showing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResult {
    /// Why nothing was looked for, for a query that cannot be read: a
    /// regular expression that does not parse.
    pub error: Option<String>,
    /// The matching lines, in the order the walk found them.
    pub hits: Vec<SearchHit>,
    /// The query this answers, so an editor that has moved on to another one can
    /// tell that this is not about the query it is showing.
    pub query: String,
    /// Whether the cap stopped the walk with matches left unreported.
    pub truncated: bool,
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

/// The search an editor just ran or repeated, which the host highlights in the
/// tab and counts in the status bar the way it does a terminal's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchReport {
    /// The cursor's place among the matches, counting from one, or 0 when it
    /// is on none.
    pub index: usize,
    /// The text looked for.
    pub query: String,
    /// Whether the search runs backwards, so the bar shows `?` rather than `/`.
    pub reverse: bool,
    /// How many matches there are, or 0 when the editor does not count them.
    pub total: usize,
}

// ========================================================================
// Functions
// ========================================================================

/// `query` as the regular expression a text search reads it as under
/// `options`: escaped unless it is one already, bounded by word edges for a
/// whole-word search, and in any case unless case counts. What a search
/// finds and what a replacement replaces are then the same text.
pub fn text_regex(query: &str, options: &SearchOptions) -> Result<Regex, String> {
    let pattern = match options.regex {
        true => query.to_string(),
        false => regex::escape(query),
    };
    let pattern = match options.whole_word {
        true => format!(r"\b(?:{pattern})\b"),
        false => pattern,
    };
    RegexBuilder::new(&pattern)
        .case_insensitive(!options.case_sensitive)
        .build()
        .map_err(|e| format!("not a regular expression: {e}"))
}

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

/// Every place `query` occurs in `text`, as `(start, end)` character columns,
/// overlapping ones included. A query typed in lower case matches either
/// case, and one with a capital in it matches exactly: the rule every editor
/// searches by, so what a search lands on is what is highlighted.
pub fn match_spans(text: &str, query: &str) -> Vec<(usize, usize)> {
    let folded = !query.chars().any(char::is_uppercase);
    // Folded a character at a time, so a column of the folded text is a
    // column of the text even where lowering a letter would lengthen it.
    let fold = |s: &str| -> Vec<char> {
        s.chars()
            .map(|c| match folded {
                true => c.to_lowercase().next().unwrap_or(c),
                false => c,
            })
            .collect()
    };
    let needle = fold(query);
    let haystack = fold(text);
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&start| haystack[start..].starts_with(&needle))
        .map(|start| (start, start + needle.len()))
        .collect()
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

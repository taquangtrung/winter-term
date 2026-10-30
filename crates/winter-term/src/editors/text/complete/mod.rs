//! Completion: what could go at the cursor, gathered from back-ends the way
//! Emacs' company and cape gather it, each answering on its own terms, and
//! ranked against what is typed.
//!
//! A back-end looks at the text before the cursor and says where the text
//! it would replace starts and what it could be replaced with: [`words`] in
//! the open buffers, [`paths`] on disk, [`snippets`] of the language, its
//! [`keywords`], and what [`latex`] and [`markdown`] documents name; and
//! each extension's, by its full id. The back-ends a language asks, in
//! order, are a setting; the first one with
//! anything to offer decides where the replacement starts, and every later
//! one answering at that same spot is merged in under it.
//!
//! Everything here reads text and the files a document names, and changes
//! nothing, so it is tested on text alone.

pub(super) mod keywords;
pub(super) mod latex;
pub(super) mod markdown;
pub(super) mod paths;
pub(super) mod snippets;
pub(super) mod words;

use std::panic::{self, AssertUnwindSafe};
use std::path::Path;

use winter_extension::{CompletionRequest, CompletionSource};

// ========================================================================
// Constants
// ========================================================================

/// The most candidates the popup is handed, past which narrowing is what
/// the reader wants rather than scrolling.
const MAX_CANDIDATES: usize = 200;

/// The back-ends a language without a setting of its own asks, in order.
/// A language server comes first where one is running, and answers
/// nothing where none is.
pub(crate) const DEFAULT_BACKENDS: &[&str] = &["lsp", "snippets", "keywords", "paths", "words"];

/// The back-ends a LaTeX and a Markdown document ask by default.
const LATEX_BACKENDS: &[&str] = &["latex", "lsp", "snippets", "paths", "words"];
const MARKDOWN_BACKENDS: &[&str] = &["markdown", "lsp", "snippets", "paths", "words"];

// ========================================================================
// Data Structures
// ========================================================================

/// What a candidate is, for the letter the popup marks it with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Kind {
    /// A citation key from a bibliography.
    Citation,
    /// A LaTeX command.
    Command,
    /// A directory.
    Directory,
    /// A LaTeX environment.
    Environment,
    /// What an extension's back-end offered.
    Extension,
    /// A file.
    File,
    /// A Markdown heading's anchor.
    Heading,
    /// A language's keyword.
    Keyword,
    /// What a language server offered.
    Server(ServerKind),
    /// A LaTeX label.
    Label,
    /// A snippet, expanded where it is chosen.
    Snippet,
    /// A word found in an open buffer.
    Word,
}

/// What a language server said a candidate is: the letter the popup marks
/// it with, and whether what goes in is a snippet to expand.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ServerKind {
    pub(crate) mark: char,
    pub(crate) snippet: bool,
}

/// One thing that could go at the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Candidate {
    /// More about it, drawn dim beside it: a snippet's description, a
    /// citation's title.
    pub(crate) detail: Option<String>,
    /// What goes in, in snippet syntax for a snippet (see [`expand`]).
    pub(crate) insert: String,
    /// What it is.
    pub(crate) kind: Kind,
    /// What the popup shows.
    pub(crate) label: String,
    /// The back-end that offered it.
    pub(crate) source: &'static str,
}

/// What a back-end looks at: the text around the cursor and where the
/// document is.
pub(crate) struct Context<'a> {
    /// The cursor's column on its line, in characters.
    pub(crate) col: usize,
    /// The language the document is written in, as settings name it.
    pub(crate) language: &'a str,
    /// The cursor's line.
    pub(crate) line: &'a str,
    /// Every line of the buffer.
    pub(crate) lines: &'a [String],
    /// The lines of every other open buffer.
    pub(crate) others: &'a [&'a [String]],
    /// Where the document is on disk.
    pub(crate) path: &'a Path,
    /// The cursor's row.
    pub(crate) row: usize,
    /// What a language server last offered here, the `lsp` back-end's
    /// answer: asked for ahead, since a server answers in its own time.
    pub(crate) server: Option<&'a Reply>,
}

/// A back-end's answer: the column the text it would replace starts at,
/// and what it could be replaced with.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct Reply {
    pub(crate) candidates: Vec<Candidate>,
    pub(crate) start: usize,
}

/// What the gathered back-ends offer at the cursor, ranked against what is
/// typed.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Offer {
    pub(crate) candidates: Vec<Candidate>,
    /// The column the replaced text starts at.
    pub(crate) start: usize,
}

/// A snippet put in: its text, line by line, and where in it the cursor
/// goes, as a row of the text and a column.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Expansion {
    pub(crate) cursor: (usize, usize),
    pub(crate) lines: Vec<String>,
}

// ========================================================================
// Context
// ========================================================================

impl Context<'_> {
    /// The characters of the cursor's line before the cursor.
    pub(crate) fn before(&self) -> Vec<char> {
        self.line.chars().take(self.col).collect()
    }

    /// Where the word ending at the cursor starts: letters, digits, and `_`
    /// back from it.
    pub(crate) fn word_start(&self) -> usize {
        let before = self.before();
        let mut start = before.len();
        while start > 0 && is_word_char(before[start - 1]) {
            start -= 1;
        }
        start
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Each extension's back-end, by its full id.
fn extension_backends() -> impl Iterator<Item = &'static str> {
    crate::extensions::registry()
        .completions()
        .iter()
        .map(|source| source.id.as_str())
}

/// The back-ends `language` asks, from the setting where it names some;
/// else its own defaults, then every extension's.
pub(crate) fn backends_for<'a>(language: &str, setting: Option<&'a [String]>) -> Vec<&'a str> {
    if let Some(named) = setting {
        return named.iter().map(String::as_str).collect();
    }
    let defaults = match language {
        "latex" => LATEX_BACKENDS,
        "markdown" => MARKDOWN_BACKENDS,
        _ => DEFAULT_BACKENDS,
    };
    defaults.iter().copied().chain(extension_backends()).collect()
}

/// What `backends` offer at the cursor `context` describes, the first one
/// with an answer deciding where the replaced text starts, ranked against
/// what is typed there. `None` where nothing is offered.
pub(crate) fn gather(context: &Context, backends: &[&str]) -> Option<Offer> {
    let mut start = None;
    let mut found: Vec<Candidate> = Vec::new();
    for name in backends {
        let Some(reply) = ask(name, context) else {
            continue;
        };
        if reply.candidates.is_empty() {
            continue;
        }
        match start {
            None => start = Some(reply.start),
            Some(at) if at != reply.start => continue,
            Some(_) => {}
        }
        found.extend(reply.candidates);
    }
    let start = start?;
    let typed: String = context.before()[start.min(context.col)..].iter().collect();
    let candidates = rank(found, &typed);
    (!candidates.is_empty()).then_some(Offer { candidates, start })
}

/// What back-end `name` answers at the cursor, if it is one this knows.
fn ask(name: &str, context: &Context) -> Option<Reply> {
    match name {
        "keywords" => keywords::complete(context),
        "latex" => latex::complete(context),
        "markdown" => markdown::complete(context),
        "paths" => paths::complete(context),
        "snippets" => snippets::complete(context),
        "words" => words::complete(context),
        "lsp" => context.server.cloned(),
        other => ask_extension(crate::extensions::completion(other)?, context),
    }
}

/// What the extension back-end `source` offers at the cursor. One that
/// panics offers nothing, and completion goes on without it.
fn ask_extension(source: &'static CompletionSource, context: &Context) -> Option<Reply> {
    let before: String = context.before().into_iter().collect();
    let request = CompletionRequest::new(
        context.path,
        context.language,
        context.line,
        (context.row as u32, utf16_len(&before)),
    );
    let asked = panic::catch_unwind(AssertUnwindSafe(|| (source.complete)(&request)));
    let offered = asked.ok().flatten()?;
    let candidates = offered
        .items
        .into_iter()
        .map(|item| Candidate {
            detail: item.detail,
            insert: item.insert,
            kind: Kind::Extension,
            label: item.label,
            source: source.id.as_str(),
        })
        .collect();
    // The start, from UTF-16 units back to characters, and never past
    // the cursor.
    let start = before
        .chars()
        .scan(0, |units, c| {
            let at = *units;
            *units += c.len_utf16() as u32;
            Some(at)
        })
        .take_while(|at| *at < offered.start)
        .count();
    Some(Reply { candidates, start })
}

/// How many UTF-16 code units `text` takes.
fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// `found` narrowed to what `typed` could be the start of, or spells in
/// order, the closest first: an exact start, then a start in another case,
/// then the letters in order. Within each, the back-ends' own order holds.
/// Each text is offered once, by the first back-end that offered it.
pub(crate) fn rank(found: Vec<Candidate>, typed: &str) -> Vec<Candidate> {
    let lower = typed.to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut scored: Vec<(u8, usize, Candidate)> = found
        .into_iter()
        .filter(|candidate| candidate.label != typed || candidate.kind == Kind::Snippet)
        .filter(|candidate| seen.insert(candidate.label.clone()))
        .enumerate()
        .filter_map(|(order, candidate)| {
            let label = candidate.label.as_str();
            let score = if label.starts_with(typed) {
                0
            } else if label.to_lowercase().starts_with(&lower) {
                1
            } else if spells(&label.to_lowercase(), &lower) {
                2
            } else {
                return None;
            };
            Some((score, order, candidate))
        })
        .collect();
    scored.sort_by_key(|(score, order, _)| (*score, *order));
    scored
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, _, candidate)| candidate)
        .collect()
}

/// Whether `label` holds the characters of `typed` in order.
fn spells(label: &str, typed: &str) -> bool {
    let mut letters = label.chars();
    typed.chars().all(|c| letters.any(|l| l == c))
}

/// Whether `c` is part of a word: letters, digits, `_`.
pub(crate) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `body`, a snippet, as the lines it puts in, each after the first
/// indented as `indent`, and where the cursor goes: the first field (`$1`,
/// `${1:default}`), else the end mark (`$0`), else the end. A field's
/// default is put in as text; a `$` not starting one is kept.
pub(crate) fn expand(body: &str, indent: &str) -> Expansion {
    let mut text = String::new();
    let mut marks: Vec<(u32, usize)> = Vec::new();
    let chars: Vec<char> = body.chars().collect();
    let mut at = 0;
    while at < chars.len() {
        let c = chars[at];
        if c != '$' {
            text.push(c);
            at += 1;
            continue;
        }
        let digits: String = chars[at + 1..].iter().take_while(|c| c.is_ascii_digit()).collect();
        if !digits.is_empty() {
            marks.push((digits.parse().unwrap_or(0), text.chars().count()));
            at += 1 + digits.len();
            continue;
        }
        if chars.get(at + 1) == Some(&'{') {
            let close = chars[at + 2..].iter().position(|c| *c == '}');
            let inner: Option<String> = close.map(|end| chars[at + 2..at + 2 + end].iter().collect());
            if let Some((number, default)) = inner.as_deref().and_then(|inner| inner.split_once(':'))
                .and_then(|(number, default)| Some((number.parse::<u32>().ok()?, default.to_string())))
            {
                marks.push((number, text.chars().count()));
                text.push_str(&default);
                at += 2 + close.unwrap_or(0) + 1;
                continue;
            }
        }
        text.push(c);
        at += 1;
    }
    let total = text.chars().count();
    let cursor_at = marks
        .iter()
        .filter(|(number, _)| *number > 0)
        .min_by_key(|(number, _)| *number)
        .or_else(|| marks.iter().find(|(number, _)| *number == 0))
        .map_or(total, |(_, at)| *at);
    let mut lines = Vec::new();
    let mut cursor = (0, 0);
    let mut seen = 0;
    for (row, line) in text.split('\n').enumerate() {
        let length = line.chars().count();
        let shown = match row {
            0 => line.to_string(),
            _ => format!("{indent}{line}"),
        };
        if cursor_at >= seen && cursor_at <= seen + length {
            let shift = if row == 0 { 0 } else { indent.chars().count() };
            cursor = (row, cursor_at - seen + shift);
        }
        seen += length + 1;
        lines.push(shown);
    }
    Expansion { cursor, lines }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn context<'a>(line: &'a str, lines: &'a [String], language: &'a str) -> Context<'a> {
        Context {
            col: line.chars().count(),
            language,
            line,
            lines,
            others: &[],
            path: Path::new("/nowhere/a.txt"),
            row: 0,
            server: None,
        }
    }

    fn candidate(label: &str, source: &'static str) -> Candidate {
        Candidate {
            detail: None,
            insert: label.to_string(),
            kind: Kind::Word,
            label: label.to_string(),
            source,
        }
    }

    #[test]
    fn test_an_exact_start_ranks_first_then_another_case_then_the_letters_in_order() {
        let found = ["format_string", "Formatter", "fmt", "for_each", "other"]
            .map(|label| candidate(label, "words"))
            .to_vec();
        let ranked: Vec<String> = rank(found, "fo").into_iter().map(|c| c.label).collect();
        assert_eq!(ranked, ["format_string", "for_each", "Formatter"]);
        let found = ["fmt", "format"].map(|label| candidate(label, "words")).to_vec();
        let ranked: Vec<String> = rank(found, "fmt").into_iter().map(|c| c.label).collect();
        assert_eq!(ranked, ["format"], "the word typed is not offered back");
    }

    #[test]
    fn test_the_first_backend_to_answer_decides_and_others_join_at_its_spot() {
        let lines = vec!["let value = val".to_string(), "valid".to_string()];
        let context = Context { row: 0, ..context("let value = val", &lines, "rust") };
        let offer = gather(&context, &["paths", "words"]).expect("words answer");
        assert_eq!(offer.start, 12);
        let labels: Vec<&str> = offer.candidates.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["value", "valid"]);
        assert!(gather(&context, &["lsp"]).is_none(), "nothing from a server not started");
        let served = Reply {
            candidates: vec![candidate("validate", "lsp")],
            start: 12,
        };
        let context = Context { server: Some(&served), ..context };
        let offer = gather(&context, &["lsp", "words"]).expect("the server answers");
        let labels: Vec<&str> = offer.candidates.iter().map(|c| c.label.as_str()).collect();
        assert_eq!(labels, ["validate", "value", "valid"], "the server's first, the words after");
    }

    #[test]
    fn test_an_extension_s_backend_answers_by_its_id_with_its_columns_in_characters() {
        use crate::extensions::tests::OFFERED;
        let lines = vec!["é = valu".to_string()];
        let context = context("é = valu", &lines, "rust");
        let offer = gather(&context, &["probe.words"]).expect("the probe answers");
        assert_eq!(offer.start, 4, "its UTF-16 start, as a column in characters");
        assert_eq!(offer.candidates[0].label, OFFERED);
        assert_eq!(offer.candidates[0].source, "probe.words");
        assert!(gather(&context, &["probe.none"]).is_none(), "no such back-end");
        assert!(crate::config::completion_backends().contains(&"probe.words"), "a setting may name it");
    }

    #[test]
    fn test_a_snippet_puts_its_fields_defaults_in_and_the_cursor_on_the_first() {
        let expansion = expand("fn ${1:name}() {\n    $0\n}", "  ");
        assert_eq!(expansion.lines, ["fn name() {", "      ", "  }"]);
        assert_eq!(expansion.cursor, (0, 3), "on the first field");
        let end = expand("\\begin{itemize}\n\\item $0\n\\end{itemize}", "");
        assert_eq!(end.cursor, (1, 6), "on the end mark");
        assert_eq!(expand("costs $5 or ${x}", "").lines, ["costs  or ${x}"]);
        assert_eq!(expand("plain", "").cursor, (0, 5));
    }

    #[test]
    fn test_a_language_asks_its_own_backends_or_the_setting_s() {
        assert_eq!(backends_for("latex", None)[0], "latex");
        let defaults = backends_for("rust", None);
        assert_eq!(defaults[..DEFAULT_BACKENDS.len()], *DEFAULT_BACKENDS);
        assert_eq!(defaults[DEFAULT_BACKENDS.len()..], ["probe.words"], "then the extensions'");
        let setting = vec!["words".to_string()];
        assert_eq!(backends_for("rust", Some(&setting)), ["words"]);
    }
}

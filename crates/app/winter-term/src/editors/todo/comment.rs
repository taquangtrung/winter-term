//! Whether a keyword the search found is a todo: in a comment, as the
//! file's grammar reads it, and either the comment's first word on its line
//! (`// TODO x`, `# FIXME`, `/** @TODO */`, a block comment's inner line) or
//! marked as a todo with `:` or `(` past it (`// parse it; TODO: cache it`).
//! The keyword in code, in a string, or in a comment's sentence (`// the
//! TODO list`) is not, nor is any in a file with no grammar.

use std::ops::Range;
use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// What may stand between a comment's start on its line and its first word
/// besides spaces: the rest of `///`, `//!`, `/**`, `##`, `;;`, Doxygen's
/// `///<`, a block's `*` at the head of its inner lines, the `@` of `@TODO`.
const HEAD_MARKS: &[char] = &['/', '*', '!', '<', '#', '-', ';', '%', '[', '@'];

/// What may follow a comment's first word as a keyword: `TODO:`,
/// `TODO(me)`, `TODO!`, `TODO - x`, `TODO [x]`, the `*/` closing it, or a
/// space.
const HEAD_ENDS: &[char] = &[':', '(', '!', '-', '[', '*'];

/// What marks a keyword in the middle of a comment as a todo: `TODO:`,
/// `TODO(me)`.
const MARKED_ENDS: &[char] = &[':', '('];

// ========================================================================
// Data Structures
// ========================================================================

/// The comments of one file at a time, by line, the file read and
/// tokenized once for the hits of it in a row.
#[derive(Debug, Default)]
pub(crate) struct Comments {
    /// Each line's comments, as byte ranges of the line.
    lines: Vec<Vec<Range<usize>>>,
    /// The file read.
    path: PathBuf,
}

// ========================================================================
// Comments Implementations
// ========================================================================

impl Comments {
    /// The comments on line `line`, counting from one, of the file at
    /// `path`: none for a file that cannot be read or has no grammar.
    pub(super) fn on_line(&mut self, path: &Path, line: usize) -> &[Range<usize>] {
        if self.path != path {
            self.path = path.to_path_buf();
            self.lines = std::fs::read_to_string(path)
                .map(|text| comments_by_line(path, &text))
                .unwrap_or_default();
        }
        line.checked_sub(1)
            .and_then(|at| self.lines.get(at))
            .map_or(&[], Vec::as_slice)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where `text`, a line whose comments are `comments`, holds one of
/// `keywords` as a todo: the first such keyword's place in `keywords` and
/// where it starts.
pub(super) fn todo_in(
    text: &str,
    comments: &[Range<usize>],
    keywords: &[String],
) -> Option<(usize, usize)> {
    keywords
        .iter()
        .enumerate()
        .filter_map(|(at, keyword)| {
            whole_words(text, keyword)
                .find(|&start| is_todo(text, comments, start, keyword))
                .map(|start| (at, start))
        })
        .min_by_key(|(_, start)| *start)
}

/// Where `word` stands whole in `text`, in its own case.
pub(super) fn whole_words<'t>(text: &'t str, word: &'t str) -> impl Iterator<Item = usize> + 't {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word)
        .map(|(at, _)| at)
        .filter(move |&at| {
            let before = text[..at].chars().next_back();
            let after = text[at + word.len()..].chars().next();
            !before.is_some_and(is_word) && !after.is_some_and(is_word)
        })
}

/// Whether `word` at `at` in `text` is a todo, in one of `comments`.
fn is_todo(text: &str, comments: &[Range<usize>], at: usize, word: &str) -> bool {
    let end = at + word.len();
    let Some(comment) = comments
        .iter()
        .find(|comment| comment.start <= at && end <= comment.end)
    else {
        return false;
    };
    let head = text[comment.start..at]
        .chars()
        .all(|c| c.is_whitespace() || HEAD_MARKS.contains(&c));
    let next = text[end..].chars().next();
    (head && next.is_none_or(|c| c.is_whitespace() || HEAD_ENDS.contains(&c)))
        || next.is_some_and(|c| MARKED_ENDS.contains(&c))
}

/// The comments of `text`, the file at `path`, as its grammar reads them:
/// each line's, as byte ranges of the line. None for a text with no grammar.
fn comments_by_line(path: &Path, text: &str) -> Vec<Vec<Range<usize>>> {
    let Some(comments) = winter_text::syntax::grammars().comments(path, text) else {
        return Vec::new();
    };
    let starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(at, _)| at + 1))
        .collect();
    let mut lines = vec![Vec::new(); starts.len()];
    for comment in comments {
        // A comment never spans a line break, so its start names its line.
        let line = starts.partition_point(|&start| start <= comment.start) - 1;
        lines[line].push(comment.start - starts[line]..comment.end - starts[line]);
    }
    lines
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    /// Whether `TODO` on line `line` of `source`, a file named `file`, is a
    /// todo, as the search reads the file.
    fn todo_on(file: &str, source: &str, line: usize) -> bool {
        static CALLS: AtomicUsize = AtomicUsize::new(0);
        let call = CALLS.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("winter-todo-{}-{call}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("a temporary directory");
        let path = dir.join(file);
        std::fs::write(&path, source).expect("the file");
        let text = source.lines().nth(line - 1).expect("the line");
        let found = todo_in(
            text,
            Comments::default().on_line(&path, line),
            &["TODO".to_string()],
        );
        let _ = std::fs::remove_dir_all(&dir);
        found.is_some()
    }

    /// Whether `TODO` in `text`, the only line of `file`, is a todo.
    fn todo(file: &str, text: &str) -> bool {
        todo_on(file, text, 1)
    }

    #[test]
    fn test_the_keyword_opening_a_comment_is_a_todo() {
        assert!(todo("a.rs", "// TODO: cache the parse"));
        assert!(todo("a.rs", "    let x = 1; // TODO(me): name it"));
        assert!(todo("a.rs", "f();// TODO"));
        assert!(todo("a.rs", "//! TODO"));
        assert!(todo("a.rs", "/** @TODO later */"));
        assert!(todo("a.py", "x = 1  # TODO - off by one"));
        assert!(todo("a.lua", "--[[ TODO ]]"));
        assert!(todo("a.html", "<!-- TODO: alt text -->"));
        assert!(todo("Makefile", "# TODO"));
    }

    #[test]
    fn test_the_keyword_marked_in_the_middle_of_a_comment_is_a_todo() {
        assert!(todo("a.rs", "// parse it here; TODO: cache it"));
        assert!(todo("a.py", "# slow, TODO(me) later"));
        assert!(todo("a.rs", "/* parse it, TODO: cache it */"));
        assert!(!todo("a.rs", "// the TODO list"), "a word of a sentence");
        assert!(
            !todo("a.rs", "// fix the TODO later"),
            "a word of a sentence"
        );
    }

    #[test]
    fn test_the_keyword_in_code_or_a_string_is_none() {
        assert!(!todo("a.rs", "const TODO: &str = \"x\";"), "code");
        assert!(
            !todo("a.rs", "hit(\"a.rs\", 3, \"// TODO: one\"),"),
            "a string"
        );
        assert!(
            !todo("a.rs", "let s = '\"'; let t = \"// TODO: x\";"),
            "a quote in a char"
        );
        assert!(!todo("a.rs", "/* x */ TODO: y"), "after the comment closes");
        assert!(!todo("a.rs", "// TODO.md lists them"), "a file name");
        assert!(!todo("a.md", "# TODO"), "a Markdown heading");
        assert!(!todo("a.json", "{\"TODO\": 1}"), "JSON has no comments");
        assert!(!todo("a.unknown-kind", "# TODO: x"), "no grammar");
    }

    #[test]
    fn test_a_block_comments_inner_lines_hold_todos() {
        let rust = "fn f() {}\n/*\n   TODO: cache it\n   the TODO list\n*/\nlet s = TODO;\n";
        assert!(todo_on("a.rs", rust, 3), "the head of an inner line");
        assert!(!todo_on("a.rs", rust, 4), "a word of a sentence");
        assert!(!todo_on("a.rs", rust, 6), "code past the block");
        let python = "def f():\n    \"\"\"Parse it.\n\n    TODO: cache it\n    \"\"\"\n";
        assert!(todo_on("a.py", python, 4), "a docstring");
        let html = "<!--\n  TODO alt text\n-->\n";
        assert!(todo_on("a.html", html, 2));
        let quoted = "let s = \"/*\";\n    TODO: not in a comment\n";
        assert!(!todo_on("a.rs", quoted, 2), "a block opened in a string");
    }
}

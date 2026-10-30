//! Comments in source code: `//` and `/* */`, `#`, or `--`, by language.
//!
//! The scan is lexical, as in magic-vscode: a comment marker inside a
//! string literal also starts a comment. A marker escaped by a backslash
//! does not.

use std::ops::Range;

use super::scan::{find_from, line_end};
use super::CommentStyle;

// ========================================================================
// Constants
// ========================================================================

/// Opens a block comment in slash-style languages.
const BLOCK_OPEN: &str = "/*";

/// Closes a block comment in slash-style languages.
const BLOCK_CLOSE: &str = "*/";

// ========================================================================
// Scanning
// ========================================================================

/// The comments of `code` in `style`, in order, each from its marker to
/// the end of its line or block.
pub(super) fn comment_ranges(code: &str, style: CommentStyle) -> Vec<Range<usize>> {
    let mut comments = Vec::new();
    let mut from = 0;
    while let Some((comment, end)) = next_comment(code, from, style) {
        comments.push(comment);
        from = end;
    }
    comments
}

/// The first comment at or after `from`, and where its match ends: past
/// the line break of a line comment.
fn next_comment(code: &str, from: usize, style: CommentStyle) -> Option<(Range<usize>, usize)> {
    let hay = &code[from..];
    let (comment, end) = hay
        .char_indices()
        .find_map(|(at, _)| comment_at(hay, at, style))?;
    Some((from + comment.start..from + comment.end, from + end))
}

/// A comment whose match starts at `at`. Like the source regexes, a match
/// starts one character before its marker unless the marker opens the
/// text; for a line comment that character is no backslash, and for a
/// block comment it is whitespace.
fn comment_at(hay: &str, at: usize, style: CommentStyle) -> Option<(Range<usize>, usize)> {
    let line_marker = match style {
        CommentStyle::Dash => "--",
        CommentStyle::Hash => "#",
        CommentStyle::Slash => "//",
    };
    let line = marker_start(hay, at, line_marker, |c| c != '\\').map(|start| {
        let end = line_end(hay, start);
        (start..end, (end + 1).min(hay.len()))
    });
    let block = || match style {
        CommentStyle::Slash => {
            let start = marker_start(hay, at, BLOCK_OPEN, char::is_whitespace)?;
            let close = find_from(hay, start + BLOCK_OPEN.len(), BLOCK_CLOSE)?;
            let end = close + BLOCK_CLOSE.len();
            Some((start..end, end))
        }
        CommentStyle::Dash | CommentStyle::Hash => None,
    };
    line.or_else(block)
}

/// Where `marker` starts in a match at `at`: at `at` itself when that
/// opens the text, else right after the character at `at`, which must
/// satisfy `is_allowed_before`.
fn marker_start(
    hay: &str,
    at: usize,
    marker: &str,
    is_allowed_before: impl Fn(char) -> bool,
) -> Option<usize> {
    if at == 0 && hay.starts_with(marker) {
        return Some(at);
    }
    let before = hay[at..].chars().next()?;
    let start = at + before.len_utf8();
    (is_allowed_before(before) && hay[start..].starts_with(marker)).then_some(start)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn comments(code: &str, style: CommentStyle) -> Vec<&str> {
        comment_ranges(code, style)
            .into_iter()
            .map(|range| &code[range])
            .collect()
    }

    #[test]
    fn test_comment_ranges_finds_slash_line_and_block_comments() {
        let code = "let x = 1; // the count\n/// docs here\nf(x); /* a block\n note */ g();";
        assert_eq!(
            comments(code, CommentStyle::Slash),
            ["// the count", "/// docs here", "/* a block\n note */"]
        );
    }

    #[test]
    fn test_comment_ranges_needs_whitespace_before_block_comment() {
        assert!(comments("a/* glued */", CommentStyle::Slash).is_empty());
    }

    #[test]
    fn test_comment_ranges_skips_escaped_marker() {
        assert_eq!(
            comments("s = '\\# no'\n# yes", CommentStyle::Hash),
            ["# yes"]
        );
    }

    #[test]
    fn test_comment_ranges_finds_hash_and_dash_comments() {
        assert_eq!(
            comments("x = 1  # set x\n#!shebang", CommentStyle::Hash),
            ["# set x", "#!shebang"]
        );
        assert_eq!(
            comments("SELECT 1; -- one row\n", CommentStyle::Dash),
            ["-- one row"]
        );
    }

    #[test]
    fn test_comment_ranges_handles_multibyte_text_before_marker() {
        assert_eq!(
            comments("é// café note", CommentStyle::Slash),
            ["// café note"]
        );
    }
}

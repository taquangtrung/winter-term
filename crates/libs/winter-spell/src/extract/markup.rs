//! BibTeX and Markdown prose.
//!
//! BibTeX keeps the field values worth reading (titles, notes, venues)
//! and drops entry heads, people, identifiers, and the braces around it
//! all. Markdown drops code (but keeps the comments of a fenced block in
//! a known language), math, HTML tags, and URLs, and unwraps emphasis.

use std::iter;
use std::ops::Range;

use super::comment_style_for_fence;
use super::comments::comment_ranges;
use super::scan::{
    char_before, find_from, inline_math_end, is_after_non_word, is_before_non_word, is_byte_at,
    line_end, url_end, whitespace_end, Hit, Mask,
};

// ========================================================================
// Constants
// ========================================================================

/// BibTeX fields that name people or identify things, never prose.
/// `volumn` is the source's spelling, kept so behavior matches.
const IGNORED_FIELDS: [&str; 6] = ["author", "editor", "volumn", "doi", "biburl", "bibsource"];

/// Opens and closes a fenced code block.
const FENCE: &str = "```";

/// Opens and closes a display math block.
const DISPLAY_MATH: &str = "$$";

/// Emphasis delimiters, strongest first so `**` is not read as two `*`.
const EMPHASIS_DELIMITERS: [&str; 4] = ["**", "__", "*", "_"];

// ========================================================================
// BibTeX
// ========================================================================

/// Drops everything in `mask` that is BibTeX structure rather than prose.
pub(super) fn drop_bibtex_markup(mask: &mut Mask) {
    // Entry heads: `@article{key,` lines.
    mask.apply(|hay, at| {
        line_starts(hay, at).find_map(|start| {
            let head = whitespace_end(hay, start);
            is_byte_at(hay, head, b'@').then(|| {
                let end = line_end_with_break(hay, head);
                Hit::dropping(start..end)
            })
        })
    });
    mask.apply(|hay, at| {
        line_starts(hay, at)
            .find_map(|start| ignored_field_end(hay, start))
            .map(|end| Hit::dropping(at..end).spaced())
    });
    // The `name = {` opening every other field, back to the line start.
    mask.apply(|hay, at| {
        line_starts(hay, at).find_map(|start| {
            let line = &hay[start..line_end(hay, start)];
            line.rmatch_indices('=').find_map(|(index, _)| {
                let brace = whitespace_end(hay, start + index + 1);
                is_byte_at(hay, brace, b'{').then(|| Hit::dropping(start..brace + 1))
            })
        })
    });
    // Closing braces at a line end, with trailing commas and spaces.
    mask.apply(|hay, at| {
        if !is_byte_at(hay, at, b'}') {
            return None;
        }
        let end = hay[at + 1..]
            .bytes()
            .position(|byte| byte != b',' && byte != b' ')
            .map_or(hay.len(), |index| at + 1 + index);
        (end == hay.len() || is_byte_at(hay, end, b'\n'))
            .then(|| Hit::new(iter::once(at..end), line_end_with_break(hay, end)))
    });
    // Inner braces, unwrapped.
    mask.apply(|hay, at| {
        if !is_byte_at(hay, at, b'{') {
            return None;
        }
        let close = find_from(hay, at + 1, "}")?;
        Some(Hit::new(vec![at..at + 1, close..close + 1], close + 1))
    });
}

/// Where an ignored field starting on the line at `start` ends: the last
/// such field name on the line, its `=`, and its value through the first
/// `}` that ends the text or is followed by a comma and a line break.
fn ignored_field_end(hay: &str, start: usize) -> Option<usize> {
    let line_end = line_end(hay, start);
    (start..line_end)
        .rev()
        .filter(|&index| hay.is_char_boundary(index))
        .find_map(|index| {
            IGNORED_FIELDS.iter().find_map(|field| {
                if !hay[index..].starts_with(field) {
                    return None;
                }
                let equals = whitespace_end(hay, index + field.len());
                is_byte_at(hay, equals, b'=')
                    .then(|| field_value_end(hay, equals + 1))
                    .flatten()
            })
        })
}

/// Past a field value from `from`: through the first `}` after which
/// only whitespace runs to the text's end, or a comma follows and then
/// whitespace holding a line break, which ends the match.
fn field_value_end(hay: &str, from: usize) -> Option<usize> {
    hay[from..].match_indices('}').find_map(|(index, _)| {
        let after = whitespace_end(hay, from + index + 1);
        if after == hay.len() {
            return Some(after);
        }
        if !is_byte_at(hay, after, b',') {
            return None;
        }
        let space_end = whitespace_end(hay, after + 1);
        hay[after + 1..space_end]
            .rfind('\n')
            .map(|newline| after + 1 + newline + 1)
    })
}

/// Where a line can start for a match at `at`: at `at` when it opens the
/// text, and past `at` when it holds a line break, as `(^|\n)` matches.
fn line_starts(hay: &str, at: usize) -> impl Iterator<Item = usize> {
    let opens_text = (at == 0).then_some(at);
    let after_break = is_byte_at(hay, at, b'\n').then_some(at + 1);
    opens_text.into_iter().chain(after_break)
}

/// The end of the line holding `at`, past its line break if it has one.
fn line_end_with_break(hay: &str, at: usize) -> usize {
    let end = line_end(hay, at);
    (end + 1).min(hay.len())
}

// ========================================================================
// Markdown
// ========================================================================

/// Drops everything in `mask` that is Markdown markup rather than prose.
pub(super) fn drop_markdown_markup(mask: &mut Mask) {
    mask.apply(code_fence);
    mask.apply(|hay, at| display_math_end(hay, at).map(|end| Hit::dropping(at..end)));
    mask.apply(|hay, at| emphasis(hay, at, "`").map(|(range, _)| Hit::dropping(range).spaced()));
    mask.apply(|hay, at| inline_math_end(hay, at).map(|end| Hit::dropping(at..end).spaced()));
    mask.apply(|hay, at| {
        let close = is_byte_at(hay, at, b'<')
            .then(|| find_from(hay, at + 1, ">"))
            .flatten()?;
        (close > at + 1).then(|| Hit::dropping(at..close + 1))
    });
    mask.apply(|hay, at| url_end(hay, at).map(|end| Hit::dropping(at..end).spaced()));
    for delimiter in EMPHASIS_DELIMITERS {
        mask.apply(|hay, at| {
            emphasis(hay, at, delimiter).map(|(range, content)| Hit::sparing(range, [content]))
        });
    }
}

/// A fenced code block opening a line at `at`, dropped but for the
/// comments of its code when its info string names a language with a
/// known comment style.
fn code_fence(hay: &str, at: usize) -> Option<Hit> {
    let opens_line = char_before(hay, at).is_none_or(|c| c == '\n');
    if !opens_line || !hay[at..].starts_with(FENCE) {
        return None;
    }
    let info_end = line_end(hay, at);
    if info_end == hay.len() {
        return None;
    }
    let code_start = info_end + 1;
    let close = find_from(hay, code_start, FENCE)?;
    let end = close + FENCE.len();
    let info = &hay[at + FENCE.len()..info_end];
    let keeps: Vec<Range<usize>> = comment_style_for_fence(info)
        .map(|style| {
            comment_ranges(&hay[code_start..close], style)
                .into_iter()
                .map(|comment| code_start + comment.start..code_start + comment.end)
                .collect()
        })
        .unwrap_or_default();
    Some(Hit::sparing(at..end, keeps).spaced())
}

/// Past `$$...$$` math opening at `at` after a non-word character, with
/// no `$$` inside, closed before a non-word character or the text's end.
fn display_math_end(hay: &str, at: usize) -> Option<usize> {
    if !is_after_non_word(hay, at) || !hay[at..].starts_with(DISPLAY_MATH) {
        return None;
    }
    let bytes = hay.as_bytes();
    let mut index = at + DISPLAY_MATH.len();
    while index < bytes.len() {
        match (bytes[index], bytes.get(index + 1)) {
            (b'$', Some(b'$')) => break,
            (b'$', Some(_)) => index += 2,
            (b'$', None) => return None,
            _ => index += 1,
        }
    }
    let end = index + DISPLAY_MATH.len();
    (hay[index..].starts_with(DISPLAY_MATH) && is_before_non_word(hay, end)).then_some(end)
}

/// A span `delimiter content delimiter` opening at `at` after a non-word
/// character: the content is on one line, neither starts nor ends with
/// whitespace or the delimiter's character, and the span closes before a
/// non-word character or the text's end. Returns the span and its
/// content, closed at the first delimiter that fits.
fn emphasis(hay: &str, at: usize, delimiter: &str) -> Option<(Range<usize>, Range<usize>)> {
    if !is_after_non_word(hay, at) || !hay[at..].starts_with(delimiter) {
        return None;
    }
    let marker = delimiter.chars().next()?;
    let is_edge_char = |c: char| !c.is_whitespace() && c != marker;
    let content_start = at + delimiter.len();
    let first = hay[content_start..].chars().next()?;
    if !is_edge_char(first) {
        return None;
    }
    let line_end = line_end(hay, content_start);
    let first_end = content_start + first.len_utf8();
    // Byte by byte, so only a character's first byte is a place to close:
    // slicing inside one (the `…` in "wait… go") would panic.
    (first_end..line_end)
        .filter(|&close| hay.is_char_boundary(close) && hay[close..].starts_with(delimiter))
        .find(|&close| {
            let end = close + delimiter.len();
            char_before(hay, close).is_some_and(is_edge_char) && is_before_non_word(hay, end)
        })
        .map(|close| (at..close + delimiter.len(), content_start..close))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{extract_prose_text, Prose};

    fn kept_markdown(text: &str) -> String {
        extract_prose_text(text, Prose::Markdown).text().to_string()
    }

    fn kept_bibtex(text: &str) -> String {
        extract_prose_text(text, Prose::Bibtex).text().to_string()
    }

    #[test]
    fn test_markdown_emphasis_over_a_multibyte_character_is_read_not_panicked_on() {
        assert!(kept_markdown("*wait… then go*").contains("wait"));
        assert!(kept_markdown("an *open star, then … and no close").contains("open star"));
    }

    #[test]
    fn test_markdown_drops_untagged_fence() {
        let kept = kept_markdown("See:\n\n```\nsome raw text\n```\n\nDone.");
        assert!(!kept.contains("some raw text"));
        assert!(kept.contains("Done."));
    }

    #[test]
    fn test_markdown_keeps_comments_of_known_language_fence() {
        let kept = kept_markdown("See:\n\n```python\n# a real comment\nx = 1\n```\n\nDone.");
        assert!(kept.contains("a real comment"));
        assert!(!kept.contains("x = 1"));
        assert!(!kept.contains("python"));
        let kept_rust = kept_markdown("```rs\nlet x = 1; // by extension\n```\n");
        assert!(kept_rust.contains("by extension"));
        assert!(!kept_rust.contains("let"));
    }

    #[test]
    fn test_markdown_drops_inline_code_math_tags_and_urls() {
        let kept = kept_markdown(
            "run `npm install` to set $x_y$ in <span class=\"q\">place</span> at https://ex.am/ple ok",
        );
        for word in ["npm install", "x_y", "span", "class", "ex.am"] {
            assert!(!kept.contains(word), "{word} kept");
        }
        for word in ["run", "to set", "place", "ok"] {
            assert!(kept.contains(word), "{word} dropped");
        }
    }

    #[test]
    fn test_markdown_unwraps_emphasis_in_place() {
        let kept = kept_markdown("a **bold** and _soft_ word, not snake_case_name");
        assert!(kept.contains("a bold and soft word"));
        assert!(kept.contains("snake_case_name"));
    }

    #[test]
    fn test_markdown_drops_display_math() {
        let kept = kept_markdown("Before\n$$\nE = mc^2\n$$\nafter.");
        assert!(!kept.contains("mc^2"));
        assert!(kept.contains("after."));
    }

    #[test]
    fn test_bibtex_keeps_titles_and_drops_people_and_structure() {
        let kept = kept_bibtex(
            "@article{knuth84,\n  author = {Donald Knuth},\n  title = {Literate {P}rogramming},\n  \
             doi = {10.1093/comjnl},\n  journal = {The Computer Journal},\n}\n",
        );
        for word in [
            "article", "knuth84", "Donald", "Knuth", "title", "doi", "comjnl", "journal", "{", "}",
        ] {
            assert!(!kept.contains(word), "{word} kept");
        }
        assert!(kept.contains("Literate"));
        assert!(kept.contains("The Computer Journal"));
    }
}

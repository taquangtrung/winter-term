//! Vim-style word and line motion helpers for Normal-mode cursor navigation.
//!
//! The pure word and line primitives these build on live one layer down, in
//! [`crate::model::vim::words`] — the foundation every Vim surface shares —
//! and are re-exported here for the callers that already reached for them
//! through this module. What stays here is bound to the
//! [`Grid`](winter_render::Grid): reading rows, revealing scrolled-off lines,
//! and the text-object spans.

pub(crate) use crate::model::vim::words::{
    char_class, find_char, first_non_blank, last_non_blank, next_word_start, prev_word_end,
    prev_word_start, word_end,
};
use winter_render::Grid;

use crate::model::input::TextObject;
use crate::model::vim::objects;
use crate::model::vim::objects::{
    bracket_object, paragraph_object, quote_object, sentence_object, word_object, Span, TextRows,
};

// ========================================================================
// Word classification
// ========================================================================

/// The matching bracket for the first bracket at or right of `col` on `line`,
/// searched over `rows_of` (a row's characters, indexed by visible row) starting
/// at `row` (Vim `%`). `None` when the cursor's line holds no bracket from `col`
/// on, or the match is not within the searched rows.
pub(super) fn matching_bracket(
    rows_of: &dyn Fn(usize) -> Vec<char>,
    rows: usize,
    row: usize,
    col: usize,
) -> Option<(usize, usize)> {
    objects::matching_bracket(
        &RowsOf {
            count: rows,
            of: rows_of,
        },
        (row, col),
    )
}

/// Rows read through a closure, for a caller whose rows are numbered its own
/// way rather than the grid's.
struct RowsOf<'a> {
    count: usize,
    of: &'a dyn Fn(usize) -> Vec<char>,
}

impl TextRows for RowsOf<'_> {
    fn row_count(&self) -> usize {
        self.count
    }

    fn row_chars(&self, row: usize) -> Vec<char> {
        (self.of)(row)
    }
}

/// The grid's absolute rows, scrollback and screen together, as the rows a
/// text object is looked for in.
struct GridRows<'a>(&'a Grid);

impl TextRows for GridRows<'_> {
    fn row_count(&self) -> usize {
        self.0.scrollback_len() + self.0.rows()
    }

    fn row_chars(&self, row: usize) -> Vec<char> {
        absolute_row_chars(self.0, row)
    }
}

/// The characters of absolute row `abs_row`, read independent of the scroll
/// position (blank cells as spaces) so buffer-wide motions can scan scrollback.
pub(super) fn absolute_row_chars(grid: &Grid, abs_row: usize) -> Vec<char> {
    (0..grid.cols())
        .map(|col| {
            grid.absolute_cell(abs_row, col)
                .map(|cell| cell.ch)
                .unwrap_or(' ')
        })
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect()
}

/// Whether absolute row `abs_row` holds nothing but blanks, read independent of
/// the scroll position so paragraph motions can scan the whole buffer.
pub(super) fn absolute_row_is_blank(grid: &Grid, abs_row: usize) -> bool {
    (0..grid.cols())
        .filter_map(|col| grid.absolute_cell(abs_row, col))
        .all(|cell| char_class(cell.ch, false) == 0)
}

/// Scroll the viewport just far enough to show absolute row `target`, and return
/// the visible row it now occupies. Coming from below lands it on the top row,
/// from above on the bottom row (Vim's minimal scrolling); an already-visible row
/// leaves the view alone. Shared by every motion that can leave the screen, so
/// none of them clamp to the viewport.
pub(super) fn reveal_absolute_row(grid: &mut Grid, rows: usize, target: usize) -> usize {
    let scrollback = grid.scrollback_len();
    let top = scrollback - grid.scroll_offset().min(scrollback);
    if target < top {
        grid.set_scroll_offset(scrollback - target);
    } else if target >= top + rows {
        grid.set_scroll_offset((rows.saturating_sub(1) + scrollback).saturating_sub(target));
    }
    let new_top = grid.scrollback_len() - grid.scroll_offset();
    target.saturating_sub(new_top).min(rows.saturating_sub(1))
}

/// `{`/`}`: the paragraph boundary nearest the cursor in the given direction: the
/// next blank line that follows a non-blank one, where Vim parks the cursor.
///
/// Searches the whole buffer (scrollback plus the live screen) and scrolls the
/// viewport when the boundary lies off screen, the way `j`/`k` and `w`/`b` do at
/// the edges; the returned value is the boundary's row in the scrolled viewport.
/// Without a further boundary it goes as far as it can, to the buffer's first or
/// last line.
pub(super) fn motion_paragraph(grid: &mut Grid, rows: usize, row: usize, forward: bool) -> usize {
    let scrollback = grid.scrollback_len();
    let total = scrollback + rows;
    let abs = grid.to_absolute_row(row);

    let target = if forward {
        (abs + 1..total)
            .find(|&r| absolute_row_is_blank(grid, r) && !absolute_row_is_blank(grid, r - 1))
            .unwrap_or(total.saturating_sub(1))
    } else {
        (0..abs)
            .rev()
            .find(|&r| absolute_row_is_blank(grid, r) && !absolute_row_is_blank(grid, r + 1))
            .unwrap_or(0)
    };

    reveal_absolute_row(grid, rows, target)
}

// ========================================================================
// Multi-line motion wrappers
// ========================================================================

/// `w`/`W`: the next word start, wrapping to the next line (scrolling at the
/// bottom edge) when the current line has no further word.
pub(super) fn motion_word_forward(
    grid: &mut Grid,
    rows: usize,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
    match next_word_start(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = next_row(grid, rows, row);
            (row, first_non_blank(&line_chars(grid, row)))
        }
    }
}

/// `b`/`B`: the previous word start, wrapping to the prior line (scrolling at the
/// top edge) when nothing precedes the cursor on the current line.
pub(super) fn motion_word_back(
    grid: &mut Grid,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
    match prev_word_start(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = prev_row(grid, row);
            let prev = line_chars(grid, row);
            (row, prev_word_start(&prev, prev.len(), big).unwrap_or(0))
        }
    }
}

/// `e`/`E`: the next word end, wrapping to the next line (scrolling at the bottom
/// edge) when the current line has no further word.
pub(super) fn motion_word_end(
    grid: &mut Grid,
    rows: usize,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
    match word_end(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = next_row(grid, rows, row);
            (row, word_end(&line_chars(grid, row), 0, big).unwrap_or(0))
        }
    }
}

/// `%`: the bracket matching the one at or right of the cursor, searched across
/// the whole buffer and scrolled into view. Returns the new visible `(row, col)`,
/// or `None` when the cursor's line holds no bracket from `col` on, or the partner
/// is missing.
pub(super) fn motion_matching_bracket(
    grid: &mut Grid,
    rows: usize,
    row: usize,
    col: usize,
) -> Option<(usize, usize)> {
    let total = grid.scrollback_len() + rows;
    let abs = grid.to_absolute_row(row);
    let (target_row, target_col) = {
        let rows_of = |r: usize| absolute_row_chars(grid, r);
        matching_bracket(&rows_of, total, abs, col)?
    };
    Some((reveal_absolute_row(grid, rows, target_row), target_col))
}

/// `ge`/`gE`: the previous word end, wrapping to the prior line (scrolling at the
/// top edge) when nothing precedes the cursor on the current line.
pub(super) fn motion_word_end_back(
    grid: &mut Grid,
    row: usize,
    col: usize,
    big: bool,
) -> (usize, usize) {
    match prev_word_end(&line_chars(grid, row), col, big) {
        Some(c) => (row, c),
        None => {
            let row = prev_row(grid, row);
            (row, last_non_blank(&line_chars(grid, row)))
        }
    }
}

/// Every landing spot for `find` on the visible screen, in search order from the
/// cursor: `f`/`t` scan right of the cursor and on down the screen, `F`/`T` left
/// and up. Each spot is already adjusted for `till` (one cell short of the target
/// character) and skipped when that adjustment would land past a line's end or
/// back on the cursor. Drives the easymotion-style `f`/`t` overlay.
pub(super) fn find_char_targets(
    grid: &Grid,
    rows: usize,
    row: usize,
    col: usize,
    find: super::input::FindChar,
) -> Vec<(usize, usize)> {
    let super::input::FindChar { ch, forward, till } = find;
    let mut out = Vec::new();

    let push_row =
        |r: usize, from: Option<usize>, to: Option<usize>, out: &mut Vec<(usize, usize)>| {
            let line = line_chars(grid, r);
            let end = nav_line_end(grid, r);
            let lo = from.unwrap_or(0);
            let hi = to.unwrap_or(line.len());
            let hits: Vec<usize> = (lo..hi.min(line.len()))
                .filter(|&i| line[i] == ch)
                .collect();
            let hits = if forward {
                hits
            } else {
                hits.into_iter().rev().collect()
            };
            for target in hits {
                let landing = if !till {
                    Some(target)
                } else if forward {
                    target.checked_sub(1)
                } else {
                    Some(target + 1)
                };
                if let Some(landing) = landing {
                    if landing <= end && (r != row || landing != col) {
                        out.push((r, landing));
                    }
                }
            }
        };

    if forward {
        push_row(row, Some(col + 1), None, &mut out);
        for r in row + 1..rows {
            push_row(r, None, None, &mut out);
        }
    } else {
        push_row(row, None, Some(col), &mut out);
        for r in (0..row).rev() {
            push_row(r, None, None, &mut out);
        }
    }
    out
}

// ========================================================================
// Row stepping
// ========================================================================

/// Step one visible row down, scrolling history at the bottom edge.
fn next_row(grid: &mut Grid, rows: usize, row: usize) -> usize {
    if row + 1 < rows {
        row + 1
    } else {
        grid.scroll_down_history(1);
        row
    }
}

/// Step one visible row up, scrolling history at the top edge.
fn prev_row(grid: &mut Grid, row: usize) -> usize {
    if row > 0 {
        row - 1
    } else {
        grid.scroll_up_history(1);
        row
    }
}

/// The rightmost column the Normal-mode cursor may occupy on visible `row`.
///
/// Usually the last printed character ([`Grid::visible_line_end`]), so the
/// cursor never wanders into the blank padding past a line. On the live prompt
/// row it extends to the shell cursor: a typed trailing space is indistinguishable
/// from blank padding in the cell grid (both are `' '`), and reaching the
/// insertion point itself keeps the cursor at the same column when the user
/// switches modes (Insert's shell cursor sits at that exact column).
pub(super) fn nav_line_end(grid: &Grid, row: usize) -> usize {
    let end = grid.visible_line_end(row);
    let (cursor_row, cursor_col) = grid.cursor();
    if grid.scroll_offset() == 0 && row == cursor_row {
        let cap = grid.cols().saturating_sub(1);
        end.max(cursor_col.min(cap))
    } else {
        end
    }
}

/// The printed characters of a visible row, trimmed of trailing blank padding so
/// motions see real line ends. A fully blank row yields an empty slice.
pub(super) fn line_chars(grid: &Grid, row: usize) -> Vec<char> {
    let end = grid.visible_line_end(row);
    let mut chars: Vec<char> = (0..=end)
        .map(|col| grid.visible_cell(row, col).map(|c| c.ch).unwrap_or(' '))
        .map(|c| if c == '\0' { ' ' } else { c })
        .collect();
    if chars.len() == 1 && char_class(chars[0], false) == 0 {
        chars.clear();
    }
    chars
}

// ========================================================================
// Text Objects
// ========================================================================

/// Compute the `(start, end)` inclusive coordinates for a word text object.
pub(super) fn text_object_word(
    grid: &Grid,
    row: usize,
    col: usize,
    big: bool,
    around: bool,
) -> Option<((usize, usize), (usize, usize))> {
    let (start, end) = word_object(&absolute_row_chars(grid, row), col, big, around)?;
    Some(((row, start), (row, end)))
}

/// Compute the `(start, end)` inclusive coordinates for a delimited quote text object.
pub(super) fn text_object_quotes(
    grid: &Grid,
    row: usize,
    col: usize,
    quote: char,
    around: bool,
) -> Option<((usize, usize), (usize, usize))> {
    let (start, end) = quote_object(&absolute_row_chars(grid, row), col, quote, around)?;
    Some(((row, start), (row, end)))
}

/// Compute the `(start, end)` inclusive coordinates for a bracket text object.
pub(super) fn text_object_brackets(
    grid: &Grid,
    row: usize,
    col: usize,
    open: char,
    close: char,
    around: bool,
) -> Option<((usize, usize), (usize, usize))> {
    let Span { end, start } = bracket_object(&GridRows(grid), (row, col), open, close, around)?;
    Some((start, end))
}

/// Compute the text object span `((start_row, start_col), (end_row, end_col))` in absolute coordinates.
pub(super) fn text_object_span(
    grid: &Grid,
    row: usize,
    col: usize,
    around: bool,
    object: TextObject,
) -> Option<((usize, usize), (usize, usize))> {
    match object {
        TextObject::Word => text_object_word(grid, row, col, false, around),
        TextObject::WordBig => text_object_word(grid, row, col, true, around),
        TextObject::Quotes(q) => text_object_quotes(grid, row, col, q, around),
        TextObject::Brackets(o, c) => text_object_brackets(grid, row, col, o, c, around),
        TextObject::Paragraph => text_object_paragraph(grid, row, around),
        TextObject::Sentence => text_object_sentence(grid, row, col, around),
    }
}

/// The sentence `col` sits in, within its own row.
fn text_object_sentence(
    grid: &Grid,
    row: usize,
    col: usize,
    around: bool,
) -> Option<((usize, usize), (usize, usize))> {
    // Trailing blanks are the row's padding, not part of a sentence: without
    // dropping them the last sentence of a row would run to the pane's edge,
    // and `as` would find blanks after it that are not really there.
    let mut line = absolute_row_chars(grid, row);
    while line.last().is_some_and(|c| c.is_whitespace()) {
        line.pop();
    }
    let (start, end) = sentence_object(&line, col, around)?;
    Some(((row, start), (row, end)))
}

/// The rows a paragraph object covers, as a span over whole rows: the text a
/// yank reads back drops the blank padding past each row's own end.
fn text_object_paragraph(
    grid: &Grid,
    row: usize,
    around: bool,
) -> Option<((usize, usize), (usize, usize))> {
    let (start, end) = paragraph_object(&GridRows(grid), row, around)?;
    Some(((start, 0), (end, grid.cols().saturating_sub(1))))
}

/// Normalize delimiter character to its opening and closing pair, and whether it is a quote.
pub(super) fn surround_pair_chars(d: char) -> Option<(char, char, bool)> {
    match d {
        '"' | '\'' | '`' => Some((d, d, true)),
        '(' | ')' | 'b' => Some(('(', ')', false)),
        '[' | ']' => Some(('[', ']', false)),
        '{' | '}' | 'B' => Some(('{', '}', false)),
        '<' | '>' => Some(('<', '>', false)),
        _ => None,
    }
}

/// Compute the exact start and end position of the surrounding delimiters.
pub(super) fn surround_pair_positions(
    grid: &Grid,
    row: usize,
    col: usize,
    delimiter: char,
) -> Option<((usize, usize), (usize, usize))> {
    let (open, close, is_quote) = surround_pair_chars(delimiter)?;
    if is_quote {
        text_object_quotes(grid, row, col, open, true)
    } else {
        text_object_brackets(grid, row, col, open, close, true)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A grid one row deep holding `line`, for the row-local text objects.
    fn grid_with(line: &str) -> Grid {
        let mut grid = Grid::new(line.chars().count() + 10, 3);
        for ch in line.chars() {
            grid.print(ch);
        }
        grid
    }

    #[test]
    fn test_a_sentence_ends_at_its_stop_and_the_quotes_that_close_after_it() {
        // Two sentences on one row, the first closing inside a quote: `is`
        // takes the sentence, `as` takes the blanks after it too.
        let grid = grid_with("One two. \"Three four.\" Five");
        let text = |span: ((usize, usize), (usize, usize))| -> String {
            let line = absolute_row_chars(&grid, 0);
            line[span.0 .1..=span.1 .1].iter().collect()
        };

        let first = text_object_sentence(&grid, 0, 2, false).expect("the first sentence");
        assert_eq!(text(first), "One two.");
        let around = text_object_sentence(&grid, 0, 2, true).expect("with its blanks");
        assert_eq!(text(around), "One two. ");

        let second = text_object_sentence(&grid, 0, 12, false).expect("the second");
        assert_eq!(
            text(second),
            "\"Three four.\"",
            "the quote closing after the stop belongs to it"
        );
    }

    #[test]
    fn test_a_stop_inside_a_word_does_not_end_a_sentence() {
        // `.` only ends one when a blank or the row's end follows, which is
        // what keeps a version number or a file name in one piece.
        let grid = grid_with("Run cargo test v1.2.3 now");
        let span = text_object_sentence(&grid, 0, 0, false).expect("one sentence");
        let line = absolute_row_chars(&grid, 0);
        let text: String = line[span.0 .1..=span.1 .1].iter().collect();
        assert_eq!(text, "Run cargo test v1.2.3 now");
    }

    #[test]
    fn test_as_takes_the_blanks_before_when_none_follow() {
        let grid = grid_with("First. Second.");
        let span = text_object_sentence(&grid, 0, 10, true).expect("the last sentence");
        let line = absolute_row_chars(&grid, 0);
        let text: String = line[span.0 .1..=span.1 .1].iter().collect();
        assert_eq!(text, " Second.");
    }

    #[test]
    fn test_nav_line_end_extends_to_shell_cursor_for_trailing_space() {
        // A command typed with a trailing space: the cell grid stores the space
        // like blank padding, so the shell cursor (col 3) marks the real end.
        let mut grid = Grid::new(20, 3);
        for ch in "cd ".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.visible_line_end(0), 1); // last printed glyph is 'd'
        assert_eq!(grid.cursor(), (0, 3));
        // nav_line_end reaches the shell cursor's column so the Normal-mode
        // cursor can sit at the same position the Insert-mode cursor did.
        assert_eq!(nav_line_end(&grid, 0), 3);
    }

    #[test]
    fn test_nav_line_end_reaches_shell_cursor_without_trailing_space() {
        let mut grid = Grid::new(20, 3);
        for ch in "cd".chars() {
            grid.print(ch);
        }
        // Even without trailing whitespace, nav_line_end reaches the shell
        // cursor (col 2) so Normal mode can start at the same column Insert's
        // shell cursor occupied.
        assert_eq!(grid.cursor(), (0, 2));
        assert_eq!(nav_line_end(&grid, 0), 2);
    }

    #[test]
    fn test_nav_line_end_does_not_extend_non_prompt_rows() {
        let mut grid = Grid::new(20, 3);
        for ch in "out ".chars() {
            grid.print(ch);
        }
        grid.line_feed(); // shell cursor moves to row 1
        grid.carriage_return();
        // Row 0 is no longer the cursor row, so its trailing space is padding.
        assert_eq!(nav_line_end(&grid, 0), 2); // last glyph 't'
    }

    #[test]
    fn test_find_char_forward_backward_and_till() {
        use crate::model::input::FindChar;
        let line: Vec<char> = "abcabc".chars().collect();
        let find = |forward, till| FindChar {
            ch: 'c',
            forward,
            till,
        };

        // `fc` from 0 lands on the first 'c' (index 2); repeating from there
        // (`;`) advances to the next 'c' at 5.
        assert_eq!(find_char(&line, 0, find(true, false)), Some(2));
        assert_eq!(find_char(&line, 2, find(true, false)), Some(5));
        // `tc` stops one cell short of the 'c'.
        assert_eq!(find_char(&line, 0, find(true, true)), Some(1));
        // `Fc` searches left; `Tc` stops one cell past it (to the right).
        assert_eq!(find_char(&line, 5, find(false, false)), Some(2));
        assert_eq!(find_char(&line, 5, find(false, true)), Some(3));
        // A miss leaves the caller to keep the cursor put.
        let miss = FindChar {
            ch: 'z',
            forward: true,
            till: false,
        };
        assert_eq!(find_char(&line, 0, miss), None);
        // A till search onto the adjacent cell would not move, so it reports None.
        assert_eq!(find_char(&line, 1, find(true, true)), None);
    }

    fn grid_from_line(s: &str) -> Grid {
        let mut grid = Grid::new(s.chars().count().max(10), 1);
        for ch in s.chars() {
            grid.print(ch);
        }
        grid
    }

    #[test]
    fn test_text_object_word_inner_and_around() {
        // Line: "hello   world   foo"
        let grid = grid_from_line("hello   world   foo");

        // iw on "hello" -> 0..4
        assert_eq!(
            text_object_word(&grid, 0, 2, false, false),
            Some(((0, 0), (0, 4)))
        );
        // aw on "hello" -> 0..7 (includes trailing whitespace)
        assert_eq!(
            text_object_word(&grid, 0, 2, false, true),
            Some(((0, 0), (0, 7)))
        );
        // iw on whitespace "   " -> 5..7
        assert_eq!(
            text_object_word(&grid, 0, 6, false, false),
            Some(((0, 5), (0, 7)))
        );
    }

    #[test]
    fn test_text_object_quotes_inner_and_around() {
        let grid = grid_from_line(r#"let msg = "hello world";"#);

        // Cursor inside "hello world" at index 14
        // i" -> 11..21 ("hello world")
        assert_eq!(
            text_object_quotes(&grid, 0, 14, '"', false),
            Some(((0, 11), (0, 21)))
        );
        // a" -> 10..22 (`"hello world"`)
        assert_eq!(
            text_object_quotes(&grid, 0, 14, '"', true),
            Some(((0, 10), (0, 22)))
        );
    }

    #[test]
    fn test_text_object_brackets_inner_and_around() {
        let grid = grid_from_line("fn foo(a, b, c) { return 42; }");

        // Cursor inside parens at index 8 ('a')
        // i( -> 7..13 ("a, b, c")
        assert_eq!(
            text_object_brackets(&grid, 0, 8, '(', ')', false),
            Some(((0, 7), (0, 13)))
        );
        // a( -> 6..14 ("(a, b, c)")
        assert_eq!(
            text_object_brackets(&grid, 0, 8, '(', ')', true),
            Some(((0, 6), (0, 14)))
        );

        // Cursor inside braces at index 20
        // i{ -> 17..28 (" return 42; ")
        assert_eq!(
            text_object_brackets(&grid, 0, 20, '{', '}', false),
            Some(((0, 17), (0, 28)))
        );
        // a{ -> 16..29 ("{ return 42; }")
        assert_eq!(
            text_object_brackets(&grid, 0, 20, '{', '}', true),
            Some(((0, 16), (0, 29)))
        );
    }

    #[test]
    fn test_text_object_unbalanced_delimiters() {
        let grid = grid_from_line("fn foo(a, b, c");
        assert_eq!(text_object_brackets(&grid, 0, 8, '(', ')', false), None);

        let grid2 = grid_from_line("fn foo \"hello");
        assert_eq!(text_object_quotes(&grid2, 0, 10, '"', false), None);
    }
}

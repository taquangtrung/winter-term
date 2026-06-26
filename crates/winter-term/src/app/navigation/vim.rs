//! Vim-style word and line motion helpers for Normal-mode cursor navigation.
//!
//! These are pure functions over a [`Grid`](winter_render::Grid) reference;
//! they do not touch `App` state directly.

use winter_render::Grid;

use crate::model::input::TextObject;

// ========================================================================
// Word classification
// ========================================================================

/// A character's word class, à la Vim. Blanks (class 0) separate words. With
/// `big` false (`w`/`b`/`e`): keyword runs (alphanumerics and `_`, class 1) are
/// distinct from punctuation runs (class 2). With `big` true (`W`/`B`/`E`): any
/// non-blank is class 1, so only whitespace breaks a WORD.
pub(super) fn char_class(c: char, big: bool) -> u8 {
    if c == '\0' || c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

// ========================================================================
// Single-line word motion primitives
// ========================================================================

/// The start column of the next word at or after `col` (Vim `w`/`W`), or `None`
/// when the rest of the line holds no further word.
pub(super) fn next_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col;
    let here = line.get(i).map(|c| char_class(*c, big)).unwrap_or(0);
    if here != 0 {
        while i < line.len() && char_class(line[i], big) == here {
            i += 1;
        }
    }
    while i < line.len() && char_class(line[i], big) == 0 {
        i += 1;
    }
    (i < line.len()).then_some(i)
}

/// The start column of the previous word before `col` (Vim `b`/`B`), or `None`
/// when nothing precedes it on the line.
pub(super) fn prev_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
    if col == 0 {
        return None;
    }
    let mut i = col - 1;
    while i > 0 && char_class(line[i], big) == 0 {
        i -= 1;
    }
    if char_class(line[i], big) == 0 {
        return None;
    }
    let class = char_class(line[i], big);
    while i > 0 && char_class(line[i - 1], big) == class {
        i -= 1;
    }
    Some(i)
}

/// The end column of the next word after `col` (Vim `e`/`E`), or `None` when the
/// rest of the line holds no further word.
pub(super) fn word_end(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col + 1;
    while i < line.len() && char_class(line[i], big) == 0 {
        i += 1;
    }
    if i >= line.len() {
        return None;
    }
    let class = char_class(line[i], big);
    while i + 1 < line.len() && char_class(line[i + 1], big) == class {
        i += 1;
    }
    Some(i)
}

/// The column of the first non-blank character (Vim `^`), or 0 for a blank line.
pub(super) fn first_non_blank(line: &[char]) -> usize {
    line.iter()
        .position(|c| char_class(*c, false) != 0)
        .unwrap_or(0)
}

/// The column of the last non-blank character (Vim `g_`), or 0 for a blank line.
pub(super) fn last_non_blank(line: &[char]) -> usize {
    line.iter()
        .rposition(|c| char_class(*c, false) != 0)
        .unwrap_or(0)
}

/// The end column of the word before `col` (Vim `ge`/`gE`), or `None` when
/// nothing precedes it on the line.
pub(super) fn prev_word_end(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col.checked_sub(1)?;
    // Step back off the word the cursor is inside, then over the blanks.
    let here = char_class(*line.get(col).unwrap_or(&' '), big);
    if here != 0 {
        while i > 0 && char_class(line[i], big) == here {
            i -= 1;
        }
        if char_class(line[i], big) == here {
            return None;
        }
    }
    while char_class(line[i], big) == 0 {
        i = i.checked_sub(1)?;
    }
    Some(i)
}

/// The matching bracket for the first bracket at or right of `col` on `line`,
/// searched over `rows_of` (a row's characters, indexed by visible row) starting
/// at `row` (Vim `%`). `None` when the cursor's line holds no bracket from `col`
/// on, or the match is not within the searched rows.
///
/// Nesting is counted, so `%` on the outer paren of `f(g(x))` lands on the outer
/// closer, not the inner one.
pub(super) fn matching_bracket(
    rows_of: &dyn Fn(usize) -> Vec<char>,
    rows: usize,
    row: usize,
    col: usize,
) -> Option<(usize, usize)> {
    const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

    let line = rows_of(row);
    // Vim starts from the first bracket at or after the cursor on its line.
    let (start_col, open, close, forward) = (col..line.len()).find_map(|c| {
        let ch = line[c];
        PAIRS.iter().find_map(|&(o, cl)| {
            if ch == o {
                Some((c, o, cl, true))
            } else if ch == cl {
                Some((c, o, cl, false))
            } else {
                None
            }
        })
    })?;

    let mut depth: i32 = 0;
    let mut r = row;
    let mut c = start_col;
    loop {
        let chars = rows_of(r);
        if let Some(&ch) = chars.get(c) {
            if ch == open {
                depth += if forward { 1 } else { -1 };
            } else if ch == close {
                depth += if forward { -1 } else { 1 };
            }
            // Back to zero means this is the partner of the starting bracket.
            if depth == 0 {
                return Some((r, c));
            }
        }
        // Step one cell in the search direction, wrapping across rows.
        if forward {
            if c + 1 < chars.len() {
                c += 1;
            } else if r + 1 < rows {
                r += 1;
                c = 0;
            } else {
                return None;
            }
        } else if c > 0 {
            c -= 1;
        } else if r > 0 {
            r -= 1;
            c = rows_of(r).len().saturating_sub(1);
        } else {
            return None;
        }
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

/// The landing column for a Vim char-search (`f`/`F`/`t`/`T`) on `line` from
/// `col`. `forward` searches right of the cursor, else left; `till` stops one
/// cell short of the match. Returns `None` when there is no match, or when a
/// `till` search would not move (target already adjacent).
pub(super) fn find_char(line: &[char], col: usize, find: super::input::FindChar) -> Option<usize> {
    let super::input::FindChar { ch, forward, till } = find;
    let target = if forward {
        (col + 1..line.len()).find(|&i| line[i] == ch)?
    } else {
        (0..col).rev().find(|&i| line[i] == ch)?
    };
    let landing = if !till {
        target
    } else if forward {
        target.checked_sub(1)?
    } else {
        target + 1
    };
    (landing != col).then_some(landing)
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
    let line = absolute_row_chars(grid, row);
    if line.is_empty() {
        return None;
    }
    let col = col.min(line.len().saturating_sub(1));
    let cls = char_class(line[col], big);

    if cls == 0 {
        // Cursor is on whitespace.
        let mut start = col;
        while start > 0 && char_class(line[start - 1], big) == 0 {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < line.len() && char_class(line[end + 1], big) == 0 {
            end += 1;
        }
        if around {
            if end + 1 < line.len() && char_class(line[end + 1], big) != 0 {
                let mut word_end = end + 1;
                let word_cls = char_class(line[word_end], big);
                while word_end + 1 < line.len() && char_class(line[word_end + 1], big) == word_cls {
                    word_end += 1;
                }
                end = word_end;
            } else if start > 0 && char_class(line[start - 1], big) != 0 {
                let mut word_start = start - 1;
                let word_cls = char_class(line[word_start], big);
                while word_start > 0 && char_class(line[word_start - 1], big) == word_cls {
                    word_start -= 1;
                }
                start = word_start;
            }
        }
        Some(((row, start), (row, end)))
    } else {
        // Cursor is on a word (keyword or punctuation run).
        let mut start = col;
        while start > 0 && char_class(line[start - 1], big) == cls {
            start -= 1;
        }
        let mut end = col;
        while end + 1 < line.len() && char_class(line[end + 1], big) == cls {
            end += 1;
        }
        if around {
            if end + 1 < line.len() && char_class(line[end + 1], big) == 0 {
                while end + 1 < line.len() && char_class(line[end + 1], big) == 0 {
                    end += 1;
                }
            } else if start > 0 && char_class(line[start - 1], big) == 0 {
                while start > 0 && char_class(line[start - 1], big) == 0 {
                    start -= 1;
                }
            }
        }
        Some(((row, start), (row, end)))
    }
}

/// Compute the `(start, end)` inclusive coordinates for a delimited quote text object.
pub(super) fn text_object_quotes(
    grid: &Grid,
    row: usize,
    col: usize,
    quote: char,
    around: bool,
) -> Option<((usize, usize), (usize, usize))> {
    let line = absolute_row_chars(grid, row);
    let mut quote_indices = Vec::new();
    let mut escaped = false;
    for (i, &c) in line.iter().enumerate() {
        if c == '\\' && !escaped {
            escaped = true;
            continue;
        }
        if c == quote && !escaped {
            quote_indices.push(i);
        }
        escaped = false;
    }

    if quote_indices.len() < 2 {
        return None;
    }

    let mut pair = None;
    for chunk in quote_indices.chunks_exact(2) {
        let (q1, q2) = (chunk[0], chunk[1]);
        if col <= q2 {
            pair = Some((q1, q2));
            break;
        }
    }

    let (q1, q2) = pair?;
    if around {
        Some(((row, q1), (row, q2)))
    } else if q2 > q1 + 1 {
        Some(((row, q1 + 1), (row, q2 - 1)))
    } else {
        Some(((row, q1 + 1), (row, q1)))
    }
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
    let total_rows = grid.scrollback_len() + grid.rows();
    let rows_of = |r: usize| absolute_row_chars(grid, r);

    let mut depth: i32 = 0;
    let mut r = row;
    let chars = rows_of(r);
    let mut c = col.min(chars.len().saturating_sub(1));

    let mut open_pos = None;
    loop {
        let line = rows_of(r);
        if let Some(&ch) = line.get(c) {
            if ch == close {
                depth += 1;
            } else if ch == open {
                if depth == 0 {
                    open_pos = Some((r, c));
                    break;
                } else {
                    depth -= 1;
                }
            }
        }
        if c > 0 {
            c -= 1;
        } else if r > 0 {
            r -= 1;
            c = rows_of(r).len().saturating_sub(1);
        } else {
            break;
        }
    }

    let (r_open, c_open) = open_pos?;

    let mut close_depth: i32 = 0;
    let mut rf = r_open;
    let mut cf = c_open;
    let mut close_pos = None;

    loop {
        let line = rows_of(rf);
        if let Some(&ch) = line.get(cf) {
            if ch == open {
                close_depth += 1;
            } else if ch == close {
                close_depth -= 1;
                if close_depth == 0 {
                    close_pos = Some((rf, cf));
                    break;
                }
            }
        }
        if cf + 1 < line.len() {
            cf += 1;
        } else if rf + 1 < total_rows {
            rf += 1;
            cf = 0;
        } else {
            break;
        }
    }

    let (r_close, c_close) = close_pos?;

    if around {
        Some(((r_open, c_open), (r_close, c_close)))
    } else {
        let (r1, c1) = {
            let line = rows_of(r_open);
            if c_open + 1 < line.len() {
                (r_open, c_open + 1)
            } else if r_open + 1 < total_rows {
                (r_open + 1, 0)
            } else {
                (r_open, c_open)
            }
        };
        let (r2, c2) = {
            if c_close > 0 {
                (r_close, c_close - 1)
            } else if r_close > 0 {
                let prev_len = rows_of(r_close - 1).len();
                (r_close - 1, prev_len.saturating_sub(1))
            } else {
                (r_close, c_close)
            }
        };
        Some(((r1, c1), (r2, c2)))
    }
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
    }
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

    #[test]
    fn test_vim_word_motions_on_a_line() {
        // f o o , _ b a r _ b a z _ q u x   (_ = space)
        let line: Vec<char> = "foo, bar_baz qux".chars().collect();

        // `w`: word starts, treating punctuation as its own word.
        assert_eq!(next_word_start(&line, 0, false), Some(3)); // foo -> ','
        assert_eq!(next_word_start(&line, 3, false), Some(5)); // ',' -> 'bar_baz'
        assert_eq!(next_word_start(&line, 5, false), Some(13)); // 'bar_baz' -> 'qux'
        assert_eq!(next_word_start(&line, 13, false), None); // nothing after 'qux'

        // `b`: previous word starts.
        assert_eq!(prev_word_start(&line, 13, false), Some(5));
        assert_eq!(prev_word_start(&line, 5, false), Some(3));
        assert_eq!(prev_word_start(&line, 0, false), None);

        // `e`: word ends.
        assert_eq!(word_end(&line, 0, false), Some(2)); // end of 'foo'
        assert_eq!(word_end(&line, 2, false), Some(3)); // the ',' is a 1-char word
        assert_eq!(word_end(&line, 5, false), Some(11)); // end of 'bar_baz'

        // `^`: first non-blank.
        assert_eq!(first_non_blank(&"   hi".chars().collect::<Vec<_>>()), 3);
        assert_eq!(first_non_blank(&"".chars().collect::<Vec<_>>()), 0);
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

    #[test]
    fn test_vim_big_word_motions_ignore_punctuation() {
        // WORD motions span punctuation: only whitespace separates WORDs.
        let line: Vec<char> = "foo, bar_baz qux".chars().collect();

        // `W`: "foo," is one WORD, so the next WORD is 'bar_baz' at 5, then 'qux'.
        assert_eq!(next_word_start(&line, 0, true), Some(5));
        assert_eq!(next_word_start(&line, 5, true), Some(13));

        // `B`: from 'qux' back to 'bar_baz' (5), then to "foo," (0).
        assert_eq!(prev_word_start(&line, 13, true), Some(5));
        assert_eq!(prev_word_start(&line, 5, true), Some(0));

        // `E`: end of "foo," is the comma at 3 (vs `e` which stops at 'foo').
        assert_eq!(word_end(&line, 0, true), Some(3));
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

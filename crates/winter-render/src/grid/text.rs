//! Printing text into the grid: graphemes, wide cells, and auto-wrap.

use super::Grid;
use super::{Cell, CellWidth, Style};
use unicode_width::UnicodeWidthChar;

// ========================================================================
// Grid: printing text
// ========================================================================

impl Grid {
    /// The first non-blank column in visible `row`, or 0 if the row is blank.
    pub fn first_non_blank_col(&self, row: usize) -> usize {
        for col in 0..self.cols {
            if let Some(cell) = self.visible_cell(row, col) {
                if cell.ch != '\0' && !cell.ch.is_whitespace() {
                    return col;
                }
            }
        }
        0
    }
    /// Auto-wrap the current row into the next row, applying hanging indent if
    /// enabled. With word wrap on, the wrap breaks at the row's last word
    /// boundary instead of exactly at the margin: the partial word after the
    /// last breakable space moves down with the wrap (see [`Self::take_word_break`]).
    pub(super) fn auto_wrap(&mut self) {
        self.cursor.wrap_pending = false;
        let prev_row = self.cursor.row;
        if let Some(flag) = self.row_wrapped.get_mut(prev_row) {
            *flag = true;
        }
        let indent = if self.wrap_indent {
            let mut first_row = prev_row;
            while first_row > 0 && self.row_wraps(first_row - 1) {
                first_row -= 1;
            }
            let fnb = self.first_non_blank_col(first_row);
            fnb.min(self.cols / 2)
        } else {
            0
        };
        // Word wrap is a primary-screen behavior only: a full-screen app
        // addresses the grid by exact coordinates, and moving its cells would
        // desync the screen model it is silently redrawing against.
        let carried = if self.word_wrap && self.alt_buffer.is_none() {
            self.take_word_break(prev_row, indent)
        } else {
            if let Some(brk) = self.row_break.get_mut(prev_row) {
                *brk = None;
            }
            Vec::new()
        };
        self.cursor.col = 0;
        self.line_feed();
        if indent > 0 && self.cursor.row < self.rows {
            if let Some(row_indent) = self.row_wrap_indent.get_mut(self.cursor.row) {
                *row_indent = indent;
            }
            self.cursor.col = indent;
        }
        if !carried.is_empty() && self.cursor.row < self.rows {
            let row = self.cursor.row;
            let carried_len = carried.len();
            for (offset, cell) in carried.into_iter().enumerate() {
                if let Some(index) = self.index(row, self.cursor.col + offset) {
                    self.cells[index] = cell;
                }
            }
            // The carried word may fill the continuation row exactly; park at
            // its last column the way a printed glyph would, so the next
            // character wraps rather than addressing a column past the edge.
            self.cursor.col += carried_len;
            if self.cursor.col >= self.cols {
                self.cursor.col = self.cols.saturating_sub(1);
                self.cursor.wrap_pending = true;
            }
        }
    }
    /// Choose where a word break falls on `row`, given the indent the
    /// continuation will start at: the rightmost breakable blank cell `k`
    /// whose following cells (the partial word) still fit beside that indent.
    /// The space at `k` is consumed by the break (recorded in
    /// [`Self::row_break`], for joining the line back into text), the cells
    /// after it are blanked and returned for the continuation row, and the
    /// row itself ends at `k`. An empty `Vec` means no cells moved — the wrap
    /// lands at the margin exactly as it would with word wrap off (no
    /// breakable space, or one too far left for the word to fit below).
    fn take_word_break(&mut self, row: usize, indent: usize) -> Vec<Cell> {
        let mut break_col = None;
        for col in (0..self.cols).rev() {
            let Some(index) = self.index(row, col) else {
                continue;
            };
            let cell = &self.cells[index];
            if cell.width == CellWidth::Single && is_breakable_blank(cell.ch) {
                // The partial word after `col` is `cols - 1 - col` cells wide;
                // it must fit on the next row beside the hanging indent.
                if indent + (self.cols - 1 - col) <= self.cols {
                    break_col = Some(col);
                    break;
                }
            }
        }
        let Some(k) = break_col else {
            if let Some(brk) = self.row_break.get_mut(row) {
                *brk = None;
            }
            return Vec::new();
        };
        if let Some(brk) = self.row_break.get_mut(row) {
            *brk = Some(k);
        }
        let row_start = row * self.cols;
        let carried = self.cells[row_start + k + 1..row_start + self.cols].to_vec();
        for index in &mut self.cells[row_start + k..row_start + self.cols] {
            *index = Cell::default();
        }
        carried
    }
    /// Print a character at the cursor and advance, wrapping and scrolling as
    /// needed. Uses deferred (pending) line wrap matching VT100/xterm semantics:
    /// when a print fills the final column, the cursor parks at `cols - 1` with
    /// the cursor's deferred-wrap flag set, and the actual wrap to the next line is
    /// deferred until the next printable character. This keeps the reported
    /// cursor column in bounds and lets full-width progress bars and spinners
    /// redraw in place via `\r` instead of scrolling prematurely.
    pub fn print(&mut self, ch: char) {
        // A zero-width combining mark composes onto the previous cell rather than
        // occupying its own; it must not flush a pending wrap or advance.
        if UnicodeWidthChar::width(ch) == Some(0) {
            self.combine_into_previous(ch);
            return;
        }
        // A skin-tone modifier or a second regional-indicator flag half merges
        // onto the preceding cell instead of printing as its own glyph.
        if self.try_merge_continuation(ch) {
            return;
        }
        if self.cursor.wrap_pending {
            self.auto_wrap();
        }
        let style = Style {
            link: self.active_link,
            ..self.style
        };
        // East-Asian-wide and emoji glyphs occupy two columns; width 0 (combining
        // marks, controls) is treated as a single cell to preserve prior behavior.
        if UnicodeWidthChar::width(ch) == Some(2) {
            // A double-width glyph can't straddle the right margin: if only one
            // column remains, wrap to the next line before placing it.
            if self.cursor.col + 1 >= self.cols {
                self.auto_wrap();
            }
            let (row, col) = (self.cursor.row, self.cursor.col);
            if let Some(index) = self.index(row, col) {
                self.cells[index] = Cell {
                    ch,
                    tail: None,
                    style,
                    width: CellWidth::Wide,
                };
            }
            if let Some(index) = self.index(row, col + 1) {
                self.cells[index] = Cell {
                    ch: ' ',
                    tail: None,
                    style,
                    width: CellWidth::Spacer,
                };
            }
            if col + 2 >= self.cols {
                self.cursor.col = self.cols.saturating_sub(1);
                self.cursor.wrap_pending = true;
            } else {
                self.cursor.col += 2;
            }
            return;
        }
        if let Some(index) = self.index(self.cursor.row, self.cursor.col) {
            self.cells[index] = Cell {
                ch,
                tail: None,
                style,
                width: CellWidth::Single,
            };
        }
        if self.cursor.col + 1 >= self.cols {
            // Filled the last column: park here and defer the wrap.
            self.cursor.wrap_pending = true;
        } else {
            self.cursor.col += 1;
        }
    }
    /// Index of the cell holding the glyph just printed: the parked column when
    /// a wrap is pending, otherwise the cell just left of the cursor. Steps back
    /// one more column when that lands on a `Spacer` (the blank right half of a
    /// double-width glyph) so it resolves to the `Wide` cell that actually holds
    /// the character: most emoji are double-width, and a combining mark/ZWJ/
    /// selector/modifier that follows one must attach to the real glyph, not
    /// its placeholder. `None` at the start of a row (nothing to attach to).
    pub(super) fn last_glyph_cell_index(&self) -> Option<usize> {
        let col = if self.cursor.wrap_pending {
            self.cursor.col
        } else if self.cursor.col > 0 {
            self.cursor.col - 1
        } else {
            return None;
        };
        let index = self.index(self.cursor.row, col)?;
        if self.cells[index].width == CellWidth::Spacer && col > 0 {
            return self.index(self.cursor.row, col - 1);
        }
        Some(index)
    }
    /// Apply a zero-width combining mark to the most recently written cell.
    /// Canonically composable marks compose directly onto that cell's
    /// character (e.g. `e` + ◌́ → `é`); a ZWJ, variation selector, or keycap
    /// enclosure that can't compose (true of all three, always) is instead
    /// appended to the cell's [`Cell::tail`], so emoji ZWJ sequences, VS16
    /// presentation selectors, and keycap sequences reach the renderer
    /// intact instead of being dropped. Any other uncomposable mark is still
    /// dropped, matching prior behavior.
    ///
    /// VS16 (`\u{FE0F}`, the emoji presentation selector) and `\u{20E3}`
    /// (COMBINING ENCLOSING KEYCAP) additionally upgrade a `Single` base to
    /// `Wide`: many common characters (the warning sign `⚠`, heart `❤`,
    /// check mark `✔`, a keycap digit `1`, ...) default to a narrow,
    /// text-only presentation and only render as their full double-width
    /// color-emoji artwork when one of these explicitly requests it (unlike
    /// `⚡`, whose emoji presentation is already the default, so
    /// [`UnicodeWidthChar::width`] already classified it `Wide` at print
    /// time). Skipped when the base glyph is parked at the row's last column
    /// with a wrap already pending: there's no column left on this row to
    /// claim as the [`CellWidth::Spacer`], and stealing the base glyph's own
    /// column would erase it.
    pub(super) fn combine_into_previous(&mut self, mark: char) {
        let Some(index) = self.last_glyph_cell_index() else {
            return;
        };
        if let Some(composed) = unicode_normalization::char::compose(self.cells[index].ch, mark) {
            self.cells[index].ch = composed;
            return;
        }
        if is_emoji_tail_mark(mark) {
            append_tail(&mut self.cells[index], mark);
            let requests_emoji_presentation = matches!(mark, '\u{FE0F}' | '\u{20E3}');
            let base_is_narrow = self.cells[index].width == CellWidth::Single;
            if requests_emoji_presentation && base_is_narrow && !self.cursor.wrap_pending {
                self.cells[index].width = CellWidth::Wide;
                if let Some(spacer_index) = self.index(self.cursor.row, self.cursor.col) {
                    let style = self.cells[spacer_index].style;
                    self.cells[spacer_index] = Cell {
                        ch: ' ',
                        tail: None,
                        style,
                        width: CellWidth::Spacer,
                    };
                }
                if self.cursor.col + 1 >= self.cols {
                    self.cursor.wrap_pending = true;
                } else {
                    self.cursor.col += 1;
                }
            }
        }
    }
    /// Merge a character that continues an emoji cluster onto the preceding
    /// glyph cell's [`Cell::tail`] instead of printing it as an independent
    /// glyph. Three cases, checked in order:
    ///
    /// 1. The preceding cell's tail already ends in a ZWJ: a ZWJ always
    ///    announces that whatever comes next joins the sequence (e.g. the next
    ///    person in a family emoji), regardless of that character's own class
    ///    or width, so it merges unconditionally.
    /// 2. `ch` is a Fitzpatrick skin-tone modifier immediately following a bare
    ///    `Wide` base emoji (no tail yet).
    /// 3. `ch` is a second regional-indicator flag half immediately following a
    ///    lone first half (`Single`, no tail yet).
    ///
    /// Returns `false` (do nothing, let `print` proceed as normal) when none of
    /// these apply, or there is no eligible preceding cell, so an unpaired
    /// regional indicator or a modifier with no base still gets its own visible
    /// glyph rather than silently vanishing.
    pub(super) fn try_merge_continuation(&mut self, ch: char) -> bool {
        let Some(index) = self.last_glyph_cell_index() else {
            return false;
        };
        let prev = &self.cells[index];
        if prev
            .tail
            .as_deref()
            .is_some_and(|t| t.ends_with('\u{200D}'))
        {
            append_tail(&mut self.cells[index], ch);
            return true;
        }
        let is_skin_tone = is_skin_tone_modifier(ch);
        let is_ri = is_regional_indicator(ch);
        if !is_skin_tone && !is_ri {
            return false;
        }
        let eligible = if is_skin_tone {
            prev.width == CellWidth::Wide && prev.tail.is_none()
        } else {
            // Mirrors `combine_into_previous`'s own `!wrap_pending` guard: if
            // the first half is parked at the row's last column, there is no
            // column left to claim as its `Spacer`, and claiming the cursor's
            // (unmoved) column would overwrite the first half itself.
            prev.width == CellWidth::Single
                && is_regional_indicator(prev.ch)
                && prev.tail.is_none()
                && !self.cursor.wrap_pending
        };
        if !eligible {
            return false;
        }
        if is_ri {
            // Upgrade the first half from Single to Wide (the standard flag-glyph
            // shape) and claim the current column as its Spacer.
            self.cells[index].width = CellWidth::Wide;
            if let Some(spacer_index) = self.index(self.cursor.row, self.cursor.col) {
                let style = self.cells[spacer_index].style;
                self.cells[spacer_index] = Cell {
                    ch: ' ',
                    tail: None,
                    style,
                    width: CellWidth::Spacer,
                };
            }
            if self.cursor.col + 1 >= self.cols {
                self.cursor.wrap_pending = true;
            } else {
                self.cursor.col += 1;
            }
        }
        // A skin-tone modifier adds no columns: the base emoji already spans two.
        append_tail(&mut self.cells[index], ch);
        true
    }
}

/// `U+200D` ZWJ joins adjacent emoji into one sequence (e.g. the family/
/// profession combos), a variation selector (`U+FE00..=FE0F`; `FE0E`/`FE0F`
/// are the text/emoji presentation pair) picks a glyph's presentation, and
/// `U+20E3` (COMBINING ENCLOSING KEYCAP) turns a preceding digit/`#`/`*` into
/// a keycap emoji (e.g. `1️⃣`). None of these ever compose via
/// [`unicode_normalization::char::compose`], so [`Grid::combine_into_previous`]
/// appends them to the base cell's [`Cell::tail`] instead of dropping them.
pub(super) fn is_emoji_tail_mark(c: char) -> bool {
    matches!(c, '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{20E3}')
}
/// Fitzpatrick skin-tone modifiers (e.g. 👍🏽): these are double-width per
/// `unicode-width`, so they'd otherwise print as their own independent `Wide`
/// glyph next to the base emoji instead of tinting it.
pub(super) fn is_skin_tone_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}
/// Regional Indicator Symbols: a flag emoji (e.g. 🇺🇸) is two of these in a
/// row. Each is single-width per `unicode-width`, so a pair must be merged
/// explicitly rather than relying on the double-width path.
pub(super) fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}
/// Whether `ch` can be consumed as the space a word break happens at: a
/// blank cell. A spacer half of a double-width glyph is excluded by the
/// caller's width check, never here.
fn is_breakable_blank(ch: char) -> bool {
    ch == '\0' || ch.is_whitespace()
}
/// Append `c` to `cell`'s combining tail, creating it if this is the first.
pub(super) fn append_tail(cell: &mut Cell, c: char) {
    let mut tail = cell.tail.take().map_or_else(String::new, String::from);
    tail.push(c);
    cell.tail = Some(tail.into_boxed_str());
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::test_support::row_text;

    #[test]
    fn test_print_advances_cursor_and_wraps() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcd".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some('c'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('d'));
        assert_eq!(grid.cursor(), (1, 1));
    }
    #[test]
    fn test_deferred_wrap_parks_cursor_at_last_column() {
        // Filling the last column must NOT advance the cursor out of bounds or
        // wrap immediately; it parks at cols-1 with a pending wrap.
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some('c'));
        // Cursor stays on row 0 at the last column, not (0, 3) which is invalid.
        assert_eq!(grid.cursor(), (0, 2));
        assert!(grid.wrap_pending());
    }
    #[test]
    fn test_deferred_wrap_resolves_on_next_print() {
        // The pending wrap fires only when the next printable char arrives.
        let mut grid = Grid::new(3, 2);
        for ch in "abcd".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('d'));
    }
    #[test]
    fn test_combining_mark_composes_onto_previous_cell() {
        let mut grid = Grid::new(5, 2);
        grid.print('e');
        assert_eq!(grid.cursor(), (0, 1));
        grid.print('\u{0301}'); // combining acute accent
        assert_eq!(grid.cell(0, 0).unwrap().ch, '\u{00e9}'); // é
        assert_eq!(grid.cursor(), (0, 1)); // the mark did not advance the cursor
    }
    #[test]
    fn test_noncomposable_mark_is_dropped() {
        let mut grid = Grid::new(5, 2);
        grid.print('x');
        grid.print('\u{0489}'); // combining mark with no precomposed form for 'x'
        assert_eq!(grid.cell(0, 0).unwrap().ch, 'x'); // unchanged
        assert_eq!(grid.cell(0, 1).unwrap().ch, ' '); // not written into the next cell
        assert_eq!(grid.cursor(), (0, 1));
    }
    #[test]
    fn test_wide_char_occupies_two_cells() {
        let mut grid = Grid::new(10, 2);
        grid.print('世');
        assert_eq!(grid.cell(0, 0).unwrap().ch, '世');
        assert_eq!(grid.cell(0, 0).unwrap().width, CellWidth::Wide);
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
        // The cursor advanced two columns, so a following char lands at col 2.
        assert_eq!(grid.cursor(), (0, 2));
        grid.print('x');
        assert_eq!(grid.cell(0, 2).unwrap().ch, 'x');
    }
    #[test]
    fn test_wide_char_wraps_at_right_margin() {
        let mut grid = Grid::new(3, 2);
        grid.print('a');
        grid.print('b');
        // Only one column remains, so the wide char wraps to the next line.
        grid.print('世');
        assert_eq!(grid.cell(1, 0).unwrap().ch, '世');
        assert_eq!(grid.cell(1, 0).unwrap().width, CellWidth::Wide);
        assert_eq!(grid.cell(1, 1).unwrap().width, CellWidth::Spacer);
    }
    #[test]
    fn test_zwj_sequence_appends_to_tail_of_the_wide_base_cell() {
        // Also a regression test for the Spacer-lookback fix: the base emoji is
        // a Wide cell, so `last_glyph_cell_index` must resolve past its Spacer
        // to attach the ZWJ sequence to cell (0, 0), not drop it on (0, 1).
        let mut grid = Grid::new(10, 2);
        grid.print('\u{1F468}'); // man (Wide)
        grid.print('\u{200D}');
        grid.print('\u{1F469}'); // woman
        grid.print('\u{200D}');
        grid.print('\u{1F467}'); // girl
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{1F468}');
        assert_eq!(cell.width, CellWidth::Wide);
        assert_eq!(
            cell.tail.as_deref(),
            Some("\u{200D}\u{1F469}\u{200D}\u{1F467}")
        );
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
        assert_eq!(grid.cursor(), (0, 2));
    }
    #[test]
    fn test_variation_selector_appends_to_tail_and_upgrades_to_wide() {
        let mut grid = Grid::new(5, 2);
        grid.print('\u{2764}'); // heavy black heart (text presentation by default, Single width)
        grid.print('\u{FE0F}'); // VS-16: request emoji presentation
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{2764}');
        assert_eq!(cell.tail.as_deref(), Some("\u{FE0F}"));
        // VS-16 requests the double-width color-emoji artwork, not the
        // narrow default text glyph, so the cell claims a second column.
        assert_eq!(cell.width, CellWidth::Wide);
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
        assert_eq!(grid.cursor(), (0, 2));
    }
    #[test]
    fn test_fully_qualified_keycap_sequence_upgrades_ascii_digit_to_wide() {
        // "1️⃣" (digit, VS-16, then the keycap enclosure) is the
        // fully-qualified form of the keycap emoji `1️⃣`: an ASCII base that
        // must still end up `Wide` with both marks preserved in its tail, or
        // the renderer has nothing to route through the color-emoji quad
        // pass and it stays a bare "1".
        let mut grid = Grid::new(5, 2);
        grid.print('1');
        grid.print('\u{FE0F}');
        grid.print('\u{20E3}');
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '1');
        assert_eq!(cell.tail.as_deref(), Some("\u{FE0F}\u{20E3}"));
        assert_eq!(cell.width, CellWidth::Wide);
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
        assert_eq!(grid.cursor(), (0, 2));
    }
    #[test]
    fn test_minimally_qualified_keycap_sequence_upgrades_to_wide() {
        // "#⃣" (no VS-16) is the minimally-qualified form of `#️⃣`; the
        // keycap enclosure alone must still trigger the same Wide promotion.
        let mut grid = Grid::new(5, 2);
        grid.print('#');
        grid.print('\u{20E3}');
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '#');
        assert_eq!(cell.tail.as_deref(), Some("\u{20E3}"));
        assert_eq!(cell.width, CellWidth::Wide);
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
    }
    #[test]
    fn test_skin_tone_modifier_merges_without_consuming_a_column() {
        let mut grid = Grid::new(10, 2);
        grid.print('\u{1F44D}'); // thumbs up (Wide)
        grid.print('\u{1F3FC}'); // medium-light skin tone
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{1F44D}');
        assert_eq!(cell.tail.as_deref(), Some("\u{1F3FC}"));
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
        // Only the base emoji's two columns were consumed, not a third for the modifier.
        assert_eq!(grid.cursor(), (0, 2));
    }
    #[test]
    fn test_regional_indicator_pair_merges_into_one_flag_cell() {
        let mut grid = Grid::new(10, 2);
        grid.print('\u{1F1FA}'); // U
        grid.print('\u{1F1F8}'); // S
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{1F1FA}');
        assert_eq!(cell.width, CellWidth::Wide);
        assert_eq!(cell.tail.as_deref(), Some("\u{1F1F8}"));
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
        assert_eq!(grid.cursor(), (0, 2));
    }
    #[test]
    fn test_lone_regional_indicator_is_not_paired() {
        let mut grid = Grid::new(10, 2);
        grid.print('\u{1F1FA}'); // U, no second half follows
        grid.print('x');
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{1F1FA}');
        assert_eq!(cell.width, CellWidth::Single);
        assert_eq!(cell.tail, None);
        assert_eq!(grid.cell(0, 1).unwrap().ch, 'x');
    }
    #[test]
    fn test_regional_indicator_pair_split_by_a_line_wrap_does_not_corrupt_the_first_half() {
        // Regression: when the first flag half lands in the last column
        // (wrap already pending), merging the second half used to upgrade
        // that cell to Wide and then immediately overwrite it with a blank
        // Spacer, since the cursor's column (unmoved while wrap is pending)
        // is that same cell.
        let mut grid = Grid::new(1, 2);
        grid.print('\u{1F1FA}'); // U, alone in the only column; wrap now pending
        grid.print('\u{1F1F8}'); // S, would complete the pair but there's no room
        let first = grid.cell(0, 0).unwrap();
        assert_eq!(first.ch, '\u{1F1FA}', "the first half must survive intact");
        assert_eq!(first.width, CellWidth::Single);
        assert_eq!(first.tail, None);
    }
    #[test]
    fn test_visible_line_end_ignores_trailing_blanks() {
        let mut grid = Grid::new(10, 2);
        for ch in "hi".chars() {
            grid.print(ch);
        }
        // "hi" then blank padding: end is the 'i' at col 1, not the grid width.
        assert_eq!(grid.visible_line_end(0), 1);
        // Trailing spaces are blank padding too.
        grid.print(' ');
        grid.print(' ');
        assert_eq!(grid.visible_line_end(0), 1);
        // A row with no printed content reports column 0.
        assert_eq!(grid.visible_line_end(1), 0);
    }

    #[test]
    fn test_word_wrap_moves_the_partial_word_down_with_the_wrap() {
        // "aaa bbbbbb" fills the row; the next character belongs to the same
        // word as the tail, so the wrap breaks at the space and carries
        // "bbbbbb" down instead of splitting it across the margin.
        let mut grid = Grid::new(10, 3);
        for ch in "aaa bbbbbbb".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "aaa");
        assert_eq!(row_text(&grid, 1), "bbbbbbb");
        assert!(grid.row_wraps(0), "still one logical line");
        assert_eq!(grid.absolute_row_break(grid.to_absolute_row(0)), Some(3));
    }

    #[test]
    fn test_word_wrap_without_a_space_falls_back_to_the_margin() {
        // One unbreakable word fills the row: breaking it anywhere is as good
        // as the margin, so the wrap lands exactly there like always.
        let mut grid = Grid::new(6, 3);
        for ch in "aaaaaabb".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "aaaaaa");
        assert_eq!(row_text(&grid, 1), "bb");
        assert_eq!(grid.absolute_row_break(grid.to_absolute_row(0)), None);
    }

    #[test]
    fn test_word_wrap_breaks_at_the_rightmost_space_not_the_first() {
        // "aa bb cc" fills the row exactly; the break must take the last
        // space, carrying only "cc" down, so the first row holds as much as
        // fits rather than as little as the first word needs.
        let mut grid = Grid::new(8, 3);
        for ch in "aa bb ccc".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "aa bb");
        assert_eq!(row_text(&grid, 1), "ccc");
    }

    #[test]
    fn test_word_wrap_can_be_turned_off() {
        let mut grid = Grid::new(10, 3).with_word_wrap(false);
        for ch in "aaa bbbbbbb".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "aaa bbbbbb");
        assert_eq!(row_text(&grid, 1), "b");
        assert_eq!(grid.absolute_row_break(grid.to_absolute_row(0)), None);
    }

    #[test]
    fn test_word_wrap_skips_the_alternate_screen() {
        // A full-screen app addresses the grid by exact coordinates; moving
        // its cells under it would desync the screen it is redrawing against.
        let mut grid = Grid::new(10, 3);
        grid.enter_alt_screen();
        for ch in "aaa bbbbbbb".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "aaa bbbbbb");
        assert_eq!(row_text(&grid, 1), "b");
    }

    #[test]
    fn test_word_wrap_carries_the_word_beside_the_hanging_indent() {
        // With hanging indent on, the carried word lands at the indent; the
        // break's word must fit beside it or the wrap falls back to the
        // margin rather than pushing the word off the row.
        let mut grid = Grid::new(10, 4);
        for ch in "  aa bbbbbb".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.row_wrap_indent(1), 2, "fixture: the indent is 2");
        assert_eq!(row_text(&grid, 0), "  aa");
        assert_eq!(row_text(&grid, 1), "  bbbbbb");
    }

    #[test]
    fn test_a_carried_word_that_fills_the_row_parks_and_wraps_the_next_char() {
        // The carried word exactly fills the continuation row: the character
        // after it wraps onto the next row rather than addressing a column
        // past the edge, where it would be dropped on the floor.
        let mut grid = Grid::new(4, 4);
        for ch in "a bbbcc".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "a");
        assert_eq!(row_text(&grid, 1), "bbbc");
        assert_eq!(row_text(&grid, 2), "c");
        let (row, col) = grid.cursor();
        assert!(col < 4, "the cursor stays inside the row at ({row}, {col})");
    }

    #[test]
    fn test_word_wrap_keeps_a_wide_glyphs_halves_together_when_carried() {
        // The carried cells move as a block, so a double-width glyph and its
        // spacer arrive on the continuation row still adjacent.
        let mut grid = Grid::new(8, 3);
        for ch in "aa 世bbbb".chars() {
            grid.print(ch);
        }
        // The row filled with "aa", a space, the wide glyph, and "bbb";
        // the break at the space carries 世 and its spacer down whole.
        grid.print('c');
        assert_eq!(row_text(&grid, 0), "aa");
        assert_eq!(grid.cell(1, 0).unwrap().ch, '世');
        assert_eq!(grid.cell(1, 1).unwrap().width, CellWidth::Spacer);
        assert_eq!(grid.cell(1, 2).unwrap().ch, 'b');
        assert_eq!(grid.cell(1, 3).unwrap().ch, 'b');
        assert_eq!(grid.cell(1, 4).unwrap().ch, 'b');
        assert_eq!(grid.cell(1, 5).unwrap().ch, 'b');
        assert_eq!(grid.cell(1, 6).unwrap().ch, 'c');
    }

    /// Used to place the cursor at the first real character of a row.
    #[test]
    fn test_first_non_blank_col_skips_leading_blanks() {
        let mut grid = Grid::new(8, 2);
        grid.move_to(0, 3);
        grid.print('x');
        assert_eq!(grid.first_non_blank_col(0), 3);
        assert_eq!(grid.first_non_blank_col(1), 0, "a blank row reports 0");
    }

    /// A combining mark must land on the wide character's own cell, not on the
    /// spacer that occupies its right half.
    #[test]
    fn test_last_glyph_cell_index_steps_back_over_a_wide_spacer() {
        let mut grid = Grid::new(8, 2);
        grid.print('世');
        let idx = grid.last_glyph_cell_index().expect("a glyph was printed");
        assert_eq!(idx, grid.index(0, 0).unwrap(), "must skip the spacer half");
        assert_eq!(grid.cell(0, 1).unwrap().width, CellWidth::Spacer);
    }
}

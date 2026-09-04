//! Erasing regions and inserting or deleting characters in place.

use super::*;

// ========================================================================
// Items
// ========================================================================

impl Grid {
    /// A blank cell for erase and scroll fills, implementing Background Color
    /// Erase (BCE): the cleared cell takes the current pen background so that
    /// full-screen apps which set a background and then clear or scroll (nvim,
    /// btop, less) fill uniformly instead of letting the default background show
    /// through as bands. Only the background carries over; other attributes
    /// (bold, underline, foreground) reset, matching xterm.
    /// Clone-based equivalent of `[Cell]::copy_within`, which needs `Cell: Copy`.
    /// `Cell` can't be `Copy` once it carries a heap-allocated combining tail, so
    /// shifting a range of cells (scrolling, insert/delete line/char) goes through
    /// a temporary `Vec` instead; correct for overlapping ranges either way since
    /// the source is fully materialized before the destination is overwritten.
    pub(super) fn copy_cells_within(&mut self, src: std::ops::Range<usize>, dst: usize) {
        let tmp = self.cells[src].to_vec();
        self.cells[dst..dst + tmp.len()].clone_from_slice(&tmp);
    }
    pub(super) fn blank_cell(&self) -> Cell {
        Cell {
            ch: ' ',
            tail: None,
            style: Style {
                background: self.style.background,
                ..Style::default()
            },
            width: CellWidth::Single,
        }
    }
    /// Erase part of the cursor's line.
    pub fn erase_in_line(&mut self, mode: EraseMode) {
        let (start, end) = self.line_range(mode);
        for col in start..end {
            if let Some(index) = self.index(self.cursor.row, col) {
                self.cells[index] = self.blank_cell();
            }
        }
    }
    /// Erase part of the display.
    pub fn erase_in_display(&mut self, mode: EraseMode) {
        self.erase_in_line(mode);
        let (first, last) = match mode {
            EraseMode::ToEnd => (self.cursor.row + 1, self.rows),
            EraseMode::ToStart => (0, self.cursor.row),
            EraseMode::Whole => (0, self.rows),
        };
        for row in first..last {
            for col in 0..self.cols {
                if let Some(index) = self.index(row, col) {
                    self.cells[index] = self.blank_cell();
                }
            }
        }
        // Fully-erased rows no longer wrap; an erase to the cursor line's end
        // also breaks its wrap. (ToStart leaves the line's tail intact.)
        for flag in &mut self.row_wrapped[first..last] {
            *flag = false;
        }
        for indent in &mut self.row_wrap_indent[first..last] {
            *indent = 0;
        }
        if matches!(mode, EraseMode::ToEnd | EraseMode::Whole) {
            if let Some(flag) = self.row_wrapped.get_mut(self.cursor.row) {
                *flag = false;
            }
            if let Some(indent) = self.row_wrap_indent.get_mut(self.cursor.row) {
                *indent = 0;
            }
        }
    }
    /// Insert `n` blank characters at the cursor position, shifting characters
    /// to the right. Characters past the end of the row are lost.
    pub fn insert_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let shift = n.min(self.cols.saturating_sub(col));
        let row_start = row * self.cols;
        let src_start = row_start + col;
        let src_end = row_start + self.cols - shift;
        if src_start < src_end {
            self.copy_cells_within(src_start..src_end, src_start + shift);
        }
        for i in src_start..src_start + shift {
            if i < self.cells.len() {
                self.cells[i] = self.blank_cell();
            }
        }
    }
    /// Delete `n` characters at the cursor position, shifting characters from the
    /// right. Blank characters appear at the end of the row.
    pub fn delete_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let col = self.cursor.col;
        let shift = n.min(self.cols.saturating_sub(col));
        let row_start = row * self.cols;
        let dst = row_start + col;
        let src = dst + shift;
        let row_end = row_start + self.cols;
        if src < row_end {
            self.copy_cells_within(src..row_end, dst);
        }
        let clear_start = row_end.saturating_sub(shift);
        for i in clear_start..row_end {
            self.cells[i] = self.blank_cell();
        }
    }
    /// Erase `n` characters starting at the cursor, replacing them with blank
    /// cells without moving the cursor or shifting the rest of the line (ECH).
    /// Spinners that repaint a fixed-width field clear it with this before
    /// redrawing the next frame.
    pub fn erase_chars(&mut self, n: usize) {
        let row = self.cursor.row;
        let end = (self.cursor.col + n).min(self.cols);
        for col in self.cursor.col..end {
            if let Some(index) = self.index(row, col) {
                self.cells[index] = self.blank_cell();
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_erase_chars_blanks_in_place_without_moving_cursor() {
        let mut grid = Grid::new(6, 1);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 1);
        grid.erase_chars(2);
        assert_eq!(grid.to_text(), "a  def");
        // ECH does not shift the tail and leaves the cursor where it was.
        assert_eq!(grid.cursor(), (0, 1));
    }
    #[test]
    fn test_erase_chars_clamps_to_row_end() {
        let mut grid = Grid::new(4, 1);
        for ch in "wxyz".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 2);
        grid.erase_chars(99);
        assert_eq!(grid.to_text(), "wx");
    }
    #[test]
    fn test_erase_in_line_to_end_clears_from_cursor() {
        let mut grid = Grid::new(5, 1);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 2);
        grid.erase_in_line(EraseMode::ToEnd);
        assert_eq!(grid.to_text(), "he");
    }
    #[test]
    fn test_erase_in_display_to_end_clears_from_cursor() {
        let mut grid = Grid::new(4, 2);
        for ch in "abcdefgh".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 2);
        grid.erase_in_display(EraseMode::ToEnd);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(' '));
    }
    #[test]
    fn test_erase_in_display_to_start_clears_to_cursor() {
        let mut grid = Grid::new(4, 2);
        for ch in "abcdefgh".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 1);
        grid.erase_in_display(EraseMode::ToStart);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 1).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 2).map(|c| c.ch), Some('g'));
    }
    #[test]
    fn test_insert_chars() {
        let mut grid = Grid::new(5, 1);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 1);
        grid.insert_chars(2);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(0, 2).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(0, 3).map(|c| c.ch), Some('e'));
        assert_eq!(grid.cell(0, 4).map(|c| c.ch), Some('l'));
    }
    #[test]
    fn test_delete_chars() {
        let mut grid = Grid::new(5, 1);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 1);
        grid.delete_chars(2);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('l'));
        assert_eq!(grid.cell(0, 4).map(|c| c.ch), Some(' '));
    }
    #[test]
    fn test_variation_selector_at_last_column_does_not_steal_its_own_cell() {
        // The base character fills the row's only column, so a pending wrap
        // is already flagged when VS-16 arrives; there's no column left on
        // this row to claim as a Spacer, and claiming the base glyph's own
        // (parked) column would silently erase it.
        let mut grid = Grid::new(1, 2);
        grid.print('\u{2764}');
        grid.print('\u{FE0F}');
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{2764}');
        assert_eq!(cell.tail.as_deref(), Some("\u{FE0F}"));
        assert_eq!(cell.width, CellWidth::Single);
    }
    #[test]
    fn test_erase_uses_background_color_erase() {
        // BCE: erasing while a background color is set fills cleared cells with
        // that background (not the default), so nvim/btop fill uniformly.
        let mut grid = Grid::new(4, 2);
        grid.set_style(Style {
            background: Color::Indexed(4),
            ..Style::default()
        });
        grid.erase_in_line(EraseMode::Whole);
        assert_eq!(grid.cell(0, 0).unwrap().style.background, Color::Indexed(4));
        // Only the background carries over; the cell is otherwise blank.
        assert_eq!(grid.cell(0, 0).unwrap().ch, ' ');
    }
}

//! Cursor motion, saved-cursor state, and cursor presentation.

use super::CursorShape;
use super::{Grid, SavedCursor};

// ========================================================================
// Grid: cursor motion
// ========================================================================

impl Grid {
    /// Move to the next row. If inside the scroll region at the bottom margin,
    /// scroll the region up. If outside the scroll region or not at the bottom
    /// margin, just move down. A line feed always clears a pending wrap.
    pub fn line_feed(&mut self) {
        self.cursor.wrap_pending = false;
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
    }
    /// Move to the previous row (Reverse Index). If at the top margin of the
    /// scroll region, scroll the region down instead of moving above it; the
    /// upward mirror of [`Self::line_feed`].
    pub fn reverse_index(&mut self) {
        self.cursor.wrap_pending = false;
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else if self.cursor.row > 0 {
            self.cursor.row -= 1;
        }
    }
    /// Move the cursor to column 0 of the current row (CR).
    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.cursor.wrap_pending = false;
    }
    /// Move the cursor one column left, stopping at column 0 (BS).
    pub fn backspace(&mut self) {
        self.cursor.wrap_pending = false;
        self.cursor.col = self.cursor.col.saturating_sub(1);
    }
    /// Move the cursor to (row, col), clamped to the grid (or the scroll region
    /// under origin mode).
    pub fn move_to(&mut self, row: usize, col: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.row = self.resolve_row(row);
        self.cursor.col = col.min(self.cols.saturating_sub(1));
    }
    /// Move the cursor up `n` rows (CUU). A cursor inside the scroll region
    /// stops at its top margin; one above it stops at the top of the screen.
    pub fn move_up(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        let top = if self.cursor.row >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        self.cursor.row = self.cursor.row.saturating_sub(n).max(top);
    }
    /// Move the cursor down `n` rows (CUD). A cursor inside the scroll region
    /// stops at its bottom margin; one below it stops at the last row.
    pub fn move_down(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        let bottom = if self.cursor.row <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.rows.saturating_sub(1)
        };
        self.cursor.row = (self.cursor.row + n).min(bottom);
    }
    /// Move to column 0 of the next row, scrolling at the bottom margin
    /// exactly as a line feed does (NEL, `ESC E`).
    pub fn next_line(&mut self) {
        self.carriage_return();
        self.line_feed();
    }
    /// Move the cursor left `n` columns, clamped to column 0 (CUB).
    pub fn move_left(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.col = self.cursor.col.saturating_sub(n);
    }
    /// Move the cursor right `n` columns, clamped to the last column (CUF).
    pub fn move_right(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.col = (self.cursor.col + n).min(self.cols.saturating_sub(1));
    }
    /// Move the cursor to an absolute column on the current row (CHA), clamped.
    /// Progress bars and spinners use this (often as `ESC[G`) to return to the
    /// start of the line and redraw in place, the same role a `\r` plays.
    pub fn move_to_column(&mut self, col: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.col = col.min(self.cols.saturating_sub(1));
    }
    /// Move the cursor to an absolute row in the current column (VPA), clamped
    /// (relative to the scroll region under origin mode).
    pub fn move_to_row(&mut self, row: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.row = self.resolve_row(row);
    }
    /// Save the cursor for a later [`Self::restore_cursor`] (DECSC, `ESC 7`):
    /// its position and pending wrap, the pen, the character sets, and
    /// origin mode, which is what a VT100 saves. A program that saves, draws
    /// a box in line-drawing glyphs and color, and restores, gets its own
    /// pen back.
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(SavedCursor {
            charsets: self.charsets,
            col: self.cursor.col,
            origin: self.modes.origin,
            row: self.cursor.row,
            style: self.style,
            wrap_pending: self.cursor.wrap_pending,
        });
    }
    /// Restore what [`Self::save_cursor`] saved (DECRC, `ESC 8`). With
    /// nothing saved, this homes the cursor with the default pen, character
    /// sets and origin mode, as xterm does. The cursor's shape is the
    /// program's DECSCUSR choice, not part of the save, and stays.
    pub fn restore_cursor(&mut self) {
        let saved = self.saved_cursor.unwrap_or_default();
        self.charsets = saved.charsets;
        self.modes.origin = saved.origin;
        self.style = saved.style;
        self.cursor.row = saved.row;
        self.cursor.col = saved.col;
        self.cursor.wrap_pending = saved.wrap_pending;
        // The save may predate a resize that shrank the grid, in which case
        // the restored position addresses rows or columns that no longer
        // exist. Row-indexed operations (`delete_chars`, `insert_chars`,
        // the erases) derive slice bounds from the cursor without
        // rechecking them against the buffer, so an unclamped restore turns
        // the next one into an out-of-bounds panic.
        self.clamp_cursor();
    }
    /// Force the cursor inside the current grid.
    ///
    /// Anything that installs a cursor position captured under different
    /// dimensions has to go through this: a resize can shrink the grid out from
    /// under a saved position, and the row-indexed operations trust the cursor.
    pub(super) fn clamp_cursor(&mut self) {
        self.cursor.row = self.cursor.row.min(self.rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(self.cols.saturating_sub(1));
    }
    /// The cursor shape the active program has explicitly requested via DECSCUSR,
    /// or `None` if it has never set one (so the host's per-mode shape applies).
    pub fn reported_cursor_shape(&self) -> Option<CursorShape> {
        self.cursor_shape_set.then_some(self.cursor.shape)
    }
    /// Whether the cursor is visible (DECTCEM). Hidden by CSI ?25l, shown by
    /// CSI ?25h; full-screen apps like btop hide it while drawing.
    pub fn cursor_visible(&self) -> bool {
        self.modes.cursor_visible
    }
    /// Set cursor shape (DECSCUSR).
    pub fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.cursor_shape_set = true;
        self.cursor.shape = shape;
    }
    /// Hand the cursor shape back to the host's configured one (DECSCUSR 0),
    /// as if the program had never set one.
    pub fn reset_cursor_shape(&mut self) {
        self.cursor_shape_set = false;
        self.cursor.shape = CursorShape::default();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::test_support::*;

    #[test]
    fn test_move_to_column_sets_absolute_column_and_clamps() {
        let mut grid = Grid::new(5, 2);
        grid.move_to(0, 4);
        grid.move_to_column(2);
        assert_eq!(grid.cursor(), (0, 2));
        grid.move_to_column(99);
        assert_eq!(grid.cursor(), (0, 4));
    }
    #[test]
    fn test_move_to_column_clears_pending_wrap() {
        // Fill the last column to set a pending wrap, then a CHA back to col 0
        // must clear it so the next print overwrites in place, not on row 1.
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.move_to_column(0);
        grid.print('X');
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('X'));
        assert_eq!(grid.cursor().0, 0);
    }
    #[test]
    fn test_move_to_row_sets_absolute_row_and_clamps() {
        let mut grid = Grid::new(4, 3);
        grid.move_to(0, 2);
        grid.move_to_row(1);
        assert_eq!(grid.cursor(), (1, 2));
        grid.move_to_row(99);
        assert_eq!(grid.cursor(), (2, 2));
    }
    #[test]
    fn test_move_to_clamps_to_bounds() {
        let mut grid = Grid::new(4, 3);
        grid.move_to(99, 99);
        assert_eq!(grid.cursor(), (2, 3));
    }
    #[test]
    fn test_backspace_moves_cursor_left() {
        let mut grid = Grid::new(5, 1);
        grid.print('a');
        grid.print('b');
        grid.backspace();
        assert_eq!(grid.cursor(), (0, 1));
        grid.print('X');
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('X'));
    }
    #[test]
    fn test_last_content_row_ignores_trailing_blank_rows() {
        let mut grid = Grid::new(10, 4);
        // Two rows of content, then blank padding rows below.
        for ch in "first".chars() {
            grid.print(ch);
        }
        grid.line_feed();
        grid.carriage_return();
        for ch in "second".chars() {
            grid.print(ch);
        }
        // Content ends at row 1; rows 2 and 3 are blank padding.
        assert_eq!(grid.last_content_row(), 1);
    }
    #[test]
    fn test_last_content_row_counts_single_char_at_column_zero() {
        let mut grid = Grid::new(10, 3);
        grid.line_feed();
        grid.print('x');
        // A lone 'x' at column 0 of row 1 still counts as content.
        assert_eq!(grid.last_content_row(), 1);
    }

    /// CUB must clamp at the left edge rather than wrapping or underflowing.
    #[test]
    fn test_move_left_clamps_at_column_zero() {
        let mut grid = Grid::new(10, 3);
        grid.move_to(0, 4);
        grid.move_left(2);
        assert_eq!(grid.cursor(), (0, 2));
        grid.move_left(99);
        assert_eq!(grid.cursor(), (0, 0));
    }

    /// CUF must clamp at the last column, never past the right edge.
    #[test]
    fn test_move_right_clamps_at_the_last_column() {
        let mut grid = Grid::new(10, 3);
        grid.move_right(3);
        assert_eq!(grid.cursor(), (0, 3));
        grid.move_right(99);
        assert_eq!(grid.cursor(), (0, 9));
    }

    /// RI steps up while there is room, and scrolls the region down at the top
    /// margin instead of moving the cursor above it.
    #[test]
    fn test_reverse_index_moves_up_then_scrolls_at_the_region_top() {
        let mut grid = Grid::new(4, 3);
        fill_rows(&mut grid, &['a', 'b', 'c']);
        grid.move_to(1, 0);
        grid.reverse_index();
        assert_eq!(grid.cursor().0, 0, "RI moves up when there is room");

        grid.reverse_index();
        assert_eq!(grid.cursor().0, 0, "RI must not move above the top margin");
        assert_eq!(row_text(&grid, 1), "aaaa", "the old top row shifted down");
    }
}

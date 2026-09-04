//! Cursor motion, saved-cursor state, and cursor presentation.

use super::CursorShape;
use super::Grid;

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
    /// Advance the cursor to the next 8-column tab stop.
    pub fn tab(&mut self) {
        self.cursor.wrap_pending = false;
        let next = (self.cursor.col / TAB_WIDTH + 1) * TAB_WIDTH;
        self.cursor.col = next.min(self.cols.saturating_sub(1));
    }
    /// Move the cursor to (row, col), clamped to the grid (or the scroll region
    /// under origin mode).
    pub fn move_to(&mut self, row: usize, col: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.row = self.resolve_row(row);
        self.cursor.col = col.min(self.cols.saturating_sub(1));
    }
    /// Move the cursor up `n` rows, clamped to the top of the screen (CUU).
    pub fn move_up(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.row = self.cursor.row.saturating_sub(n);
    }
    /// Move the cursor down `n` rows, clamped to the last row (CUD).
    pub fn move_down(&mut self, n: usize) {
        self.cursor.wrap_pending = false;
        self.cursor.row = (self.cursor.row + n).min(self.rows.saturating_sub(1));
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
    /// Save the cursor position and style for a later
    /// [`Self::restore_cursor`] (DECSC).
    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some(self.cursor);
    }
    /// Restore the cursor saved by [`Self::save_cursor`], or do nothing if
    /// none was saved (DECRC).
    pub fn restore_cursor(&mut self) {
        if let Some(saved) = self.saved_cursor {
            self.cursor = saved;
            // The save may predate a resize that shrank the grid, in which case
            // the restored position addresses rows or columns that no longer
            // exist. Row-indexed operations (`delete_chars`, `insert_chars`,
            // the erases) derive slice bounds from the cursor without
            // rechecking them against the buffer, so an unclamped restore turns
            // the next one into an out-of-bounds panic.
            self.clamp_cursor();
        }
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
        self.cursor_visible
    }
    /// Set cursor shape (DECSCUSR).
    pub fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.cursor_shape_set = true;
        self.cursor.shape = shape;
    }
}

pub(super) const TAB_WIDTH: usize = 8;

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_tab_advances_to_next_stop() {
        let mut grid = Grid::new(20, 1);
        grid.print('x');
        grid.tab();
        assert_eq!(grid.cursor(), (0, 8));
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
    fn test_carriage_return_resets_to_col_0() {
        let mut grid = Grid::new(5, 1);
        grid.print('a');
        grid.print('b');
        grid.carriage_return();
        assert_eq!(grid.cursor(), (0, 0));
    }
    #[test]
    fn test_save_restore_cursor() {
        let mut grid = Grid::new(5, 3);
        grid.move_to(2, 4);
        grid.save_cursor();
        grid.move_to(0, 0);
        assert_eq!(grid.cursor(), (0, 0));
        grid.restore_cursor();
        assert_eq!(grid.cursor(), (2, 4));
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
    #[test]
    fn test_reported_cursor_shape_unset_until_decscusr() {
        let mut grid = Grid::new(3, 2);
        assert_eq!(grid.reported_cursor_shape(), None);
        grid.set_cursor_shape(CursorShape::Bar);
        assert_eq!(grid.reported_cursor_shape(), Some(CursorShape::Bar));
    }
}

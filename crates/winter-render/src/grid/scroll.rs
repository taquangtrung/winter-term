//! Scrolling, the scroll region, and the scrollback history.

use super::Cell;
use super::Grid;

// ========================================================================
// Grid: scrolling and scrollback
// ========================================================================

impl Grid {
    /// Clear the scrollback history (CSI 3 J). Used by `clear` / `tput clear` so
    /// the pre-clear output is no longer scrollable; also returns the view to the
    /// live bottom so the scrollbar reflects the now-empty history.
    pub fn clear_scrollback(&mut self) {
        self.scrollback.clear();
        self.scrollback_wrapped.clear();
        self.scrollback_wrap_indent.clear();
        self.scroll_offset = 0;
    }
    /// Scroll the scroll region up by `n` rows. The top row of the region is
    /// saved to the scrollback buffer (only if the region starts at row 0);
    /// blank rows scroll in at the bottom of the region.
    pub fn scroll_up(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom.saturating_sub(top) + 1);
        if top == 0 && self.alt_buffer.is_none() {
            for row in 0..shift {
                let start = row * self.cols;
                let end = start + self.cols;
                let scrolled: Vec<Cell> = self.cells[start..end].to_vec();
                self.scrollback.push(scrolled);
                self.scrollback_wrapped
                    .push(self.row_wrapped.get(row).copied().unwrap_or(false));
                self.scrollback_wrap_indent
                    .push(self.row_wrap_indent.get(row).copied().unwrap_or(0));
            }
            if self.scrollback.len() > self.max_scrollback {
                let excess = self.scrollback.len() - self.max_scrollback;
                self.scrollback.drain(0..excess);
                self.scrollback_wrapped.drain(0..excess);
                self.scrollback_wrap_indent.drain(0..excess);
            }
        }
        let region_len = bottom + 1 - top;
        if shift < region_len {
            for row in top..=bottom - shift {
                let src = (row + shift) * self.cols;
                let dst = row * self.cols;
                let end = src + self.cols;
                self.copy_cells_within(src..end, dst);
            }
        }
        for row in (bottom + 1 - shift)..=bottom {
            let start = row * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = self.blank_cell();
            }
        }
        if shift < region_len {
            self.row_wrapped.copy_within(top + shift..=bottom, top);
            self.row_wrap_indent.copy_within(top + shift..=bottom, top);
        }
        for flag in &mut self.row_wrapped[(bottom + 1 - shift)..=bottom] {
            *flag = false;
        }
        for indent in &mut self.row_wrap_indent[(bottom + 1 - shift)..=bottom] {
            *indent = 0;
        }
        self.scroll_offset = 0;
    }
    /// Scroll the scroll region down by `n` rows. Blank rows scroll in at the
    /// top of the region; the bottom rows are discarded.
    pub fn scroll_down(&mut self, n: usize) {
        let top = self.scroll_top;
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom.saturating_sub(top) + 1);
        for row in (top + shift..=bottom).rev() {
            let src = (row - shift) * self.cols;
            let dst = row * self.cols;
            let end = src + self.cols;
            self.copy_cells_within(src..end, dst);
        }
        for row in top..top + shift {
            let start = row * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = self.blank_cell();
            }
        }
        let region_len = bottom + 1 - top;
        if shift < region_len {
            self.row_wrapped
                .copy_within(top..=bottom - shift, top + shift);
            self.row_wrap_indent
                .copy_within(top..=bottom - shift, top + shift);
        }
        for flag in &mut self.row_wrapped[top..top + shift] {
            *flag = false;
        }
        for indent in &mut self.row_wrap_indent[top..top + shift] {
            *indent = 0;
        }
    }
    /// How many rows of scrollback history are available.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }
    /// The current scroll offset (0 = no scroll, at the live view).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }
    /// Scroll up in history by `n` rows, clamped to the available scrollback.
    pub fn scroll_up_history(&mut self, n: usize) {
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }
    /// Scroll down in history by `n` rows, clamped to 0.
    pub fn scroll_down_history(&mut self, n: usize) {
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }
    /// Set the scroll offset directly, clamped to the available scrollback.
    pub fn set_scroll_offset(&mut self, offset: usize) {
        self.scroll_offset = offset.min(self.scrollback.len());
    }
    /// Confine scrolling to rows `top..=bottom`, each clamped to the grid
    /// (DECSTBM).
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.scroll_top = top.min(self.rows.saturating_sub(1));
        self.scroll_bottom = bottom.min(self.rows.saturating_sub(1));
        if self.scroll_top > self.scroll_bottom {
            std::mem::swap(&mut self.scroll_top, &mut self.scroll_bottom);
        }
        self.cursor.row = 0;
        self.cursor.col = 0;
        self.cursor.wrap_pending = false;
    }
    /// Restore the scroll region to the full screen.
    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }
    /// Insert `n` blank lines at the cursor row, shifting existing lines down
    /// within the scroll region. Lines that fall below the bottom margin are lost.
    pub fn insert_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        let shift = n.min(bottom - row + 1);
        for r in (row + shift..=bottom).rev() {
            let src = (r - shift) * self.cols;
            let dst = r * self.cols;
            let end = src + self.cols;
            self.copy_cells_within(src..end, dst);
        }
        for r in row..row + shift {
            let start = r * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = self.blank_cell();
            }
        }
        if shift <= bottom - row {
            self.row_wrapped
                .copy_within(row..=bottom - shift, row + shift);
            self.row_wrap_indent
                .copy_within(row..=bottom - shift, row + shift);
        }
        for flag in &mut self.row_wrapped[row..row + shift] {
            *flag = false;
        }
        for indent in &mut self.row_wrap_indent[row..row + shift] {
            *indent = 0;
        }
    }
    /// Insert `n` blank rows at screen row `row`, shifting the rows below it
    /// down. Rows pushed past the bottom of the screen move into scrollback
    /// (unlike [`Self::insert_lines`], which discards them), so live content
    /// survives when a block's reserved band grows. The cursor rides the
    /// shift when it sits at or below `row`. Ignored on the alternate screen,
    /// where reserved bands don't render.
    pub fn insert_rows_at(&mut self, row: usize, n: usize) {
        if n == 0 || row >= self.rows || self.alt_buffer.is_some() {
            return;
        }
        for _ in 0..n {
            let bottom = self.rows - 1;
            let start = bottom * self.cols;
            let end = start + self.cols;
            let displaced: Vec<Cell> = self.cells[start..end].to_vec();
            self.scrollback.push(displaced);
            self.scrollback_wrapped
                .push(self.row_wrapped.get(bottom).copied().unwrap_or(false));
            self.scrollback_wrap_indent
                .push(self.row_wrap_indent.get(bottom).copied().unwrap_or(0));
            self.copy_cells_within(row * self.cols..bottom * self.cols, (row + 1) * self.cols);
            self.row_wrapped.copy_within(row..bottom, row + 1);
            self.row_wrap_indent.copy_within(row..bottom, row + 1);
            for i in row * self.cols..row * self.cols + self.cols {
                self.cells[i] = self.blank_cell();
            }
            self.row_wrapped[row] = false;
            self.row_wrap_indent[row] = 0;
        }
        if self.scrollback.len() > self.max_scrollback {
            let excess = self.scrollback.len() - self.max_scrollback;
            self.scrollback.drain(0..excess);
            self.scrollback_wrapped.drain(0..excess);
            self.scrollback_wrap_indent.drain(0..excess);
        }
        if self.cursor.row >= row {
            self.cursor.row = (self.cursor.row + n).min(self.rows - 1);
        }
        self.scroll_offset = 0;
    }
    /// Delete `n` lines at the cursor row, shifting lines up from below within
    /// the scroll region. Blank lines appear at the bottom margin.
    pub fn delete_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        let bottom = self.scroll_bottom;
        let region_rows = bottom - row + 1;
        let shift = n.min(region_rows);
        // Rows that survive and move up, counted forward rather than as
        // `bottom - shift`: when the delete covers the whole region from row 0
        // that expression is `0 - 1` and underflows, so `CSI H` followed by
        // `CSI 999 M` crashed the terminal. Any program can send those two.
        let moved = region_rows - shift;
        for r in row..row + moved {
            let src = (r + shift) * self.cols;
            let dst = r * self.cols;
            let end = src + self.cols;
            self.copy_cells_within(src..end, dst);
        }
        for r in (bottom + 1 - shift)..=bottom {
            let start = r * self.cols;
            let end = start + self.cols;
            for i in start..end {
                self.cells[i] = self.blank_cell();
            }
        }
        if shift <= bottom - row {
            self.row_wrapped.copy_within(row + shift..=bottom, row);
            self.row_wrap_indent.copy_within(row + shift..=bottom, row);
        }
        for flag in &mut self.row_wrapped[(bottom + 1 - shift)..=bottom] {
            *flag = false;
        }
        for indent in &mut self.row_wrap_indent[(bottom + 1 - shift)..=bottom] {
            *indent = 0;
        }
    }
    /// Top of the scroll region (0-based row).
    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }
    /// Bottom of the scroll region (0-based row, inclusive).
    pub fn scroll_bottom(&self) -> usize {
        self.scroll_bottom
    }
}

/// Default cap on retained scrollback rows per grid. Overridable per pane
/// via [`Grid::with_max_scrollback`] and the `scrollback-lines` setting.
pub const MAX_SCROLLBACK: usize = 10_000;

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{Color, Style};
    use super::*;
    use crate::grid::test_support::*;

    #[test]
    fn test_deferred_wrap_cleared_by_carriage_return_for_spinner() {
        // A full-width progress bar redrawn with \r must overwrite in place,
        // not scroll to the next line.
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        // Line is full and pending; \r resets to column 0 and clears the wrap.
        grid.carriage_return();
        assert_eq!(grid.cursor(), (0, 0));
        grid.print('X');
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('X'));
        // Still on row 0 — no premature newline.
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('b'));
        assert_eq!(grid.cursor().0, 0);
    }
    #[test]
    fn test_line_feed_at_bottom_scrolls() {
        let mut grid = Grid::new(2, 2);
        grid.print('a');
        grid.carriage_return();
        grid.line_feed();
        grid.print('b');
        grid.line_feed(); // at bottom row -> scrolls
        assert_eq!(grid.to_text(), "b");
        assert_eq!(grid.cursor().0, 1);
    }
    #[test]
    fn test_scroll_up_shifts_content() {
        let mut grid = Grid::new(2, 3);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('c'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('e'));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some(' '));
    }
    #[test]
    fn test_clear_scrollback_empties_history_and_resets_offset() {
        let mut grid = Grid::new(2, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        grid.scroll_up_history(1);
        assert!(grid.scrollback_len() > 0);
        assert!(grid.scroll_offset() > 0);
        grid.clear_scrollback();
        assert_eq!(grid.scrollback_len(), 0);
        assert_eq!(grid.scroll_offset(), 0);
    }
    #[test]
    fn test_scroll_up_saves_to_scrollback() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        assert_eq!(grid.scrollback_len(), 1);
        assert_eq!(grid.scrollback[0][0].ch, 'a');
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('d'));
    }
    #[test]
    fn test_scroll_history_navigates_scrollback() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1);
        assert_eq!(grid.scrollback_len(), 1);
        grid.scroll_up_history(1);
        assert_eq!(grid.scroll_offset(), 1);
        grid.scroll_down_history(1);
        assert_eq!(grid.scroll_offset(), 0);
    }
    #[test]
    fn test_scroll_offset_clamps_to_zero() {
        let mut grid = Grid::new(3, 2);
        grid.scroll_down_history(10);
        assert_eq!(grid.scroll_offset(), 0);
    }
    #[test]
    fn test_to_absolute_row_is_stable_as_scroll_offset_changes() {
        // Regression: a Selection endpoint captured at one scroll offset must
        // still name the same line after further scrolling, or a held-button
        // drag that auto-scrolls the view would silently reinterpret its
        // anchor as different text.
        let mut grid = Grid::new(3, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1); // scrollback: "abc"; live: "def", blank.

        let abs_before = grid.to_absolute_row(0); // Viewport row 0 shows "def".
        assert_eq!(grid.absolute_cell(abs_before, 0).map(|c| c.ch), Some('d'));

        grid.scroll_up_history(1); // "def" shifts down to viewport row 1.
        let abs_after = grid.to_absolute_row(1);
        assert_eq!(abs_before, abs_after);
        assert_eq!(grid.absolute_cell(abs_after, 0).map(|c| c.ch), Some('d'));
    }
    #[test]
    fn test_absolute_cell_reads_scrollback_and_live_rows() {
        let mut grid = Grid::new(3, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_up(1); // scrollback: "abc"; live: "def", blank.

        assert_eq!(
            grid.absolute_cell(0, 0).map(|c| c.ch),
            Some('a'),
            "abs row 0 is the oldest scrollback row"
        );
        assert_eq!(
            grid.absolute_cell(1, 0).map(|c| c.ch),
            Some('d'),
            "abs row 1 is the first live row"
        );
        assert!(
            grid.absolute_cell(3, 0).is_none(),
            "past the last live row is out of bounds"
        );
    }
    #[test]
    fn test_scroll_region() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdefghijkl".chars() {
            grid.print(ch);
        }
        grid.set_scroll_region(1, 2);
        grid.move_to(2, 0);
        grid.line_feed();
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('g'));
    }
    #[test]
    fn test_reset_scroll_region() {
        let mut grid = Grid::new(3, 4);
        grid.set_scroll_region(1, 2);
        grid.reset_scroll_region();
        assert_eq!(grid.scroll_top, 0);
        assert_eq!(grid.scroll_bottom, 3);
    }
    #[test]
    fn test_insert_lines() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdefghijkl".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 0);
        grid.insert_lines(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('d'));
        assert_eq!(grid.cell(3, 0).map(|c| c.ch), Some('g'));
    }
    #[test]
    fn test_delete_lines() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdefghijkl".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 0);
        grid.delete_lines(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('g'));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('j'));
        assert_eq!(grid.cell(3, 0).map(|c| c.ch), Some(' '));
    }
    #[test]
    fn test_scroll_down_inserts_blank_at_top() {
        let mut grid = Grid::new(2, 3);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.scroll_down(1);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.cell(1, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(2, 0).map(|c| c.ch), Some('c'));
    }
    #[test]
    fn test_scroll_up_blank_rows_use_background_color() {
        let mut grid = Grid::new(2, 3);
        grid.set_style(Style {
            background: Color::Indexed(2),
            ..Style::default()
        });
        grid.scroll_up(1);
        assert_eq!(grid.cell(2, 0).unwrap().style.background, Color::Indexed(2));
    }
    #[test]
    fn test_scroll_region_accessors() {
        let mut grid = Grid::new(10, 5);
        assert_eq!(grid.scroll_top(), 0);
        assert_eq!(grid.scroll_bottom(), 4);
        grid.set_scroll_region(1, 3);
        assert_eq!(grid.scroll_top(), 1);
        assert_eq!(grid.scroll_bottom(), 3);
        grid.reset_scroll_region();
        assert_eq!(grid.scroll_top(), 0);
        assert_eq!(grid.scroll_bottom(), 4);
    }
    #[test]
    fn test_delete_lines_covering_the_whole_region_from_row_zero() {
        // Regression: `bottom - shift` underflowed when the delete covered the
        // entire scroll region starting at row 0, so `CSI H` then `CSI 999 M`
        // crashed the terminal. Any program that can write to a PTY can send
        // those two bytes sequences.
        let mut grid = Grid::new(10, 5);
        for line in ["one", "two", "three", "four", "five"] {
            for ch in line.chars() {
                grid.print(ch);
            }
            grid.carriage_return();
            grid.line_feed();
        }
        grid.move_to(0, 0);
        grid.delete_lines(9999);

        assert_eq!(grid.cursor(), (0, 0));
        assert_eq!(
            grid.to_text(),
            "",
            "deleting the whole region should leave it blank"
        );
        for row in 0..grid.rows() {
            for col in 0..grid.cols() {
                assert!(grid.cell(row, col).is_some());
            }
        }
    }
    #[test]
    fn test_delete_lines_of_part_of_the_region_shifts_the_rest_up() {
        // The other side of the same fix: a partial delete must still move the
        // surviving rows up rather than blanking everything.
        let mut grid = Grid::new(10, 4);
        // No trailing line feed: a fourth one at the bottom row would scroll
        // "aaa" into scrollback and shift what is on screen.
        for (index, line) in ["aaa", "bbb", "ccc", "ddd"].iter().enumerate() {
            if index > 0 {
                grid.carriage_return();
                grid.line_feed();
            }
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        grid.move_to(0, 0);
        grid.delete_lines(2);
        assert_eq!(grid.to_text(), "ccc\nddd");
    }
    #[test]
    fn test_insert_rows_at_shifts_rows_below_and_saves_the_bottom_to_scrollback() {
        // Regression: a band growing mid-screen must shift the rows below it
        // down, and the row pushed off the bottom must land in scrollback —
        // `insert_lines` (CSI L) would discard it, eating the output beneath
        // the block on every growth.
        let mut grid = Grid::new(4, 3);
        fill_rows(&mut grid, &['A', 'B', 'C']);

        grid.insert_rows_at(1, 1);

        assert_eq!(grid.visible_cell(0, 0).map(|c| c.ch), Some('A'));
        assert_eq!(grid.visible_cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.visible_cell(2, 0).map(|c| c.ch), Some('B'));
        assert_eq!(grid.scrollback_len(), 1);
        // The displaced 'C' row is the newest scrollback line.
        assert_eq!(grid.absolute_cell(0, 0).map(|c| c.ch), Some('C'));
    }
    #[test]
    fn test_insert_rows_at_moves_the_cursor_with_the_shift() {
        // The shell's cursor sits below the grown band; if it stayed put the
        // next print would overwrite the shifted content.
        let mut grid = Grid::new(4, 4);
        fill_rows(&mut grid, &['A', 'B', 'C', 'D']);
        grid.move_to(2, 0);

        grid.insert_rows_at(1, 1);

        assert_eq!(grid.cursor().0, 3);
        // A cursor above the insert point is untouched.
        grid.move_to(0, 0);
        grid.insert_rows_at(2, 1);
        assert_eq!(grid.cursor().0, 0);
    }
}

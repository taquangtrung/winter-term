//! Scrolling, the scroll region, and the scrollback history.

use super::{Grid, Row, RowMeta};

// ========================================================================
// Constants
// ========================================================================

/// Default cap on retained scrollback lines per grid. Overridable per pane
/// via [`Grid::with_max_scrollback`] and the `scrollback-lines` setting.
pub const MAX_SCROLLBACK: usize = 10_000;

/// How many rows the history may hold per line of its cap: a bound on memory
/// when lines are long, since the cap itself counts lines, not rows.
pub(super) const HISTORY_ROW_CEILING: usize = 4;

// ========================================================================
// Grid: scrolling and scrollback
// ========================================================================

impl Grid {
    // --------------------------------------------------------------------
    // History
    // --------------------------------------------------------------------

    /// Clear the scrollback history (CSI 3 J). Used by `clear` / `tput clear` so
    /// the pre-clear output is no longer scrollable; also returns the view to the
    /// live bottom so the scrollbar reflects the now-empty history.
    pub fn clear_scrollback(&mut self) {
        // Absolute addressing stays monotonic across the clear, so anchors into
        // the discarded history resolve to nothing rather than silently
        // re-pointing at whatever line later lands at the same index.
        self.trimmed_rows += self.scrollback.len();
        self.scrollback.clear();
        self.history_lines = 0;
        self.scroll_offset = 0;
    }
    /// How many rows of scrollback history are available.
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }
    /// Choose whether new output returns a scrolled-back view to the live
    /// bottom (the default) or leaves it on the history it shows, so a user
    /// reading back is not yanked away by a program that keeps printing.
    pub fn set_scroll_to_bottom_on_output(&mut self, enabled: bool) {
        self.scroll_to_bottom_on_output = enabled;
    }
    /// Whether new output returns a scrolled-back view to the live bottom.
    pub fn scroll_to_bottom_on_output(&self) -> bool {
        self.scroll_to_bottom_on_output
    }
    /// Copy live row `row` onto the end of the history, then evict the oldest
    /// lines past the cap (see [`Self::trim_history`]), reusing an evicted
    /// row's allocation for the next row pushed.
    fn push_history(&mut self, row: usize) {
        let start = row * self.cols;
        let meta = self.row_meta.get(row).copied().unwrap_or_default();
        let mut cells = self
            .spare_row
            .take()
            .unwrap_or_else(|| Vec::with_capacity(self.cols));
        cells.clear();
        cells.extend_from_slice(&self.cells[start..start + self.cols]);
        self.scrollback.push_back(Row { cells, meta });
        if !meta.wrapped {
            self.history_lines += 1;
        }
        self.trim_history();
    }
    /// Evict whole lines from the front of the history while it holds more
    /// lines than its cap, or more rows than [`HISTORY_ROW_CEILING`] allows.
    /// A line goes with all its rows, so no line is left cut at its top.
    pub(super) fn trim_history(&mut self) {
        let row_ceiling = self.max_scrollback.saturating_mul(HISTORY_ROW_CEILING);
        while self.history_lines > self.max_scrollback || self.scrollback.len() > row_ceiling {
            while let Some(evicted) = self.scrollback.pop_front() {
                self.trimmed_rows += 1;
                let ended_line = !evicted.meta.wrapped;
                self.spare_row = Some(evicted.cells);
                if ended_line {
                    self.history_lines -= 1;
                    break;
                }
            }
            if self.scrollback.is_empty() {
                self.history_lines = 0;
                break;
            }
        }
        // A view scrolled back past what is left would read rows that are
        // gone: `scrollback.len() - scroll_offset` underflows.
        self.scroll_offset = self.scroll_offset.min(self.scrollback.len());
    }
    /// Bring the view back to the live bottom after output, or, with
    /// [`Self::scroll_to_bottom_on_output`] off, keep it on the history it
    /// showed: `added` rows just joined the history below the viewed ones.
    fn follow_output(&mut self, added: usize) {
        self.scroll_offset = match self.scroll_to_bottom_on_output || self.scroll_offset == 0 {
            true => 0,
            false => (self.scroll_offset + added).min(self.scrollback.len()),
        };
    }

    // --------------------------------------------------------------------
    // Shifting live rows
    // --------------------------------------------------------------------

    /// Move rows `first..=last` up by `n`, blanking the `n` rows freed at
    /// `last`. Rotates in place: no row is cloned or allocated.
    fn shift_rows_up(&mut self, first: usize, last: usize, n: usize) {
        let n = n.min(last + 1 - first);
        self.cells[first * self.cols..(last + 1) * self.cols].rotate_left(n * self.cols);
        self.row_meta[first..=last].rotate_left(n);
        self.blank_rows(last + 1 - n, last + 1);
    }
    /// Move rows `first..=last` down by `n`, blanking the `n` rows freed at
    /// `first`. The mirror of [`Self::shift_rows_up`].
    fn shift_rows_down(&mut self, first: usize, last: usize, n: usize) {
        let n = n.min(last + 1 - first);
        self.cells[first * self.cols..(last + 1) * self.cols].rotate_right(n * self.cols);
        self.row_meta[first..=last].rotate_right(n);
        self.blank_rows(first, first + n);
    }
    /// Blank rows `first..end` in the erase background, with no wrap facts.
    fn blank_rows(&mut self, first: usize, end: usize) {
        let blank = self.blank_cell();
        self.cells[first * self.cols..end * self.cols].fill(blank);
        self.row_meta[first..end].fill(RowMeta::default());
    }

    // --------------------------------------------------------------------
    // Scrolling
    // --------------------------------------------------------------------

    /// Scroll the scroll region up by `n` rows. The top row of the region is
    /// saved to the scrollback buffer (only if the region starts at row 0);
    /// blank rows scroll in at the bottom of the region.
    pub fn scroll_up(&mut self, n: usize) {
        let (top, bottom) = (self.scroll_top, self.scroll_bottom);
        let shift = n.min(bottom.saturating_sub(top) + 1);
        let saved = top == 0 && self.alt_buffer.is_none();
        if saved {
            for row in 0..shift {
                self.push_history(row);
            }
        }
        self.shift_rows_up(top, bottom, shift);
        self.follow_output(if saved { shift } else { 0 });
    }
    /// Scroll the scroll region down by `n` rows. Blank rows scroll in at the
    /// top of the region; the bottom rows are discarded.
    pub fn scroll_down(&mut self, n: usize) {
        self.shift_rows_down(self.scroll_top, self.scroll_bottom, n);
    }
    /// The current scroll offset (0 = no scroll, at the live view).
    pub fn scroll_offset(&self) -> usize {
        self.scroll_offset
    }
    /// Scroll up in history by `n` rows, clamped to the available scrollback.
    /// Ignored on the alternate screen, which has no history of its own: the
    /// retained rows belong to the primary buffer, so scrolling a full-screen
    /// app's viewport into them paints unrelated shell output over the app.
    pub fn scroll_up_history(&mut self, n: usize) {
        if self.alt_buffer.is_some() {
            return;
        }
        let max = self.scrollback.len();
        self.scroll_offset = (self.scroll_offset + n).min(max);
    }
    /// Scroll down in history by `n` rows, clamped to 0. Ignored on the
    /// alternate screen, for the reason on [`Self::scroll_up_history`].
    pub fn scroll_down_history(&mut self, n: usize) {
        if self.alt_buffer.is_some() {
            return;
        }
        self.scroll_offset = self.scroll_offset.saturating_sub(n);
    }
    /// Scroll the view so absolute row `abs_row` (see
    /// [`Self::to_absolute_row`]) is its top row, as near as the history
    /// allows: a live row returns the view to the bottom, and a row already
    /// evicted shows the oldest one kept.
    pub fn scroll_to_absolute_row(&mut self, abs_row: usize) {
        self.set_scroll_offset(self.absolute_live_top().saturating_sub(abs_row));
    }
    /// Set the scroll offset directly, clamped to the available scrollback.
    /// Ignored on the alternate screen, for the reason on
    /// [`Self::scroll_up_history`].
    pub fn set_scroll_offset(&mut self, offset: usize) {
        if self.alt_buffer.is_some() {
            return;
        }
        self.scroll_offset = offset.min(self.scrollback.len());
    }

    // --------------------------------------------------------------------
    // Scroll region
    // --------------------------------------------------------------------

    /// Confine scrolling to rows `top..=bottom`, each clamped to the grid
    /// (DECSTBM), and home the cursor: to the region's top-left under origin
    /// mode (DECOM), where home is relative to the margins, else to (0, 0).
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        self.scroll_top = top.min(self.rows.saturating_sub(1));
        self.scroll_bottom = bottom.min(self.rows.saturating_sub(1));
        if self.scroll_top > self.scroll_bottom {
            std::mem::swap(&mut self.scroll_top, &mut self.scroll_bottom);
        }
        self.cursor.row = self.resolve_row(0);
        self.cursor.col = 0;
        self.cursor.wrap_pending = false;
    }
    /// Restore the scroll region to the full screen.
    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
    }
    /// Top of the scroll region (0-based row).
    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }
    /// Bottom of the scroll region (0-based row, inclusive).
    pub fn scroll_bottom(&self) -> usize {
        self.scroll_bottom
    }

    // --------------------------------------------------------------------
    // Inserting and deleting rows
    // --------------------------------------------------------------------

    /// Insert `n` blank lines at the cursor row, shifting existing lines down
    /// within the scroll region. Lines that fall below the bottom margin are lost.
    pub fn insert_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        self.shift_rows_down(row, self.scroll_bottom, n);
    }
    /// Delete `n` lines at the cursor row, shifting lines up from below within
    /// the scroll region. Blank lines appear at the bottom margin. A delete
    /// covering the whole region from row 0 (`CSI H` then `CSI 999 M`, which
    /// any program can send) blanks it rather than underflowing.
    pub fn delete_lines(&mut self, n: usize) {
        let row = self.cursor.row;
        if row < self.scroll_top || row > self.scroll_bottom {
            return;
        }
        self.shift_rows_up(row, self.scroll_bottom, n);
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
        let bottom = self.rows - 1;
        for _ in 0..n {
            self.push_history(bottom);
            self.shift_rows_down(row, bottom, 1);
        }
        if self.cursor.row >= row {
            self.cursor.row = (self.cursor.row + n).min(bottom);
        }
        self.follow_output(n);
    }
    /// Delete `n` rows starting at screen row `row`, shifting the rows below
    /// them up and blanking the rows freed at the bottom. The mirror of
    /// [`Self::insert_rows_at`], and the same shape as [`Self::delete_lines`]
    /// except that it ignores the scroll region and addresses a given row
    /// rather than the cursor's.
    ///
    /// Used to hand back the part of a block's reserved band the block turned
    /// out not to need, once its content has actually been laid out. The
    /// cursor rides the shift when it sits below the removed rows, so the
    /// shell's prompt stays under the block instead of being left with a gap
    /// above it. Ignored on the alternate screen, where reserved bands do not
    /// render.
    pub fn remove_rows_at(&mut self, row: usize, n: usize) {
        if n == 0 || row >= self.rows || self.alt_buffer.is_some() {
            return;
        }
        let n = n.min(self.rows - row);
        self.shift_rows_up(row, self.rows - 1, n);
        if self.cursor.row >= row + n {
            self.cursor.row -= n;
        } else if self.cursor.row > row {
            self.cursor.row = row;
        }
        self.follow_output(0);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{Color, DecMode, Style};
    use super::*;
    use crate::grid::test_support::*;

    #[test]
    fn test_history_scrolling_is_ignored_on_the_alt_screen() {
        // The scrollback that survives `enter_alt_screen` is the *primary*
        // buffer's. Scrolling a full-screen app's viewport into it painted
        // unrelated shell output over the app, so history scrolling is inert
        // until the app exits and the primary buffer comes back.
        let mut grid = Grid::new(4, 2);
        for line in ["a", "b", "c", "d"] {
            for ch in line.chars() {
                grid.print(ch);
            }
            grid.carriage_return();
            grid.line_feed();
        }
        assert!(grid.scrollback_len() > 0, "fixture needs history");

        grid.enter_alt_screen();
        grid.scroll_up_history(1);
        assert_eq!(
            grid.scroll_offset(),
            0,
            "no history on the alternate screen"
        );
        grid.set_scroll_offset(1);
        assert_eq!(
            grid.scroll_offset(),
            0,
            "nor by setting the offset directly"
        );

        // The primary buffer keeps its history, reachable again on return.
        grid.leave_alt_screen();
        grid.scroll_up_history(1);
        assert_eq!(grid.scroll_offset(), 1);
    }

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
        // Still on row 0: no premature newline.
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
        assert_eq!(grid.scrollback[0].cells[0].ch, 'a');
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
    fn test_remove_rows_at_pulls_the_rows_below_up_and_blanks_the_tail() {
        // Handing back the unused part of a block's band must close the gap:
        // the rows under it move up by exactly `n`, and the rows freed at the
        // bottom come back blank rather than repeating the old tail.
        // Six rows so the fixture's four lines fit without scrolling any of
        // them into history before the removal.
        let mut grid = Grid::new(3, 6);
        for line in ["aaa", "bbb", "ccc", "ddd"] {
            for ch in line.chars() {
                grid.print(ch);
            }
            grid.carriage_return();
            grid.line_feed();
        }
        grid.remove_rows_at(1, 2); // drop "bbb" and "ccc"
        let row = |r: usize| -> String {
            (0..3)
                .map(|c| grid.visible_cell(r, c).map(|x| x.ch).unwrap_or(' '))
                .collect()
        };
        assert_eq!(row(0), "aaa");
        assert_eq!(row(1), "ddd", "the row below the removal moved up by two");
        assert_eq!(row(2), "   ", "and the freed tail is blank");
        assert_eq!(row(3), "   ");
        assert_eq!(grid.scrollback_len(), 0, "nothing was pushed into history");
    }

    #[test]
    fn test_remove_rows_at_carries_the_cursor_so_the_prompt_stays_put() {
        // The shell's prompt sits below the band. If the cursor did not ride
        // the shift, shrinking the band would leave the prompt stranded a few
        // rows lower than the text it belongs to.
        let mut grid = Grid::new(3, 6);
        grid.move_to(5, 0);
        grid.remove_rows_at(1, 2);
        assert_eq!(grid.cursor().0, 3);

        // A cursor inside the removed span has nowhere to ride to; it lands on
        // the first surviving row rather than underflowing.
        let mut grid = Grid::new(3, 6);
        grid.move_to(2, 0);
        grid.remove_rows_at(1, 2);
        assert_eq!(grid.cursor().0, 1);
    }

    /// Print `lines` as separate lines (each ended by CR LF but the last),
    /// so each is one line of history rather than one soft-wrapped line.
    fn print_lines(grid: &mut Grid, lines: &[&str]) {
        for (i, line) in lines.iter().enumerate() {
            if i > 0 {
                grid.carriage_return();
                grid.line_feed();
            }
            for ch in line.chars() {
                grid.print(ch);
            }
        }
    }

    #[test]
    fn test_scroll_to_absolute_row_puts_that_row_at_the_top() {
        let mut grid = Grid::new(3, 2);
        print_lines(&mut grid, &["abc", "def", "ghi", "jkl"]);
        grid.scroll_to_absolute_row(1);
        assert_eq!(grid.to_absolute_row(0), 1);
        assert_eq!(grid.visible_cell(0, 0).map(|c| c.ch), Some('d'));
        grid.scroll_to_absolute_row(grid.absolute_live_top() + 1);
        assert_eq!(grid.scroll_offset(), 0, "a live row shows the live screen");
    }

    #[test]
    fn test_to_absolute_row_is_stable_as_the_retention_budget_evicts_history() {
        // Regression: absolute rows counted from the oldest *retained* line, so
        // every eviction slid the whole address space down by one. A long-lived
        // anchor (a rich block's reserved band, a `gv` selection) then named a
        // line further and further below the text it was captured on.
        let mut grid = Grid::new(3, 2).with_max_scrollback(2);
        print_lines(&mut grid, &["abc", "def", "ghi"]);
        grid.scroll_up(1); // scrollback: abc, def; live: ghi, blank.
        let abs_ghi = grid.to_absolute_row(0);
        assert_eq!(grid.absolute_cell(abs_ghi, 0).map(|c| c.ch), Some('g'));

        grid.scroll_up(1); // "abc" is evicted; scrollback: def, ghi.
        assert_eq!(grid.scrollback_len(), 2, "the budget held at two rows");
        assert_eq!(
            grid.absolute_cell(abs_ghi, 0).map(|c| c.ch),
            Some('g'),
            "the anchor still names the line it was captured on"
        );
        assert_eq!(
            grid.to_viewport_row(abs_ghi),
            -1,
            "now one row above the view"
        );
    }
    #[test]
    fn test_absolute_cell_reports_a_line_evicted_from_history_as_gone() {
        // An anchor whose line has been dropped must read as absent rather
        // than silently resolving to whatever line now sits at that index.
        let mut grid = Grid::new(3, 2).with_max_scrollback(2);
        print_lines(&mut grid, &["abc", "def", "ghi"]); // scrollback: abc; live: def, ghi.
        let abs_abc = grid.to_absolute_row(0) - grid.scrollback_len();
        assert_eq!(grid.absolute_cell(abs_abc, 0).map(|c| c.ch), Some('a'));

        grid.scroll_up(1); // scrollback: abc, def.
        grid.scroll_up(1); // "abc" falls off the front of the budget.
        assert_eq!(grid.absolute_cell(abs_abc, 0), None);
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
        // down, and the row pushed off the bottom must land in scrollback;
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

    #[test]
    fn test_decstbm_homes_to_the_margin_under_origin_mode() {
        // Under DECOM, home is the top-left of the scroll region, so setting
        // the margins must put the cursor there rather than at screen row 0.
        let mut grid = Grid::new(10, 10);
        grid.set_dec_mode(DecMode::Origin, true);
        grid.set_scroll_region(3, 7);
        assert_eq!(grid.cursor(), (3, 0));
        grid.set_dec_mode(DecMode::Origin, false);
        grid.set_scroll_region(3, 7);
        assert_eq!(grid.cursor(), (0, 0));
    }

    #[test]
    fn test_output_leaves_a_scrolled_back_view_put_when_asked_to() {
        // With the snap off, the history being read stays on screen while
        // output scrolls more rows in beneath it; with it on, output brings
        // the view back to the live bottom.
        let mut grid = Grid::new(4, 2);
        for _ in 0..6 {
            grid.line_feed();
        }
        grid.set_scroll_to_bottom_on_output(false);
        grid.set_scroll_offset(2);
        let viewed = grid.to_absolute_row(0);
        grid.line_feed();
        grid.line_feed();
        assert_eq!(grid.scroll_offset(), 4);
        assert_eq!(
            grid.to_absolute_row(0),
            viewed,
            "the same line stays on top"
        );
        grid.set_scroll_to_bottom_on_output(true);
        grid.line_feed();
        assert_eq!(grid.scroll_offset(), 0);
    }

    #[test]
    fn test_history_at_the_cap_keeps_the_newest_rows_in_order() {
        // Evicting at the cap reuses the oldest row's storage for the newest;
        // the retained rows must still read oldest to newest.
        let mut grid = Grid::new(3, 1).with_max_scrollback(2);
        for marker in ['a', 'b', 'c', 'd'] {
            grid.print(marker);
            grid.carriage_return();
            grid.line_feed();
        }
        let history: Vec<char> = (0..grid.scrollback_len())
            .map(|row| grid.scrollback[row].cells[0].ch)
            .collect();
        assert_eq!(history, vec!['c', 'd']);
        assert_eq!(grid.scrollback[1].cells.len(), 3);
    }
}

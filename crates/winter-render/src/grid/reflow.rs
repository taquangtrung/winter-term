//! Resizing the grid and reflowing wrapped rows to the new width.

use super::*;

// ========================================================================
// Items
// ========================================================================

impl Grid {
    /// Configure whether soft-wrapped continuation lines inherit hanging indent.
    pub fn with_wrap_indent(mut self, enabled: bool) -> Self {
        self.wrap_indent = enabled;
        self
    }
    /// Enable or disable hanging indent for soft-wrapped continuation lines.
    pub fn set_wrap_indent(&mut self, enabled: bool) {
        self.wrap_indent = enabled;
    }
    /// Whether hanging indent is currently active.
    pub fn wrap_indent(&self) -> bool {
        self.wrap_indent
    }
    /// The hanging indent width of visible `row`, in columns.
    pub fn row_wrap_indent(&self, row: usize) -> usize {
        if row >= self.rows {
            return 0;
        }
        let abs = self.to_absolute_row(row);
        self.absolute_row_wrap_indent(abs)
    }
    /// The hanging indent width of absolute line `abs_row`, in columns.
    pub fn absolute_row_wrap_indent(&self, abs_row: usize) -> usize {
        if abs_row < self.scrollback_wrap_indent.len() {
            self.scrollback_wrap_indent[abs_row]
        } else {
            let live_row = abs_row.saturating_sub(self.scrollback.len());
            self.row_wrap_indent.get(live_row).copied().unwrap_or(0)
        }
    }
    /// Whether absolute line `abs_row` soft-wrapped into the line below.
    pub fn absolute_row_wraps(&self, abs_row: usize) -> bool {
        if abs_row < self.scrollback_wrapped.len() {
            self.scrollback_wrapped[abs_row]
        } else if abs_row < self.scrollback.len() {
            self.absolute_cell(abs_row, self.cols.saturating_sub(1))
                .is_some_and(|cell| cell.ch != '\0' && !cell.ch.is_whitespace())
        } else {
            let live = abs_row - self.scrollback.len();
            self.row_wrapped.get(live).copied().unwrap_or(false)
        }
    }
    /// Whether visible row `row` soft-wrapped into the row below, rather than
    /// ending at a newline.
    pub fn row_wraps(&self, row: usize) -> bool {
        if row >= self.rows || self.cols == 0 {
            return false;
        }
        let abs = self.to_absolute_row(row);
        self.absolute_row_wraps(abs)
    }
    /// The inclusive span of visible rows making up the logical line that visible
    /// `row` belongs to, following soft wraps both ways (see [`Self::row_wraps`]).
    /// A line that continues past the viewport's edge is clipped to it.
    pub fn wrapped_row_span(&self, row: usize) -> (usize, usize) {
        let mut start = row.min(self.rows.saturating_sub(1));
        while start > 0 && self.row_wraps(start - 1) {
            start -= 1;
        }
        let mut end = row.min(self.rows.saturating_sub(1));
        while end + 1 < self.rows && self.row_wraps(end) {
            end += 1;
        }
        (start, end)
    }
    /// Reflow a flat `old_cols`×`old_rows` cell buffer (with per-row soft-wrap
    /// flags `wrapped` and a cursor at `(cur_row, cur_col)`) into a fresh
    /// `cols`×`rows` grid, replaying its logical lines through a throwaway
    /// [`Grid`] so wrapping, wide cells, and the cursor position are recomputed
    /// identically to [`Grid::resize`]. Returns the new cells, the new per-row
    /// wrap flags, and the recomputed cursor `(row, col)`.
    ///
    /// This is the shared reflow core, factored out so that a stored
    /// alternate-screen (primary) buffer can be reflowed alongside the live
    /// buffer when the grid is resized.
    pub(super) fn reflow_buffer(
        old_size: (usize, usize),
        src: &[Cell],
        wrapped: &[bool],
        cursor: (usize, usize),
        new_size: (usize, usize),
    ) -> (Vec<Cell>, Vec<bool>, usize, usize) {
        let (old_cols, old_rows) = old_size;
        let (cols, rows) = new_size;
        let (cur_row, cur_col) = cursor;
        let mut g = Grid::new(old_cols, old_rows);
        g.cells = src.to_vec();
        g.row_wrapped = wrapped.to_vec();
        g.cursor.row = cur_row.min(old_rows.saturating_sub(1));
        g.cursor.col = cur_col.min(old_cols.saturating_sub(1));
        g.resize(cols, rows);
        (g.cells, g.row_wrapped, g.cursor.row, g.cursor.col)
    }
    /// Resize the grid, reflowing the live screen: soft-wrapped rows are merged
    /// back into logical lines and re-wrapped at the new width, so narrowing no
    /// longer truncates content and widening no longer leaves ragged breaks.
    /// Hard newlines are preserved and the cursor follows its logical position.
    /// Scrollback keeps its existing per-row widths (it is not reflowed).
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 {
            return;
        }
        // A resize to the SAME dimensions is a no-op: the grid, cursor, and
        // scroll region are already correct for this size. Running the reflow
        // anyway would discard the shell's exact cursor position and replace it
        // with a re-derived approximation — e.g. a prompt "> " has its trailing
        // space trimmed during line collection, so the replayed line is just ">"
        // and the cursor falls from col 2 to col 1 (">|" instead of "> |"). This
        // happens whenever a pane is re-resized without its size actually
        // changing (such as the unchanged pane during a split on the other side,
        // or a focus change that re-runs resize_all_panes).
        if cols == self.cols && rows == self.rows {
            return;
        }

        // 1. Collect logical lines from the live grid, dropping wide-char spacers
        //    (the lead glyph is replayed and recreates them) and trimming trailing
        //    blank padding from each line's final row. Track the cursor's logical
        //    position so it can be restored after re-wrapping.
        let old_cols = self.cols;
        let old_rows = self.rows;
        let mut lines: Vec<Vec<Cell>> = Vec::new();
        let mut cur: Vec<Cell> = Vec::new();
        let mut cursor_target: Option<(usize, usize)> = None;
        for r in 0..self.rows {
            let is_cont = r > 0 && self.row_wrapped.get(r - 1).copied().unwrap_or(false);
            let indent = if is_cont {
                self.row_wrap_indent.get(r).copied().unwrap_or(0)
            } else {
                0
            };
            for c in 0..old_cols {
                if r == self.cursor.row && c == self.cursor.col {
                    cursor_target = Some((lines.len(), cur.len()));
                }
                if is_cont && c < indent {
                    continue;
                }
                let cell = self.cells[r * old_cols + c].clone();
                if cell.width != CellWidth::Spacer {
                    cur.push(cell);
                }
            }
            if !self.row_wrapped.get(r).copied().unwrap_or(false) {
                while cur.last().is_some_and(|l| {
                    l.ch == ' ' && l.style == Style::default() && l.width == CellWidth::Single
                }) {
                    cur.pop();
                }
                lines.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            lines.push(cur);
        }
        // Drop trailing empty lines, but never past the cursor's line.
        let keep = lines
            .iter()
            .rposition(|l| !l.is_empty())
            .map(|i| i + 1)
            .unwrap_or(0)
            .max(cursor_target.map_or(0, |(li, _)| li + 1));
        lines.truncate(keep);

        // 2. Reset to a blank grid at the new size (scrollback retained), then
        //    replay the logical lines through `print`, which rebuilds wrapping,
        //    wide cells, soft-wrap flags, and scrollback overflow consistently.
        self.cells = vec![Cell::default(); cols * rows];
        self.cols = cols;
        self.rows = rows;
        self.row_wrap_indent = vec![0; rows];
        self.row_wrapped = vec![false; rows];
        self.cursor = Cursor::default();
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);

        let saved_style = self.style;
        let saved_link = self.active_link;
        let mut new_cursor: Option<(usize, usize)> = None;
        for (i, line) in lines.iter().enumerate() {
            for (j, cell) in line.iter().enumerate() {
                self.style = cell.style;
                self.active_link = cell.style.link;
                self.print(cell.ch);
                for tail_ch in cell.tail.iter().flat_map(|t| t.chars()) {
                    self.print(tail_ch);
                }
                if cursor_target == Some((i, j)) {
                    // Capture *after* printing, not before: `print` defers a
                    // wrap from the previous character (parking the cursor on
                    // the old row until the next glyph arrives), so capturing
                    // beforehand would land a cursor whose target is the first
                    // character of a new wrapped row back on the old row/col
                    // instead. When this character itself deferred a wrap, the
                    // cursor is parked right where it was written (not yet
                    // advanced); otherwise back off the one-past-the-glyph
                    // advance `print` just applied.
                    let col = if self.cursor.wrap_pending {
                        self.cursor.col
                    } else {
                        self.cursor.col.saturating_sub(1)
                    };
                    new_cursor = Some((self.cursor.row, col));
                }
            }
            if new_cursor.is_none()
                && cursor_target.is_some_and(|(ci, cj)| ci == i && cj >= line.len())
            {
                new_cursor = Some((self.cursor.row, self.cursor.col));
            }
            if i + 1 < lines.len() {
                self.carriage_return();
                self.line_feed();
            }
        }
        self.style = saved_style;
        self.active_link = saved_link;

        let (cr, cc) = new_cursor.unwrap_or((self.cursor.row, self.cursor.col));
        self.cursor.row = cr.min(rows.saturating_sub(1));
        self.cursor.col = cc.min(cols.saturating_sub(1));
        self.cursor.wrap_pending = false;

        // 3. Reflow the stored primary buffer (captured when the alt screen was
        //    entered) to the new dimensions too, so it stays consistent with
        //    `cols`/`rows`. Without this, a resize while a fullscreen app owns
        //    the alt screen leaves the primary buffer at the old size; when the
        //    app quits and `leave_alt_screen` restores it into a grid whose
        //    `cols`/`rows` are now larger, the next erase panics with an
        //    out-of-bounds index (`cells.len() < cols * rows`). The primary
        //    buffer never had its soft-wrap flags saved, so reflow treats every
        //    row as a hard-broken line — matching how `leave_alt_screen` reports
        //    them (all unwrapped) and sufficient for the mostly-short shell
        //    prompt content it holds.
        if let Some(alt) = self.alt_buffer.as_mut() {
            let wrapped = vec![false; old_rows];
            let (cells, _wrapped, cr, cc) = Self::reflow_buffer(
                (old_cols, old_rows),
                &alt.cells,
                &wrapped,
                (alt.cursor.row, alt.cursor.col),
                (cols, rows),
            );
            alt.cells = cells;
            alt.cursor.row = cr.min(rows.saturating_sub(1));
            alt.cursor.col = cc.min(cols.saturating_sub(1));
            alt.cursor.wrap_pending = false;
        }
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
    fn test_noop_resize_preserves_cursor_at_trailing_space() {
        // Regression: a resize to the SAME dimensions must be a no-op. Previously
        // the grid reflow always ran, trimming the prompt's trailing space and
        // re-deriving the cursor approximately — so the cursor at "> |" (col 2)
        // fell to ">|" (col 1). This surfaced whenever a pane was re-resized
        // without its size actually changing (e.g. a horizontal split on the
        // other side re-running resize_all_panes over an unchanged pane), and the
        // shell never corrected it because the SIGWINCH reported the same size.
        let mut grid = Grid::new(10, 2);
        // Prompt "> " with the shell cursor after the space, at column 2.
        grid.print('>');
        grid.print(' ');
        assert_eq!(grid.cursor(), (0, 2));
        // A resize to identical dimensions must not move the cursor.
        grid.resize(10, 2);
        assert_eq!(grid.cursor(), (0, 2));
    }
    #[test]
    fn test_resize_while_in_alt_screen_does_not_panic_on_leave() {
        // Regression: a resize (e.g. zooming a pane) while a fullscreen app owns
        // the alternate screen must also reflow the stored primary buffer. Before
        // the fix, the primary buffer kept its old (smaller) dimensions; when the
        // app quit and `leave_alt_screen` restored it into a grid whose
        // `cols`/`rows` were now larger, the next erase panicked with an
        // out-of-bounds index.
        let mut grid = Grid::new(10, 5);
        // Put some content on the primary screen, then switch to the alt screen
        // (simulating a fullscreen app like btop).
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.enter_alt_screen();
        // Grow the grid while the alt screen is active (zoom / window resize).
        grid.resize(20, 10);
        // Leaving the alt screen must restore a buffer that matches the new dims.
        grid.leave_alt_screen();
        // An erase after leaving must not index out of bounds.
        grid.move_to(0, 0);
        grid.erase_in_display(EraseMode::Whole);
        assert_eq!(grid.cells.len(), 20 * 10);
    }
    #[test]
    fn test_resize_while_in_alt_screen_preserves_primary_content() {
        let mut grid = Grid::new(10, 5);
        for ch in "prompt".chars() {
            grid.print(ch);
        }
        grid.enter_alt_screen();
        grid.resize(20, 10);
        grid.leave_alt_screen();
        // The primary content survives the resize-through-alt-screen round trip.
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('p'));
        assert_eq!(grid.cell(0, 5).map(|c| c.ch), Some('t'));
    }
    #[test]
    fn test_resize_preserves_top_left() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.resize(2, 2);
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(grid.cell(0, 1).map(|c| c.ch), Some('b'));
        assert_eq!(grid.cols(), 2);
    }
    #[test]
    fn test_wrapped_row_span_covers_every_row_of_a_soft_wrapped_line() {
        // A line printed past the width auto-wraps; both of its rows belong to one
        // logical line, so `cursorline` (and anything else asking) gets the pair.
        let mut grid = Grid::new(4, 3);
        for ch in "abcdefg".chars() {
            grid.print(ch);
        }
        assert!(grid.row_wraps(0), "row 0 wrapped into row 1");
        assert_eq!(grid.wrapped_row_span(0), (0, 1));
        assert_eq!(grid.wrapped_row_span(1), (0, 1), "found from either row");
    }
    #[test]
    fn test_wrapped_row_span_is_a_single_row_for_an_unwrapped_line() {
        let mut grid = Grid::new(8, 3);
        for ch in "hi".chars() {
            grid.print(ch);
        }
        grid.carriage_return();
        grid.line_feed();
        for ch in "there".chars() {
            grid.print(ch);
        }
        assert!(!grid.row_wraps(0));
        assert_eq!(grid.wrapped_row_span(0), (0, 0));
        assert_eq!(grid.wrapped_row_span(1), (1, 1));
    }
    #[test]
    fn test_resize_narrow_rewraps_long_line() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        // "abc" soft-wrapped into "def"; narrowing already at width 3 is a no-op,
        // so widen then re-narrow to exercise rewrap both ways.
        grid.resize(6, 4);
        assert_eq!(row_text(&grid, 0), "abcdef");
        grid.resize(3, 4);
        assert_eq!(row_text(&grid, 0), "abc");
        assert_eq!(row_text(&grid, 1), "def");
    }
    #[test]
    fn test_resize_narrow_places_cursor_on_new_wrap_boundary() {
        // Regression: narrowing used to snapshot the cursor before replaying
        // the character it targets, so a character that becomes the first
        // one on a newly-wrapped row picked up the previous character's
        // still-parked (deferred-wrap) position instead of its own.
        let mut grid = Grid::new(6, 2);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        // Sit the cursor on 'd' (not past it): the character that becomes the
        // first one of the second row once narrowed to 3 columns.
        grid.move_to(0, 3);
        grid.resize(3, 4);
        assert_eq!(row_text(&grid, 0), "abc");
        assert_eq!(row_text(&grid, 1), "def");
        assert_eq!(grid.cursor(), (1, 0));
    }
    #[test]
    fn test_resize_widen_merges_soft_wrapped_line() {
        let mut grid = Grid::new(3, 4);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.resize(6, 4);
        assert_eq!(row_text(&grid, 0), "abcdef");
        assert_eq!(row_text(&grid, 1), "");
    }
    #[test]
    fn test_resize_preserves_hard_newlines() {
        let mut grid = Grid::new(6, 4);
        for ch in "ab".chars() {
            grid.print(ch);
        }
        grid.line_feed();
        grid.carriage_return();
        for ch in "cd".chars() {
            grid.print(ch);
        }
        grid.resize(3, 4);
        // The explicit newline must not be merged away by reflow.
        assert_eq!(row_text(&grid, 0), "ab");
        assert_eq!(row_text(&grid, 1), "cd");
    }
    #[test]
    fn test_zwj_sequence_survives_resize_reflow() {
        let mut grid = Grid::new(10, 2);
        grid.print('\u{1F468}');
        grid.print('\u{200D}');
        grid.print('\u{1F469}');
        grid.resize(12, 2);
        let cell = grid.cell(0, 0).unwrap();
        assert_eq!(cell.ch, '\u{1F468}');
        assert_eq!(cell.tail.as_deref(), Some("\u{200D}\u{1F469}"));
    }
    #[test]
    fn test_wrap_indent_continuation_line_indents_to_first_non_blank() {
        // Grid cols = 10, rows = 3. Line starts with 2 leading spaces: "  abc12345" (10 chars).
        let mut grid = Grid::new(10, 3).with_wrap_indent(true);
        for ch in "  abc12345XYZ".chars() {
            grid.print(ch);
        }
        // Row 0 has "  abc12345"
        assert!(grid.row_wraps(0));
        assert_eq!(grid.row_wrap_indent(0), 0);
        // Row 1 starts at col 2 (hanging indent of 2) with "XYZ"
        assert_eq!(grid.row_wrap_indent(1), 2);
        assert_eq!(grid.visible_cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(grid.visible_cell(1, 1).map(|c| c.ch), Some(' '));
        assert_eq!(grid.visible_cell(1, 2).map(|c| c.ch), Some('X'));
        assert_eq!(grid.visible_cell(1, 3).map(|c| c.ch), Some('Y'));
        assert_eq!(grid.visible_cell(1, 4).map(|c| c.ch), Some('Z'));
    }
    #[test]
    fn test_wrap_indent_disabled_starts_at_column_zero() {
        let mut grid = Grid::new(10, 3).with_wrap_indent(false);
        for ch in "  abc12345XYZ".chars() {
            grid.print(ch);
        }
        assert!(grid.row_wraps(0));
        assert_eq!(grid.row_wrap_indent(1), 0);
        assert_eq!(grid.visible_cell(1, 0).map(|c| c.ch), Some('X'));
        assert_eq!(grid.visible_cell(1, 1).map(|c| c.ch), Some('Y'));
        assert_eq!(grid.visible_cell(1, 2).map(|c| c.ch), Some('Z'));
    }
    #[test]
    fn test_wrap_indent_multi_row_inherits_first_line_indent() {
        // 10-column grid: 2 leading spaces, then fills 2 full rows and spills into 3rd row.
        let mut grid = Grid::new(10, 4).with_wrap_indent(true);
        // Row 0: "  01234567" (10 chars)
        // Row 1: "  89ABCDEF" (10 chars, 2 indent + 8 content)
        // Row 2: "  GH"
        for ch in "  0123456789ABCDEFGH".chars() {
            grid.print(ch);
        }
        assert!(grid.row_wraps(0));
        assert!(grid.row_wraps(1));
        assert_eq!(grid.row_wrap_indent(0), 0);
        assert_eq!(grid.row_wrap_indent(1), 2);
        assert_eq!(grid.row_wrap_indent(2), 2);
        assert_eq!(grid.visible_cell(2, 2).map(|c| c.ch), Some('G'));
        assert_eq!(grid.visible_cell(2, 3).map(|c| c.ch), Some('H'));
    }
    #[test]
    fn test_restoring_a_cursor_saved_before_a_shrink_is_clamped() {
        // Regression: DECSC captured a position, a resize shrank the grid, and
        // DECRC restored the stale row unclamped. The row-indexed operations
        // derive slice bounds from the cursor, so the next `CSI P` indexed past
        // the cell buffer and panicked. Reachable by resizing the window while
        // a full-screen app has the cursor saved.
        let mut grid = Grid::new(24, 10);
        grid.move_to(8, 20);
        grid.save_cursor();
        grid.resize(24, 5);
        grid.restore_cursor();

        let (row, col) = grid.cursor();
        assert!(
            row < grid.rows(),
            "restored row {row} outside {} rows",
            grid.rows()
        );
        assert!(
            col < grid.cols(),
            "restored col {col} outside {} cols",
            grid.cols()
        );

        // The operation that used to panic.
        grid.delete_chars(8);
        grid.insert_chars(8);
        grid.erase_chars(8);
    }
    #[test]
    fn test_wrap_indent_scrollback_retains_wrap_and_indent() {
        let mut grid = Grid::new(10, 2).with_wrap_indent(true);
        for ch in "  01234567XYZ".chars() {
            grid.print(ch);
        }
        // Scroll up into history
        grid.line_feed();
        assert_eq!(grid.scrollback_len(), 1);
        assert!(grid.absolute_row_wraps(0));
        assert_eq!(grid.absolute_row_wrap_indent(1), 2);
    }
}

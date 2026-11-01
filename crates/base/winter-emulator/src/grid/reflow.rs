//! Resizing the grid and reflowing wrapped rows to the new width.

use super::charset::Charsets;
use super::{Cell, CellWidth, RowMeta, RowRemap, Style};
use super::{Cursor, Grid, MAX_GRID_COLS, MAX_GRID_ROWS};

// ========================================================================
// Data Structures
// ========================================================================

/// A buffer reflowed to a new size by [`Grid::reflow_buffer`]: its cells and
/// row facts, where its cursor landed, and how its old rows map onto the new
/// ones (relative to the new live top).
struct ReflowedBuffer {
    cells: Vec<Cell>,
    cursor_col: usize,
    cursor_row: usize,
    remap: Option<RowRemap>,
    row_meta: Vec<RowMeta>,
}

/// The logical lines of a screen or history about to be reflowed, with where
/// the cursor sits among them.
pub(super) struct LogicalLines {
    /// `(line, offset)`: the cursor's line and its offset into that line's
    /// cells, which may lie past the line's (trimmed) end.
    pub(super) cursor: Option<(usize, usize)>,
    /// The row each line started on, parallel to `lines`.
    pub(super) line_starts: Vec<usize>,
    pub(super) lines: Vec<Vec<Cell>>,
}

/// The live state a reflow's replay must not be steered by, set aside for
/// the replay and put back after it. Replayed cells already hold what was
/// printed: the character sets have translated them, and they must wrap at
/// the new margin and never shift what came before them.
struct ReplayGuard {
    active_link: u16,
    auto_wrap: bool,
    charsets: Charsets,
    insert: bool,
    last_graphic: Option<char>,
    style: Style,
}

// ========================================================================
// Grid: resize and reflow
// ========================================================================

impl Grid {
    // --------------------------------------------------------------------
    // Wrap settings
    // --------------------------------------------------------------------

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
    /// Configure whether soft wraps break at word boundaries.
    pub fn with_word_wrap(mut self, enabled: bool) -> Self {
        self.word_wrap = enabled;
        self
    }
    /// Enable or disable breaking soft wraps at word boundaries.
    pub fn set_word_wrap(&mut self, enabled: bool) {
        self.word_wrap = enabled;
    }
    /// Whether soft wraps currently break at word boundaries.
    pub fn word_wrap(&self) -> bool {
        self.word_wrap
    }

    // --------------------------------------------------------------------
    // Row facts
    // --------------------------------------------------------------------

    /// The row facts of absolute line `abs_row`, from the history or the
    /// live screen, or `None` for a line evicted or past the screen.
    fn absolute_row_meta(&self, abs_row: usize) -> Option<RowMeta> {
        let row = self.to_retained_row(abs_row)?;
        match self.scrollback.get(row) {
            Some(history) => Some(history.meta),
            None => self.row_meta.get(row - self.scrollback.len()).copied(),
        }
    }
    /// The word-break column of absolute line `abs_row`: `Some(k)` when that
    /// row soft-wrapped at a word boundary, consuming the space at column
    /// `k`. Joining the logical line back into text contributes a space
    /// there, so a word-wrapped line copies with its words apart.
    pub fn absolute_row_break(&self, abs_row: usize) -> Option<usize> {
        self.absolute_row_meta(abs_row)?.break_col
    }
    /// The hanging indent width of visible `row`, in columns.
    pub fn row_wrap_indent(&self, row: usize) -> usize {
        if row >= self.rows {
            return 0;
        }
        self.absolute_row_wrap_indent(self.to_absolute_row(row))
    }
    /// The hanging indent width of absolute line `abs_row`, in columns.
    pub fn absolute_row_wrap_indent(&self, abs_row: usize) -> usize {
        self.absolute_row_meta(abs_row)
            .map_or(0, |meta| meta.indent)
    }
    /// Whether absolute line `abs_row` soft-wrapped into the line below.
    pub fn absolute_row_wraps(&self, abs_row: usize) -> bool {
        self.absolute_row_meta(abs_row)
            .is_some_and(|meta| meta.wrapped)
    }
    /// Whether visible row `row` soft-wrapped into the row below, rather than
    /// ending at a newline.
    pub fn row_wraps(&self, row: usize) -> bool {
        if row >= self.rows || self.cols == 0 {
            return false;
        }
        self.absolute_row_wraps(self.to_absolute_row(row))
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

    // --------------------------------------------------------------------
    // Resize
    // --------------------------------------------------------------------

    /// Reflow a flat `old_size` cell buffer (with its row facts `meta` and a
    /// cursor at `cursor`) into a fresh `new_size` grid, replaying its
    /// logical lines through a throwaway [`Grid`] so wrapping, wide cells,
    /// and the cursor position are recomputed identically to
    /// [`Grid::resize`]. The remap's rows are relative to the new live top,
    /// so the caller can rebase them into its own absolute address space.
    ///
    /// This is the shared reflow core, factored out so that a stored
    /// alternate-screen (primary) buffer can be reflowed alongside the live
    /// buffer when the grid is resized.
    fn reflow_buffer(
        &self,
        old_size: (usize, usize),
        src: &[Cell],
        meta: &[RowMeta],
        cursor: Cursor,
        new_size: (usize, usize),
    ) -> ReflowedBuffer {
        let (old_cols, old_rows) = old_size;
        let mut g = Grid::new(old_cols, old_rows);
        g.cells = src.to_vec();
        g.row_meta = meta.to_vec();
        g.cursor.row = cursor.row.min(old_rows.saturating_sub(1));
        g.cursor.col = cursor.col.min(old_cols.saturating_sub(1));
        g.resize(new_size.0, new_size.1);
        let scrolled = g.scrollback.len();
        let remap = g.take_row_remap().map(|remap| RowRemap {
            old_top: 0,
            new_abs: remap
                .new_abs
                .into_iter()
                .map(|abs| abs.saturating_sub(scrolled))
                .collect(),
        });
        ReflowedBuffer {
            cells: g.cells,
            cursor_col: g.cursor.col,
            cursor_row: g.cursor.row,
            remap,
            row_meta: g.row_meta,
        }
    }
    /// Resize the grid, reflowing the live screen: soft-wrapped rows are merged
    /// back into logical lines and re-wrapped at the new width, so narrowing no
    /// longer truncates content and widening no longer leaves ragged breaks.
    /// Hard newlines are preserved and the cursor follows its logical position.
    /// The live screen always wraps exactly at the margin, the way a program
    /// redrawing it expects. When the width changed and word wrap or hanging
    /// indent is on, the history is marked for a rebuild at the new width
    /// with them (see [`Self::rebuild_history`]); until then, and always
    /// with both off, it keeps the widths it was printed at.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == 0 || rows == 0 {
            return;
        }
        let (cols, rows) = (cols.min(MAX_GRID_COLS), rows.min(MAX_GRID_ROWS));
        // A resize to the SAME dimensions is a no-op: the grid, cursor, and
        // scroll region are already correct for this size, and a reflow
        // would only re-derive them. This happens whenever a pane is
        // re-resized without its size actually changing (such as the
        // unchanged pane during a split on the other side, or a focus change
        // that re-runs resize_all_panes).
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.tabs.resize(cols);
        let (old_cols, old_rows) = (self.cols, self.rows);
        let old_live_top = self.absolute_live_top();
        let logical = self.collect_logical_lines();

        // Reset to a blank grid at the new size (scrollback retained), then
        // replay the logical lines, which rebuilds wrapping, wide cells, soft
        // wrap facts, and scrollback overflow consistently.
        self.cells = vec![Cell::default(); cols * rows];
        self.cols = cols;
        self.rows = rows;
        self.row_meta = vec![RowMeta::default(); rows];
        self.cursor = Cursor::default();
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = rows.saturating_sub(1);
        let line_new_abs = self.replay_logical_lines(&logical);
        let mut remap = self.live_row_remap(&logical, &line_new_abs, old_live_top, old_rows);

        // Reflow the stored primary buffer (captured when the alt screen was
        // entered) to the new dimensions too, so it stays consistent with
        // `cols`/`rows`. Without this, a resize while a fullscreen app owns
        // the alt screen leaves the primary buffer at the old size; when the
        // app quits and `leave_alt_screen` restores it into a grid whose
        // `cols`/`rows` are now larger, the next erase panics with an
        // out-of-bounds index (`cells.len() < cols * rows`).
        if let Some(alt) = self.alt_buffer.take() {
            let mut alt = alt;
            let reflowed = self.reflow_buffer(
                (old_cols, old_rows),
                &alt.cells,
                &alt.row_meta,
                alt.cursor,
                (cols, rows),
            );
            alt.cells = reflowed.cells;
            alt.row_meta = reflowed.row_meta;
            alt.cursor.row = reflowed.cursor_row.min(rows.saturating_sub(1));
            alt.cursor.col = reflowed.cursor_col.min(cols.saturating_sub(1));
            alt.cursor.wrap_pending = false;
            // While the alt screen owns the live rows, the anchors the remap
            // is for (block bands, selections) name the stored primary
            // screen, so the published remap must describe THAT reflow;
            // pushing them through the alt screen's replay would re-point
            // them at the full-screen app's rows instead.
            if let Some(temp) = reflowed.remap {
                let live_top = self.absolute_live_top();
                remap = Some(RowRemap {
                    old_top: live_top,
                    new_abs: temp.new_abs.into_iter().map(|rel| live_top + rel).collect(),
                });
            }
            self.alt_buffer = Some(alt);
        }
        // Rebuilding the history at the new width is left to the owner, once
        // the size settles (see [`Self::rebuild_history`]): it replays every
        // history line, too slow to repeat on each step of a drag.
        if cols != old_cols && (self.word_wrap || self.wrap_indent) {
            self.history_stale = true;
        }
        self.pending_remap = remap;
    }
    /// Collect the live screen's logical lines, dropping wide-char spacers
    /// (the lead glyph is replayed and recreates them) and trimming trailing
    /// blank padding from each line's final row, and note where the cursor
    /// sits among them.
    fn collect_logical_lines(&self) -> LogicalLines {
        let mut logical = LogicalLines {
            cursor: None,
            line_starts: Vec::new(),
            lines: Vec::new(),
        };
        let mut current_line_start = 0usize;
        let mut cur: Vec<Cell> = Vec::new();
        for r in 0..self.rows {
            let meta = self.row_meta.get(r).copied().unwrap_or_default();
            let is_cont = r > 0 && self.row_meta.get(r - 1).is_some_and(|m| m.wrapped);
            let indent = if is_cont { meta.indent } else { 0 };
            // A word-broken row's cells after the break are blank residue: the
            // word moved down. Contributing them would re-insert phantom spaces
            // into the replayed line (and re-wrap it differently); contributing
            // through the break cell keeps the space the break consumed, so
            // the replay re-breaks at the same word.
            let row_end = meta.break_col.map_or(self.cols, |k| (k + 1).min(self.cols));
            for c in 0..row_end {
                if r == self.cursor.row && c == self.cursor.col {
                    logical.cursor = Some((logical.lines.len(), cur.len()));
                }
                if is_cont && c < indent {
                    continue;
                }
                let cell = &self.cells[r * self.cols + c];
                if cell.width != CellWidth::Spacer {
                    cur.push(cell.clone());
                }
            }
            // A cursor in a word-broken row's residue stands past the break.
            if r == self.cursor.row && self.cursor.col >= row_end {
                let past = self.cursor.col - row_end;
                logical.cursor = Some((logical.lines.len(), cur.len() + past));
            }
            if !meta.wrapped {
                while cur.last().is_some_and(is_trimmable_blank) {
                    cur.pop();
                }
                logical.lines.push(std::mem::take(&mut cur));
                logical.line_starts.push(current_line_start);
                current_line_start = r + 1;
            }
        }
        if !cur.is_empty() {
            logical.lines.push(cur);
            logical.line_starts.push(current_line_start);
        }
        // Drop trailing empty lines, but never past the cursor's line.
        let keep = logical
            .lines
            .iter()
            .rposition(|l| !l.is_empty())
            .map(|i| i + 1)
            .unwrap_or(0)
            .max(logical.cursor.map_or(0, |(li, _)| li + 1));
        logical.lines.truncate(keep);
        logical.line_starts.truncate(keep);
        logical
    }
    /// Print `logical`'s lines onto the blank grid one after another, put the
    /// cursor where its logical position landed, and return the absolute row
    /// each line now starts at.
    pub(super) fn replay_logical_lines(&mut self, logical: &LogicalLines) -> Vec<usize> {
        let guard = self.begin_replay();
        let mut new_cursor: Option<(usize, usize)> = None;
        let mut line_new_abs: Vec<usize> = Vec::with_capacity(logical.lines.len());
        for (i, line) in logical.lines.iter().enumerate() {
            // The cursor sits at this line's first row here: after the previous
            // line's CR/LF (which also cleared any deferred wrap), or at the
            // fresh (0, 0) for the first line. Printing the first glyph may
            // scroll the screen, and `to_absolute_row` absorbs that, so the
            // capture is taken before any of this line's printing.
            line_new_abs.push(self.to_absolute_row(self.cursor.row));
            for (j, cell) in line.iter().enumerate() {
                self.style = cell.style;
                self.active_link = cell.style.link;
                self.print_glyph(cell.ch);
                for tail_ch in cell.tail.iter().flat_map(|t| t.chars()) {
                    self.print_glyph(tail_ch);
                }
                if logical.cursor == Some((i, j)) {
                    // Capture *after* printing: a print defers a wrap from
                    // the previous character (parking the cursor on the old
                    // row until the next glyph arrives), so capturing before
                    // would land a cursor whose target begins a new wrapped
                    // row back on the old one. When this character itself
                    // deferred a wrap, the cursor is parked right where it
                    // was written; otherwise back off the one-past-the-glyph
                    // advance the print just applied.
                    let col = if self.cursor.wrap_pending {
                        self.cursor.col
                    } else {
                        self.cursor.col.saturating_sub(1)
                    };
                    new_cursor = Some((self.cursor.row, col));
                }
            }
            if let Some((_, offset)) = logical
                .cursor
                .filter(|&(ci, cj)| ci == i && cj >= line.len())
            {
                // Past the line's trimmed end (a prompt's trailing space):
                // keep the cursor as far beyond the content as it stood, so
                // the shell's next character still lands after that space.
                let end = match self.cursor.wrap_pending {
                    true => self.cols,
                    false => self.cursor.col,
                };
                new_cursor = Some((self.cursor.row, end + (offset - line.len())));
            }
            if i + 1 < logical.lines.len() {
                self.carriage_return();
                self.line_feed();
            }
        }
        let (cr, cc) = new_cursor.unwrap_or((self.cursor.row, self.cursor.col));
        self.end_replay(guard);
        self.cursor.row = cr.min(self.rows.saturating_sub(1));
        self.cursor.col = cc.min(self.cols.saturating_sub(1));
        self.cursor.wrap_pending = false;
        line_new_abs
    }
    /// Set aside the live state a replay must not be steered by (see
    /// [`ReplayGuard`]) and put the replay's own in force.
    fn begin_replay(&mut self) -> ReplayGuard {
        let guard = ReplayGuard {
            active_link: self.active_link,
            auto_wrap: self.modes.auto_wrap,
            charsets: self.charsets,
            insert: self.modes.insert,
            last_graphic: self.last_graphic,
            style: self.style,
        };
        self.modes.auto_wrap = true;
        self.modes.insert = false;
        guard
    }
    /// Put back what [`Self::begin_replay`] set aside.
    fn end_replay(&mut self, guard: ReplayGuard) {
        self.active_link = guard.active_link;
        self.modes.auto_wrap = guard.auto_wrap;
        self.charsets = guard.charsets;
        self.modes.insert = guard.insert;
        self.last_graphic = guard.last_graphic;
        self.style = guard.style;
    }
    /// How the pre-resize live rows map onto the replayed screen: a row that
    /// began a logical line maps exactly onto that line's new first row (the
    /// anchor case: a block's reserved band is made of blank hard lines), and
    /// a row inside a wrapped line is clamped inside that line's new extent
    /// (selection endpoints). Rows whose lines were dropped as trailing blanks
    /// collapse onto the last surviving line.
    pub(super) fn live_row_remap(
        &self,
        logical: &LogicalLines,
        line_new_abs: &[usize],
        old_live_top: usize,
        old_rows: usize,
    ) -> Option<RowRemap> {
        let &last_new_abs = line_new_abs.last()?;
        let final_abs = self.to_absolute_row(self.cursor.row).max(last_new_abs);
        let starts = &logical.line_starts;
        let mut new_abs: Vec<usize> = Vec::with_capacity(old_rows);
        let mut containing = 0usize;
        for r in 0..old_rows {
            while containing + 1 < starts.len() && starts[containing + 1] <= r {
                containing += 1;
            }
            let new_start = line_new_abs[containing];
            let new_height = match line_new_abs.get(containing + 1) {
                Some(&next) => next.saturating_sub(new_start),
                None => final_abs.saturating_sub(new_start),
            };
            let offset = (r - starts[containing]).min(new_height.saturating_sub(1));
            new_abs.push(new_start + offset);
        }
        Some(RowRemap {
            new_abs,
            old_top: old_live_top,
        })
    }
}

/// Whether `cell` is the blank padding a line's last row is trimmed of: a
/// plain space in the default pen.
pub(super) fn is_trimmable_blank(cell: &Cell) -> bool {
    cell.ch == ' ' && cell.style == Style::default() && cell.width == CellWidth::Single
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{AnsiMode, Charset, CharsetSlot, DecMode, EraseMode};
    use super::*;
    use crate::grid::test_support::*;

    #[test]
    fn test_noop_resize_preserves_cursor_at_trailing_space() {
        // Regression: a resize to the SAME dimensions must be a no-op. Previously
        // the grid reflow always ran, trimming the prompt's trailing space and
        // re-deriving the cursor approximately, so the cursor at "> |" (col 2)
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
        let mut grid = Grid::new(10, 3)
            .folding_on_print()
            .with_wrap_indent(true)
            .with_word_wrap(false);
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
        let mut grid = Grid::new(10, 3)
            .folding_on_print()
            .with_wrap_indent(false)
            .with_word_wrap(false);
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
    fn test_wrap_indent_is_a_primary_screen_behavior_only() {
        // A full-screen app addresses the grid by exact coordinates and lays
        // out its own continuations, so the alternate screen must wrap at
        // column zero even with hanging indent on.
        let mut grid = Grid::new(10, 3)
            .folding_on_print()
            .with_wrap_indent(true)
            .with_word_wrap(false);
        grid.enter_alt_screen();
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
        let mut grid = Grid::new(10, 4)
            .folding_on_print()
            .with_wrap_indent(true)
            .with_word_wrap(false);
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
        let mut grid = Grid::new(10, 2)
            .folding_on_print()
            .with_wrap_indent(true)
            .with_word_wrap(false);
        for ch in "  01234567XYZ".chars() {
            grid.print(ch);
        }
        // Scroll up into history
        grid.line_feed();
        assert_eq!(grid.scrollback_len(), 1);
        assert!(grid.absolute_row_wraps(0));
        assert_eq!(grid.absolute_row_wrap_indent(1), 2);
    }

    #[test]
    fn test_resize_replays_a_word_wrapped_line_at_word_boundaries() {
        // The reflow reconstructs the logical line, including the space each
        // break consumed, so re-wrapping at a new width breaks at words
        // again instead of gluing the carried words onto their neighbors.
        let mut grid = Grid::new(10, 4)
            .folding_on_print()
            .with_word_wrap(true)
            .with_wrap_indent(true);
        for ch in "aaa bbb ccc ddd".chars() {
            grid.print(ch);
        }
        assert_eq!(row_text(&grid, 0), "aaa bbb");
        assert_eq!(row_text(&grid, 1), "ccc ddd");

        grid.resize(14, 4);
        assert_eq!(row_text(&grid, 0), "aaa bbb ccc");
        assert_eq!(row_text(&grid, 1), "ddd");
        assert_eq!(
            grid.absolute_row_break(grid.to_absolute_row(0)),
            Some(11),
            "the re-wrapped break is recorded where it now falls"
        );
    }

    #[test]
    fn test_resize_row_remap_tracks_a_band_anchor_through_a_rewrap() {
        // Regression (the "blocks detach on resize" bug): a long line above a
        // block's band re-wraps from 2 rows to 3 when the grid narrows, moving
        // every row below it down one, but the band's anchor kept its old
        // row and the block drew over the line above it. The remap must land
        // the anchor on the band's own first blank line after the reflow.
        let mut grid = Grid::new(20, 8);
        // A 40-char line of distinct characters: rows 0-1 at 20 cols.
        let long = "abcdefghijklmnopqrstuvwxyz0123456789ABCD";
        assert_eq!(long.chars().count(), 40);
        for ch in long.chars() {
            grid.print(ch);
        }
        grid.carriage_return();
        grid.line_feed();
        // The band's three blank rows (2, 3, 4), then a line below.
        for _ in 0..3 {
            grid.line_feed();
        }
        for ch in "tail".chars() {
            grid.print(ch);
        }
        let anchor = grid.to_absolute_row(2);

        grid.resize(10, 8); // the 40 chars now wrap rows 0-3

        assert_eq!(
            row_text(&grid, 3),
            "456789ABCD",
            "fixture: the line re-wrapped to 4 rows"
        );
        let remap = grid
            .take_row_remap()
            .expect("a reflowing resize produces a remap");
        assert_eq!(
            remap.map(anchor),
            grid.to_absolute_row(4),
            "the anchor must follow the band below the re-wrapped line"
        );
    }

    #[test]
    fn test_resize_row_remap_maps_scrollback_rows_to_themselves() {
        // The scrollback is never reflowed, so an anchor into it (a band that
        // scrolled into history) must survive the resize unchanged.
        let mut grid = Grid::new(4, 3);
        for ch in "abcdefgh".chars() {
            grid.print(ch);
        }
        grid.line_feed(); // rows pushed into history
        let anchor = grid.to_absolute_row(0);
        grid.resize(8, 3);
        let remap = grid.take_row_remap().expect("remap");
        assert_eq!(remap.map(anchor), anchor);
    }

    #[test]
    fn test_same_size_resize_produces_no_remap() {
        let mut grid = Grid::new(6, 3);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.resize(6, 3);
        assert!(grid.take_row_remap().is_none());
    }

    #[test]
    fn test_resize_row_remap_describes_the_primary_screen_while_alt_is_active() {
        // While a full-screen app owns the alt screen, block anchors name the
        // stored primary screen, so the published remap must reflow THAT
        // buffer, not the app's live rows, which is what the live replay
        // describes. Pushing a primary anchor through the alt replay would
        // re-point it at the app's content.
        let mut grid = Grid::new(20, 8);
        let long = "abcdefghijklmnopqrstuvwxyz0123"; // 30 chars: rows 0-1
        for ch in long.chars() {
            grid.print(ch);
        }
        grid.carriage_return();
        grid.line_feed(); // the band's first row: 2
        for _ in 0..3 {
            grid.line_feed(); // blank band rows 2-4
        }
        for ch in "tail".chars() {
            grid.print(ch); // row 5
        }
        let anchor = grid.to_absolute_row(2);

        grid.enter_alt_screen();
        grid.resize(10, 8);
        let remap = grid
            .take_row_remap()
            .expect("a remap for the primary screen");
        grid.leave_alt_screen();

        // The primary's 30 chars re-wrap under the all-hard-lines assumption
        // (10 + 20 chars): rows 0-2, so the band starts at row 3.
        assert_eq!(
            remap.map(anchor),
            grid.to_absolute_row(3),
            "the anchor must land on the primary screen's re-flowed band row"
        );
    }

    #[test]
    fn test_a_resize_to_an_impossible_size_is_held_to_the_grid_cap() {
        // What a hostile mux server can report: four billion cells, unbounded.
        let mut grid = Grid::new(80, 24);
        grid.resize(65535, 65535);
        assert_eq!((grid.cols(), grid.rows()), (MAX_GRID_COLS, MAX_GRID_ROWS));
        let fresh = Grid::new(usize::MAX, 3);
        assert_eq!((fresh.cols(), fresh.rows()), (MAX_GRID_COLS, 3));
    }

    #[test]
    fn test_a_resize_keeps_the_cursor_after_a_prompts_trailing_space() {
        // The space after `$` is trimmed as padding when the line is
        // collected; the cursor stood past it and must still, or the shell's
        // next character lands on top of the space.
        let mut grid = Grid::new(20, 5);
        grid.print('$');
        grid.print(' ');
        assert_eq!(grid.cursor(), (0, 2));
        grid.resize(30, 5);
        assert_eq!(grid.cursor(), (0, 2));
        grid.resize(12, 5);
        assert_eq!(grid.cursor(), (0, 2));
    }

    #[test]
    fn test_a_resize_replays_cells_as_printed_whatever_the_live_charset() {
        // The cells already hold what the character set made of them: a
        // replay through it again turned `lqx` into box drawing.
        let mut grid = Grid::new(20, 3);
        for ch in "lqx".chars() {
            grid.print(ch);
        }
        grid.designate_charset(CharsetSlot::G0, Charset::DecSpecialGraphics);
        grid.resize(10, 3);
        assert_eq!(row_text(&grid, 0), "lqx");
        grid.print('q');
        assert_eq!(
            row_text(&grid, 0),
            "lqx\u{2500}",
            "the live set still applies"
        );
    }

    #[test]
    fn test_a_resize_rewraps_with_autowrap_off_and_insert_mode_on() {
        // A narrowing replay must wrap at the new margin rather than pile the
        // line's tail into its last column, and must not shift what it has
        // already replayed; the program's own modes come back afterwards.
        let mut grid = Grid::new(10, 3);
        for ch in "abcdefgh".chars() {
            grid.print(ch);
        }
        grid.set_dec_mode(DecMode::AutoWrap, false);
        grid.set_ansi_mode(AnsiMode::Insert, true);
        grid.resize(4, 3);
        assert_eq!(row_text(&grid, 0), "abcd");
        assert_eq!(row_text(&grid, 1), "efgh");
        assert!(!grid.dec_mode(DecMode::AutoWrap));
        assert!(grid.ansi_mode(AnsiMode::Insert));
    }

    #[test]
    fn test_the_primary_screens_soft_wraps_survive_a_full_screen_app() {
        // The primary screen's wrap facts are set aside with its cells, so a
        // wrapped line still reads as one logical line once the app exits.
        let mut grid = Grid::new(4, 3);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        assert!(grid.row_wraps(0), "fixture: row 0 wraps");
        grid.enter_alt_screen();
        assert!(!grid.row_wraps(0), "the alt screen starts unwrapped");
        grid.leave_alt_screen();
        assert!(grid.row_wraps(0));
        assert_eq!(grid.wrapped_row_span(1), (0, 1));
    }
}

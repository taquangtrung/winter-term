//! The cell grid: styled cells, a cursor, and the intrinsic screen operations
//! that VT sequences map onto. No `vte` here; [`crate::screen`] does the parsing.

mod cell;
mod cursor;
mod erase;
mod link;
mod reflow;
mod screen;
mod scroll;
mod text;

pub use cell::{Cell, CellWidth, Color, CursorShape, EraseMode, RgbColor, Style};
pub use scroll::MAX_SCROLLBACK;

use screen::AltBuffer;
// The DEC private-mode flags are read by every module that touches grid state.

use std::time::Instant;

// ========================================================================
// Data Structures
// ========================================================================

/// A fixed-size grid of styled cells with a cursor, a current pen style, and a
/// scrollback history ring. Scrolling up reveals previously scrolled-off rows.
#[derive(Clone, Debug)]
pub struct Grid {
    /// Intern ID of the currently active OSC 8 hyperlink; 0 = none.
    active_link: u16,
    alt_buffer: Option<Box<AltBuffer>>,
    bracketed_paste: bool,
    cells: Vec<Cell>,
    cols: usize,
    cursor: Cursor,
    /// Whether the active program has explicitly set a cursor shape via DECSCUSR.
    /// Until it has, the host's configured per-mode shape applies; once set, the
    /// program's reported shape drives rendering (e.g. vim's block/bar by mode).
    /// Reset on alt-screen transitions so a full-screen app's shape never leaks.
    cursor_shape_set: bool,
    /// Cursor visibility (DECTCEM): cleared by CSI ?25l, set by CSI ?25h.
    /// Full-screen apps like btop hide the cursor while they draw.
    cursor_visible: bool,
    focus_event: bool,
    /// Intern table for OSC 8 URLs; index 0 is always the empty string (no link).
    link_table: Vec<String>,
    max_scrollback: usize,
    mouse_button: bool,
    mouse_drag: bool,
    mouse_sgr: bool,
    /// Debounce timestamp for [`Grid::detect_urls`]; `None` means it has never
    /// run yet (so the next call always scans).
    next_url_scan: Option<Instant>,
    /// DECOM origin mode: when set, absolute cursor positioning (CUP/HVP/VPA) is
    /// relative to the scroll region top and confined within it.
    origin_mode: bool,
    /// Number of hanging indent columns on each live row.
    row_wrap_indent: Vec<usize>,
    /// Per-row soft-wrap flags (length == `rows`): `row_wrapped[r]` is true when
    /// row `r` filled the width and auto-wrapped into row `r + 1` (rather than
    /// ending at an explicit newline). Used only by [`Grid::resize`] to rewrap
    /// logical lines, so staleness affects reflow alone, never live scrolling.
    row_wrapped: Vec<bool>,
    rows: usize,
    saved_cursor: Option<Cursor>,
    scroll_bottom: usize,
    scroll_offset: usize,
    scroll_top: usize,
    scrollback: Vec<Vec<Cell>>,
    scrollback_wrap_indent: Vec<usize>,
    scrollback_wrapped: Vec<bool>,
    style: Style,
    /// Absolute row spans (`[start, end)`) the screen has erased since the last
    /// [`Grid::take_erased_spans`]. Rich blocks anchor a band of rows and draw
    /// an image over it; erasing those rows blanks the grid but cannot reach
    /// the image, so the owner has to be told to drop the block. Without this a
    /// `clear` leaves every block on screen, painted over the fresh output.
    erased_spans: Vec<(usize, usize)>,
    /// How many scrollback rows have been evicted (by the retention budget or
    /// by `clear`) over this grid's lifetime. Absolute row addressing counts
    /// from the first line ever written rather than from the oldest *retained*
    /// one, so a long-lived anchor (a selection, a block's band) keeps naming
    /// the same text once the history ring starts dropping its front. See
    /// [`Grid::to_absolute_row`].
    trimmed_rows: usize,
    /// Whether soft-wrapped continuation lines inherit the first non-blank indent.
    wrap_indent: bool,
    /// The row remap produced by the last [`Grid::resize`], until the owner
    /// drains it with [`Grid::take_row_remap`]. `None` when no resize has
    /// reflowed the live screen since the last drain (or the last resize was
    /// the same-size no-op, which moves nothing).
    pending_remap: Option<RowRemap>,
}

/// How the rows of a pre-resize live screen map onto the post-resize one,
/// produced by a reflowing [`Grid::resize`] and drained via
/// [`Grid::take_row_remap`].
///
/// The reflow rebuilds the live screen by replaying logical lines, so a row
/// that re-wraps differently moves everything below it; an anchor that still
/// points at the old row then names content that has scrolled away from it
/// (a block's band overlapping the line above it). [`RowRemap::map`] pushes
/// an old absolute row through the replay: a row that begins a logical line
/// maps exactly onto that line's new first row, and a row inside a wrapped
/// line is clamped inside the line's new extent. Rows of the scrollback —
/// never reflowed — map to themselves.
#[derive(Clone, Debug)]
pub struct RowRemap {
    /// Absolute row of the first live row before the resize.
    old_live_top: usize,
    /// For each pre-resize live row (top to bottom), its post-resize
    /// absolute row.
    new_abs: Vec<usize>,
}

impl RowRemap {
    /// The post-resize absolute row naming the content that sat at
    /// pre-resize absolute row `old_abs`. Scrollback rows (never reflowed)
    /// and rows past the live screen map to themselves / the last known row.
    pub fn map(&self, old_abs: usize) -> usize {
        match old_abs.checked_sub(self.old_live_top) {
            Some(row) => self.new_abs[row.min(self.new_abs.len().saturating_sub(1))],
            None => old_abs,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Cursor {
    col: usize,
    row: usize,
    shape: CursorShape,
    /// Deferred-wrap ("last column") flag, mirroring the VT100 Last Column Flag.
    /// Set when a print fills the final column: the cursor parks at `cols - 1`
    /// and the wrap (newline + return to column 0) is deferred until the *next*
    /// printable character. This is what lets progress bars and spinners that
    /// fill the terminal width redraw in place instead of scrolling prematurely,
    /// and keeps the reported cursor column within bounds.
    wrap_pending: bool,
}

// ========================================================================
// CursorShape
// ========================================================================

// ========================================================================
// Constants
// ========================================================================

// ========================================================================
// Grid
// ========================================================================

impl Grid {
    /// A blank grid of `cols` x `rows` cells with the cursor at the origin.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            active_link: 0,
            alt_buffer: None,
            bracketed_paste: false,
            cells: vec![Cell::default(); cols * rows],
            cols,
            cursor: Cursor::default(),
            cursor_shape_set: false,
            cursor_visible: true,
            erased_spans: Vec::new(),
            focus_event: false,
            // Index 0 is the sentinel "no link" entry so id 0 always means none.
            link_table: vec![String::new()],
            max_scrollback: MAX_SCROLLBACK,
            mouse_button: false,
            mouse_drag: false,
            mouse_sgr: false,
            next_url_scan: None,
            origin_mode: false,
            row_wrap_indent: vec![0; rows],
            row_wrapped: vec![false; rows],
            rows,
            saved_cursor: None,
            scroll_bottom: rows.saturating_sub(1),
            scroll_offset: 0,
            scroll_top: 0,
            scrollback: Vec::new(),
            scrollback_wrap_indent: Vec::new(),
            scrollback_wrapped: Vec::new(),
            style: Style::default(),
            trimmed_rows: 0,
            wrap_indent: true,
            pending_remap: None,
        }
    }

    /// Set the maximum number of scrollback rows retained. Must be called before
    /// any output is produced; existing scrollback is not retroactively trimmed.
    pub fn with_max_scrollback(mut self, max: usize) -> Self {
        self.max_scrollback = max.max(1);
        self
    }

    /// Grid width in cells.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// Grid height in cells, excluding scrollback.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// The cursor's (row, col).
    pub fn cursor(&self) -> (usize, usize) {
        (self.cursor.row, self.cursor.col)
    }

    /// Note that live rows `first..last` were just blanked, in absolute terms,
    /// for [`Self::take_erased_spans`]. Ignored on the alternate screen, where
    /// no block bands are reserved and full-screen apps erase constantly.
    pub(super) fn record_erased_rows(&mut self, (first, last): (usize, usize)) {
        if first >= last || self.alt_buffer.is_some() {
            return;
        }
        let top = self.absolute_live_top();
        self.erased_spans.push((top + first, top + last));
    }

    /// Absolute row spans erased since the last call, and clear the record.
    ///
    /// The owner of any anchor into the live grid (a rich block's reserved
    /// band) must drop what falls inside one: the rows are blank now, and
    /// anything still drawn over them is painted on top of newer output.
    pub fn take_erased_spans(&mut self) -> Vec<(usize, usize)> {
        std::mem::take(&mut self.erased_spans)
    }

    /// The row remap from the last reflowing [`Grid::resize`], if any: how
    /// absolute rows of the pre-resize live screen map to the post-resize
    /// one. Every anchor kept against the live screen (a rich block's band,
    /// a selection endpoint) must be pushed through it, or it keeps naming
    /// the old row while the re-wrapped content moved off it.
    pub fn take_row_remap(&mut self) -> Option<RowRemap> {
        self.pending_remap.take()
    }

    /// The absolute line index (see [`Self::to_absolute_row`]) of the live
    /// grid's first row, i.e. of everything the history does not hold.
    ///
    /// Not the same as `to_absolute_row(0)`: that names the top *visible* row,
    /// which is a history line whenever the view is scrolled back.
    pub fn absolute_live_top(&self) -> usize {
        self.trimmed_rows + self.scrollback.len()
    }

    /// The cursor row's absolute line index (see [`Self::to_absolute_row`]).
    pub fn absolute_cursor_row(&self) -> usize {
        self.absolute_live_top() + self.cursor.row
    }

    /// Whether the cursor is parked at the last column with a line wrap
    /// deferred (see [`Grid::print`]), so [`Grid::cursor`] reports the glyph
    /// just printed rather than an empty cell after it.
    pub fn wrap_pending(&self) -> bool {
        self.cursor.wrap_pending
    }

    /// The cell at (row, col), or `None` if out of bounds.
    pub fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        self.cells.get(self.index(row, col)?)
    }

    /// The grid as text, one line per row (trailing blanks trimmed). For tests
    /// and debugging; the GPU renderer reads cells directly.
    pub fn to_text(&self) -> String {
        let mut lines = Vec::with_capacity(self.rows);
        for row in 0..self.rows {
            let mut line = String::with_capacity(self.cols);
            for col in 0..self.cols {
                let cell = &self.cells[row * self.cols + col];
                line.push(cell.ch);
                if let Some(tail) = &cell.tail {
                    line.push_str(tail);
                }
            }
            lines.push(line.trim_end().to_string());
        }
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }

    /// The current pen style, applied to printed cells.
    pub fn style(&self) -> Style {
        self.style
    }

    /// Set the style applied to subsequently printed cells (SGR state).
    pub fn set_style(&mut self, style: Style) {
        self.style = style;
    }

    /// Resolve a requested absolute row to a grid row, honoring DECOM origin
    /// mode: when set, the row is relative to the scroll region top and confined
    /// within `[scroll_top, scroll_bottom]`; otherwise it is clamped to the grid.
    fn resolve_row(&self, row: usize) -> usize {
        if self.origin_mode {
            (self.scroll_top + row).min(self.scroll_bottom)
        } else {
            row.min(self.rows.saturating_sub(1))
        }
    }

    /// The effective cell at (row, col), accounting for scroll offset.
    /// When scrolled back, row 0 is the oldest visible scrollback row.
    pub fn visible_cell(&self, row: usize, col: usize) -> Option<&Cell> {
        if col >= self.cols || row >= self.rows {
            return None;
        }
        if self.scroll_offset > 0 {
            let scrolled_rows = self.scrollback.len() - self.scroll_offset;
            if row < self.scroll_offset.min(self.rows) {
                let sb_index = scrolled_rows + row;
                if sb_index < self.scrollback.len() {
                    return self.scrollback[sb_index].get(col);
                }
                return None;
            }
            let live_row = row - self.scroll_offset.min(self.rows);
            return self.cells.get(live_row * self.cols + col);
        }
        self.cells.get(row * self.cols + col)
    }

    /// Convert a currently-visible viewport row (0..[`Self::rows`]) to an
    /// absolute line index counted from the first line ever written (0)
    /// through the live grid's last row. Unlike a viewport row, this stays
    /// stable as [`Self::scroll_offset`] changes and as the retention budget
    /// evicts the front of the history, so a row captured while dragging a
    /// selection (or anchoring a block's band) still names the same line
    /// however far the view has since scrolled; pair with
    /// [`Self::absolute_cell`] to read it back.
    pub fn to_absolute_row(&self, viewport_row: usize) -> usize {
        self.trimmed_rows + self.scrollback.len() - self.scroll_offset + viewport_row
    }

    /// Convert an absolute line index (see [`Self::to_absolute_row`]) back to
    /// the viewport row it currently occupies. Signed, and deliberately
    /// unclamped: a line scrolled above the viewport reads negative and one
    /// below reads at or past [`Self::rows`], which is what lets a caller
    /// drawing a multi-row band (a rich block's image) clip the part that
    /// straddles an edge instead of dropping the whole band.
    pub fn to_viewport_row(&self, abs_row: usize) -> isize {
        abs_row as isize - self.to_absolute_row(0) as isize
    }

    /// The cell at absolute line `abs_row` (see [`Self::to_absolute_row`]),
    /// column `col`, independent of the current scroll position. `None` if
    /// out of bounds, which includes a line the retention budget has already
    /// evicted.
    pub fn absolute_cell(&self, abs_row: usize, col: usize) -> Option<&Cell> {
        if col >= self.cols {
            return None;
        }
        let row = self.to_retained_row(abs_row)?;
        if row < self.scrollback.len() {
            return self.scrollback[row].get(col);
        }
        let live_row = row - self.scrollback.len();
        if live_row >= self.rows {
            return None;
        }
        self.cells.get(live_row * self.cols + col)
    }

    /// Index `abs_row` into the *retained* rows (`scrollback` then `cells`),
    /// or `None` when it names a line already evicted from the history.
    pub(super) fn to_retained_row(&self, abs_row: usize) -> Option<usize> {
        abs_row.checked_sub(self.trimmed_rows)
    }

    /// The column of the last non-blank cell in visible `row`, or 0 for a blank
    /// row. Lets Normal-mode navigation stop at a line's real end instead of
    /// running into the trailing blank padding of prompts and outputs.
    pub fn visible_line_end(&self, row: usize) -> usize {
        self.visible_line_content_end(row).unwrap_or(0)
    }

    /// The column of the last non-blank cell in visible `row`, or `None` when
    /// the row holds nothing at all.
    ///
    /// The distinction [`Self::visible_line_end`] flattens away: it answers 0
    /// both for a blank row and for one whose only character sits in column 0.
    /// A caller sizing a selection highlight needs them apart, or every blank
    /// line swept up by a drag (and every row of a rich block's reserved band)
    /// picks up a stray one-cell highlight down the left edge.
    pub fn visible_line_content_end(&self, row: usize) -> Option<usize> {
        let mut end = None;
        for col in 0..self.cols {
            if let Some(cell) = self.visible_cell(row, col) {
                if cell.ch != '\0' && !cell.ch.is_whitespace() {
                    end = Some(col);
                }
            }
        }
        end
    }

    /// The last visible row that holds any printed character, or 0 when the
    /// screen is blank. The vertical analog of [`Grid::visible_line_end`]: lets
    /// Normal-mode navigation stop at the real bottom of content instead of
    /// descending into the blank padding below the prompt.
    pub fn last_content_row(&self) -> usize {
        (0..self.rows)
            .rev()
            .find(|&row| self.row_has_content(row))
            .unwrap_or(0)
    }

    /// Whether visible `row` holds any non-blank cell.
    fn row_has_content(&self, row: usize) -> bool {
        (0..self.cols).any(|col| {
            self.visible_cell(row, col)
                .is_some_and(|cell| cell.ch != '\0' && !cell.ch.is_whitespace())
        })
    }

    fn line_range(&self, mode: EraseMode) -> (usize, usize) {
        match mode {
            EraseMode::ToEnd => (self.cursor.col, self.cols),
            EraseMode::ToStart => (0, self.cursor.col + 1),
            EraseMode::Whole => (0, self.cols),
        }
    }

    fn index(&self, row: usize, col: usize) -> Option<usize> {
        if row < self.rows && col < self.cols {
            Some(row * self.cols + col)
        } else {
            None
        }
    }
}

// ========================================================================
// Emoji clustering helpers
// ========================================================================

// ========================================================================
// URL detection helpers
// ========================================================================

// ========================================================================
// Cell
// ========================================================================

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;

    pub(crate) fn row_text(grid: &Grid, row: usize) -> String {
        (0..grid.cols())
            .filter_map(|c| grid.cell(row, c).map(|cell| cell.ch))
            .collect::<String>()
            .trim_end()
            .to_string()
    }
    /// Fill rows with a repeating marker character, one distinct char per row.
    pub(crate) fn fill_rows(grid: &mut Grid, markers: &[char]) {
        for (row, marker) in markers.iter().enumerate() {
            grid.move_to(row, 0);
            for _ in 0..grid.cols() {
                grid.print(*marker);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_last_content_row_zero_when_blank() {
        let grid = Grid::new(10, 4);
        assert_eq!(grid.last_content_row(), 0);
    }

    /// The pen reported by `style` is the one printed cells receive.
    #[test]
    fn test_style_reports_the_pen_installed_by_set_style() {
        let mut grid = Grid::new(4, 2);
        let pen = Style {
            bold: true,
            foreground: Color::Indexed(3),
            ..Style::default()
        };
        grid.set_style(pen);
        assert_eq!(grid.style(), pen);
        grid.print('x');
        assert_eq!(grid.cell(0, 0).unwrap().style, pen);
    }

    /// The builder's cap actually bounds retained history; without it a long
    /// session grows the scrollback without limit.
    #[test]
    fn test_with_max_scrollback_caps_retained_history() {
        let mut grid = Grid::new(4, 2).with_max_scrollback(3);
        for _ in 0..40 {
            grid.line_feed();
        }
        assert!(
            grid.scrollback_len() <= 3,
            "history grew past the cap: {}",
            grid.scrollback_len()
        );
    }
}

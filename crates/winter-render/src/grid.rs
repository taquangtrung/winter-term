//! The cell grid: styled cells, a cursor, and the intrinsic screen operations
//! that VT sequences map onto. No `vte` here; [`crate::screen`] does the parsing.

use std::time::{Duration, Instant};

use unicode_width::UnicodeWidthChar;

/// Minimum time between full-grid [`Grid::detect_urls`] scans. URL highlighting
/// is a passive affordance, not something that needs sub-frame freshness, so a
/// small bounded delay is imperceptible — but skipping the O(rows*cols) rescan
/// on every call matters a lot for a shell whose line-editor retypes the whole
/// input line on every keystroke (e.g. cmd.exe under ConPTY), which would
/// otherwise trigger that scan on nearly every keystroke.
const URL_SCAN_INTERVAL: Duration = Duration::from_millis(100);

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
    /// Whether soft-wrapped continuation lines inherit the first non-blank indent.
    wrap_indent: bool,
}

/// One screen cell: a character, its styling, and its display width role.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// The cell's base character. A blank cell holds `' '`.
    pub ch: char,
    /// Trailing codepoints that combine onto `ch` but can't be composed into a
    /// single `char`: emoji ZWJ sequences, variation selectors, skin-tone
    /// modifiers, and a paired second regional-indicator flag half. `None` in
    /// the common case.
    pub tail: Option<Box<str>>,
    /// Colors and attributes applied when this cell is drawn.
    pub style: Style,
    /// This cell's role in double-width layout.
    pub width: CellWidth,
}

/// A cell's role in double-width (CJK/emoji) layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CellWidth {
    /// A normal single-column cell.
    #[default]
    Single,
    /// The left half of a double-width character; `ch` holds the character.
    Wide,
    /// The right half of a double-width character: a placeholder with no glyph,
    /// skipped at render time so the wide glyph spans both columns.
    Spacer,
}

/// A cell's visual attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    /// Cell background color.
    pub background: Color,
    /// SGR 1.
    pub bold: bool,
    /// Glyph color.
    pub foreground: Color,
    /// SGR 3.
    pub italic: bool,
    /// Intern ID into the grid's link table; 0 means no hyperlink.
    pub link: u16,
    /// SGR 7 (reverse video): foreground and background are swapped at render time.
    pub reversed: bool,
    /// SGR 4.
    pub underline: bool,
}

/// A foreground or background color.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Color {
    /// The terminal's default fg/bg.
    #[default]
    Default,
    /// A 256-color palette index.
    Indexed(u8),
    /// A 24-bit true color.
    Rgb(RgbColor),
}

/// A 24-bit color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbColor {
    /// Red channel, 0-255.
    pub r: u8,
    /// Green channel, 0-255.
    pub g: u8,
    /// Blue channel, 0-255.
    pub b: u8,
}

/// Which region an erase affects, relative to the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EraseMode {
    /// From the start of the region up to and including the cursor.
    ToStart,
    /// From the cursor to the end of the region.
    ToEnd,
    /// The whole region.
    Whole,
}

/// How the cursor is drawn. Configurable per interaction mode.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CursorShape {
    /// A filled cell-sized rectangle.
    #[default]
    Block,
    /// A line along the cell's bottom edge.
    Underline,
    /// A vertical line at the cell's left edge.
    Bar,
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

impl CursorShape {
    /// Interpret a `cursor` config value (`"block"`/`"bar"`/`"underline"`).
    /// Common synonyms (`"beam"`, `"underscore"`) are accepted; unknown values
    /// fall back to `Block` so a typo never produces a missing cursor.
    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "bar" | "beam" | "line" => Self::Bar,
            "underline" | "underscore" => Self::Underline,
            _ => Self::Block,
        }
    }

    /// The canonical config value for this shape (round-trips through
    /// [`Self::from_value`]).
    pub fn as_value(&self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bar => "bar",
            Self::Underline => "underline",
        }
    }
}

#[derive(Clone, Debug)]
struct AltBuffer {
    cells: Vec<Cell>,
    cursor: Cursor,
    saved_cursor: Option<Cursor>,
    style: Style,
}

// ========================================================================
// Constants
// ========================================================================

const TAB_WIDTH: usize = 8;
/// Default cap on retained scrollback rows per grid. Overridable per pane
/// via [`Grid::with_max_scrollback`] and the `scrollback-lines` setting.
pub const MAX_SCROLLBACK: usize = 10_000;

const MODE_ALT_SCREEN: u16 = 1049;
/// Legacy alternate-screen switch (no cursor save). xterm's older `?47`.
const MODE_ALT_SCREEN_47: u16 = 47;
/// Legacy alternate-screen switch that clears on leave (xterm's `?1047`).
const MODE_ALT_SCREEN_1047: u16 = 1047;
const MODE_BRACKETED_PASTE: u16 = 2004;
const MODE_CURSOR: u16 = 25;
const MODE_MOUSE_BUTTON: u16 = 1000;
const MODE_MOUSE_DRAG: u16 = 1002;
const MODE_MOUSE_SGR: u16 = 1006;
const MODE_FOCUS_EVENT: u16 = 1004;
const MODE_ORIGIN: u16 = 6;
/// Save/restore cursor as a DEC private mode (xterm's `?1048`).
const MODE_SAVE_CURSOR: u16 = 1048;

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
            wrap_indent: true,
        }
    }

    /// Set the maximum number of scrollback rows retained. Must be called before
    /// any output is produced; existing scrollback is not retroactively trimmed.
    pub fn with_max_scrollback(mut self, max: usize) -> Self {
        self.max_scrollback = max.max(1);
        self
    }

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

    /// Open or close an OSC 8 hyperlink. `None` or an empty string clears the
    /// active link; any other value is interned and stamped into future cells.
    pub fn set_active_link(&mut self, url: Option<&str>) {
        self.active_link = match url {
            None | Some("") => 0,
            Some(u) => self.intern_link(u),
        };
    }

    /// Intern `url` into the link table, returning its ID (>0). Reuses an
    /// existing slot when the same URL has been seen before.
    fn intern_link(&mut self, url: &str) -> u16 {
        if let Some(i) = self.link_table.iter().position(|s| s == url) {
            return i as u16;
        }
        let id = self.link_table.len() as u16;
        self.link_table.push(url.to_string());
        id
    }

    /// Resolve a link ID to its URL. Returns `None` for id 0 (no link).
    pub fn link_url(&self, id: u16) -> Option<&str> {
        if id == 0 {
            return None;
        }
        self.link_table.get(id as usize).map(String::as_str)
    }

    /// The hyperlink URL of the visible cell at (row, col), if any.
    pub fn cell_link(&self, row: usize, col: usize) -> Option<&str> {
        let cell = self.visible_cell(row, col)?;
        self.link_url(cell.style.link)
    }

    /// Return the link ID (non-zero) for the given URL, or 0 if it has never
    /// been interned. Used to resolve a URL string back to its rendering ID so
    /// the renderer can highlight all cells belonging to the hovered link.
    pub fn find_link_id(&self, url: &str) -> u16 {
        self.link_table
            .iter()
            .position(|s| s == url)
            .map(|i| i as u16)
            .unwrap_or(0)
    }

    /// Scan the live cell buffer for plain-text `http://` / `https://` patterns
    /// and stamp matching cells with auto-detected link IDs. Cells that already
    /// carry an OSC 8 link are left untouched. Only the live (non-scrollback)
    /// rows are scanned; scrollback is read-only.
    pub fn detect_urls(&mut self) {
        let now = Instant::now();
        if self.next_url_scan.is_some_and(|next| now < next) {
            return;
        }
        self.next_url_scan = Some(now + URL_SCAN_INTERVAL);

        // Phase 1: collect (cell_start_idx, span_len, url_string) triples by
        // reading self.cells without taking any long-lived borrows.
        let mut spans: Vec<(usize, usize, String)> = Vec::new();

        for row in 0..self.rows {
            let row_start = row * self.cols;
            let mut col = 0;
            while col < self.cols {
                let prefix_len = url_prefix_len(&self.cells, row_start, col, self.cols);
                if prefix_len == 0 {
                    col += 1;
                    continue;
                }
                let start = col;
                let mut end = col + prefix_len;
                while end < self.cols {
                    let ch = self.cells[row_start + end].ch;
                    if is_url_stop(ch) {
                        break;
                    }
                    end += 1;
                }
                if end > start + prefix_len {
                    let url: String = (start..end).map(|c| self.cells[row_start + c].ch).collect();
                    spans.push((row_start + start, end - start, url));
                }
                col = end;
            }
        }

        // Phase 2: intern collected URLs and stamp cells (link_table borrow ends
        // before each cells access).
        for (cell_start, len, url) in spans {
            let link_id = self.intern_link(&url);
            for i in 0..len {
                let idx = cell_start + i;
                if self.cells[idx].style.link == 0 {
                    self.cells[idx].style.link = link_id;
                }
            }
        }
    }

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

    /// Auto-wrap the current row into the next row, applying hanging indent if enabled.
    fn auto_wrap(&mut self) {
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
        self.cursor.col = 0;
        self.line_feed();
        if indent > 0 && self.cursor.row < self.rows {
            if let Some(row_indent) = self.row_wrap_indent.get_mut(self.cursor.row) {
                *row_indent = indent;
            }
            self.cursor.col = indent;
        }
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
    /// the character — most emoji are double-width, and a combining mark/ZWJ/
    /// selector/modifier that follows one must attach to the real glyph, not
    /// its placeholder. `None` at the start of a row (nothing to attach to).
    fn last_glyph_cell_index(&self) -> Option<usize> {
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
    fn combine_into_previous(&mut self, mark: char) {
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
    /// these apply, or there is no eligible preceding cell — so an unpaired
    /// regional indicator or a modifier with no base still gets its own visible
    /// glyph rather than silently vanishing.
    fn try_merge_continuation(&mut self, ch: char) -> bool {
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
    fn copy_cells_within(&mut self, src: std::ops::Range<usize>, dst: usize) {
        let tmp = self.cells[src].to_vec();
        self.cells[dst..dst + tmp.len()].clone_from_slice(&tmp);
    }

    fn blank_cell(&self) -> Cell {
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
    fn clamp_cursor(&mut self) {
        self.cursor.row = self.cursor.row.min(self.rows.saturating_sub(1));
        self.cursor.col = self.cursor.col.min(self.cols.saturating_sub(1));
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

    /// Switch to the alternate screen buffer, saving the current state.
    pub fn enter_alt_screen(&mut self) {
        if self.alt_buffer.is_some() {
            return;
        }
        self.alt_buffer = Some(Box::new(AltBuffer {
            cells: std::mem::take(&mut self.cells),
            cursor: self.cursor,
            saved_cursor: self.saved_cursor,
            style: self.style,
        }));
        self.cells = vec![Cell::default(); self.cols * self.rows];
        self.cursor = Cursor::default();
        self.saved_cursor = None;
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.active_link = 0;
        self.cursor_shape_set = false;
        self.cursor_visible = true;
        self.origin_mode = false;
        self.row_wrapped.fill(false);
        self.row_wrap_indent.fill(0);
    }

    /// Switch back to the primary screen buffer, restoring the saved state.
    pub fn leave_alt_screen(&mut self) {
        let Some(alt) = self.alt_buffer.take() else {
            return;
        };
        self.cells = alt.cells;
        self.cursor = alt.cursor;
        self.saved_cursor = alt.saved_cursor;
        // The primary buffer's cursor was captured when the alt screen was
        // entered, which may have been at a different size. Same hazard as
        // `restore_cursor`.
        self.clamp_cursor();
        self.style = alt.style;
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.active_link = 0;
        self.cursor_shape_set = false;
        self.cursor_visible = true;
        self.origin_mode = false;
        self.row_wrapped.fill(false);
        self.row_wrap_indent.fill(0);
    }

    /// Whether the alternate screen buffer is active, i.e. a fullscreen
    /// application (vim, less, htop) is running.
    pub fn is_alt_screen(&self) -> bool {
        self.alt_buffer.is_some()
    }

    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }

    /// Whether any mouse tracking mode is active (button or drag).
    pub fn mouse_tracking(&self) -> bool {
        self.mouse_button || self.mouse_drag
    }

    /// Whether drag tracking (CSI ?1002h) specifically is active.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.mouse_drag
    }

    /// Whether SGR extended mouse mode (CSI ?1006h) is active.
    pub fn mouse_sgr(&self) -> bool {
        self.mouse_sgr
    }

    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.focus_event
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

    /// Top of the scroll region (0-based row).
    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    /// Bottom of the scroll region (0-based row, inclusive).
    pub fn scroll_bottom(&self) -> usize {
        self.scroll_bottom
    }

    /// Set cursor shape (DECSCUSR).
    pub fn set_cursor_shape(&mut self, shape: CursorShape) {
        self.cursor_shape_set = true;
        self.cursor.shape = shape;
    }

    /// Handle DECSET/DECRST for a single mode number. Called from screen.rs
    /// which parses the CSI ? sequences.
    pub fn set_private_mode(&mut self, mode: u16, set: bool) {
        match mode {
            MODE_ALT_SCREEN => {
                if set {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            MODE_ALT_SCREEN_47 | MODE_ALT_SCREEN_1047 => {
                if set {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            MODE_SAVE_CURSOR => {
                if set {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            MODE_BRACKETED_PASTE => self.bracketed_paste = set,
            MODE_CURSOR => self.cursor_visible = set,
            // DECOM: positioning becomes relative to the scroll region and the
            // mode change homes the cursor to that region's top-left.
            MODE_ORIGIN => {
                self.origin_mode = set;
                self.move_to(0, 0);
            }
            MODE_FOCUS_EVENT => self.focus_event = set,
            MODE_MOUSE_BUTTON => self.mouse_button = set,
            MODE_MOUSE_DRAG => self.mouse_drag = set,
            MODE_MOUSE_SGR => self.mouse_sgr = set,
            _ => {}
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
    /// absolute line index counted from the oldest scrollback line (0)
    /// through the live grid's last row. Unlike a viewport row, this stays
    /// stable as [`Self::scroll_offset`] changes, so a row captured while
    /// dragging a selection still names the same line after the view
    /// scrolls further; pair with [`Self::absolute_cell`] to read it back.
    pub fn to_absolute_row(&self, viewport_row: usize) -> usize {
        self.scrollback.len() - self.scroll_offset + viewport_row
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

    /// The cell at absolute line `abs_row` (see [`Self::to_absolute_row`]),
    /// column `col`, independent of the current scroll position. `None` if
    /// out of bounds.
    pub fn absolute_cell(&self, abs_row: usize, col: usize) -> Option<&Cell> {
        if col >= self.cols {
            return None;
        }
        if abs_row < self.scrollback.len() {
            return self.scrollback[abs_row].get(col);
        }
        let live_row = abs_row - self.scrollback.len();
        if live_row >= self.rows {
            return None;
        }
        self.cells.get(live_row * self.cols + col)
    }

    /// The column of the last non-blank cell in visible `row`, or 0 for a blank
    /// row. Lets Normal-mode navigation stop at a line's real end instead of
    /// running into the trailing blank padding of prompts and outputs.
    pub fn visible_line_end(&self, row: usize) -> usize {
        let mut end = 0;
        for col in 0..self.cols {
            if let Some(cell) = self.visible_cell(row, col) {
                if cell.ch != '\0' && !cell.ch.is_whitespace() {
                    end = col;
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
    fn reflow_buffer(
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

/// `U+200D` ZWJ joins adjacent emoji into one sequence (e.g. the family/
/// profession combos), a variation selector (`U+FE00..=FE0F`; `FE0E`/`FE0F`
/// are the text/emoji presentation pair) picks a glyph's presentation, and
/// `U+20E3` (COMBINING ENCLOSING KEYCAP) turns a preceding digit/`#`/`*` into
/// a keycap emoji (e.g. `1️⃣`). None of these ever compose via
/// [`unicode_normalization::char::compose`], so [`Grid::combine_into_previous`]
/// appends them to the base cell's [`Cell::tail`] instead of dropping them.
fn is_emoji_tail_mark(c: char) -> bool {
    matches!(c, '\u{200D}' | '\u{FE00}'..='\u{FE0F}' | '\u{20E3}')
}

/// Fitzpatrick skin-tone modifiers (e.g. 👍🏽) — these are double-width per
/// `unicode-width`, so they'd otherwise print as their own independent `Wide`
/// glyph next to the base emoji instead of tinting it.
fn is_skin_tone_modifier(c: char) -> bool {
    ('\u{1F3FB}'..='\u{1F3FF}').contains(&c)
}

/// Regional Indicator Symbols — a flag emoji (e.g. 🇺🇸) is two of these in a
/// row. Each is single-width per `unicode-width`, so a pair must be merged
/// explicitly rather than relying on the double-width path.
fn is_regional_indicator(c: char) -> bool {
    ('\u{1F1E6}'..='\u{1F1FF}').contains(&c)
}

/// Append `c` to `cell`'s combining tail, creating it if this is the first.
fn append_tail(cell: &mut Cell, c: char) {
    let mut tail = cell.tail.take().map_or_else(String::new, String::from);
    tail.push(c);
    cell.tail = Some(tail.into_boxed_str());
}

// ========================================================================
// URL detection helpers
// ========================================================================

/// Number of characters in the URL scheme+authority prefix starting at `col`,
/// or 0 if the cell sequence does not begin `https://` or `http://`.
fn url_prefix_len(cells: &[Cell], row_start: usize, col: usize, cols: usize) -> usize {
    let matches = |pat: &[u8]| -> bool {
        pat.iter().enumerate().all(|(i, &b)| {
            cells
                .get(row_start + col + i)
                .is_some_and(|c| c.ch as u8 == b && c.ch.is_ascii())
        })
    };
    if col + 8 <= cols && matches(b"https://") {
        8
    } else if col + 7 <= cols && matches(b"http://") {
        7
    } else {
        0
    }
}

/// Returns true for characters that terminate a URL in plain terminal text.
fn is_url_stop(ch: char) -> bool {
    ch == '\0'
        || ch == ' '
        || ch == '\t'
        || ch == '"'
        || ch == '\''
        || ch == '<'
        || ch == '>'
        || (ch as u32) < 0x20
}

// ========================================================================
// Cell
// ========================================================================

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            tail: None,
            style: Style::default(),
            width: CellWidth::Single,
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
    fn test_move_to_clamps_to_bounds() {
        let mut grid = Grid::new(4, 3);
        grid.move_to(99, 99);
        assert_eq!(grid.cursor(), (2, 3));
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
    fn test_tab_advances_to_next_stop() {
        let mut grid = Grid::new(20, 1);
        grid.print('x');
        grid.tab();
        assert_eq!(grid.cursor(), (0, 8));
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

    fn row_text(grid: &Grid, row: usize) -> String {
        (0..grid.cols())
            .filter_map(|c| grid.cell(row, c).map(|cell| cell.ch))
            .collect::<String>()
            .trim_end()
            .to_string()
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
    fn test_origin_mode_positions_relative_to_scroll_region() {
        let mut grid = Grid::new(10, 10);
        // Region rows 3..=7 (1-based) -> 0-based top 2, bottom 6.
        grid.set_scroll_region(2, 6);
        grid.set_private_mode(MODE_ORIGIN, true);
        // Enabling origin mode homes the cursor to the region's top-left.
        assert_eq!(grid.cursor(), (2, 0));
        // CUP rows are relative to the region top.
        grid.move_to(3, 4);
        assert_eq!(grid.cursor(), (5, 4));
        // Rows past the region bottom clamp to it.
        grid.move_to(100, 0);
        assert_eq!(grid.cursor(), (6, 0));
        // Disabling returns to absolute, screen-relative positioning.
        grid.set_private_mode(MODE_ORIGIN, false);
        assert_eq!(grid.cursor(), (0, 0));
        grid.move_to(3, 4);
        assert_eq!(grid.cursor(), (3, 4));
    }

    #[test]
    fn test_legacy_alt_screen_mode_47() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.set_private_mode(MODE_ALT_SCREEN_47, true);
        assert!(grid.is_alt_screen());
        grid.set_private_mode(MODE_ALT_SCREEN_47, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_save_cursor_mode_1048() {
        let mut grid = Grid::new(5, 5);
        grid.move_to(2, 3);
        grid.set_private_mode(MODE_SAVE_CURSOR, true);
        grid.move_to(0, 0);
        grid.set_private_mode(MODE_SAVE_CURSOR, false);
        assert_eq!(grid.cursor(), (2, 3));
    }

    #[test]
    fn test_alt_screen_switches_and_restores() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.enter_alt_screen();
        assert!(grid.is_alt_screen());
        assert_eq!(grid.to_text(), "");
        for ch in "xyz".chars() {
            grid.print(ch);
        }
        grid.leave_alt_screen();
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_set_private_mode_alt_screen() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.set_private_mode(MODE_ALT_SCREEN, true);
        assert!(grid.is_alt_screen());
        grid.set_private_mode(MODE_ALT_SCREEN, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_bracketed_paste_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.bracketed_paste());
        grid.set_private_mode(MODE_BRACKETED_PASTE, true);
        assert!(grid.bracketed_paste());
        grid.set_private_mode(MODE_BRACKETED_PASTE, false);
        assert!(!grid.bracketed_paste());
    }

    #[test]
    fn test_mouse_modes() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());

        grid.set_private_mode(MODE_MOUSE_BUTTON, true);
        assert!(grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());

        grid.set_private_mode(MODE_MOUSE_DRAG, true);
        assert!(grid.mouse_tracking());
        assert!(grid.mouse_drag_tracking());

        grid.set_private_mode(MODE_MOUSE_SGR, true);
        assert!(grid.mouse_sgr());

        grid.set_private_mode(MODE_MOUSE_BUTTON, false);
        grid.set_private_mode(MODE_MOUSE_DRAG, false);
        grid.set_private_mode(MODE_MOUSE_SGR, false);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());
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
    fn test_last_content_row_zero_when_blank() {
        let grid = Grid::new(10, 4);
        assert_eq!(grid.last_content_row(), 0);
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
    fn test_cursor_visibility_dectcem() {
        let mut grid = Grid::new(3, 2);
        assert!(grid.cursor_visible());
        grid.set_private_mode(MODE_CURSOR, false);
        assert!(!grid.cursor_visible());
        grid.set_private_mode(MODE_CURSOR, true);
        assert!(grid.cursor_visible());
    }

    #[test]
    fn test_reported_cursor_shape_unset_until_decscusr() {
        let mut grid = Grid::new(3, 2);
        assert_eq!(grid.reported_cursor_shape(), None);
        grid.set_cursor_shape(CursorShape::Bar);
        assert_eq!(grid.reported_cursor_shape(), Some(CursorShape::Bar));
    }

    #[test]
    fn test_alt_screen_resets_reported_shape_and_visibility() {
        let mut grid = Grid::new(3, 2);
        grid.set_cursor_shape(CursorShape::Bar);
        grid.set_private_mode(MODE_CURSOR, false);
        // A full-screen app's cursor state must not leak across the alt-screen
        // boundary: leaving restores an unset shape and a visible cursor.
        grid.set_private_mode(MODE_ALT_SCREEN, true);
        assert_eq!(grid.reported_cursor_shape(), None);
        assert!(grid.cursor_visible());
        grid.set_cursor_shape(CursorShape::Underline);
        grid.set_private_mode(MODE_CURSOR, false);
        grid.set_private_mode(MODE_ALT_SCREEN, false);
        assert_eq!(grid.reported_cursor_shape(), None);
        assert!(grid.cursor_visible());
    }

    #[test]
    fn test_focus_event_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.focus_event());
        grid.set_private_mode(MODE_FOCUS_EVENT, true);
        assert!(grid.focus_event());
        grid.set_private_mode(MODE_FOCUS_EVENT, false);
        assert!(!grid.focus_event());
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
    fn test_intern_link_deduplicates_same_url() {
        let mut grid = Grid::new(5, 1);
        let id1 = grid.intern_link("https://a.com");
        let id2 = grid.intern_link("https://a.com");
        assert!(id1 > 0);
        assert_eq!(id1, id2);
    }

    #[test]
    fn test_intern_link_different_urls_get_different_ids() {
        let mut grid = Grid::new(5, 1);
        let id1 = grid.intern_link("https://a.com");
        let id2 = grid.intern_link("https://b.com");
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_link_url_zero_returns_none() {
        let grid = Grid::new(5, 1);
        assert_eq!(grid.link_url(0), None);
    }

    #[test]
    fn test_set_active_link_stamps_cells() {
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://example.com"));
        grid.print('h');
        grid.print('i');
        grid.set_active_link(None);
        grid.print('!');

        let id = grid.cell(0, 0).unwrap().style.link;
        assert!(id > 0);
        assert_eq!(grid.link_url(id), Some("https://example.com"));
        assert_eq!(grid.cell(0, 1).unwrap().style.link, id);
        assert_eq!(grid.cell(0, 2).unwrap().style.link, 0);
    }

    #[test]
    fn test_cell_link_returns_url_for_linked_cell() {
        let mut grid = Grid::new(5, 1);
        grid.set_active_link(Some("https://x.io"));
        grid.print('x');
        assert_eq!(grid.cell_link(0, 0), Some("https://x.io"));
    }

    #[test]
    fn test_cell_link_returns_none_for_unlinked_cell() {
        let mut grid = Grid::new(5, 1);
        grid.print('x');
        assert_eq!(grid.cell_link(0, 0), None);
    }

    #[test]
    fn test_detect_urls_stamps_https_link() {
        let mut grid = Grid::new(40, 1);
        for ch in "visit https://example.com/page here".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        // The 'h' of 'https' starts the link; every char until space gets the ID.
        let link_id = grid.cells[6].style.link;
        assert!(link_id > 0, "https:// cell should have a link ID");
        let url = grid.link_url(link_id).unwrap();
        assert_eq!(url, "https://example.com/page");
        // Cells before and after the URL have no link.
        assert_eq!(grid.cells[0].style.link, 0);
        assert_eq!(grid.cells[30].style.link, 0);
    }

    #[test]
    fn test_detect_urls_http_scheme() {
        let mut grid = Grid::new(30, 1);
        for ch in "http://foo.io end".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        let link_id = grid.cells[0].style.link;
        assert!(link_id > 0);
        assert_eq!(grid.link_url(link_id), Some("http://foo.io"));
        // Space terminates the URL; "end" has no link.
        assert_eq!(grid.cells[14].style.link, 0);
    }

    #[test]
    fn test_detect_urls_does_not_override_osc8_link() {
        let mut grid = Grid::new(40, 1);
        grid.set_active_link(Some("https://osc8.io"));
        for ch in "https://osc8.io".chars() {
            grid.print(ch);
        }
        grid.set_active_link(None);
        // Manually store the osc8 link id before calling detect_urls.
        let osc8_id = grid.cells[0].style.link;
        assert!(osc8_id > 0);
        grid.detect_urls();
        // detect_urls should not replace the existing osc8 link.
        assert_eq!(grid.cells[0].style.link, osc8_id);
    }

    #[test]
    fn test_detect_urls_plain_text_no_urls_unchanged() {
        let mut grid = Grid::new(20, 1);
        for ch in "no links here today".chars() {
            grid.print(ch);
        }
        grid.detect_urls();
        for i in 0..19 {
            assert_eq!(grid.cells[i].style.link, 0);
        }
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

    /// Fill rows with a repeating marker character, one distinct char per row.
    fn fill_rows(grid: &mut Grid, markers: &[char]) {
        for (row, marker) in markers.iter().enumerate() {
            grid.move_to(row, 0);
            for _ in 0..grid.cols() {
                grid.print(*marker);
            }
        }
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

//! The view types a caller fills in to describe one frame's contents.

use std::sync::Arc;

use super::display::CellOverlay;
use crate::grid::{CursorShape, Grid};
use crate::theme::Rgb;

// ========================================================================
// Data Structures
// ========================================================================

/// A viewport rect for one pane, in pixels from the surface top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    /// Height in physical pixels.
    pub height: f32,
    /// Width in physical pixels.
    pub width: f32,
    /// Left edge, in physical pixels from the surface's left.
    pub x: f32,
    /// Top edge, in physical pixels from the surface's top.
    pub y: f32,
}
/// One pane's rendering input: where to draw and what grid to draw.
pub struct PaneView<'a> {
    /// Whether blinking text (SGR 5) is in its shown phase. It blinks on the
    /// cursor's timer, so both go dark together.
    pub blink_visible: bool,
    /// Cells drawn over the grid in place of its own: an input method's
    /// uncommitted text at the cursor.
    pub overlay: Option<&'a CellOverlay>,
    /// Bracket glyphs to recolor by nesting depth, as viewport `(row, col)`
    /// plus the resolved RGB. Driven by the `rainbow-parens` config option;
    /// computed app-side (see `navigation::reading::bracket_marks`) so the
    /// renderer stays free of depth logic.
    pub bracket_colors: &'a [(usize, usize, (u8, u8, u8))],
    /// Shape to draw the cursor with, resolved from the pane's current mode
    /// and the `cursor` config block.
    pub cursor_shape: CursorShape,
    /// Draw the grid cursor in its unfocused form: a hollow outline of its
    /// cell, whatever `cursor_shape` is. Set when the OS window has lost focus, so
    /// a glance at the cursor tells you whether keystrokes actually reach this
    /// pane.
    pub cursor_unfocused: bool,
    /// Whether the cursor should be drawn this frame. Set to `false` on the
    /// "off" phase of cursor blink; the nav cursor ignores this flag.
    pub cursor_visible: bool,
    /// Blend all colors toward the background to visually de-emphasize this pane.
    /// Controlled by the `dim-inactive` config option.
    pub dim: bool,
    /// Whether this pane is the focused (active) pane.
    pub focused: bool,
    /// The cell grid to draw, including its scrollback.
    pub grid: &'a Grid,
    /// Link ID currently under the mouse pointer. Cells sharing this link get
    /// a highlighted underline color. 0 means no link is hovered.
    pub hovered_link: u16,
    /// Quick-select block labels as `(row, col, label)`, or `None` when
    /// quick-select is not active.
    pub labels: Option<&'a [(usize, usize, char)]>,
    /// Easymotion-style `f`/`t` jump labels as `(row, col, label)`, drawn in the
    /// theme's [`Theme::find_label_bg`]/[`Theme::find_label_fg`] so they read
    /// differently from the quick-select block labels above.
    pub find_labels: &'a [(usize, usize, char)],
    /// The Normal-mode traversal cursor, in viewport `(row, col)`, drawn in
    /// [`Self::cursor_shape`]. `None` when the pane is not being navigated.
    pub nav_cursor: Option<(usize, usize)>,
    /// The viewport row span `(top_row, row_count)` of the rich block whose
    /// reserved band the nav cursor sits in: the block *is* the cursor there,
    /// drawn as an accent outline around the whole band instead of a 1-cell
    /// block somewhere in the blank space behind the image. `None` otherwise.
    pub block_band: Option<(usize, usize)>,
    /// The row to band with [`Theme::cursor_line_bg`] (with its soft-wrapped
    /// continuations). Separate from [`Self::nav_cursor`] so an unfocused pane can
    /// keep showing where its cursor is even though the cursor block itself is
    /// only drawn for the focused pane.
    pub cursor_line_row: Option<usize>,
    /// The row lit to show this pane just took the keyboard, and how strongly
    /// it is lit: from 1 as it happens, fading to 0. `None` once it has
    /// faded, and in every pane but the one that took it.
    pub line_flash: Option<(usize, f32)>,
    /// Whether the nav cursor is in its visible blink phase. Tracked separately
    /// from [`Self::cursor_visible`], which the shell can suppress outright
    /// (DECTCEM): the Normal-mode cursor is Winter's own and only blinks.
    pub nav_cursor_visible: bool,
    /// Where in the surface this pane draws.
    pub rect: PaneRect,
    /// Active search query matches as `(row, col)` pairs; empty when no search
    /// is active. Highlighted in the background pass with
    /// [`Theme::search_match_bg`].
    pub search_matches: &'a [(usize, usize)],
    /// Sentence-highlight bands as row-clipped column spans `(row, start, end,
    /// tone)`, alternating `tone` parity per sentence. Driven by the
    /// `sentence-highlight` config option; computed app-side (see
    /// `navigation::reading::sentence_spans`).
    pub sentence_spans: &'a [(usize, usize, usize, u8)],
    /// The cells of the one match `n`/`N` is parked on, a subset of
    /// [`Self::search_matches`], highlighted with [`Theme::search_current_bg`]
    /// instead so the focused match stands out from the rest.
    pub search_current: &'a [(usize, usize)],
    /// The selected region as `(start_row, start_col, end_row, end_col)` in
    /// viewport coordinates, or `None` when nothing is selected.
    pub selection: Option<(usize, usize, usize, usize)>,
    /// When `true`, selection is rectangular (blockwise `C-v`).
    pub selection_block: bool,
    /// How many rows the pane is currently scrolled up from the live bottom.
    pub scroll_offset: usize,
    /// Total rows in the scrollback buffer above the live grid.
    pub scrollback_len: usize,
    /// Underline cells carrying a link (auto-detected URL or OSC 8 hyperlink).
    /// Controlled by the `url-underline` config option.
    pub url_underline: bool,
}
/// An active interactive prompt keyed into the status bar: the label (e.g.
/// `"/"`, `"Search: "`, `"Filter: "`), the typed value, and the caret index.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StatusPrompt {
    /// How many Unicode characters sit before the caret.
    pub caret: usize,
    /// The prompt prefix or question, e.g. `"/"` or `"Filter: "`.
    pub label: String,
    /// What has been typed so far.
    pub value: String,
}

/// A Vim-style bottom status bar: the `label` (e.g. " NORMAL ") is drawn over a
/// segment filled with `accent`, atop a full-width strip in the theme's status
/// colors. Occupies the bottom-most cell row of the surface.
pub struct StatusBar {
    /// Fill color of the mode segment, chosen per mode by the caller.
    pub accent: Rgb,
    /// The notifications glyph, drawn last at the bar's right edge.
    pub bell: Option<StatusBell>,
    /// The part under the pointer, drawn on a hover background.
    pub hovered: Option<StatusHover>,
    /// Where the focused tab's commands run (an icon, then the host's or
    /// container's name), drawn at the bar's left edge; empty for none.
    pub host: String,
    /// The mode label drawn over `accent`, e.g. `" NORMAL "`.
    pub mode: String,
    /// A transient notice (an error or an info confirmation), shown after the
    /// mode label until it expires.
    pub notice: Option<StatusNotice>,
    /// The active interactive prompt being typed into the status bar, if any.
    pub prompt: Option<StatusPrompt>,
    /// The in-progress or last-run `/` search query, shown after the mode
    /// label while a search is active and cleared once it's cancelled.
    pub search: Option<StatusSearch>,
    /// The right side's segments, in order from the left, drawn right-aligned
    /// before the bell. When short of room the first ones are dropped.
    pub segments: Vec<StatusSegment>,
}
/// The live `/`/`?` search state shown in the status bar: the query text as
/// typed, plus the current match position out of the total found (both zero
/// while there are no matches, e.g. an empty query or no hits yet).
#[derive(Clone)]
pub struct StatusSearch {
    /// The query as typed, without its `/` or `?` prefix.
    pub query: String,
    /// 1-based position of the current match, or 0 when there are none.
    pub match_index: usize,
    /// How many matches the query found, 0 when there are none.
    pub match_total: usize,
    /// `true` for a `?`/`#` backward search, shown with a `?` prefix instead
    /// of `/`.
    pub reverse: bool,
}
/// A space a status segment's text draws half a cell wide rather than a whole
/// one, to set its words closer than the grid would.
pub const THIN_SPACE: char = '\u{2009}';

/// A part of the status bar's right side the pointer can rest on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusHover {
    /// The notifications glyph at the right edge.
    Bell,
    /// The segment at this index into [`StatusBar::segments`].
    Segment(usize),
}

/// One piece of the status bar's right side: an optional icon, then its
/// text, e.g. a provider's logo then `42% 14:05`, a tool's `12:5` alone, or a
/// tool's icon with no text.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusSegment {
    /// The SVG drawn in the cells before `text`.
    pub icon: Option<Arc<[u8]>>,
    /// How long the segment holds on when the bar is short of room: the
    /// lowest is dropped first, and among equals the left-most.
    pub priority: u32,
    /// What the segment says in fewer cells, for a bar short of room, e.g. a
    /// quota's `42% 10%` without its reset times; `None` when `text` is as
    /// short as it gets. The bar shortens every segment before it drops any.
    pub short: Option<String>,
    /// What the segment says; empty for an icon alone.
    pub text: String,
}

/// A transient status-bar notice and how it should read: [`NoticeKind::Error`]
/// draws in red to stand out, [`NoticeKind::Info`] in green to confirm an
/// action such as copying to the clipboard. Also drawn as a floating toast
/// overlay (bottom-center) when the status bar is hidden.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusNotice {
    /// Whether this reports a problem or confirms an action.
    pub kind: NoticeKind,
    /// The message shown to the user.
    pub text: String,
}
/// Whether a [`StatusNotice`] reports a problem or confirms an action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    /// A problem, drawn in red.
    Error,
    /// A confirmation, drawn in green.
    Info,
    /// Work still under way, drawn muted in the status bar.
    Progress,
    /// Something worth knowing that is not a failure, drawn in yellow.
    Warning,
}

/// One toast card in the window's top-right corner.
#[derive(Clone, Debug, PartialEq)]
pub struct ToastCard {
    /// Whether the pointer is on the card, which shows its close button.
    pub hovered: bool,
    /// Names the toast across frames, so a click finds the card it was on.
    pub id: u64,
    /// How much it matters, which colors its icon and its edge.
    pub kind: NoticeKind,
    /// Who said it, shown under the text; `None` for the window itself.
    pub source: Option<String>,
    /// What was said, wrapped to the card's width.
    pub text: String,
}

/// What the pointer is on when it is on a toast card.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ToastHit {
    /// Whether it is on the card's close button rather than its body.
    pub close: bool,
    /// The card's [`ToastCard::id`].
    pub id: u64,
}

/// Where a toast card was drawn: its panel and its close button, as
/// `(x, y, width, height)` in surface pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ToastRect {
    pub(crate) close: (f32, f32, f32, f32),
    pub(crate) id: u64,
    pub(crate) panel: (f32, f32, f32, f32),
}

/// The notifications glyph at the status bar's right edge: `text` is the
/// glyph and any unread count, drawn in the color of `kind`, the most
/// pressing unread notification, or muted when there is none.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusBell {
    /// The most pressing unread notification, if any is unread.
    pub kind: Option<NoticeKind>,
    /// The glyph, then the unread count when there is one.
    pub text: String,
}

// ========================================================================
// PaneRect
// ========================================================================

impl PaneRect {
    /// The part of this pane its rows of cells are drawn in: the whole rows
    /// that fit, centred in the pane.
    ///
    /// A pane is rarely a whole number of rows tall. The part of a row left
    /// over is split between the top and the bottom, so the rows sit evenly
    /// inside the pane rather than with a gap on one side that reads as an
    /// empty line. The top share is whole pixels, so text stays on the pixel
    /// grid; the odd pixel goes below.
    pub fn grid_area(self, cell_height: f32) -> PaneRect {
        if cell_height <= 0.0 || self.height < cell_height {
            return self;
        }
        let rows = (self.height / cell_height).floor();
        let leftover = self.height - rows * cell_height;
        PaneRect {
            height: rows * cell_height,
            y: self.y + (leftover / 2.0).floor(),
            ..self
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
    fn test_the_grid_area_splits_the_leftover_between_top_and_bottom() {
        let pane = PaneRect {
            height: 189.0,
            width: 400.0,
            x: 0.0,
            y: 0.0,
        };
        let grid = pane.grid_area(22.0);
        assert_eq!(grid.height, 176.0, "eight whole rows");
        assert_eq!(grid.y, 6.0, "half the 13 left over goes above them");
        assert_eq!(
            pane.y + pane.height - (grid.y + grid.height),
            7.0,
            "the rest, with the odd pixel, below"
        );

        let exact = PaneRect {
            height: 176.0,
            ..pane
        };
        assert_eq!(
            exact.grid_area(22.0),
            exact,
            "a whole number of rows moves nothing"
        );
    }
}

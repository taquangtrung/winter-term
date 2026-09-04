//! The view types a caller fills in to describe one frame's contents.

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
    /// Bracket glyphs to recolor by nesting depth, as viewport `(row, col)`
    /// plus the resolved RGB. Driven by the `rainbow-parens` config option;
    /// computed app-side (see `navigation::reading::bracket_marks`) so the
    /// renderer stays free of depth logic.
    pub bracket_colors: &'a [(usize, usize, (u8, u8, u8))],
    /// Shape to draw the cursor with, resolved from the pane's current mode
    /// and the `cursor` config block.
    pub cursor_shape: CursorShape,
    /// Draw the grid cursor in its unfocused form: a `Block` cursor becomes a
    /// hollow outline, while `Bar` and `Underline` keep their thin filled shape
    /// but fade toward the background. Set when the OS window has lost focus, so
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
    /// The row to band with [`Theme::cursor_line_bg`] (with its soft-wrapped
    /// continuations). Separate from [`Self::nav_cursor`] so an unfocused pane can
    /// keep showing where its cursor is even though the cursor block itself is
    /// only drawn for the focused pane.
    pub cursor_line_row: Option<usize>,
    /// Whether the nav cursor is in its visible blink phase. Tracked separately
    /// from [`Self::cursor_visible`], which the shell can suppress outright
    /// (DECTCEM) — the Normal-mode cursor is Winter's own and only blinks.
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
    /// When `true`, selection is rectangular (blockwise `Ctrl-V`).
    pub selection_block: bool,
    /// How many rows the pane is currently scrolled up from the live bottom.
    pub scroll_offset: usize,
    /// Total rows in the scrollback buffer above the live grid.
    pub scrollback_len: usize,
    /// Underline cells carrying a link (auto-detected URL or OSC 8 hyperlink).
    /// Controlled by the `url-underline` config option.
    pub url_underline: bool,
}
/// A Vim-style bottom status bar: the `label` (e.g. " NORMAL ") is drawn over a
/// segment filled with `accent`, atop a full-width strip in the theme's status
/// colors. Occupies the bottom-most cell row of the surface.
pub struct StatusBar {
    /// Fill color of the mode segment, chosen per mode by the caller.
    pub accent: Rgb,
    /// The mode label drawn over `accent`, e.g. `" NORMAL "`.
    pub mode: String,
    /// The in-progress or last-run `/` search query, shown after the mode
    /// label while a search is active and cleared once it's cancelled.
    pub search: Option<StatusSearch>,
    /// A transient notice (an error or an info confirmation), shown after the
    /// mode label until it expires.
    pub notice: Option<StatusNotice>,
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
/// A transient status-bar notice and how it should read: [`NoticeKind::Error`]
/// draws in red to stand out, [`NoticeKind::Info`] in green to confirm an
/// action such as copying to the clipboard. Also drawn as a floating toast
/// overlay (bottom-center) when the status bar is hidden.
#[derive(Clone)]
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
}

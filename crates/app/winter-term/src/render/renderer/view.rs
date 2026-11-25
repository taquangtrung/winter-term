//! The view types a caller fills in to describe one frame's contents.

use misty::Rect;
use winter_model::view::NoticeKind;

use super::display::CellOverlay;
use winter_emulator::{CursorShape, Grid};
use winter_model::view::ToolbarView;

// ========================================================================
// Data Structures
// ========================================================================

/// One pane's rendering input: where to draw and what grid to draw.
#[derive(Clone)]
pub struct PaneView<'a> {
    /// The pane's whole area, its strip of tabs included, which its borders
    /// (the separators it shares with its neighbors) run along.
    pub area: Rect,
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
    /// A pane's gutter, a strip for each of its lanes, drawn beside
    /// [`Self::rect`] rather than in its grid, so the lanes can stand off the
    /// text and off each other by a fraction of a column.
    pub gutter: Vec<GridStrip<'a>>,
    /// Link ID currently under the mouse pointer. Cells sharing this link get
    /// a highlighted underline color. 0 means no link is hovered.
    pub hovered_link: u16,
    /// Quick-select block labels as `(row, col, label)`, or `None` when
    /// quick-select is not active.
    pub labels: Option<&'a [(usize, usize, char)]>,
    /// Easymotion-style `f`/`t` jump labels as `(row, col, label)`, drawn in the
    /// theme's [`Theme::find_label_bg`](winter_theme::Theme::find_label_bg)/[`Theme::find_label_fg`](winter_theme::Theme::find_label_fg) so they read
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
    /// The row to band with [`Theme::cursor_line_bg`](winter_theme::Theme::cursor_line_bg) (with its soft-wrapped
    /// continuations). Separate from [`Self::nav_cursor`] so an unfocused pane can
    /// keep showing where its cursor is even though the cursor block itself is
    /// only drawn for the focused pane.
    pub cursor_line_row: Option<usize>,
    /// Whether that band covers its screen row alone, not the soft-wrapped
    /// rest of the row: where the cursor goes by screen rows.
    pub cursor_line_alone: bool,
    /// The row to band with [`Theme::jump_target_bg`](winter_theme::Theme::jump_target_bg) during jump preview.
    pub jump_target_row: Option<usize>,
    /// The row lit to show this pane just took the keyboard, and how strongly
    /// it is lit: from 1 as it happens, fading to 0. `None` once it has
    /// faded, and in every pane but the one that took it.
    pub line_flash: Option<(usize, f32)>,
    /// Whether the nav cursor is in its visible blink phase. Tracked separately
    /// from [`Self::cursor_visible`], which the shell can suppress outright
    /// (DECTCEM): the Normal-mode cursor is Winter's own and only blinks.
    pub nav_cursor_visible: bool,
    /// Where in the surface this pane draws.
    pub rect: Rect,
    /// Active search query matches as `(row, col)` pairs; empty when no search
    /// is active. Highlighted in the background pass with
    /// [`Theme::search_match_bg`](winter_theme::Theme::search_match_bg).
    pub search_matches: &'a [(usize, usize)],
    /// Sentence-highlight bands as row-clipped column spans `(row, start, end,
    /// tone)`, alternating `tone` parity per sentence. Driven by the
    /// `sentence-highlight` config option; computed app-side (see
    /// `navigation::reading::sentence_spans`).
    pub sentence_spans: &'a [(usize, usize, usize, u8)],
    /// Runs of cells drawn with a thin border and no fill of their own
    /// `(row, start_col, end_col)`, the end not included: what an editor
    /// lights without hiding the text's own colors (the occurrences of the
    /// word under the cursor).
    pub outlines: &'a [(usize, usize, usize)],
    /// The cells of the one match `n`/`N` is parked on, a subset of
    /// [`Self::search_matches`], highlighted with [`Theme::search_current_bg`](winter_theme::Theme::search_current_bg)
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
    /// Whether this pane's scrollbar is active (undimmed).
    pub scrollbar_active: bool,
    /// Whether the pointer is hovering over or dragging this pane's scrollbar.
    pub scrollbar_hover: bool,
    /// How much of the scrollbar shows, from 0 (none) to 1 (in full), as the
    /// `scrollbar` setting and the pane's scrolling say.
    pub scrollbar_opacity: f32,
    /// The toolbar drawn over the pane's top rows in the window's own font,
    /// for an editor that has one; the rows under it are left blank for it.
    pub toolbar: Option<&'a ToolbarView>,
    /// Underline cells carrying a link (auto-detected URL or OSC 8 hyperlink).
    /// Controlled by the `url-underline` config option.
    pub url_underline: bool,
}
/// Rows of cells an editor draws off its pane's grid, in a strip beside it:
/// its gutter, level with the pane's rows.
#[derive(Clone, Copy)]
pub struct GridStrip<'a> {
    /// The strip's rows.
    pub grid: &'a Grid,
    /// Where in the surface the strip draws.
    pub rect: Rect,
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

/// A part of the status bar the pointer can rest on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusHover {
    /// The notifications glyph at the right edge.
    Bell,
    /// The branch the focused tab's directory is on.
    Branch,
    /// Where the focused tab's commands run, off this machine.
    Host,
    /// The mode indicator on the left side (Normal, Insert, Visual, etc.).
    Mode,
    /// The project the focused tab's directory is in.
    Project,
    /// The running tasks' progress, after the message.
    Progress,
    /// The segment at this index into the right side's segments.
    Segment(usize),
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
/// The notifications bell at the status bar's right edge.
#[derive(Clone, Debug, PartialEq)]
pub struct StatusBell {
    /// The bell's icon, named in the status bar's icon set: dotted while a
    /// notification is unread. Drawn in the bar's text color whatever is
    /// unread.
    pub icon: &'static str,
}

// ========================================================================
// Functions
// ========================================================================

/// The part of pane `rect` its rows of cells are drawn in: the whole rows
/// that fit, centred in the pane.
///
/// A pane is rarely a whole number of rows tall. The part of a row left
/// over is split between the top and the bottom, so the rows sit evenly
/// inside the pane rather than with a gap on one side that reads as an
/// empty line. The top share is whole pixels, so text stays on the pixel
/// grid; the odd pixel goes below.
pub fn grid_area(rect: Rect, cell_height: f32) -> Rect {
    if cell_height <= 0.0 || rect.height < cell_height {
        return rect;
    }
    let rows = (rect.height / cell_height).floor();
    let leftover = rect.height - rows * cell_height;
    Rect {
        height: rows * cell_height,
        y: rect.y + (leftover / 2.0).floor(),
        ..rect
    }
}

/// The part of pane `rect` its content is drawn in: the grid area less the
/// side padding `pad` on each side (see [`crate::render::PANE_H_PAD`]).
/// Cells, icons, and pictures all start at its left edge, so every kind of
/// tab keeps the same margin.
pub fn text_area(rect: Rect, cell_height: f32, pad: f32) -> Rect {
    let grid = grid_area(rect, cell_height);
    Rect {
        width: (grid.width - 2.0 * pad).max(0.0),
        x: grid.x + pad,
        ..grid
    }
}

/// Where cell (`row`, `col`) of pane `rect` is drawn: its left edge, top
/// edge, and bottom edge in surface pixels, in the [`text_area`] for side
/// padding `pad`, as the renderer draws its text. Whatever is placed over a
/// cell from outside the renderer (a popup, a card, the input method's box)
/// is placed from this, so it sits on its cell.
pub fn cell_box(
    rect: Rect,
    (row, col): (usize, usize),
    (cell_width, cell_height): (f32, f32),
    pad: f32,
) -> (f32, f32, f32) {
    let area = text_area(rect, cell_height, pad);
    let top = area.y + row as f32 * cell_height;
    (area.x + col as f32 * cell_width, top, top + cell_height)
}

/// The cell of pane `rect` that surface point (`x`, `y`) falls in, the
/// inverse of [`cell_box`]: a point above or below the rows takes the
/// nearest row, and one left of the text the first column.
pub fn cell_at(
    rect: Rect,
    (x, y): (f32, f32),
    (cell_width, cell_height): (f32, f32),
    pad: f32,
) -> (usize, usize) {
    let grid = text_area(rect, cell_height, pad);
    let last = ((grid.height / cell_height).round() as usize).saturating_sub(1);
    let col = ((x - grid.x) / cell_width).floor().max(0.0) as usize;
    let row = (((y - grid.y) / cell_height).floor().max(0.0) as usize).min(last);
    (row, col)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_cell_s_box_and_the_cell_at_a_point_agree() {
        let pane = Rect {
            height: 189.0,
            width: 300.0,
            x: 10.0,
            y: 40.0,
        };
        let cell = (9.0, 20.0);
        // The padding of a display at twice the scale.
        let pad = crate::render::pane_h_pad(2.0);
        let (left, top, bottom) = cell_box(pane, (2, 3), cell, pad);
        // The rows sit in the grid area, below the pane's top by half the
        // leftover, and the text past its side padding.
        assert_eq!(top, grid_area(pane, 20.0).y + 40.0);
        assert_eq!(left, 10.0 + pad + 27.0);
        assert_eq!(bottom - top, 20.0);
        assert_eq!(cell_at(pane, (left + 1.0, top + 1.0), cell, pad), (2, 3));
        assert_eq!(cell_at(pane, (left + 8.0, bottom - 1.0), cell, pad), (2, 3));
        assert_eq!(
            cell_at(pane, (0.0, 0.0), cell, pad),
            (0, 0),
            "outside clamps to the first cell"
        );
    }

    #[test]
    fn test_the_grid_area_splits_the_leftover_between_top_and_bottom() {
        let pane = Rect {
            height: 189.0,
            width: 400.0,
            x: 0.0,
            y: 0.0,
        };
        let grid = grid_area(pane, 22.0);
        assert_eq!(grid.height, 176.0, "eight whole rows");
        assert_eq!(grid.y, 6.0, "half the 13 left over goes above them");
        assert_eq!(
            pane.y + pane.height - (grid.y + grid.height),
            7.0,
            "the rest, with the odd pixel, below"
        );

        let exact = Rect {
            height: 176.0,
            ..pane
        };
        assert_eq!(
            grid_area(exact, 22.0),
            exact,
            "a whole number of rows moves nothing"
        );
    }
}

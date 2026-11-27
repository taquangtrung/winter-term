//! Background pass: cell, cursor, selection, and scrollbar fills, which
//! misty draws under each pane's glyphs.
//!
//! A pane's quads are built in layers: cell backgrounds first, merged into
//! one quad per run of equally colored cells on a row, then what draws over
//! them (an unfocused cursor's outline, underlines, overlines and
//! strikethroughs), then the scrollbar.

mod cell_color;
mod cursor;
mod decoration;
mod divider;
mod scrollbar;

pub(super) use cursor::{CURSOR_HOLLOW_STROKE_WIDTH, cursor_outline_quads, cursor_quad};
pub(super) use divider::{DIVIDER_THICKNESS, SeparatorStyle, separator_fills, separators};

use std::collections::HashSet;

use super::display::{CellOverlay, ScreenPresentation, shown_cell};
use cell_color::{CellBgOverlays, FLASH_PEAK_ALPHA, cell_bg_color, lerp_to_bg};
use decoration::{DecorationStyle, push_decorations};
use misty::{Rect, Rgba, SolidFill};
use misty::grid::CellHighlight;
use scrollbar::{ScrollbarParams, scrollbar_quads};
use winter_emulator::{Color as GridColor, CursorShape, Grid};
use winter_theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// How thick an outlined run's border is, in pixels.
const OUTLINE_STROKE_WIDTH: f32 = 1.0;

/// How far an outlined run's border is drawn from the selection's hue
/// toward the text's, so it shows on the dark and the light themes alike.
const OUTLINE_TOWARD_TEXT: f32 = 0.35;

// ========================================================================
// Data Structures
// ========================================================================

/// The rows a focus flash lights, and how strongly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LineFlash {
    /// The inclusive span of viewport rows: a soft-wrapped line's every row.
    pub(super) rows: (usize, usize),
    /// From 1 as it lights to 0 once it has faded.
    pub(super) strength: f32,
}

/// Parameters for building one pane's background fills.
pub(super) struct BgParams<'a> {
    pub(super) ch: f32,
    /// Draw the grid cursor in its unfocused form instead of filling
    /// `cursor_shape`. See [`PaneView::cursor_unfocused`](crate::render::renderer::PaneView::cursor_unfocused).
    pub(super) cursor_unfocused: bool,
    pub(super) cursor_shape: CursorShape,
    pub(super) cw: f32,
    /// Right edge (px) of the pane's content area; the scrollbar hugs it.
    pub(super) content_right: f32,
    /// When `true`, lerp all drawn colors toward the background to dim this pane.
    pub(super) dim: bool,
    pub(super) hide_cursor: bool,
    pub(super) hovered_link: u16,
    pub(super) labels: Option<&'a [(usize, usize, char)]>,
    /// How many columns and rows of cells the pane has room for. A grid not
    /// yet reflowed to a smaller pane is larger, and painting all of it bled
    /// its backgrounds and cursor into the pane beside it.
    pub(super) room: (usize, usize),
    pub(super) find_labels: &'a [(usize, usize, char)],
    pub(super) offset_x: f32,
    /// Cells drawn over the grid in place of its own (see
    /// [`PaneView::overlay`](crate::render::renderer::PaneView::overlay)).
    pub(super) overlay: Option<&'a CellOverlay>,
    /// The pane-wide state cells are shown under (reverse video, conceal,
    /// the blink phase).
    pub(super) presentation: ScreenPresentation,
    pub(super) offset_y: f32,
    /// The inclusive span of viewport rows to paint with [`Theme::cursor_line_bg`]
    /// : the Normal-mode cursor's logical line, which covers every row of a
    /// soft-wrapped line. `None` outside Normal/Visual mode.
    pub(super) cursor_line: Option<(usize, usize)>,
    /// The inclusive span of viewport rows to paint with [`Theme::jump_target_bg`]
    pub(super) jump_line: Option<(usize, usize)>,
    /// The focus flash, while it lasts (see [`PaneView::line_flash`](crate::render::renderer::PaneView::line_flash)).
    pub(super) line_flash: Option<LineFlash>,
    pub(super) scroll_offset: usize,
    pub(super) scrollback_len: usize,
    pub(super) scrollbar_active: bool,
    pub(super) scrollbar_hover: bool,
    /// How much of the scrollbar shows, from 0 (none) to 1 (in full).
    pub(super) scrollbar_opacity: f32,
    pub(super) search_current: &'a [(usize, usize)],
    pub(super) search_matches: &'a [(usize, usize)],
    /// Sentence-highlight bands `(row, start, end, tone)`; see
    /// [`PaneView::sentence_spans`](crate::render::renderer::PaneView::sentence_spans).
    pub(super) sentence_spans: &'a [(usize, usize, usize, u8)],
    /// Runs of cells outlined over their own backgrounds (see
    /// [`PaneView::outlines`](crate::render::renderer::PaneView::outlines)).
    pub(super) outlines: &'a [(usize, usize, usize)],
    pub(super) selection: Option<(usize, usize, usize, usize)>,
    pub(super) selection_block: bool,
    pub(super) theme: &'a Theme,
    /// Underline cells carrying a link. See [`PaneView::url_underline`](crate::render::renderer::PaneView::url_underline).
    pub(super) url_underline: bool,
}

/// The per-cell overlay positions of one pane, as sets, so each cell's
/// membership is one lookup.
struct CellLookups {
    current_matches: HashSet<(usize, usize)>,
    find_labels: HashSet<(usize, usize)>,
    labels: HashSet<(usize, usize)>,
    matches: HashSet<(usize, usize)>,
}

/// What one pane draws besides its glyphs: the cells' backgrounds, which
/// misty lays under the glyphs, and the shapes drawn over them.
#[derive(Debug, Default)]
pub(crate) struct PaneFills {
    /// What is drawn over the cells, in order: a bar or underline cursor,
    /// outlines, lines, the scrollbar.
    pub(crate) fills: Vec<SolidFill>,
    /// The cells' backgrounds and highlights, the block cursor among them,
    /// one run of equally colored cells on a row each.
    pub(crate) highlights: Vec<CellHighlight>,
}

/// Cell backgrounds as they are laid down, merging each run of equally
/// colored cells on a row into one highlight: a full-screen app that paints
/// its own background otherwise costs one per cell, enough at a large window
/// and a small font to overflow any fixed budget.
#[derive(Default)]
struct BgRuns {
    painted: PaneFills,
    pending: Option<CellRun>,
}

/// Cells `start_col..end_col` of `row`, all in `color`.
#[derive(Clone, Copy)]
struct CellRun {
    color: (f32, f32, f32),
    end_col: usize,
    row: usize,
    start_col: usize,
}

// ========================================================================
// BgRuns
// ========================================================================

impl BgRuns {
    /// Paint cell `(row, col)` in `color`, extending the run it continues.
    fn push_cell(&mut self, row: usize, col: usize, color: (f32, f32, f32)) {
        if let Some(run) = self.pending.as_mut()
            && run.row == row
            && run.end_col == col
            && run.color == color
        {
            run.end_col += 1;
            return;
        }
        self.flush();
        self.pending = Some(CellRun {
            color,
            end_col: col + 1,
            row,
            start_col: col,
        });
    }

    /// Paint the pixel rect `(x0, y0, x1, y1)` over the cells, as a bar or
    /// underline cursor is drawn over the glyph it marks.
    fn push_rect(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), color: (f32, f32, f32)) {
        self.painted.fills.push(quad_fill(x0, y0, x1, y1, color));
    }

    fn flush(&mut self) {
        let Some(run) = self.pending.take() else {
            return;
        };
        self.painted.highlights.push(CellHighlight {
            color: fill_color(run.color),
            cols: run.start_col..run.end_col,
            row: run.row,
        });
    }

    fn finish(mut self) -> PaneFills {
        self.flush();
        self.painted
    }
}

// ========================================================================
// CellLookups
// ========================================================================

impl CellLookups {
    fn new(params: &BgParams) -> Self {
        let positions = |cells: &[(usize, usize, char)]| -> HashSet<(usize, usize)> {
            cells.iter().map(|&(r, c, _)| (r, c)).collect()
        };
        Self {
            current_matches: params.search_current.iter().copied().collect(),
            find_labels: positions(params.find_labels),
            labels: params.labels.map(positions).unwrap_or_default(),
            matches: params.search_matches.iter().copied().collect(),
        }
    }
}

// ========================================================================
// Background fills
// ========================================================================

/// The inclusive column span `[start, end]` of `sel_norm` that should be
/// highlighted on `abs_row` (an absolute row from `Grid::to_absolute_row`),
/// clamped to `content_end` (the row's last printable column, `None` for a
/// blank row) so the selection hugs the text instead of spilling past the
/// newline across the trailing blank cells. `last_col` is the row's final
/// column index, used for rows that run to the line's end. Returns `None` when
/// `abs_row` is outside the selection or no content falls within the selected
/// span.
///
/// A blank row highlights nothing at all. Clamping it to column 0 instead
/// leaves one stray highlighted cell on the left edge of every blank line a
/// drag crosses, which reads as a broken selection down the reserved band of
/// a rich block: the band is a dozen blank rows in a row.
fn selection_span_on_row(
    sel_norm: Option<(usize, usize, usize, usize)>,
    block: bool,
    abs_row: usize,
    last_col: usize,
    content_end: Option<usize>,
) -> Option<(usize, usize)> {
    let (sr1, sc1, sr2, sc2) = sel_norm?;
    if abs_row < sr1 || abs_row > sr2 {
        return None;
    }
    let content_end = content_end?;
    if block {
        let start = sc1.min(sc2);
        let end = sc1.max(sc2).min(content_end);
        return (start <= end).then_some((start, end));
    }
    let start = if abs_row == sr1 { sc1 } else { 0 };
    let end = if abs_row == sr2 { sc2 } else { last_col };
    let end = end.min(content_end);
    (start <= end).then_some((start, end))
}

/// The selection with its endpoints in reading order; a block selection
/// orders its rows only, keeping its column edges as given.
fn normalize_selection(
    selection: Option<(usize, usize, usize, usize)>,
    block: bool,
) -> Option<(usize, usize, usize, usize)> {
    selection.map(|(r1, c1, r2, c2)| {
        if block {
            (r1.min(r2), c1, r1.max(r2), c2)
        } else if (r1, c1) > (r2, c2) {
            (r2, c2, r1, c1)
        } else {
            (r1, c1, r2, c2)
        }
    })
}

/// Build one pane's background quads: cell backgrounds and highlights,
/// the cursor, decorations, and the scrollbar.
pub(super) fn build_bg_fills(grid: &Grid, params: BgParams) -> PaneFills {
    let (cursor_row, cursor_col) = grid.cursor();
    let theme = params.theme;
    let bg_lin = theme.background.as_linear();
    let (cw, ch) = (params.cw, params.ch);
    let sel_norm = normalize_selection(params.selection, params.selection_block);
    let lookups = CellLookups::new(&params);
    let decoration = DecorationStyle {
        bg_lin,
        dim: params.dim,
        hovered_link: params.hovered_link,
        theme,
        url_underline: params.url_underline,
    };
    let mut cells = BgRuns::default();
    // What draws over the cell backgrounds: an unfocused cursor's outline
    // and every decoration.
    let mut over: Vec<SolidFill> = Vec::new();
    // This row's sentence bands (see `PaneView::sentence_spans`), found once
    // per row instead of per cell.
    let mut row_sentence_spans: Vec<(usize, usize, u8)> = Vec::new();
    push_outlines(&mut over, params.outlines, &params, &decoration);

    let (room_cols, room_rows) = params.room;
    for row in 0..grid.rows().min(room_rows) {
        // The selection columns active on this row, clamped to the row's content
        // end so the highlight hugs the text and never extends past the newline
        // into the trailing blank cells. `selection` addresses rows absolutely
        // (see `Grid::to_absolute_row`), so the viewport row is converted before
        // comparing.
        let row_sel = selection_span_on_row(
            sel_norm,
            params.selection_block,
            grid.to_absolute_row(row),
            grid.cols().saturating_sub(1),
            grid.visible_line_content_end(row),
        );
        row_sentence_spans.clear();
        row_sentence_spans.extend(
            params
                .sentence_spans
                .iter()
                .filter(|&&(srow, ..)| srow == row)
                .map(|&(_, start, end, tone)| (start, end, tone)),
        );
        let in_cursor_line = params
            .cursor_line
            .is_some_and(|(first, last)| row >= first && row <= last);
        let in_jump_line = params
            .jump_line
            .is_some_and(|(first, last)| row >= first && row <= last);
        let flash = params
            .line_flash
            .filter(|flash| row >= flash.rows.0 && row <= flash.rows.1)
            .map(|flash| FLASH_PEAK_ALPHA * flash.strength);

        for col in 0..grid.cols().min(room_cols) {
            let shown = shown_cell(grid, params.overlay, row, col, params.presentation);
            let cell = shown.as_deref();
            let x0 = params.offset_x + col as f32 * cw;
            let y0 = params.offset_y + row as f32 * ch;
            let is_cursor = !params.hide_cursor
                && row == cursor_row + params.scroll_offset
                && col == cursor_col;
            // An unfocused cursor of any shape draws only a cell-sized outline
            // (below, over this cell's own background), so it never forces the
            // full-cell fill.
            let cursor_outline = is_cursor && params.cursor_unfocused;
            let reversed = cell.is_some_and(|c| c.style.reversed);
            let bg = cell.map(|c| c.style.background).unwrap_or_default();
            // The cursor's row and the sentence bands tint only cells without
            // a background of their own, so program output isn't recolored.
            let plain_bg = matches!(bg, GridColor::Default) && !reversed;
            let overlays = CellBgOverlays {
                cursor_filled: is_cursor && !cursor_outline,
                dim: params.dim,
                is_current_match: lookups.current_matches.contains(&(row, col)),
                flash,
                is_cursor_line: in_cursor_line && plain_bg,
                is_jump_target: in_jump_line && plain_bg,
                is_find_label: lookups.find_labels.contains(&(row, col)),
                is_label: lookups.labels.contains(&(row, col)),
                is_search_match: lookups.matches.contains(&(row, col)),
                is_selected: row_sel.is_some_and(|(start, end)| col >= start && col <= end),
                reversed,
                sentence_tone: plain_bg
                    .then(|| {
                        row_sentence_spans
                            .iter()
                            .find(|&&(start, end, _)| col >= start && col < end)
                            .map(|&(_, _, tone)| tone)
                    })
                    .flatten(),
            };
            let fg = cell.map(|c| c.style.foreground).unwrap_or_default();
            // A bar or an underline covers only part of its cell, so the
            // cell's own fill (a flash, a selection, a program's color) goes
            // down first; skipped, the rest of the cell shows as a hole of
            // the pane's plain background.
            let under = CellBgOverlays {
                cursor_filled: false,
                ..overlays
            };
            let partial = params.cursor_shape != CursorShape::Block;
            if overlays.cursor_filled && partial && under.paints(bg) {
                cells.push_cell(row, col, cell_bg_color(under, bg, fg, theme, bg_lin));
            }
            if overlays.paints(bg) {
                let color = cell_bg_color(overlays, bg, fg, theme, bg_lin);
                // A block cursor is its cell's background, under the glyph
                // drawn in its contrast color; a bar or an underline is drawn
                // over the glyph.
                match overlays.cursor_filled && partial {
                    true => cells.push_rect(
                        cursor_quad(params.cursor_shape, x0, y0, cw, ch, grid.wrap_pending()),
                        color,
                    ),
                    false => cells.push_cell(row, col, color),
                }
            }
            if cursor_outline {
                push_cursor_outline(&mut over, (x0, y0, cw, ch), &decoration);
            }
            if let Some(cell) = cell {
                push_decorations(&mut over, cell, (x0, y0, cw, ch), &decoration);
            }
        }
    }

    let mut painted = cells.finish();
    painted.fills.extend_from_slice(&over);
    // Scrollbar: shown only when there is scrollback to navigate.
    painted
        .fills
        .extend_from_slice(&scrollbar_quads(ScrollbarParams {
            active: params.scrollbar_active,
            bg_lin,
            cell_h: ch,
            content_right: params.content_right,
            dim: params.dim,
            hovered: params.scrollbar_hover,
            opacity: params.scrollbar_opacity,
            offset_y: params.offset_y,
            rows: grid.rows(),
            scroll_offset: params.scroll_offset,
            scrollback_len: params.scrollback_len,
            theme,
        }));
    painted
}

/// A hollow cursor's outline over the cell at `rect`, drawn on top of
/// whatever the cell resolved rather than folded into that fill, so the
/// glyph underneath keeps its normal color instead of the cursor's contrast
/// fix.
fn push_cursor_outline(
    out: &mut Vec<SolidFill>,
    (x0, y0, cw, ch): (f32, f32, f32, f32),
    style: &DecorationStyle,
) {
    let color = style.theme.cursor_bg.as_linear();
    let color = match style.dim {
        true => lerp_to_bg(color, style.bg_lin),
        false => color,
    };
    for (qx0, qy0, qx1, qy1) in cursor_outline_quads(x0, y0, cw, ch, CURSOR_HOLLOW_STROKE_WIDTH) {
        out.push(quad_fill(qx0, qy0, qx1, qy1, color));
    }
}

/// Each run of `outlines` drawn as a thin border around its cells, in the
/// selection's hue drawn toward the text's, over whatever the cells' own
/// backgrounds are.
fn push_outlines(
    out: &mut Vec<SolidFill>,
    outlines: &[(usize, usize, usize)],
    params: &BgParams,
    style: &DecorationStyle,
) {
    let theme = style.theme;
    let mix = |a: (f32, f32, f32), b: (f32, f32, f32), t: f32| {
        (
            a.0 + (b.0 - a.0) * t,
            a.1 + (b.1 - a.1) * t,
            a.2 + (b.2 - a.2) * t,
        )
    };
    let color = mix(
        theme.selection_bg.as_linear(),
        theme.foreground.as_linear(),
        OUTLINE_TOWARD_TEXT,
    );
    let color = match style.dim {
        true => lerp_to_bg(color, style.bg_lin),
        false => color,
    };
    for &(row, start, end) in outlines {
        if end <= start {
            continue;
        }
        let x0 = params.offset_x + start as f32 * params.cw;
        let y0 = params.offset_y + row as f32 * params.ch;
        let width = (end - start) as f32 * params.cw;
        for (qx0, qy0, qx1, qy1) in
            cursor_outline_quads(x0, y0, width, params.ch, OUTLINE_STROKE_WIDTH)
        {
            out.push(quad_fill(qx0, qy0, qx1, qy1, color));
        }
    }
}

/// The pixel rect `[px0, px1] x [py0, py1]` filled in `color`, its channels
/// `0.0..=1.0` sRGB.
pub(crate) fn quad_fill(
    px0: f32,
    py0: f32,
    px1: f32,
    py1: f32,
    color: (f32, f32, f32),
) -> SolidFill {
    let (x0, x1) = (px0.min(px1), px0.max(px1));
    let (y0, y1) = (py0.min(py1), py0.max(py1));
    SolidFill::new(Rect::new(x0, y0, x1 - x0, y1 - y0), fill_color(color))
}

/// `color`, its channels `0.0..=1.0` sRGB, as an opaque fill color.
pub(crate) fn fill_color((r, g, b): (f32, f32, f32)) -> Rgba {
    let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Rgba::new(channel(r), channel(g), channel(b), u8::MAX)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::colors::grid_color_to_rgb;
    use super::cell_color::{
        QUICK_SELECT_BG, SEARCH_CURRENT_ALPHA, SEARCH_MATCH_ALPHA, blend_over,
    };
    use super::*;
    use winter_emulator::RgbColor;
    use winter_emulator::{Style, UnderlineStyle};

    const PLAIN_SCREEN: ScreenPresentation = ScreenPresentation {
        blink_visible: true,
        reverse_video: false,
    };

    /// Cell width and height every `build_bg_fills` test below lays its grid
    /// out at.
    const TEST_CELL_W: f32 = 10.0;
    const TEST_CELL_H: f32 = 20.0;

    /// What [`build_bg_fills`] draws for `grid`, as the rectangles it comes
    /// to, in drawing order: each highlight run over its cells, then the
    /// fills over them.
    fn drawn(grid: &Grid, params: BgParams) -> Vec<SolidFill> {
        let origin = (params.offset_x, params.offset_y);
        let painted = build_bg_fills(grid, params);
        let runs = painted.highlights.iter().map(|run| SolidFill {
            color: run.color,
            rect: Rect::new(
                origin.0 + run.cols.start as f32 * TEST_CELL_W,
                origin.1 + run.row as f32 * TEST_CELL_H,
                run.cols.len() as f32 * TEST_CELL_W,
                TEST_CELL_H,
            ),
        });
        runs.chain(painted.fills).collect()
    }

    /// How many cells the fills painted in `color` cover, however they are
    /// merged into runs.
    fn painted_cells(fills: &[SolidFill], color: (f32, f32, f32)) -> usize {
        let width: f32 = fills
            .iter()
            .filter(|fill| fill.color == fill_color(color))
            .map(|fill| fill.rect.width)
            .sum();
        (width / TEST_CELL_W).round() as usize
    }

    #[test]
    fn test_build_bg_fills_empty_grid() {
        let grid = Grid::new(4, 2);
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: true,
            },
        );
        assert_eq!(fills.len(), 1);
    }

    #[test]
    fn test_an_outlined_run_draws_four_thin_edges_and_no_fill() {
        let grid = Grid::new(10, 2);
        let build = |outlines: &[(usize, usize, usize)]| {
            drawn(
                &grid,
                BgParams {
                    ch: 20.0,
                    content_right: 100.0,
                    cursor_unfocused: false,
                    cursor_shape: CursorShape::Block,
                    cw: TEST_CELL_W,
                    dim: false,
                    hide_cursor: true,
                    hovered_link: 0,
                    labels: None,
                    room: (usize::MAX, usize::MAX),
                    find_labels: &[],
                    offset_x: 0.0,
                    overlay: None,
                    presentation: PLAIN_SCREEN,
                    offset_y: 0.0,
                    cursor_line: None,
                    jump_line: None,
                    line_flash: None,
                    scroll_offset: 0,
                    scrollback_len: 0,
                    scrollbar_active: false,
                    scrollbar_hover: false,
                    scrollbar_opacity: 1.0,
                    search_matches: &[],
                    search_current: &[],
                    sentence_spans: &[],
                    outlines,
                    selection: None,
                    selection_block: false,
                    theme: &Theme::default(),
                    url_underline: false,
                },
            )
        };
        let plain = build(&[]);
        let outlined = build(&[(1, 2, 5)]);
        let added = &outlined[plain.len()..];
        assert_eq!(added.len(), 4, "four edges");
        let lo = added
            .iter()
            .map(|fill| fill.rect.x)
            .fold(f32::MAX, f32::min);
        let hi = added
            .iter()
            .map(|fill| fill.rect.x + fill.rect.width)
            .fold(f32::MIN, f32::max);
        assert_eq!(
            (lo.round(), hi.round()),
            (20.0, 50.0),
            "around cells 2 to 4"
        );
        let widest = added
            .iter()
            .map(|fill| fill.rect.width.min(fill.rect.height))
            .fold(0.0, f32::max);
        assert!(
            widest <= OUTLINE_STROKE_WIDTH + 0.01,
            "each edge is a thin stroke, not a fill: {widest}"
        );
    }

    #[test]
    fn test_build_bg_fills_hollow_cursor_draws_an_outline_not_a_fill() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: true,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: true,
            },
        );
        // A filled cursor is one fill (see the sibling test above); a hollow
        // one is four thin outline strips instead.
        assert_eq!(fills.len(), 4);
        let cursor_color = Theme::default().cursor_bg.as_linear();
        assert!(
            fills.iter().all(|v| v.color == fill_color(cursor_color)),
            "every fill should be one of the outline strips"
        );
    }

    #[test]
    fn test_build_bg_fills_unfocused_bar_cursor_draws_a_hollow_block() {
        // An unfocused window draws a Bar cursor as the same hollow cell
        // outline a Block gets: four strips in the cursor color, not the
        // bar's own thin strip.
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let theme = Theme::default();
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: true,
                cursor_shape: CursorShape::Bar,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );
        assert_eq!(fills.len(), 4);
        let cursor_color = theme.cursor_bg.as_linear();
        assert!(
            fills.iter().all(|v| v.color == fill_color(cursor_color)),
            "every fill should be one of the outline strips"
        );
    }

    #[test]
    fn test_search_highlights_tint_current_and_other_matches_differently() {
        // The focused match gets `search_current_bg`, the rest `search_match_bg`,
        // and both are composited over the cell's background rather than painted
        // opaque, so the glyph's own foreground still reads on top.
        let mut grid = Grid::new(4, 1);
        grid.print('a');
        grid.print('b');
        let theme = Theme::default();
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[(0, 0), (0, 1)],
                search_current: &[(0, 1)],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );

        let bits = fill_color;
        let drawn: HashSet<Rgba> = fills.iter().map(|v| v.color).collect();
        let bg = theme.background.as_linear();

        assert!(
            drawn.contains(&bits(blend_over(
                theme.search_match_bg.as_linear(),
                bg,
                SEARCH_MATCH_ALPHA
            ))),
            "the non-current match should be tinted with search_match_bg"
        );
        assert!(
            drawn.contains(&bits(blend_over(
                theme.search_current_bg.as_linear(),
                bg,
                SEARCH_CURRENT_ALPHA
            ))),
            "the current match should be tinted with search_current_bg"
        );
        assert!(
            !drawn.contains(&bits(theme.search_match_bg.as_linear()))
                && !drawn.contains(&bits(theme.search_current_bg.as_linear())),
            "neither highlight should be drawn at full opacity"
        );
    }

    #[test]
    fn test_selection_highlight_covers_both_endpoints_including_the_cursor_cell() {
        // The Visual-mode selection runs anchor..cursor inclusive, so the cell the
        // cursor sits on is highlighted like the rest: three cells for a
        // three-column span, not two.
        let mut grid = Grid::new(8, 1);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 80.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: Some((0, 0, 0, 2)),
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );

        let sel = theme.selection_bg.as_linear();
        let selected_cells = painted_cells(&fills, sel);
        assert_eq!(selected_cells, 3, "cols 0, 1 and 2 should all be selected");
    }

    #[test]
    fn test_a_bar_cursor_leaves_its_cells_own_fill_around_it() {
        // Regression: a bar cursor's cell drew only the bar, so the rest of it
        // showed the pane's plain background, a dark hole in a selection or a
        // flashed line.
        let mut grid = Grid::new(4, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.move_to(0, 1);
        let theme = Theme::default();
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Bar,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: Some((0, 0, 0, 2)),
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );
        let sel = theme.selection_bg.as_linear();
        assert_eq!(
            painted_cells(&fills, sel),
            3,
            "the bar's cell is selected too"
        );
    }

    #[test]
    fn test_cursor_line_paints_the_whole_row_but_spares_colored_cells() {
        // Normal mode washes the cursor's row with `cursor_line_bg`, across the
        // full pane width: except cells carrying their own background, which keep
        // it so program output isn't recolored.
        let mut grid = Grid::new(4, 2);
        let colored = winter_emulator::Style {
            background: GridColor::Rgb(RgbColor { r: 9, g: 90, b: 9 }),
            ..Default::default()
        };
        grid.set_style(colored);
        grid.print('x');
        grid.set_style(winter_emulator::Style::default());
        let theme = Theme::default();
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: Some((0, 0)),
                jump_line: None,
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );

        let want = theme.cursor_line_bg.as_linear();
        let banded = painted_cells(&fills, want);
        assert_eq!(
            banded, 3,
            "3 of the row's 4 cells are washed; the SGR-colored one keeps its own bg"
        );

        // No cursor line (Insert mode) means no wash at all.
        let plain = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );
        assert_eq!(
            plain.iter().filter(|v| v.color == fill_color(want)).count(),
            0,
            "no cursor line outside Normal/Visual"
        );
    }

    #[test]
    fn test_jump_line_paints_with_jump_target_bg_and_differs_from_cursor_line() {
        let grid = Grid::new(4, 2);
        let theme = Theme::default();
        assert_ne!(
            theme.jump_target_bg, theme.cursor_line_bg,
            "jump target highlight must differ from cursor line background"
        );
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: Some((0, 0)),
                jump_line: Some((0, 0)),
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );

        let jump_want = theme.jump_target_bg.as_linear();
        let cursor_want = theme.cursor_line_bg.as_linear();
        assert_eq!(
            painted_cells(&fills, jump_want),
            4,
            "all 4 cells in the row are painted with jump_target_bg"
        );
        assert_eq!(
            painted_cells(&fills, cursor_want),
            0,
            "jump_target_bg takes precedence over cursor_line_bg"
        );
    }

    #[test]
    fn test_cursor_line_band_covers_a_soft_wrapped_line() {
        // A wrapped command line is one logical line, so the band covers both of
        // its rows: the span comes from `Grid::wrapped_row_span`.
        let mut grid = Grid::new(4, 3);
        for ch in "abcdefg".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: Some(grid.wrapped_row_span(1)),
                jump_line: None,
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );

        let want = theme.cursor_line_bg.as_linear();
        let banded = painted_cells(&fills, want);
        assert_eq!(
            banded, 8,
            "both 4-column rows of the wrapped line are washed"
        );
    }

    #[test]
    fn test_find_label_boxes_use_the_theme_find_label_background() {
        // The `f`/`t` jump labels get their own light box, distinct from the amber
        // quick-select labels, so the two overlays can't be confused.
        let mut grid = Grid::new(4, 1);
        grid.print('a');
        let theme = Theme::default();
        let find_labels: &[(usize, usize, char)] = &[(0, 2, 's')];
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: true,
            },
        );

        let want = theme.find_label_bg.as_linear();
        let boxes = painted_cells(&fills, want);
        assert_eq!(boxes, 1, "one label box drawn in find_label_bg");
        assert_ne!(want, QUICK_SELECT_BG, "and it differs from quick-select's");
    }

    #[test]
    fn test_build_bg_fills_produces_same_count() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let with_offset = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 100.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 50.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: true,
            },
        );
        let without_offset = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: true,
            },
        );
        assert_eq!(with_offset.len(), without_offset.len());
    }

    #[test]
    fn test_selection_span_clamps_to_content_end() {
        // 10-col rows; row 0 has content through col 4, rows fully inside the
        // selection run to the last column but must clamp to that content end.
        let sel = Some((0, 2, 2, 9));
        // First row: starts at the anchor col, ends at the content end (4), not 9.
        assert_eq!(
            selection_span_on_row(sel, false, 0, 9, Some(4)),
            Some((2, 4))
        );
        // Middle row fully selected: 0..=last_col clamped to its own content (6).
        assert_eq!(
            selection_span_on_row(sel, false, 1, 9, Some(6)),
            Some((0, 6))
        );
        // Last row: ends at the anchor col (9) clamped to content end (3).
        assert_eq!(
            selection_span_on_row(sel, false, 2, 9, Some(3)),
            Some((0, 3))
        );
    }

    #[test]
    fn test_selection_span_skips_rows_outside_and_empty_spans() {
        let sel = Some((1, 0, 1, 5));
        // Rows above/below the selection are never highlighted.
        assert_eq!(selection_span_on_row(sel, false, 0, 9, Some(9)), None);
        assert_eq!(selection_span_on_row(sel, false, 2, 9, Some(9)), None);
        // A selection that starts past the row's content highlights nothing.
        assert_eq!(
            selection_span_on_row(Some((0, 5, 0, 9)), false, 0, 9, Some(2)),
            None
        );
        // No selection at all.
        assert_eq!(selection_span_on_row(None, false, 0, 9, Some(9)), None);
    }

    #[test]
    fn test_selection_span_highlights_nothing_on_a_blank_row() {
        // Regression: a blank row's content end read as column 0, the same
        // answer as a row whose only character sits there, so every blank line
        // a drag crossed picked up a one-cell highlight on its left edge. A
        // rich block reserves a dozen blank rows, which turned that artifact
        // into a visible column of stray cells straight down the band.
        let sel = Some((0, 0, 2, 9));
        assert_eq!(selection_span_on_row(sel, false, 1, 9, None), None);
        assert_eq!(selection_span_on_row(sel, true, 1, 9, None), None);
        // A row whose sole character is in column 0 still highlights it.
        assert_eq!(
            selection_span_on_row(sel, false, 1, 9, Some(0)),
            Some((0, 0))
        );
    }

    #[test]
    fn test_selection_span_blockwise() {
        let sel = Some((0, 3, 2, 6));
        // Block selection on rows 0, 1, 2 between columns 3 and 6:
        assert_eq!(
            selection_span_on_row(sel, true, 0, 9, Some(8)),
            Some((3, 6))
        );
        assert_eq!(
            selection_span_on_row(sel, true, 1, 9, Some(8)),
            Some((3, 6))
        );
        assert_eq!(
            selection_span_on_row(sel, true, 2, 9, Some(5)),
            Some((3, 5))
        );
        // Outside row:
        assert_eq!(selection_span_on_row(sel, true, 3, 9, Some(8)), None);
    }

    #[test]
    fn test_build_bg_fills_selection_uses_absolute_row_not_viewport_row() {
        // Regression: `BgParams::selection` addresses rows absolutely (see
        // `Grid::to_absolute_row`), not by the currently-visible viewport
        // row. If the row were compared without converting it first, a
        // selection anchored to scrolled-back content would silently stop
        // highlighting anything once the view had scrolled far enough that
        // no viewport row number happened to equal the absolute one.
        let mut grid = Grid::new(4, 2);
        for line in ["AAAA", "BBBB", "CCCC", "DDDD", "EEEE", "FFFF", "GGGG"] {
            for ch in line.chars() {
                grid.print(ch);
            }
            grid.carriage_return();
            grid.line_feed();
        }
        grid.scroll_up_history(3);
        let abs_row0 = grid.to_absolute_row(0);
        assert_ne!(
            abs_row0, 0,
            "fixture needs the absolute row to diverge from the viewport row"
        );

        let theme = Theme::default();
        let params = |selection| BgParams {
            ch: 20.0,
            content_right: 40.0,
            cursor_unfocused: false,
            cursor_shape: CursorShape::Block,
            cw: 10.0,
            dim: false,
            hide_cursor: false,
            hovered_link: 0,
            labels: None,
            room: (usize::MAX, usize::MAX),
            find_labels: &[],
            cursor_line: None,
            jump_line: None,
            line_flash: None,
            offset_x: 0.0,
            overlay: None,
            presentation: PLAIN_SCREEN,
            offset_y: 0.0,
            scroll_offset: grid.scroll_offset(),
            scrollback_len: grid.scrollback_len(),
            scrollbar_active: false,
            scrollbar_hover: false,
            scrollbar_opacity: 1.0,
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
            outlines: &[],
            selection,
            selection_block: false,
            theme: &theme,
            url_underline: true,
        };
        let selected = drawn(&grid, params(Some((abs_row0, 0, abs_row0, 3))));
        let unselected = drawn(&grid, params(None));

        assert!(
            selected.len() > unselected.len(),
            "a selection covering the current top row's absolute address should add highlight quads"
        );
    }

    #[test]
    fn test_build_bg_fills_url_underline_toggle() {
        let mut grid = Grid::new(4, 2);
        grid.set_active_link(Some("https://example.com"));
        grid.print('a');
        grid.set_active_link(None);

        let with_underline = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: true,
            },
        );
        let without_underline = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: false,
            },
        );
        assert_eq!(
            with_underline.len(),
            without_underline.len() + 1,
            "url_underline #false should drop the link cell's underline quad"
        );
    }

    #[test]
    fn test_build_bg_fills_with_labels() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let labels: &[(usize, usize, char)] = &[(0, 0, 's'), (1, 2, 'd')];
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: false,
                hovered_link: 0,
                labels: Some(labels),
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &Theme::default(),
                url_underline: true,
            },
        );
        assert!(
            fills.len() >= 2,
            "label cells add extra quads beyond cursor"
        );
    }

    #[test]
    fn test_build_bg_fills_emits_quads_for_sentence_spans() {
        let mut grid = Grid::new(10, 2);
        grid.move_to(0, 0);
        for ch in "Hi! There!".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let spans = [(0, 0, 3, 0), (0, 4, 10, 1)];
        let fills = drawn(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 100.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                cursor_line: None,
                jump_line: None,
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                scrollbar_active: false,
                scrollbar_hover: false,
                scrollbar_opacity: 1.0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &spans,
                outlines: &[],
                selection: None,
                selection_block: false,
                theme: &theme,
                url_underline: false,
            },
        );
        // 3 cells for sentence 1 + 6 cells for sentence 2, one run each, as
        // the two tones differ.
        assert_eq!(fills.len(), 2);
        let width: f32 = fills.iter().map(|fill| fill.rect.width).sum();
        let cells = (width / TEST_CELL_W).round() as usize;
        assert_eq!(cells, 3 + 6);
    }

    #[test]
    fn test_a_screen_of_one_background_is_one_quad_per_row() {
        // A full-screen app painting its own background must not cost a quad
        // per cell: equal neighbours merge into one run per row.
        let mut grid = Grid::new(40, 3);
        grid.set_style(Style {
            background: GridColor::Indexed(4),
            ..Style::default()
        });
        for row in 0..3 {
            grid.move_to(row, 0);
            for _ in 0..40 {
                grid.print('x');
            }
        }
        let theme = Theme::default();
        let fills = drawn(&grid, plain_params(&theme));
        assert_eq!(fills.len(), 3, "one fill per row");
        let color = grid_color_to_rgb(&GridColor::Indexed(4), &theme);
        assert_eq!(painted_cells(&fills, color), 3 * 40);
    }

    #[test]
    fn test_decorations_draw_over_the_merged_backgrounds() {
        // Runs are laid down before anything over them; an underline drawn
        // with its own cell and then covered by the run's quad would vanish.
        let mut grid = Grid::new(4, 1);
        grid.set_style(Style {
            background: GridColor::Indexed(2),
            underline: UnderlineStyle::Single,
            ..Style::default()
        });
        for _ in 0..4 {
            grid.print('x');
        }
        let theme = Theme::default();
        let fills = drawn(&grid, plain_params(&theme));
        let background = grid_color_to_rgb(&GridColor::Indexed(2), &theme);
        let background = fill_color(background);
        assert_eq!(fills[0].color, background);
        assert!(fills.len() > 1, "the underline fills come after the run");
        assert!(fills[1..].iter().all(|fill| fill.color != background));
    }

    #[test]
    fn test_cell_backgrounds_and_a_block_cursor_lie_under_the_glyphs_and_a_bar_over_them() {
        // Drawn as fills after the cells, a program's colored line, a diff's
        // red and green rows, and the block cursor covered their glyphs.
        let mut grid = Grid::new(4, 1);
        grid.set_style(Style {
            background: GridColor::Indexed(1),
            ..Style::default()
        });
        grid.print('x');
        let theme = Theme::default();
        let shown = |shape: CursorShape| {
            build_bg_fills(
                &grid,
                BgParams {
                    cursor_shape: shape,
                    hide_cursor: false,
                    ..plain_params(&theme)
                },
            )
        };
        let block = shown(CursorShape::Block);
        assert!(block.fills.is_empty(), "nothing over the glyphs");
        let cursor = fill_color(theme.cursor_bg.as_linear());
        assert!(block.highlights.iter().any(|run| run.color == cursor));
        assert!(
            block
                .highlights
                .iter()
                .any(|run| run.row == 0 && run.cols.contains(&0))
        );
        let bar = shown(CursorShape::Bar);
        assert_eq!(bar.fills.len(), 1, "the bar, over its glyph");
        assert!(bar.highlights.iter().all(|run| run.color != cursor));
    }

    /// Parameters drawing `grid` with no cursor, selection, or overlay, at
    /// the test cell and surface widths.
    fn plain_params(theme: &Theme) -> BgParams<'_> {
        BgParams {
            ch: 20.0,
            content_right: 100.0,
            cursor_unfocused: false,
            cursor_shape: CursorShape::Block,
            cw: TEST_CELL_W,
            dim: false,
            hide_cursor: true,
            hovered_link: 0,
            labels: None,
            room: (usize::MAX, usize::MAX),
            find_labels: &[],
            offset_x: 0.0,
            overlay: None,
            presentation: PLAIN_SCREEN,
            offset_y: 0.0,
            cursor_line: None,
            jump_line: None,
            line_flash: None,
            scroll_offset: 0,
            scrollback_len: 0,
            scrollbar_active: false,
            scrollbar_hover: false,
            scrollbar_opacity: 1.0,
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
            outlines: &[],
            selection: None,
            selection_block: false,
            theme,
            url_underline: false,
        }
    }
}

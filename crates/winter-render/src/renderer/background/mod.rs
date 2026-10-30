//! Background pass: cell, cursor, selection, and scrollbar quads, plus
//! the wgpu pipelines that draw them.
//!
//! A pane's quads are built in layers: cell backgrounds first, merged into
//! one quad per run of equally colored cells on a row, then what draws over
//! them (an unfocused cursor's outline, underlines, overlines and
//! strikethroughs), then the scrollbar.

mod cell_color;
mod cursor;
mod decoration;
mod divider;
mod pipeline;
mod scrollbar;

pub(super) use cursor::{cursor_outline_quads, cursor_quad, CURSOR_HOLLOW_STROKE_WIDTH};
pub(super) use divider::{separator_vertices, separators, SeparatorStyle, DIVIDER_THICKNESS};
pub(super) use pipeline::{
    create_bg_pipeline, create_dot_pipeline, create_vertex_buffer, ensure_vertex_capacity,
    BG_BUFFER_LABEL, DOT_BUFFER_LABEL,
};

use std::collections::HashSet;

use super::colors::grid_color_to_rgb;
use super::display::{shown_cell, CellOverlay, ScreenPresentation};
use super::glyphs::{is_braille, push_braille_dots};
use crate::theme::Theme;
use cell_color::{cell_bg_color, lerp_to_bg, CellBgOverlays, FLASH_PEAK_ALPHA};
use decoration::{push_decorations, DecorationStyle};
use scrollbar::{scrollbar_quads, ScrollbarParams};
use winter_core::{Cell, Color as GridColor, CursorShape, Grid};

// ========================================================================
// Constants
// ========================================================================

/// Size the background vertex buffer starts at; it grows when a frame needs
/// more.
pub(super) const BG_BUFFER_SIZE: u64 = 4 * 1024 * 1024;

/// Size the braille dot vertex buffer starts at; it grows when a frame
/// needs more.
pub(super) const DOT_BUFFER_SIZE: u64 = 8 * 1024 * 1024;

/// How thick an outlined run's border is, in pixels.
const OUTLINE_STROKE_WIDTH: f32 = 1.0;

/// How far an outlined run's border is drawn from the selection's hue
/// toward the text's, so it shows on the dark and the light themes alike.
const OUTLINE_TOWARD_TEXT: f32 = 0.35;

// ========================================================================
// Data Structures
// ========================================================================

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct BgVertex {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) r: f32,
    pub(super) g: f32,
    pub(super) b: f32,
}

/// A vertex for the anti-aliased dot pipeline (procedural braille). `u`/`v` run
/// [-1, 1] across the dot's quad so the fragment shader can draw a smooth circle.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub(super) struct DotVertex {
    pub(super) x: f32,
    pub(super) y: f32,
    pub(super) u: f32,
    pub(super) v: f32,
    pub(super) r: f32,
    pub(super) g: f32,
    pub(super) b: f32,
}

/// The rows a focus flash lights, and how strongly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct LineFlash {
    /// The inclusive span of viewport rows: a soft-wrapped line's every row.
    pub(super) rows: (usize, usize),
    /// From 1 as it lights to 0 once it has faded.
    pub(super) strength: f32,
}

/// Parameters for building background vertices for one pane.
pub(super) struct BgParams<'a> {
    pub(super) ch: f32,
    /// Draw the grid cursor in its unfocused form instead of filling
    /// `cursor_shape`. See [`PaneView::cursor_unfocused`](crate::renderer::PaneView::cursor_unfocused).
    pub(super) cursor_unfocused: bool,
    pub(super) cursor_shape: CursorShape,
    pub(super) cw: f32,
    /// Right edge (px) of the pane's content area; the scrollbar hugs it.
    pub(super) content_right: f32,
    /// When `true`, lerp all drawn colors toward the background to dim this pane.
    pub(super) dim: bool,
    /// Draw braille cells procedurally as dot quads. Set when the active font
    /// lacks braille glyphs; when it has them, braille is rendered by the font.
    pub(super) draw_braille_dots: bool,
    /// Whether this pane is focused; an unfocused pane's scrollbar is dimmed.
    pub(super) focused: bool,
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
    /// [`PaneView::overlay`](crate::renderer::PaneView::overlay)).
    pub(super) overlay: Option<&'a CellOverlay>,
    /// The pane-wide state cells are shown under (reverse video, conceal,
    /// the blink phase).
    pub(super) presentation: ScreenPresentation,
    pub(super) offset_y: f32,
    /// The inclusive span of viewport rows to paint with [`Theme::cursor_line_bg`]
    ///: the Normal-mode cursor's logical line, which covers every row of a
    /// soft-wrapped line. `None` outside Normal/Visual mode.
    pub(super) cursor_line: Option<(usize, usize)>,
    /// The focus flash, while it lasts (see [`PaneView::line_flash`](crate::renderer::PaneView::line_flash)).
    pub(super) line_flash: Option<LineFlash>,
    pub(super) scroll_offset: usize,
    pub(super) scrollback_len: usize,
    pub(super) search_current: &'a [(usize, usize)],
    pub(super) search_matches: &'a [(usize, usize)],
    /// Sentence-highlight bands `(row, start, end, tone)`; see
    /// [`PaneView::sentence_spans`](crate::renderer::PaneView::sentence_spans).
    pub(super) sentence_spans: &'a [(usize, usize, usize, u8)],
    /// Runs of cells outlined over their own backgrounds (see
    /// [`PaneView::outlines`](crate::renderer::PaneView::outlines)).
    pub(super) outlines: &'a [(usize, usize, usize)],
    pub(super) selection: Option<(usize, usize, usize, usize)>,
    pub(super) selection_block: bool,
    pub(super) surface_h: f32,
    pub(super) surface_w: f32,
    pub(super) theme: &'a Theme,
    /// Underline cells carrying a link. See [`PaneView::url_underline`](crate::renderer::PaneView::url_underline).
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

/// Cell backgrounds as they are laid down, merging each run of equally
/// colored cells on a row into one quad: a full-screen app that paints its
/// own background otherwise costs a quad per cell, enough at a large window
/// and a small font to overflow any fixed vertex budget.
struct BgRuns {
    ch: f32,
    cw: f32,
    offset: (f32, f32),
    pending: Option<CellRun>,
    surface: (f32, f32),
    verts: Vec<BgVertex>,
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
    fn new(params: &BgParams) -> Self {
        Self {
            ch: params.ch,
            cw: params.cw,
            offset: (params.offset_x, params.offset_y),
            pending: None,
            surface: (params.surface_w, params.surface_h),
            verts: Vec::new(),
        }
    }

    /// Paint cell `(row, col)` in `color`, extending the run it continues.
    fn push_cell(&mut self, row: usize, col: usize, color: (f32, f32, f32)) {
        if let Some(run) = self.pending.as_mut() {
            if run.row == row && run.end_col == col && run.color == color {
                run.end_col += 1;
                return;
            }
        }
        self.flush();
        self.pending = Some(CellRun {
            color,
            end_col: col + 1,
            row,
            start_col: col,
        });
    }

    /// Paint the pixel rect `(x0, y0, x1, y1)` on its own, after every cell
    /// before it, as the order of a filled cursor's shape requires.
    fn push_rect(&mut self, (x0, y0, x1, y1): (f32, f32, f32, f32), color: (f32, f32, f32)) {
        self.flush();
        let (surface_w, surface_h) = self.surface;
        self.verts
            .extend_from_slice(&quad_vertices(x0, y0, x1, y1, color, surface_w, surface_h));
    }

    fn flush(&mut self) {
        let Some(run) = self.pending.take() else {
            return;
        };
        let (ox, oy) = self.offset;
        let y0 = oy + run.row as f32 * self.ch;
        let (surface_w, surface_h) = self.surface;
        self.verts.extend_from_slice(&quad_vertices(
            ox + run.start_col as f32 * self.cw,
            y0,
            ox + run.end_col as f32 * self.cw,
            y0 + self.ch,
            run.color,
            surface_w,
            surface_h,
        ));
    }

    fn finish(mut self) -> Vec<BgVertex> {
        self.flush();
        self.verts
    }
}

// ========================================================================
// BgVertex and DotVertex
// ========================================================================

impl BgVertex {
    pub(super) fn to_bytes(self) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0..4].copy_from_slice(&self.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.r.to_le_bytes());
        out[12..16].copy_from_slice(&self.g.to_le_bytes());
        out[16..20].copy_from_slice(&self.b.to_le_bytes());
        out
    }
}

impl DotVertex {
    pub(super) fn to_bytes(self) -> [u8; 28] {
        let mut out = [0u8; 28];
        out[0..4].copy_from_slice(&self.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.u.to_le_bytes());
        out[12..16].copy_from_slice(&self.v.to_le_bytes());
        out[16..20].copy_from_slice(&self.r.to_le_bytes());
        out[20..24].copy_from_slice(&self.g.to_le_bytes());
        out[24..28].copy_from_slice(&self.b.to_le_bytes());
        out
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
// Background vertex construction
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
/// the cursor, decorations, and the scrollbar, appending braille dots the
/// font cannot draw to `dots`.
pub(super) fn build_bg_vertices_offset(
    grid: &Grid,
    params: BgParams,
    dots: &mut Vec<DotVertex>,
) -> Vec<BgVertex> {
    let (cursor_row, cursor_col) = grid.cursor();
    let theme = params.theme;
    let bg_lin = theme.background.as_linear();
    let (cw, ch) = (params.cw, params.ch);
    let surface = (params.surface_w, params.surface_h);
    let sel_norm = normalize_selection(params.selection, params.selection_block);
    let lookups = CellLookups::new(&params);
    let decoration = DecorationStyle {
        bg_lin,
        dim: params.dim,
        hovered_link: params.hovered_link,
        surface,
        theme,
        url_underline: params.url_underline,
    };
    let mut cells = BgRuns::new(&params);
    // What draws over the cell backgrounds: an unfocused cursor's outline
    // and every decoration.
    let mut over: Vec<BgVertex> = Vec::new();
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
                match overlays.cursor_filled {
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
                push_decorations(&mut over, dots, cell, (x0, y0, cw, ch), &decoration);
                if params.draw_braille_dots && is_braille(cell.ch) {
                    push_cell_braille(dots, cell, (x0, y0, cw, ch), &decoration);
                }
            }
        }
    }

    let mut verts = cells.finish();
    verts.extend_from_slice(&over);
    // Scrollbar: shown only when there is scrollback to navigate.
    verts.extend_from_slice(&scrollbar_quads(ScrollbarParams {
        bg_lin,
        cell_h: ch,
        content_right: params.content_right,
        dim: params.dim,
        focused: params.focused,
        offset_y: params.offset_y,
        rows: grid.rows(),
        scroll_offset: params.scroll_offset,
        scrollback_len: params.scrollback_len,
        surface_h: params.surface_h,
        surface_w: params.surface_w,
        theme,
    }));
    verts
}

/// A hollow cursor's outline over the cell at `rect`, drawn on top of
/// whatever the cell resolved rather than folded into that fill, so the
/// glyph underneath keeps its normal color instead of the cursor's contrast
/// fix.
fn push_cursor_outline(
    out: &mut Vec<BgVertex>,
    (x0, y0, cw, ch): (f32, f32, f32, f32),
    style: &DecorationStyle,
) {
    let color = style.theme.cursor_bg.as_linear();
    let color = match style.dim {
        true => lerp_to_bg(color, style.bg_lin),
        false => color,
    };
    let (surface_w, surface_h) = style.surface;
    for (qx0, qy0, qx1, qy1) in cursor_outline_quads(x0, y0, cw, ch, CURSOR_HOLLOW_STROKE_WIDTH) {
        out.extend_from_slice(&quad_vertices(
            qx0, qy0, qx1, qy1, color, surface_w, surface_h,
        ));
    }
}

/// Each run of `outlines` drawn as a thin border around its cells, in the
/// selection's hue drawn toward the text's, over whatever the cells' own
/// backgrounds are.
fn push_outlines(
    out: &mut Vec<BgVertex>,
    outlines: &[(usize, usize, usize)],
    params: &BgParams,
    style: &DecorationStyle,
) {
    let theme = style.theme;
    let mix = |a: (f32, f32, f32), b: (f32, f32, f32), t: f32| {
        (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t, a.2 + (b.2 - a.2) * t)
    };
    let color = mix(theme.selection_bg.as_linear(), theme.foreground.as_linear(), OUTLINE_TOWARD_TEXT);
    let color = match style.dim {
        true => lerp_to_bg(color, style.bg_lin),
        false => color,
    };
    let (surface_w, surface_h) = style.surface;
    for &(row, start, end) in outlines {
        if end <= start {
            continue;
        }
        let x0 = params.offset_x + start as f32 * params.cw;
        let y0 = params.offset_y + row as f32 * params.ch;
        let width = (end - start) as f32 * params.cw;
        for (qx0, qy0, qx1, qy1) in cursor_outline_quads(x0, y0, width, params.ch, OUTLINE_STROKE_WIDTH) {
            out.extend_from_slice(&quad_vertices(qx0, qy0, qx1, qy1, color, surface_w, surface_h));
        }
    }
}

/// A braille cell drawn as dot quads in its foreground color, used when
/// the font lacks braille, so btop's graphs stay aligned and legible instead
/// of using a misaligned proportional fallback.
fn push_cell_braille(
    dots: &mut Vec<DotVertex>,
    cell: &Cell,
    rect: (f32, f32, f32, f32),
    style: &DecorationStyle,
) {
    let fg = match cell.style.foreground {
        GridColor::Default => style.theme.foreground.as_linear(),
        foreground => grid_color_to_rgb(&foreground, style.theme),
    };
    let color = match style.dim {
        true => lerp_to_bg(fg, style.bg_lin),
        false => fg,
    };
    push_braille_dots(dots, cell.ch, rect, color, style.surface);
}

/// Two triangles covering the pixel rect `[px0,px1] x [py0,py1]` in `color`,
/// converted to normalized device coordinates for the bg pipeline.
pub(crate) fn quad_vertices(
    px0: f32,
    py0: f32,
    px1: f32,
    py1: f32,
    color: (f32, f32, f32),
    surface_w: f32,
    surface_h: f32,
) -> [BgVertex; 6] {
    let (r, g, b) = color;
    let ndc_x0 = px0 * 2.0 / surface_w - 1.0;
    let ndc_y0 = 1.0 - py0 * 2.0 / surface_h;
    let ndc_x1 = px1 * 2.0 / surface_w - 1.0;
    let ndc_y1 = 1.0 - py1 * 2.0 / surface_h;

    [
        BgVertex {
            x: ndc_x0,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x0,
            y: ndc_y1,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y0,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x1,
            y: ndc_y1,
            r,
            g,
            b,
        },
        BgVertex {
            x: ndc_x0,
            y: ndc_y1,
            r,
            g,
            b,
        },
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::cell_color::{
        blend_over, QUICK_SELECT_BG, SEARCH_CURRENT_ALPHA, SEARCH_MATCH_ALPHA,
    };
    use super::*;
    use winter_core::RgbColor;
    use winter_core::{Style, UnderlineStyle};

    const PLAIN_SCREEN: ScreenPresentation = ScreenPresentation {
        blink_visible: true,
        reverse_video: false,
    };

    /// Cell width and surface width every `build_bg_vertices_offset` test
    /// below lays its grid out at.
    const TEST_CELL_W: f32 = 10.0;
    const TEST_SURFACE_W: f32 = 800.0;

    /// How many cells the quads painted in `color` cover, however they are
    /// merged into runs.
    fn painted_cells(verts: &[BgVertex], color: (f32, f32, f32)) -> usize {
        let width: f32 = verts
            .chunks_exact(6)
            .filter(|quad| (quad[0].r, quad[0].g, quad[0].b) == color)
            .map(|quad| {
                let xs = quad.iter().map(|v| (v.x + 1.0) * TEST_SURFACE_W / 2.0);
                let (lo, hi) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
                hi - lo
            })
            .sum();
        (width / TEST_CELL_W).round() as usize
    }

    #[test]
    fn test_build_bg_vertices_empty_grid() {
        let grid = Grid::new(4, 2);
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn test_an_outlined_run_draws_four_thin_edges_and_no_fill() {
        let grid = Grid::new(10, 2);
        let build = |outlines: &[(usize, usize, usize)]| {
            build_bg_vertices_offset(
                &grid,
                BgParams {
                    ch: 20.0,
                    content_right: 100.0,
                    cursor_unfocused: false,
                    cursor_shape: CursorShape::Block,
                    cw: TEST_CELL_W,
                    dim: false,
                    draw_braille_dots: true,
                    focused: true,
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
                    line_flash: None,
                    scroll_offset: 0,
                    scrollback_len: 0,
                    search_matches: &[],
                    search_current: &[],
                    sentence_spans: &[],
                    outlines,
                    selection: None,
                    selection_block: false,
                    surface_h: 600.0,
                    surface_w: TEST_SURFACE_W,
                    theme: &Theme::default(),
                    url_underline: false,
                },
                &mut Vec::new(),
            )
        };
        let plain = build(&[]);
        let outlined = build(&[(1, 2, 5)]);
        let added = &outlined[plain.len()..];
        assert_eq!(added.len(), 4 * 6, "four edges, two triangles each");
        let to_px = |v: &BgVertex| (v.x + 1.0) * TEST_SURFACE_W / 2.0;
        let xs: Vec<f32> = added.iter().map(to_px).collect();
        let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(*x), hi.max(*x)));
        assert_eq!((lo.round(), hi.round()), (20.0, 50.0), "around cells 2 to 4");
        let widest = added
            .chunks_exact(6)
            .map(|quad| {
                let xs: Vec<f32> = quad.iter().map(to_px).collect();
                let ys: Vec<f32> = quad.iter().map(|v| (1.0 - v.y) * 300.0).collect();
                let span = |vals: &[f32]| vals.iter().cloned().fold(f32::MIN, f32::max) - vals.iter().cloned().fold(f32::MAX, f32::min);
                span(&xs).min(span(&ys))
            })
            .fold(0.0, f32::max);
        assert!(widest <= OUTLINE_STROKE_WIDTH + 0.01, "each edge is a thin stroke, not a fill: {widest}");
    }

    #[test]
    fn test_build_bg_vertices_hollow_cursor_draws_an_outline_not_a_fill() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: true,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        // A filled cursor is one quad (6 vertices, see the sibling test above); a
        // hollow one is four thin outline strips (24 vertices) instead.
        assert_eq!(verts.len(), 24);
        let cursor_color = Theme::default().cursor_bg.as_linear();
        assert!(
            verts.iter().all(|v| (v.r, v.g, v.b) == cursor_color),
            "every vertex should belong to the outline strips"
        );
    }

    #[test]
    fn test_build_bg_vertices_unfocused_bar_cursor_draws_a_hollow_block() {
        // An unfocused window draws a Bar cursor as the same hollow cell
        // outline a Block gets: four strips in the cursor color, not the
        // bar's own thin strip.
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: true,
                cursor_shape: CursorShape::Bar,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(verts.len(), 24);
        let cursor_color = theme.cursor_bg.as_linear();
        assert!(
            verts.iter().all(|v| (v.r, v.g, v.b) == cursor_color),
            "every vertex should belong to the outline strips"
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
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[(0, 0), (0, 1)],
                search_current: &[(0, 1)],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let bits = |(r, g, b): (f32, f32, f32)| (r.to_bits(), g.to_bits(), b.to_bits());
        let drawn: std::collections::HashSet<(u32, u32, u32)> =
            verts.iter().map(|v| bits((v.r, v.g, v.b))).collect();
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
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 80.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: Some((0, 0, 0, 2)),
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let sel = theme.selection_bg.as_linear();
        // Six vertices per cell quad.
        let selected_cells = painted_cells(&verts, sel);
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
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Bar,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: Some((0, 0, 0, 2)),
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );
        let sel = theme.selection_bg.as_linear();
        assert_eq!(
            painted_cells(&verts, sel),
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
        let colored = winter_core::Style {
            background: GridColor::Rgb(RgbColor { r: 9, g: 90, b: 9 }),
            ..Default::default()
        };
        grid.set_style(colored);
        grid.print('x');
        grid.set_style(winter_core::Style::default());
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: Some((0, 0)),
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let want = theme.cursor_line_bg.as_linear();
        let banded = painted_cells(&verts, want);
        assert_eq!(
            banded, 3,
            "3 of the row's 4 cells are washed; the SGR-colored one keeps its own bg"
        );

        // No cursor line (Insert mode) means no wash at all.
        let plain = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: None,
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(
            plain.iter().filter(|v| (v.r, v.g, v.b) == want).count(),
            0,
            "no cursor line outside Normal/Visual"
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
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                room: (usize::MAX, usize::MAX),
                find_labels: &[],
                cursor_line: Some(grid.wrapped_row_span(1)),
                line_flash: None,
                offset_x: 0.0,
                overlay: None,
                presentation: PLAIN_SCREEN,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let want = theme.cursor_line_bg.as_linear();
        let banded = painted_cells(&verts, want);
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
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let want = theme.find_label_bg.as_linear();
        let boxes = painted_cells(&verts, want);
        assert_eq!(boxes, 1, "one label box drawn in find_label_bg");
        assert_ne!(want, QUICK_SELECT_BG, "and it differs from quick-select's");
    }

    #[test]
    fn test_build_bg_vertices_offset_produces_same_count() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let with_offset = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        let without_offset = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
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
    fn test_build_bg_vertices_selection_uses_absolute_row_not_viewport_row() {
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
            draw_braille_dots: true,
            focused: true,
            hide_cursor: false,
            hovered_link: 0,
            labels: None,
            room: (usize::MAX, usize::MAX),
            find_labels: &[],
            cursor_line: None,
            line_flash: None,
            offset_x: 0.0,
            overlay: None,
            presentation: PLAIN_SCREEN,
            offset_y: 0.0,
            scroll_offset: grid.scroll_offset(),
            scrollback_len: grid.scrollback_len(),
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
            outlines: &[],
            selection,
            selection_block: false,
            surface_h: 600.0,
            surface_w: 800.0,
            theme: &theme,
            url_underline: true,
        };
        let selected = build_bg_vertices_offset(
            &grid,
            params(Some((abs_row0, 0, abs_row0, 3))),
            &mut Vec::new(),
        );
        let unselected = build_bg_vertices_offset(&grid, params(None), &mut Vec::new());

        assert!(
            selected.len() > unselected.len(),
            "a selection covering the current top row's absolute address should add highlight quads"
        );
    }

    #[test]
    fn test_build_bg_vertices_url_underline_toggle() {
        let mut grid = Grid::new(4, 2);
        grid.set_active_link(Some("https://example.com"));
        grid.print('a');
        grid.set_active_link(None);

        let with_underline = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        let without_underline = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: false,
            },
            &mut Vec::new(),
        );
        assert_eq!(
            with_underline.len(),
            without_underline.len() + 6,
            "url_underline #false should drop the link cell's underline quad"
        );
    }

    #[test]
    fn test_build_bg_vertices_with_labels() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let labels: &[(usize, usize, char)] = &[(0, 0, 's'), (1, 2, 'd')];
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert!(
            verts.len() >= 12,
            "label cells add extra quads beyond cursor"
        );
    }

    #[test]
    fn test_build_bg_vertices_emits_quads_for_sentence_spans() {
        let mut grid = Grid::new(10, 2);
        grid.move_to(0, 0);
        for ch in "Hi! There!".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let spans = [(0, 0, 3, 0), (0, 4, 10, 1)];
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 100.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
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
                line_flash: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &spans,
                outlines: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: false,
            },
            &mut Vec::new(),
        );
        // 3 cells for sentence 1 + 6 cells for sentence 2, one run each, as
        // the two tones differ.
        let quads: Vec<&[BgVertex]> = verts.chunks_exact(6).collect();
        assert_eq!(quads.len(), 2);
        let cells: usize = quads
            .iter()
            .map(|q| painted_cells(q, (q[0].r, q[0].g, q[0].b)))
            .sum();
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
        let verts = build_bg_vertices_offset(&grid, plain_params(&theme), &mut Vec::new());
        assert_eq!(verts.len(), 3 * 6, "one quad per row");
        let color = grid_color_to_rgb(&GridColor::Indexed(4), &theme);
        assert_eq!(painted_cells(&verts, color), 3 * 40);
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
        let verts = build_bg_vertices_offset(&grid, plain_params(&theme), &mut Vec::new());
        let background = grid_color_to_rgb(&GridColor::Indexed(2), &theme);
        let first = &verts[..6];
        assert_eq!((first[0].r, first[0].g, first[0].b), background);
        assert!(verts.len() > 6, "the underline quads come after the run");
        assert!(verts[6..]
            .chunks_exact(6)
            .all(|quad| (quad[0].r, quad[0].g, quad[0].b) != background));
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
            draw_braille_dots: true,
            focused: true,
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
            line_flash: None,
            scroll_offset: 0,
            scrollback_len: 0,
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
            outlines: &[],
            selection: None,
            selection_block: false,
            surface_h: 600.0,
            surface_w: TEST_SURFACE_W,
            theme,
            url_underline: false,
        }
    }
}

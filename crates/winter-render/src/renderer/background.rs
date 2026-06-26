//! Background pass: cell, cursor, selection, and scrollbar quads, plus
//! the wgpu pipelines that draw them.

use super::chrome::DIM_FACTOR;
use super::colors::{grid_color_to_rgb, resolve_fg_linear};
use super::glyphs::{is_braille, push_braille_dots};
use super::PaneRect;
use crate::grid::{Color as GridColor, CursorShape, Grid};
use crate::theme::Theme;
use wgpu::{
    ColorTargetState, ColorWrites, Device, FragmentState, FrontFace, MultisampleState,
    PipelineLayoutDescriptor, PolygonMode, PrimitiveState, PrimitiveTopology, RenderPipeline,
    RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, TextureFormat, VertexAttribute,
    VertexBufferLayout, VertexFormat, VertexState, VertexStepMode,
};

// ========================================================================
// Constants
// ========================================================================

const BG_SHADER: &str = include_str!("../bg.wgsl");

const DOT_SHADER: &str = include_str!("../dot.wgsl");

const CURSOR_UNDERLINE_HEIGHT_RATIO: f32 = 0.2;

/// Width in pixels of a `Bar` (I-beam) cursor's vertical stroke.
const CURSOR_BAR_WIDTH: f32 = 2.0;

/// Offset in pixels applied to a `Bar` cursor's left position to keep it clear
/// of the glyph's ink at the left edge of a cell.
const CURSOR_BAR_OFFSET: f32 = -0.5;

/// Width in pixels of a hollow cursor's outline stroke (see [`cursor_outline_quads`]),
/// matching the window's own 1px [`Theme::window_border`] hairline.
pub(super) const CURSOR_HOLLOW_STROKE_WIDTH: f32 = 1.0;

pub(super) const BG_BUFFER_SIZE: u64 = 4 * 1024 * 1024;

pub(super) const DOT_BUFFER_SIZE: u64 = 8 * 1024 * 1024;

/// Opacity of the `/` search highlights over the cell's own background, so
/// highlighted text keeps its foreground color instead of being washed out by an
/// opaque block of tint. The current match is painted more strongly than the
/// others, which sit further back.
pub(super) const SEARCH_CURRENT_ALPHA: f32 = 0.8;

const SEARCH_MATCH_ALPHA: f32 = 0.6;

/// Opacities of the two alternating sentence-highlight tones, blended over the
/// cell's own background like the search tints. Kept low and close together so
/// the bands read as a subtle rhythm, not a zebra stripe; tone parity is the
/// signal, not brightness.
const SENTENCE_TINT_ALPHA_EVEN: f32 = 0.28;

const SENTENCE_TINT_ALPHA_ODD: f32 = 0.14;

// ========================================================================
// Implementation
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

// ========================================================================
// Background vertex construction
// ========================================================================

const QUICK_SELECT_BG: (f32, f32, f32) = (0.6, 0.45, 0.1);

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

/// Parameters for building background vertices for one pane.
pub(super) struct BgParams<'a> {
    pub(super) ch: f32,
    /// Draw the grid cursor in its unfocused form instead of filling
    /// `cursor_shape`. See [`PaneView::cursor_unfocused`].
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
    pub(super) find_labels: &'a [(usize, usize, char)],
    pub(super) offset_x: f32,
    pub(super) offset_y: f32,
    /// The inclusive span of viewport rows to paint with [`Theme::cursor_line_bg`]
    ///: the Normal-mode cursor's logical line, which covers every row of a
    /// soft-wrapped line. `None` outside Normal/Visual mode.
    pub(super) cursor_line: Option<(usize, usize)>,
    pub(super) scroll_offset: usize,
    pub(super) scrollback_len: usize,
    pub(super) search_current: &'a [(usize, usize)],
    pub(super) search_matches: &'a [(usize, usize)],
    /// Sentence-highlight bands `(row, start, end, tone)`; see
    /// [`PaneView::sentence_spans`].
    pub(super) sentence_spans: &'a [(usize, usize, usize, u8)],
    pub(super) selection: Option<(usize, usize, usize, usize)>,
    pub(super) selection_block: bool,
    pub(super) surface_h: f32,
    pub(super) surface_w: f32,
    pub(super) theme: &'a Theme,
    /// Underline cells carrying a link. See [`PaneView::url_underline`].
    pub(super) url_underline: bool,
}

/// The inclusive column span `[start, end]` of `sel_norm` that should be
/// highlighted on `abs_row` (an absolute row from `Grid::to_absolute_row`),
/// clamped to `content_end` (the row's last printable column) so the
/// selection hugs the text instead of spilling past the newline across the
/// trailing blank cells. `last_col` is the row's final column index, used for
/// rows that run to the line's end. Returns `None` when `abs_row` is outside
/// the selection or no content falls within the selected span.
fn selection_span_on_row(
    sel_norm: Option<(usize, usize, usize, usize)>,
    block: bool,
    abs_row: usize,
    last_col: usize,
    content_end: usize,
) -> Option<(usize, usize)> {
    let (sr1, sc1, sr2, sc2) = sel_norm?;
    if abs_row < sr1 || abs_row > sr2 {
        return None;
    }
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

pub(super) fn build_bg_vertices_offset(
    grid: &Grid,
    params: BgParams,
    dots: &mut Vec<DotVertex>,
) -> Vec<BgVertex> {
    let BgParams {
        ch,
        content_right,
        cursor_unfocused,
        cursor_shape,
        cw,
        dim,
        draw_braille_dots,
        focused,
        hide_cursor,
        hovered_link,
        labels,
        find_labels,
        offset_x,
        offset_y,
        cursor_line,
        scroll_offset,
        scrollback_len,
        search_current,
        search_matches,
        sentence_spans,
        selection,
        selection_block,
        surface_h,
        surface_w,
        theme,
        url_underline,
    } = params;
    let mut verts = Vec::new();
    let (cursor_row, cursor_col) = grid.cursor();
    let bg_lin = theme.background.as_linear();

    let sel_norm = selection.map(|(r1, c1, r2, c2)| {
        if selection_block {
            (r1.min(r2), c1, r1.max(r2), c2)
        } else if (r1, c1) > (r2, c2) {
            (r2, c2, r1, c1)
        } else {
            (r1, c1, r2, c2)
        }
    });

    let label_set: std::collections::HashSet<(usize, usize)> = labels
        .map(|l| l.iter().map(|&(r, c, _)| (r, c)).collect())
        .unwrap_or_default();
    let find_label_set: std::collections::HashSet<(usize, usize)> =
        find_labels.iter().map(|&(r, c, _)| (r, c)).collect();

    let match_set: std::collections::HashSet<(usize, usize)> =
        search_matches.iter().copied().collect();
    let current_match_set: std::collections::HashSet<(usize, usize)> =
        search_current.iter().copied().collect();

    // Sentence bands of this row (see `PaneView::sentence_spans`), found once
    // per row instead of per cell.
    let mut row_sentence_spans: Vec<(usize, usize, u8)> = Vec::new();

    for row in 0..grid.rows() {
        // The selection columns active on this row, clamped to the row's content
        // end so the highlight hugs the text and never extends past the newline
        // into the trailing blank cells. `selection` addresses rows absolutely
        // (see `Grid::to_absolute_row`), so the viewport row is converted before
        // comparing.
        let row_sel = selection_span_on_row(
            sel_norm,
            selection_block,
            grid.to_absolute_row(row),
            grid.cols().saturating_sub(1),
            grid.visible_line_end(row),
        );

        for col in 0..grid.cols() {
            let cell = grid.visible_cell(row, col);
            let is_cursor = !hide_cursor && row == cursor_row + scroll_offset && col == cursor_col;
            // The unfocused treatment is shape-aware: a Block cursor draws only
            // its outline (below, after this cell's own background is resolved
            // normally), so it never forces the full-cell fill; Bar and
            // Underline keep their filled strip, faded in the color resolution
            // below, because a strip that thin can't read as an outline.
            let cursor_outline =
                is_cursor && cursor_unfocused && cursor_shape == CursorShape::Block;
            let cursor_filled = is_cursor && !cursor_outline;
            let reversed = cell.is_some_and(|c| c.style.reversed);
            let bg = cell.map(|c| c.style.background).unwrap_or_default();
            let draw_bg = cursor_filled || reversed || !matches!(bg, GridColor::Default);

            let is_selected = row_sel.is_some_and(|(start, end)| col >= start && col <= end);

            let is_label = label_set.contains(&(row, col));
            let is_search_match = match_set.contains(&(row, col));
            let is_current_match = current_match_set.contains(&(row, col));
            let is_find_label = find_label_set.contains(&(row, col));
            // The cursor's own row reads as one band; cells carrying their own
            // background keep it, so program output isn't recolored.
            let plain_bg = matches!(bg, GridColor::Default) && !reversed;
            let is_cursor_line =
                cursor_line.is_some_and(|(first, last)| row >= first && row <= last) && plain_bg;

            // This row's sentence bands, gathered for the loop below. A band
            // tints only plain-background cells (like the cursor-line band) so
            // program output with its own colors is never recolored.
            row_sentence_spans.clear();
            if plain_bg {
                for &(srow, start, end, tone) in sentence_spans {
                    if srow == row {
                        row_sentence_spans.push((start, end, tone));
                    }
                }
            }
            let in_sentence_band = |col: usize| {
                row_sentence_spans
                    .iter()
                    .any(|&(s, e, _)| col >= s && col < e)
            };
            let sentence_tone = |col: usize| {
                row_sentence_spans
                    .iter()
                    .find(|&&(s, e, _)| col >= s && col < e)
                    .map(|&(_, _, t)| t)
            };

            if draw_bg
                || is_selected
                || is_label
                || is_find_label
                || is_search_match
                || is_cursor_line
                || in_sentence_band(col)
            {
                let (r, g, b) = cell_bg_color(
                    CellBgOverlays {
                        cursor_filled,
                        cursor_unfocused,
                        dim,
                        is_current_match,
                        is_cursor_line,
                        is_find_label,
                        is_label,
                        is_search_match,
                        is_selected,
                        reversed,
                        sentence_tone: sentence_tone(col),
                    },
                    bg,
                    cell.map(|c| c.style.foreground).unwrap_or_default(),
                    theme,
                    bg_lin,
                );

                let px0 = offset_x + col as f32 * cw;
                let py0 = offset_y + row as f32 * ch;
                let (px0, py0, px1, py1) = if cursor_filled {
                    cursor_quad(cursor_shape, px0, py0, cw, ch, grid.wrap_pending())
                } else {
                    (px0, py0, px0 + cw, py0 + ch)
                };

                verts.extend_from_slice(&quad_vertices(
                    px0,
                    py0,
                    px1,
                    py1,
                    (r, g, b),
                    surface_w,
                    surface_h,
                ));
            }

            // A hollow cursor draws its outline on top of whatever the cell above
            // already resolved, rather than folding into that fill, so the glyph
            // underneath keeps its normal color instead of the cursor's contrast fix.
            if cursor_outline {
                let x0 = offset_x + col as f32 * cw;
                let y0 = offset_y + row as f32 * ch;
                let color = {
                    let c = theme.cursor_bg.as_linear();
                    if dim {
                        lerp_to_bg(c, bg_lin)
                    } else {
                        c
                    }
                };
                for (qx0, qy0, qx1, qy1) in
                    cursor_outline_quads(x0, y0, cw, ch, CURSOR_HOLLOW_STROKE_WIDTH)
                {
                    verts.extend_from_slice(&quad_vertices(
                        qx0, qy0, qx1, qy1, color, surface_w, surface_h,
                    ));
                }
            }

            // Underline bar for SGR 4 (style.underline) or, when enabled, link cells
            // (auto-detected URL or OSC 8 hyperlink).
            if let Some(cell) = cell {
                if cell.style.underline || (cell.style.link != 0 && url_underline) {
                    let ul_color = if hovered_link != 0 && cell.style.link == hovered_link {
                        theme.cursor_bg.as_linear()
                    } else if let GridColor::Default = cell.style.foreground {
                        theme.foreground.as_linear()
                    } else {
                        grid_color_to_rgb(&cell.style.foreground, theme)
                    };
                    let (ur, ug, ub) = if dim {
                        lerp_to_bg(ul_color, bg_lin)
                    } else {
                        ul_color
                    };
                    let px0 = offset_x + col as f32 * cw;
                    let py_top = offset_y + row as f32 * ch + ch
                        - UNDERLINE_BOTTOM_OFFSET
                        - UNDERLINE_THICKNESS;
                    verts.extend_from_slice(&quad_vertices(
                        px0,
                        py_top,
                        px0 + cw,
                        py_top + UNDERLINE_THICKNESS,
                        (ur, ug, ub),
                        surface_w,
                        surface_h,
                    ));
                }
            }

            // When the font lacks braille, draw braille cells as dot quads (in
            // the cell's foreground color) so btop's graphs stay aligned and
            // legible instead of using a misaligned proportional fallback.
            if let Some(cell) = cell {
                if draw_braille_dots && is_braille(cell.ch) {
                    let fg = if let GridColor::Default = cell.style.foreground {
                        theme.foreground.as_linear()
                    } else {
                        grid_color_to_rgb(&cell.style.foreground, theme)
                    };
                    let color = if dim { lerp_to_bg(fg, bg_lin) } else { fg };
                    let cell_x = offset_x + col as f32 * cw;
                    let cell_y = offset_y + row as f32 * ch;
                    push_braille_dots(
                        dots,
                        cell.ch,
                        (cell_x, cell_y, cw, ch),
                        color,
                        (surface_w, surface_h),
                    );
                }
            }
        }
    }

    // Scrollbar: shown only when there is scrollback to navigate.
    verts.extend_from_slice(&scrollbar_quads(ScrollbarParams {
        bg_lin,
        cell_h: ch,
        content_right,
        dim,
        focused,
        offset_y,
        rows: grid.rows(),
        scroll_offset,
        scrollback_len,
        surface_h,
        surface_w,
        theme,
    }));

    verts
}

const SCROLLBAR_WIDTH: f32 = 2.0;
/// How far the scrollbar track is mixed from the pane background toward the
/// divider color, so the track reads as a recess without becoming a hard line.
const SCROLLBAR_TRACK_DIVIDER_MIX: f32 = 0.3;

/// Saturation multiplier for the scrollbar thumb (>1 pushes channels away from
/// gray for a more vivid color); applied in every pane.
const SCROLLBAR_SATURATE: f32 = 1.8;
/// How far the scrollbar thumb is lerped toward pure blue, applied in every pane.
const SCROLLBAR_BLUE: f32 = 0.18;
/// How far the scrollbar thumb is lerped toward white, lifting its brightness a
/// touch in every pane.
const SCROLLBAR_BRIGHTEN: f32 = 0.15;
/// How far an unfocused pane's scrollbar is lerped toward the background
/// (0 = unchanged, 1 = fully background). Stronger than the content dim so the
/// active pane's scrollbar clearly stands out.
const SCROLLBAR_DIM_FACTOR: f32 = 0.5;
const SCROLLBAR_MIN_THUMB: f32 = 6.0;

pub(super) const DIVIDER_THICKNESS: f32 = 1.0;
/// Height of the underline bar drawn for SGR 4 and OSC 8 hyperlink cells.
const UNDERLINE_THICKNESS: f32 = 1.0;
/// Distance from the bottom of a cell to the top of its underline bar.
const UNDERLINE_BOTTOM_OFFSET: f32 = 2.0;

pub(super) fn lerp_to_bg(c: (f32, f32, f32), bg: (f32, f32, f32)) -> (f32, f32, f32) {
    (
        c.0 + (bg.0 - c.0) * DIM_FACTOR,
        c.1 + (bg.1 - c.1) * DIM_FACTOR,
        c.2 + (bg.2 - c.2) * DIM_FACTOR,
    )
}

/// `over` composited on `under` at `alpha` (0 = fully `under`, 1 = fully `over`).
/// The background pass draws opaque quads, so translucent highlights are flattened
/// here rather than blended by the GPU: same result, no pipeline state to order.
fn blend_over(over: (f32, f32, f32), under: (f32, f32, f32), alpha: f32) -> (f32, f32, f32) {
    let a = alpha.clamp(0.0, 1.0);
    (
        under.0 + (over.0 - under.0) * a,
        under.1 + (over.1 - under.1) * a,
        under.2 + (over.2 - under.2) * a,
    )
}

pub(super) fn compute_divider(
    a: PaneRect,
    b: PaneRect,
    surface_w: f32,
    surface_h: f32,
    divider_color: (f32, f32, f32),
    width: f32,
) -> Option<[BgVertex; 6]> {
    // A vertical divider exists when one pane's right edge meets the other's left
    // edge and their y-ranges overlap (handles mixed-height layouts).
    //
    // Pane rects arrive pixel-snapped to integers. Adjacent panes share an
    // exact pixel edge when snapped corner-to-corner (see the app's
    // `layout_rect_to_pane`); the `<= 1.0` tolerance also covers a 1 px
    // sub-pixel snap mismatch so a divider is never dropped over a rounding
    // artifact.
    let a_right = a.x + a.width;
    let b_right = b.x + b.width;
    let a_bot = a.y + a.height;
    let b_bot = b.y + b.height;

    let (px0, py0, px1, py1) = if (a_right - b.x).abs() <= 1.0 || (b_right - a.x).abs() <= 1.0 {
        let y0 = a.y.max(b.y);
        let y1 = a_bot.min(b_bot);
        if y1 <= y0 {
            return None;
        }
        let x = if a.x < b.x { a_right } else { b_right };
        let x = x - width / 2.0;
        (x, y0, x + width, y1)
    } else if (a_bot - b.y).abs() <= 1.0 || (b_bot - a.y).abs() <= 1.0 {
        // A horizontal divider when one pane's bottom edge meets the other's top
        // and their x-ranges overlap (handles mixed-width layouts). The same 1 px
        // rounding tolerance as the vertical case above applies here.
        // Span the full overlapping width with no edge inset, so this divider
        // meets any crossing vertical divider (and the window's left/right
        // edges) cleanly, mirroring how the vertical divider runs the full
        // overlapping height above.
        let x0 = a.x.max(b.x);
        let x1 = a_right.min(b_right);
        if x1 <= x0 {
            return None;
        }
        let y = if a.y < b.y { a_bot } else { b_bot };
        let y = y - width / 2.0;
        (x0, y, x1, y + width)
    } else {
        return None;
    };

    let (r, g, b) = divider_color;
    let ndc_x0 = px0 * 2.0 / surface_w - 1.0;
    let ndc_y0 = 1.0 - py0 * 2.0 / surface_h;
    let ndc_x1 = px1 * 2.0 / surface_w - 1.0;
    let ndc_y1 = 1.0 - py1 * 2.0 / surface_h;

    Some([
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
    ])
}

// ========================================================================
// Background pipeline
// ========================================================================

pub(super) fn create_bg_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("winter bg shader"),
        source: ShaderSource::Wgsl(BG_SHADER.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("winter bg layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });

    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("winter bg pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(VertexBufferLayout {
                array_stride: std::mem::size_of::<BgVertex>() as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute {
                        offset: 0,
                        format: VertexFormat::Float32x2,
                        shader_location: 0,
                    },
                    VertexAttribute {
                        offset: 8,
                        format: VertexFormat::Float32x3,
                        shader_location: 1,
                    },
                ],
            })],
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::REPLACE),
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

/// Pipeline for procedural braille dots: like the bg pipeline but with a `uv`
/// attribute and alpha blending, so the fragment shader can draw anti-aliased
/// circles that blend over the cell background.
pub(super) fn create_dot_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("winter dot shader"),
        source: ShaderSource::Wgsl(DOT_SHADER.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("winter dot layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });

    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("winter dot pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(VertexBufferLayout {
                array_stride: std::mem::size_of::<DotVertex>() as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute {
                        offset: 0,
                        format: VertexFormat::Float32x2,
                        shader_location: 0,
                    },
                    VertexAttribute {
                        offset: 8,
                        format: VertexFormat::Float32x2,
                        shader_location: 1,
                    },
                    VertexAttribute {
                        offset: 16,
                        format: VertexFormat::Float32x3,
                        shader_location: 2,
                    },
                ],
            })],
        },
        fragment: Some(FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            compilation_options: Default::default(),
            targets: &[Some(ColorTargetState {
                format,
                blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

/// The pixel rect of a cursor of `shape` whose full cell starts at `(x, y)`
/// with size `(cw, ch)`. Block fills the cell; Bar covers a thin strip on the
/// left edge (or, with `wrap_pending`, protrudes just past the cell's right
/// edge instead); Underline covers a thin strip on the bottom edge.
///
/// `wrap_pending` is set when the grid has deferred a line wrap (VT100 "last
/// column" behavior): the cursor is parked on the cell holding the glyph just
/// printed rather than an empty cell after it. A monospace glyph's ink
/// typically spans close to both edges of its cell, so a `Bar` drawn at
/// *either* edge of that same cell still touches it; parking the stroke just
/// past the cell's right edge instead keeps it clear of the glyph entirely,
/// matching how the `Bar` already sits past the last character on every other
/// column. `Block` and `Underline` already cover the full cell width and need
/// no adjustment.
pub(super) fn cursor_quad(
    shape: CursorShape,
    x: f32,
    y: f32,
    cw: f32,
    ch: f32,
    wrap_pending: bool,
) -> (f32, f32, f32, f32) {
    let x1_full = x + cw;
    let y1_full = y + ch;
    match shape {
        CursorShape::Block => (x, y, x1_full, y1_full),
        CursorShape::Bar => {
            if wrap_pending {
                let x0 = x1_full;
                (x0, y, x0 + CURSOR_BAR_WIDTH, y1_full)
            } else {
                let x0 = x + CURSOR_BAR_OFFSET;
                (x0, y, x0 + CURSOR_BAR_WIDTH, y1_full)
            }
        }
        CursorShape::Underline => {
            let y0 = y1_full - ch * CURSOR_UNDERLINE_HEIGHT_RATIO;
            (x, y0, x1_full, y1_full)
        }
    }
}

/// The four pixel rects (top, bottom, left, right) that frame a cell of size
/// `(cw, ch)` starting at `(x, y)` with a `stroke`-wide outline, used to draw a
/// hollow cursor. Each rect is `(px0, py0, px1, py1)`, ready for
/// [`quad_vertices`]. The side rects run the cell's full height so the corners
/// aren't left with gaps.
pub(super) fn cursor_outline_quads(
    x: f32,
    y: f32,
    cw: f32,
    ch: f32,
    stroke: f32,
) -> [(f32, f32, f32, f32); 4] {
    let x1 = x + cw;
    let y1 = y + ch;
    [
        (x, y, x1, y + stroke),
        (x, y1 - stroke, x1, y1),
        (x, y, x + stroke, y1),
        (x1 - stroke, y, x1, y1),
    ]
}

/// Where and how to draw one pane's scrollbar.
struct ScrollbarParams<'a> {
    /// The pane's background in linear space, dimmed toward when unfocused.
    bg_lin: (f32, f32, f32),
    cell_h: f32,
    /// Right edge of the pane's content area, which the bar hugs.
    content_right: f32,
    dim: bool,
    focused: bool,
    offset_y: f32,
    /// Visible grid rows, i.e. the height of the scroll window.
    rows: usize,
    scroll_offset: usize,
    scrollback_len: usize,
    surface_h: f32,
    surface_w: f32,
    theme: &'a Theme,
}

/// The scrollbar's track and thumb quads for one pane. Empty when there is no
/// scrollback to navigate, which is when the bar is not shown at all.
fn scrollbar_quads(p: ScrollbarParams) -> Vec<BgVertex> {
    let mut verts = Vec::new();
    if p.scrollback_len == 0 {
        return verts;
    }
    let total = (p.rows + p.scrollback_len) as f32;
    let visible = p.rows as f32;
    let thumb_h_frac = (visible / total).max(0.0);
    let top_virtual = p.scrollback_len.saturating_sub(p.scroll_offset) as f32;
    let thumb_top_frac = (top_virtual / total).clamp(0.0, 1.0 - thumb_h_frac);

    // Hug the right content edge of the pane (snapped to whole pixels so
    // every pane's scrollbar renders at the same crisp width, instead of
    // landing on the last grid column with a variable sub-column gap).
    let sb_x1 = p.content_right.round();
    let sb_x0 = sb_x1 - SCROLLBAR_WIDTH;
    let track_y0 = p.offset_y;
    let track_y1 = p.offset_y + p.rows as f32 * p.cell_h;
    let track_h = track_y1 - track_y0;

    // Size the thumb first, then place it. A deep scrollback drives the
    // proportional height toward zero, so [`SCROLLBAR_MIN_THUMB`] keeps it
    // grabbable; on a track too short to hold even that, the thumb fills the
    // track instead of overflowing it.
    let thumb_h = (thumb_h_frac * track_h)
        .max(SCROLLBAR_MIN_THUMB)
        .min(track_h);
    // Keeping the thumb inside the track has to move it, not shrink it.
    // Clamping the bottom edge instead would cancel the minimum height exactly
    // when it matters most: a pane sitting at its live bottom (the resting
    // state) parks the thumb against the track's end, where a bottom clamp
    // collapses it back to a hairline.
    let thumb_y0 = (track_y0 + thumb_top_frac * track_h).min(track_y1 - thumb_h);
    let thumb_y1 = thumb_y0 + thumb_h;

    let (bg_r, bg_g, bg_b) = p.theme.background.as_linear();
    let (dv_r, dv_g, dv_b) = p.theme.divider.as_linear();
    let mix = |bg: f32, dv: f32| {
        bg * (1.0 - SCROLLBAR_TRACK_DIVIDER_MIX) + dv * SCROLLBAR_TRACK_DIVIDER_MIX
    };
    let mut track_color = (mix(bg_r, dv_r), mix(bg_g, dv_g), mix(bg_b, dv_b));
    // Make the thumb more saturated and bluer than the configured color, in
    // every pane: push the channels away from gray, then shift toward pure
    // blue.
    let mut thumb_color = {
        let c = p.theme.scrollbar.as_linear();
        let avg = (c.0 + c.1 + c.2) / 3.0;
        let sat = |x: f32| (avg + (x - avg) * SCROLLBAR_SATURATE).clamp(0.0, 1.0);
        let (r, g, b) = (sat(c.0), sat(c.1), sat(c.2));
        let (r, g, b) = (
            r + (0.0 - r) * SCROLLBAR_BLUE,
            g + (0.0 - g) * SCROLLBAR_BLUE,
            b + (1.0 - b) * SCROLLBAR_BLUE,
        );
        // Lift it a touch toward white so both panes read a bit brighter.
        (
            r + (1.0 - r) * SCROLLBAR_BRIGHTEN,
            g + (1.0 - g) * SCROLLBAR_BRIGHTEN,
            b + (1.0 - b) * SCROLLBAR_BRIGHTEN,
        )
    };
    // An unfocused pane's scrollbar is then dimmed toward the background so
    // the active pane's still stands out (independent of the content dim).
    if !p.focused || p.dim {
        let dim_to_bg = |c: (f32, f32, f32)| {
            (
                c.0 + (p.bg_lin.0 - c.0) * SCROLLBAR_DIM_FACTOR,
                c.1 + (p.bg_lin.1 - c.1) * SCROLLBAR_DIM_FACTOR,
                c.2 + (p.bg_lin.2 - c.2) * SCROLLBAR_DIM_FACTOR,
            )
        };
        track_color = dim_to_bg(track_color);
        thumb_color = dim_to_bg(thumb_color);
    }

    verts.extend_from_slice(&quad_vertices(
        sb_x0,
        track_y0,
        sb_x1,
        track_y1,
        track_color,
        p.surface_w,
        p.surface_h,
    ));
    verts.extend_from_slice(&quad_vertices(
        sb_x0,
        thumb_y0,
        sb_x1,
        thumb_y1,
        thumb_color,
        p.surface_w,
        p.surface_h,
    ));
    verts
}

/// Which overlays claim one cell this frame, in the order the background color
/// resolution consults them. At most one wins; the rest are ignored.
#[derive(Clone, Copy)]
struct CellBgOverlays {
    /// The cell holds a filled (non-outline) cursor.
    cursor_filled: bool,
    /// The window has lost focus, so a filled cursor fades toward the background.
    cursor_unfocused: bool,
    /// The whole pane is dimmed because it is not the focused one.
    dim: bool,
    is_current_match: bool,
    is_cursor_line: bool,
    is_find_label: bool,
    is_label: bool,
    is_search_match: bool,
    is_selected: bool,
    /// SGR 7: the cell's foreground and background are swapped.
    reversed: bool,
    /// The sentence band's alternating tone, when the cell sits inside one.
    sentence_tone: Option<u8>,
}

/// Resolve one cell's background color in linear space: the highest-priority
/// overlay claiming the cell wins, and the result fades toward the pane
/// background when the pane (or an unfocused cursor) is dimmed.
fn cell_bg_color(
    overlays: CellBgOverlays,
    bg: GridColor,
    fg: GridColor,
    theme: &Theme,
    bg_lin: (f32, f32, f32),
) -> (f32, f32, f32) {
    let c = if overlays.is_find_label {
        theme.find_label_bg.as_linear()
    } else if overlays.is_label {
        QUICK_SELECT_BG
    } else if overlays.is_selected {
        theme.selection_bg.as_linear()
    } else if overlays.is_search_match {
        // Search highlights are translucent: the tint blends into
        // whatever the cell's own background is, leaving the
        // glyph's foreground color legible on top of it (the text
        // pass draws matches in their normal color).
        let under = if overlays.cursor_filled {
            theme.cursor_bg.as_linear()
        } else {
            grid_color_to_rgb(&bg, theme)
        };
        let (tint, alpha) = if overlays.is_current_match {
            (theme.search_current_bg, SEARCH_CURRENT_ALPHA)
        } else {
            (theme.search_match_bg, SEARCH_MATCH_ALPHA)
        };
        blend_over(tint.as_linear(), under, alpha)
    } else if overlays.cursor_filled {
        theme.cursor_bg.as_linear()
    } else if overlays.reversed {
        // SGR 7: the highlight quad paints the resolved foreground,
        // swapped with the glyph color in the text pass below.
        resolve_fg_linear(fg, theme)
    } else if overlays.is_cursor_line {
        theme.cursor_line_bg.as_linear()
    } else if let Some(tone) = overlays.sentence_tone {
        // The sentence band shares the cursor-line hue at a
        // lower, tone-dependent opacity, so the two reading
        // aids read as one family.
        let alpha = if tone == 0 {
            SENTENCE_TINT_ALPHA_EVEN
        } else {
            SENTENCE_TINT_ALPHA_ODD
        };
        blend_over(
            theme.cursor_line_bg.as_linear(),
            grid_color_to_rgb(&bg, theme),
            alpha,
        )
    } else {
        grid_color_to_rgb(&bg, theme)
    };
    // An unfocused Bar/Underline cursor fades toward the background
    // like a dimmed pane's cursor does, signaling "not receiving
    // keystrokes" without an outline its thin shape can't carry.
    if overlays.dim || (overlays.cursor_filled && overlays.cursor_unfocused) {
        lerp_to_bg(c, bg_lin)
    } else {
        c
    }
}

/// Two triangles covering the pixel rect `[px0,px1] x [py0,py1]` in `color`,
/// converted to normalized device coordinates for the bg pipeline.
pub(super) fn quad_vertices(
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
    use super::*;
    use crate::RgbColor;

    /// Every vertex's y coordinate, in surface pixels rather than NDC.
    fn pixel_ys(verts: &[BgVertex], surface_h: f32) -> Vec<f32> {
        verts
            .iter()
            .map(|v| (1.0 - v.y) * surface_h / 2.0)
            .collect()
    }

    fn scrollbar_params(scrollback_len: usize, scroll_offset: usize) -> ScrollbarParams<'static> {
        static THEME: std::sync::OnceLock<Theme> = std::sync::OnceLock::new();
        ScrollbarParams {
            bg_lin: (0.0, 0.0, 0.0),
            cell_h: 10.0,
            content_right: 100.0,
            dim: false,
            focused: true,
            offset_y: 0.0,
            rows: 10,
            scroll_offset,
            scrollback_len,
            surface_h: 100.0,
            surface_w: 100.0,
            theme: THEME.get_or_init(Theme::dark),
        }
    }

    #[test]
    fn test_scrollbar_is_absent_without_scrollback() {
        // Nothing to navigate means no bar at all, not a full-height thumb.
        assert!(scrollbar_quads(scrollbar_params(0, 0)).is_empty());
    }

    #[test]
    fn test_scrollbar_thumb_stays_inside_its_track() {
        // A huge scrollback drives the proportional thumb toward zero height,
        // and `scroll_offset: 0` (the resting state, sitting at the live
        // bottom) parks it against the track's end. That is the case where a
        // bottom-edge clamp used to cancel the minimum height.
        let verts = scrollbar_quads(scrollbar_params(100_000, 0));
        let ys = pixel_ys(&verts, 100.0);
        let track_bottom = 10.0 * 10.0;
        let thumb_ys = &ys[6..];
        let (top, bottom) = thumb_ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(top >= -f32::EPSILON, "thumb top {top} is above the track");
        assert!(
            bottom <= track_bottom + f32::EPSILON,
            "thumb bottom {bottom} overruns the track at {track_bottom}"
        );
        assert!(
            bottom - top >= SCROLLBAR_MIN_THUMB - f32::EPSILON,
            "thumb {top}..{bottom} is thinner than the {SCROLLBAR_MIN_THUMB}px minimum"
        );
    }

    #[test]
    fn test_scrollbar_thumb_fills_a_track_shorter_than_the_minimum() {
        // A pane only a couple of rows tall has a track shorter than
        // `SCROLLBAR_MIN_THUMB`; the minimum must not push the thumb past the
        // track's end there.
        let mut params = scrollbar_params(500, 0);
        params.rows = 1;
        params.cell_h = 4.0;
        let verts = scrollbar_quads(params);
        let ys = pixel_ys(&verts, 100.0);
        let thumb_ys = &ys[6..];
        let (top, bottom) = thumb_ys
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(top >= -f32::EPSILON, "thumb top {top} is above the track");
        assert!(
            bottom <= 4.0 + f32::EPSILON,
            "thumb bottom {bottom} overruns the 4px track"
        );
    }

    #[test]
    fn test_find_label_outranks_selection_in_cell_background() {
        // Both overlays claim the cell; the `f`/`t` jump label has to win, or
        // the label the user is reading disappears into the selection wash.
        let theme = Theme::dark();
        let overlays = CellBgOverlays {
            cursor_filled: false,
            cursor_unfocused: false,
            dim: false,
            is_current_match: false,
            is_cursor_line: false,
            is_find_label: true,
            is_label: false,
            is_search_match: false,
            is_selected: true,
            reversed: false,
            sentence_tone: None,
        };
        let color = cell_bg_color(
            overlays,
            GridColor::Default,
            GridColor::Default,
            &theme,
            theme.background.as_linear(),
        );
        assert_eq!(color, theme.find_label_bg.as_linear());
    }

    #[test]
    fn test_bg_vertex_bytes_roundtrip() {
        let v = BgVertex {
            x: 1.0,
            y: -1.0,
            r: 0.5,
            g: 0.25,
            b: 0.0,
        };
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), 20);
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
    fn test_build_bg_vertices_cursor_only() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
    fn test_build_bg_vertices_unfocused_bar_cursor_fades_instead_of_outlining() {
        // A Bar cursor is too thin to read as an outline, so an unfocused window
        // keeps its filled strip but fades it toward the background: one quad,
        // in the dimmed color, not the four strips of a hollow cell outline and
        // not the full-strength cursor color.
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(verts.len(), 6);
        let faded = lerp_to_bg(theme.cursor_bg.as_linear(), theme.background.as_linear());
        assert!(
            verts.iter().all(|v| (v.r, v.g, v.b) == faded),
            "the unfocused bar should fade toward the background"
        );
    }
    #[test]
    fn test_blend_over_composites_at_the_given_alpha() {
        assert_eq!(
            blend_over((1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            blend_over((1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 1.0),
            (1.0, 1.0, 1.0)
        );
        let (r, g, b) = blend_over((1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.5);
        assert!((r - 0.5).abs() < 1e-6 && g.abs() < 1e-6 && (b - 0.5).abs() < 1e-6);
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[(0, 0), (0, 1)],
                search_current: &[(0, 1)],
                sentence_spans: &[],
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
        let selected_cells = verts.iter().filter(|v| (v.r, v.g, v.b) == sel).count() / 6;
        assert_eq!(selected_cells, 3, "cols 0, 1 and 2 should all be selected");
    }
    #[test]
    fn test_cursor_line_paints_the_whole_row_but_spares_colored_cells() {
        // Normal mode washes the cursor's row with `cursor_line_bg`, across the
        // full pane width: except cells carrying their own background, which keep
        // it so program output isn't recolored.
        let mut grid = Grid::new(4, 2);
        let colored = crate::grid::Style {
            background: GridColor::Rgb(RgbColor { r: 9, g: 90, b: 9 }),
            ..Default::default()
        };
        grid.set_style(colored);
        grid.print('x');
        grid.set_style(crate::grid::Style::default());
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
                find_labels: &[],
                cursor_line: Some((0, 0)),
                offset_x: 0.0,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
        let banded = verts.iter().filter(|v| (v.r, v.g, v.b) == want).count() / 6;
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
                find_labels: &[],
                cursor_line: None,
                offset_x: 0.0,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
                find_labels: &[],
                cursor_line: Some(grid.wrapped_row_span(1)),
                offset_x: 0.0,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
        let banded = verts.iter().filter(|v| (v.r, v.g, v.b) == want).count() / 6;
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
                find_labels,
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
        let boxes = verts.iter().filter(|v| (v.r, v.g, v.b) == want).count() / 6;
        assert_eq!(boxes, 1, "one label box drawn in find_label_bg");
        assert_ne!(want, QUICK_SELECT_BG, "and it differs from quick-select's");
    }
    #[test]
    fn test_cursor_quad_bar_wrap_pending_clears_the_cell() {
        // Regression: a Bar cursor parked on the last-typed glyph (deferred
        // wrap) must not draw inside that glyph's cell on either edge, since
        // monospace ink commonly spans close to both edges.
        let (x0, _, x1, _) = cursor_quad(CursorShape::Bar, 100.0, 0.0, 10.0, 20.0, true);
        assert!(
            x0 >= 110.0,
            "bar must start at or past the cell's right edge, got x0={x0}"
        );
        assert!(x1 > x0);

        let (x0_no_wrap, _, _, _) = cursor_quad(CursorShape::Bar, 100.0, 0.0, 10.0, 20.0, false);
        assert_eq!(
            x0_no_wrap,
            100.0 + CURSOR_BAR_OFFSET,
            "without a pending wrap the bar sits slightly left of the cell's left edge"
        );
    }
    #[test]
    fn test_cursor_outline_quads_frames_the_cell_on_all_four_sides() {
        let [top, bottom, left, right] = cursor_outline_quads(100.0, 50.0, 10.0, 20.0, 2.0);
        assert_eq!(top, (100.0, 50.0, 110.0, 52.0));
        assert_eq!(bottom, (100.0, 68.0, 110.0, 70.0));
        assert_eq!(left, (100.0, 50.0, 102.0, 70.0));
        assert_eq!(right, (108.0, 50.0, 110.0, 70.0));
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
                find_labels: &[],
                offset_x: 100.0,
                offset_y: 50.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
        assert_eq!(selection_span_on_row(sel, false, 0, 9, 4), Some((2, 4)));
        // Middle row fully selected: 0..=last_col clamped to its own content (6).
        assert_eq!(selection_span_on_row(sel, false, 1, 9, 6), Some((0, 6)));
        // Last row: ends at the anchor col (9) clamped to content end (3).
        assert_eq!(selection_span_on_row(sel, false, 2, 9, 3), Some((0, 3)));
    }
    #[test]
    fn test_selection_span_skips_rows_outside_and_empty_spans() {
        let sel = Some((1, 0, 1, 5));
        // Rows above/below the selection are never highlighted.
        assert_eq!(selection_span_on_row(sel, false, 0, 9, 9), None);
        assert_eq!(selection_span_on_row(sel, false, 2, 9, 9), None);
        // A selection that starts past the row's content highlights nothing.
        assert_eq!(
            selection_span_on_row(Some((0, 5, 0, 9)), false, 0, 9, 2),
            None
        );
        // No selection at all.
        assert_eq!(selection_span_on_row(None, false, 0, 9, 9), None);
    }
    #[test]
    fn test_selection_span_blockwise() {
        let sel = Some((0, 3, 2, 6));
        // Block selection on rows 0, 1, 2 between columns 3 and 6:
        assert_eq!(selection_span_on_row(sel, true, 0, 9, 8), Some((3, 6)));
        assert_eq!(selection_span_on_row(sel, true, 1, 9, 8), Some((3, 6)));
        assert_eq!(selection_span_on_row(sel, true, 2, 9, 5), Some((3, 5)));
        // Outside row:
        assert_eq!(selection_span_on_row(sel, true, 3, 9, 8), None);
    }
    #[test]
    fn test_build_bg_vertices_with_selection() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: Some((0, 0, 0, 1)),
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert!(verts.len() >= 12, "selection adds extra quads");
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
            find_labels: &[],
            cursor_line: None,
            offset_x: 0.0,
            offset_y: 0.0,
            scroll_offset: grid.scroll_offset(),
            scrollback_len: grid.scrollback_len(),
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
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
    // Two horizontally-split panes whose pixel-snapped rects are off by exactly
    // 1 px at the shared edge (top bottom = 411, bottom top = 410) must still
    // produce a horizontal divider. Reproduces the missing-divider bug caused by
    // `layout_rect_to_pane` rounding each field independently.
    #[test]
    fn test_horizontal_divider_drawn_with_one_pixel_rounding_gap() {
        let a = PaneRect {
            x: 0.0,
            y: 55.0,
            width: 800.0,
            height: 356.0,
        };
        let b = PaneRect {
            x: 0.0,
            y: 410.0,
            width: 800.0,
            height: 356.0,
        };
        assert_eq!(a.y + a.height, 411.0); // top pane bottom edge
        assert_eq!((a.y + a.height) - b.y, 1.0); // one-pixel rounding discrepancy
        let dv = compute_divider(a, b, 800.0, 800.0, (1.0, 1.0, 1.0), 1.0);
        assert!(dv.is_some(), "divider must be drawn despite the 1 px gap");
    }
    // Same check for a vertical split whose left/right shared edge disagrees by
    // 1 px after independent rounding.
    #[test]
    fn test_vertical_divider_drawn_with_one_pixel_rounding_gap() {
        let a = PaneRect {
            x: 0.0,
            y: 0.0,
            width: 401.0,
            height: 800.0,
        };
        let b = PaneRect {
            x: 400.0,
            y: 0.0,
            width: 400.0,
            height: 800.0,
        };
        assert_eq!((a.x + a.width) - b.x, 1.0); // one-pixel rounding discrepancy
        let dv = compute_divider(a, b, 800.0, 800.0, (1.0, 1.0, 1.0), 1.0);
        assert!(dv.is_some(), "divider must be drawn despite the 1 px gap");
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
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &spans,
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: false,
            },
            &mut Vec::new(),
        );
        // 3 cells for sentence 1 + 6 cells for sentence 2 = 9 tinted cells * 6 verts = 54 verts
        assert_eq!(verts.len(), (3 + 6) * 6);
    }
}

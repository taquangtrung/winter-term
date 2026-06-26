//! Background pass: cell, cursor, selection, and scrollbar quads, plus
//! the wgpu pipelines that draw them.

use super::*;

// ========================================================================
// Constants
// ========================================================================

pub(super) const BG_SHADER: &str = include_str!("../bg.wgsl");

pub(super) const DOT_SHADER: &str = include_str!("../dot.wgsl");

pub(super) const CURSOR_UNDERLINE_HEIGHT_RATIO: f32 = 0.2;

/// Width in pixels of a `Bar` (I-beam) cursor's vertical stroke.
pub(super) const CURSOR_BAR_WIDTH: f32 = 2.0;

/// Offset in pixels applied to a `Bar` cursor's left position to keep it clear
/// of the glyph's ink at the left edge of a cell.
pub(super) const CURSOR_BAR_OFFSET: f32 = -0.5;

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

pub(super) const SEARCH_MATCH_ALPHA: f32 = 0.6;

/// Opacities of the two alternating sentence-highlight tones, blended over the
/// cell's own background like the search tints. Kept low and close together so
/// the bands read as a subtle rhythm, not a zebra stripe; tone parity is the
/// signal, not brightness.
pub(super) const SENTENCE_TINT_ALPHA_EVEN: f32 = 0.28;

pub(super) const SENTENCE_TINT_ALPHA_ODD: f32 = 0.14;

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

pub(super) const QUICK_SELECT_BG: (f32, f32, f32) = (0.6, 0.45, 0.1);

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
    /// — the Normal-mode cursor's logical line, which covers every row of a
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
pub(super) fn selection_span_on_row(
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
                let (r, g, b) = {
                    let c = if is_find_label {
                        theme.find_label_bg.as_linear()
                    } else if is_label {
                        QUICK_SELECT_BG
                    } else if is_selected {
                        theme.selection_bg.as_linear()
                    } else if is_search_match {
                        // Search highlights are translucent: the tint blends into
                        // whatever the cell's own background is, leaving the
                        // glyph's foreground color legible on top of it (the text
                        // pass draws matches in their normal color).
                        let under = if cursor_filled {
                            theme.cursor_bg.as_linear()
                        } else {
                            grid_color_to_rgb(&bg, theme)
                        };
                        let (tint, alpha) = if is_current_match {
                            (theme.search_current_bg, SEARCH_CURRENT_ALPHA)
                        } else {
                            (theme.search_match_bg, SEARCH_MATCH_ALPHA)
                        };
                        blend_over(tint.as_linear(), under, alpha)
                    } else if cursor_filled {
                        theme.cursor_bg.as_linear()
                    } else if reversed {
                        // SGR 7: the highlight quad paints the resolved foreground,
                        // swapped with the glyph color in the text pass below.
                        resolve_fg_linear(
                            cell.map(|c| c.style.foreground).unwrap_or_default(),
                            theme,
                        )
                    } else if is_cursor_line {
                        theme.cursor_line_bg.as_linear()
                    } else if let Some(tone) = sentence_tone(col) {
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
                    if dim || (cursor_filled && cursor_unfocused) {
                        lerp_to_bg(c, bg_lin)
                    } else {
                        c
                    }
                };

                let px0 = offset_x + col as f32 * cw;
                let py0 = offset_y + row as f32 * ch;
                let (px0, py0, px1, py1) = if cursor_filled {
                    cursor_quad(cursor_shape, px0, py0, cw, ch, grid.wrap_pending())
                } else {
                    (px0, py0, px0 + cw, py0 + ch)
                };

                let ndc_x0 = px0 * 2.0 / surface_w - 1.0;
                let ndc_y0 = 1.0 - py0 * 2.0 / surface_h;
                let ndc_x1 = px1 * 2.0 / surface_w - 1.0;
                let ndc_y1 = 1.0 - py1 * 2.0 / surface_h;

                verts.push(BgVertex {
                    x: ndc_x0,
                    y: ndc_y0,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x1,
                    y: ndc_y0,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x0,
                    y: ndc_y1,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x1,
                    y: ndc_y0,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x1,
                    y: ndc_y1,
                    r,
                    g,
                    b,
                });
                verts.push(BgVertex {
                    x: ndc_x0,
                    y: ndc_y1,
                    r,
                    g,
                    b,
                });
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
                    let py_bot = py_top + UNDERLINE_THICKNESS;
                    let ux0 = px0 * 2.0 / surface_w - 1.0;
                    let ux1 = (px0 + cw) * 2.0 / surface_w - 1.0;
                    let uy0 = 1.0 - py_top * 2.0 / surface_h;
                    let uy1 = 1.0 - py_bot * 2.0 / surface_h;
                    verts.push(BgVertex {
                        x: ux0,
                        y: uy0,
                        r: ur,
                        g: ug,
                        b: ub,
                    });
                    verts.push(BgVertex {
                        x: ux1,
                        y: uy0,
                        r: ur,
                        g: ug,
                        b: ub,
                    });
                    verts.push(BgVertex {
                        x: ux0,
                        y: uy1,
                        r: ur,
                        g: ug,
                        b: ub,
                    });
                    verts.push(BgVertex {
                        x: ux1,
                        y: uy0,
                        r: ur,
                        g: ug,
                        b: ub,
                    });
                    verts.push(BgVertex {
                        x: ux1,
                        y: uy1,
                        r: ur,
                        g: ug,
                        b: ub,
                    });
                    verts.push(BgVertex {
                        x: ux0,
                        y: uy1,
                        r: ur,
                        g: ug,
                        b: ub,
                    });
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
    if scrollback_len > 0 {
        let total = (grid.rows() + scrollback_len) as f32;
        let visible = grid.rows() as f32;
        let thumb_h_frac = (visible / total).max(0.0);
        let top_virtual = scrollback_len.saturating_sub(scroll_offset) as f32;
        let thumb_top_frac = (top_virtual / total).clamp(0.0, 1.0 - thumb_h_frac);

        // Hug the right content edge of the pane (snapped to whole pixels so
        // every pane's scrollbar renders at the same crisp width, instead of
        // landing on the last grid column with a variable sub-column gap).
        let sb_x1 = content_right.round();
        let sb_x0 = sb_x1 - SCROLLBAR_WIDTH;
        let track_y0 = offset_y;
        let track_y1 = offset_y + grid.rows() as f32 * ch;
        let track_h = track_y1 - track_y0;

        let thumb_y0 = track_y0 + thumb_top_frac * track_h;
        let thumb_y1 = (thumb_y0 + thumb_h_frac * track_h)
            .max(thumb_y0 + SCROLLBAR_MIN_THUMB)
            .min(track_y1);

        let (bg_r, bg_g, bg_b) = theme.background.as_linear();
        let (dv_r, dv_g, dv_b) = theme.divider.as_linear();
        let mut track_color = (
            bg_r * 0.7 + dv_r * 0.3,
            bg_g * 0.7 + dv_g * 0.3,
            bg_b * 0.7 + dv_b * 0.3,
        );
        // Make the thumb more saturated and bluer than the configured color, in
        // every pane: push the channels away from gray, then shift toward pure
        // blue.
        let mut thumb_color = {
            let c = theme.scrollbar.as_linear();
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
        if !focused || dim {
            let dim_to_bg = |c: (f32, f32, f32)| {
                (
                    c.0 + (bg_lin.0 - c.0) * SCROLLBAR_DIM_FACTOR,
                    c.1 + (bg_lin.1 - c.1) * SCROLLBAR_DIM_FACTOR,
                    c.2 + (bg_lin.2 - c.2) * SCROLLBAR_DIM_FACTOR,
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
            surface_w,
            surface_h,
        ));
        verts.extend_from_slice(&quad_vertices(
            sb_x0,
            thumb_y0,
            sb_x1,
            thumb_y1,
            thumb_color,
            surface_w,
            surface_h,
        ));
    }

    verts
}

pub(super) const SCROLLBAR_WIDTH: f32 = 2.0;
/// Saturation multiplier for the scrollbar thumb (>1 pushes channels away from
/// gray for a more vivid color); applied in every pane.
pub(super) const SCROLLBAR_SATURATE: f32 = 1.8;
/// How far the scrollbar thumb is lerped toward pure blue, applied in every pane.
pub(super) const SCROLLBAR_BLUE: f32 = 0.18;
/// How far the scrollbar thumb is lerped toward white, lifting its brightness a
/// touch in every pane.
pub(super) const SCROLLBAR_BRIGHTEN: f32 = 0.15;
/// How far an unfocused pane's scrollbar is lerped toward the background
/// (0 = unchanged, 1 = fully background). Stronger than the content dim so the
/// active pane's scrollbar clearly stands out.
pub(super) const SCROLLBAR_DIM_FACTOR: f32 = 0.5;
pub(super) const SCROLLBAR_MIN_THUMB: f32 = 6.0;

pub(super) const DIVIDER_THICKNESS: f32 = 1.0;
/// Height of the underline bar drawn for SGR 4 and OSC 8 hyperlink cells.
pub(super) const UNDERLINE_THICKNESS: f32 = 1.0;
/// Distance from the bottom of a cell to the top of its underline bar.
pub(super) const UNDERLINE_BOTTOM_OFFSET: f32 = 2.0;

pub(super) fn lerp_to_bg(c: (f32, f32, f32), bg: (f32, f32, f32)) -> (f32, f32, f32) {
    (
        c.0 + (bg.0 - c.0) * DIM_FACTOR,
        c.1 + (bg.1 - c.1) * DIM_FACTOR,
        c.2 + (bg.2 - c.2) * DIM_FACTOR,
    )
}

/// `over` composited on `under` at `alpha` (0 = fully `under`, 1 = fully `over`).
/// The background pass draws opaque quads, so translucent highlights are flattened
/// here rather than blended by the GPU — same result, no pipeline state to order.
pub(super) fn blend_over(
    over: (f32, f32, f32),
    under: (f32, f32, f32),
    alpha: f32,
) -> (f32, f32, f32) {
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
        // edges) cleanly — mirroring how the vertical divider runs the full
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
            buffers: &[VertexBufferLayout {
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
            }],
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
            buffers: &[VertexBufferLayout {
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
            }],
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

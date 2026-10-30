//! Cell decorations drawn in the background pass: underlines in each SGR 4
//! style, the overline, and the strikethrough bar. The curly underline (a
//! problem's squiggle, SGR `4:3`) is a smooth anti-aliased wave drawn
//! through the dot pipeline; the rest are bars.

use std::f32::consts::TAU;

use super::super::colors::grid_color_to_rgb;
use super::cell_color::lerp_to_bg;
use super::{quad_vertices, BgVertex, DotVertex};
use crate::theme::Theme;
use winter_core::{Cell, Color as GridColor, UnderlineStyle};

// ========================================================================
// Constants
// ========================================================================

/// Height of the underline bar drawn for SGR 4 and OSC 8 hyperlink cells.
const UNDERLINE_THICKNESS: f32 = 1.0;
/// Distance from the bottom of a cell to the top of its underline bar.
const UNDERLINE_BOTTOM_OFFSET: f32 = 2.0;
/// Height of the strikethrough bar drawn for SGR 9.
const STRIKETHROUGH_THICKNESS: f32 = 1.0;
/// Distance from the top of a cell to the top of its overline bar (SGR 53).
const OVERLINE_TOP_OFFSET: f32 = 1.0;
/// Space between the two bars of a double underline.
const DOUBLE_UNDERLINE_GAP: f32 = 1.0;
/// Horizontal period of a dotted underline: one dot, one gap.
const DOTTED_UNDERLINE_PERIOD: f32 = 2.0 * UNDERLINE_THICKNESS;
/// Dashes a dashed underline draws per cell, and the share of each dash's
/// slot the dash fills.
const DASHES_PER_CELL: usize = 2;
const DASH_FILL: f32 = 0.6;
/// A curly underline's stroke, as a share of the cell's height, and the
/// thinnest it is drawn, in pixels: thick enough to read at any size.
const UNDERCURL_THICKNESS_SHARE: f32 = 0.06;
const UNDERCURL_MIN_THICKNESS: f32 = 1.2;
/// How far the wave swings either way, as a share of the cell's height, and
/// the least it swings, in pixels. One full period per cell, so a run of
/// cells is one unbroken wave.
const UNDERCURL_AMPLITUDE_SHARE: f32 = 0.07;
const UNDERCURL_MIN_AMPLITUDE: f32 = 1.25;
/// The most pixels between the points the wave is drawn through, so it
/// reads as a curve rather than a run of straight pieces.
const UNDERCURL_SAMPLE_PX: f32 = 1.0;
/// The fewest points per cell, however narrow the cell.
const UNDERCURL_MIN_SAMPLES: usize = 8;
/// The room past the stroke's edge its quads take, in pixels, for the dot
/// shader to feather the edge across.
const UNDERCURL_FEATHER: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// The pane-wide state a cell's decorations are drawn under.
pub(super) struct DecorationStyle<'a> {
    /// The pane background in linear space, faded toward when `dim`.
    pub(super) bg_lin: (f32, f32, f32),
    pub(super) dim: bool,
    /// The link under the pointer (0 for none), whose underline takes the
    /// cursor color.
    pub(super) hovered_link: u16,
    pub(super) surface: (f32, f32),
    pub(super) theme: &'a Theme,
    /// Underline cells carrying a link even without SGR 4.
    pub(super) url_underline: bool,
}

// ========================================================================
// Decorations
// ========================================================================

/// Append `cell`'s underline (SGR 4 and its `4:n` styles, or a link cell's
/// plain underline when enabled), overline (SGR 53), and strikethrough
/// (SGR 9) quads for the cell whose top-left is `(x0, row_top)`.
pub(super) fn push_decorations(
    out: &mut Vec<BgVertex>,
    dots: &mut Vec<DotVertex>,
    cell: &Cell,
    (x0, row_top, cw, ch): (f32, f32, f32, f32),
    style: &DecorationStyle,
) {
    let fade = |color: (f32, f32, f32)| match style.dim {
        true => lerp_to_bg(color, style.bg_lin),
        false => color,
    };
    let (surface_w, surface_h) = style.surface;
    let underline = match cell.style.underline {
        UnderlineStyle::None if cell.style.link != 0 && style.url_underline => {
            UnderlineStyle::Single
        }
        underline => underline,
    };
    if underline.is_drawn() || cell.style.overline {
        // Both draw in the underline color (SGR 58) if one is set.
        let line_color = match style.hovered_link != 0 && cell.style.link == style.hovered_link {
            true => style.theme.cursor_bg.as_linear(),
            false => decoration_color(cell, style.theme),
        };
        let color = fade(line_color);
        let bottom = row_top + ch - UNDERLINE_BOTTOM_OFFSET;
        if underline == UnderlineStyle::Curly {
            push_undercurl(dots, undercurl_points(x0, bottom, cw, ch), ch, color, style.surface);
        }
        let mut quads = underline_quads(underline, x0, bottom, cw);
        if cell.style.overline {
            let y0 = row_top + OVERLINE_TOP_OFFSET;
            quads.push((x0, y0, x0 + cw, y0 + UNDERLINE_THICKNESS));
        }
        for (qx0, qy0, qx1, qy1) in quads {
            out.extend_from_slice(&quad_vertices(
                qx0, qy0, qx1, qy1, color, surface_w, surface_h,
            ));
        }
    }
    if cell.style.strikethrough {
        // Through the middle of the cell, in the glyph's foreground color.
        let strike_color = match cell.style.foreground {
            GridColor::Default => style.theme.foreground.as_linear(),
            foreground => grid_color_to_rgb(&foreground, style.theme),
        };
        let py_top = row_top + (ch - STRIKETHROUGH_THICKNESS) / 2.0;
        out.extend_from_slice(&quad_vertices(
            x0,
            py_top,
            x0 + cw,
            py_top + STRIKETHROUGH_THICKNESS,
            fade(strike_color),
            surface_w,
            surface_h,
        ));
    }
}

/// The color a cell's underline and overline draw in: the underline color
/// it set (SGR 58), else its glyph's foreground.
fn decoration_color(cell: &Cell, theme: &Theme) -> (f32, f32, f32) {
    let color = match cell.style.underline_color {
        GridColor::Default => cell.style.foreground,
        explicit => explicit,
    };
    match color {
        GridColor::Default => theme.foreground.as_linear(),
        explicit => grid_color_to_rgb(&explicit, theme),
    }
}

/// The bars `style` draws under one cell, as `(x0, y0, x1, y1)` quads, the
/// cell `width` wide starting at `x0`, with the lowest bar's bottom at
/// `bottom`. Empty for [`UnderlineStyle::None`], and for
/// [`UnderlineStyle::Curly`], which is a wave ([`push_undercurl`]).
fn underline_quads(
    style: UnderlineStyle,
    x0: f32,
    bottom: f32,
    width: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let x1 = x0 + width;
    let top = bottom - UNDERLINE_THICKNESS;
    match style {
        UnderlineStyle::None | UnderlineStyle::Curly => Vec::new(),
        UnderlineStyle::Single => vec![(x0, top, x1, bottom)],
        UnderlineStyle::Double => {
            let upper_bottom = top - DOUBLE_UNDERLINE_GAP;
            vec![
                (x0, top, x1, bottom),
                (x0, upper_bottom - UNDERLINE_THICKNESS, x1, upper_bottom),
            ]
        }
        UnderlineStyle::Dotted => {
            let dots = (width / DOTTED_UNDERLINE_PERIOD).ceil() as usize;
            (0..dots)
                .map(|i| x0 + i as f32 * DOTTED_UNDERLINE_PERIOD)
                .map(|x| (x, top, (x + UNDERLINE_THICKNESS).min(x1), bottom))
                .collect()
        }
        UnderlineStyle::Dashed => {
            let slot = width / DASHES_PER_CELL as f32;
            (0..DASHES_PER_CELL)
                .map(|i| x0 + i as f32 * slot)
                .map(|x| (x, top, x + slot * DASH_FILL, bottom))
                .collect()
        }
    }
}

/// The stroke of a curly underline in a cell `ch` tall: how thick it is and
/// how far it swings either way, in pixels.
fn undercurl_size(ch: f32) -> (f32, f32) {
    let thickness = (ch * UNDERCURL_THICKNESS_SHARE).max(UNDERCURL_MIN_THICKNESS);
    let amplitude = (ch * UNDERCURL_AMPLITUDE_SHARE).max(UNDERCURL_MIN_AMPLITUDE);
    (thickness, amplitude)
}

/// The points a curly underline's wave runs through under a cell `width`
/// wide starting at `x0`, `ch` tall, with the stroke's lowest edge at
/// `bottom`: each point and the unit normal to the wave there. One full
/// period across the cell, level at both ends, so it joins its neighbours'.
fn undercurl_points(x0: f32, bottom: f32, width: f32, ch: f32) -> Vec<((f32, f32), (f32, f32))> {
    let (thickness, amplitude) = undercurl_size(ch);
    let centre = bottom - thickness / 2.0 - amplitude;
    let samples = ((width / UNDERCURL_SAMPLE_PX).ceil() as usize).max(UNDERCURL_MIN_SAMPLES);
    (0..=samples)
        .map(|i| {
            let t = i as f32 / samples as f32;
            let y = centre + amplitude * (t * TAU).sin();
            let slope = amplitude * TAU / width * (t * TAU).cos();
            let length = (1.0 + slope * slope).sqrt();
            ((x0 + t * width, y), (-slope / length, 1.0 / length))
        })
        .collect()
}

/// Append the anti-aliased stroke of a curly underline through `points`
/// (see [`undercurl_points`]) in a cell `ch` tall, in `color`: one strip of
/// quads, each sharing its edges with the next along the normals, so the
/// wave has no gaps and no doubled-up joins. The dot shader feathers the
/// stroke's edges, `v` running across it from -1 to 1 and `u` held at 0.
fn push_undercurl(
    dots: &mut Vec<DotVertex>,
    points: Vec<((f32, f32), (f32, f32))>,
    ch: f32,
    (r, g, b): (f32, f32, f32),
    (surface_w, surface_h): (f32, f32),
) {
    let half = undercurl_size(ch).0 / 2.0 + UNDERCURL_FEATHER;
    let vertex = |((x, y), (nx, ny)): ((f32, f32), (f32, f32)), side: f32| DotVertex {
        x: (x + nx * half * side) * 2.0 / surface_w - 1.0,
        y: 1.0 - (y + ny * half * side) * 2.0 / surface_h,
        u: 0.0,
        v: side,
        r,
        g,
        b,
    };
    for pair in points.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let corners = [vertex(a, -1.0), vertex(a, 1.0), vertex(b, 1.0), vertex(b, -1.0)];
        dots.extend_from_slice(&[corners[0], corners[1], corners[2], corners[0], corners[2], corners[3]]);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_core::Style;

    #[test]
    fn test_each_underline_style_stays_inside_its_cell() {
        let (x0, bottom, width) = (10.0, 20.0, 9.0);
        for style in [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
        ] {
            let quads = underline_quads(style, x0, bottom, width);
            assert!(!quads.is_empty(), "{style:?} draws");
            for (qx0, qy0, qx1, qy1) in quads {
                assert!(qx0 >= x0 && qx1 <= x0 + width + f32::EPSILON, "{style:?} x");
                assert!(qy0 < qy1 && qy1 <= bottom, "{style:?} y");
            }
        }
        assert!(underline_quads(UnderlineStyle::None, x0, bottom, width).is_empty());
    }

    #[test]
    fn test_double_underline_draws_two_separate_bars() {
        let quads = underline_quads(UnderlineStyle::Double, 0.0, 20.0, 9.0);
        assert_eq!(quads.len(), 2);
        assert!(
            quads[1].3 < quads[0].1,
            "the upper bar ends above the lower"
        );
    }

    #[test]
    fn test_the_undercurl_is_a_smooth_wave_inside_its_cell() {
        let (x0, width, ch) = (10.0, 9.0, 20.0);
        let bottom = ch - UNDERLINE_BOTTOM_OFFSET;
        let points = undercurl_points(x0, bottom, width, ch);
        let ys: Vec<f32> = points.iter().map(|((_, y), _)| *y).collect();
        let (low, high) = ys.iter().fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(high - low > 2.0, "a wave, not a flat bar: {ys:?}");
        let (thickness, _) = undercurl_size(ch);
        assert!(high + thickness / 2.0 <= bottom + 1e-3, "the stroke stays above the cell's foot");
        assert_eq!(points.first().map(|((x, _), _)| *x), Some(x0));
        assert!((points.last().map_or(0.0, |((x, _), _)| *x) - (x0 + width)).abs() < 1e-3, "spans the cell");
        // Drawn through points close enough together to read as a curve.
        for pair in points.windows(2) {
            let ((ax, ay), _) = pair[0];
            let ((bx, by), _) = pair[1];
            assert!((bx - ax).hypot(by - ay) <= 1.5, "a step of {} px", (bx - ax).hypot(by - ay));
        }
    }

    #[test]
    fn test_the_undercurls_of_neighbouring_cells_join() {
        let (width, ch) = (9.0, 20.0);
        let left = undercurl_points(0.0, 18.0, width, ch);
        let right = undercurl_points(width, 18.0, width, ch);
        let (end, end_normal) = *left.last().expect("points");
        let (start, start_normal) = right[0];
        assert!((end.1 - start.1).abs() < 1e-3, "level where they meet");
        assert!((end_normal.0 - start_normal.0).abs() < 1e-3, "and turning the same way");
    }

    #[test]
    fn test_the_undercurl_is_one_feathered_strip_of_quads() {
        let mut dots = Vec::new();
        let points = undercurl_points(0.0, 18.0, 9.0, 20.0);
        let segments = points.len() - 1;
        push_undercurl(&mut dots, points, 20.0, (1.0, 0.0, 0.0), (100.0, 100.0));
        assert_eq!(dots.len(), segments * 6, "two triangles a segment");
        assert!(dots.iter().all(|dot| dot.u == 0.0 && dot.v.abs() == 1.0), "feathered across, not along");
    }

    #[test]
    fn test_decoration_color_prefers_the_underline_color() {
        let theme = Theme::default();
        let red = GridColor::Indexed(1);
        let cell = Cell {
            style: Style {
                foreground: GridColor::Indexed(2),
                underline_color: red,
                ..Default::default()
            },
            ..Cell::default()
        };
        assert_eq!(
            decoration_color(&cell, &theme),
            grid_color_to_rgb(&red, &theme)
        );
        let plain = Cell::default();
        assert_eq!(
            decoration_color(&plain, &theme),
            theme.foreground.as_linear()
        );
    }
}

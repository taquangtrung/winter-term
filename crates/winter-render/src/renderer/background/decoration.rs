//! Cell decorations drawn in the background pass: underlines in each SGR 4
//! style, the overline, and the strikethrough bar.

use std::f32::consts::TAU;

use super::super::colors::grid_color_to_rgb;
use super::cell_color::lerp_to_bg;
use super::{quad_vertices, BgVertex};
use crate::grid::{Cell, Color as GridColor, UnderlineStyle};
use crate::theme::Theme;

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
/// Steps a curly underline's wave is drawn in per cell (one full period per
/// cell, so neighbouring cells join), and how far it swings either way.
const CURLY_UNDERLINE_STEPS: usize = 8;
const CURLY_UNDERLINE_AMPLITUDE: f32 = 1.5;

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
        let mut quads = underline_quads(underline, x0, row_top + ch - UNDERLINE_BOTTOM_OFFSET, cw);
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
/// `bottom`. Empty for [`UnderlineStyle::None`].
fn underline_quads(
    style: UnderlineStyle,
    x0: f32,
    bottom: f32,
    width: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let x1 = x0 + width;
    let top = bottom - UNDERLINE_THICKNESS;
    match style {
        UnderlineStyle::None => Vec::new(),
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
        UnderlineStyle::Curly => {
            let step = width / CURLY_UNDERLINE_STEPS as f32;
            let centre = top - CURLY_UNDERLINE_AMPLITUDE;
            (0..CURLY_UNDERLINE_STEPS)
                .map(|i| {
                    let phase = (i as f32 + 0.5) / CURLY_UNDERLINE_STEPS as f32;
                    let y = centre + CURLY_UNDERLINE_AMPLITUDE * (phase * TAU).sin();
                    let x = x0 + i as f32 * step;
                    (x, y, x + step, y + UNDERLINE_THICKNESS)
                })
                .collect()
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Style;

    #[test]
    fn test_each_underline_style_stays_inside_its_cell() {
        let (x0, bottom, width) = (10.0, 20.0, 9.0);
        for style in [
            UnderlineStyle::Single,
            UnderlineStyle::Double,
            UnderlineStyle::Dotted,
            UnderlineStyle::Dashed,
            UnderlineStyle::Curly,
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
    fn test_curly_underline_rises_and_falls_across_the_cell() {
        let quads = underline_quads(UnderlineStyle::Curly, 0.0, 20.0, 8.0);
        let tops: Vec<f32> = quads.iter().map(|q| q.1).collect();
        let (low, high) = tops
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &y| (lo.min(y), hi.max(y)));
        assert!(high - low > 1.0, "a wave, not a flat bar: {tops:?}");
        assert_eq!(quads.last().map(|q| q.2), Some(8.0), "spans the whole cell");
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

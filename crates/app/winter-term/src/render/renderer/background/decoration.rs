//! Cell decorations drawn in the background pass: underlines in each SGR 4
//! style, the overline, the strikethrough bar, and an editor's indentation
//! guides, all as bars. The curly underline (a problem's squiggle, SGR
//! `4:3`) misty draws with the cell.

use super::super::colors::grid_color_to_rgb;
use super::cell_color::lerp_to_bg;
use super::quad_fill;
use misty::SolidFill;
use winter_emulator::{Cell, Color as GridColor, UnderlineStyle};
use winter_model::paint::INDENT_GUIDE;
use winter_theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// Height of the underline bar drawn for SGR 4 and OSC 8 hyperlink cells.
const UNDERLINE_THICKNESS: f32 = 1.0;
/// Distance from the bottom of a cell to the top of its underline bar.
const UNDERLINE_BOTTOM_OFFSET: f32 = 2.0;
/// Height of the strikethrough bar drawn for SGR 9.
const STRIKETHROUGH_THICKNESS: f32 = 1.0;
/// Width of an indentation guide's line, as thin as an underline, and how
/// far it stands in from its cell's left edge, so it clears the glyph of
/// the column before it.
const INDENT_GUIDE_THICKNESS: f32 = 1.0;
const INDENT_GUIDE_INSET: f32 = 1.0;
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
    out: &mut Vec<SolidFill>,
    cell: &Cell,
    (x0, row_top, cw, ch): (f32, f32, f32, f32),
    style: &DecorationStyle,
) {
    let fade = |color: (f32, f32, f32)| match style.dim {
        true => lerp_to_bg(color, style.bg_lin),
        false => color,
    };
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
        let mut quads = underline_quads(underline, x0, bottom, cw);
        if cell.style.overline {
            let y0 = row_top + OVERLINE_TOP_OFFSET;
            quads.push((x0, y0, x0 + cw, y0 + UNDERLINE_THICKNESS));
        }
        for (qx0, qy0, qx1, qy1) in quads {
            out.push(quad_fill(qx0, qy0, qx1, qy1, color));
        }
    }
    // The glyph's foreground color, which a strikethrough and a guide take.
    let text_color = match cell.style.foreground {
        GridColor::Default => style.theme.foreground.as_linear(),
        foreground => grid_color_to_rgb(&foreground, style.theme),
    };
    // An indentation guide: a hairline just in from the cell's left edge,
    // where the indented text of its level starts, the full row tall, so the rows of a
    // block join into one line.
    if cell.ch == INDENT_GUIDE {
        let px_left = (x0 + INDENT_GUIDE_INSET).round();
        out.push(quad_fill(
            px_left,
            row_top,
            px_left + INDENT_GUIDE_THICKNESS,
            row_top + ch,
            fade(text_color),
        ));
    }
    if cell.style.strikethrough {
        // Through the middle of the cell, in the glyph's foreground color.
        let strike_color = text_color;
        let py_top = row_top + (ch - STRIKETHROUGH_THICKNESS) / 2.0;
        out.push(quad_fill(
            x0,
            py_top,
            x0 + cw,
            py_top + STRIKETHROUGH_THICKNESS,
            fade(strike_color),
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
/// [`UnderlineStyle::Curly`], which misty draws.
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_emulator::Style;

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
    fn test_an_indentation_guide_is_a_hairline_the_cell_s_full_height() {
        // A line one pixel wide down the cell's left edge, whatever the
        // font's box lines are, and the row's full height so a block's rows
        // join.
        let theme = Theme::default();
        let style = DecorationStyle {
            bg_lin: (0.0, 0.0, 0.0),
            dim: false,
            hovered_link: 0,
            theme: &theme,
            url_underline: false,
        };
        let cell = Cell {
            ch: INDENT_GUIDE,
            ..Cell::default()
        };
        let mut out = Vec::new();
        push_decorations(&mut out, &cell, (10.0, 20.0, 9.0, 18.0), &style);
        assert_eq!(out.len(), 1, "one rectangle");
        let rect = out[0].rect;
        let (left, width) = (rect.x, rect.width);
        assert!(
            (left - (10.0 + INDENT_GUIDE_INSET)).abs() < 1e-3,
            "just in from the cell's left edge, got {left}"
        );
        let (top, bottom) = (rect.y, rect.y + rect.height);
        assert!(
            (width - INDENT_GUIDE_THICKNESS).abs() < 1e-3,
            "one pixel wide, got {width}"
        );
        assert!(
            (top - 20.0).abs() < 1e-3 && (bottom - 38.0).abs() < 1e-3,
            "the row's height"
        );

        let plain = Cell::default();
        let mut none = Vec::new();
        push_decorations(&mut none, &plain, (10.0, 20.0, 9.0, 18.0), &style);
        assert!(none.is_empty(), "a plain cell draws no line");
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

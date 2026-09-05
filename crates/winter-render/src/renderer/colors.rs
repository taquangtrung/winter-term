//! Color resolution: grid colors, the indexed palettes, and blending
//! math shared by every pass.

use crate::grid::{Cell, Color as GridColor, CursorShape, RgbColor};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// Minimum luminance gap (summed 0-255 channels, so out of 765) below which a
/// glyph on a highlighted background is synthetically bolded. The sRGB surface
/// blends glyph coverage in linear light (see `ColorMode::Accurate` below) so
/// partially-covered edge pixels lean toward whichever of foreground/background
/// is brighter: that keeps light text on a dark background crisp, but the same
/// math thins dark text on a bright highlight (a selection, search match, or an
/// explicit SGR background). Bolding compensates by covering more of each cell.
const DARK_ON_LIGHT_BOLD_MARGIN: i32 = 90;

/// Minimum RGB distance (see `color_distance`, max ~441) between a glyph's color
/// and the block cursor's fill before the glyph is considered lost in the cursor
/// and repainted in a contrasting color.
const CURSOR_CONTRAST_MIN: f32 = 96.0;

// ========================================================================
// Implementation
// ========================================================================

/// Linearly interpolate two colors channel-wise; `t` clamps to `0.0..=1.0`,
/// where `0.0` returns `a` and `1.0` returns `b`.
pub(super) fn mix_rgb(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    let lerp = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Rgb {
        r: lerp(a.r, b.r),
        g: lerp(a.g, b.g),
        b: lerp(a.b, b.b),
    }
}

/// Decode one 8-bit sRGB channel to a linear `0.0..=1.0` value. Used for the
/// surface clear, which an sRGB target interprets as linear.
pub(super) fn srgb_to_linear_f64(channel: u8) -> f64 {
    let c = channel as f64 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

/// Whether `fg` on `bg` is dark-on-light enough to need the synthetic-bold
/// compensation described at `DARK_ON_LIGHT_BOLD_MARGIN`.
pub(super) fn needs_dark_on_light_bold(fg: (u8, u8, u8), bg: (u8, u8, u8)) -> bool {
    let sum = |c: (u8, u8, u8)| c.0 as i32 + c.1 as i32 + c.2 as i32;
    sum(fg) + DARK_ON_LIGHT_BOLD_MARGIN < sum(bg)
}

/// Straight-line RGB distance between two colors, 0 (identical) to ~441 (black
/// to white). Crude but stable, and enough to tell "invisible on this fill" from
/// "readable on this fill".
fn color_distance(a: (u8, u8, u8), b: (u8, u8, u8)) -> f32 {
    let d = |x: u8, y: u8| x as f32 - y as f32;
    (d(a.0, b.0).powi(2) + d(a.1, b.1).powi(2) + d(a.2, b.2).powi(2)).sqrt()
}

/// The viewport cell an opaque block cursor covers, whose glyph may need
/// repainting to stay visible (see [`cursor_contrast_fg`]).
///
/// It follows whichever cursor is actually drawn: the Normal/Visual traversal
/// cursor when the pane has one, otherwise the shell's own cursor, which in
/// Insert mode moves with every keystroke, so the fix tracks the caret as the
/// user types. `Bar` and `Underline` cursors sit clear of the glyph's ink and
/// need no help, an unfocused Block cursor's hollow outline doesn't cover its
/// cell's fill either, and a hidden cursor covers nothing.
pub(super) fn block_cursor_cell(
    shape: CursorShape,
    unfocused: bool,
    nav_cursor: Option<(usize, usize)>,
    cursor_visible: bool,
    grid_cursor: (usize, usize),
    scroll_offset: usize,
) -> Option<(usize, usize)> {
    if shape != CursorShape::Block || unfocused {
        return None;
    }
    nav_cursor.or_else(|| {
        cursor_visible.then(|| {
            let (row, col) = grid_cursor;
            (row + scroll_offset, col)
        })
    })
}

/// A replacement glyph color for the cell sitting under an opaque block cursor,
/// when the cell's own foreground `fg` is too close to [`Theme::cursor_bg`] for
/// the character to be visible against it (a blue prompt under a blue cursor, for
/// instance). Returns [`Theme::cursor_fg`], or plain black/white when that color
/// is itself too close to the cursor fill. `None` when `fg` already contrasts and
/// should be left alone.
pub(super) fn cursor_contrast_fg(fg: (u8, u8, u8), theme: &Theme) -> Option<(u8, u8, u8)> {
    let cursor = (theme.cursor_bg.r, theme.cursor_bg.g, theme.cursor_bg.b);
    if color_distance(fg, cursor) >= CURSOR_CONTRAST_MIN {
        return None;
    }
    let cursor_fg = (theme.cursor_fg.r, theme.cursor_fg.g, theme.cursor_fg.b);
    if color_distance(cursor_fg, cursor) >= CURSOR_CONTRAST_MIN {
        return Some(cursor_fg);
    }
    let bright = cursor.0 as u32 + cursor.1 as u32 + cursor.2 as u32 > 382;
    Some(if bright { (0, 0, 0) } else { (255, 255, 255) })
}

pub(super) fn grid_color_to_rgb(color: &GridColor, theme: &Theme) -> (f32, f32, f32) {
    match color {
        GridColor::Default => theme.background.as_linear(),
        GridColor::Indexed(i) => {
            let (r, g, b) = theme_indexed_color(theme, *i);
            (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
        }
        GridColor::Rgb(RgbColor { r, g, b }) => {
            (*r as f32 / 255.0, *g as f32 / 255.0, *b as f32 / 255.0)
        }
    }
}

/// Resolve a foreground `GridColor` to linear RGB. Unlike [`grid_color_to_rgb`],
/// `GridColor::Default` resolves to the theme's foreground, not its background,
/// since a reversed cell (SGR 7) paints its background quad with the resolved
/// foreground color.
pub(super) fn resolve_fg_linear(color: GridColor, theme: &Theme) -> (f32, f32, f32) {
    match color {
        GridColor::Default => theme.foreground.as_linear(),
        GridColor::Indexed(i) => {
            let (r, g, b) = theme_indexed_color(theme, i);
            (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
        }
        GridColor::Rgb(RgbColor { r, g, b }) => {
            (r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
        }
    }
}

/// Resolve a 256-color palette index to RGB using the theme: ANSI 0-15 and any
/// custom indexed overrides come from the theme; the rest fall back to the
/// standard xterm 256-color cube and grey ramp.
pub(super) fn theme_indexed_color(theme: &Theme, index: u8) -> (u8, u8, u8) {
    if (index as usize) < 16 {
        return theme.ansi_color(index);
    }
    if let Some(rgb) = theme.indexed_color(index) {
        return rgb;
    }
    xterm_256_to_rgb(index)
}

/// A cell's on-screen glyph color: its own foreground, or its background if
/// reverse video (SGR 7) is set. Used for a selected cell so the selection
/// highlight changes only the background behind the glyph, never the
/// glyph's own color — the same swap a reversed cell gets unselected.
/// `None` (no cell at this position) falls back to the theme's foreground.
pub(super) fn cell_text_fg(cell: Option<&Cell>, theme: &Theme) -> (u8, u8, u8) {
    let fg_rgb = match cell.map(|c| c.style.foreground) {
        Some(GridColor::Rgb(rgb)) => (rgb.r, rgb.g, rgb.b),
        Some(GridColor::Indexed(idx)) => theme_indexed_color(theme, idx),
        _ => (theme.foreground.r, theme.foreground.g, theme.foreground.b),
    };
    let bg_rgb = match cell.map(|c| c.style.background) {
        Some(GridColor::Rgb(rgb)) => (rgb.r, rgb.g, rgb.b),
        Some(GridColor::Indexed(idx)) => theme_indexed_color(theme, idx),
        _ => (theme.background.r, theme.background.g, theme.background.b),
    };
    if cell.is_some_and(|c| c.style.reversed) {
        bg_rgb
    } else {
        fg_rgb
    }
}

fn xterm_256_to_rgb(index: u8) -> (u8, u8, u8) {
    if index < 16 {
        return ANSI_COLORS[index as usize];
    }
    if index < 232 {
        let i = index - 16;
        let b_val = i % 6;
        let g_val = (i / 6) % 6;
        let r_val = (i / 36) % 6;
        return (
            if r_val > 0 { 55 + 40 * r_val } else { 0 },
            if g_val > 0 { 55 + 40 * g_val } else { 0 },
            if b_val > 0 { 55 + 40 * b_val } else { 0 },
        );
    }
    let grey = 8 + 10 * (index - 232);
    (grey, grey, grey)
}

const ANSI_COLORS: [(u8, u8, u8); 16] = [
    (0, 0, 0),
    (128, 0, 0),
    (0, 128, 0),
    (128, 128, 0),
    (0, 0, 128),
    (128, 0, 128),
    (0, 128, 128),
    (192, 192, 192),
    (128, 128, 128),
    (255, 0, 0),
    (0, 255, 0),
    (255, 255, 0),
    (0, 0, 255),
    (255, 0, 255),
    (0, 255, 255),
    (255, 255, 255),
];

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Style;

    #[test]
    fn test_xterm_256_first_16_are_ansi() {
        assert_eq!(xterm_256_to_rgb(0), (0, 0, 0));
        assert_eq!(xterm_256_to_rgb(1), (128, 0, 0));
        assert_eq!(xterm_256_to_rgb(7), (192, 192, 192));
        assert_eq!(xterm_256_to_rgb(15), (255, 255, 255));
    }
    #[test]
    fn test_xterm_256_cube() {
        let (r, g, b) = xterm_256_to_rgb(16 + 36 + 6 + 1);
        assert!(r > 0);
        assert!(g > 0);
        assert!(b > 0);
    }
    #[test]
    fn test_xterm_256_grey_ramp() {
        let (r, g, b) = xterm_256_to_rgb(232);
        assert_eq!(r, g);
        assert_eq!(g, b);
        assert!(r >= 8);
    }
    #[test]
    fn test_grid_color_default() {
        let theme = Theme::default();
        let (r, g, b) = grid_color_to_rgb(&GridColor::Default, &theme);
        assert_eq!((r, g, b), theme.background.as_linear());
    }
    #[test]
    fn test_cell_text_fg_keeps_the_cells_own_rgb_foreground() {
        // Regression: selected text used to be repainted with a fixed
        // `theme.selection_fg`, so a cell's own ANSI/RGB color (e.g.
        // syntax-highlighted output) changed when selected. Selection
        // should only add a background highlight.
        let theme = Theme::default();
        let cell = Cell {
            style: Style {
                foreground: GridColor::Rgb(RgbColor {
                    r: 10,
                    g: 200,
                    b: 30,
                }),
                ..Style::default()
            },
            ..Cell::default()
        };
        assert_eq!(cell_text_fg(Some(&cell), &theme), (10, 200, 30));
    }
    #[test]
    fn test_cell_text_fg_swaps_on_reverse_video() {
        let theme = Theme::default();
        let cell = Cell {
            style: Style {
                foreground: GridColor::Rgb(RgbColor {
                    r: 10,
                    g: 200,
                    b: 30,
                }),
                background: GridColor::Rgb(RgbColor { r: 5, g: 5, b: 5 }),
                reversed: true,
                ..Style::default()
            },
            ..Cell::default()
        };
        assert_eq!(cell_text_fg(Some(&cell), &theme), (5, 5, 5));
    }
    #[test]
    fn test_cell_text_fg_falls_back_to_theme_foreground_for_default_color() {
        let theme = Theme::default();
        let cell = Cell::default();
        assert_eq!(
            cell_text_fg(Some(&cell), &theme),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b)
        );
        assert_eq!(
            cell_text_fg(None, &theme),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b)
        );
    }
    #[test]
    fn test_resolve_fg_linear_default() {
        let theme = Theme::default();
        let (r, g, b) = resolve_fg_linear(GridColor::Default, &theme);
        assert_eq!((r, g, b), theme.foreground.as_linear());
    }
    #[test]
    fn test_grid_color_rgb() {
        let theme = Theme::default();
        let (r, g, b) = grid_color_to_rgb(
            &GridColor::Rgb(RgbColor {
                r: 255,
                g: 128,
                b: 0,
            }),
            &theme,
        );
        assert!((r - 1.0).abs() < 0.01);
        assert!((g - 0.5).abs() < 0.01);
        assert!((b - 0.0).abs() < 0.01);
    }
    #[test]
    fn test_needs_dark_on_light_bold_flags_black_on_bright_highlight() {
        // A dark badge foreground (e.g. an SGR-colored log-level tag) on a
        // bright background is exactly the case the sRGB linear-light blend
        // thins out, so it must be flagged for the synthetic-bold fix.
        assert!(needs_dark_on_light_bold((0, 0, 0), (93, 162, 235)));
        // The default theme's selection colors are also dark-on-light.
        let theme = Theme::default();
        assert!(needs_dark_on_light_bold(
            (
                theme.selection_fg.r,
                theme.selection_fg.g,
                theme.selection_fg.b
            ),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        ));
    }
    #[test]
    fn test_needs_dark_on_light_bold_ignores_light_on_dark_and_close_luminance() {
        // Light text on a dark background is the case linear-light blending
        // already renders crisp; it must not also get bolded.
        assert!(!needs_dark_on_light_bold((255, 255, 255), (0, 0, 0)));
        // Two colors within the margin are too close in luminance for the
        // thinning artifact to be visible, so no compensation is needed.
        assert!(!needs_dark_on_light_bold((100, 100, 100), (110, 110, 110)));
    }
    #[test]
    fn test_cursor_contrast_fg_repaints_only_text_that_clashes_with_the_cursor() {
        let theme = Theme::dark();
        let cursor = (theme.cursor_bg.r, theme.cursor_bg.g, theme.cursor_bg.b);

        // Text in (or near) the cursor's own color would be invisible inside the
        // block, so it's repainted in something that stands out.
        let fixed = cursor_contrast_fg(cursor, &theme).expect("identical color needs a repaint");
        assert!(color_distance(fixed, cursor) >= CURSOR_CONTRAST_MIN);

        // Ordinary text already contrasts and keeps its own color.
        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
        assert_eq!(cursor_contrast_fg(fg, &theme), None);
    }
    #[test]
    fn test_cursor_contrast_fg_falls_back_when_cursor_fg_also_clashes() {
        // A theme whose cursor_fg is as lost in the cursor fill as the text is
        // still has to produce something readable.
        let mut theme = Theme::dark();
        theme.cursor_bg = Rgb::new(0, 0, 0);
        theme.cursor_fg = Rgb::new(4, 4, 4);
        assert_eq!(cursor_contrast_fg((0, 0, 0), &theme), Some((255, 255, 255)));

        theme.cursor_bg = Rgb::new(255, 255, 255);
        theme.cursor_fg = Rgb::new(250, 250, 250);
        assert_eq!(cursor_contrast_fg((255, 255, 255), &theme), Some((0, 0, 0)));
    }
    #[test]
    fn test_block_cursor_cell_follows_the_shell_cursor_without_a_nav_cursor() {
        // Insert mode has no nav cursor, so the covered cell tracks the shell's
        // own caret (shifted by the scroll offset, as the drawn quad is).
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, None, true, (3, 7), 0),
            Some((3, 7))
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, None, true, (3, 7), 5),
            Some((8, 7))
        );
        // Normal/Visual: the traversal cursor wins.
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, Some((1, 2)), true, (3, 7), 0),
            Some((1, 2))
        );
    }
    #[test]
    fn test_block_cursor_cell_is_none_for_thin_hidden_or_hollow_cursors() {
        // A bar or underline leaves the glyph's ink visible, a hidden cursor
        // covers nothing, and a hollow cursor's fill doesn't cover the glyph
        // either, so none of these need a repaint.
        assert_eq!(
            block_cursor_cell(CursorShape::Bar, false, None, true, (0, 0), 0),
            None
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Underline, false, None, true, (0, 0), 0),
            None
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, None, false, (0, 0), 0),
            None
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Block, true, None, true, (0, 0), 0),
            None
        );
    }
}

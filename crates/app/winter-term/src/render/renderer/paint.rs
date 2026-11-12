//! Panes as plain shapes, for a host that draws them itself: each pane's
//! cells (the glyph, its color and face, resolved from every overlay that
//! claims it) and the rectangles drawn over their backgrounds (the cell
//! backgrounds and highlights, the cursor, decorations, the scrollbar), and
//! the separators between panes. The same rules the GPU passes use decide
//! every color; only the output differs.

use super::background::BgVertex;
use super::colors::{
    block_cursor_cell, cell_text_fg, cursor_contrast_fg, mix_rgb, needs_dark_on_light_bold,
    FAINT_FACTOR,
};
use super::display::shown_cell;
use super::frame::{
    pane_cell_extent, strip_panes, window_border_verts, FrameLayers, SurfaceSize, CURRENT_MATCH_FG,
    SEARCH_MATCH_FG,
};
use super::{GpuRenderer, PaneView};
use winter_emulator::{Cell, CellWidth, Color as GridColor, UnderlineStyle};
use winter_model::paint::INDENT_GUIDE;
use winter_theme::DIM_FACTOR;
use winter_theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// The quick-select labels' glyph color, amber.
const QUICK_LABEL_FG: (u8, u8, u8) = (255, 200, 50);

/// How many vertices one rectangle of the background pass takes.
const QUAD_VERTICES: usize = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// A rectangle in one color, in window pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaintFill {
    /// The color, sRGB.
    pub color: (u8, u8, u8),
    /// Height in pixels.
    pub height: f32,
    /// Width in pixels.
    pub width: f32,
    /// Left edge.
    pub x: f32,
    /// Top edge.
    pub y: f32,
}

/// One cell as it is drawn: its glyph, color, and face. Its background is
/// among the pane's fills.
#[derive(Clone, Debug, PartialEq)]
pub struct PaintCell {
    /// Whether the glyph is bold.
    pub bold: bool,
    /// The glyph, a space for none.
    pub ch: char,
    /// The curly underline's color, when the cell has one (the other lines
    /// are among the fills).
    pub curly: Option<(u8, u8, u8)>,
    /// The glyph's color, sRGB.
    pub fg: (u8, u8, u8),
    /// Whether the glyph is italic.
    pub italic: bool,
    /// Combining characters drawn with `ch` as one glyph.
    pub tail: Option<Box<str>>,
    /// How many columns the cell takes.
    pub width: PaintWidth,
}

/// How many columns a [`PaintCell`] takes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaintWidth {
    /// One column.
    Single,
    /// The right half of a wide cell, drawing nothing.
    Spacer,
    /// Two columns.
    Wide,
}

/// One pane, or one strip of its gutter, as plain shapes.
#[derive(Clone, Debug, PartialEq)]
pub struct PanePaint {
    /// Whether any cell blinks (SGR 5), which the host needs a blink timer
    /// running for.
    pub blinks: bool,
    /// Every cell, row by row, `cols` to a row.
    pub cells: Vec<PaintCell>,
    /// How many columns.
    pub cols: usize,
    /// What is drawn over the cells' backgrounds and under their glyphs, in
    /// order: cell backgrounds and highlights, the cursor, lines, the
    /// scrollbar.
    pub fills: Vec<PaintFill>,
    /// The top left corner of cell 0, 0, in window pixels.
    pub origin: (f32, f32),
    /// How many rows.
    pub rows: usize,
}

// ========================================================================
// GpuRenderer: panes as shapes
// ========================================================================

impl GpuRenderer {
    /// `pane` and each strip of its gutter as plain shapes.
    pub fn paint_pane(&self, pane: &PaneView) -> Vec<PanePaint> {
        let mut painted = vec![self.paint_one(pane)];
        painted.extend(strip_panes(pane).iter().map(|strip| self.paint_one(strip)));
        painted
    }

    /// The separators between `panes`, and the window's own outline when
    /// the system draws none.
    pub fn paint_separators(&self, panes: &[PaneView]) -> Vec<PaintFill> {
        let surface = self.surface_size();
        let mut layers = FrameLayers::default();
        self.push_separator_verts(panes, surface, &mut layers);
        let mut fills = fills_of(&layers.over_image_verts, surface);
        let border = window_border_verts(
            surface,
            self.theme.window_border.as_linear(),
            self.chrome.decorated,
        );
        fills.extend(fills_of(&border, surface));
        fills
    }

    /// One pane, without its gutter's strips.
    fn paint_one(&self, pane: &PaneView) -> PanePaint {
        let (cw, ch) = (self.fonts.cell_width, self.fonts.cell_height);
        let rect = pane.rect.grid_area(ch);
        let extent = pane_cell_extent(rect, (cw, ch), self.pane_pad());
        let surface = self.surface_size();
        let mut layers = FrameLayers::default();
        self.push_pane_background(pane, surface, &mut layers);
        let blinks = (0..pane.grid.rows().min(extent.rows)).any(|row| {
            (0..pane.grid.cols().min(extent.cols)).any(|col| {
                pane.grid
                    .visible_cell(row, col)
                    .is_some_and(|cell| cell.style.blink)
            })
        });
        PanePaint {
            blinks,
            cells: self.paint_cells(pane, (extent.cols, extent.rows)),
            cols: extent.cols,
            fills: fills_of(&layers.bg_verts, surface),
            origin: (rect.x + self.pane_pad(), rect.y),
            rows: extent.rows,
        }
    }

    /// The window's size in pixels.
    fn surface_size(&self) -> SurfaceSize {
        SurfaceSize {
            height: self.size.height as f32,
            width: self.size.width as f32,
        }
    }

    /// Each cell of `pane` within `room` columns and rows, as the text pass
    /// would draw it: the label in place of the glyph where one stands, the
    /// selection's and the search matches' glyph colors, the block cursor's
    /// contrast fix, reverse video, faint text, rainbow brackets, and the
    /// whole pane faded when dimmed.
    fn paint_cells(
        &self,
        pane: &PaneView,
        (room_cols, room_rows): (usize, usize),
    ) -> Vec<PaintCell> {
        let grid = pane.grid;
        let theme = &self.theme;
        let presentation = pane.presentation();
        let cols = grid.cols().min(room_cols);
        let rows = grid.rows().min(room_rows);
        let selection = pane
            .selection
            .map(|(r1, c1, r2, c2)| match (r1, c1) > (r2, c2) {
                true => (r2, c2, r1, c1),
                false => (r1, c1, r2, c2),
            });
        let block_cursor = block_cursor_cell(
            pane.cursor_shape,
            pane.cursor_unfocused,
            pane.nav_cursor.filter(|_| pane.nav_cursor_visible),
            pane.cursor_visible,
            grid.cursor(),
            pane.scroll_offset,
        );
        let find_label_fg = (
            theme.find_label_fg.r,
            theme.find_label_fg.g,
            theme.find_label_fg.b,
        );
        let label_at = |row: usize, col: usize| -> Option<(char, (u8, u8, u8))> {
            pane.find_labels
                .iter()
                .find(|&&(r, c, _)| (r, c) == (row, col))
                .map(|&(_, _, ch)| (ch, find_label_fg))
                .or_else(|| {
                    pane.labels?
                        .iter()
                        .find(|&&(r, c, _)| (r, c) == (row, col))
                        .map(|&(_, _, ch)| (ch, QUICK_LABEL_FG))
                })
        };
        let dim = |(r, g, b): (u8, u8, u8)| dimmed((r, g, b), theme.background, pane.dim);
        let mut cells = Vec::with_capacity(cols * rows);
        for row in 0..rows {
            let abs_row = grid.to_absolute_row(row);
            for col in 0..cols {
                let shown = shown_cell(grid, pane.overlay, row, col, presentation);
                let cell = shown.as_deref();
                let width = match cell.map(|cell| cell.width) {
                    Some(CellWidth::Spacer) => PaintWidth::Spacer,
                    Some(CellWidth::Wide) => PaintWidth::Wide,
                    _ => PaintWidth::Single,
                };
                let cursor_fg = block_cursor
                    .filter(|&at| at == (row, col))
                    .and_then(|_| cursor_contrast_fg(cell_text_fg(cell, theme), theme));
                let label = label_at(row, col);
                let raw = label
                    .map(|(ch, _)| ch)
                    .unwrap_or_else(|| cell.map_or(' ', |cell| cell.ch));
                // An indentation guide is a line among the fills, never a glyph.
                let ch = match raw == INDENT_GUIDE && label.is_none() {
                    true => ' ',
                    false => raw,
                };
                let tail = cell
                    .filter(|_| label.is_none())
                    .and_then(|cell| cell.tail.clone());
                let selected = selection.is_some_and(|(sr1, sc1, sr2, sc2)| {
                    (abs_row, col) >= (sr1, sc1) && (abs_row, col) <= (sr2, sc2)
                });
                let italic = cell.is_some_and(|cell| cell.style.italic);
                let (fg, bold) = if let Some((_, color)) = label {
                    (color, true)
                } else if selected {
                    let (r, g, b) = cursor_fg.unwrap_or_else(|| cell_text_fg(cell, theme));
                    let readable = Rgb::new(r, g, b).readable_on(theme.selection_bg);
                    (
                        dim((readable.r, readable.g, readable.b)),
                        cell.is_some_and(|cell| cell.style.bold),
                    )
                } else if pane.search_current.contains(&(row, col)) {
                    (dim(CURRENT_MATCH_FG), true)
                } else if pane.search_matches.contains(&(row, col)) {
                    (dim(SEARCH_MATCH_FG), true)
                } else if let Some(cell) = cell {
                    let bracket = pane
                        .bracket_colors
                        .iter()
                        .find(|&&(r, c, _)| (r, c) == (row, col))
                        .map(|&(_, _, rgb)| rgb);
                    let (fg, bold) = glyph_color(cell, theme, cursor_fg, bracket);
                    (dim(fg), bold)
                } else {
                    (
                        dim((theme.foreground.r, theme.foreground.g, theme.foreground.b)),
                        false,
                    )
                };
                let curly = cell
                    .filter(|cell| cell.style.underline == UnderlineStyle::Curly)
                    .map(|cell| dim(underline_color(cell, theme)));
                cells.push(PaintCell {
                    bold,
                    ch,
                    curly,
                    fg,
                    italic,
                    tail,
                    width,
                });
            }
        }
        cells
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The rectangles of the background pass's `verts`, each six vertices, back
/// in window pixels and sRGB.
fn fills_of(verts: &[BgVertex], surface: SurfaceSize) -> Vec<PaintFill> {
    verts
        .chunks_exact(QUAD_VERTICES)
        .map(|quad| {
            let xs = quad
                .iter()
                .map(|vertex| (vertex.x + 1.0) / 2.0 * surface.width);
            let ys = quad
                .iter()
                .map(|vertex| (1.0 - vertex.y) / 2.0 * surface.height);
            let (x0, x1) = xs.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            let (y0, y1) = ys.fold((f32::MAX, f32::MIN), |(lo, hi), y| (lo.min(y), hi.max(y)));
            let channel = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
            PaintFill {
                color: (channel(quad[0].r), channel(quad[0].g), channel(quad[0].b)),
                height: y1 - y0,
                width: x1 - x0,
                x: x0,
                y: y0,
            }
        })
        .collect()
}

/// An ordinary cell's glyph color and whether it is bold: reverse video
/// swaps it with the background, the block cursor's contrast fix
/// `cursor_fg` wins, faint text fades toward what is behind it, a rainbow
/// `bracket` color recolors it, and dark text on a light highlight is
/// thickened.
fn glyph_color(
    cell: &Cell,
    theme: &Theme,
    cursor_fg: Option<(u8, u8, u8)>,
    bracket: Option<(u8, u8, u8)>,
) -> ((u8, u8, u8), bool) {
    let rgb_of = |color: GridColor, default: Rgb| match color {
        GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
        GridColor::Indexed(index) => theme.palette_color(index),
        GridColor::Default => (default.r, default.g, default.b),
    };
    let fg = rgb_of(cell.style.foreground, theme.foreground);
    let bg = rgb_of(cell.style.background, theme.background);
    let text = match cell.style.reversed {
        true => bg,
        false => fg,
    };
    let text = cursor_fg.unwrap_or(text);
    let text = match cell.style.faint {
        true => {
            let behind = match cell.style.reversed {
                true => fg,
                false => bg,
            };
            let faded = mix_rgb(
                Rgb::new(text.0, text.1, text.2),
                Rgb::new(behind.0, behind.1, behind.2),
                FAINT_FACTOR,
            );
            (faded.r, faded.g, faded.b)
        }
        false => text,
    };
    let highlighted = cell.style.reversed || !matches!(cell.style.background, GridColor::Default);
    let highlight = match cell.style.reversed {
        true => fg,
        false => bg,
    };
    let bold = cell.style.bold || (highlighted && needs_dark_on_light_bold(text, highlight));
    let color = match bracket {
        Some(rgb) => cursor_fg.unwrap_or(rgb),
        None => text,
    };
    (color, bold)
}

/// The color `cell`'s underline draws in: its own (SGR 58), else its glyph's.
fn underline_color(cell: &Cell, theme: &Theme) -> (u8, u8, u8) {
    let color = match cell.style.underline_color {
        GridColor::Default => cell.style.foreground,
        explicit => explicit,
    };
    match color {
        GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
        GridColor::Indexed(index) => theme.palette_color(index),
        GridColor::Default => (theme.foreground.r, theme.foreground.g, theme.foreground.b),
    }
}

/// `color` faded toward `background` when `dim`, as an inactive pane's text.
fn dimmed((r, g, b): (u8, u8, u8), background: Rgb, dim: bool) -> (u8, u8, u8) {
    if !dim {
        return (r, g, b);
    }
    let lerp =
        |v: u8, toward: u8| (v as f32 + (toward as f32 - v as f32) * DIM_FACTOR).round() as u8;
    (
        lerp(r, background.r),
        lerp(g, background.g),
        lerp(b, background.b),
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_quad_s_vertices_come_back_as_its_rectangle() {
        let surface = SurfaceSize {
            height: 200.0,
            width: 400.0,
        };
        let verts = super::super::background::quad_vertices(
            10.0,
            20.0,
            30.0,
            50.0,
            (1.0, 0.5, 0.0),
            surface.width,
            surface.height,
        );
        let fills = fills_of(&verts, surface);
        assert_eq!(fills.len(), 1);
        let fill = fills[0];
        assert!((fill.x - 10.0).abs() < 1e-3 && (fill.y - 20.0).abs() < 1e-3);
        assert!((fill.width - 20.0).abs() < 1e-3 && (fill.height - 30.0).abs() < 1e-3);
        assert_eq!(fill.color, (255, 128, 0));
    }

    #[test]
    fn test_dimming_fades_a_color_toward_the_background_and_not_otherwise() {
        let bg = Rgb::new(0, 0, 0);
        assert_eq!(dimmed((200, 100, 50), bg, false), (200, 100, 50));
        let faded = dimmed((200, 100, 50), bg, true);
        assert!(faded.0 < 200 && faded.1 < 100);
    }
}

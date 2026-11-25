//! Panes as plain shapes, for a host that draws them itself: each pane's
//! cells (the glyph, its color and face, resolved from every overlay that
//! claims it), their backgrounds and highlights as runs laid under the
//! glyphs, the rectangles drawn over them (a bar cursor, decorations, the
//! scrollbar), and the separators between panes.

use super::colors::{
    FAINT_FACTOR, block_cursor_cell, cell_text_fg, cursor_contrast_fg, mix_rgb,
    needs_dark_on_light_bold,
};
use super::display::shown_cell;
use super::frame::{CURRENT_MATCH_FG, SEARCH_MATCH_FG, pane_cell_extent, strip_panes};
use super::{GpuRenderer, PaneView, grid_area};
use misty::{Attributes, CellColor, CellHighlight, CellStyle, SolidFill};
use winter_emulator::{Cell, CellWidth, Color as GridColor, UnderlineStyle};
use winter_model::paint::INDENT_GUIDE;
use winter_theme::DIM_FACTOR;
use winter_theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// The quick-select labels' glyph color, amber.
const QUICK_LABEL_FG: (u8, u8, u8) = (255, 200, 50);

// ========================================================================
// Data Structures
// ========================================================================

/// One pane, or one strip of its gutter, as plain shapes.
#[derive(Clone, Debug, PartialEq)]
pub struct PanePaint {
    /// Whether any cell blinks (SGR 5), which the host needs a blink timer
    /// running for.
    pub blinks: bool,
    /// Every cell, row by row, `cols` to a row: its glyph, color, and face
    /// on the default background (its own background is among the fills).
    pub cells: Vec<misty::Cell>,
    /// How many columns.
    pub cols: usize,
    /// What is drawn over the cells and their glyphs, in order: a bar or
    /// underline cursor, outlines, lines, the scrollbar.
    pub fills: Vec<SolidFill>,
    /// The cells' backgrounds and highlights, the block cursor among them,
    /// which misty lays under the glyphs.
    pub highlights: Vec<CellHighlight>,
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

    /// One pane, without its gutter's strips.
    fn paint_one(&self, pane: &PaneView) -> PanePaint {
        let (cw, ch) = (self.fonts.cell_width, self.fonts.cell_height);
        let rect = grid_area(pane.rect, ch);
        let extent = pane_cell_extent(rect, (cw, ch), self.pane_pad());
        let painted = self.pane_fills(pane);
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
            fills: painted.fills,
            highlights: painted.highlights,
            origin: (rect.x + self.pane_pad(), rect.y),
            rows: extent.rows,
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
    ) -> Vec<misty::Cell> {
        let grid = pane.grid;
        let theme = self.themes.winter();
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
                    Some(CellWidth::Spacer) => misty::CellWidth::Continuation,
                    Some(CellWidth::Wide) => misty::CellWidth::Wide,
                    _ => misty::CellWidth::Single,
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
                let mut attributes = Attributes::NONE;
                if bold {
                    attributes = attributes | Attributes::BOLD;
                }
                if italic {
                    attributes = attributes | Attributes::ITALIC;
                }
                // A curly underline is drawn as misty draws one; the other
                // lines are among the fills.
                let (underline, line_color) = match cell
                    .filter(|cell| cell.style.underline == UnderlineStyle::Curly)
                    .map(|cell| dim(underline_color(cell, theme)))
                {
                    Some(color) => (misty::UnderlineStyle::Curly, cell_rgb(color)),
                    None => (misty::UnderlineStyle::None, CellColor::Default),
                };
                cells.push(misty::Cell {
                    ch,
                    style: CellStyle {
                        attributes,
                        background: CellColor::Default,
                        foreground: cell_rgb(fg),
                        underline,
                        underline_color: line_color,
                    },
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

/// Misty's exact cell color for `(r, g, b)`.
fn cell_rgb((r, g, b): (u8, u8, u8)) -> CellColor {
    CellColor::Rgb(misty::Rgb::new(r, g, b))
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
    fn test_dimming_fades_a_color_toward_the_background_and_not_otherwise() {
        let bg = Rgb::new(0, 0, 0);
        assert_eq!(dimmed((200, 100, 50), bg, false), (200, 100, 50));
        let faded = dimmed((200, 100, 50), bg, true);
        assert!(faded.0 < 200 && faded.1 < 100);
    }
}

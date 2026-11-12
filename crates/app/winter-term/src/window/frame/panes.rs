//! The panes' cells, drawn by misty: each pane, and each strip of its
//! gutter, as the renderer resolved them (glyphs and their colors, the
//! backgrounds and highlights, the cursor, lines, the scrollbar), in the
//! terminal's font, and the separators between the panes; then the
//! pictures the renderer keeps textures of (images, icons, the toolbars)
//! where they lie, and its popups over everything.

use misty::cell::{Attributes, Cell, CellSource, CellWidth, Color, Cursor, Style, UnderlineStyle};
use misty::color::{Rgb, Rgba};
use misty::{BoxStyle, Font, Frame, GpuTexture, Point, Rect};
use winter_render::{ImagePlacement, PaintCell, PaintFill, PaintWidth, PanePaint};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Data Structures
// ========================================================================

/// One pane's cells, as misty's grid reads them.
struct PaneCells {
    /// Every cell, row by row.
    cells: Vec<Cell>,
    /// How many columns.
    cols: usize,
    /// How many rows.
    rows: usize,
}

// ========================================================================
// PaneCells
// ========================================================================

impl PaneCells {
    /// Misty's cells for `paint`'s.
    fn of(paint: &PanePaint) -> Self {
        Self {
            cells: paint.cells.iter().map(cell_of).collect(),
            cols: paint.cols,
            rows: paint.rows,
        }
    }
}

impl CellSource for PaneCells {
    fn cols(&self) -> usize {
        self.cols
    }

    fn rows(&self) -> usize {
        self.rows
    }

    fn cell(&self, row: usize, col: usize) -> Option<&Cell> {
        (row < self.rows && col < self.cols)
            .then(|| self.cells.get(row * self.cols + col))
            .flatten()
    }

    /// The renderer drew the cursor among the fills.
    fn cursor(&self) -> Option<Cursor> {
        None
    }
}

// ========================================================================
// App: the panes misty draws
// ========================================================================

impl App {
    /// Draw the panes the last frame worked out over `frame`, then the
    /// renderer's pictures over them and its popups over everything.
    pub(in crate::window) fn draw_panes(&mut self, frame: &mut Frame) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let theme = misty_theme(renderer.theme());
        let background = renderer.theme().background;
        let alpha = renderer.theme().background_alpha;
        frame.clear(Rgba::new(background.r, background.g, background.b, alpha));
        let (size, family, line_height, ligatures) = renderer.grid_font();
        let font = Font::new(size)
            .with_line_height(line_height)
            .with_ligatures(ligatures);
        let font = match family {
            Some(family) => font.with_family(family),
            None => font,
        };
        for paint in &self.chrome.drawn_panes {
            let origin = Point::new(paint.origin.0, paint.origin.1);
            frame.draw_cells_in(&PaneCells::of(paint), origin, &font, &theme);
            for fill in &paint.fills {
                draw_fill(frame, fill);
            }
        }
        for fill in &self.chrome.drawn_separators {
            draw_fill(frame, fill);
        }
        let hits = renderer.hits();
        for placement in hits.pictures() {
            draw_placement(frame, placement, renderer.image_texture(placement.id));
        }
        frame.overlay(|frame| {
            for placement in hits.popups() {
                draw_placement(frame, placement, renderer.image_texture(placement.id));
            }
        });
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's cell for the renderer's `paint`: its glyph, color, and face on
/// the default background (its own background is among the fills).
fn cell_of(paint: &PaintCell) -> Cell {
    let mut attributes = Attributes::NONE;
    if paint.bold {
        attributes = attributes | Attributes::BOLD;
    }
    if paint.italic {
        attributes = attributes | Attributes::ITALIC;
    }
    let (r, g, b) = paint.fg;
    let (underline, underline_color) = match paint.curly {
        Some((r, g, b)) => (UnderlineStyle::Curly, Color::Rgb(Rgb::new(r, g, b))),
        None => (UnderlineStyle::None, Color::Default),
    };
    Cell {
        ch: paint.ch,
        style: Style {
            attributes,
            background: Color::Default,
            foreground: Color::Rgb(Rgb::new(r, g, b)),
            underline,
            underline_color,
        },
        tail: paint.tail.clone(),
        width: match paint.width {
            PaintWidth::Single => CellWidth::Single,
            PaintWidth::Spacer => CellWidth::Continuation,
            PaintWidth::Wide => CellWidth::Wide,
        },
    }
}

/// The part of `texture` that `placement` samples, stretched over its
/// rectangle and faded by its alpha; nothing without a texture.
fn draw_placement(frame: &mut Frame, placement: &ImagePlacement, texture: Option<&GpuTexture>) {
    let Some(texture) = texture else {
        return;
    };
    let across = (placement.u_max - placement.u_min).max(f32::EPSILON);
    let down = (placement.v_max - placement.v_min).max(f32::EPSILON);
    let (width, height) = (placement.width / across, placement.height / down);
    let whole = Rect::new(
        placement.x - placement.u_min * width,
        placement.y - placement.v_min * height,
        width,
        height,
    );
    let shown = Rect::new(placement.x, placement.y, placement.width, placement.height);
    frame.with_opacity(placement.alpha, |frame| {
        frame.clipped(shown, |frame| frame.draw_texture(texture, whole));
    });
}

/// `fill` as a plain box in `frame`.
fn draw_fill(frame: &mut Frame, fill: &PaintFill) {
    let (r, g, b) = fill.color;
    let style = BoxStyle {
        border: None,
        corner_radius: 0.0,
        fill: Rgba::new(r, g, b, u8::MAX),
        shadow: None,
    };
    frame.draw_box(Rect::new(fill.x, fill.y, fill.width, fill.height), &style);
}

//! The panes' cells, drawn by misty: each pane, and each strip of its
//! gutter, as the renderer resolved them (glyphs and their colors, the
//! backgrounds and highlights, the cursor, lines, the scrollbar), in the
//! terminal's font; then the pictures the renderer keeps textures of
//! (images, icons, pages) where they lie. The separators between the panes
//! are drawn apart, after the pane strips, which they divide too.

use std::time::Instant;

use crate::render::{ImagePlacement, PAGE_DIM_MIX, PanePaint, mix};
use misty::grid::{Cell, CellHighlight, CellSource, Cursor};
use misty::{Frame, GpuTexture, Point, Rect, Rgba};

use crate::window::App;

// ========================================================================
// PanePaint
// ========================================================================

/// One pane's cells, as misty's grid reads them.
impl CellSource for PanePaint {
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

    /// The backgrounds and highlights the renderer resolved, under the
    /// glyphs.
    fn highlights(&self) -> &[CellHighlight] {
        &self.highlights
    }
}

// ========================================================================
// App: the panes misty draws
// ========================================================================

impl App {
    /// Draw the panes the last frame worked out over `frame`, then the
    /// renderer's pictures over them.
    pub(in crate::window) fn draw_panes(&mut self, frame: &mut Frame) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        let theme = renderer.misty_theme();
        let background = renderer.theme().background;
        let alpha = renderer.theme().background_alpha;
        frame.clear(Rgba::new(background.r, background.g, background.b, alpha));
        let font = renderer.grid_font();
        for paint in &self.parts.drawn_panes {
            let origin = Point::new(paint.origin.0, paint.origin.1);
            frame.draw_cells_in(paint, origin, &font, theme);
            for fill in &paint.fills {
                frame.fill_rect(fill.rect, fill.color);
            }
        }
        let hits = renderer.hits();
        for placement in hits.pictures() {
            draw_placement(frame, placement, renderer.image_texture(placement.id));
        }
        self.draw_spinners(frame);
    }

    /// The editors' spinners over their rows, as a busy tab's is drawn, in
    /// the dim of the words beside them, turning in step from when the first
    /// showed.
    fn draw_spinners(&mut self, frame: &mut Frame) {
        if self.parts.spinner_slots.is_empty() {
            self.parts.spinner_epoch = None;
            return;
        }
        let Some(renderer) = &self.renderer else {
            return;
        };
        let theme = renderer.theme();
        // The cell grid's dim mixes in the background's share; this the text's.
        let color = mix(theme.foreground, theme.background, 1.0 - PAGE_DIM_MIX);
        let epoch = *self.parts.spinner_epoch.get_or_insert_with(Instant::now);
        for slot in &self.parts.spinner_slots {
            frame.draw_spinner(epoch, *slot, slot.y, color);
        }
    }
}

// ========================================================================
// App: the separators misty draws
// ========================================================================

impl App {
    /// Draw the lines between the panes the last frame worked out over
    /// `frame`, after the strips, which each pane's runs through as well as
    /// its body, so a split divides the strips too; but not over the
    /// focused strip's accent, which takes its top border over.
    pub(in crate::window) fn draw_separators(&self, frame: &mut Frame) {
        let accents: Vec<Rect> = self
            .parts
            .strips
            .iter()
            .filter_map(|strip| strip.accent_rect())
            .collect();
        for fill in &self.parts.drawn_separators {
            for part in uncovered(fill.rect, &accents) {
                frame.fill_rect(part, fill.color);
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What of `rect` is left out of the columns of every one of `holes` that
/// crosses it, left to right.
fn uncovered(rect: Rect, holes: &[Rect]) -> Vec<Rect> {
    let mut parts = vec![rect];
    for hole in holes {
        parts = parts
            .into_iter()
            .flat_map(|part| match part.intersection(hole) {
                None => vec![part],
                Some(cut) => {
                    let right = part.x + part.width;
                    let left = Rect::new(part.x, part.y, cut.x - part.x, part.height);
                    let after = cut.x + cut.width;
                    let rest = Rect::new(after, part.y, right - after, part.height);
                    [left, rest]
                        .into_iter()
                        .filter(|side| side.width > 0.0)
                        .collect()
                }
            })
            .collect();
    }
    parts
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

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_separator_is_cut_where_an_accent_crosses_it_and_whole_elsewhere() {
        let line = Rect::new(0.0, 39.0, 300.0, 1.0);
        let accent = Rect::new(100.0, 39.0, 50.0, 2.0);
        let apart = Rect::new(100.0, 80.0, 50.0, 2.0);
        assert_eq!(uncovered(line, &[apart]), [line]);
        assert_eq!(
            uncovered(line, &[accent, apart]),
            [
                Rect::new(0.0, 39.0, 100.0, 1.0),
                Rect::new(150.0, 39.0, 150.0, 1.0)
            ]
        );
    }
}

//! A drop-down list an editor opens over its pane, as the PDF viewer's
//! outline does under its toolbar: an elevated card of rows, each indented
//! to its depth behind a chevron where it folds, its title in the window's
//! own font, and a dim note (the page) at its right, the chosen row on a
//! rounded pill.
//!
//! The card lies on the pane's cells, one row to a cell row, so a click the
//! editor reads by cell lands on the row and the chevron drawn there.

use glyphon::{FontSystem, SwashCache};

use super::view::DropListView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::chrome::Chrome;
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// How far in from the card's sides the chosen row's pill sits, in pixels.
const PILL_INSET: f32 = 3.0;

/// How round the pill's corners are, in pixels.
const PILL_RADIUS: f32 = 4.0;

/// The chevrons before a row that folds: folded, and unfolded.
const FOLDED: &str = "›";
const UNFOLDED: &str = "⌄";

/// Room between a title and the note at the row's right, in cells.
const NOTE_GAP_CELLS: f32 = 2.0;

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the list `view` describes, or `None` when it has no rows.
pub(in crate::renderer) fn drop_list_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &DropListView,
) -> Option<DropdownImage> {
    if view.rows.is_empty() || view.cols == 0 {
        return None;
    }
    let (cell_w, cell_h) = view.cell;
    let panel_w = (view.cols as f32 * cell_w).round();
    let panel_h = (view.rows.len() as f32 * cell_h).round();
    let margin = DROPDOWN_SHADOW;
    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    // The text sits centered on its cell row.
    let text_dy = ((cell_h - ctx.line_height) / 2.0).round();

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);
    for (at, row) in view.rows.iter().enumerate() {
        let top = margin + at as f32 * cell_h;
        let chosen = view.selected == Some(at);
        if chosen {
            let pill = (margin + PILL_INSET, top + 1.0, panel_w - 2.0 * PILL_INSET, cell_h - 2.0);
            frame.fill(pill, PILL_RADIUS, theme.menu_hover_bg, 1.0);
        }
        let text_y = (top + text_dy) as i32;
        // The chevron on its own cell, where the editor reads a click on it.
        let mark_x = margin + row.mark_col as f32 * cell_w;
        if let Some(open) = row.fold {
            let mark = if open { UNFOLDED } else { FOLDED };
            let color = if chosen { foreground } else { muted };
            let mut glyph = shape_chrome_line(font_system, ctx, mark, color, false, true);
            let glyph_w = buffer_width(&glyph);
            let x = (mark_x + (cell_w - glyph_w) / 2.0).round() as i32;
            frame.draw_text(font_system, swash_cache, &mut glyph, (x, text_y), color);
        }
        let note_w = match row.note.is_empty() {
            true => 0.0,
            false => {
                let note = shape_chrome_line(font_system, ctx, &row.note, muted, false, true);
                buffer_width(&note).ceil()
            }
        };
        let label_x = mark_x + 2.0 * cell_w;
        let right = margin + panel_w - cell_w;
        let room = (right - note_w - NOTE_GAP_CELLS * cell_w - label_x).max(0.0);
        let color = match row.dim {
            true => muted,
            false => foreground,
        };
        let label = fit(font_system, ctx, &row.label, room);
        let mut label = shape_chrome_line(font_system, ctx, &label, color, false, true);
        frame.draw_text(font_system, swash_cache, &mut label, (label_x.round() as i32, text_y), color);
        if note_w > 0.0 {
            let mut note = shape_chrome_line(font_system, ctx, &row.note, muted, false, true);
            frame.draw_text(font_system, swash_cache, &mut note, ((right - note_w).round() as i32, text_y), muted);
        }
    }
    Some(frame.into_image(view.origin.0, view.origin.1))
}

/// `text` cut short with an ellipsis to fit `room` pixels.
fn fit(font_system: &mut FontSystem, ctx: &FontCtx, text: &str, room: f32) -> String {
    let color = glyphon::Color::rgb(0, 0, 0);
    let width = |font_system: &mut FontSystem, text: &str| {
        buffer_width(&shape_chrome_line(font_system, ctx, text, color, false, true))
    };
    if width(font_system, text) <= room {
        return text.to_string();
    }
    let chars: Vec<char> = text.chars().collect();
    // The longest prefix that fits with the ellipsis, found by halving.
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let cut: String = chars[..mid].iter().collect();
        match width(font_system, &format!("{}…", cut.trim_end())) <= room {
            true => lo = mid,
            false => hi = mid - 1,
        }
    }
    let cut: String = chars[..lo].iter().collect();
    format!("{}…", cut.trim_end())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::view::DropListRow;
    use super::*;
    use crate::renderer::test_support::*;

    fn view(count: usize) -> DropListView {
        DropListView {
            cell: (10.0, 20.0),
            cols: 30,
            origin: (40.0, 60.0),
            rows: (0..count)
                .map(|at| DropListRow {
                    dim: false,
                    fold: (at == 0).then_some(false),
                    label: format!("{at} A section with a rather long title that will not fit"),
                    mark_col: 1,
                    note: (at + 1).to_string(),
                })
                .collect(),
            selected: Some(0),
        }
    }

    #[test]
    fn test_the_card_lies_on_the_cells_it_was_given() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let theme = Theme::dark();
        let image = drop_list_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(4)).expect("a card");
        assert_eq!((image.x, image.y), (40.0 - DROPDOWN_SHADOW, 60.0 - DROPDOWN_SHADOW));
        assert_eq!(image.width as f32, 300.0 + 2.0 * DROPDOWN_SHADOW, "30 cells wide");
        assert_eq!(image.height as f32, 80.0 + 2.0 * DROPDOWN_SHADOW, "a cell row per row");
        assert!(drop_list_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(0)).is_none());
    }

    #[test]
    fn test_a_title_too_long_for_its_room_is_cut_short() {
        let mut font_system = FontSystem::new();
        let ctx = sample_font_ctx();
        let cut = fit(&mut font_system, &ctx, "A section with a rather long title", 80.0);
        assert!(cut.ends_with('…') && cut.chars().count() < 34, "{cut}");
        assert_eq!(fit(&mut font_system, &ctx, "Short", 500.0), "Short");
    }
}

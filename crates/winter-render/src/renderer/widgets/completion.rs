//! The completion popup: a list hung under the word being completed, its
//! entries one to a row in the editor's own font, each marked with a letter
//! for what it is and followed, dim, by more about it and the back-end that
//! offered it. Over the word instead where the window has no room below.

use glyphon::{FontSystem, SwashCache};

use super::view::CompletionView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::chrome::{anchored, Chrome};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// The most entries shown at once; the window over them follows the one
/// chosen.
const MAX_ROWS: usize = 10;

/// The room around a row's text, in pixels: above and below it, and at the
/// card's sides.
const ROW_PAD_Y: f32 = 2.0;
const PAD_X: f32 = 8.0;

/// Between the kind's letter, the label, the detail, and the source, in
/// cells.
const GAP_CELLS: f32 = 1.5;

/// The widest a detail is drawn, in characters, before it is cut short.
const MAX_DETAIL_CHARS: usize = 40;

/// The widest the card grows, as a share of the window.
const MAX_WIDTH_SHARE: f32 = 0.6;

// ========================================================================
// Free functions
// ========================================================================

/// The rows shown of `count`, a window of [`MAX_ROWS`] that holds
/// `selected`, as its first row and how many.
pub(in crate::renderer) fn shown_rows(count: usize, selected: usize) -> (usize, usize) {
    let shown = count.min(MAX_ROWS);
    let first = selected.saturating_sub(shown.saturating_sub(1)).min(count - shown);
    (first, shown)
}

/// Rasterize the popup `view` describes, inside a `surface_w` by
/// `surface_h` window.
pub(in crate::renderer) fn completion_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &CompletionView,
    surface_w: f32,
    surface_h: f32,
) -> Option<DropdownImage> {
    if view.rows.is_empty() {
        return None;
    }
    let margin = DROPDOWN_SHADOW;
    let (first, shown) = shown_rows(view.rows.len(), view.selected);
    let rows = &view.rows[first..first + shown];
    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    let gap = (ctx.cell_w * GAP_CELLS).round();
    let detail_of = |detail: &Option<String>| {
        detail.as_ref().map(|text| {
            let cut: String = text.chars().take(MAX_DETAIL_CHARS).collect();
            match text.chars().count() > MAX_DETAIL_CHARS {
                true => format!("{cut}…"),
                false => cut,
            }
        })
    };
    let width_of = |font_system: &mut FontSystem, text: &str, proportional: bool| {
        buffer_width(&shape_chrome_line(font_system, ctx, text, foreground, false, proportional)).ceil()
    };
    let kind_w = ctx.cell_w.ceil();
    let label_w = rows
        .iter()
        .map(|row| width_of(font_system, &row.label, false))
        .fold(0.0, f32::max);
    let detail_w = rows
        .iter()
        .filter_map(|row| detail_of(&row.detail))
        .map(|detail| width_of(font_system, &detail, true))
        .fold(0.0, f32::max);
    let source_w = rows
        .iter()
        .map(|row| width_of(font_system, &row.source, true))
        .fold(0.0, f32::max);
    let content_w = kind_w + gap + label_w + if detail_w > 0.0 { gap + detail_w } else { 0.0 } + gap + source_w;
    let panel_w = (content_w + 2.0 * PAD_X).min(surface_w * MAX_WIDTH_SHARE).ceil();
    let row_h = (ctx.line_height + 2.0 * ROW_PAD_Y).round();
    let panel_h = row_h * shown as f32 + 2.0;

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);
    for (at, row) in rows.iter().enumerate() {
        let top = margin + 1.0 + row_h * at as f32;
        if first + at == view.selected {
            frame.fill((margin + 2.0, top, panel_w - 4.0, row_h), 3.0, theme.menu_hover_bg, 1.0);
        }
        let text_y = (top + ROW_PAD_Y).round() as i32;
        let mut x = margin + PAD_X;
        let mut kind = shape_chrome_line(font_system, ctx, &row.kind.to_string(), accent, false, false);
        frame.draw_text(font_system, swash_cache, &mut kind, (x as i32, text_y), accent);
        x += kind_w + gap;
        let mut label = shape_chrome_line(font_system, ctx, &row.label, foreground, false, false);
        frame.draw_text(font_system, swash_cache, &mut label, (x as i32, text_y), foreground);
        x += label_w + gap;
        if let Some(detail) = detail_of(&row.detail) {
            let mut detail = shape_chrome_line(font_system, ctx, &detail, muted, false, true);
            frame.draw_text(font_system, swash_cache, &mut detail, (x as i32, text_y), muted);
        }
        let mut source = shape_chrome_line(font_system, ctx, &row.source, muted, false, true);
        let source_x = margin + panel_w - PAD_X - width_of(font_system, &row.source, true);
        frame.draw_text(font_system, swash_cache, &mut source, (source_x as i32, text_y), muted);
    }

    let (x, y) = anchored(view.anchor, (panel_w, panel_h), margin, (surface_w, surface_h));
    Some(frame.into_image(x, y))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::view::CompletionRow;
    use super::*;
    use crate::renderer::test_support::*;

    fn view(count: usize, anchor: (f32, f32, f32)) -> CompletionView {
        CompletionView {
            anchor,
            rows: (0..count)
                .map(|at| CompletionRow {
                    detail: (at == 0).then(|| "a function".to_string()),
                    kind: 'w',
                    label: format!("candidate{at}"),
                    source: "buffer".to_string(),
                })
                .collect(),
            selected: 0,
        }
    }

    #[test]
    fn test_the_window_over_the_entries_follows_the_one_chosen() {
        assert_eq!(shown_rows(3, 0), (0, 3));
        assert_eq!(shown_rows(30, 0), (0, 10));
        assert_eq!(shown_rows(30, 14), (5, 10));
        assert_eq!(shown_rows(30, 29), (20, 10));
    }

    #[test]
    fn test_the_card_hangs_under_the_word_or_over_it_near_the_bottom() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let theme = Theme::dark();
        let under = completion_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(3, (100.0, 200.0, 218.0)), 1000.0, 800.0)
            .expect("a card");
        assert!(under.y >= 218.0 - DROPDOWN_SHADOW, "under the word: {}", under.y);
        let over = completion_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(3, (100.0, 760.0, 778.0)), 1000.0, 800.0)
            .expect("a card");
        assert!(over.y + (over.height as f32) <= 760.0 + DROPDOWN_SHADOW, "over it: {}", over.y);
        assert!(completion_rgba(&mut font_system, &mut swash, &ctx, &theme, &view(0, (0.0, 0.0, 0.0)), 1000.0, 800.0).is_none());
    }
}

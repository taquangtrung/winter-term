//! The find popup: the line a search is typed on, hung from the top-right
//! corner of the pane being searched, the way an editor's find widget is.
//! How far the search has got stays in the status bar.

use glyphon::{FontSystem, SwashCache};

use super::view::FindView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::popup::{
    draw_caret, draw_field_well, PopupFrame, INPUT_FIELD_PAD_RATIO, INPUT_FIELD_TEXT_INSET,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// How wide the popup is, in cells, and the least it shrinks to in a narrow
/// pane before it is clipped by the pane instead.
const FIND_WIDTH_CELLS: f32 = 34.0;
const FIND_MIN_WIDTH_CELLS: f32 = 14.0;

/// How far in from the pane's top and right edges the popup hangs, in
/// pixels.
const FIND_EDGE_GAP: f32 = 6.0;

/// The room around the card's contents, and between the label and the
/// field, as shares of a cell's width.
const FIND_PAD_CELLS: f32 = 0.6;
const FIND_LABEL_GAP_CELLS: f32 = 0.5;

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the find popup: the search's direction (`/` or `?`), then the
/// field holding what has been typed with its caret, on a small card at the
/// top-right of the pane. A query wider than the field scrolls so the caret
/// stays in sight.
pub(in crate::renderer) fn find_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &FindView,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let (pane_x, pane_y, pane_w) = view.pane;
    let pad = (ctx.cell_w * FIND_PAD_CELLS).round();
    let field_pad_y = (ctx.cell_h * INPUT_FIELD_PAD_RATIO).round();
    let field_h = ctx.line_height + field_pad_y * 2.0;
    let room = (pane_w - 2.0 * FIND_EDGE_GAP).max(ctx.cell_w * FIND_MIN_WIDTH_CELLS);
    let panel_w = (ctx.cell_w * FIND_WIDTH_CELLS).min(room).floor();
    let panel_h = (field_h + pad * 2.0).ceil();

    let mut frame = PopupFrame::new(theme, panel_w, panel_h, margin);
    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let top = margin + pad;
    let text_y = (top + field_pad_y) as i32;

    // The direction, beside the field rather than in it.
    let mut label = shape_chrome_line(font_system, ctx, &view.label, muted, true, true);
    let label_w = buffer_width(&label).ceil();
    let label_x = margin + pad;
    frame.draw_text(
        font_system,
        swash_cache,
        &mut label,
        (label_x as i32, text_y),
        muted,
    );

    let field_x = label_x + label_w + (ctx.cell_w * FIND_LABEL_GAP_CELLS).round();
    let field_w = (margin + panel_w - pad - field_x).max(ctx.cell_w);
    draw_field_well(&mut frame, theme, (field_x, top, field_w, field_h));

    // Only the part of the query that fits is drawn, starting late enough
    // that the caret is inside the field.
    let inset = (ctx.cell_w * INPUT_FIELD_TEXT_INSET).round();
    let text_room = (field_w - 2.0 * inset).max(1.0);
    let chars: Vec<char> = view.query.chars().collect();
    let caret = view.caret.min(chars.len());
    let width_of = |font_system: &mut FontSystem, text: &str| {
        buffer_width(&shape_chrome_line(
            font_system,
            ctx,
            text,
            foreground,
            false,
            true,
        ))
    };
    let mut first = 0;
    while first < caret {
        let before: String = chars[first..caret].iter().collect();
        if width_of(font_system, &before) <= text_room {
            break;
        }
        first += 1;
    }
    let mut shown: String = chars[first..].iter().collect();
    while !shown.is_empty() && width_of(font_system, &shown) > text_room {
        shown.pop();
    }
    let text_x = field_x + inset;
    let mut text = shape_chrome_line(font_system, ctx, &shown, foreground, false, true);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut text,
        (text_x as i32, text_y),
        foreground,
    );
    if view.caret_visible {
        let before: String = chars[first..caret].iter().collect();
        let caret_x = text_x + width_of(font_system, &before).ceil() + 1.0;
        draw_caret(&mut frame, ctx, theme, (caret_x, text_y as f32 + 1.0));
    }

    let panel_x = pane_x + pane_w - FIND_EDGE_GAP - panel_w;
    let panel_y = pane_y + FIND_EDGE_GAP;
    frame.into_image(panel_x, panel_y)
}

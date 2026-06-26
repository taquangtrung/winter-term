//! The tooltip that floats under a link the pointer is over, naming its
//! address.

use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::popup::PopupFrame;
use crate::renderer::raster::view::DropdownImage;
use crate::tabbar::{Region, TabbarLayout};
use crate::theme::Theme;
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize a URL tooltip bubble near the cursor. `anchor` is the cursor cell
/// rect; the tooltip floats below it, clamped within the surface bounds.
#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn url_tooltip_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    url: &str,
    anchor: &Region,
    surface_w: f32,
    surface_h: f32,
    tab_layout: Option<&TabbarLayout>,
    hovered_tab: Option<usize>,
) -> DropdownImage {
    const PAD_X: f32 = 8.0;
    const PAD_Y: f32 = 4.0;
    const SHADOW_MARGIN: f32 = 6.0;
    const MAX_URL_CHARS: usize = 80;

    let display_url = if url.len() > MAX_URL_CHARS {
        format!("{}...", &url[..MAX_URL_CHARS])
    } else {
        url.to_string()
    };

    let text_color = theme.foreground.to_glyphon();
    let mut label_buf = shape_chrome_line(font_system, ctx, &display_url, text_color, false, true);
    let text_w = buffer_width(&label_buf).ceil();
    let text_h = ctx.line_height;

    let panel_w = text_w + PAD_X * 2.0;
    let panel_h = text_h + PAD_Y * 2.0;
    let total_w = (panel_w + SHADOW_MARGIN * 2.0) as u32;
    let total_h = (panel_h + SHADOW_MARGIN * 2.0) as u32;

    // Place tooltip below the cursor, then flip above if it would go off-screen.
    let (tip_x, tip_y) = if let (Some(layout), Some(idx)) = (tab_layout, hovered_tab) {
        let tab_region = layout.tabs[idx];
        let tab_center_x = tab_region.x + tab_region.w * 0.5;
        let x = (tab_center_x - total_w as f32 * 0.5)
            .max(0.0)
            .min((surface_w - total_w as f32).max(0.0))
            .round();

        let tab_bottom_y = tab_region.y + tab_region.h;
        let y = (tab_bottom_y + 2.0 - SHADOW_MARGIN)
            .max(0.0)
            .min((surface_h - total_h as f32).max(0.0))
            .round();
        (x, y)
    } else {
        let x = (anchor.x - panel_w * 0.5 - SHADOW_MARGIN)
            .max(0.0)
            .min((surface_w - total_w as f32).max(0.0))
            .round();
        let below_y = anchor.y + anchor.h + 2.0 - SHADOW_MARGIN;
        let y = if below_y + total_h as f32 > surface_h {
            (anchor.y - total_h as f32 - 2.0 + SHADOW_MARGIN)
                .max(0.0)
                .round()
        } else {
            below_y.round()
        };
        (x, y)
    };

    let mut frame = PopupFrame::new(theme, panel_w, panel_h, SHADOW_MARGIN);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut label_buf,
        (
            (SHADOW_MARGIN + PAD_X) as i32,
            (SHADOW_MARGIN + PAD_Y) as i32,
        ),
        text_color,
    );
    frame.into_image(tip_x + SHADOW_MARGIN, tip_y + SHADOW_MARGIN)
}

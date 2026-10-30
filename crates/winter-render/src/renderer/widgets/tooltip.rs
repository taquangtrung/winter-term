//! The tooltip: floats under what the pointer is over (a link, a button, a
//! tab), saying what it is. A tooltip only shows, has no title bar, and
//! hangs from what it is about.

use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::chrome::Chrome;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::view::DropdownImage;
use crate::tabbar::Region;
use crate::theme::Theme;
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize a tooltip saying `url` under `anchor`, the element it is about
/// (or the pointer's cell): centered on it and just below it, or just above
/// it where there is no room below, kept within the surface.
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
) -> DropdownImage {
    const GAP: f32 = 2.0;
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

    let (tip_x, tip_y) = tooltip_origin(
        anchor,
        (total_w as f32, total_h as f32),
        (surface_w, surface_h),
        GAP,
        SHADOW_MARGIN,
    );

    let mut frame = Chrome::new(theme, panel_w, panel_h, SHADOW_MARGIN);
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

/// Where a tooltip `size` big, its shadow `margin` included, goes for
/// `anchor` on a `surface`: its panel centered on the anchor and `gap`
/// above it where room permits (leaving content below unobstructed),
/// or `gap` below it where it would run off the top, kept within the surface.
fn tooltip_origin(
    anchor: &Region,
    size: (f32, f32),
    surface: (f32, f32),
    gap: f32,
    margin: f32,
) -> (f32, f32) {
    let (w, h) = size;
    let x = (anchor.x + anchor.w * 0.5 - w * 0.5)
        .min(surface.0 - w)
        .max(0.0);
    let above = anchor.y - h - gap + margin;
    let below = anchor.y + anchor.h + gap - margin;
    let y = match above >= 0.0 {
        true => above,
        false => match below + h <= surface.1 {
            true => below,
            false => above.max(0.0),
        },
    };
    (x.round(), y.round())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const SURFACE: (f32, f32) = (800.0, 600.0);

    fn region(x: f32, y: f32, w: f32, h: f32) -> Region {
        Region { h, w, x, y }
    }

    #[test]
    fn test_a_tooltip_hangs_centered_just_above_what_it_is_about_when_room_permits() {
        let tab = region(100.0, 40.0, 120.0, 30.0);
        let (x, y) = tooltip_origin(&tab, (80.0, 32.0), SURFACE, 2.0, 6.0);
        assert_eq!(x, 120.0, "its middle on the tab's middle");
        assert_eq!(y + 32.0 - 6.0, 38.0, "its panel just above the tab's top edge");
    }

    #[test]
    fn test_a_tooltip_with_no_room_above_goes_below_and_none_runs_off_the_sides() {
        let top = region(0.0, 0.0, 10.0, 16.0);
        let (x, y) = tooltip_origin(&top, (80.0, 32.0), SURFACE, 2.0, 6.0);
        assert_eq!(x, 0.0, "kept inside the left edge");
        assert_eq!(y + 6.0, 18.0, "its panel just below the top anchor");

        let low = region(780.0, 580.0, 10.0, 16.0);
        let (x, y) = tooltip_origin(&low, (80.0, 32.0), SURFACE, 2.0, 6.0);
        assert_eq!(x, 720.0, "kept inside the right edge");
        assert_eq!(y + 32.0 - 6.0, 578.0, "its panel just over the anchor");
    }
}

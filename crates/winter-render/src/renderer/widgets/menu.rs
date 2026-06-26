//! Menu panels: the open dropdown and its submenu, and the right-click
//! context menu, all one elevated card of rows.

use super::MENU_HOVER_INSET;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{blend_px, buffer_width, shape_chrome_line};
use crate::renderer::raster::popup::PopupFrame;
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::tabbar::{layout as tabbar_layout, DropdownLayout, MenuItem, TopTabbar};
use crate::theme::Theme;
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the open dropdown overlay (soft shadow, elevated rounded panel,
/// rounded hover pill, and sans-serif item text) to an RGBA image without the
/// GPU. Returns `None` when no menu is open.
/// The parent dropdown panel for the open menu, or `None` when none is open.
pub(in crate::renderer) fn dropdown_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    tabbar: &TopTabbar,
    surface_w: f32,
) -> Option<DropdownImage> {
    let layout = tabbar_layout(tabbar, surface_w, ctx.cell_w, ctx.cell_h);
    let menu = tabbar.menus.get(tabbar.open_menu?)?;
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &menu.items,
        &layout.dropdown?,
        tabbar.selected_item,
    );
    Some(panel)
}
/// The open submenu's child panel, or `None` when no submenu is open.
pub(in crate::renderer) fn submenu_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    tabbar: &TopTabbar,
    surface_w: f32,
) -> Option<DropdownImage> {
    let layout = tabbar_layout(tabbar, surface_w, ctx.cell_w, ctx.cell_h);
    let parent = tabbar
        .menus
        .get(tabbar.open_menu?)?
        .items
        .get(tabbar.open_submenu?)?;
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &parent.children,
        &layout.submenu?,
        tabbar.selected_subitem,
    );
    Some(panel)
}
/// Rasterize the right-click context menu panel, or `None` when none is open.
pub(in crate::renderer) fn context_menu_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    tabbar: &TopTabbar,
    surface_w: f32,
) -> Option<DropdownImage> {
    let cm = tabbar.context_menu.as_ref()?;
    let layout = tabbar_layout(tabbar, surface_w, ctx.cell_w, ctx.cell_h);
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &cm.items,
        &layout.context_menu?,
        cm.selected,
    );
    Some(panel)
}
/// Rasterize one menu panel (`items` at `layout`) into an elevated, rounded,
/// shadowed card. `selected` is the hover-highlighted row. A submenu parent
/// (an item with children) gets a `›` chevron instead of a shortcut.
fn panel_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    items: &[MenuItem],
    layout: &DropdownLayout,
    selected: Option<usize>,
) -> DropdownImage {
    // The panel sits inside a margin that holds its soft drop shadow, so the
    // texture is larger than the panel and is placed offset by the margin. The
    // panel interior still lands exactly at origin_x/top, keeping the app's
    // hit-testing geometry valid.
    let panel_w = layout.width.floor().max(1.0);
    let panel_h = (layout.items as f32 * layout.item_h + 2.0 * layout.pad)
        .floor()
        .max(1.0);
    let mut frame = PopupFrame::new(theme, panel_w, panel_h, DROPDOWN_SHADOW);
    let margin = frame.margin();

    // The rounded hover pill for the selected row. The first row begins below
    // the panel's top padding.
    let row_top = margin + layout.pad;
    if let Some(sel) = selected {
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            row_top + sel as f32 * layout.item_h + MENU_HOVER_INSET,
            panel_w - 2.0 * MENU_HOVER_INSET,
            layout.item_h - 2.0 * MENU_HOVER_INSET,
        );
        let radius = 2.0;
        frame.fill(hover_rect, radius, theme.menu_hover_bg, 1.0);
    }

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let pad = (ctx.cell_w * 0.9) as i32;
    let origin = margin as i32;
    // Center each label vertically within its taller row.
    let text_dy = ((layout.item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    for (i, item) in items.iter().enumerate() {
        let row_y = (row_top + i as f32 * layout.item_h) as i32;
        if item.label == "-" {
            let line_y = row_y + (layout.item_h / 2.0) as i32;
            let (rgba, (width, height)) = frame.pixels();
            for x in (origin + pad)..(origin + panel_w as i32 - pad) {
                if (0..width as i32).contains(&x) && (0..height as i32).contains(&line_y) {
                    let idx = ((line_y * width as i32 + x) * 4) as usize;
                    blend_px(rgba, idx, theme.divider, 1.0);
                }
            }
            continue;
        }
        let text_y = row_y + text_dy;
        let mut label = shape_chrome_line(font_system, ctx, &item.label, foreground, false, true);
        let pos = (origin + pad, text_y);
        frame.draw_text(font_system, swash_cache, &mut label, pos, foreground);

        // A submenu parent shows a chevron; a leaf shows its shortcut, if any.
        let trailing = if item.has_children() {
            Some("\u{203a}".to_string())
        } else if !item.shortcut.is_empty() {
            Some(item.shortcut.clone())
        } else {
            None
        };
        if let Some(text) = trailing {
            let mut buf = shape_chrome_line(font_system, ctx, &text, muted, false, true);
            let buf_w = buffer_width(&buf).ceil() as i32;
            let pos = (origin + panel_w as i32 - buf_w - pad, text_y);
            frame.draw_text(font_system, swash_cache, &mut buf, pos, muted);
        }
    }

    frame.into_image(layout.origin_x, layout.top)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_dropdown_rgba_is_an_elevated_rounded_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let tabbar = sample_menu_chrome(None);

        let image = dropdown_rgba(&mut font_system, &mut swash, &ctx, &theme, &tabbar, 1000.0)
            .expect("menu is open");
        let pixel = |x: u32, y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            (
                image.rgba[i],
                image.rgba[i + 1],
                image.rgba[i + 2],
                image.rgba[i + 3],
            )
        };
        let margin = DROPDOWN_SHADOW as u32;

        // The outer corner is fully transparent: the panel does not fill the
        // shadow margin (this is a floating card, not a full-bleed rectangle).
        assert_eq!(pixel(0, 0).3, 0);
        // The panel's own top-left corner is rounded away (not fully opaque)...
        assert!(pixel(margin, margin).3 < 255);
        // ...while the top padding band is the opaque elevated surface color,
        // proving both the lighter menu_bg and the vertical padding are applied.
        let (r, g, b, a) = pixel(image.width / 2, margin + 2);
        assert_eq!(
            (r, g, b, a),
            (theme.menu_bg.r, theme.menu_bg.g, theme.menu_bg.b, 255)
        );
    }
    #[test]
    fn test_dropdown_overlay_is_pixel_snapped() {
        // Real displays have fractional cell metrics; without snapping, the
        // overlay lands on sub-pixel boundaries and the linear sampler blurs it.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = FontCtx {
            cell_h: 19.4,
            cell_w: 9.6,
            family: None,
            font_has_bold: true,
            font_size: 14.0,
            line_height: 19.4,
            normal_weight: None,
            bold_weight: None,
        };
        let tabbar = sample_menu_chrome(None);
        let image = dropdown_rgba(&mut font_system, &mut swash, &ctx, &theme, &tabbar, 1003.0)
            .expect("menu is open");
        assert_eq!(image.x.fract(), 0.0, "overlay x must be whole pixels");
        assert_eq!(image.y.fract(), 0.0, "overlay y must be whole pixels");
    }
}

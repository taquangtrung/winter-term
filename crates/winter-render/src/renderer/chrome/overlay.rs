//! Rasterizing each floating surface into an RGBA buffer.

use super::paint::{
    blend_px, buffer_width, composite_buffer, draw_highlighted_label, fill_rounded_rect,
    rounded_rect_sdf, shape_chrome_line,
};
use super::view::{DropdownImage, PaletteView, WhichKeyView};
use super::{
    DROPDOWN_RADIUS, DROPDOWN_SHADOW, DROPDOWN_SHADOW_ALPHA, MENU_BORDER_MIX, MENU_HOVER_INSET,
    PALETTE_ITEM_PAD_X, PALETTE_ITEM_PAD_Y, PALETTE_MAX_ITEMS, PALETTE_TOP_RATIO,
    PALETTE_WIDTH_RATIO, SHADOW_COLOR,
};
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::tabbar::{
    layout as tabbar_layout, DropdownLayout, MenuItem, Region, TabbarLayout, TopTabbar,
};
use crate::theme::{Rgb, Theme};
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Overlay rasterizers
// ========================================================================

/// Rasterize a transient toast pill with `text` in `text_color`, anchored to the
/// top-right of the surface and dropped `top_inset` pixels (the tab-bar height)
/// from the top so it sits just below the tabs. A rounded panel with a soft
/// shadow, positioned as a free-floating overlay.
#[allow(clippy::too_many_arguments)]
pub(super) fn toast_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    text: &str,
    text_color: Rgb,
    surface_w: f32,
    top_inset: f32,
) -> DropdownImage {
    const PAD_X: f32 = 14.0;
    const PAD_Y: f32 = 7.0;
    const RADIUS: f32 = 8.0;
    const SHADOW_MARGIN: f32 = 8.0;
    /// Gap from the right edge and from the bottom of the tab bar, in pixels.
    const EDGE_GAP: f32 = 16.0;

    let mut label_buf =
        shape_chrome_line(font_system, ctx, text, text_color.to_glyphon(), true, true);
    let text_w = buffer_width(&label_buf).ceil();
    let text_h = ctx.line_height;

    let panel_w = text_w + PAD_X * 2.0;
    let panel_h = text_h + PAD_Y * 2.0;
    let total_w = (panel_w + SHADOW_MARGIN * 2.0) as u32;
    let total_h = (panel_h + SHADOW_MARGIN * 2.0) as u32;
    let canvas = (total_w, total_h);

    // Top-right: right edge of the pill sits EDGE_GAP from the surface edge, its
    // top sits EDGE_GAP below the tab bar. SHADOW_MARGIN offsets keep the visible
    // panel (not its transparent shadow padding) at those gaps.
    let tip_x = (surface_w - EDGE_GAP - total_w as f32 + SHADOW_MARGIN).max(0.0);
    let tip_y = (top_inset + EDGE_GAP - SHADOW_MARGIN).max(0.0);

    let mut rgba = vec![0u8; (total_w * total_h * 4) as usize];

    // Soft drop-shadow.
    for py in 0..total_h {
        for px in 0..total_w {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
                RADIUS,
            );
            let falloff = (1.0 - sdf / SHADOW_MARGIN).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * total_w + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Panel background with a hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
        RADIUS,
        border,
        1.0,
    );
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (
            SHADOW_MARGIN + 1.0,
            SHADOW_MARGIN + 1.0,
            panel_w - 2.0,
            panel_h - 2.0,
        ),
        RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut label_buf,
        (
            (SHADOW_MARGIN + PAD_X) as i32,
            (SHADOW_MARGIN + PAD_Y) as i32,
        ),
        text_color.to_glyphon(),
    );

    DropdownImage {
        height: total_h,
        rgba,
        width: total_w,
        x: tip_x,
        y: tip_y,
    }
}
/// Rasterize a URL tooltip bubble near the cursor. `anchor` is the cursor cell
/// rect; the tooltip floats below it, clamped within the surface bounds.
#[allow(clippy::too_many_arguments)]
pub(super) fn url_tooltip_rgba(
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
    const RADIUS: f32 = 4.0;
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
    let canvas = (total_w, total_h);

    // Place tooltip below the cursor, then flip above if it would go off-screen.
    let (tip_x, tip_y) = if let (Some(layout), Some(idx)) = (tab_layout, hovered_tab) {
        let tab_region = layout.tabs[idx];
        let tab_center_x = tab_region.x + tab_region.w * 0.5;
        let x = (tab_center_x - total_w as f32 * 0.5)
            .max(0.0)
            .min((surface_w - total_w as f32).max(0.0))
            .round();

        let tab_bottom_y = layout.new_tab.y + layout.new_tab.h;
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

    let mut rgba = vec![0u8; (total_w * total_h * 4) as usize];

    for py in 0..total_h {
        for px in 0..total_w {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
                RADIUS,
            );
            let falloff = (1.0 - sdf / SHADOW_MARGIN).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * total_w + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
        RADIUS,
        border,
        1.0,
    );
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (
            SHADOW_MARGIN + 1.0,
            SHADOW_MARGIN + 1.0,
            panel_w - 2.0,
            panel_h - 2.0,
        ),
        RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut label_buf,
        (
            (SHADOW_MARGIN + PAD_X) as i32,
            (SHADOW_MARGIN + PAD_Y) as i32,
        ),
        text_color,
    );

    DropdownImage {
        height: total_h,
        rgba,
        width: total_w,
        x: tip_x,
        y: tip_y,
    }
}
/// Rasterize the open dropdown overlay (soft shadow, elevated rounded panel,
/// rounded hover pill, and sans-serif item text) to an RGBA image without the
/// GPU. Returns `None` when no menu is open.
/// The parent dropdown panel for the open menu, or `None` when none is open.
pub(super) fn dropdown_rgba(
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
pub(super) fn submenu_rgba(
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
pub(super) fn context_menu_rgba(
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
    let margin = DROPDOWN_SHADOW;
    let panel_w = layout.width.floor().max(1.0);
    let panel_h = (layout.items as f32 * layout.item_h + 2.0 * layout.pad)
        .floor()
        .max(1.0);
    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow: opacity fades with distance outside the panel.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel: a subtle hairline border, then the surface inset
    // one pixel within it, then the rounded hover pill for the selected row. The
    // first row begins below the panel's top padding.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );
    let row_top = margin + layout.pad;
    if let Some(sel) = selected {
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            row_top + sel as f32 * layout.item_h + MENU_HOVER_INSET,
            panel_w - 2.0 * MENU_HOVER_INSET,
            layout.item_h - 2.0 * MENU_HOVER_INSET,
        );
        let radius = (DROPDOWN_RADIUS - MENU_HOVER_INSET).max(0.0);
        fill_rounded_rect(
            &mut rgba,
            canvas,
            hover_rect,
            radius,
            theme.menu_hover_bg,
            1.0,
        );
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
            for x in (origin + pad)..(origin + panel_w as i32 - pad) {
                if (0..width as i32).contains(&x) && (0..height as i32).contains(&line_y) {
                    let idx = ((line_y * width as i32 + x) * 4) as usize;
                    blend_px(&mut rgba, idx, theme.divider, 1.0);
                }
            }
            continue;
        }
        let text_y = row_y + text_dy;
        let mut label = shape_chrome_line(font_system, ctx, &item.label, foreground, false, true);
        let pos = (origin + pad, text_y);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut label,
            pos,
            foreground,
        );

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
            composite_buffer(
                font_system,
                swash_cache,
                &mut rgba,
                canvas,
                &mut buf,
                pos,
                muted,
            );
        }
    }

    // Snap the overlay to whole pixels. The texture is 1:1 texel-to-pixel, so a
    // fractional placement (origin_x/top derive from fractional cell sizes) would
    // make the shared linear sampler smear every baked glyph, blurring the menu.
    DropdownImage {
        height,
        rgba,
        width,
        x: (layout.origin_x - margin).round(),
        y: (layout.top - margin).round(),
    }
}
/// Rasterize the command palette: a centered floating panel with a search input
/// at the top and a fuzzy-filtered list of commands below. Styled like the
/// dropdown but wider and vertically centered in the upper portion of the window.
pub(super) fn palette_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PaletteView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let inner_pad = ctx.cell_h * 0.5;
    let input_h = ctx.cell_h * 2.2;
    let item_h = (ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0).round();
    let display_count = view.items.len().min(PALETTE_MAX_ITEMS);
    let row_count = display_count.max(1);
    let panel_w = (surface_w * PALETTE_WIDTH_RATIO)
        .clamp(300.0, 680.0)
        .floor();
    let panel_h = (inner_pad + input_h + 1.0 + item_h * row_count as f32 + inner_pad)
        .floor()
        .max(1.0);

    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel with hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Hover highlight on the selected result row.
    let divider_y = margin + inner_pad + input_h;
    let results_top = divider_y + 1.0;
    if display_count > 0 {
        let sel = view.selected.min(display_count - 1);
        // The highlight fills the row vertically (only a 1px breath) so it covers
        // the text and selected rows butt against their neighbours with no gap.
        let hover_v_inset = 1.0;
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            results_top + sel as f32 * item_h + hover_v_inset,
            panel_w - 2.0 * MENU_HOVER_INSET,
            item_h - 2.0 * hover_v_inset,
        );
        let radius = (DROPDOWN_RADIUS - MENU_HOVER_INSET).max(0.0);
        fill_rounded_rect(
            &mut rgba,
            canvas,
            hover_rect,
            radius,
            theme.menu_hover_bg,
            1.0,
        );
    }

    // Hairline divider between the input and the results.
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin, divider_y, panel_w, 1.0),
        0.0,
        theme.divider,
        0.5,
    );

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    // Fuzzy-matched characters are tinted blue to stand out from the label.
    let match_color = theme.ansi[4].to_glyphon();
    let pad_x = (ctx.cell_w * 1.0) as i32;
    let origin = margin as i32;
    let input_text_dy = ((input_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let item_text_dy = ((item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let input_top = (margin + inner_pad) as i32;

    // "❯" prompt.
    let mut prompt_buf = shape_chrome_line(font_system, ctx, "\u{276f} ", accent, false, true);
    let prompt_w = buffer_width(&prompt_buf).ceil() as i32;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut prompt_buf,
        (origin + pad_x, input_top + input_text_dy),
        accent,
    );

    // Query text.
    let query_text = view.query.clone();
    let query_color = foreground;
    let mut query_buf = shape_chrome_line(font_system, ctx, &query_text, query_color, true, true);
    let query_w = buffer_width(&query_buf).ceil() as i32;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut query_buf,
        (origin + pad_x + prompt_w, input_top + input_text_dy),
        query_color,
    );

    // Caret: a thin vertical bar right after the query text. A glyph caret would
    // sit centered in a full cell box, leaving an unnatural gap from the last
    // character, so draw the bar directly instead.
    let caret_x = (origin + pad_x + prompt_w + query_w) as f32 + 1.0;
    let caret_y = (input_top + input_text_dy) as f32 + 1.0;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (caret_x, caret_y, 2.0, (ctx.line_height - 2.0).max(1.0)),
        1.0,
        theme.cursor_bg,
        1.0,
    );

    // Result rows.
    if display_count == 0 {
        let mut no_match =
            shape_chrome_line(font_system, ctx, &view.empty_message, muted, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut no_match,
            (origin + pad_x, results_top as i32 + item_text_dy),
            muted,
        );
    } else {
        let item_pad = PALETTE_ITEM_PAD_X as i32;
        for (i, item) in view.items.iter().take(display_count).enumerate() {
            let row_y = (results_top + i as f32 * item_h) as i32 + item_text_dy;

            if item.match_positions.is_empty() || view.query.is_empty() {
                let mut label_buf =
                    shape_chrome_line(font_system, ctx, &item.label, foreground, true, true);
                composite_buffer(
                    font_system,
                    swash_cache,
                    &mut rgba,
                    canvas,
                    &mut label_buf,
                    (origin + pad_x + item_pad, row_y),
                    foreground,
                );
            } else {
                draw_highlighted_label(
                    font_system,
                    swash_cache,
                    ctx,
                    &mut rgba,
                    canvas,
                    &item.label,
                    &item.match_positions,
                    foreground,
                    match_color,
                    view.match_underline,
                    true,
                    (origin + pad_x + item_pad, row_y),
                );
            }

            let hint = if !item.shortcut.is_empty() {
                item.shortcut.as_str()
            } else if !item.action.is_empty() {
                item.action.as_str()
            } else {
                ""
            };
            if !hint.is_empty() {
                let mut hint_buf = shape_chrome_line(font_system, ctx, hint, muted, false, true);
                let hint_w = buffer_width(&hint_buf).ceil() as i32;
                composite_buffer(
                    font_system,
                    swash_cache,
                    &mut rgba,
                    canvas,
                    &mut hint_buf,
                    (origin + panel_w as i32 - hint_w - pad_x - item_pad, row_y),
                    muted,
                );
            }
        }
    }

    // Center horizontally; place in the upper third of the window.
    let palette_x = ((surface_w - panel_w) / 2.0).max(0.0);
    let palette_y = surface_h * PALETTE_TOP_RATIO;

    DropdownImage {
        height,
        rgba,
        width,
        x: (palette_x - margin).round(),
        y: (palette_y - margin).round(),
    }
}
/// Rasterize the which-key hint popup: an elevated card showing continuation keys
/// and labels when the user pauses mid-prefix.
pub(super) fn which_key_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &WhichKeyView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let pad_x = (ctx.cell_w * 1.5).max(14.0);
    let pad_y = (ctx.cell_h * 0.8).max(10.0);
    let title_h = ctx.line_height + 4.0;
    let item_h = (ctx.line_height + 4.0).round();

    let cols = if view.items.len() > 6 { 2 } else { 1 };
    let rows = view.items.len().div_ceil(cols);

    let col_w = (ctx.cell_w * 26.0).max(220.0);
    let panel_w = (pad_x * 2.0 + col_w * cols as f32)
        .min(surface_w - 40.0)
        .floor()
        .max(220.0);
    let panel_h = (pad_y * 2.0 + title_h + rows as f32 * item_h + 8.0)
        .min(surface_h - 40.0)
        .floor()
        .max(60.0);

    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel with hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Hairline divider under title.
    let divider_y = margin + pad_y + title_h;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin, divider_y, panel_w, 1.0),
        0.0,
        theme.divider,
        0.5,
    );

    let foreground = theme.foreground.to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();

    // Draw Title.
    let mut title_buf = shape_chrome_line(font_system, ctx, &view.title, accent, true, true);
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut title_buf,
        ((margin + pad_x) as i32, (margin + pad_y) as i32),
        accent,
    );

    // Draw Items.
    let items_top = divider_y + 6.0;
    for (i, (key, label)) in view.items.iter().enumerate() {
        let col = i / rows;
        let row = i % rows;
        let item_x = margin + pad_x + col as f32 * col_w;
        let item_y = items_top + row as f32 * item_h;

        let mut key_buf = shape_chrome_line(font_system, ctx, key, accent, true, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut key_buf,
            (item_x as i32, item_y as i32),
            accent,
        );

        let mut arrow_buf = shape_chrome_line(font_system, ctx, "→", muted, false, true);
        let key_w = ctx.cell_w * 7.0;
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut arrow_buf,
            ((item_x + key_w) as i32, item_y as i32),
            muted,
        );

        let mut label_buf = shape_chrome_line(font_system, ctx, label, foreground, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut label_buf,
            ((item_x + key_w + ctx.cell_w * 2.0) as i32, item_y as i32),
            foreground,
        );
    }

    let tip_x = ((surface_w - panel_w) * 0.5).max(0.0);
    let tip_y = ((surface_h - panel_h) * 0.5).max(0.0);

    DropdownImage {
        height,
        rgba,
        width,
        x: (tip_x - margin).round(),
        y: (tip_y - margin).round(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_which_key_rgba_is_centered_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1000.0;
        let surface_h = 800.0;

        let view = WhichKeyView {
            title: "g + ...".to_string(),
            items: vec![
                ("g".to_string(), "top of buffer".to_string()),
                ("v".to_string(), "restore visual".to_string()),
            ],
        };

        let image = which_key_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        // Centered horizontally: left margin plus card width / 2 is close to surface_w / 2
        let card_center_x = image.x + image.width as f32 / 2.0;
        assert!(
            (card_center_x - surface_w / 2.0).abs() < 50.0,
            "which-key card is roughly centered"
        );
        // Body is non-empty and opaque
        assert!(image.width > 100);
        assert!(image.height > 50);
        let center_idx = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[center_idx + 3], 255, "card interior is opaque");
    }
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
    fn test_toast_rgba_is_a_top_right_pill_below_the_tabbar() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1000.0;
        let top_inset = 40.0;

        let image = toast_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            "Copied to clipboard",
            theme.ansi[4],
            surface_w,
            top_inset,
        );

        // Anchored to the right: the pill's right edge is near the surface edge,
        // sitting in the right half of the surface rather than centered.
        assert!(image.x > surface_w / 2.0, "toast sits in the right half");
        assert!(
            image.x + image.width as f32 <= surface_w,
            "not clipped right"
        );
        // Dropped below the tab bar: its top is at or beneath the tab-bar height.
        assert!(image.y >= top_inset - ctx.line_height, "below the tabbar");
        // The panel's padding band is the opaque elevated surface color.
        let i = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[i + 3], 255, "pill body is opaque");
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

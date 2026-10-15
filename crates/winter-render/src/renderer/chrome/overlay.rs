//! Rasterizing each floating surface into an RGBA buffer.

use super::paint::{
    blend_px, buffer_width, composite_buffer, draw_highlighted_label, fill_rounded_rect,
    rounded_rect_sdf, shape_chrome_line,
};
use super::view::{DropdownImage, InputView, PaletteView, WhichKeyView};
use super::{
    DROPDOWN_RADIUS, DROPDOWN_SHADOW, DROPDOWN_SHADOW_ALPHA, INPUT_WIDTH_RATIO, MENU_HOVER_INSET,
    PALETTE_HEIGHT_RATIO, PALETTE_ITEM_PAD_X, PALETTE_ITEM_PAD_Y, PALETTE_JUMP_HEIGHT_RATIO,
    PALETTE_WIDTH_RATIO, SHADOW_COLOR,
};
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
    const RADIUS: f32 = 4.0;
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
    let border = theme.divider;
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

    let border = theme.divider;
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
    let border = theme.divider;
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
        let radius = 2.0;
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
    // A titled list is headed by what it is a list of, above the line the
    // filter is typed on; a palette that names nothing keeps its own height.
    let title_h = match view.title.is_empty() {
        true => 0.0,
        false => ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0,
    };
    let item_h = (ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0).round();
    // The panel is a fixed fraction of the window, and how many results fit
    // follows from its height: it does not resize as the query narrows the
    // list, which would walk its own rows out from under the eye.
    let panel_w = (surface_w * PALETTE_WIDTH_RATIO)
        .min(surface_w - 40.0)
        .floor()
        .max(300.0);
    let height_ratio = match view.align_top {
        true => PALETTE_JUMP_HEIGHT_RATIO,
        false => PALETTE_HEIGHT_RATIO,
    };
    let panel_h = (surface_h * height_ratio)
        .min(surface_h - 40.0)
        .floor()
        .max(inner_pad * 2.0 + title_h + input_h + 1.0 + item_h);
    let row_count =
        (((panel_h - inner_pad * 2.0 - title_h - input_h - 1.0) / item_h).floor() as usize).max(1);
    let total_items = view.items.len();
    let display_count = total_items.min(row_count);
    let start_idx = if total_items <= row_count {
        0
    } else {
        view.selected
            .saturating_sub(row_count / 2)
            .min(total_items.saturating_sub(row_count))
    };

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
    let border = theme.divider;
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
    let divider_y = margin + inner_pad + title_h + input_h;
    let results_top = divider_y + 1.0;
    if display_count > 0 {
        let sel = view
            .selected
            .saturating_sub(start_idx)
            .min(display_count.saturating_sub(1));
        // The highlight fills the row vertically (only a 1px breath) so it covers
        // the text and selected rows butt against their neighbours with no gap.
        let hover_v_inset = 1.0;
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            results_top + sel as f32 * item_h + hover_v_inset,
            panel_w - 2.0 * MENU_HOVER_INSET,
            item_h - 2.0 * hover_v_inset,
        );
        let radius = 2.0;
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
    let input_top = (margin + inner_pad + title_h) as i32;

    // The title, above the input line: what the list is of, which the key
    // that opened it is too far back to say by the time it is read.
    if !view.title.is_empty() {
        let mut title_buf = shape_chrome_line(font_system, ctx, &view.title, muted, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut title_buf,
            (
                origin + pad_x,
                (margin + inner_pad) as i32 + PALETTE_ITEM_PAD_Y as i32,
            ),
            muted,
        );
    }

    // The "❯" prompt opening the input line.
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
        for (i, item) in view
            .items
            .iter()
            .skip(start_idx)
            .take(display_count)
            .enumerate()
        {
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

    // Centred both ways, where the key-hint card sits: both are asking for
    // one keystroke's worth of attention, and a picker opened from the middle
    // of a view should not send the eye to the top of the window for it.
    let palette_x = ((surface_w - panel_w) / 2.0).max(0.0);
    let palette_y = match view.align_top {
        true => 0.0,
        false => ((surface_h - panel_h) / 2.0).max(0.0),
    };

    DropdownImage {
        height,
        rgba,
        width,
        x: (palette_x - margin).round(),
        y: (palette_y - margin).round(),
    }
}
/// The signature royal blue active indicator used across Winter's chrome tabs.
const POPUP_ACTIVE_ACCENT: Rgb = Rgb::new(0x1d, 0x4e, 0xd8);

/// Rasterize the which-key hint popup: a modern, content-proportional card showing
/// continuation keys in sleek keycap badges alongside their action descriptions.
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
    let pad_x = (ctx.cell_w * 1.5).max(16.0);
    let pad_y = (ctx.cell_h * 0.8).max(12.0);
    let title_h = ctx.line_height + 4.0;
    let item_h = (ctx.line_height + 6.0).round();
    let row_gap = 2.0;

    let foreground = theme.foreground.to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();

    // 1. Column and row layout: determine optimal columns for readability.
    let count = view.items.len();
    let cols = match count {
        0..=6 => 1,
        7..=14 => 2,
        _ => 3.min(count.div_ceil(8)),
    };
    let rows = count.div_ceil(cols).max(1);

    // 2. Shape headers and item lines to compute natural content dimensions.
    let mut title_buf = shape_chrome_line(font_system, ctx, &view.title, foreground, true, true);
    let title_w = buffer_width(&title_buf).ceil();

    let dismiss_text = "Esc to cancel";
    let mut hint_buf = shape_chrome_line(font_system, ctx, dismiss_text, muted, false, true);
    let hint_w = buffer_width(&hint_buf).ceil();
    let header_min_w = title_w + hint_w + 32.0;

    // Measure key badges and labels
    let mut shaped_items = Vec::with_capacity(count);
    let mut max_item_w: f32 = 0.0;
    for (key, label) in &view.items {
        let key_buf = shape_chrome_line(font_system, ctx, key, accent, true, true);
        let key_text_w = buffer_width(&key_buf).ceil();
        let badge_w = (key_text_w + 12.0).max(22.0);

        let label_buf = shape_chrome_line(font_system, ctx, label, foreground, false, true);
        let label_w = buffer_width(&label_buf).ceil();

        let total_item_w = badge_w + 10.0 + label_w;
        if total_item_w > max_item_w {
            max_item_w = total_item_w;
        }
        shaped_items.push((key_buf, key_text_w, badge_w, label_buf));
    }

    // Column width accommodates the widest item comfortably
    let min_col_w = (max_item_w + 16.0).max(200.0);
    let natural_w = cols as f32 * min_col_w + pad_x * 2.0;
    let panel_w = natural_w
        .max(header_min_w + pad_x * 2.0)
        .min(surface_w - 40.0)
        .floor()
        .max(260.0);
    let col_w = (panel_w - pad_x * 2.0) / cols as f32;

    let content_h = pad_y * 2.0 + title_h + 8.0 + rows as f32 * (item_h + row_gap);
    let panel_h = content_h.min(surface_h - 40.0).floor().max(70.0);

    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // 3. Drop shadow
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

    // 4. Elevated rounded panel with hairline border
    let border = theme.divider;
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Winter Term signature royal blue top accent indicator
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, 2.0),
        1.0,
        POPUP_ACTIVE_ACCENT,
        1.0,
    );

    // 5. Header: title + dismiss hint + divider
    let title_y = margin + pad_y;
    // Accent indicator dot before title
    let dot_size = 5.0;
    let dot_y = title_y + (title_h - dot_size) / 2.0;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + pad_x, dot_y, dot_size, dot_size),
        dot_size / 2.0,
        POPUP_ACTIVE_ACCENT,
        1.0,
    );

    let title_x = margin + pad_x + dot_size + 8.0;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut title_buf,
        (title_x as i32, title_y as i32),
        foreground,
    );

    // Right-aligned dismiss hint
    let hint_x = margin + panel_w - pad_x - hint_w;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut hint_buf,
        (hint_x as i32, title_y as i32),
        muted,
    );

    // Hairline divider under title
    let divider_y = title_y + title_h + 4.0;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + pad_x, divider_y, panel_w - pad_x * 2.0, 1.0),
        0.0,
        theme.divider,
        0.5,
    );

    // 6. Draw items with keycap badges and styled rows
    let items_top = divider_y + 8.0;
    for (i, (mut key_buf, key_text_w, badge_w, mut label_buf)) in
        shaped_items.into_iter().enumerate()
    {
        let col = i / rows;
        let row = i % rows;
        let item_x = margin + pad_x + col as f32 * col_w;
        let item_y = items_top + row as f32 * (item_h + row_gap);

        // Subtle row card container
        let row_w = col_w - 8.0;
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (item_x, item_y, row_w, item_h),
            3.0,
            theme.menu_hover_bg,
            0.3,
        );

        // Keycap badge (<kbd> pill)
        let badge_h = (ctx.line_height + 2.0).round();
        let badge_x = item_x + 5.0;
        let badge_y = item_y + (item_h - badge_h) / 2.0;

        // Keycap border & background
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (badge_x, badge_y, badge_w, badge_h),
            3.0,
            theme.divider,
            0.8,
        );
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (badge_x + 1.0, badge_y + 1.0, badge_w - 2.0, badge_h - 2.0),
            2.0,
            theme.menu_bg,
            1.0,
        );

        // Center key text inside keycap
        let key_x = badge_x + (badge_w - key_text_w) / 2.0;
        let key_y = badge_y + (badge_h - ctx.line_height) / 2.0;
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut key_buf,
            (key_x.round() as i32, key_y.round() as i32),
            accent,
        );

        // Label beside badge
        let label_x = badge_x + badge_w + 8.0;
        let label_y = item_y + (item_h - ctx.line_height) / 2.0;
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut label_buf,
            (label_x.round() as i32, label_y.round() as i32),
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

/// Rasterize the input dialog: a centered floating panel holding one question,
/// the line it is answered on, and how to answer it. The shape the command
/// palette has, minus the list, since there is nothing to choose from.
pub(super) fn input_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &InputView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let inner_pad = ctx.cell_h * 0.6;
    let label_h = ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0;
    let input_h = match view.input {
        Some(_) => ctx.cell_h * 2.0,
        None => 0.0,
    };
    let hint_h = match view.hint.is_empty() {
        true => 0.0,
        false => ctx.line_height + PALETTE_ITEM_PAD_Y,
    };

    let panel_w = (surface_w * INPUT_WIDTH_RATIO)
        .min(surface_w - 40.0)
        .floor()
        .max(280.0);
    let panel_h = (inner_pad * 2.0 + label_h + input_h + hint_h).ceil();

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
    let border = theme.divider;
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    let pad_x = (ctx.cell_w * 1.0) as i32;
    let origin = margin as i32;

    // The question itself, at the top, in the ordinary text color: it is what
    // is being read, where the palette's own heading only names a list.
    let mut label_buf = shape_chrome_line(font_system, ctx, &view.label, foreground, false, true);
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &mut label_buf,
        (
            origin + pad_x,
            (margin + inner_pad) as i32 + PALETTE_ITEM_PAD_Y as i32,
        ),
        foreground,
    );

    // The line being typed on, under it, opened by the palette's own mark so
    // the two read as the same kind of thing.
    if let Some(input) = &view.input {
        let input_top = (margin + inner_pad + label_h) as i32;
        let input_text_dy = ((input_h - ctx.line_height) / 2.0).max(0.0) as i32;

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

        let mut input_buf = shape_chrome_line(font_system, ctx, input, foreground, true, true);
        let input_w = buffer_width(&input_buf).ceil() as i32;
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut input_buf,
            (origin + pad_x + prompt_w, input_top + input_text_dy),
            foreground,
        );

        // A bar drawn directly rather than a glyph, which would sit centered
        // in a cell box and leave a gap after the last character typed. One
        // inside the text sits after the characters before it, measured by
        // shaping them alone.
        let before: String = input.chars().take(view.caret).collect();
        let caret_w = match before.len() == input.len() {
            true => input_w,
            false => {
                let before_buf =
                    shape_chrome_line(font_system, ctx, &before, foreground, true, true);
                buffer_width(&before_buf).ceil() as i32
            }
        };
        let caret_x = (origin + pad_x + prompt_w + caret_w) as f32 + 1.0;
        let caret_y = (input_top + input_text_dy) as f32 + 1.0;
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (caret_x, caret_y, 2.0, (ctx.line_height - 2.0).max(1.0)),
            1.0,
            theme.cursor_bg,
            1.0,
        );
    }

    // How to answer, at the foot: a dialog that takes one key has to say which.
    if !view.hint.is_empty() {
        let hint_top = (margin + inner_pad + label_h + input_h) as i32;
        let mut hint_buf = shape_chrome_line(font_system, ctx, &view.hint, muted, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut hint_buf,
            (origin + pad_x, hint_top),
            muted,
        );
    }

    // Centred both ways, where the palette and the key-hint card sit: a
    // question interrupts whatever was being read, so it belongs where the
    // eye already is rather than at an edge.
    let x = ((surface_w - panel_w) / 2.0).max(0.0);
    let y = ((surface_h - panel_h) / 2.0).max(0.0);

    DropdownImage {
        height,
        rgba,
        width,
        x: (x - margin).round(),
        y: (y - margin).round(),
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
    fn test_the_input_dialog_is_centered_and_only_as_tall_as_it_needs() {
        // It holds one question, so it is sized to the question: the palette's
        // fixed height would put a filename prompt in a panel most of which is
        // the empty list it does not have.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);

        let typed = InputView {
            caret: 8,
            hint: "Enter to accept, Esc to cancel".to_string(),
            input: Some("notes.md".to_string()),
            label: "New file".to_string(),
        };
        let image = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &typed,
            surface_w,
            surface_h,
        );
        let center_x = image.x + image.width as f32 / 2.0;
        let center_y = image.y + image.height as f32 / 2.0;
        assert!((center_x - surface_w / 2.0).abs() < 2.0, "centered across");
        assert!((center_y - surface_h / 2.0).abs() < 2.0, "and down");
        assert!(
            (image.height as f32) < surface_h * 0.3,
            "a question is not a panel: {} of {surface_h}",
            image.height
        );
        let middle = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[middle + 3], 255, "the panel interior is opaque");

        // A question answered by one key has no line to type on, so it is
        // shorter than one that does.
        let confirmed = InputView {
            caret: 0,
            hint: "y to confirm, any other key to cancel".to_string(),
            input: None,
            label: "Delete 3 entries?".to_string(),
        };
        let shorter = input_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &confirmed,
            surface_w,
            surface_h,
        );
        assert!(
            shorter.height < image.height,
            "{} is not shorter than {}",
            shorter.height,
            image.height
        );
    }

    #[test]
    fn test_the_picker_is_one_size_however_much_the_query_matches() {
        // Typing narrows the list, and a panel sized to what is left would
        // shrink under every keystroke, moving the rows out from under the
        // eye reading them.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let mut card = |matches: usize| {
            let view = PaletteView {
                align_top: false,
                empty_message: "Nothing matches".to_string(),
                items: (0..matches)
                    .map(|i| crate::renderer::chrome::view::PaletteItem {
                        action: i.to_string(),
                        label: format!("entry {i}"),
                        match_positions: Vec::new(),
                        shortcut: String::new(),
                    })
                    .collect(),
                match_underline: false,
                query: String::new(),
                selected: 0,
                title: String::new(),
            };
            let image = palette_rgba(
                &mut font_system,
                &mut swash,
                &ctx,
                &theme,
                &view,
                surface_w,
                surface_h,
            );
            (image.width, image.height, image.y)
        };

        let full = card(40);
        assert_eq!(card(3), full, "three matches draw the box a full list does");
        assert_eq!(card(0), full, "and so does none");
        assert!(
            (full.0 as f32 - (surface_w * PALETTE_WIDTH_RATIO + 2.0 * DROPDOWN_SHADOW)).abs() < 2.0,
            "and it is sized against the window, got {full:?}"
        );
    }

    #[test]
    fn test_palette_align_top_docked_at_window_top_with_half_height() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let view = PaletteView {
            align_top: true,
            empty_message: "No targets".to_string(),
            items: (0..20)
                .map(|i| crate::renderer::chrome::view::PaletteItem {
                    action: i.to_string(),
                    label: format!("line {i}"),
                    match_positions: Vec::new(),
                    shortcut: String::new(),
                })
                .collect(),
            match_underline: false,
            query: String::new(),
            selected: 0,
            title: "Jump: Buffer".to_string(),
        };
        let image = palette_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        // Aligned at the top: y equals -DROPDOWN_SHADOW so panel rect begins at y = 0.0
        assert_eq!(image.y, -DROPDOWN_SHADOW);
        // Height is half window height + 2 * shadow margin
        let expected_panel_h = (surface_h * PALETTE_JUMP_HEIGHT_RATIO).floor();
        let expected_total_h = (expected_panel_h + 2.0 * DROPDOWN_SHADOW) as u32;
        assert_eq!(image.height, expected_total_h);
    }

    #[test]
    fn test_the_key_hint_card_is_content_adaptive_and_compact() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let mut card = |count: usize| {
            let view = WhichKeyView {
                title: "Branch".to_string(),
                items: (0..count)
                    .map(|i| (i.to_string(), format!("choice {i}")))
                    .collect(),
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
            (image.width, image.height)
        };

        let small = card(2);
        let large = card(15);
        assert!(
            small.0 < large.0 || small.1 < large.1,
            "small menu is more compact than 15-item menu, got small={small:?} large={large:?}"
        );
        // It stays bounded within the surface margins
        assert!(large.0 as f32 <= surface_w + 2.0 * DROPDOWN_SHADOW);
        assert!(large.1 as f32 <= surface_h + 2.0 * DROPDOWN_SHADOW);
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

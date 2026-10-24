//! The picker: a query line over a fuzzy-filtered list.

use super::view::PickerView;
use super::MENU_HOVER_INSET;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{
    blit_svg, buffer_width, draw_highlighted_label, shape_chrome_line,
};
use crate::renderer::raster::popup::{
    centered, draw_caret, draw_divider, title_bar_height, PopupFrame, TITLE_BAR_TOP_PAD,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::{
    DROPDOWN_SHADOW, ICON_GAP_CELLS, ICON_HEIGHT_RATIO, PALETTE_ITEM_PAD_Y,
};
use crate::theme::Theme;
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Constants
// ========================================================================

/// The palette's width and height as fractions of the surface.
///
/// A fixed size, as the key-hint card is: the list narrows as the query is
/// typed, and a panel that shrank with it would walk its own rows out from
/// under the eye reading them. How many results are visible follows from the
/// height rather than from a count of its own, so a taller window shows more
/// of them.
const PALETTE_WIDTH_RATIO: f32 = 0.7;

const PALETTE_HEIGHT_RATIO: f32 = 0.6;

/// The palette's height as a fraction of the surface when docked at the top for Jump mode.
const PALETTE_JUMP_HEIGHT_RATIO: f32 = 0.5;

/// Extra horizontal padding inside each palette result row, in pixels, applied
/// to both the left label and the right shortcut hint.
const PALETTE_ITEM_PAD_X: f32 = 5.0;

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the picker: a centered floating panel with a search input
/// at the top and a fuzzy-filtered list of items below. Styled like the
/// dropdown but wider and vertically centered in the upper portion of the window.
pub(in crate::renderer) fn picker_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let inner_pad = ctx.cell_h * 0.5;
    let item_h = (ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0).round();
    // The line typed on is a row like the title above it and the results
    // below it, so the three stack as one rhythm.
    let input_h = item_h;
    // A titled list is headed by what it is a list of, above the line the
    // filter is typed on; a palette that names nothing keeps its own height.
    let (top_pad, title_h) = match view.title.is_empty() {
        true => (inner_pad, 0.0),
        false => (TITLE_BAR_TOP_PAD, title_bar_height(ctx)),
    };
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
        .max(top_pad + inner_pad + title_h + input_h + 1.0 + item_h);
    let row_count = (((panel_h - top_pad - inner_pad - title_h - input_h - 1.0) / item_h).floor()
        as usize)
        .max(1);
    let total_items = view.items.len();
    let display_count = total_items.min(row_count);
    let start_idx = if total_items <= row_count {
        0
    } else {
        view.selected
            .saturating_sub(row_count / 2)
            .min(total_items.saturating_sub(row_count))
    };

    let mut frame = PopupFrame::new(theme, panel_w, panel_h, margin);

    // Hover highlight on the selected result row.
    let divider_y = margin + top_pad + title_h + input_h;
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
        frame.fill(hover_rect, radius, theme.menu_hover_bg, 1.0);
    }

    // Hairlines between the title and the input, and between the input and
    // the results, so the three read as the parts they are.
    if !view.title.is_empty() {
        draw_divider(&mut frame, theme, margin + top_pad + title_h, panel_w);
    }
    draw_divider(&mut frame, theme, divider_y, panel_w);

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    // Fuzzy-matched characters are tinted blue to stand out from the label.
    let match_color = theme.ansi[4].to_glyphon();
    let pad_x = (ctx.cell_w * 1.0) as i32;
    let origin = margin as i32;
    let input_text_dy = ((input_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let item_text_dy = ((item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let input_top = (margin + top_pad + title_h) as i32;
    // Icons get a column of their own when any entry has one, so every label
    // starts on one line whether it has an icon or not.
    let icon_px = (ctx.cell_h * ICON_HEIGHT_RATIO).round().max(1.0) as u32;
    let label_indent = match view.items.iter().any(|item| item.icon.is_some()) {
        true => icon_px as i32 + (ctx.cell_w * ICON_GAP_CELLS).round() as i32,
        false => 0,
    };

    // The title bar, above the input line: what the list is of, which the key
    // that opened it is too far back to say by the time it is read.
    if !view.title.is_empty() {
        frame.draw_title_bar(font_system, swash_cache, ctx, theme, &view.title, "");
    }

    // Query text, from the panel's first column, where a dialog's input line
    // starts too: the caret of an empty query sits at the edge rather than
    // floating over an icon column.
    let query_x = origin + pad_x;
    let mut query_buf = shape_chrome_line(font_system, ctx, &view.query, foreground, true, true);
    let query_w = buffer_width(&query_buf).ceil() as i32;
    frame.draw_text(
        font_system,
        swash_cache,
        &mut query_buf,
        (query_x, input_top + input_text_dy),
        foreground,
    );

    // Caret: a thin vertical bar after the characters before it. A glyph caret
    // would sit centered in a full cell box, leaving an unnatural gap from the
    // last character, so draw the bar directly instead; one inside the query
    // is placed by shaping the characters before it alone.
    if view.caret_visible {
        let before: String = view.query.chars().take(view.caret).collect();
        let caret_w = match before.len() == view.query.len() {
            true => query_w,
            false => {
                let before_buf =
                    shape_chrome_line(font_system, ctx, &before, foreground, true, true);
                buffer_width(&before_buf).ceil() as i32
            }
        };
        let caret_x = (query_x + caret_w) as f32 + 1.0;
        let caret_y = (input_top + input_text_dy) as f32 + 1.0;
        draw_caret(&mut frame, ctx, theme, (caret_x, caret_y));
    }

    // Result rows.
    if display_count == 0 {
        let mut no_match =
            shape_chrome_line(font_system, ctx, &view.empty_message, muted, false, true);
        frame.draw_text(
            font_system,
            swash_cache,
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
            let row_top = results_top + i as f32 * item_h;
            let row_y = row_top as i32 + item_text_dy;
            let icon_x = origin + pad_x + item_pad;
            let label_x = icon_x + label_indent;

            if let Some(svg) = &item.icon {
                let icon_y = (row_top + (item_h - icon_px as f32) / 2.0).round() as i32;
                let (rgba, canvas) = frame.pixels();
                blit_svg(rgba, canvas, svg, (icon_x, icon_y), icon_px);
            }

            if item.match_positions.is_empty() || view.query.is_empty() {
                let mut label_buf =
                    shape_chrome_line(font_system, ctx, &item.label, foreground, true, true);
                frame.draw_text(
                    font_system,
                    swash_cache,
                    &mut label_buf,
                    (label_x, row_y),
                    foreground,
                );
            } else {
                let (rgba, canvas) = frame.pixels();
                draw_highlighted_label(
                    font_system,
                    swash_cache,
                    ctx,
                    rgba,
                    canvas,
                    &item.label,
                    &item.match_positions,
                    foreground,
                    match_color,
                    view.match_underline,
                    true,
                    (label_x, row_y),
                );
            }

            let hint = if !item.shortcut.is_empty() {
                item.shortcut.as_str()
            } else {
                ""
            };
            if !hint.is_empty() {
                let mut hint_buf = shape_chrome_line(font_system, ctx, hint, muted, false, true);
                let hint_w = buffer_width(&hint_buf).ceil() as i32;
                frame.draw_text(
                    font_system,
                    swash_cache,
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
    let (palette_x, centred_y) = centered(surface_w, surface_h, panel_w, panel_h);
    let palette_y = match view.align_top {
        true => 0.0,
        false => centred_y,
    };
    frame.into_image(palette_x, palette_y)
}

pub(in crate::renderer) use picker_rgba as palette_rgba;

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

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
            let view = PickerView {
                align_top: false,
                caret: 0,
                caret_visible: true,
                empty_message: "Nothing matches".to_string(),
                items: (0..matches)
                    .map(|i| crate::renderer::widgets::PickerItem {
                        action: i.to_string(),
                        icon: None,
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
        let view = PickerView {
            align_top: true,
            caret: 0,
            caret_visible: true,
            empty_message: "No targets".to_string(),
            items: (0..20)
                .map(|i| crate::renderer::widgets::PickerItem {
                    action: i.to_string(),
                    icon: None,
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

    /// A picker with a few rows, titled `title`, its caret shown or not.
    fn sample_picker(title: &str, caret_visible: bool) -> PickerView {
        PickerView {
            align_top: false,
            caret: 0,
            caret_visible,
            empty_message: String::new(),
            items: (0..3)
                .map(|i| crate::renderer::widgets::PickerItem {
                    action: i.to_string(),
                    icon: None,
                    label: format!("entry {i}"),
                    match_positions: Vec::new(),
                    shortcut: String::new(),
                })
                .collect(),
            match_underline: false,
            query: "ent".to_string(),
            selected: 0,
            title: title.to_string(),
        }
    }

    fn render_picker(view: &PickerView) -> DropdownImage {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        palette_rgba(
            &mut font_system,
            &mut swash,
            &sample_font_ctx(),
            &Theme::dark(),
            view,
            1000.0,
            800.0,
        )
    }

    #[test]
    fn test_the_palette_caret_is_drawn_only_in_its_shown_phase() {
        let shown = render_picker(&sample_picker("", true));
        let hidden = render_picker(&sample_picker("", false));
        assert_eq!((shown.width, shown.height), (hidden.width, hidden.height));
        assert_ne!(shown.rgba, hidden.rgba, "the caret blinks off");
    }

    #[test]
    fn test_a_titled_palette_draws_a_divider_between_its_title_and_its_input() {
        let ctx = sample_font_ctx();
        let image = render_picker(&sample_picker("Files", true));
        let y = (DROPDOWN_SHADOW + TITLE_BAR_TOP_PAD + title_bar_height(&ctx)).round() as u32;
        // Sampled near the right edge, clear of the title and the input text.
        let x = image.width - DROPDOWN_SHADOW as u32 - 4;
        let pixel = |y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            image.rgba[i..i + 3].to_vec()
        };
        assert_ne!(pixel(y), pixel(y + 3), "a hairline at the title's foot");
    }
}

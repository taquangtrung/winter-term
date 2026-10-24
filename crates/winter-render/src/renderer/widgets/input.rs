//! The input dialog: a question, the field it is answered in, and the keys
//! that answer it, or a confirmation answered by one key.

use super::super::colors::mix_rgb;
use super::button::{draw_buttons_row, measure_buttons_row};
use super::view::{ButtonView, InputView};
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line, truncate_label};
use crate::renderer::raster::popup::{
    centered, draw_caret, draw_field_well, PopupFrame, INPUT_FIELD_PAD_RATIO,
    INPUT_FIELD_TEXT_INSET,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::Theme;
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Constants
// ========================================================================

/// The input dialog's width as a fraction of the surface.
///
/// Narrower than the palette: it holds one line of an answer rather than a
/// list of choices, and a box the width of the window for a filename reads as
/// a panel rather than as a question.
const INPUT_WIDTH_RATIO: f32 = 0.5;

/// The input dialog's padding inside its card, in cell heights.
const INPUT_PAD_RATIO: f32 = 0.9;

/// The space between the dialog's question, field, and keys, in cell heights.
const INPUT_GAP_RATIO: f32 = 0.6;

/// A keycap's padding around its key: across in cell widths, down in pixels.
const KEYCAP_PAD_X_RATIO: f32 = 0.5;
const KEYCAP_PAD_Y: f32 = 2.0;

/// A keycap's corner radius, in pixels.
const KEYCAP_RADIUS: f32 = 4.0;

/// How far a keycap leans from the card's color to the text's.
const KEYCAP_MIX: f32 = 0.14;

/// The space after a keycap before what it does, and between two keys, in
/// cell widths.
const KEYCAP_LABEL_GAP_RATIO: f32 = 0.5;
const KEYCAP_PAIR_GAP_RATIO: f32 = 1.5;

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the input dialog: a centered floating card holding one
/// question in bold, the field it is answered in, and the keys that answer
/// it as keycaps along the foot, right-aligned the way a modern dialog puts
/// its buttons. A question answered by one key, with no field, is a
/// confirmation, drawn as one (see [`confirm_rgba`]).
pub(in crate::renderer) fn input_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &InputView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let has_buttons = !view.buttons.is_empty();
    if (view.input.is_none() && !view.keys.is_empty()) || has_buttons {
        return confirm_rgba(
            font_system,
            swash_cache,
            ctx,
            theme,
            view,
            surface_w,
            surface_h,
        );
    }
    let margin = DROPDOWN_SHADOW;
    let pad = (ctx.cell_h * INPUT_PAD_RATIO).round();
    let gap = (ctx.cell_h * INPUT_GAP_RATIO).round();
    let field_pad_y = (ctx.cell_h * INPUT_FIELD_PAD_RATIO).round();
    let field_h = match view.input {
        Some(_) => ctx.line_height + field_pad_y * 2.0,
        None => 0.0,
    };
    let keycap_h = ctx.line_height + KEYCAP_PAD_Y * 2.0;
    let keys_h = match view.keys.is_empty() {
        true => 0.0,
        false => keycap_h,
    };
    // Each part below the question stands a gap below the one before it.
    let below: f32 = [field_h, keys_h]
        .iter()
        .filter(|&&h| h > 0.0)
        .map(|h| h + gap)
        .sum();

    let panel_w = (surface_w * INPUT_WIDTH_RATIO)
        .min(surface_w - 40.0)
        .floor()
        .max(280.0);
    let panel_h = (pad * 2.0 + ctx.line_height + below).ceil();

    let mut frame = PopupFrame::new(theme, panel_w, panel_h, margin);
    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let left = margin + pad;
    let right = margin + panel_w - pad;
    let mut y = margin + pad;

    // The question, in bold: it is the dialog's heading.
    let mut label_buf = shape_chrome_line(font_system, ctx, &view.label, foreground, true, true);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut label_buf,
        (left as i32, y as i32),
        foreground,
    );
    y += ctx.line_height + gap;

    // The field answered in.
    if let Some(input) = &view.input {
        draw_field_well(&mut frame, theme, (left, y, right - left, field_h));

        let text_x = (left + ctx.cell_w * INPUT_FIELD_TEXT_INSET) as i32;
        let text_y = (y + field_pad_y) as i32;
        let mut input_buf = shape_chrome_line(font_system, ctx, input, foreground, false, true);
        let input_w = buffer_width(&input_buf).ceil() as i32;
        frame.draw_text(
            font_system,
            swash_cache,
            &mut input_buf,
            (text_x, text_y),
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
                    shape_chrome_line(font_system, ctx, &before, foreground, false, true);
                buffer_width(&before_buf).ceil() as i32
            }
        };
        if view.caret_visible {
            let caret = ((text_x + caret_w) as f32 + 1.0, text_y as f32 + 1.0);
            draw_caret(&mut frame, ctx, theme, caret);
        }
        y += field_h + gap;
    }

    // The keys that answer, each a keycap with what it does beside it,
    // laid out from the right edge in.
    if !view.keys.is_empty() {
        let keycap_bg = mix_rgb(theme.menu_bg, theme.foreground, KEYCAP_MIX);
        let mut shaped: Vec<(glyphon::Buffer, f32, glyphon::Buffer, f32)> = view
            .keys
            .iter()
            .map(|(key, does)| {
                let key_buf = shape_chrome_line(font_system, ctx, key, foreground, true, true);
                let does_buf = shape_chrome_line(font_system, ctx, does, muted, false, true);
                let key_w = buffer_width(&key_buf).ceil();
                let does_w = buffer_width(&does_buf).ceil();
                (key_buf, key_w, does_buf, does_w)
            })
            .collect();
        let keycap_pad_x = (ctx.cell_w * KEYCAP_PAD_X_RATIO).round();
        let label_gap = (ctx.cell_w * KEYCAP_LABEL_GAP_RATIO).round();
        let pair_gap = (ctx.cell_w * KEYCAP_PAIR_GAP_RATIO).round();
        let widths: Vec<f32> = shaped
            .iter()
            .map(|(_, key_w, _, does_w)| key_w + keycap_pad_x * 2.0 + label_gap + does_w)
            .collect();
        let total = widths.iter().sum::<f32>() + pair_gap * (widths.len() - 1) as f32;
        let mut x = (right - total).max(left);
        for ((key_buf, key_w, does_buf, _), width) in shaped.iter_mut().zip(&widths) {
            let keycap_w = *key_w + keycap_pad_x * 2.0;
            frame.fill((x, y, keycap_w, keycap_h), KEYCAP_RADIUS, keycap_bg, 1.0);
            frame.draw_text(
                font_system,
                swash_cache,
                key_buf,
                ((x + keycap_pad_x) as i32, (y + KEYCAP_PAD_Y) as i32),
                foreground,
            );
            frame.draw_text(
                font_system,
                swash_cache,
                does_buf,
                ((x + keycap_w + label_gap) as i32, (y + KEYCAP_PAD_Y) as i32),
                muted,
            );
            x += width + pair_gap;
        }
    }

    // Centred both ways, where the palette and the key-hint card sit: a
    // question interrupts whatever was being read, so it belongs where the
    // eye already is rather than at an edge.
    let (x, y) = centered(surface_w, surface_h, panel_w, panel_h);
    frame.into_image(x, y)
}

/// Rasterize a confirmation: a centered floating card holding the question
/// in bold and interactive buttons along the foot. Consistent with the rest
/// of the window chrome without an emphasis leftbar or badge.
fn confirm_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &InputView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let pad = (ctx.cell_h * INPUT_PAD_RATIO).round();
    let gap = (ctx.cell_h * INPUT_GAP_RATIO).round();

    let buttons: Vec<ButtonView> = if !view.buttons.is_empty() {
        view.buttons.clone()
    } else {
        view.keys
            .iter()
            .enumerate()
            .map(|(i, (k, does))| ButtonView {
                focused: i == 0,
                primary: i == 0,
                shortcut: Some(k.clone()),
                text: does.clone(),
            })
            .collect()
    };

    let (_buttons_w, button_h) = measure_buttons_row(font_system, ctx, &buttons);

    let panel_w = (surface_w * INPUT_WIDTH_RATIO)
        .min(surface_w - 40.0)
        .floor()
        .max(280.0);
    let panel_h = (pad * 2.0 + ctx.line_height + gap + button_h).ceil();
    let mut frame = PopupFrame::new(theme, panel_w, panel_h, margin);

    let foreground = theme.foreground.to_glyphon();
    let left = margin + pad;
    let right = margin + panel_w - pad;
    let top = margin + pad;

    // The question in bold, consistent with input_rgba
    let label_room = ((right - left) / ctx.cell_w).floor().max(1.0) as usize;
    let label = truncate_label(&view.label, label_room);
    let mut label_buf = shape_chrome_line(font_system, ctx, &label, foreground, true, true);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut label_buf,
        (left as i32, top as i32),
        foreground,
    );

    // The buttons laid out along the foot, right-aligned
    let button_y = top + ctx.line_height + gap;
    draw_buttons_row(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        theme,
        &buttons,
        right,
        button_y,
    );

    let (x, y) = centered(surface_w, surface_h, panel_w, panel_h);
    frame.into_image(x, y)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;
    use crate::theme::Rgb;

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
            buttons: Vec::new(),
            caret: 8,
            caret_visible: true,
            input: Some("notes.md".to_string()),
            keys: vec![
                ("Enter".to_string(), "accept".to_string()),
                ("Esc".to_string(), "cancel".to_string()),
            ],
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
            buttons: Vec::new(),
            caret: 0,
            caret_visible: true,
            input: None,
            keys: vec![
                ("y".to_string(), "Yes".to_string()),
                ("any key".to_string(), "No".to_string()),
            ],
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
    fn test_a_confirmation_has_no_left_stripe_and_focused_button_is_filled() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let view = |input: Option<&str>| InputView {
            buttons: vec![
                ButtonView {
                    focused: true,
                    primary: true,
                    shortcut: Some("y".to_string()),
                    text: "Yes".to_string(),
                },
                ButtonView {
                    focused: false,
                    primary: false,
                    shortcut: Some("n".to_string()),
                    text: "No".to_string(),
                },
            ],
            caret: 0,
            caret_visible: false,
            input: input.map(String::from),
            keys: Vec::new(),
            label: "Delete 3 entries?".to_string(),
        };
        let draw = |font_system: &mut FontSystem, swash: &mut SwashCache, view: &InputView| {
            input_rgba(font_system, swash, &ctx, &theme, view, 1000.0, 800.0)
        };
        let has = |image: &DropdownImage, color: Rgb| {
            image
                .rgba
                .chunks_exact(4)
                .any(|px| px[3] == 255 && (px[0], px[1], px[2]) == (color.r, color.g, color.b))
        };
        let edge = |image: &DropdownImage| {
            let (x, y) = (DROPDOWN_SHADOW as u32 + 1, image.height / 2);
            let i = ((y * image.width + x) * 4) as usize;
            Rgb::new(image.rgba[i], image.rgba[i + 1], image.rgba[i + 2])
        };

        let confirm = draw(&mut font_system, &mut swash, &view(None));
        let typed = draw(&mut font_system, &mut swash, &view(Some("x")));
        let warning = theme.ansi[3];
        assert_ne!(edge(&confirm), warning, "no left stripe on confirmation");
        assert_ne!(edge(&typed), warning, "a typed question is not marked");
        assert!(
            has(&confirm, theme.active_indicator),
            "the focused button is filled with accent"
        );
    }
}

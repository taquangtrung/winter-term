//! Modular button rendering: measuring, laying out, and drawing buttons
//! in dialogs and cards.

use glyphon::{FontSystem, SwashCache};

use super::view::ButtonView;
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line};
use crate::renderer::raster::chrome::Chrome;
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// Padding around button text: across in cell widths, down in cell heights.
pub(crate) const BUTTON_PAD_X_RATIO: f32 = 1.0;
pub(crate) const BUTTON_PAD_Y_RATIO: f32 = 0.25;

/// Corner radius of a button, in pixels.
pub(crate) const BUTTON_RADIUS: f32 = 4.0;

/// Gap between two adjacent buttons, in cell widths.
pub(crate) const BUTTON_GAP_RATIO: f32 = 0.8;

/// Outline color mix factor when a button is unfocused and not primary.
pub(crate) const BUTTON_OUTLINE_MIX: f32 = 0.28;

/// The brightness threshold `0.0..=1.0` over which a filled button takes dark text.
const LIGHT_FILL_LUMA: f32 = 0.6;

// ========================================================================
// Free functions
// ========================================================================

/// Measure a single button: returns `(width, height)` in pixels.
pub(crate) fn measure_button(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    button: &ButtonView,
) -> (f32, f32) {
    let dummy_color = glyphon::Color::rgb(0, 0, 0);
    let text_buf = shape_chrome_line(font_system, ctx, &button.text, dummy_color, true, true);
    let text_w = buffer_width(&text_buf).ceil();
    let pad_x = (ctx.cell_w * BUTTON_PAD_X_RATIO).round();
    let pad_y = (ctx.cell_h * BUTTON_PAD_Y_RATIO).round();
    let w = pad_x * 2.0 + text_w;
    let h = ctx.line_height + pad_y * 2.0;
    (w, h)
}

/// Measure a slice of buttons laid out horizontally in a row: returns `(total_width, row_height)`.
pub(crate) fn measure_buttons_row(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    buttons: &[ButtonView],
) -> (f32, f32) {
    if buttons.is_empty() {
        return (0.0, 0.0);
    }
    let gap = (ctx.cell_w * BUTTON_GAP_RATIO).round();
    let mut total_w = 0.0;
    let mut max_h = 0.0f32;
    for (i, btn) in buttons.iter().enumerate() {
        let (w, h) = measure_button(font_system, ctx, btn);
        total_w += w;
        if i + 1 < buttons.len() {
            total_w += gap;
        }
        max_h = max_h.max(h);
    }
    (total_w, max_h)
}

/// Draw a single button at rectangle `(x, y, w, h)`.
pub(crate) fn draw_button(
    frame: &mut Chrome,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    button: &ButtonView,
    rect: (f32, f32, f32, f32),
) {
    let (x, y, w, h) = rect;
    let pad_x = (ctx.cell_w * BUTTON_PAD_X_RATIO).round();
    let pad_y = (ctx.cell_h * BUTTON_PAD_Y_RATIO).round();

    // Determine colors based on focus: a focused button is filled with the
    // active indicator's accent; an unfocused one, primary or not, sits on the
    // menu background, and the outline drawn below is what tells a primary
    // button apart.
    let (fill, text_color) = if button.focused {
        let accent = theme.active_indicator;
        (accent, text_on(accent, theme))
    } else {
        (theme.menu_bg, theme.foreground)
    };

    let button_rect = (x, y, w, h);
    if button.focused {
        frame.fill(button_rect, BUTTON_RADIUS, fill, 1.0);
    } else if button.primary {
        let outline = theme.active_indicator;
        frame.fill(button_rect, BUTTON_RADIUS, outline, 1.0);
        let inset = (x + 1.0, y + 1.0, w - 2.0, h - 2.0);
        frame.fill(inset, BUTTON_RADIUS - 1.0, fill, 1.0);
    } else {
        let outline = mix_rgb(theme.menu_bg, theme.foreground, BUTTON_OUTLINE_MIX);
        frame.fill(button_rect, BUTTON_RADIUS, outline, 1.0);
        let inset = (x + 1.0, y + 1.0, w - 2.0, h - 2.0);
        frame.fill(inset, BUTTON_RADIUS - 1.0, fill, 1.0);
    }

    let text_y = (y + pad_y) as i32;
    let mut text_buf = shape_chrome_line(
        font_system,
        ctx,
        &button.text,
        text_color.to_glyphon(),
        true,
        true,
    );
    frame.draw_text(
        font_system,
        swash_cache,
        &mut text_buf,
        ((x + pad_x) as i32, text_y),
        text_color.to_glyphon(),
    );

    // Key hint underline: instead of displaying the character separately,
    // underline the matching shortcut character in the button text.
    if let Some(shortcut) = &button.shortcut {
        if let Some(target_ch) = shortcut.chars().find(|c| c.is_alphanumeric()) {
            let target_lower = target_ch.to_ascii_lowercase();
            if let Some((byte_offset, _)) = button
                .text
                .char_indices()
                .find(|(_, c)| c.to_ascii_lowercase() == target_lower)
            {
                for run in text_buf.layout_runs() {
                    for glyph in run.glyphs.iter() {
                        if glyph.start == byte_offset {
                            let underline_x = x + pad_x + glyph.x;
                            let underline_w = glyph.w.ceil().max(1.0);
                            let underline_y = y + pad_y + ctx.line_height - 2.0;
                            let underline_color = if button.focused {
                                text_color
                            } else {
                                theme.active_indicator
                            };
                            frame.fill(
                                (underline_x, underline_y, underline_w, 1.0),
                                0.0,
                                underline_color,
                                1.0,
                            );
                            break;
                        }
                    }
                }
            }
        }
    }
}

/// Draw a row of buttons right-aligned so the last ends at `right_end`'s `x`,
/// on its baseline `y`. Returns total width taken by the button row.
pub(crate) fn draw_buttons_row(
    frame: &mut Chrome,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    buttons: &[ButtonView],
    right_end: (f32, f32),
) -> f32 {
    let (right_x, y) = right_end;
    if buttons.is_empty() {
        return 0.0;
    }
    let gap = (ctx.cell_w * BUTTON_GAP_RATIO).round();
    let measured: Vec<((f32, f32), &ButtonView)> = buttons
        .iter()
        .map(|b| (measure_button(font_system, ctx, b), b))
        .collect();

    let total_w: f32 = measured.iter().map(|((w, _), _)| *w).sum::<f32>()
        + gap * (buttons.len().saturating_sub(1) as f32);

    // Lay out buttons right-to-left
    let mut current_right = right_x;
    for ((w, h), btn) in measured.into_iter().rev() {
        let x = current_right - w;
        draw_button(
            frame,
            font_system,
            swash_cache,
            ctx,
            theme,
            btn,
            (x, y, w, h),
        );
        current_right = x - gap;
    }

    total_w
}

/// The text color that reads on `fill`: the theme's dark background on a
/// light fill, its light foreground on a dark one, whichever the theme is.
pub(crate) fn text_on(fill: Rgb, theme: &Theme) -> Rgb {
    let (dark, light) = match luma_of(theme.background) < luma_of(theme.foreground) {
        true => (theme.background, theme.foreground),
        false => (theme.foreground, theme.background),
    };
    match luma_of(fill) > LIGHT_FILL_LUMA {
        true => dark,
        false => light,
    }
}

/// How bright `color` is, `0.0..=1.0`.
fn luma_of(color: Rgb) -> f32 {
    (0.299 * color.r as f32 + 0.587 * color.g as f32 + 0.114 * color.b as f32) / 255.0
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_measuring_buttons_row_includes_all_buttons_and_gaps() {
        let mut font_system = FontSystem::new();
        let ctx = sample_font_ctx();
        let buttons = vec![
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
        ];
        let (w1, h1) = measure_button(&mut font_system, &ctx, &buttons[0]);
        let (w2, h2) = measure_button(&mut font_system, &ctx, &buttons[1]);
        let (total_w, row_h) = measure_buttons_row(&mut font_system, &ctx, &buttons);
        let gap = (ctx.cell_w * BUTTON_GAP_RATIO).round();

        assert_eq!(total_w, w1 + w2 + gap);
        assert_eq!(row_h, h1.max(h2));
    }

    #[test]
    fn test_button_with_shortcut_draws_underline() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let button = ButtonView {
            focused: false,
            primary: false,
            shortcut: Some("y".to_string()),
            text: "Yes".to_string(),
        };
        let mut frame = Chrome::new(&theme, 200.0, 100.0, 10.0);
        draw_button(
            &mut frame,
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &button,
            (10.0, 10.0, 80.0, 30.0),
        );
        let (pixels, _canvas) = frame.pixels();
        // Since button is not focused and not primary, its background is menu_bg,
        // but the shortcut underline for 'y' is drawn in theme.active_indicator.
        let has_accent = pixels.chunks_exact(4).any(|px| {
            px[3] == 255
                && (px[0], px[1], px[2])
                    == (
                        theme.active_indicator.r,
                        theme.active_indicator.g,
                        theme.active_indicator.b,
                    )
        });
        assert!(has_accent, "shortcut character gets an accent underline");
    }
}

//! The tooltip of the TextMate token and scopes under the cursor: what is
//! there, the theme rule coloring it, and the scope stack, with no title bar,
//! hung from the cursor's cell.

use super::view::TokenInspectView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line, truncate_label};
use crate::renderer::raster::chrome::{anchored, Chrome};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::{Rgb, Theme};
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Constants
// ========================================================================

const INSPECT_BASE_FONT_PX: f32 = 13.0;
const INSPECT_WIDTH: f32 = 420.0;
const PAD_X: f32 = 14.0;
const TITLE_ALPHA: f32 = 0.90;
const HINT_ALPHA: f32 = 0.55;
const LABEL_ALPHA: f32 = 0.60;
const DIVIDER_PAD_Y: f32 = 6.0;
const SWATCH_SIZE: f32 = 10.0;

// ========================================================================
// Functions
// ========================================================================

/// Rasterize the token inspection card near the cursor: character & codepoint,
/// document line/col, syntax language, matching theme color rule & hex, and
/// the full TextMate scope stack.
pub(in crate::renderer) fn token_inspect_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &TokenInspectView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let s = ctx.font_size / INSPECT_BASE_FONT_PX;
    let margin = DROPDOWN_SHADOW;
    let line_h = (ctx.line_height * 0.92).max(16.0 * s).round();

    // 1. Calculate needed height
    let body_pad_top = 8.0 * s;
    let body_pad_bottom = 10.0 * s;
    let divider_spacing = (DIVIDER_PAD_Y * 2.0 * s + 1.0).round();

    // Sections:
    // Basic info: 3 rows (char, position, language)
    let info_rows = 3.0 * line_h;
    // Theme rule: heading (1 row) + color_rule + hex + style (3 rows)
    let rule_rows = 4.0 * line_h;
    // Scope stack: heading (1 row) + scopes
    let scope_count = view.scopes.len().max(1);
    let scope_rows = (1 + scope_count) as f32 * line_h;

    let natural_h = body_pad_top
        + info_rows
        + divider_spacing
        + rule_rows
        + divider_spacing
        + scope_rows
        + body_pad_bottom;

    let panel_w = (INSPECT_WIDTH * s).min(surface_w - 2.0 * margin);
    let panel_h = natural_h.min(surface_h - 2.0 * margin);
    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);

    // 2. The colors the body is drawn in.
    let fg_color = theme.foreground.to_glyphon();
    let title_color = glyphon::Color::rgba(
        fg_color.r(),
        fg_color.g(),
        fg_color.b(),
        (TITLE_ALPHA * 255.0).round() as u8,
    );
    let hint_color = glyphon::Color::rgba(
        fg_color.r(),
        fg_color.g(),
        fg_color.b(),
        (HINT_ALPHA * 255.0).round() as u8,
    );
    let label_color = glyphon::Color::rgba(
        fg_color.r(),
        fg_color.g(),
        fg_color.b(),
        (LABEL_ALPHA * 255.0).round() as u8,
    );
    let accent_color = theme.ansi[4].to_glyphon();

    // 3. Body contents
    let mut curr_y = margin + body_pad_top;
    let left_x = margin + PAD_X * s;
    let col2_x = left_x + 64.0 * s;
    let pad_x_s = PAD_X * s;

    // Character
    draw_kv(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        "char",
        &view.character,
        label_color,
        fg_color,
        true,
        left_x,
        col2_x,
        panel_w,
        margin,
        pad_x_s,
        curr_y,
    );
    curr_y += line_h;

    // Position
    draw_kv(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        "position",
        &view.position,
        label_color,
        fg_color,
        false,
        left_x,
        col2_x,
        panel_w,
        margin,
        pad_x_s,
        curr_y,
    );
    curr_y += line_h;

    // Language
    draw_kv(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        "language",
        &view.language,
        label_color,
        fg_color,
        false,
        left_x,
        col2_x,
        panel_w,
        margin,
        pad_x_s,
        curr_y,
    );
    curr_y += line_h;

    // Divider
    curr_y += (DIVIDER_PAD_Y * s).round();
    frame.fill((left_x, curr_y, panel_w - 2.0 * pad_x_s, 1.0), 0.0, theme.divider, 1.0);
    curr_y += 1.0 + (DIVIDER_PAD_Y * s).round();

    // Theme Rule Section
    let mut sec_buf =
        shape_chrome_line(font_system, ctx, "Theme Rule", title_color, true, true);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut sec_buf,
        (left_x.round() as i32, curr_y.round() as i32),
        title_color,
    );
    curr_y += line_h;

    let rule_name = view.color_rule.as_deref().unwrap_or("none");
    draw_kv(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        "rule",
        rule_name,
        label_color,
        accent_color,
        true,
        left_x,
        col2_x,
        panel_w,
        margin,
        pad_x_s,
        curr_y,
    );
    curr_y += line_h;

    let hex_val = view.foreground_hex.as_deref().unwrap_or("none");
    if let Some(parsed) = view.foreground_hex.as_deref().and_then(Rgb::parse_hex) {
        let swatch_sz = SWATCH_SIZE * s;
        let swatch_y = curr_y + (line_h - swatch_sz) * 0.5;
        frame.fill((col2_x, swatch_y, swatch_sz, swatch_sz), 2.0 * s, parsed, 1.0);

        let mut key_buf = shape_chrome_line(font_system, ctx, "color", label_color, false, true);
        frame.draw_text(
            font_system,
            swash_cache,
            &mut key_buf,
            (left_x.round() as i32, curr_y.round() as i32),
            label_color,
        );

        let mut hex_buf = shape_chrome_line(font_system, ctx, hex_val, fg_color, false, true);
        let hex_x = col2_x + swatch_sz + 6.0 * s;
        frame.draw_text(
            font_system,
            swash_cache,
            &mut hex_buf,
            (hex_x.round() as i32, curr_y.round() as i32),
            fg_color,
        );
        curr_y += line_h;
    } else {
        draw_kv(
            &mut frame,
            font_system,
            swash_cache,
            ctx,
            "color",
            hex_val,
            label_color,
            fg_color,
            true,
            left_x,
            col2_x,
            panel_w,
            margin,
            pad_x_s,
            curr_y,
        );
        curr_y += line_h;
    }

    let style_val = view.style.as_deref().unwrap_or("normal");
    draw_kv(
        &mut frame,
        font_system,
        swash_cache,
        ctx,
        "style",
        style_val,
        label_color,
        fg_color,
        false,
        left_x,
        col2_x,
        panel_w,
        margin,
        pad_x_s,
        curr_y,
    );
    curr_y += line_h;

    // Divider
    curr_y += (DIVIDER_PAD_Y * s).round();
    frame.fill((left_x, curr_y, panel_w - 2.0 * pad_x_s, 1.0), 0.0, theme.divider, 1.0);
    curr_y += 1.0 + (DIVIDER_PAD_Y * s).round();

    // TextMate Scope Stack Section
    let mut scopes_heading =
        shape_chrome_line(font_system, ctx, "TextMate Scopes", title_color, true, true);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut scopes_heading,
        (left_x.round() as i32, curr_y.round() as i32),
        title_color,
    );
    curr_y += line_h;

    if view.scopes.is_empty() {
        let mut none_buf =
            shape_chrome_line(font_system, ctx, "(no scopes)", hint_color, false, true);
        frame.draw_text(
            font_system,
            swash_cache,
            &mut none_buf,
            (left_x.round() as i32, curr_y.round() as i32),
            hint_color,
        );
    } else {
        let last_idx = view.scopes.len() - 1;
        for (i, scope) in view.scopes.iter().enumerate() {
            if curr_y + line_h > margin + panel_h {
                break;
            }
            let is_leaf = i == last_idx;
            let (indent, scope_color) = if is_leaf {
                ("  ", accent_color)
            } else {
                ("", label_color)
            };
            let text = format!("{indent}{scope}");
            let fitted = truncate_label(&text, 65);
            let mut scope_buf =
                shape_chrome_line(font_system, ctx, &fitted, scope_color, is_leaf, true);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut scope_buf,
                (left_x.round() as i32, curr_y.round() as i32),
                scope_color,
            );
            curr_y += line_h;
        }
    }

    // 4. Hung from the cursor's cell, as every tooltip is.
    let (panel_x, panel_y) = anchored(view.anchor, (panel_w, panel_h), margin, (surface_w, surface_h));
    frame.into_image(panel_x, panel_y)
}

#[allow(clippy::too_many_arguments)]
fn draw_kv(
    frame: &mut Chrome,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    key: &str,
    val: &str,
    key_color: glyphon::Color,
    val_color: glyphon::Color,
    val_mono: bool,
    left_x: f32,
    col2_x: f32,
    panel_w: f32,
    margin: f32,
    pad_x_s: f32,
    curr_y: f32,
) {
    let mut key_buf = shape_chrome_line(font_system, ctx, key, key_color, false, true);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut key_buf,
        (left_x.round() as i32, curr_y.round() as i32),
        key_color,
    );

    let max_val_w = (margin + panel_w - pad_x_s - col2_x).max(20.0);
    let fitted = truncate_label(val, 60);
    let mut val_buf = shape_chrome_line(font_system, ctx, &fitted, val_color, false, !val_mono);
    if buffer_width(&val_buf) > max_val_w {
        let truncated = truncate_label(val, 40);
        val_buf = shape_chrome_line(font_system, ctx, &truncated, val_color, false, !val_mono);
    }
    frame.draw_text(
        font_system,
        swash_cache,
        &mut val_buf,
        (col2_x.round() as i32, curr_y.round() as i32),
        val_color,
    );
}

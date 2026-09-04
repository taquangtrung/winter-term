//! Low-level pixel work shared by every chrome surface.

use crate::renderer::glyphs::*;
use crate::theme::Rgb;
use glyphon::{Attrs, BufferLine, Color, Family, FontSystem, Shaping, SwashCache, TextBounds};

// ========================================================================
// Painting primitives
// ========================================================================

/// A glyphon clip rect from pixel edges.
pub(super) fn text_bounds(left: f32, top: f32, right: f32, bottom: f32) -> TextBounds {
    TextBounds {
        left: left as i32,
        top: top as i32,
        right: right as i32,
        bottom: bottom as i32,
    }
}
/// Shorten `s` to at most `max_chars`, appending an ellipsis when truncated.
pub(super) fn truncate_label(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    match max_chars {
        0 => String::new(),
        1 => "\u{2026}".to_string(),
        _ => {
            let mut out: String = s.chars().take(max_chars - 1).collect();
            out.push('\u{2026}');
            out
        }
    }
}
/// Fill an opaque `(x, y, w, h)` rectangle of `color` into a `(canvas_w,
/// canvas_h)` `rgba` buffer, clipped to the canvas.
/// Alpha-composite `color` over the pixel at byte `idx` of an RGBA buffer.
pub(super) fn blend_px(rgba: &mut [u8], idx: usize, color: Rgb, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    rgba[idx] = (color.r as f32 * a + rgba[idx] as f32 * inv) as u8;
    rgba[idx + 1] = (color.g as f32 * a + rgba[idx + 1] as f32 * inv) as u8;
    rgba[idx + 2] = (color.b as f32 * a + rgba[idx + 2] as f32 * inv) as u8;
    let out_a = a + (rgba[idx + 3] as f32 / 255.0) * inv;
    rgba[idx + 3] = (out_a * 255.0) as u8;
}
/// Signed distance from `(px, py)` to the rounded rectangle `(x, y, w, h)` with
/// corner `radius`: negative inside, zero on the edge, positive outside.
pub(super) fn rounded_rect_sdf(px: f32, py: f32, rect: (f32, f32, f32, f32), radius: f32) -> f32 {
    let (rx, ry, rw, rh) = rect;
    let half_w = rw / 2.0;
    let half_h = rh / 2.0;
    let r = radius.min(half_w).min(half_h);
    let qx = (px - (rx + half_w)).abs() - (half_w - r);
    let qy = (py - (ry + half_h)).abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}
/// Fill a rounded rectangle into `rgba`, anti-aliasing the edge over a 1px band
/// and compositing at up to `max_alpha` opacity. All four corners share `radius`.
/// Fill a rounded rectangle into `rgba`, anti-aliasing the edge over a 1px band
/// and compositing at up to `max_alpha` opacity.
pub(super) fn fill_rounded_rect(
    rgba: &mut [u8],
    canvas: (u32, u32),
    rect: (f32, f32, f32, f32),
    radius: f32,
    color: Rgb,
    max_alpha: f32,
) {
    let (canvas_w, canvas_h) = canvas;
    let (rx, ry, rw, rh) = rect;
    let x0 = rx.floor().max(0.0) as u32;
    let y0 = ry.floor().max(0.0) as u32;
    let x1 = ((rx + rw).ceil() as i64).clamp(0, canvas_w as i64) as u32;
    let y1 = ((ry + rh).ceil() as i64).clamp(0, canvas_h as i64) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let sdf = rounded_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, rect, radius);
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let idx = ((py * canvas_w + px) * 4) as usize;
            blend_px(rgba, idx, color, coverage * max_alpha);
        }
    }
}
/// Fill an anti-aliased line segment from `p1` to `p2` with given `thickness` and color.
pub(super) fn fill_line_segment(
    rgba: &mut [u8],
    canvas: (u32, u32),
    p1: (f32, f32),
    p2: (f32, f32),
    thickness: f32,
    color: Rgb,
    max_alpha: f32,
) {
    let (canvas_w, canvas_h) = canvas;
    let (x0, y0) = p1;
    let (x1, y1) = p2;
    let bx0 = x0.min(x1) - thickness;
    let by0 = y0.min(y1) - thickness;
    let bx1 = x0.max(x1) + thickness;
    let by1 = y0.max(y1) + thickness;

    let rx0 = bx0.floor().max(0.0) as u32;
    let ry0 = by0.floor().max(0.0) as u32;
    let rx1 = (bx1.ceil() as u32).min(canvas_w);
    let ry1 = (by1.ceil() as u32).min(canvas_h);

    let dx = x1 - x0;
    let dy = y1 - y0;
    let len_sq = dx * dx + dy * dy;

    for py in ry0..ry1 {
        for px in rx0..rx1 {
            let px_f = px as f32 + 0.5;
            let py_f = py as f32 + 0.5;

            let wx = px_f - x0;
            let wy = py_f - y0;

            let t = if len_sq > 0.0 {
                ((wx * dx + wy * dy) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let cx = x0 + t * dx;
            let cy = y0 + t * dy;

            let dist_sq = (px_f - cx) * (px_f - cx) + (py_f - cy) * (py_f - cy);
            let dist = dist_sq.sqrt();

            let sdf = dist - thickness / 2.0;
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let idx = ((py * canvas_w + px) * 4) as usize;
                blend_px(rgba, idx, color, coverage * max_alpha);
            }
        }
    }
}
/// The shaped pixel width of a single-line text buffer, used to right-align the
/// dropdown shortcuts under a proportional font.
pub(super) fn buffer_width(buffer: &glyphon::Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}
/// Shape one line of chrome text into a buffer without touching the GPU. With
/// `proportional`, it uses a sans-serif UI font instead of the terminal family.
pub(super) fn shape_chrome_line(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    text: &str,
    color: Color,
    bold: bool,
    proportional: bool,
) -> glyphon::Buffer {
    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_size(font_system, Some(f32::MAX), Some(ctx.line_height));
    let family = if proportional {
        Family::SansSerif
    } else {
        base_family(ctx.family)
    };
    let mut attrs = Attrs::new().family(family).color(color);
    if bold {
        let font_has_bold = proportional || ctx.font_has_bold;
        attrs = attrs.weight(effective_bold_weight(ctx.bold_weight, font_has_bold));
    } else {
        attrs = attrs.weight(parse_weight(
            ctx.normal_weight,
            glyphon::cosmic_text::Weight::NORMAL,
        ));
    }
    buffer.set_text(font_system, text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}
/// Composite `buffer`'s glyph coverage onto a `(canvas_w, canvas_h)` RGBA buffer
/// at pixel `offset`, clipping to the canvas. Bakes dropdown text into its texture.
pub(super) fn composite_buffer(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    rgba: &mut [u8],
    canvas: (u32, u32),
    buffer: &glyphon::Buffer,
    offset: (i32, i32),
    default_color: Color,
) {
    let (canvas_w, canvas_h) = canvas;
    let (ox, oy) = offset;
    buffer.draw(
        font_system,
        swash_cache,
        default_color,
        |x, y, w, h, color| {
            let alpha = color.a() as f32 / 255.0;
            if alpha <= 0.0 {
                return;
            }
            let (cr, cg, cb) = (color.r() as f32, color.g() as f32, color.b() as f32);
            for py in y..y + h as i32 {
                for px in x..x + w as i32 {
                    let gx = px + ox;
                    let gy = py + oy;
                    if gx < 0 || gy < 0 || gx >= canvas_w as i32 || gy >= canvas_h as i32 {
                        continue;
                    }
                    let idx = ((gy as u32 * canvas_w + gx as u32) * 4) as usize;
                    rgba[idx] = (cr * alpha + rgba[idx] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 1] = (cg * alpha + rgba[idx + 1] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 2] = (cb * alpha + rgba[idx + 2] as f32 * (1.0 - alpha)) as u8;
                }
            }
        },
    );
}
/// Render `text` into `rgba` at `offset`, coloring characters listed in
/// `match_positions` with `accent_color` and the rest with `base_color`.
/// When `underline` is true, a 1-pixel line is drawn below each matched span.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_highlighted_label(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    rgba: &mut [u8],
    canvas: (u32, u32),
    text: &str,
    match_positions: &[usize],
    base_color: Color,
    accent_color: Color,
    underline: bool,
    bold: bool,
    offset: (i32, i32),
) {
    let (canvas_w, canvas_h) = canvas;
    let weight = if bold {
        parse_weight(ctx.bold_weight, DEFAULT_BOLD_WEIGHT)
    } else {
        parse_weight(ctx.normal_weight, glyphon::cosmic_text::Weight::NORMAL)
    };

    // Shape the whole label in one buffer so every glyph shares the same font
    // and size; color the matched characters with attribute spans. Shaping the
    // label in per-match fragments lets cosmic-text resolve different fallback
    // fonts per segment, which makes the matched glyphs render unevenly.
    let base_attrs = Attrs::new()
        .family(Family::SansSerif)
        .color(base_color)
        .weight(weight);
    let mut attrs_list = glyphon::AttrsList::new(&base_attrs);
    let mut matched_bytes: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut byte = 0usize;
    for (i, ch) in text.chars().enumerate() {
        let len = ch.len_utf8();
        if match_positions.binary_search(&i).is_ok() {
            let span = Attrs::new()
                .family(Family::SansSerif)
                .color(accent_color)
                .weight(weight);
            attrs_list.add_span(byte..byte + len, &span);
            matched_bytes.insert(byte);
        }
        byte += len;
    }

    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_size(font_system, Some(f32::MAX), Some(ctx.line_height));
    buffer.lines.clear();
    buffer.lines.push(BufferLine::new(
        text,
        glyphon::cosmic_text::LineEnding::default(),
        attrs_list,
        Shaping::Advanced,
    ));
    buffer.shape_until_scroll(font_system, false);
    composite_buffer(
        font_system,
        swash_cache,
        rgba,
        canvas,
        &buffer,
        offset,
        base_color,
    );

    // Underline the matched glyphs, located from the shaped layout.
    let underline_y = offset.1 + ctx.line_height as i32 - 2;
    if underline && underline_y >= 0 && underline_y < canvas_h as i32 {
        let uy = underline_y as u32;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                if !matched_bytes.contains(&glyph.start) {
                    continue;
                }
                let gx0 = offset.0 + glyph.x as i32;
                let gx1 = gx0 + glyph.w.ceil() as i32;
                for px in gx0.max(0)..gx1.min(canvas_w as i32) {
                    let idx = (uy * canvas_w + px as u32) as usize * 4;
                    rgba[idx] = accent_color.r();
                    rgba[idx + 1] = accent_color.g();
                    rgba[idx + 2] = accent_color.b();
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rounded_rect_sdf_signs() {
        let rect = (0.0, 0.0, 100.0, 100.0);
        let radius = 20.0;
        // The center is well inside (negative distance).
        assert!(rounded_rect_sdf(50.0, 50.0, rect, radius) < 0.0);
        // A point far outside is positive.
        assert!(rounded_rect_sdf(150.0, 50.0, rect, radius) > 0.0);
        // The very corner of the bounding box lies outside the rounded shape...
        assert!(rounded_rect_sdf(1.0, 1.0, rect, radius) > 0.0);
        // ...while the middle of an edge, the same depth in, is inside.
        assert!(rounded_rect_sdf(1.0, 50.0, rect, radius) < 0.0);
    }
}

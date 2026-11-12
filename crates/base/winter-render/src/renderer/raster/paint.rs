//! Low-level pixel work shared by every chrome surface.

use glyphon::{Attrs, Color, Family, FontSystem, Shaping, SwashCache};

use crate::renderer::glyphs::{base_family, effective_bold_weight, parse_weight, FontCtx};
use crate::theme::Rgb;

// ========================================================================
// Painting primitives
// ========================================================================

/// Shorten `s` to at most `max_chars`, appending an ellipsis when truncated.
pub(in crate::renderer) fn truncate_label(s: &str, max_chars: usize) -> String {
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
pub(in crate::renderer) fn blend_px(rgba: &mut [u8], idx: usize, color: Rgb, alpha: f32) {
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
pub(in crate::renderer) fn rounded_rect_sdf(
    px: f32,
    py: f32,
    rect: (f32, f32, f32, f32),
    radius: f32,
) -> f32 {
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
pub(in crate::renderer) fn fill_rounded_rect(
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
/// The shaped pixel width of a single-line text buffer, used to right-align the
/// dropdown shortcuts under a proportional font.
pub(in crate::renderer) fn buffer_width(buffer: &glyphon::Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}
/// Query the desktop environment's configured interface font name.
fn system_desktop_font_name() -> Option<&'static str> {
    static FONT_NAME: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();
    FONT_NAME
        .get_or_init(|| {
            #[cfg(target_os = "linux")]
            {
                let output = std::process::Command::new("gsettings")
                    .args(["get", "org.gnome.desktop.interface", "font-name"])
                    .output()
                    .ok()?;
                if output.status.success() {
                    let raw = String::from_utf8_lossy(&output.stdout);
                    return parse_system_font_name(&raw);
                }
            }
            #[cfg(target_os = "macos")]
            {
                return Some("SF Pro".to_string());
            }
            #[cfg(target_os = "windows")]
            {
                return Some("Segoe UI".to_string());
            }
            #[allow(unreachable_code)]
            None
        })
        .as_deref()
}

/// Parse a raw font name string from desktop settings (e.g. `'Noto Sans 10'` -> `"Noto Sans"`).
fn parse_system_font_name(raw: &str) -> Option<String> {
    let trimmed = raw.trim().trim_matches('\'').trim_matches('"').trim();
    if trimmed.is_empty() {
        return None;
    }
    let font_name = if let Some(last_space) = trimmed.rfind(' ') {
        let suffix = &trimmed[last_space + 1..];
        if suffix.chars().all(|c| c.is_ascii_digit() || c == '.') {
            trimmed[..last_space].trim()
        } else {
            trimmed
        }
    } else {
        trimmed
    };
    if font_name.is_empty() {
        None
    } else {
        Some(font_name.to_string())
    }
}

/// Resolve the best available proportional UI family, preferring the desktop
/// Where the symbols begin: General Punctuation's block and every block past
/// it hold the marks a title's symbols come from, below it is text.
const SYMBOLS_START: u32 = 0x2000;

/// environment's configured interface font, then clean modern sans-serif fallbacks.
pub(in crate::renderer) fn ui_family(font_system: &FontSystem) -> Family<'static> {
    if let Some(sys_name) = system_desktop_font_name() {
        if font_system.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(sys_name))
        }) {
            return Family::Name(sys_name);
        }
    }
    for candidate in ["Inter", "SF Pro"] {
        if font_system.db().faces().any(|face| {
            face.families
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(candidate))
        }) {
            return Family::Name(candidate);
        }
    }
    Family::SansSerif
}

/// Whether `c` is a symbol rather than text: a dingbat, an arrow, a
/// pictograph, the marks a program puts in its title. The chrome's font
/// rarely has one, and another font fills in for it.
fn is_symbol(c: char) -> bool {
    !c.is_alphanumeric() && !c.is_whitespace() && c as u32 >= SYMBOLS_START
}

/// `text` cut into runs of symbols and of the rest, each with whether it is
/// symbols.
fn symbol_runs(text: &str) -> Vec<(&str, bool)> {
    let mut runs: Vec<(&str, bool)> = Vec::new();
    let mut start = 0;
    let mut current = None;
    for (at, c) in text.char_indices() {
        let symbol = is_symbol(c);
        match current {
            Some(kind) if kind != symbol => {
                runs.push((&text[start..at], kind));
                start = at;
            }
            _ => {}
        }
        current = Some(symbol);
    }
    if let Some(kind) = current {
        runs.push((&text[start..], kind));
    }
    runs
}

/// Shape one line of chrome text into a buffer without touching the GPU. With
/// `proportional`, it uses a sans-serif UI font instead of the terminal family.
pub(in crate::renderer) fn shape_chrome_line(
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
    buffer.set_size(Some(f32::MAX), Some(ctx.line_height));
    let family = if proportional {
        ui_family(font_system)
    } else {
        base_family(ctx.family)
    };
    let normal = Attrs::new()
        .family(family)
        .color(color)
        .weight(parse_weight(
            ctx.normal_weight,
            glyphon::cosmic_text::Weight::NORMAL,
        ));
    if !bold {
        buffer.set_text(text, &normal, Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);
        return buffer;
    }
    let font_has_bold = proportional || ctx.font_has_bold;
    let heavy = normal
        .clone()
        .weight(effective_bold_weight(ctx.bold_weight, font_has_bold));
    // The symbols stay at the normal weight: the font that fills in for one
    // the chrome's font lacks differs between weights, so in bold a title
    // (a focused tab's) would draw its symbols at another font's size than
    // the same title does unfocused.
    let runs = symbol_runs(text).into_iter().map(|(run, symbol)| {
        (
            run,
            if symbol {
                normal.clone()
            } else {
                heavy.clone()
            },
        )
    });
    buffer.set_rich_text(runs, &heavy, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}
/// Composite `buffer`'s glyph coverage onto a `(canvas_w, canvas_h)` RGBA buffer
/// at pixel `offset`, clipping to the canvas. Bakes dropdown text into its texture.
pub(in crate::renderer) fn composite_buffer(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    rgba: &mut [u8],
    canvas: (u32, u32),
    buffer: &mut glyphon::Buffer,
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
        // The very corner of the bounding box lies outside the rounded shape.
        assert!(rounded_rect_sdf(1.0, 1.0, rect, radius) > 0.0);
        // ...while the middle of an edge, the same depth in, is inside.
        assert!(rounded_rect_sdf(1.0, 50.0, rect, radius) < 0.0);
    }

    #[test]
    fn test_parse_system_font_name() {
        assert_eq!(
            parse_system_font_name("'Noto Sans 10'"),
            Some("Noto Sans".to_string())
        );
        assert_eq!(
            parse_system_font_name("\"Cantarell 11\""),
            Some("Cantarell".to_string())
        );
        assert_eq!(
            parse_system_font_name("Inter Medium 10.5"),
            Some("Inter Medium".to_string())
        );
        assert_eq!(parse_system_font_name("SF Pro"), Some("SF Pro".to_string()));
        assert_eq!(parse_system_font_name(""), None);
    }

    #[test]
    fn test_ui_family_resolves_clean_sans_serif() {
        let font_system = FontSystem::new();
        let family = ui_family(&font_system);
        assert!(matches!(family, Family::Name(_) | Family::SansSerif));
    }

    #[test]
    fn test_a_title_splits_into_its_symbols_and_its_words() {
        assert_eq!(
            symbol_runs("\u{2733} Claude Code"),
            [("\u{2733}", true), (" Claude Code", false)]
        );
        assert_eq!(
            symbol_runs("1: vim \u{2014} main.rs"),
            [("1: vim ", false), ("\u{2014}", true), (" main.rs", false)]
        );
        assert_eq!(
            symbol_runs("caf\u{e9}"),
            [("caf\u{e9}", false)],
            "accented letters are text"
        );
        assert!(symbol_runs("").is_empty());
    }
}

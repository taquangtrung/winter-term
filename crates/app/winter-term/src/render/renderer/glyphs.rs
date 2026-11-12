//! Glyph machinery: font configuration and loading, cell metrics,
//! fallback glyphs, and shaping helpers.

use super::background::DotVertex;
use super::GpuRenderer;
use glyphon::{Attrs, Family, FontSystem, Shaping};

// ========================================================================
// Constants
// ========================================================================

pub(super) const DEFAULT_FONT_SIZE: f32 = 15.0;

/// The smallest and largest logical font size the renderer takes. Below the
/// floor the line height rounds to zero, which the text shaper asserts
/// against, so a size from config is held inside these before any use.
const MIN_FONT_SIZE: f32 = 6.0;
const MAX_FONT_SIZE: f32 = 72.0;

pub(super) const DEFAULT_LINE_HEIGHT: f32 = 20.0;

/// Default weight for bold/bright cells when the user has set no `font-weight-bold`.
pub(super) const DEFAULT_BOLD_WEIGHT: glyphon::cosmic_text::Weight =
    glyphon::cosmic_text::Weight::BOLD;

/// `size` held to the font sizes the renderer can draw: inside
/// [`MIN_FONT_SIZE`]..=[`MAX_FONT_SIZE`], and the default for a size that is
/// no number at all (NaN, infinite), which no clamp can place.
pub fn clamp_font_size(size: f32) -> f32 {
    match size.is_finite() {
        true => size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE),
        false => DEFAULT_FONT_SIZE,
    }
}

/// Diameter of a procedurally-drawn braille dot as a fraction of its sub-cell.
/// The anti-aliased circle leaves natural corner gaps, so this can be fairly
/// large while still reading as distinct dots like wezterm's built-in braille.
const BRAILLE_DOT_RATIO: f32 = 0.9;

// ========================================================================
// Implementation
// ========================================================================

/// Font selection for the renderer. `family` is the primary family name (e.g.
/// "JetBrains Mono"); `None` falls back to the system monospace font.
/// Glyphs missing from the primary font are filled in from the system font
/// database automatically.
#[derive(Clone, Debug)]
pub struct FontConfig {
    /// Primary font family, or `None` for the system monospace font.
    pub family: Option<String>,
    /// Size in points.
    pub size: f32,
    /// Weight used for unstyled text, as a name (`"medium"`) or a numeric
    /// string (`"400"`). `None` uses the family's default.
    pub normal_weight: Option<String>,
    /// Weight used for SGR-bold text. `None` uses the family's bold face.
    pub bold_weight: Option<String>,
}

impl Default for FontConfig {
    fn default() -> Self {
        Self {
            family: None,
            size: DEFAULT_FONT_SIZE,
            normal_weight: None,
            bold_weight: None,
        }
    }
}

/// A handle to a background system-font scan. Scanning system fonts takes
/// ~150ms and needs no GPU, so it runs on its own thread that overlaps GPU
/// initialization. Created by [`start_font_load`], consumed by [`GpuRenderer::new`].
pub struct FontLoad(std::thread::JoinHandle<FontSystem>);

impl FontLoad {
    pub(super) fn join(self) -> FontSystem {
        self.0.join().expect("font-load thread panicked")
    }
}

/// Start scanning system fonts on a background thread. Call this as early as
/// possible (before creating the wgpu instance/adapter/device) so the scan
/// overlaps GPU initialization instead of adding to it.
pub fn start_font_load() -> FontLoad {
    FontLoad(std::thread::spawn(FontSystem::new))
}

impl GpuRenderer {
    /// Change the logical font size and recompute cell dimensions. Returns
    /// `Some((cols, rows))` if the size changed, `None` if already at that size.
    pub fn set_font_size(&mut self, logical_size: f32) -> Option<(usize, usize)> {
        let logical_size = clamp_font_size(logical_size);
        if (self.fonts.logical_font_size - logical_size).abs() < 0.01 {
            return None;
        }
        self.fonts.logical_font_size = logical_size;
        self.fonts.font_size = logical_size * self.fonts.scale_factor as f32;
        let base_line_h =
            (self.fonts.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let (base_cell_w, base_cell_h) = measure_cell(
            &mut self.font_system,
            self.fonts.font_size,
            base_line_h,
            self.fonts.font_family.as_deref(),
            self.fonts.normal_weight.as_deref(),
        );
        self.cols = ((self.size.width as f32 / base_cell_w).floor() as usize).max(1);
        // Keep `cell_width` and `cell_height`/`line_height` on the measured cell
        // stride (do NOT stretch either to fill the window); see the matching
        // comment in `resize` for why a stretched stride drifts the cursor off
        // its glyph (columns, since glyphs render at their natural advance and
        // the shaping site does not snap them) or off its line (rows).
        // `cols`/`rows` stay the
        // full-window counts; the app subtracts the chrome rows itself, and its
        // remainder-padding logic absorbs the (sub-one-cell) leftover space.
        self.fonts.cell_width = base_cell_w;
        let cell_h = base_cell_h;
        self.rows = ((self.size.height as f32 / cell_h).floor() as usize).max(1);
        self.fonts.cell_height = cell_h;
        self.fonts.line_height = cell_h;
        Some((self.cols, self.rows))
    }

    /// Enable or disable OpenType ligatures in the grid's text.
    pub fn set_ligatures(&mut self, enabled: bool) {
        self.fonts.ligatures = enabled;
    }
}

/// The cosmic-text font family for a configured family name, defaulting to the
/// system monospace when no family is set.
pub(super) fn base_family(name: Option<&str>) -> Family<'_> {
    match name {
        Some(n) => Family::Name(n),
        None => Family::Monospace,
    }
}

/// Whether `c` is a Braille Patterns glyph (U+2800..=U+28FF). btop renders its
/// history graphs with these. When the active font has braille glyphs they are
/// rendered directly; when it doesn't, the system fallback supplies them
/// proportionally (narrower than a cell, sparse and drifting), so they are drawn
/// procedurally instead (see [`push_braille_dots`]).
pub(super) fn is_braille(c: char) -> bool {
    ('\u{2800}'..='\u{28FF}').contains(&c)
}

/// Emit anti-aliased dot quads for the set dots of a Braille Patterns glyph,
/// snapped to the cell's 2x4 dot matrix so the result is cell-aligned. Each dot
/// is a quad carrying `uv` in [-1, 1] that the dot shader fills as a smooth
/// circle. `color` is sRGB (the shader linearizes it); positions are converted
/// to NDC against the surface size.
pub(super) fn push_braille_dots(
    verts: &mut Vec<DotVertex>,
    glyph: char,
    cell: (f32, f32, f32, f32),
    color: (f32, f32, f32),
    surface: (f32, f32),
) {
    let (cell_x, cell_y, cw, cell_h) = cell;
    let (surface_w, surface_h) = surface;
    // Braille dot bit (0..8) -> (sub-column, sub-row) in the 2-wide, 4-tall
    // dot matrix, following the Unicode dot numbering (1,2,3,7 | 4,5,6,8).
    const DOTS: [(u8, u8); 8] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (1, 0),
        (1, 1),
        (1, 2),
        (0, 3),
        (1, 3),
    ];
    let bits = (glyph as u32).wrapping_sub(0x2800) as u8;
    let (r, g, b) = color;
    let sub_w = cw / 2.0;
    let sub_h = cell_h / 4.0;
    // Center a dot of diameter ratio*sub-cell in each sub-cell.
    let dot_w = sub_w * BRAILLE_DOT_RATIO;
    let dot_h = sub_h * BRAILLE_DOT_RATIO;
    let gap_x = (sub_w - dot_w) / 2.0;
    let gap_y = (sub_h - dot_h) / 2.0;
    for (i, &(dc, dr)) in DOTS.iter().enumerate() {
        if bits & (1 << i) == 0 {
            continue;
        }
        let px0 = cell_x + dc as f32 * sub_w + gap_x;
        let py0 = cell_y + dr as f32 * sub_h + gap_y;
        let x0 = px0 * 2.0 / surface_w - 1.0;
        let x1 = (px0 + dot_w) * 2.0 / surface_w - 1.0;
        let y0 = 1.0 - py0 * 2.0 / surface_h;
        let y1 = 1.0 - (py0 + dot_h) * 2.0 / surface_h;
        verts.push(DotVertex {
            x: x0,
            y: y0,
            u: -1.0,
            v: -1.0,
            r,
            g,
            b,
        });
        verts.push(DotVertex {
            x: x1,
            y: y0,
            u: 1.0,
            v: -1.0,
            r,
            g,
            b,
        });
        verts.push(DotVertex {
            x: x0,
            y: y1,
            u: -1.0,
            v: 1.0,
            r,
            g,
            b,
        });
        verts.push(DotVertex {
            x: x1,
            y: y0,
            u: 1.0,
            v: -1.0,
            r,
            g,
            b,
        });
        verts.push(DotVertex {
            x: x1,
            y: y1,
            u: 1.0,
            v: 1.0,
            r,
            g,
            b,
        });
        verts.push(DotVertex {
            x: x0,
            y: y1,
            u: -1.0,
            v: 1.0,
            r,
            g,
            b,
        });
    }
}

pub(super) fn parse_weight(
    weight: Option<&str>,
    default: glyphon::cosmic_text::Weight,
) -> glyphon::cosmic_text::Weight {
    match weight {
        None => default,
        Some(w) => match w.to_lowercase().as_str() {
            "thin" | "100" => glyphon::cosmic_text::Weight::THIN,
            "extra-light" | "extralight" | "200" => glyphon::cosmic_text::Weight::EXTRA_LIGHT,
            "light" | "300" => glyphon::cosmic_text::Weight::LIGHT,
            "normal" | "regular" | "400" => glyphon::cosmic_text::Weight::NORMAL,
            "medium" | "500" => glyphon::cosmic_text::Weight::MEDIUM,
            "semibold" | "semi-bold" | "600" => glyphon::cosmic_text::Weight::SEMIBOLD,
            "bold" | "700" => glyphon::cosmic_text::Weight::BOLD,
            "extra-bold" | "extrabold" | "800" => glyphon::cosmic_text::Weight::EXTRA_BOLD,
            "black" | "heavy" | "900" => glyphon::cosmic_text::Weight::BLACK,
            parsed => {
                if let Ok(num) = parsed.parse::<u16>() {
                    glyphon::cosmic_text::Weight(num)
                } else {
                    default
                }
            }
        },
    }
}

/// The weight to request for a cell that wants bold, given whether the active
/// family actually has a bold face ([`FontState::font_has_bold`](crate::render::renderer::FontState::font_has_bold) /
/// [`font_covers_bold_weight`]). Falls back to `Weight::NORMAL` rather than
/// requesting a weight that would resolve to an unrelated fallback family.
pub(super) fn effective_bold_weight(
    bold_weight: Option<&str>,
    font_has_bold: bool,
) -> glyphon::cosmic_text::Weight {
    if font_has_bold {
        parse_weight(bold_weight, DEFAULT_BOLD_WEIGHT)
    } else {
        glyphon::cosmic_text::Weight::NORMAL
    }
}

pub(super) fn measure_cell(
    font_system: &mut FontSystem,
    font_size: f32,
    line_height: f32,
    family: Option<&str>,
    normal_weight: Option<&str>,
) -> (f32, f32) {
    let metrics = glyphon::Metrics::new(font_size, line_height);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);
    buffer.set_size(Some(f32::MAX), Some(line_height));
    let attrs = Attrs::new()
        .family(base_family(family))
        .weight(parse_weight(
            normal_weight,
            glyphon::cosmic_text::Weight::NORMAL,
        ));
    buffer.set_text("M", &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);

    if let Some(run) = buffer.layout_runs().next() {
        let glyph_w = run.glyphs.first().map(|g| g.w).unwrap_or(font_size * 0.6);
        // Return the natural advance unrounded (unlike `line_height.round()`
        // below). The renderer draws glyphs at this advance and the cursor at
        // `col * cell_width`, so the two must be the same number down to the
        // last fraction of a pixel; rounding here would walk the cursor off its
        // glyph one column at a time, worse the further right you type. This
        // holds only because the shaping site deliberately does NOT call
        // `set_monospace_width`, which quantizes the advance at fractional sizes;
        // see the comment there.
        return (glyph_w, line_height.round());
    }

    (font_size * 0.6, line_height.round())
}

/// Whether the active font contains Braille Patterns glyphs. Probed by shaping
/// one with `Shaping::Basic`, which uses only the matched primary font with no
/// fallback: a real glyph id means the font has braille (render it directly), a
/// zero id (.notdef) means it does not (fall back to drawn dots).
pub(super) fn font_covers_braille(
    font_system: &mut FontSystem,
    font_size: f32,
    line_height: f32,
    family: Option<&str>,
    normal_weight: Option<&str>,
) -> bool {
    let metrics = glyphon::Metrics::new(font_size, line_height);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);
    // A size must be set or `shape_until_scroll` produces no layout runs, which
    // would make this always report "no braille" (matching `measure_cell`).
    buffer.set_size(Some(f32::MAX), Some(line_height));
    let attrs = Attrs::new()
        .family(base_family(family))
        .weight(parse_weight(
            normal_weight,
            glyphon::cosmic_text::Weight::NORMAL,
        ));
    buffer.set_text("\u{28ff}", &attrs, Shaping::Basic, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .flat_map(|run| run.glyphs.iter())
        .any(|glyph| glyph.glyph_id != 0)
}

/// Whether asking `fontdb` for `family` at `bold_weight` resolves to a face in
/// the same family as the plain (`Weight::NORMAL`) request, rather than
/// falling through to an unrelated system font. Probed by shaping the same
/// character at both weights and comparing the matched face's family name:
/// a real bold cut of the font keeps that name, while a fallback substitution
/// (see [`FontState::font_has_bold`](crate::render::renderer::FontState::font_has_bold)) picks a different family entirely.
pub(super) fn font_covers_bold_weight(
    font_system: &mut FontSystem,
    font_size: f32,
    line_height: f32,
    family: Option<&str>,
    bold_weight: Option<&str>,
) -> bool {
    let matched_family = |weight: glyphon::cosmic_text::Weight, font_system: &mut FontSystem| {
        let metrics = glyphon::Metrics::new(font_size, line_height);
        let mut buffer = glyphon::Buffer::new(font_system, metrics);
        buffer.set_size(Some(f32::MAX), Some(line_height));
        let attrs = Attrs::new().family(base_family(family)).weight(weight);
        buffer.set_text("M", &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(font_system, false);
        let font_id = buffer
            .layout_runs()
            .next()
            .and_then(|run| run.glyphs.first())
            .map(|g| g.font_id)?;
        font_system
            .db()
            .face(font_id)
            .and_then(|face| face.families.first().map(|(name, _)| name.clone()))
    };
    let normal = matched_family(glyphon::cosmic_text::Weight::NORMAL, font_system);
    let bold = matched_family(
        parse_weight(bold_weight, glyphon::cosmic_text::Weight::BOLD),
        font_system,
    );
    normal.is_some() && normal == bold
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_braille_quads_count_per_set_dot() {
        // Six vertices (two triangles) per set dot; the blank glyph emits none,
        // the full glyph emits all eight.
        let mut blank = Vec::new();
        push_braille_dots(
            &mut blank,
            '\u{2800}',
            (0.0, 0.0, 8.0, 16.0),
            (1.0, 1.0, 1.0),
            (100.0, 100.0),
        );
        assert_eq!(blank.len(), 0);

        let mut full = Vec::new();
        push_braille_dots(
            &mut full,
            '\u{28FF}',
            (0.0, 0.0, 8.0, 16.0),
            (1.0, 1.0, 1.0),
            (100.0, 100.0),
        );
        assert_eq!(full.len(), 8 * 6);

        let mut one = Vec::new();
        push_braille_dots(
            &mut one,
            '\u{2801}',
            (0.0, 0.0, 8.0, 16.0),
            (1.0, 1.0, 1.0),
            (100.0, 100.0),
        );
        assert_eq!(one.len(), 6);
    }
    #[test]
    fn test_braille_dot1_is_top_left_subcell() {
        // Dot 1 (U+2801) occupies the top-left of the 2x4 matrix: x in [0, cw/2],
        // y in [cell_y, cell_y + cell_h/4]. With cw=8,cell_h=16 over a 100px
        // surface, that is px x in [0,4] and py y in [0,4].
        let mut v = Vec::new();
        push_braille_dots(
            &mut v,
            '\u{2801}',
            (0.0, 0.0, 8.0, 16.0),
            (1.0, 1.0, 1.0),
            (100.0, 100.0),
        );
        let px = |ndc: f32| (ndc + 1.0) * 100.0 / 2.0;
        let py = |ndc: f32| (1.0 - ndc) * 100.0 / 2.0;
        let xs: Vec<f32> = v.iter().map(|q| px(q.x)).collect();
        let ys: Vec<f32> = v.iter().map(|q| py(q.y)).collect();
        assert!(xs.iter().all(|&x| (0.0..=4.0001).contains(&x)), "xs={xs:?}");
        assert!(ys.iter().all(|&y| (0.0..=4.0001).contains(&y)), "ys={ys:?}");
    }

    #[test]
    fn test_effective_bold_weight_falls_back_to_normal_without_a_bold_face() {
        // A family whose bold request resolves to an unrelated fallback font
        // (see `font_covers_bold_weight`) must render bold cells at NORMAL
        // weight instead, so they stay in the correct monospace family.
        assert_eq!(
            effective_bold_weight(None, false),
            glyphon::cosmic_text::Weight::NORMAL
        );
        assert_eq!(effective_bold_weight(None, true), DEFAULT_BOLD_WEIGHT);
        assert_eq!(
            effective_bold_weight(Some("900"), true),
            glyphon::cosmic_text::Weight(900)
        );
        // A configured override is still ignored when the family has no bold face.
        assert_eq!(
            effective_bold_weight(Some("900"), false),
            glyphon::cosmic_text::Weight::NORMAL
        );
    }
    #[test]
    fn test_cursor_stride_tracks_rendered_advance_across_fractional_sizes() {
        // The cursor and cell backgrounds are drawn at `col * cell_width`, while
        // text is drawn where cosmic-text lays each glyph out. Those two must
        // agree down to the last fraction of a pixel, or the cursor walks off its
        // glyph one column at a time, worse the further right you type. This
        // regressed on Windows only, because it only surfaces at fractional
        // physical font sizes (fractional DPI scale: logical 15px at 125% is
        // 18.75px), where cosmic-text's `set_monospace_width` quantizes advances.
        // The fix removes that call from the shaping site, so this asserts, at
        // both an integer and two fractional sizes, that a line shaped the way
        // the renderer shapes primary-font ASCII (Basic, no `set_monospace_width`)
        // lands every glyph on the `col * cell_width` grid that `measure_cell`
        // defines. If `measure_cell` starts rounding, or the shaping site starts
        // snapping advances again, this drifts and fails.
        let mut font_system = FontSystem::new();
        for size in [16.0f32, 18.75, 22.5] {
            let line_height = (size * 1.2).round();
            let (cell_width, _) = measure_cell(&mut font_system, size, line_height, None, None);

            let metrics = glyphon::Metrics::new(size, line_height);
            let mut buffer = glyphon::Buffer::new(&mut font_system, metrics);
            buffer.set_size(Some(4000.0), Some(line_height));
            let attrs = Attrs::new().family(glyphon::cosmic_text::Family::Monospace);
            buffer.set_text(&"w".repeat(80), &attrs, Shaping::Basic, None);
            buffer.shape_until_scroll(&mut font_system, false);

            let run = buffer.layout_runs().next().expect("shaped line");
            for (col, glyph) in run.glyphs.iter().enumerate() {
                let expected_x = col as f32 * cell_width;
                assert!(
                    (glyph.x - expected_x).abs() < 0.05,
                    "size={size} col={col} expected_x={expected_x} actual_x={} cell_width={cell_width}",
                    glyph.x
                );
            }
        }
    }

    #[test]
    fn test_a_font_size_is_held_to_what_the_shaper_can_lay_out() {
        assert_eq!(
            clamp_font_size(0.0),
            MIN_FONT_SIZE,
            "a zero line height panics"
        );
        assert_eq!(clamp_font_size(-4.0), MIN_FONT_SIZE);
        assert_eq!(clamp_font_size(500.0), MAX_FONT_SIZE);
        assert_eq!(
            clamp_font_size(f32::NAN),
            DEFAULT_FONT_SIZE,
            "no clamp places NaN"
        );
        assert_eq!(clamp_font_size(13.0), 13.0);
    }
}

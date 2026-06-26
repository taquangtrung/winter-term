//! Glyph machinery: font configuration and loading, cell metrics,
//! fallback glyphs, and shaping helpers.

use super::background::DotVertex;
use super::GpuRenderer;
use crate::glyph_quad::GlyphTexture;
use glyphon::{Attrs, Color, Family, FontSystem, Shaping, SwashContent};

// ========================================================================
// Constants
// ========================================================================

pub(super) const DEFAULT_FONT_SIZE: f32 = 15.0;

pub(super) const DEFAULT_LINE_HEIGHT: f32 = 20.0;

/// Default weight for bold/bright cells when the user has set no `font-weight-bold`.
pub(super) const DEFAULT_BOLD_WEIGHT: glyphon::cosmic_text::Weight =
    glyphon::cosmic_text::Weight::BOLD;

/// Diameter of a procedurally-drawn braille dot as a fraction of its sub-cell.
/// The anti-aliased circle leaves natural corner gaps, so this can be fairly
/// large while still reading as distinct dots like wezterm's built-in braille.
const BRAILLE_DOT_RATIO: f32 = 0.9;

// ========================================================================
// Implementation
// ========================================================================

/// Scalar font metrics needed to shape and lay out tabbar text offscreen,
/// independent of the GPU. Bundled so the dropdown rasterizer (and its tests)
/// can run without a `Renderer`.
pub(super) struct FontCtx<'a> {
    pub(super) cell_h: f32,
    pub(super) cell_w: f32,
    pub(super) family: Option<&'a str>,
    /// See [`GpuRenderer::font_has_bold`]: whether `bold_weight` actually
    /// resolves to a face in `family` rather than an unrelated fallback font.
    pub(super) font_has_bold: bool,
    pub(super) font_size: f32,
    pub(super) line_height: f32,
    pub(super) normal_weight: Option<&'a str>,
    pub(super) bold_weight: Option<&'a str>,
}

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

/// A [`GpuRenderer::fallback_glyph_cache`] entry: the `(font_size, cell_width)`
/// it was measured at, the fit scale (`None` if the glyph doesn't need
/// correcting), and whether it's a genuine color glyph.
pub(super) type FallbackGlyphFit = (f32, f32, Option<f32>, bool);

impl GpuRenderer {
    /// Change the logical font size and recompute cell dimensions. Returns
    /// `Some((cols, rows))` if the size changed, `None` if already at that size.
    pub fn set_font_size(&mut self, logical_size: f32) -> Option<(usize, usize)> {
        let logical_size = logical_size.clamp(6.0, 72.0);
        if (self.logical_font_size - logical_size).abs() < 0.01 {
            return None;
        }
        self.logical_font_size = logical_size;
        self.font_size = logical_size * self.scale_factor as f32;
        let base_line_h = (self.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let (base_cell_w, base_cell_h) = measure_cell(
            &mut self.font_system,
            self.font_size,
            base_line_h,
            self.font_family.as_deref(),
            self.normal_weight.as_deref(),
        );
        self.text_buffers.clear();
        self.status_buffer = None;
        self.cols = ((self.config.width as f32 / base_cell_w).floor() as usize).max(1);
        // Keep `cell_width` and `cell_height`/`line_height` on the measured cell
        // stride (do NOT stretch either to fill the window); see the matching
        // comment in `resize` for why a stretched stride drifts the cursor off
        // its glyph (columns, since glyphs render at their natural advance and
        // the shaping site does not snap them) or off its line (rows).
        // `cols`/`rows` stay the
        // full-window counts; the app subtracts the chrome rows itself, and its
        // remainder-padding logic absorbs the (sub-one-cell) leftover space.
        self.cell_width = base_cell_w;
        let cell_h = base_cell_h;
        self.rows = ((self.config.height as f32 / cell_h).floor() as usize).max(1);
        self.cell_height = cell_h;
        self.line_height = cell_h;
        Some((self.cols, self.rows))
    }

    /// Enable or disable OpenType ligatures. Clears the text-buffer cache when
    /// the setting changes so the new shaping mode takes effect immediately.
    pub fn set_ligatures(&mut self, enabled: bool) {
        if self.ligatures == enabled {
            return;
        }
        self.ligatures = enabled;
        self.text_buffers.clear();
    }

    /// Uniform scale that fits `ch`'s rasterized texture (see
    /// [`Self::rasterize_fallback_glyph`]) to one cell width, or two when
    /// `is_wide` (a double-width glyph like most emoji, or a paired regional-
    /// indicator flag half), or `None` if `ch` doesn't need
    /// [`Shaping::Advanced`] (see [`needs_complex_shaping`]), already advances
    /// by its target width on its own (renders normally through cosmic-text),
    /// or rasterized to no ink. Fallback fonts are proportional, so a
    /// Dingbat/symbol glyph like the star Claude Code cycles through as a
    /// progress spinner can shape to a much wider advance than the cell
    /// (measured ~38% over in the default font), overflowing its tile.
    /// Uploading its texture to [`GlyphQuadPass`] and scaling *that quad*
    /// (rather than reshaping the glyph through cosmic-text at an adjusted
    /// font size) brings its advance back to its target width without
    /// perturbing the ascent/descent cosmic-text computes for the rest of the
    /// row: mixed fallback glyphs with very different natural advances (as the
    /// spinner's rotating characters are) previously fought over that shared
    /// value, visibly bobbing the whole line each animation frame. Cached per
    /// `(character, is_wide)` since shaping to measure it is comparatively
    /// expensive; the cache entry is invalidated by comparing the
    /// `font_size`/`cell_width` it was measured at against the current ones.
    ///
    /// A wide glyph's fit is judged from its rasterized ink bounds (see
    /// [`ink_contain_scale`]) instead of its pen advance: bitmap-strike
    /// selection and hmtx advance are set independently for CBDT/COLR
    /// color-emoji fonts, and some symbol fonts advance an emoji-presentation
    /// character (like the high-voltage sign) as if it were single-width. The
    /// advance heuristic then computes a spurious ~2x upscale to hit the
    /// two-cell target, which the ink-bounds fit avoids by sizing from what
    /// the glyph actually rasterizes to, not from how far it advances the
    /// cursor. A wide glyph therefore always routes through the quad pass so
    /// this ink-based fit is what actually lands on screen.
    ///
    /// `tail` is the cell's combining suffix (ZWJ sequences, variation
    /// selectors, skin-tone modifiers, a paired flag half; see
    /// [`Cell::tail`]). The fit scale is still cached per base `(ch, is_wide)`
    /// (a skin-toned emoji's footprint is close enough to its bare form to
    /// share that decision), but the texture itself is uploaded and looked up
    /// under the full `(ch, tail)` grapheme (see [`glyph_key`]), so distinct
    /// tails of the same base character each get their own rasterized quad
    /// instead of colliding on one, or silently going undrawn when a later
    /// tail variant of an already-cached base character has no texture yet.
    ///
    /// `is_wide` alone can pull an ASCII `ch` past [`needs_complex_shaping`]'s
    /// usual ASCII bypass: a keycap sequence (`1️⃣`, `#️⃣`, `*️⃣`) has an ASCII
    /// digit/`#`/`*` base, but [`Grid::combine_into_previous`] already
    /// promoted its cell to `Wide` (the same signal a non-ASCII VS16 upgrade
    /// uses), so it needs the same color-emoji-family, ink-fit quad treatment
    /// as any other wide emoji rather than being left as plain ASCII text
    /// with a dropped or unshaped keycap tail.
    pub(super) fn ensure_fallback_glyph_quad(
        &mut self,
        ch: char,
        tail: Option<&str>,
        is_wide: bool,
    ) -> Option<f32> {
        if !needs_fallback_quad_attempt(ch, is_wide) {
            return None;
        }
        if let Some(&(fs, cw, scale, _is_color)) = self.fallback_glyph_cache.get(&(ch, is_wide)) {
            let fresh = (fs - self.font_size).abs() < 0.01 && (cw - self.cell_width).abs() < 0.01;
            let texture_ready =
                scale.is_none() || self.glyph_quad_pass.dims(&glyph_key(ch, tail)).is_some();
            if fresh && texture_ready {
                return scale;
            }
        }
        let target_width = if is_wide {
            self.cell_width * 2.0
        } else {
            self.cell_width
        };
        let mut scale = if is_wide {
            None
        } else {
            let natural = measure_glyph_advance(
                &mut self.font_system,
                self.font_size,
                self.line_height,
                self.font_family.as_deref(),
                self.normal_weight.as_deref(),
                ch,
            );
            advance_scale_ratio(target_width, natural)
        };
        let mut is_color = false;
        if scale.is_some() || is_wide {
            match self.rasterize_fallback_glyph(ch, tail, is_wide) {
                Some((width, height, pixels, color)) => {
                    is_color = color;
                    if is_wide {
                        // Bounded to `font_size` (the em box), not
                        // `cell_height`/`line_height` (`font_size` scaled up
                        // by the fixed `DEFAULT_LINE_HEIGHT /
                        // DEFAULT_FONT_SIZE` leading ratio): a normal glyph's
                        // ink only fills the em box, with the extra leading
                        // in `line_height` left as blank space above/below
                        // it, so fitting emoji ink to the full line height
                        // renders it taller than surrounding text even
                        // though it no longer overflows the row.
                        scale = (width > 0 && height > 0).then(|| {
                            ink_contain_scale(
                                target_width,
                                self.font_size,
                                width as f32,
                                height as f32,
                            )
                        });
                    }
                    if scale.is_some() {
                        let texture = GlyphTexture {
                            height,
                            is_color,
                            pixels: &pixels,
                            width,
                        };
                        self.glyph_quad_pass.upload(
                            &self.device,
                            &self.queue,
                            &glyph_key(ch, tail),
                            texture,
                        );
                    }
                }
                None => scale = None,
            }
        }
        self.fallback_glyph_cache.insert(
            (ch, is_wide),
            (self.font_size, self.cell_width, scale, is_color),
        );
        scale
    }

    /// Rasterize `ch` plus its combining `tail` (see [`glyph_key`]) at the
    /// unmodified base font size (never reshaped, so its natural
    /// ascent/descent proportions are preserved) into a pixel buffer cropped
    /// to its ink bounding box, or `None` if it produced no ink at all.
    /// Shaping `ch` and `tail` together in one run (rather than `ch` alone)
    /// lets cosmic-text's GSUB rules compose a ZWJ sequence, a skin-tone
    /// modifier, or a paired flag half into their single intended glyph
    /// instead of rasterizing just the bare base character. `is_wide` picks
    /// the font family (see [`fallback_family`]). The returned
    /// `bool` is whether the result is a genuine color glyph (COLR/CBDT
    /// color emoji, detected via swash's [`SwashContent`]): when
    /// `true` the buffer is 4-byte RGBA, straight (non-premultiplied) alpha,
    /// preserving the glyph's own color so [`GlyphQuadPass`] draws it as-is;
    /// when `false` it's a single-channel coverage mask, tinted per-instance by
    /// [`GlyphQuadPass`] with the cell's foreground color, same as before.
    fn rasterize_fallback_glyph(
        &mut self,
        ch: char,
        tail: Option<&str>,
        is_wide: bool,
    ) -> Option<(u32, u32, Vec<u8>, bool)> {
        let attrs = Attrs::new()
            .family(fallback_family(self.font_family.as_deref(), is_wide))
            .weight(parse_weight(
                self.normal_weight.as_deref(),
                glyphon::cosmic_text::Weight::NORMAL,
            ));
        let metrics = glyphon::Metrics::new(self.font_size, self.line_height);
        let mut buffer = glyphon::Buffer::new(&mut self.font_system, metrics);
        let canvas_w = (self.font_size * 3.0).ceil().max(1.0) as u32;
        let canvas_h = (self.line_height * 2.0).ceil().max(1.0) as u32;
        buffer.set_size(Some(canvas_w as f32), Some(canvas_h as f32));
        let text = glyph_key(ch, tail);
        buffer.set_text(&text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut is_color = false;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs {
                let physical = glyph.physical((0., run.line_y), 1.0);
                if let Some(image) = self
                    .swash_cache
                    .get_image(&mut self.font_system, physical.cache_key)
                {
                    if image.content == SwashContent::Color {
                        is_color = true;
                    }
                }
            }
        }

        // Always composite full RGBA: for a color glyph this is the real
        // artwork; for a mask glyph, requesting a white base means R=G=B=255
        // throughout, so the alpha channel alone reconstructs the coverage
        // mask below, without a second compositing pass.
        let mut canvas = vec![0u8; (canvas_w * canvas_h) as usize * 4];
        let white = Color::rgba(255, 255, 255, 255);
        let font_system = &mut self.font_system;
        let swash_cache = &mut self.swash_cache;
        buffer.draw(font_system, swash_cache, white, |x, y, w, h, color| {
            if color.a() == 0 {
                return;
            }
            for py in y..y + h as i32 {
                if py < 0 || py >= canvas_h as i32 {
                    continue;
                }
                for px in x..x + w as i32 {
                    if px < 0 || px >= canvas_w as i32 {
                        continue;
                    }
                    let idx = ((py as u32 * canvas_w + px as u32) * 4) as usize;
                    if color.a() > canvas[idx + 3] {
                        canvas[idx] = color.r();
                        canvas[idx + 1] = color.g();
                        canvas[idx + 2] = color.b();
                        canvas[idx + 3] = color.a();
                    }
                }
            }
        });

        let (width, height, rgba) = crop_to_ink_rgba(&canvas, canvas_w, canvas_h)?;
        if is_color {
            Some((width, height, rgba, true))
        } else {
            let alpha: Vec<u8> = rgba.chunks_exact(4).map(|pixel| pixel[3]).collect();
            Some((width, height, alpha, false))
        }
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

/// [`base_family`], except when `is_wide` requests the color-emoji family by
/// name instead. Cosmic-text's built-in Unix fallback order lists generic
/// sans-serif families (Noto Sans, DejaVu Sans, FreeSans, ...) ahead of Noto
/// Color Emoji for any codepoint without a dedicated script (which is every
/// emoji: they're all Unicode script `Common`), so a plain fallback search
/// hands back a monochrome symbol-font substitute even when a real
/// color-emoji font is installed and covers the character.
///
/// `is_wide` (see [`CellWidth::Wide`]) rather than a hand-picked set of
/// "emoji blocks" is what decides this, because it's already the exact
/// signal for "should render as full emoji artwork, not a narrow text
/// glyph": [`UnicodeWidthChar::width`] (used to classify a cell at print
/// time) already reflects Unicode's own `Emoji_Presentation` property for
/// codepoints that default to emoji rendering (`⚡`), and [`Grid`]'s VS16
/// handling promotes a codepoint that instead defaults to narrow text
/// presentation (`⚠`, `❤`, `Ⓜ`, ...) to `Wide` the moment an explicit VS16
/// selector requests emoji presentation for it. A CJK ideograph is also
/// `Wide`, but harmlessly so: cosmic-text falls through to its normal
/// fallback search whenever the requested family doesn't cover the
/// character, so asking for the color-emoji family here never breaks
/// non-emoji wide glyphs, and requesting it for a system that lacks
/// Noto Color Emoji entirely is equally harmless.
fn fallback_family(name: Option<&str>, is_wide: bool) -> Family<'_> {
    if is_wide {
        Family::Name("Noto Color Emoji")
    } else {
        base_family(name)
    }
}

/// The [`GlyphQuadPass`] cache/shaping key for a cell's glyph: `ch` alone, or
/// `ch` plus its combining `tail` (ZWJ sequences, variation selectors,
/// skin-tone modifiers, a paired flag half; see [`Cell::tail`]). Two cells
/// sharing a base character but differing tails (a bare emoji versus its
/// skin-toned form, or a lone regional indicator versus a completed flag)
/// rasterize to visibly different artwork and so must not share one texture.
pub(super) fn glyph_key(ch: char, tail: Option<&str>) -> String {
    let mut key = String::with_capacity(ch.len_utf8() + tail.map_or(0, str::len));
    key.push(ch);
    if let Some(tail) = tail {
        key.push_str(tail);
    }
    key
}

/// Whether `c` needs Advanced (font-fallback + complex) shaping rather than
/// Basic shaping on the primary monospace font. ASCII, the Box Drawing / Block
/// Elements ranges, and Braille render at native cell-width advances in
/// monospace fonts, so they stay on Basic to keep grid columns aligned (TUIs
/// like btop draw their borders, bars, and graphs with these). Everything else
/// non-ASCII (emoji, CJK, accents) needs fallback. Braille that the font lacks
/// never reaches shaping: it is blanked and drawn procedurally (see
/// [`is_braille`] / [`push_braille_dots`]).
pub(super) fn needs_complex_shaping(c: char) -> bool {
    if c.is_ascii() {
        return false;
    }
    !matches!(c, '\u{2500}'..='\u{259F}' | '\u{2800}'..='\u{28FF}')
}

/// Whether [`GpuRenderer::ensure_fallback_glyph_quad`] should attempt a
/// fallback quad for `ch` rather than bailing out immediately. Normally just
/// [`needs_complex_shaping`], except `is_wide` alone is also enough: a
/// keycap sequence (`1️⃣`, `#️⃣`, `*️⃣`) has an ASCII digit/`#`/`*` base, which
/// `needs_complex_shaping` always bypasses, but [`Grid::combine_into_previous`]
/// already promoted its cell to `Wide` on seeing the keycap enclosure, and
/// that promotion is reason enough on its own to route it through the same
/// color-emoji-family, ink-fit quad treatment as any other wide emoji.
fn needs_fallback_quad_attempt(ch: char, is_wide: bool) -> bool {
    needs_complex_shaping(ch) || is_wide
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
/// family actually has a bold face ([`GpuRenderer::font_has_bold`] /
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

/// Uniform scale that fits a glyph's rasterized mask (natively `natural_advance`
/// wide) into `cell_width`. `None` when `natural_advance` is degenerate (no
/// glyph shaped) or already within half a pixel of the cell, so well-behaved
/// glyphs (including ones the primary font covers, which already render at the
/// cell's natural advance) are left to render through cosmic-text's normal
/// text layout instead of the quad pass.
fn advance_scale_ratio(cell_width: f32, natural_advance: f32) -> Option<f32> {
    (natural_advance > 0.5 && (natural_advance - cell_width).abs() > 0.5)
        .then(|| cell_width / natural_advance)
}

/// Uniform scale that fits a rasterized ink box (`ink_width` x `ink_height`)
/// inside a `(max_width, max_height)` cell box, growing or shrinking it as
/// needed so it touches (but never crosses) the nearer pair of edges. Unlike
/// [`advance_scale_ratio`], which judges fit from a glyph's hmtx pen advance,
/// this judges fit from the glyph's actual rasterized bounds: a fallback
/// font's pen advance for a wide (double-width) glyph has no reliable
/// relationship to how large it actually rasterizes (a color-emoji bitmap
/// strike can advance correctly while its raster is far taller than the
/// line; a symbol font can advance a wide glyph as if it were single-width,
/// which would otherwise read as a spurious ~2x upscale need). Callers must
/// ensure `ink_width` and `ink_height` are both positive.
fn ink_contain_scale(max_width: f32, max_height: f32, ink_width: f32, ink_height: f32) -> f32 {
    (max_width / ink_width).min(max_height / ink_height)
}

/// Crop an RGBA `canvas` (`canvas_w * canvas_h * 4` bytes, straight alpha) to
/// its tight ink bounding box, returning `(width, height, pixels)`, or `None`
/// if every pixel is fully transparent. A pixel counts as ink when its alpha
/// channel (every 4th byte) is nonzero.
fn crop_to_ink_rgba(canvas: &[u8], canvas_w: u32, canvas_h: u32) -> Option<(u32, u32, Vec<u8>)> {
    let mut min_x = canvas_w;
    let mut min_y = canvas_h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;
    for y in 0..canvas_h {
        for x in 0..canvas_w {
            if canvas[((y * canvas_w + x) * 4 + 3) as usize] > 0 {
                any = true;
                min_x = min_x.min(x);
                max_x = max_x.max(x);
                min_y = min_y.min(y);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        return None;
    }
    let width = max_x - min_x + 1;
    let height = max_y - min_y + 1;
    let mut cropped = vec![0u8; (width * height * 4) as usize];
    for row in 0..height {
        let src_start = (((min_y + row) * canvas_w + min_x) * 4) as usize;
        let dst_start = (row * width * 4) as usize;
        let src = &canvas[src_start..src_start + (width * 4) as usize];
        cropped[dst_start..dst_start + (width * 4) as usize].copy_from_slice(src);
    }
    Some((width, height, cropped))
}

/// Shape a single character with `Shaping::Advanced` (the mode the grid uses
/// for any character [`needs_complex_shaping`] flags) and return its
/// horizontal advance in pixels, so a fallback glyph's natural width can be
/// compared against the cell width it must fit. Only ever called for a
/// non-`Wide` cell (see [`GpuRenderer::ensure_fallback_glyph_quad`]), which
/// by construction is never emoji-presentation (see [`fallback_family`]), so
/// this always shapes on the plain configured/monospace family.
fn measure_glyph_advance(
    font_system: &mut FontSystem,
    font_size: f32,
    line_height: f32,
    family: Option<&str>,
    normal_weight: Option<&str>,
    ch: char,
) -> f32 {
    let metrics = glyphon::Metrics::new(font_size, line_height);
    let mut buffer = glyphon::Buffer::new(font_system, metrics);
    buffer.set_size(Some(f32::MAX), Some(line_height));
    let attrs = Attrs::new()
        .family(base_family(family))
        .weight(parse_weight(
            normal_weight,
            glyphon::cosmic_text::Weight::NORMAL,
        ));
    let mut buf = [0u8; 4];
    buffer.set_text(ch.encode_utf8(&mut buf), &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
        .layout_runs()
        .next()
        .and_then(|run| run.glyphs.first().map(|g| g.w))
        .unwrap_or(0.0)
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
/// (see [`GpuRenderer::font_has_bold`]) picks a different family entirely.
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
    fn test_advance_scale_ratio_none_when_advance_already_fits_cell() {
        // Within half a pixel of the cell: leave the glyph alone rather than
        // routing it through the quad pass for an imperceptible difference.
        assert_eq!(advance_scale_ratio(9.6, 9.6), None);
        assert_eq!(advance_scale_ratio(9.6, 9.9), None);
        // Degenerate measurement (no glyph shaped): don't divide by ~0.
        assert_eq!(advance_scale_ratio(9.6, 0.0), None);
    }
    #[test]
    fn test_advance_scale_ratio_shrinks_oversized_fallback_glyph() {
        // A Dingbat resolved to a proportional fallback font (measured: DejaVu
        // Sans) advances ~38% wider than the cell at the same font size; the
        // scale ratio should bring its rasterized mask back down to the cell
        // width instead of spilling into the next column.
        let scale = advance_scale_ratio(9.6, 13.2).expect("oversized glyph needs correction");
        assert!((scale - 0.7273).abs() < 0.001, "scale={scale}");
    }
    #[test]
    fn test_ink_contain_scale_shrinks_oversized_color_emoji() {
        // A color-emoji bitmap strike whose pen advance already matches the
        // two-cell target width but whose rasterized ink is much taller than
        // the line (bitmap-strike selection and hmtx advance are independent
        // for CBDT/COLR fonts) must still be shrunk to fit the cell.
        let scale = ink_contain_scale(19.2, 18.0, 18.0, 30.0);
        assert!((scale - 0.6).abs() < 0.001, "scale={scale}");
    }
    #[test]
    fn test_ink_contain_scale_caps_spurious_advance_based_upscale() {
        // A symbol font can advance a wide (double-width) emoji-presentation
        // character as if it were single-width, e.g. the high-voltage sign;
        // judging fit from that advance would compute a ~2x upscale need
        // (natural ~= 1 cell, target = 2 cells) and apply it to both
        // dimensions, overflowing the glyph well past the line height. Sizing
        // from the glyph's actual ink (already close to a single cell here)
        // must land near a 1.1x fill of the two-cell box, not 2x.
        let scale = ink_contain_scale(19.2, 18.0, 9.6, 16.0);
        assert!((scale - 1.125).abs() < 0.001, "scale={scale}");
    }
    #[test]
    fn test_crop_to_ink_rgba_trims_transparent_border() {
        // 4x4 RGBA canvas with a single opaque red pixel at (2, 1).
        let mut canvas = vec![0u8; 16 * 4];
        let idx = (4 + 2) * 4;
        canvas[idx..idx + 4].copy_from_slice(&[255, 0, 0, 255]);
        let (w, h, pixels) = crop_to_ink_rgba(&canvas, 4, 4).expect("has ink");
        assert_eq!((w, h), (1, 1));
        assert_eq!(pixels, vec![255, 0, 0, 255]);
    }
    #[test]
    fn test_crop_to_ink_rgba_none_when_fully_transparent() {
        assert_eq!(crop_to_ink_rgba(&[0u8; 16 * 4], 4, 4), None);
    }
    #[test]
    fn test_needs_complex_shaping_covers_dingbats() {
        // Claude Code's rotating progress spinner cycles through Dingbat
        // asterisks/stars; they must route through Advanced (fallback) shaping
        // to render at all, which is what makes them eligible for correction.
        for star in ['\u{2733}', '\u{273b}', '\u{2722}', '\u{273d}'] {
            assert!(
                needs_complex_shaping(star),
                "{star:?} should need complex shaping"
            );
        }
        assert!(!needs_complex_shaping('*'));
        assert!(!needs_complex_shaping('\u{2500}')); // box drawing stays on Basic
    }
    #[test]
    fn test_needs_complex_shaping_covers_emoji() {
        // `render()` uses this to decide which spans keep the font's default
        // GSUB features when ligatures are disabled: emoji sequences often
        // rely on those same "liga"/"dlig" features to compose a base glyph
        // and its ZWJ/variation-selector modifiers into one presentation
        // glyph, so they must be exempt from ligature-disabling or they fail
        // to render whenever `ligatures = false`.
        assert!(needs_complex_shaping('\u{1F600}')); // grinning face emoji
        assert!(needs_complex_shaping('\u{200D}')); // zero-width joiner
        assert!(needs_complex_shaping('\u{FE0F}')); // emoji variation selector
    }
    #[test]
    fn test_needs_fallback_quad_attempt_lets_a_wide_ascii_keycap_through() {
        // A keycap sequence's base is an ASCII digit, which
        // `needs_complex_shaping` always bypasses; `Grid::combine_into_previous`
        // promoting its cell to `Wide` must still be enough on its own to
        // route it through the fallback quad, or it renders as a bare "1"
        // with its keycap tail silently dropped.
        assert!(needs_fallback_quad_attempt('1', true));
        assert!(needs_fallback_quad_attempt('#', true));
        assert!(needs_fallback_quad_attempt('*', true));
        // A plain narrow ASCII character (never promoted to Wide) must still
        // bypass the fallback quad entirely, or ordinary text goes through
        // unnecessary shaping/rasterization.
        assert!(!needs_fallback_quad_attempt('1', false));
        assert!(!needs_fallback_quad_attempt('a', false));
    }
    #[test]
    fn test_fallback_family_prefers_color_emoji_only_for_wide_cells() {
        // `is_wide` is the single signal for "this cell renders full emoji
        // artwork, not a narrow text glyph" (see the doc comment on
        // `fallback_family`): a wide cell always requests the color-emoji
        // family by name, even over a configured font, so cosmic-text's
        // fallback search doesn't hand back a monochrome symbol-font
        // substitute for it (the actual bug behind the high-voltage sign
        // `⚡` rendering as plain black DejaVu Sans instead of its real
        // color artwork).
        assert_eq!(
            fallback_family(Some("Fira Code"), true),
            Family::Name("Noto Color Emoji")
        );
        // A non-wide cell (including CJK-adjacent narrow glyphs and
        // fallback symbols like the Claude Code spinner) must keep using
        // the configured/default family: it isn't emoji presentation.
        assert_eq!(
            fallback_family(Some("Fira Code"), false),
            Family::Name("Fira Code")
        );
        assert_eq!(fallback_family(None, false), Family::Monospace);
    }
    #[test]
    fn test_glyph_key_distinguishes_tail_variants_of_the_same_base_char() {
        // A skin-toned emoji and its bare form share a base character but
        // rasterize to visibly different artwork; they must land under
        // different `GlyphQuadPass` cache keys or one clobbers the other.
        let bare = glyph_key('\u{1F44D}', None);
        let skin_toned = glyph_key('\u{1F44D}', Some("\u{1F3FD}"));
        assert_ne!(bare, skin_toned);
        assert_eq!(bare, "\u{1F44D}");
        assert_eq!(skin_toned, "\u{1F44D}\u{1F3FD}");
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
    fn test_measure_glyph_advance_matches_a_known_font() {
        // `measure_cell`'s docs already establish that the primary font family
        // resolves and shapes through cosmic-text; sanity-check that
        // `measure_glyph_advance` does the same (a positive, finite advance)
        // rather than pinning it to a specific fallback font's metrics, which
        // varies by machine/CI fontconfig setup.
        let mut font_system = FontSystem::new();
        let advance = measure_glyph_advance(&mut font_system, 16.0, 20.0, None, None, 'M');
        assert!(advance > 0.0 && advance.is_finite(), "advance={advance}");
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
}

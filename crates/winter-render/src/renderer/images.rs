//! Image and rich-content uploads: decoded rasters, SVG, markdown, and
//! text, cached as GPU textures.

use super::glyphs::*;
use super::GpuRenderer;
use glyphon::{Attrs, Family, Shaping};

// ========================================================================
// Constants
// ========================================================================

pub(super) const MAX_SVG_DIM: u32 = 4096;

// ========================================================================
// Implementation
// ========================================================================

impl GpuRenderer {
    /// Decode `encoded` image bytes (PNG/JPEG/GIF/WebP) and cache them as a GPU
    /// texture under `id`. Returns the pixel dimensions, or `None` if the bytes
    /// could not be decoded.
    pub fn upload_image(&mut self, id: u64, encoded: &[u8]) -> Option<(u32, u32)> {
        let rgba = image::load_from_memory(encoded).ok()?.to_rgba8();
        let (width, height) = rgba.dimensions();
        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// Rasterize an SVG document (at its intrinsic size) and cache it as a GPU
    /// texture under `id`. Returns the rasterized pixel dimensions, or `None` if
    /// the SVG could not be parsed. (Rasterizing at intrinsic size keeps sizing
    /// consistent with raster images; display-size re-rasterization is a future
    /// refinement.)
    pub fn upload_svg(&mut self, id: u64, svg: &[u8]) -> Option<(u32, u32)> {
        let fontdb = self.svg_fontdb();
        let options = resvg::usvg::Options {
            fontdb,
            ..Default::default()
        };
        let tree = resvg::usvg::Tree::from_data(svg, &options).ok()?;
        let size = tree.size();
        if size.width() <= 0.0 || size.height() <= 0.0 {
            return None;
        }
        let width = (size.width().round() as u32).clamp(1, MAX_SVG_DIM);
        let height = (size.height().round() as u32).clamp(1, MAX_SVG_DIM);

        let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)?;
        let transform = resvg::tiny_skia::Transform::from_scale(
            width as f32 / size.width(),
            height as f32 / size.height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        // tiny-skia stores premultiplied alpha; the image pass blends straight
        // alpha, so demultiply on the way out.
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for pixel in pixmap.pixels() {
            let color = pixel.demultiply();
            rgba.extend_from_slice(&[color.red(), color.green(), color.blue(), color.alpha()]);
        }

        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// The system-font database for SVG text, scanned once on first use and
    /// then reused.
    pub(super) fn svg_fontdb(&mut self) -> std::sync::Arc<resvg::usvg::fontdb::Database> {
        if let Some(db) = &self.svg_fontdb {
            return db.clone();
        }
        let mut db = resvg::usvg::fontdb::Database::new();
        db.load_system_fonts();
        let db = std::sync::Arc::new(db);
        self.svg_fontdb = Some(db.clone());
        db
    }

    /// Lay out `markdown` with `cosmic-text` (wrapped to `wrap_width`), software-
    /// rasterize it over the theme background, and cache it as a GPU texture
    /// under `id`. Returns the rendered pixel dimensions.
    pub fn upload_markdown(
        &mut self,
        id: u64,
        markdown: &str,
        wrap_width: f32,
    ) -> Option<(u32, u32)> {
        let width = (wrap_width.floor() as u32).clamp(1, MAX_SVG_DIM);
        let spans = crate::markdown::parse(markdown);
        let fam = self.font_family.clone();
        let fg = self.theme.foreground.to_glyphon();
        let code_color = self.theme.ansi[3].to_glyphon();

        let mut buffer = glyphon::Buffer::new(
            &mut self.font_system,
            glyphon::Metrics::new(self.font_size, self.line_height),
        );
        buffer.set_size(&mut self.font_system, Some(width as f32), None);

        let default_attrs = Attrs::new().family(base_family(fam.as_deref())).color(fg);
        let attr_spans: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|span| {
                let mut attrs = if span.mono {
                    Attrs::new().family(Family::Monospace).color(code_color)
                } else {
                    Attrs::new().family(base_family(fam.as_deref())).color(fg)
                };
                if span.bold {
                    attrs = attrs.weight(effective_bold_weight(
                        self.bold_weight.as_deref(),
                        self.font_has_bold,
                    ));
                }
                if span.italic {
                    attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
                }
                (span.text.as_str(), attrs)
            })
            .collect();
        buffer.set_rich_text(
            &mut self.font_system,
            attr_spans,
            &default_attrs,
            Shaping::Advanced,
            None,
        );
        self.rasterize_buffer(id, buffer, width)
    }

    /// Lay out preformatted monospace text (CSV tables, pretty-printed JSON, ...)
    /// wrapped to `wrap_width` and rasterize it to a cached texture. Returns the
    /// rendered pixel dimensions.
    pub fn upload_text(&mut self, id: u64, text: &str, wrap_width: f32) -> Option<(u32, u32)> {
        let width = (wrap_width.floor() as u32).clamp(1, MAX_SVG_DIM);
        let attrs = Attrs::new()
            .family(Family::Monospace)
            .color(self.theme.foreground.to_glyphon());
        let mut buffer = glyphon::Buffer::new(
            &mut self.font_system,
            glyphon::Metrics::new(self.font_size, self.line_height),
        );
        buffer.set_size(&mut self.font_system, Some(width as f32), None);
        buffer.set_rich_text(
            &mut self.font_system,
            [(text, attrs.clone())],
            &attrs,
            Shaping::Advanced,
            None,
        );
        self.rasterize_buffer(id, buffer, width)
    }

    /// Shape `buffer`, measure its height, software-rasterize its glyphs over an
    /// opaque themed background, and cache the result as a texture under `id`.
    pub(super) fn rasterize_buffer(
        &mut self,
        id: u64,
        mut buffer: glyphon::Buffer,
        width: u32,
    ) -> Option<(u32, u32)> {
        buffer.shape_until_scroll(&mut self.font_system, false);
        let mut content_h = 0.0_f32;
        for run in buffer.layout_runs() {
            content_h = content_h.max(run.line_top + run.line_height);
        }
        let height = (content_h.ceil() as u32).clamp(1, MAX_SVG_DIM);

        // Software-composite glyph coverage over an opaque themed background.
        let bg = self.theme.background;
        let fg = self.theme.foreground.to_glyphon();
        let mut rgba = vec![0u8; (width * height * 4) as usize];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel.copy_from_slice(&[bg.r, bg.g, bg.b, 255]);
        }
        let font_system = &mut self.font_system;
        let swash_cache = &mut self.swash_cache;
        buffer.draw(font_system, swash_cache, fg, |x, y, w, h, color| {
            let alpha = color.a() as f32 / 255.0;
            if alpha <= 0.0 {
                return;
            }
            let (cr, cg, cb) = (color.r() as f32, color.g() as f32, color.b() as f32);
            for py in y..y + h as i32 {
                for px in x..x + w as i32 {
                    if px < 0 || py < 0 || px >= width as i32 || py >= height as i32 {
                        continue;
                    }
                    let idx = ((py as u32 * width + px as u32) * 4) as usize;
                    rgba[idx] = (cr * alpha + rgba[idx] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 1] = (cg * alpha + rgba[idx + 1] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 2] = (cb * alpha + rgba[idx + 2] as f32 * (1.0 - alpha)) as u8;
                }
            }
        });

        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// Whether an image texture is already cached for `id`.
    pub fn has_image(&self, id: u64) -> bool {
        self.image_pass.has(id)
    }
}

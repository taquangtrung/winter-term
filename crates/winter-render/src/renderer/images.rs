//! Image and rich-content uploads: decoded rasters, SVG, markdown, and
//! text, cached as GPU textures.

use std::io::Cursor;

use super::glyphs::{base_family, effective_bold_weight};
use super::svg::{rasterize_in_box, MAX_SVG_DIM};
use super::GpuRenderer;
use glyphon::{Attrs, Family, Shaping};
use image::{ImageReader, Limits, RgbaImage};
use vello::peniko::Color;
use vello::{AaConfig, AaSupport, RenderParams, Renderer, RendererOptions, Scene};

// ========================================================================
// Constants
// ========================================================================

/// The widest and tallest raster image decoded at all. Images come from
/// whatever a program prints, so a few hundred bytes of PNG must not be let
/// claim gigabytes on its way to the screen.
const MAX_DECODE_DIM: u32 = 16384;

/// The most memory one raster decode may take.
const MAX_DECODE_ALLOC: u64 = 256 * 1024 * 1024;

// ========================================================================
// Implementation
// ========================================================================

impl GpuRenderer {
    /// Decode `encoded` image bytes (PNG/JPEG/GIF/WebP) and cache them as a GPU
    /// texture under `id`. Returns the pixel dimensions, or `None` if the bytes
    /// could not be decoded.
    pub fn upload_image(&mut self, id: u64, encoded: &[u8]) -> Option<(u32, u32)> {
        let rgba = decode_limited(encoded)?;
        // A texture past the GPU's limit is a validation error, and one image
        // must not be able to take every window down: it is drawn smaller.
        let max = self.device.limits().max_texture_dimension_2d;
        let rgba = fit_within(rgba, max);
        let (width, height) = rgba.dimensions();
        self.image_pass
            .upload(&self.device, &self.queue, id, &rgba, width, height);
        Some((width, height))
    }

    /// Rasterize an SVG to fill a fixed pixel box, preserving its aspect ratio
    /// and centering it in whatever room is left over.
    ///
    /// Drawn at the size it is shown rather than sampled down from another:
    /// what keeps an icon, a block, or a zoomed drawing sharp.
    pub fn upload_svg_in_box(&mut self, id: u64, svg: &[u8], box_w: u32, box_h: u32) -> bool {
        let options = resvg::usvg::Options {
            fontdb: self.svg_fontdb(),
            ..Default::default()
        };
        let Some(raster) = rasterize_in_box(svg, &options, box_w, box_h) else {
            return false;
        };
        self.image_pass.upload(
            &self.device,
            &self.queue,
            id,
            &raster.rgba,
            raster.width,
            raster.height,
        );
        true
    }

    /// The size an SVG document gives itself, in pixels, or `None` for one
    /// that does not parse or has no area.
    pub fn svg_size(&mut self, svg: &[u8]) -> Option<(f32, f32)> {
        let options = resvg::usvg::Options {
            fontdb: self.svg_fontdb(),
            ..Default::default()
        };
        let size = resvg::usvg::Tree::from_data(svg, &options).ok()?.size();
        (size.width() > 0.0 && size.height() > 0.0).then(|| (size.width(), size.height()))
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
        let fam = self.fonts.font_family.clone();
        let fg = self.theme.foreground.to_glyphon();
        let code_color = self.theme.ansi[3].to_glyphon();

        let mut buffer = glyphon::Buffer::new(
            &mut self.font_system,
            glyphon::Metrics::new(self.fonts.font_size, self.fonts.line_height),
        );
        buffer.set_size(Some(width as f32), None);

        let default_attrs = Attrs::new().family(base_family(fam.as_deref())).color(fg);
        let attr_spans: Vec<(&str, Attrs)> = spans
            .iter()
            .map(|span| {
                let family = match span.mono {
                    true => Family::Monospace,
                    false => base_family(fam.as_deref()),
                };
                let color = match span.code {
                    true => code_color,
                    false => fg,
                };
                let mut attrs = Attrs::new().family(family).color(color);
                if span.bold {
                    attrs = attrs.weight(effective_bold_weight(
                        self.fonts.bold_weight.as_deref(),
                        self.fonts.font_has_bold,
                    ));
                }
                if span.italic {
                    attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
                }
                (span.text.as_str(), attrs)
            })
            .collect();
        buffer.set_rich_text(attr_spans, &default_attrs, Shaping::Advanced, None);
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
            glyphon::Metrics::new(self.fonts.font_size, self.fonts.line_height),
        );
        buffer.set_size(Some(width as f32), None);
        buffer.set_rich_text([(text, attrs.clone())], &attrs, Shaping::Advanced, None);
        self.rasterize_buffer(id, buffer, width)
    }

    /// Shape `buffer`, measure its height, software-rasterize its glyphs over an
    /// opaque themed background, and cache the result as a texture under `id`.
    fn rasterize_buffer(
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

    /// Upload pre-rasterized RGBA bytes as a GPU texture under `id`.
    pub fn upload_rgba(&mut self, id: u64, rgba: &[u8], width: u32, height: u32) {
        self.image_pass
            .upload(&self.device, &self.queue, id, rgba, width, height);
    }

    /// Render a Vello `scene` into a `width` x `height` texture cached under
    /// `id`, on this renderer's own device. Says whether it was drawn: not
    /// when the size is past the GPU's limit or the scene renderer could not
    /// start.
    pub fn upload_scene(&mut self, id: u64, scene: &Scene, width: u32, height: u32) -> bool {
        let max = self.device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return false;
        }
        let device = &self.device;
        // Built on first use, so a window that never shows a scene never
        // compiles Vello's shaders; tried once, so a GPU that cannot run them
        // is told once.
        let started = self.scene_renderer.get_or_insert_with(|| {
            let options = RendererOptions {
                antialiasing_support: AaSupport::area_only(),
                ..Default::default()
            };
            Renderer::new(device, options)
                .inspect_err(|error| eprintln!("winter: cannot start the scene renderer: {error}"))
                .ok()
        });
        let Some(renderer) = started else {
            return false;
        };
        let view = self.image_pass.target(&self.device, id, width, height);
        let params = RenderParams {
            antialiasing_method: AaConfig::Area,
            base_color: Color::TRANSPARENT,
            height,
            width,
        };
        renderer
            .render_to_texture(&self.device, &self.queue, scene, &view, &params)
            .inspect_err(|error| eprintln!("winter: cannot draw a scene: {error}"))
            .is_ok()
    }

    /// Release the texture cached for `id`. A no-op for an unknown `id`,
    /// so callers can free on every removal path without first checking
    /// the block ever rasterized.
    pub fn free_image(&mut self, id: u64) {
        self.image_pass.remove(id)
    }
}

// ========================================================================
// Decoding
// ========================================================================

/// Decode `encoded` as a raster image within [`MAX_DECODE_DIM`] and
/// [`MAX_DECODE_ALLOC`], or `None` when it is no image or too large to take.
fn decode_limited(encoded: &[u8]) -> Option<RgbaImage> {
    let mut reader = ImageReader::new(Cursor::new(encoded))
        .with_guessed_format()
        .ok()?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_DECODE_DIM);
    limits.max_image_height = Some(MAX_DECODE_DIM);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    Some(reader.decode().ok()?.to_rgba8())
}

/// `image` scaled down, keeping its shape, so neither side exceeds `max`
/// pixels; as it is when it already fits.
fn fit_within(image: RgbaImage, max: u32) -> RgbaImage {
    let (width, height) = image.dimensions();
    if width <= max && height <= max {
        return image;
    }
    let scale = f64::from(max) / f64::from(width.max(height));
    let fitted = |side: u32| ((f64::from(side) * scale).floor() as u32).clamp(1, max);
    image::imageops::thumbnail(&image, fitted(width), fitted(height))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_image_past_the_texture_limit_is_drawn_smaller_not_refused() {
        let wide = RgbaImage::new(9000, 30);
        let fitted = fit_within(wide, 8192);
        assert_eq!(
            fitted.dimensions(),
            (8192, 27),
            "shape kept, within the limit"
        );
        let small = RgbaImage::new(10, 10);
        assert_eq!(fit_within(small, 8192).dimensions(), (10, 10));
    }

    #[test]
    fn test_a_decode_past_the_size_limit_is_refused() {
        let mut png = Vec::new();
        RgbaImage::new(MAX_DECODE_DIM + 1, 1)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .expect("encode");
        assert!(decode_limited(&png).is_none());
        let mut ok = Vec::new();
        RgbaImage::new(4, 4)
            .write_to(&mut Cursor::new(&mut ok), image::ImageFormat::Png)
            .expect("encode");
        assert_eq!(decode_limited(&ok).map(|i| i.dimensions()), Some((4, 4)));
    }
}

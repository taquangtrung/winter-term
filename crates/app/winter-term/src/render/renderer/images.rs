//! Image and rich-content uploads: decoded rasters, SVG, markdown, and
//! text, cached as GPU textures.

use std::io::Cursor;

use super::GpuRenderer;
use super::svg::{MAX_SVG_DIM, rasterize_in_box};
use crate::render::image::BYTES_PER_PIXEL;
use image::{ImageReader, Limits, RgbaImage};
use misty::{FontFamily, Paragraph, ParagraphMeasurer, Rgba, Span, TextColor};
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
        self.image_pass.upload(&self.gpu, id, &rgba, width, height);
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
        self.image_pass
            .upload(&self.gpu, id, &raster.rgba, raster.width, raster.height);
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

    /// Lay out `markdown` in the grid's font, wrapped to `wrap_width`, draw
    /// it over the theme background, and cache it as a GPU texture under
    /// `id`. Returns the rendered pixel dimensions.
    pub fn upload_markdown(
        &mut self,
        id: u64,
        markdown: &str,
        wrap_width: f32,
    ) -> Option<(u32, u32)> {
        let theme = self.themes.winter();
        let text = text_color(theme.foreground);
        let code = text_color(theme.ansi[3]);
        let prose = match &self.fonts.font_family {
            Some(family) => FontFamily::Named(family.clone()),
            None => FontFamily::Monospace,
        };
        let paragraph = crate::render::markdown::parse(markdown).into_iter().fold(
            Paragraph::new(),
            |paragraph, md| {
                let color = match md.code {
                    true => code,
                    false => text,
                };
                let family = match md.mono {
                    true => FontFamily::Monospace,
                    false => prose.clone(),
                };
                let mut span = Span::new(md.text, color).family(family);
                if md.bold {
                    span = span.bold();
                }
                if md.italic {
                    span = span.italic();
                }
                paragraph.with(span)
            },
        );
        self.upload_paragraph(id, &paragraph, wrap_width)
    }

    /// Lay out preformatted monospace text (CSV tables, pretty-printed JSON, …)
    /// wrapped to `wrap_width` and draw it to a cached texture. Returns the
    /// rendered pixel dimensions.
    pub fn upload_text(&mut self, id: u64, text: &str, wrap_width: f32) -> Option<(u32, u32)> {
        let color = text_color(self.themes.winter().foreground);
        let paragraph = Paragraph::new().with(Span::new(text, color).family(FontFamily::Monospace));
        self.upload_paragraph(id, &paragraph, wrap_width)
    }

    /// Draw `paragraph` in the grid's font, wrapped to `wrap_width` over the
    /// opaque theme background, and cache it as a texture under `id`.
    fn upload_paragraph(
        &mut self,
        id: u64,
        paragraph: &Paragraph,
        wrap_width: f32,
    ) -> Option<(u32, u32)> {
        let width = (wrap_width.floor() as u32).clamp(1, MAX_SVG_DIM);
        let measurer = ParagraphMeasurer::for_font(&self.grid_font(), self.fonts.scale_factor);
        let background = self.themes.winter().background;
        let background = Rgba::new(background.r, background.g, background.b, u8::MAX);
        let image = measurer.render(paragraph, width as f32, self.misty_theme(), background);
        let height = image.height().min(MAX_SVG_DIM);
        let rows = (width * height) as usize * BYTES_PER_PIXEL;
        let pixels = image.pixels().get(..rows)?;
        self.image_pass.upload(&self.gpu, id, pixels, width, height);
        Some((width, height))
    }

    /// Upload pre-rasterized RGBA bytes as a GPU texture under `id`.
    pub fn upload_rgba(&mut self, id: u64, rgba: &[u8], width: u32, height: u32) {
        self.image_pass.upload(&self.gpu, id, rgba, width, height);
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
        let Some(view) = self.image_pass.target(&self.gpu, id, width, height) else {
            return false;
        };
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

    /// The texture cached for `id`, for a host drawing a placement of it
    /// itself.
    pub fn image_texture(&self, id: u64) -> Option<&misty::GpuTexture> {
        self.image_pass.texture(id)
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

/// Misty's exact text color for `color`.
fn text_color(color: winter_theme::Rgb) -> TextColor {
    TextColor::Rgba(Rgba::new(color.r, color.g, color.b, u8::MAX))
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

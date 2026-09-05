//! GPU surface and device setup, resizing, and renderer configuration.

use super::background::{
    create_bg_pipeline, create_dot_pipeline, BG_BUFFER_SIZE, DIVIDER_THICKNESS, DOT_BUFFER_SIZE,
};
use super::glyphs::{
    font_covers_bold_weight, font_covers_braille, measure_cell, FontConfig, FontLoad,
    DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT,
};
use super::GpuRenderer;
use super::PaneRect;
use crate::glyph_quad::GlyphQuadPass;
use crate::image::ImagePass;
use crate::theme::Theme;
use glyphon::{Cache, ColorMode, SwashCache, TextAtlas, TextRenderer, Viewport};
use wgpu::{
    BufferUsages, DeviceDescriptor, MultisampleState, Surface, SurfaceConfiguration, TextureFormat,
};

// ========================================================================
// GpuRenderer: surface and configuration
// ========================================================================

impl GpuRenderer {
    /// Create the renderer, the wgpu surface, device, and queue.
    ///
    /// The `surface` is created externally (by the app crate from a winit
    /// window) and moved in. Call [`Self::resize`] before the first
    /// [`Self::render`].
    pub fn new(
        surface: Surface<'static>,
        adapter: wgpu::Adapter,
        width: u32,
        height: u32,
        scale_factor: f64,
        font: FontConfig,
        font_load: FontLoad,
    ) -> Self {
        let (device, queue) =
            pollster::block_on(adapter.request_device(&DeviceDescriptor::default()))
                .expect("request wgpu device");

        let caps = surface.get_capabilities(&adapter);
        // Prefer an sRGB surface so the GPU encodes linear shader output to sRGB
        // on write: glyph and overlay antialiasing then blends in linear space
        // (gamma-correct), which keeps light-on-dark text crisp instead of thin
        // and fuzzy. The bg/image shaders output linear to match.
        let format = caps
            .formats
            .iter()
            .find(|f| {
                matches!(
                    f,
                    TextureFormat::Bgra8UnormSrgb | TextureFormat::Rgba8UnormSrgb
                )
            })
            .or_else(|| {
                caps.formats
                    .iter()
                    .find(|f| matches!(f, TextureFormat::Bgra8Unorm | TextureFormat::Rgba8Unorm))
            })
            .copied()
            .unwrap_or(caps.formats[0]);

        let config = SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width: width.max(1),
            height: height.max(1),
            present_mode: wgpu::PresentMode::AutoVsync,
            alpha_mode: caps.alpha_modes[0],
            // Auto is what wgpu chose implicitly before 30 made it explicit.
            color_space: wgpu::SurfaceColorSpace::Auto,
            view_formats: vec![],
            desired_maximum_frame_latency: 2,
        };
        surface.configure(&device, &config);

        let font_size = font.size * scale_factor as f32;
        // Round to whole pixels so the line stride cosmic-text uses for shaping
        // matches `cell_height` (which `measure_cell` also rounds). Without this,
        // a fractional `line_height` (e.g. font_size 14 → 18.667) accumulates a
        // sub-pixel drift per row until the cursor — drawn at `row * cell_height` —
        // sits half a line below the glyphs cosmic-text laid out at
        // `row * line_height`.
        let line_height = (font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let font_family = font.family;
        let normal_weight = font.normal_weight;
        let bold_weight = font.bold_weight;
        let swash_cache = SwashCache::new();
        let cache = Cache::new(&device);
        // ColorMode::Accurate makes glyphon convert text colors to linear and
        // blend glyph coverage in linear space, which the sRGB surface encodes
        // back on write: gamma-correct antialiasing, so text stays crisp rather
        // than thin and fuzzy on dark backgrounds.
        let color_mode = if format.is_srgb() {
            ColorMode::Accurate
        } else {
            ColorMode::Web
        };
        let mut text_atlas =
            TextAtlas::with_color_mode(&device, &queue, &cache, format, color_mode);
        let text_renderer =
            TextRenderer::new(&mut text_atlas, &device, MultisampleState::default(), None);
        let viewport = Viewport::new(&device, &cache);

        let mut font_system = font_load.join();
        let (cell_width, cell_height) = measure_cell(
            &mut font_system,
            font_size,
            line_height,
            font_family.as_deref(),
            normal_weight.as_deref(),
        );

        let bg_pipeline = create_bg_pipeline(&device, format);
        let bg_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("winter bg vertices"),
            size: BG_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let dot_pipeline = create_dot_pipeline(&device, format);
        let dot_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("winter braille dot vertices"),
            size: DOT_BUFFER_SIZE,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let cols = ((width as f32 / cell_width).floor() as usize).max(1);
        let rows = ((height as f32 / cell_height).floor() as usize).max(1);
        // Keep `cell_width`/`cell_height` on the measured cell stride instead of
        // stretching either (`width / cols`, `height / rows`) to exactly fill the
        // window. `cell_width` in particular must stay exactly what `measure_cell`
        // saw cosmic-text actually render a glyph at (fractional pixels and all):
        // the shaping site deliberately does not call `set_monospace_width` (it
        // desyncs the cursor at fractional sizes), so a stretched `cell_width`
        // would just be a different number from what's on screen, drifting the
        // cursor off its glyph one column at a time.
        // These values are recomputed by the first `resize()` before rendering.
        let line_height = cell_height;

        let image_pass = ImagePass::new(&device, format);
        let tabbar_strip_pass = ImagePass::new(&device, format);
        let glyph_quad_pass = GlyphQuadPass::new(&device, format);

        let font_has_braille = font_covers_braille(
            &mut font_system,
            font_size,
            line_height,
            font_family.as_deref(),
            normal_weight.as_deref(),
        );
        let font_has_bold = font_covers_bold_weight(
            &mut font_system,
            font_size,
            line_height,
            font_family.as_deref(),
            bold_weight.as_deref(),
        );

        Self {
            device,
            queue,
            surface,
            config,
            font_system,
            swash_cache,
            cache,
            text_atlas,
            text_renderer,
            viewport,
            bg_pipeline,
            bg_buffer,
            dot_pipeline,
            dot_buffer,
            font_has_braille,
            font_has_bold,
            cell_width,
            cell_height,
            cols: cols.max(1),
            rows: rows.max(1),
            font_family,
            font_size,
            line_height,
            logical_font_size: font.size,
            scale_factor,
            normal_weight,
            bold_weight,
            ligatures: false,
            fallback_glyph_cache: std::collections::HashMap::new(),
            glyph_quad_pass,
            theme: Theme::default(),
            text_buffers: Vec::new(),
            status_buffer: None,
            image_pass,
            tabbar_strip_pass,
            svg_fontdb: None,
            divider_width: DIVIDER_THICKNESS,
            modern: true,
            tabbar_enabled: true,
            status_bar_enabled: true,
            decorated: true,
        }
    }
    /// Apply a new color theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
    }
    /// Set the thickness (in logical pixels) of the line drawn between adjacent panes.
    pub fn set_divider_width(&mut self, width: f32) {
        self.divider_width = width.max(1.0);
    }
    /// Record whether the OS window manager draws its own frame. Undecorated
    /// windows get the renderer's own outline drawn around their outer edge.
    pub fn set_decorated(&mut self, decorated: bool) {
        self.decorated = decorated;
    }
    /// Current theme colors.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
    /// The number of terminal columns and rows that fit the full viewport.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }
    /// The number of columns and rows that fit within a specific rect.
    pub fn grid_size_for(&self, rect: PaneRect) -> (usize, usize) {
        let cols = ((rect.width - crate::PANE_H_PAD * 2.0) / self.cell_width).floor() as usize;
        let rows = (rect.height / self.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }
    /// The cell dimensions in pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.cell_width, self.cell_height)
    }
    /// Resize the surface and recompute grid dimensions. Returns `Some((cols, rows))`
    /// if the size or scale factor actually changed.
    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) -> Option<(usize, usize)> {
        let width = width.max(1);
        let height = height.max(1);

        let size_changed = self.config.width != width || self.config.height != height;
        let scale_changed = (self.scale_factor - scale_factor).abs() > 1e-5;

        if !size_changed && !scale_changed {
            return None;
        }

        self.config.width = width;
        self.config.height = height;
        self.scale_factor = scale_factor;
        self.surface.configure(&self.device, &self.config);
        self.viewport
            .update(&self.queue, glyphon::Resolution { width, height });

        if scale_changed {
            // Recompute physical font size and line height
            self.font_size = self.logical_font_size * scale_factor as f32;
            let base_line_h = (self.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();

            // Re-measure cell
            let (cell_width, cell_height) = measure_cell(
                &mut self.font_system,
                self.font_size,
                base_line_h,
                self.font_family.as_deref(),
                self.normal_weight.as_deref(),
            );
            self.cell_width = cell_width;
            self.cell_height = cell_height;

            // Clear existing buffers so they are re-allocated with updated physical metrics next frame
            self.text_buffers.clear();
            self.status_buffer = None;
        }

        let base_line_h = (self.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let (base_cell_w, base_cell_h) = measure_cell(
            &mut self.font_system,
            self.font_size,
            base_line_h,
            self.font_family.as_deref(),
            self.normal_weight.as_deref(),
        );
        self.cols = ((width as f32 / base_cell_w).floor() as usize).max(1);
        // Keep `cell_width` and `cell_height`/`line_height` on the measured cell
        // stride (do NOT stretch either to fill the window). `cell_height` needs
        // this because cosmic-text lays out lines on a whole-pixel stride; the
        // cursor is drawn at `row * cell_height`, and a fractional, per-resize
        // stride drifts them apart line-by-line. `cell_width` needs it for a
        // different reason: the shaping site deliberately does not call
        // `set_monospace_width` (it quantizes advances at fractional sizes and
        // desyncs the cursor; see the comment there), so glyphs render at their
        // natural advance and `cell_width` must equal `base_cell_w`, the exact
        // (generally fractional) advance cosmic-text renders each glyph at, or
        // `col * cell_width` drifts away from the glyph it should sit next to,
        // worse the more columns you type. Because both strides are
        // now stable across pure window resizes, the per-pane glyphon buffers'
        // `Metrics` and the cursor's stride always agree with what's on screen.
        // `cols`/`rows` stay the full-window counts; the app subtracts the chrome
        // rows itself, and its remainder-padding logic absorbs the leftover space.
        self.cell_width = base_cell_w;
        let cell_h = base_cell_h;
        self.rows = ((height as f32 / cell_h).floor() as usize).max(1);
        self.cell_height = cell_h;
        self.line_height = cell_h;
        Some((self.cols, self.rows))
    }
    /// Acquire the next swapchain texture, recovering a stale surface in place.
    ///
    /// On `Outdated`/`Lost` (after a resize, GPU reset, or a monitor sleep/wake)
    /// the surface configuration no longer matches its swapchain. We reconfigure
    /// and retry once so this frame paints a valid image, rather than leaving the
    /// freshly reconfigured swapchain showing uninitialized garbage. Transient
    /// states (`Timeout`, `Occluded`) and validation errors skip the frame; the
    /// previously presented frame stays on screen until the next redraw.
    pub(super) fn acquire_surface_texture(&mut self) -> Option<wgpu::SurfaceTexture> {
        match self.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(texture)
            | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.surface.configure(&self.device, &self.config);
                match self.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(texture)
                    | wgpu::CurrentSurfaceTexture::Suboptimal(texture) => Some(texture),
                    _ => None,
                }
            }
            wgpu::CurrentSurfaceTexture::Timeout
            | wgpu::CurrentSurfaceTexture::Occluded
            | wgpu::CurrentSurfaceTexture::Validation => None,
        }
    }
}

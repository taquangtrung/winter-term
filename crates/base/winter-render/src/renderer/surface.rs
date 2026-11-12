//! GPU surface and device setup, resizing, and renderer configuration.

use super::background::DIVIDER_THICKNESS;
use super::glyphs::{
    clamp_font_size, font_covers_bold_weight, font_covers_braille, measure_cell, FontConfig,
    FontLoad, DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT,
};
use super::PaneRect;
use super::{ChromeLayout, FontState, FrameHits, FrameSize, GpuRenderer};
use crate::image::ImagePass;
use crate::theme::Theme;
use glyphon::SwashCache;

// ========================================================================
// GpuRenderer: surface and configuration
// ========================================================================

impl GpuRenderer {
    /// Create the renderer on `device` and `queue`, the GPU the window
    /// draws with, rendering frames `width` by `height` into textures of
    /// `format`.
    ///
    /// An sRGB `format` has the GPU encode linear shader output to sRGB on
    /// write: glyph and overlay antialiasing then blends in linear space
    /// (gamma-correct), which keeps light-on-dark text crisp instead of thin
    /// and fuzzy. The bg/image shaders output linear to match. Call
    /// [`Self::resize`] before the first [`Self::render`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpu: misty::Gpu,
        width: u32,
        height: u32,
        scale_factor: f64,
        font: FontConfig,
        font_load: FontLoad,
    ) -> Self {
        let device = gpu.device().clone();
        let queue = gpu.queue().clone();
        let size = FrameSize {
            height: surface_side(height, &device),
            width: surface_side(width, &device),
        };

        // Whatever the config or the environment asked for, a size the text
        // shaper can lay out: a zero line height would panic it.
        let logical_font_size = clamp_font_size(font.size);
        let font_size = logical_font_size * scale_factor as f32;
        // Round to whole pixels so the line stride cosmic-text uses for shaping
        // matches `cell_height` (which `measure_cell` also rounds). Without this,
        // a fractional `line_height` (e.g. font_size 14 → 18.667) accumulates a
        // sub-pixel drift per row until the cursor, drawn at `row * cell_height`,
        // sits half a line below the glyphs cosmic-text laid out at
        // `row * line_height`.
        let line_height = (font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let font_family = font.family;
        let normal_weight = font.normal_weight;
        let bold_weight = font.bold_weight;
        let swash_cache = SwashCache::new();
        let mut font_system = font_load.join();
        let (cell_width, cell_height) = measure_cell(
            &mut font_system,
            font_size,
            line_height,
            font_family.as_deref(),
            normal_weight.as_deref(),
        );

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

        let image_pass = ImagePass::default();

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
            chrome: ChromeLayout {
                decorated: true,
                divider_width: DIVIDER_THICKNESS,
                popup_input_height: crate::DEFAULT_POPUP_INPUT_HEIGHT,
                status_bar_enabled: true,
                window_opacity: 1.0,
            },
            fonts: FontState {
                bold_weight,
                cell_height,
                cell_width,
                font_family,
                font_has_bold,
                font_has_braille,
                font_size,
                ligatures: false,
                line_height,
                logical_font_size,
                normal_weight,
                scale_factor,
            },
            hits: FrameHits {
                find_rect: None,
                input_button_rects: Vec::new(),
                input_rect: None,
                pictures: Vec::new(),
                popups: Vec::new(),
                toolbar_hits: Vec::new(),
            },
            device,
            gpu,
            queue,
            size,
            font_system,
            swash_cache,
            cols: cols.max(1),
            rows: rows.max(1),
            theme: Theme::default(),
            image_pass,
            overlay_caches: Default::default(),
            theme_generation: 0,
            svg_fontdb: None,
            scene_renderer: None,
        }
    }
    /// A number that moves with every theme change, for a texture drawn in
    /// the theme's colors to know it has to be drawn again.
    pub fn theme_generation(&self) -> u64 {
        self.theme_generation
    }

    /// Apply a new color theme.
    pub fn set_theme(&mut self, theme: Theme) {
        self.theme = theme;
        self.theme_generation += 1;
    }
    /// Set the window's opacity, `0.0` to `1.0`, on top of the theme's own
    /// background alpha.
    pub fn set_window_opacity(&mut self, opacity: f32) {
        self.chrome.window_opacity = opacity.clamp(0.0, 1.0);
    }
    /// How opaque the terminal background is drawn: the window's opacity times
    /// the theme's background alpha.
    pub fn background_opacity(&self) -> f32 {
        self.chrome.window_opacity * self.theme.background_alpha as f32 / 255.0
    }
    /// Whether the window has to be transparent for the background to show
    /// the desktop through it.
    pub fn is_translucent(&self) -> bool {
        self.background_opacity() < 1.0
    }
    /// Set the thickness (in logical pixels) of the line drawn between adjacent panes.
    pub fn set_divider_width(&mut self, width: f32) {
        self.chrome.divider_width = width.max(1.0);
    }
    /// Set how tall the field typed into is in every popup, in rows of the
    /// font, kept within [`crate::POPUP_INPUT_HEIGHT_RANGE`].
    pub fn set_popup_input_height(&mut self, rows: f32) {
        let (least, most) = crate::POPUP_INPUT_HEIGHT_RANGE;
        self.chrome.popup_input_height = rows.clamp(least, most);
    }

    /// The field height in pixels at the current font, for the popups.
    pub(super) fn popup_field_height(&self) -> f32 {
        super::raster::chrome::field_height(
            self.fonts.cell_height,
            self.fonts.line_height,
            self.chrome.popup_input_height,
        )
    }

    /// Record whether the OS window manager draws its own frame. Undecorated
    /// windows get the renderer's own outline drawn around their outer edge.
    pub fn set_decorated(&mut self, decorated: bool) {
        self.chrome.decorated = decorated;
    }
    /// Current theme colors.
    pub fn theme(&self) -> &Theme {
        &self.theme
    }
    /// Screen pixels to a logical pixel, as the window last reported it.
    pub fn scale_factor(&self) -> f64 {
        self.fonts.scale_factor
    }
    /// The number of terminal columns and rows that fit the full viewport.
    pub fn grid_size(&self) -> (usize, usize) {
        (self.cols, self.rows)
    }
    /// The number of columns and rows that fit within a specific rect.
    pub fn grid_size_for(&self, rect: PaneRect) -> (usize, usize) {
        let cols = ((rect.width - self.pane_pad() * 2.0) / self.fonts.cell_width).floor() as usize;
        let rows = (rect.height / self.fonts.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }
    /// Where the last frame drew what the pointer can act on, to test a
    /// click or a hover against.
    pub fn hits(&self) -> &FrameHits {
        &self.hits
    }

    /// The font the grid is drawn in, for a host drawing it itself: its
    /// size in logical pixels, its family (`None` for the system's
    /// monospace), its line height as a multiple of its size, and whether
    /// its ligatures are drawn.
    pub fn grid_font(&self) -> (f32, Option<String>, f32, bool) {
        (
            self.fonts.logical_font_size,
            self.fonts.font_family.clone(),
            super::glyphs::DEFAULT_LINE_HEIGHT / super::glyphs::DEFAULT_FONT_SIZE,
            self.fonts.ligatures,
        )
    }

    /// The cell dimensions in pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.fonts.cell_width, self.fonts.cell_height)
    }
    /// How far a pane's content is inset from each side of its rect, in
    /// screen pixels: [`crate::PANE_H_PAD`] at the display's scale.
    pub fn pane_pad(&self) -> f32 {
        crate::pane_h_pad(self.fonts.scale_factor)
    }
    /// The size of the frames rendered, `(width, height)` in pixels.
    pub fn size(&self) -> (u32, u32) {
        (self.size.width, self.size.height)
    }

    /// Resize the frames and recompute grid dimensions. Returns `Some((cols, rows))`
    /// if the size or scale factor actually changed.
    pub fn resize(&mut self, width: u32, height: u32, scale_factor: f64) -> Option<(usize, usize)> {
        let width = surface_side(width, &self.device);
        let height = surface_side(height, &self.device);

        let size_changed = self.size.width != width || self.size.height != height;
        let scale_changed = (self.fonts.scale_factor - scale_factor).abs() > 1e-5;

        if !size_changed && !scale_changed {
            return None;
        }

        self.size = FrameSize { height, width };
        self.fonts.scale_factor = scale_factor;

        if scale_changed {
            // Recompute physical font size and line height
            self.fonts.font_size = self.fonts.logical_font_size * scale_factor as f32;
            let base_line_h =
                (self.fonts.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();

            // Re-measure cell
            let (cell_width, cell_height) = measure_cell(
                &mut self.font_system,
                self.fonts.font_size,
                base_line_h,
                self.fonts.font_family.as_deref(),
                self.fonts.normal_weight.as_deref(),
            );
            self.fonts.cell_width = cell_width;
            self.fonts.cell_height = cell_height;
        }

        let base_line_h =
            (self.fonts.font_size * (DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)).round();
        let (base_cell_w, base_cell_h) = measure_cell(
            &mut self.font_system,
            self.fonts.font_size,
            base_line_h,
            self.fonts.font_family.as_deref(),
            self.fonts.normal_weight.as_deref(),
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
        self.fonts.cell_width = base_cell_w;
        let cell_h = base_cell_h;
        self.rows = ((height as f32 / cell_h).floor() as usize).max(1);
        self.fonts.cell_height = cell_h;
        self.fonts.line_height = cell_h;
        Some((self.cols, self.rows))
    }
}

/// One side of the window surface, held to what the GPU can configure: at
/// least a pixel, and no more than its largest texture. A window stretched
/// across several screens can pass that limit; past it the surface is drawn
/// at the limit rather than refused.
fn surface_side(side: u32, device: &wgpu::Device) -> u32 {
    side.clamp(1, device.limits().max_texture_dimension_2d)
}

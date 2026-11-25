//! GPU surface and device setup, resizing, and renderer configuration.

use super::background::DIVIDER_THICKNESS;
use super::glyphs::{DEFAULT_FONT_SIZE, DEFAULT_LINE_HEIGHT, FontConfig, clamp_font_size};
use super::{ChromeLayout, FontState, FrameHits, FrameSize, GpuRenderer};
use crate::render::Themes;
use crate::render::image::ImagePass;
use misty::{Font, Rect};
use winter_theme::Theme;

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
    /// [`Self::resize`] before the first frame is prepared.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gpu: misty::Gpu,
        width: u32,
        height: u32,
        scale_factor: f64,
        font: FontConfig,
    ) -> Self {
        let device = gpu.device().clone();
        let queue = gpu.queue().clone();
        let adapter = device.adapter_info();
        crate::diagnose::note_gpu(format!(
            "{} ({:?}, {:?}, {} {})",
            adapter.name, adapter.device_type, adapter.backend, adapter.driver, adapter.driver_info
        ));
        let size = FrameSize {
            height: surface_side(height, &device),
            width: surface_side(width, &device),
        };

        // Whatever the config or the environment asked for, a size the text
        // shaper can lay out: a zero line height would panic it.
        let logical_font_size = clamp_font_size(font.size);
        let font_family = font.family;
        // The cell misty draws the grid in; the first `resize()` measures
        // again at the window's scale factor.
        let cell =
            grid_font(logical_font_size, font_family.clone(), false).cell_metrics(scale_factor);
        let (cell_width, cell_height) = (cell.width, cell.height);
        let cols = ((width as f32 / cell_width).floor() as usize).max(1);
        let rows = ((height as f32 / cell_height).floor() as usize).max(1);
        let image_pass = ImagePass::default();

        Self {
            chrome: ChromeLayout {
                decorated: true,
                divider_width: DIVIDER_THICKNESS,
                popup_input_height: crate::render::DEFAULT_POPUP_INPUT_HEIGHT,
                status_bar_height: None,
                window_opacity: 1.0,
            },
            fonts: FontState {
                cell_height,
                cell_width,
                font_family,
                ligatures: false,
                logical_font_size,
                scale_factor,
            },
            hits: FrameHits::default(),
            device,
            gpu,
            queue,
            size,
            cols: cols.max(1),
            rows: rows.max(1),
            themes: Themes::new(Theme::default()),
            image_pass,
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

    /// Apply a new color theme, misty's built from it with it.
    pub fn set_theme(&mut self, theme: Theme) {
        self.themes.set(theme);
        self.theme_generation += 1;
    }

    /// Show scrollbars as `show` says.
    pub fn set_scrollbar_show(&mut self, show: misty::ScrollbarShow) {
        self.themes.set_scrollbar_show(show);
    }

    /// Set the window's opacity, `0.0` to `1.0`, on top of the theme's own
    /// background alpha.
    pub fn set_window_opacity(&mut self, opacity: f32) {
        self.chrome.window_opacity = opacity.clamp(0.0, 1.0);
    }
    /// How opaque the terminal background is drawn: the window's opacity times
    /// the theme's background alpha.
    pub fn background_opacity(&self) -> f32 {
        self.chrome.window_opacity * self.themes.winter().background_alpha as f32 / 255.0
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
    /// font, kept within [`crate::render::POPUP_INPUT_HEIGHT_RANGE`].
    pub fn set_popup_input_height(&mut self, rows: f32) {
        let (least, most) = crate::render::POPUP_INPUT_HEIGHT_RANGE;
        self.chrome.popup_input_height = rows.clamp(least, most);
    }

    /// Record whether the OS window manager draws its own frame. Undecorated
    /// windows get the renderer's own outline drawn around their outer edge.
    pub fn set_decorated(&mut self, decorated: bool) {
        self.chrome.decorated = decorated;
    }

    /// Whether the system draws the window's frame, so Winter draws no
    /// outline of its own.
    pub fn is_decorated(&self) -> bool {
        self.chrome.decorated
    }
    /// Current theme colors.
    pub fn theme(&self) -> &Theme {
        self.themes.winter()
    }
    /// Misty's theme, built from the current one when it was set.
    pub fn misty_theme(&self) -> &misty::Theme {
        self.themes.misty()
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
    pub fn grid_size_for(&self, rect: Rect) -> (usize, usize) {
        let cols = ((rect.width - self.pane_pad() * 2.0) / self.fonts.cell_width).floor() as usize;
        let rows = (rect.height / self.fonts.cell_height).floor() as usize;
        (cols.max(1), rows.max(1))
    }
    /// Where the last frame drew what the pointer can act on, to test a
    /// click or a hover against.
    pub fn hits(&self) -> &FrameHits {
        &self.hits
    }

    /// The font misty draws the grid in.
    pub fn grid_font(&self) -> Font {
        grid_font(
            self.fonts.logical_font_size,
            self.fonts.font_family.clone(),
            self.fonts.ligatures,
        )
    }

    /// Measure the grid's cell in its font at the window's scale factor, as
    /// misty draws it, and count the whole cells the window holds.
    ///
    /// The cell keeps its measured size, never stretched to fill the
    /// window: misty lays glyphs out on that stride, and the backgrounds and
    /// the cursor drawn at `col * cell_width` and `row * cell_height` must
    /// land on the same pixels. `cols` and `rows` are the full window's; the
    /// app takes the chrome's rows off itself and pads out the remainder.
    pub(super) fn measure_grid(&mut self) {
        let cell = self.grid_font().cell_metrics(self.fonts.scale_factor);
        self.fonts.cell_width = cell.width;
        self.fonts.cell_height = cell.height;
        self.cols = ((self.size.width as f32 / cell.width).floor() as usize).max(1);
        self.rows = ((self.size.height as f32 / cell.height).floor() as usize).max(1);
    }

    /// The cell dimensions in pixels.
    pub fn cell_size(&self) -> (f32, f32) {
        (self.fonts.cell_width, self.fonts.cell_height)
    }
    /// How far a pane's content is inset from each side of its rect, in
    /// screen pixels: [`crate::render::PANE_H_PAD`] at the display's scale.
    pub fn pane_pad(&self) -> f32 {
        crate::render::pane_h_pad(self.fonts.scale_factor)
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

        self.measure_grid();
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

/// The font misty draws the grid in at `size` logical pixels: `family`, or
/// the system's monospace for `None`, with Winter's line height and, when
/// asked, the family's ligatures.
fn grid_font(size: f32, family: Option<String>, ligatures: bool) -> Font {
    let font = Font::new(size)
        .with_line_height(DEFAULT_LINE_HEIGHT / DEFAULT_FONT_SIZE)
        .with_ligatures(ligatures);
    match family {
        Some(family) => font.with_family(family),
        None => font,
    }
}

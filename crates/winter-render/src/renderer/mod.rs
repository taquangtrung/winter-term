//! GPU text renderer: renders [`Grid`]s to a wgpu surface using `glyphon` +
//! `cosmic-text` for glyph rasterization and `wgpu` for compositing.
//!
//! The renderer draws two layers per frame:
//! 1. **Background layer** — colored quads for cells with non-default
//!    backgrounds, plus the cursor.
//! 2. **Text layer** — glyphon renders shaped glyphs with per-cell foreground
//!    colors.
//!
//! Multi-pane rendering is supported: pass a slice of [`PaneView`] items, each
//! with a viewport rect and a grid reference. Each pane is clipped to its rect.

use glyphon::{
    Attrs, BufferLine, Cache, Color, ColorMode, Family, FontSystem, Shaping, SwashCache,
    SwashContent, TextArea, TextAtlas, TextBounds, TextRenderer, Viewport,
};
use wgpu::{
    BufferUsages, ColorTargetState, ColorWrites, Device, DeviceDescriptor, FragmentState,
    FrontFace, LoadOp, MultisampleState, PipelineLayoutDescriptor, PolygonMode, PrimitiveState,
    PrimitiveTopology, Queue, RenderPassColorAttachment, RenderPassDescriptor, RenderPipeline,
    RenderPipelineDescriptor, ShaderModuleDescriptor, ShaderSource, StoreOp, Surface,
    SurfaceConfiguration, TextureFormat, VertexAttribute, VertexBufferLayout, VertexFormat,
    VertexState, VertexStepMode,
};

use crate::glyph_quad::{GlyphQuadPass, GlyphQuadPlacement, GlyphTexture};
use crate::grid::{Cell, CellWidth, Color as GridColor, CursorShape, Grid, RgbColor};
use crate::image::{ImagePass, ImagePlacement};
use crate::tabbar::{
    self, layout as tabbar_layout, DropdownLayout, MenuItem, Region, TabbarHit, TabbarLayout,
    TopTabbar, HOVER_PILL_H_PAD_CELLS, NEW_TAB_BOTTOM_INSET_RATIO, TAB_H_PAD_CELLS, ZOOM_CELLS,
};
use crate::theme::{Rgb, Theme};
mod background;
mod chrome;
mod colors;
mod glyphs;
mod images;

use background::*;
use chrome::*;
pub use chrome::{PaletteItem, PaletteView, WhichKeyView};
use colors::*;
use glyphs::*;
pub use glyphs::{start_font_load, FontConfig, FontLoad};

// ========================================================================
// Data Structures
// ========================================================================

/// A viewport rect for one pane, in pixels from the surface top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PaneRect {
    /// Height in physical pixels.
    pub height: f32,
    /// Width in physical pixels.
    pub width: f32,
    /// Left edge, in physical pixels from the surface's left.
    pub x: f32,
    /// Top edge, in physical pixels from the surface's top.
    pub y: f32,
}

/// One pane's rendering input: where to draw and what grid to draw.
pub struct PaneView<'a> {
    /// Bracket glyphs to recolor by nesting depth, as viewport `(row, col)`
    /// plus the resolved RGB. Driven by the `rainbow-parens` config option;
    /// computed app-side (see `navigation::reading::bracket_marks`) so the
    /// renderer stays free of depth logic.
    pub bracket_colors: &'a [(usize, usize, (u8, u8, u8))],
    /// Shape to draw the cursor with, resolved from the pane's current mode
    /// and the `cursor` config block.
    pub cursor_shape: CursorShape,
    /// Draw the grid cursor in its unfocused form: a `Block` cursor becomes a
    /// hollow outline, while `Bar` and `Underline` keep their thin filled shape
    /// but fade toward the background. Set when the OS window has lost focus, so
    /// a glance at the cursor tells you whether keystrokes actually reach this
    /// pane.
    pub cursor_unfocused: bool,
    /// Whether the cursor should be drawn this frame. Set to `false` on the
    /// "off" phase of cursor blink; the nav cursor ignores this flag.
    pub cursor_visible: bool,
    /// Blend all colors toward the background to visually de-emphasize this pane.
    /// Controlled by the `dim-inactive` config option.
    pub dim: bool,
    /// Whether this pane is the focused (active) pane.
    pub focused: bool,
    /// The cell grid to draw, including its scrollback.
    pub grid: &'a Grid,
    /// Link ID currently under the mouse pointer. Cells sharing this link get
    /// a highlighted underline color. 0 means no link is hovered.
    pub hovered_link: u16,
    /// Quick-select block labels as `(row, col, label)`, or `None` when
    /// quick-select is not active.
    pub labels: Option<&'a [(usize, usize, char)]>,
    /// Easymotion-style `f`/`t` jump labels as `(row, col, label)`, drawn in the
    /// theme's [`Theme::find_label_bg`]/[`Theme::find_label_fg`] so they read
    /// differently from the quick-select block labels above.
    pub find_labels: &'a [(usize, usize, char)],
    /// The Normal-mode traversal cursor, in viewport `(row, col)`, drawn in
    /// [`Self::cursor_shape`]. `None` when the pane is not being navigated.
    pub nav_cursor: Option<(usize, usize)>,
    /// The row to band with [`Theme::cursor_line_bg`] (with its soft-wrapped
    /// continuations). Separate from [`Self::nav_cursor`] so an unfocused pane can
    /// keep showing where its cursor is even though the cursor block itself is
    /// only drawn for the focused pane.
    pub cursor_line_row: Option<usize>,
    /// Whether the nav cursor is in its visible blink phase. Tracked separately
    /// from [`Self::cursor_visible`], which the shell can suppress outright
    /// (DECTCEM) — the Normal-mode cursor is Winter's own and only blinks.
    pub nav_cursor_visible: bool,
    /// Where in the surface this pane draws.
    pub rect: PaneRect,
    /// Active search query matches as `(row, col)` pairs; empty when no search
    /// is active. Highlighted in the background pass with
    /// [`Theme::search_match_bg`].
    pub search_matches: &'a [(usize, usize)],
    /// Sentence-highlight bands as row-clipped column spans `(row, start, end,
    /// tone)`, alternating `tone` parity per sentence. Driven by the
    /// `sentence-highlight` config option; computed app-side (see
    /// `navigation::reading::sentence_spans`).
    pub sentence_spans: &'a [(usize, usize, usize, u8)],
    /// The cells of the one match `n`/`N` is parked on, a subset of
    /// [`Self::search_matches`], highlighted with [`Theme::search_current_bg`]
    /// instead so the focused match stands out from the rest.
    pub search_current: &'a [(usize, usize)],
    /// The selected region as `(start_row, start_col, end_row, end_col)` in
    /// viewport coordinates, or `None` when nothing is selected.
    pub selection: Option<(usize, usize, usize, usize)>,
    /// When `true`, selection is rectangular (blockwise `Ctrl-V`).
    pub selection_block: bool,
    /// How many rows the pane is currently scrolled up from the live bottom.
    pub scroll_offset: usize,
    /// Total rows in the scrollback buffer above the live grid.
    pub scrollback_len: usize,
    /// Underline cells carrying a link (auto-detected URL or OSC 8 hyperlink).
    /// Controlled by the `url-underline` config option.
    pub url_underline: bool,
}

/// A Vim-style bottom status bar: the `label` (e.g. " NORMAL ") is drawn over a
/// segment filled with `accent`, atop a full-width strip in the theme's status
/// colors. Occupies the bottom-most cell row of the surface.
pub struct StatusBar {
    /// Fill color of the mode segment, chosen per mode by the caller.
    pub accent: Rgb,
    /// The mode label drawn over `accent`, e.g. `" NORMAL "`.
    pub mode: String,
    /// The in-progress or last-run `/` search query, shown after the mode
    /// label while a search is active and cleared once it's cancelled.
    pub search: Option<StatusSearch>,
    /// A transient notice (an error or an info confirmation), shown after the
    /// mode label until it expires.
    pub notice: Option<StatusNotice>,
}

/// The live `/`/`?` search state shown in the status bar: the query text as
/// typed, plus the current match position out of the total found (both zero
/// while there are no matches, e.g. an empty query or no hits yet).
#[derive(Clone)]
pub struct StatusSearch {
    /// The query as typed, without its `/` or `?` prefix.
    pub query: String,
    /// 1-based position of the current match, or 0 when there are none.
    pub match_index: usize,
    /// How many matches the query found, 0 when there are none.
    pub match_total: usize,
    /// `true` for a `?`/`#` backward search, shown with a `?` prefix instead
    /// of `/`.
    pub reverse: bool,
}

/// A transient status-bar notice and how it should read: [`NoticeKind::Error`]
/// draws in red to stand out, [`NoticeKind::Info`] in green to confirm an
/// action such as copying to the clipboard. Also drawn as a floating toast
/// overlay (bottom-center) when the status bar is hidden.
#[derive(Clone)]
pub struct StatusNotice {
    /// Whether this reports a problem or confirms an action.
    pub kind: NoticeKind,
    /// The message shown to the user.
    pub text: String,
}

/// Whether a [`StatusNotice`] reports a problem or confirms an action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NoticeKind {
    /// A problem, drawn in red.
    Error,
    /// A confirmation, drawn in green.
    Info,
}

/// GPU text renderer: owns the wgpu device/queue, glyph atlas, and render
/// pipelines. Designed to be created once and reused across frames.
pub struct GpuRenderer {
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    font_system: FontSystem,
    swash_cache: SwashCache,
    #[allow(dead_code)]
    cache: Cache,
    text_atlas: TextAtlas,
    text_renderer: TextRenderer,
    viewport: Viewport,
    bg_pipeline: RenderPipeline,
    bg_buffer: wgpu::Buffer,
    dot_pipeline: RenderPipeline,
    dot_buffer: wgpu::Buffer,
    cell_width: f32,
    cell_height: f32,
    cols: usize,
    rows: usize,
    font_family: Option<String>,
    font_size: f32,
    line_height: f32,
    logical_font_size: f32,
    scale_factor: f64,
    normal_weight: Option<String>,
    bold_weight: Option<String>,
    /// Whether the active font actually contains Braille Patterns glyphs. When it
    /// does, braille is rendered from the font (crisp, anti-aliased, cell-aligned
    /// like btop in any other terminal); when it doesn't (e.g. the default
    /// DejaVu Sans Mono), braille is drawn procedurally as a dot grid instead.
    font_has_braille: bool,
    /// Whether requesting [`Weight::BOLD`](glyphon::cosmic_text::Weight::BOLD)
    /// for `font_family` actually resolves to a face in that same family. Some
    /// fonts (e.g. Cascadia Code as commonly installed on Windows) register
    /// their bold cut under a distinct family name rather than as a bold face
    /// of the regular family; asking `fontdb` for `family + weight(700)` then
    /// falls through to an unrelated system font (observed: Segoe UI, a
    /// proportional UI font), which renders bold spans in a visibly different
    /// size and shape from the rest of the line. When `false`, bold cells fall
    /// back to [`Weight::NORMAL`](glyphon::cosmic_text::Weight::NORMAL) so they
    /// stay in the correct monospace family, sacrificing visual weight rather
    /// than glyph metrics.
    font_has_bold: bool,
    /// When `false` (the default), text buffers use `Shaping::Basic` (no
    /// ligatures, each glyph at its nominal cell position); when `true`,
    /// `Shaping::Advanced` is used.
    ligatures: bool,
    /// Per-`(character, is_wide)` cell-width fit for glyphs that need
    /// [`Shaping::Advanced`] (fallback fonts). A proportional fallback font's
    /// glyph advance rarely lands exactly on a cell width (see
    /// [`needs_complex_shaping`]), so this caches the `(font_size, cell_width)`
    /// it was measured at, the uniform scale that fits its rasterized texture
    /// (drawn via [`GlyphQuadPass`], not reshaped through cosmic-text) to one
    /// cell width or two (`is_wide` — a double-width glyph like most emoji
    /// fits two, so the same character can need a different scale in each
    /// role, e.g. a regional indicator alone versus paired into a flag), and
    /// whether it's a genuine color glyph (COLR/CBDT color emoji) rather than
    /// a monochrome mask tinted by the cell's foreground color. Recomputed
    /// lazily when the cached font size or cell width goes stale.
    fallback_glyph_cache: std::collections::HashMap<(char, bool), FallbackGlyphFit>,
    /// Textured-quad pass for the glyphs `fallback_glyph_cache` corrects: each
    /// is rasterized once at the unmodified font size and drawn as its own
    /// scaled quad, so fitting its width to the cell never perturbs another
    /// glyph's line layout (see [`GpuRenderer::ensure_fallback_glyph_quad`]).
    glyph_quad_pass: GlyphQuadPass,
    theme: Theme,
    /// Persistent per-pane text buffers, reused across frames so cosmic-text
    /// only re-shapes lines whose content changed (one line per keystroke
    /// instead of the whole screen).
    text_buffers: Vec<glyphon::Buffer>,
    /// Persistent one-line buffer for the bottom status bar.
    status_buffer: Option<glyphon::Buffer>,
    /// Textured-quad pass for image blocks rendered natively (no webview).
    image_pass: ImagePass,
    /// Image pass for the rasterized top-tabbar strip. Rendered between the bg
    /// quads and the text so the rounded tab cards sit under the tab titles.
    tabbar_strip_pass: ImagePass,
    /// System fonts for SVG text, loaded lazily on first SVG with text and then
    /// reused (the scan costs ~150ms, so it is deferred off the startup path).
    svg_fontdb: Option<std::sync::Arc<resvg::usvg::fontdb::Database>>,
    /// Thickness of the 1-D line drawn between adjacent panes (in logical pixels).
    divider_width: f32,
    modern: bool,
    tabbar_enabled: bool,
    status_bar_enabled: bool,
    /// Whether the OS window manager draws its own frame around the window.
    /// When `false` (the undecorated "Modern" title bar style), the renderer
    /// draws its own 1px [`Theme::window_border`] outline instead.
    decorated: bool,
}

// ========================================================================
// GpuRenderer
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
    fn acquire_surface_texture(&mut self) -> Option<wgpu::SurfaceTexture> {
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

    /// Render multiple panes to the surface. Each `PaneView` specifies a grid
    /// and its viewport rect. Pane dividers are drawn between adjacent panes.
    /// When `status` is set, a status bar is drawn across the bottom cell row;
    /// callers must leave that row free of panes (see [`Self::cell_size`]).
    /// When `tabbar` is set, the tabbar/menubar is drawn across the top cell
    /// row(s) (likewise reserved by the caller) and any open dropdown is
    /// composited over the content via the image pass.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        panes: &[PaneView],
        status: Option<&StatusBar>,
        tabbar: Option<&TopTabbar>,
        images: &[ImagePlacement],
        palette: Option<&PaletteView>,
        toast: Option<&StatusNotice>,
        which_key: Option<&WhichKeyView>,
    ) {
        self.tabbar_enabled = tabbar.is_some();
        self.status_bar_enabled = status.is_some();
        if let Some(t) = tabbar {
            self.modern = t.menu_style == crate::tabbar::MenuStyle::Modern;
        }
        let surface_texture = match self.acquire_surface_texture() {
            Some(texture) => texture,
            None => return,
        };

        let view = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let surface_w = self.config.width as f32;
        let surface_h = self.config.height as f32;

        let resolution = glyphon::Resolution {
            width: self.config.width,
            height: self.config.height,
        };
        self.viewport.update(&self.queue, resolution);

        let mut all_bg_verts = Vec::new();
        let mut all_dot_verts: Vec<DotVertex> = Vec::new();
        let mut all_glyph_quads: Vec<GlyphQuadPlacement> = Vec::new();
        // Reuse last frame's buffers so unchanged lines keep their cached shaping.
        let mut text_buffers = std::mem::take(&mut self.text_buffers);
        text_buffers.truncate(panes.len());

        // Owned (not borrowed from `self`) because building each row's attrs
        // interleaves with `glyph_correction_font_size`, which needs `&mut self`.
        let fam = self.font_family.clone();

        // When ligatures are disabled, suppress the OpenType features that build
        // them. Pure-ASCII lines already avoid ligatures via Basic shaping, but
        // any line with non-ASCII must use Advanced shaping (for font fallback),
        // which would otherwise re-enable ligatures; these features turn them off
        // there too. `None` when ligatures are on, so they shape normally.
        let lig_off_features = (!self.ligatures).then(|| {
            let mut ff = glyphon::cosmic_text::FontFeatures::new();
            ff.disable(glyphon::cosmic_text::FeatureTag::CONTEXTUAL_ALTERNATES);
            ff.disable(glyphon::cosmic_text::FeatureTag::STANDARD_LIGATURES);
            ff.disable(glyphon::cosmic_text::FeatureTag::CONTEXTUAL_LIGATURES);
            ff.disable(glyphon::cosmic_text::FeatureTag::DISCRETIONARY_LIGATURES);
            ff
        });

        for (pane_idx, pane) in panes.iter().enumerate() {
            let grid = pane.grid;
            let rect = pane.rect;
            let pane_cols =
                ((rect.width - crate::PANE_H_PAD * 2.0) / self.cell_width).floor() as usize;
            let pane_rows = (rect.height / self.cell_height).floor() as usize;

            let bg_verts = build_bg_vertices_offset(
                grid,
                BgParams {
                    ch: self.cell_height,
                    content_right: rect.x + rect.width - crate::PANE_H_PAD,
                    cursor_unfocused: pane.cursor_unfocused,
                    cursor_shape: pane.cursor_shape,
                    cw: self.cell_width,
                    dim: pane.dim,
                    draw_braille_dots: !self.font_has_braille,
                    focused: pane.focused,
                    hide_cursor: !pane.cursor_visible || pane.nav_cursor.is_some(),
                    hovered_link: pane.hovered_link,
                    labels: pane.labels,
                    find_labels: pane.find_labels,
                    offset_x: rect.x + crate::PANE_H_PAD,
                    offset_y: rect.y,
                    // The cursor's row gets a `cursorline` wash while the pane is
                    // being navigated; it doesn't blink with the cursor itself.
                    cursor_line: pane.cursor_line_row.map(|row| grid.wrapped_row_span(row)),
                    scroll_offset: pane.scroll_offset,
                    scrollback_len: pane.scrollback_len,
                    search_current: pane.search_current,
                    search_matches: pane.search_matches,
                    sentence_spans: pane.sentence_spans,
                    selection: pane.selection,
                    selection_block: pane.selection_block,
                    surface_h,
                    surface_w,
                    theme: &self.theme,
                    url_underline: pane.url_underline,
                },
                &mut all_dot_verts,
            );
            all_bg_verts.extend_from_slice(&bg_verts);

            if let Some((nav_row, nav_col)) = pane.nav_cursor.filter(|_| pane.nav_cursor_visible) {
                if nav_row < pane_rows && nav_col < pane_cols {
                    let px0 = rect.x + crate::PANE_H_PAD + nav_col as f32 * self.cell_width;
                    let py0 = rect.y + nav_row as f32 * self.cell_height;
                    // The traversal cursor takes the same unfocused form as the
                    // shell's (see `build_bg_vertices_offset`): block-shaped, a
                    // hollow outline; bar- or underline-shaped, a faded fill.
                    if pane.cursor_unfocused && pane.cursor_shape == CursorShape::Block {
                        for (qx0, qy0, qx1, qy1) in cursor_outline_quads(
                            px0,
                            py0,
                            self.cell_width,
                            self.cell_height,
                            CURSOR_HOLLOW_STROKE_WIDTH,
                        ) {
                            all_bg_verts.extend_from_slice(&quad_vertices(
                                qx0,
                                qy0,
                                qx1,
                                qy1,
                                self.theme.cursor_bg.as_linear(),
                                surface_w,
                                surface_h,
                            ));
                        }
                    } else {
                        let (qx0, qy0, qx1, qy1) = cursor_quad(
                            pane.cursor_shape,
                            px0,
                            py0,
                            self.cell_width,
                            self.cell_height,
                            false,
                        );
                        let color = if pane.cursor_unfocused {
                            lerp_to_bg(
                                self.theme.cursor_bg.as_linear(),
                                self.theme.background.as_linear(),
                            )
                        } else {
                            self.theme.cursor_bg.as_linear()
                        };
                        all_bg_verts.extend_from_slice(&quad_vertices(
                            qx0, qy0, qx1, qy1, color, surface_w, surface_h,
                        ));
                    }
                }
            }

            let mut default_attrs =
                Attrs::new()
                    .family(base_family(fam.as_deref()))
                    .weight(parse_weight(
                        self.normal_weight.as_deref(),
                        glyphon::cosmic_text::Weight::NORMAL,
                    ));
            if let Some(ff) = &lig_off_features {
                default_attrs = default_attrs.font_features(ff.clone());
            }
            let mut rows_data: Vec<(String, glyphon::AttrsList)> = Vec::with_capacity(pane_rows);

            let theme_bg = self.theme.background;
            let pane_dim = pane.dim;
            let dim_text = |r: u8, g: u8, b: u8| -> Color {
                if pane_dim {
                    let lerp = |v: u8, bv: u8| -> u8 {
                        (v as f32 + (bv as f32 - v as f32) * DIM_FACTOR).round() as u8
                    };
                    Color::rgba(
                        lerp(r, theme_bg.r),
                        lerp(g, theme_bg.g),
                        lerp(b, theme_bg.b),
                        255,
                    )
                } else {
                    Color::rgba(r, g, b, 255)
                }
            };

            let sel = pane.selection;
            let sel_norm = sel.map(|(r1, c1, r2, c2)| {
                if (r1, c1) > (r2, c2) {
                    (r2, c2, r1, c1)
                } else {
                    (r1, c1, r2, c2)
                }
            });

            let quick_label_color = Color::rgba(255, 200, 50, 255);
            let find_label_color = self.theme.find_label_fg.to_glyphon();
            // Both overlays replace the cell's glyph with their label; the quick
            // select labels are amber, the `f`/`t` jump labels take the theme's
            // find-label color over their light box.
            let label_map: std::collections::HashMap<(usize, usize), (char, Color)> = pane
                .labels
                .map(|l| {
                    l.iter()
                        .map(|&(r, c, ch)| ((r, c), (ch, quick_label_color)))
                        .collect()
                })
                .unwrap_or_default();
            let bracket_map: std::collections::HashMap<(usize, usize), (u8, u8, u8)> = pane
                .bracket_colors
                .iter()
                .map(|&(row, col, rgb)| ((row, col), rgb))
                .collect();
            let label_map: std::collections::HashMap<(usize, usize), (char, Color)> = label_map
                .into_iter()
                .chain(
                    pane.find_labels
                        .iter()
                        .map(|&(r, c, ch)| ((r, c), (ch, find_label_color))),
                )
                .collect();

            // The viewport cell covered by an opaque block cursor, so the glyph
            // under it can be repainted when it would otherwise disappear into
            // the cursor's fill.
            let block_cursor = block_cursor_cell(
                pane.cursor_shape,
                pane.cursor_unfocused,
                pane.nav_cursor.filter(|_| pane.nav_cursor_visible),
                pane.cursor_visible,
                grid.cursor(),
                pane.scroll_offset,
            );

            for row in 0..grid.rows().min(pane_rows) {
                let mut text = String::with_capacity(grid.cols());
                let mut attrs_list = glyphon::AttrsList::new(&default_attrs);
                // `sel_norm` addresses rows absolutely (see
                // `Grid::to_absolute_row`), so this row's membership is tested
                // against its absolute index rather than the viewport row.
                let abs_row = grid.to_absolute_row(row);

                for col in 0..grid.cols().min(pane_cols) {
                    let start = text.len();
                    let cell = grid.visible_cell(row, col);

                    // The right half of a double-width char carries no glyph; skip
                    // it so the wide glyph from the left half spans both columns.
                    if matches!(cell.map(|c| c.width), Some(CellWidth::Spacer)) {
                        continue;
                    }

                    let label = label_map.get(&(row, col)).copied();
                    let label_char = label.map(|(ch, _)| ch);
                    // Set when this cell is under the block cursor and its own
                    // color would vanish into the cursor fill.
                    let cursor_text_fg: Option<(u8, u8, u8)> =
                        block_cursor.filter(|&pos| pos == (row, col)).and_then(|_| {
                            cursor_contrast_fg(cell_text_fg(cell, &self.theme), &self.theme)
                        });
                    let raw = label_char.unwrap_or_else(|| cell.map(|c| c.ch).unwrap_or(' '));
                    // When the font lacks braille, it is painted as quads in the
                    // background pass instead; emit a space here so the glyph
                    // layer doesn't draw a misaligned, proportionally-shaped
                    // fallback on top of it. When the font has braille, keep it
                    // so the font renders it directly (crisp and anti-aliased).
                    let ch = if label_char.is_none() && !self.font_has_braille && is_braille(raw) {
                        ' '
                    } else {
                        raw
                    };

                    // Trailing combining codepoints (ZWJ sequences, variation
                    // selectors, skin-tone modifiers, a paired flag half); labels
                    // never carry a tail (they're a single synthesized char).
                    let tail = if label_char.is_none() {
                        cell.and_then(|c| c.tail.as_deref())
                    } else {
                        None
                    };

                    // Fallback-font glyphs (Dingbats, symbols, ...) rarely advance by
                    // exactly one cell (e.g. Claude Code's rotating star spinner).
                    // Draw them as their own independently-scaled quad instead of
                    // asking cosmic-text to reshape them to fit, which would
                    // otherwise perturb this row's shared ascent/descent. Emit a
                    // space here so the glyph layer leaves this cell's ink to the
                    // quad pass below.
                    let is_wide = matches!(cell.map(|c| c.width), Some(CellWidth::Wide));
                    let quad_scale = self.ensure_fallback_glyph_quad(ch, tail, is_wide);
                    if quad_scale.is_some() {
                        text.push(' ');
                    } else {
                        text.push(ch);
                        // `tail` rides along in the same shaping run as `ch` so
                        // cosmic-text's GSUB rules can compose them into one glyph.
                        if let Some(tail) = tail {
                            text.push_str(tail);
                        }
                    }

                    if let Some(scale) = quad_scale {
                        // Selection highlights only the background (see the
                        // span below); the glyph keeps its own foreground
                        // whether or not it falls inside the selection.
                        let color = if let Some((_, label_color)) = label {
                            label_color
                        } else if let Some((r, g, b)) = cursor_text_fg {
                            dim_text(r, g, b)
                        } else {
                            match cell.map(|c| c.style.foreground) {
                                Some(GridColor::Rgb(rgb)) => dim_text(rgb.r, rgb.g, rgb.b),
                                Some(GridColor::Indexed(idx)) => {
                                    let (r, g, b) = theme_indexed_color(&self.theme, idx);
                                    dim_text(r, g, b)
                                }
                                _ => {
                                    let fg = self.theme.foreground;
                                    dim_text(fg.r, fg.g, fg.b)
                                }
                            }
                        };
                        let key = glyph_key(ch, tail);
                        if let Some((w, h)) = self.glyph_quad_pass.dims(&key) {
                            let quad_w = w as f32 * scale;
                            let quad_h = h as f32 * scale;
                            // A wide glyph's quad centers across the two columns it
                            // occupies (this cell plus its Spacer), not just the first.
                            let target_width = if is_wide {
                                self.cell_width * 2.0
                            } else {
                                self.cell_width
                            };
                            let cell_x = rect.x + crate::PANE_H_PAD + col as f32 * self.cell_width;
                            let cell_y = rect.y + row as f32 * self.cell_height;
                            all_glyph_quads.push(GlyphQuadPlacement {
                                color: (
                                    color.r() as f32 / 255.0,
                                    color.g() as f32 / 255.0,
                                    color.b() as f32 / 255.0,
                                ),
                                height: quad_h,
                                key,
                                width: quad_w,
                                x: cell_x + (target_width - quad_w) / 2.0,
                                y: cell_y + (self.cell_height - quad_h) / 2.0,
                            });
                        }
                    } else if let Some((_, label_color)) = label {
                        let label_attrs = Attrs::new()
                            .family(base_family(fam.as_deref()))
                            .weight(effective_bold_weight(
                                self.bold_weight.as_deref(),
                                self.font_has_bold,
                            ))
                            .color(label_color);
                        attrs_list.add_span(start..text.len(), &label_attrs);
                    } else if sel_norm.is_some_and(|(sr1, sc1, sr2, sc2)| {
                        (abs_row, col) >= (sr1, sc1) && (abs_row, col) <= (sr2, sc2)
                    }) {
                        // The selection highlights the background only; the
                        // glyph keeps the cell's own foreground (reverse video
                        // still swaps, as it would unselected) so selected
                        // text reads the same as the rest of the line.
                        let sel_bg = self.theme.selection_bg;
                        let text_fg =
                            cursor_text_fg.unwrap_or_else(|| cell_text_fg(cell, &self.theme));
                        let mut span_attrs = Attrs::new()
                            .family(base_family(fam.as_deref()))
                            .color(dim_text(text_fg.0, text_fg.1, text_fg.2));
                        let cell_bold = cell.is_some_and(|c| c.style.bold);
                        if cell_bold
                            || needs_dark_on_light_bold(text_fg, (sel_bg.r, sel_bg.g, sel_bg.b))
                        {
                            span_attrs = span_attrs.weight(effective_bold_weight(
                                self.bold_weight.as_deref(),
                                self.font_has_bold,
                            ));
                        }
                        if cell.is_some_and(|c| c.style.italic) {
                            span_attrs = span_attrs.style(glyphon::cosmic_text::Style::Italic);
                        }
                        // Disabling ligature features would also block the GSUB
                        // substitutions some fonts use to compose emoji/ZWJ
                        // sequences into a single glyph, so only strip them for
                        // characters that don't need complex shaping.
                        if !needs_complex_shaping(ch) {
                            if let Some(ff) = &lig_off_features {
                                span_attrs = span_attrs.font_features(ff.clone());
                            }
                        }
                        attrs_list.add_span(start..text.len(), &span_attrs);
                    } else if let Some(cell) = cell {
                        let mut attrs = Attrs::new().family(base_family(fam.as_deref()));

                        let fg_rgb = match cell.style.foreground {
                            GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
                            GridColor::Indexed(idx) => theme_indexed_color(&self.theme, idx),
                            GridColor::Default => {
                                let fg = self.theme.foreground;
                                (fg.r, fg.g, fg.b)
                            }
                        };
                        let bg_rgb = match cell.style.background {
                            GridColor::Rgb(rgb) => (rgb.r, rgb.g, rgb.b),
                            GridColor::Indexed(idx) => theme_indexed_color(&self.theme, idx),
                            GridColor::Default => {
                                let bg = self.theme.background;
                                (bg.r, bg.g, bg.b)
                            }
                        };
                        // SGR 7 (reverse video): the glyph paints in what would
                        // otherwise be the background, matching the swapped
                        // highlight quad drawn in the background pass.
                        let text_fg = if cell.style.reversed { bg_rgb } else { fg_rgb };
                        // A glyph that would be lost inside the block cursor's
                        // fill is repainted in a contrasting color instead.
                        let text_fg = cursor_text_fg.unwrap_or(text_fg);
                        // An explicit cell background (an SGR color, not the pane's
                        // base background) or a reversed cell counts as a highlight,
                        // so it gets the same synthetic-bold compensation as a
                        // selection.
                        let is_highlighted = cell.style.reversed
                            || !matches!(cell.style.background, GridColor::Default);
                        let highlight_bg = if cell.style.reversed { fg_rgb } else { bg_rgb };
                        let bg_needs_bold =
                            is_highlighted && needs_dark_on_light_bold(text_fg, highlight_bg);

                        if cell.style.bold || bg_needs_bold {
                            attrs = attrs.weight(effective_bold_weight(
                                self.bold_weight.as_deref(),
                                self.font_has_bold,
                            ));
                        } else {
                            attrs = attrs.weight(parse_weight(
                                self.normal_weight.as_deref(),
                                glyphon::cosmic_text::Weight::NORMAL,
                            ));
                        }
                        if cell.style.italic {
                            attrs = attrs.style(glyphon::cosmic_text::Style::Italic);
                        }

                        let text_color_explicit = cell.style.foreground != GridColor::Default
                            || cell.style.reversed
                            || cursor_text_fg.is_some()
                            || bracket_map.contains_key(&(row, col));
                        if let Some((r, g, b)) = bracket_map.get(&(row, col)) {
                            // Rainbow parens recolor the bracket glyph but keep
                            // the cell's own bold/italic — and yield to the
                            // cursor-contrast fix above, which exists to keep
                            // the glyph visible at all.
                            let fg = cursor_text_fg.unwrap_or((*r, *g, *b));
                            attrs = attrs.color(dim_text(fg.0, fg.1, fg.2));
                        } else if text_color_explicit {
                            attrs = attrs.color(dim_text(text_fg.0, text_fg.1, text_fg.2));
                        }

                        // Same as the selection span above: complex-shaping
                        // characters (emoji, CJK, accents) keep the font's
                        // default GSUB features so composed glyphs still form
                        // when ligatures are otherwise disabled. This needs its
                        // own span even in the common default-color/no-bold/
                        // no-italic case, since that case otherwise inherits
                        // `default_attrs`, which does carry the disabling
                        // features.
                        let is_complex = needs_complex_shaping(ch);
                        if !is_complex {
                            if let Some(ff) = &lig_off_features {
                                attrs = attrs.font_features(ff.clone());
                            }
                        }

                        if text_color_explicit
                            || cell.style.bold
                            || cell.style.italic
                            || bg_needs_bold
                            || (is_complex && lig_off_features.is_some())
                        {
                            attrs_list.add_span(start..text.len(), &attrs);
                        }
                    }
                }

                rows_data.push((text, attrs_list));
            }

            if pane_idx >= text_buffers.len() {
                text_buffers.push(glyphon::Buffer::new(
                    &mut self.font_system,
                    glyphon::Metrics::new(self.font_size, self.line_height),
                ));
            }
            let buffer = &mut text_buffers[pane_idx];
            // Do NOT call `buffer.set_monospace_width` here; it is not a no-op.
            // At fractional physical font sizes (fractional DPI scale: logical
            // 15px at 125% is 18.75px) cosmic-text 0.18.2 quantizes the requested
            // advance to a coarser grid (measured: a Cascadia Code "M" asked for
            // 10.986px but then rendered at 11.133px), so glyph advances stop
            // equaling `self.cell_width`. The cursor and cell backgrounds are
            // drawn at `col * cell_width`, so that per-glyph mismatch accumulates
            // one column at a time until the cursor sits on top of already-typed
            // text, worse the further right you go. With the call gone,
            // primary-font glyphs render at their natural advance, which is
            // exactly what `measure_cell` sets `cell_width` to, so the glyph run
            // and the cursor stride stay locked at every size. Wide/fallback
            // glyphs never relied on this call: `ensure_fallback_glyph_quad`
            // pulls any glyph whose advance diverges from the cell out of this
            // buffer into the manually positioned quad pass, itself drawn at
            // `col * cell_width`.
            let ending = glyphon::cosmic_text::LineEnding::default();
            let row_count = rows_data.len();
            for (i, (text, attrs_list)) in rows_data.into_iter().enumerate() {
                // Basic shaping keeps glyphs on the primary monospace font at
                // native cell-width advances, which is what the grid relies on
                // for column alignment. Only escalate to Advanced shaping (font
                // fallback + complex shaping) for characters that actually need
                // it (emoji, CJK, accents): forcing the box-drawing and braille
                // ranges through Advanced picks fallback glyphs with non-cell
                // advances, which drifts and breaks TUIs like btop.
                let advanced = self.ligatures || text.chars().any(needs_complex_shaping);
                let shaping = if advanced {
                    Shaping::Advanced
                } else {
                    Shaping::Basic
                };
                if i < buffer.lines.len() {
                    // A line's shaping follows from its text under the (stable)
                    // ligatures setting, so infer the existing line's mode from
                    // its current text: keep the glyph cache with set_text when
                    // the mode is unchanged, else reset to the new shaping.
                    let cur_advanced =
                        self.ligatures || buffer.lines[i].text().chars().any(needs_complex_shaping);
                    if advanced == cur_advanced {
                        buffer.lines[i].set_text(&text, ending, attrs_list);
                    } else {
                        buffer.lines[i].reset_new(text, ending, attrs_list, shaping);
                    }
                } else {
                    buffer
                        .lines
                        .push(BufferLine::new(&text, ending, attrs_list, shaping));
                }
            }
            buffer.lines.truncate(row_count);
            buffer.shape_until_scroll(&mut self.font_system, false);
        }

        if panes.len() > 1 {
            for i in 0..panes.len() {
                for j in (i + 1)..panes.len() {
                    let a = panes[i].rect;
                    let b = panes[j].rect;
                    let divider = compute_divider(
                        a,
                        b,
                        surface_w,
                        surface_h,
                        self.theme.divider.as_linear(),
                        self.divider_width,
                    );
                    if let Some(dv) = divider {
                        all_bg_verts.extend_from_slice(&dv);
                    }
                }
            }
        }

        let mut status_buffer = self.status_buffer.take().unwrap_or_else(|| {
            glyphon::Buffer::new(
                &mut self.font_system,
                glyphon::Metrics::new(self.font_size, self.line_height),
            )
        });
        // Exactly one cell row, flush with the window's bottom edge.
        let status_top = crate::status_bar_top_px(surface_h, self.cell_height);

        if let Some(status) = status {
            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top,
                surface_w,
                status_top + 1.0,
                self.theme.status_bar_border.as_linear(),
                surface_w,
                surface_h,
            ));

            all_bg_verts.extend_from_slice(&quad_vertices(
                0.0,
                status_top + 1.0,
                surface_w,
                status_top + crate::STATUS_BAR_HEIGHT * self.cell_height,
                self.theme.background.as_linear(),
                surface_w,
                surface_h,
            ));

            // Build status bar text segments dynamically
            let mut status_text = String::new();
            let mut spans = Vec::new();

            // Font attributes
            let accent_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(status.accent.to_glyphon());
            let muted_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(parse_weight(
                    self.normal_weight.as_deref(),
                    glyphon::cosmic_text::Weight::NORMAL,
                ))
                .color(self.theme.ansi[8].to_glyphon());
            let error_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(self.theme.ansi[1].to_glyphon());
            let info_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .weight(effective_bold_weight(
                    self.bold_weight.as_deref(),
                    self.font_has_bold,
                ))
                .color(self.theme.ansi[4].to_glyphon());

            // Mode label (e.g. Normal, Insert, Block)
            let mode_start = status_text.len();
            status_text.push_str(&status.mode);
            let mode_end = status_text.len();
            spans.push((mode_start..mode_end, accent_attrs));

            // The live `/` search query follows the mode label while a search
            // is active, showing the query text and its match position; hidden
            // once the search is cancelled (`status.search` is `None`).
            if let Some(ref search) = status.search {
                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));

                let query_start = status_text.len();
                status_text.push(if search.reverse { '?' } else { '/' });
                status_text.push_str(&search.query);
                let query_end = status_text.len();
                spans.push((query_start..query_end, info_attrs.clone()));

                if search.match_total > 0 {
                    let count_start = status_text.len();
                    status_text
                        .push_str(&format!("  {}/{}", search.match_index, search.match_total));
                    let count_end = status_text.len();
                    spans.push((count_start..count_end, muted_attrs.clone()));
                } else if !search.query.is_empty() {
                    let none_start = status_text.len();
                    status_text.push_str("  no matches");
                    let none_end = status_text.len();
                    spans.push((none_start..none_end, muted_attrs.clone()));
                }
            }

            // A transient notice follows the mode label: red for errors, green
            // for info confirmations (e.g. "Copied to clipboard").
            if let Some(ref notice) = status.notice {
                let sep_start = status_text.len();
                status_text.push_str("  •  ");
                let sep_end = status_text.len();
                spans.push((sep_start..sep_end, muted_attrs.clone()));

                let notice_attrs = match notice.kind {
                    NoticeKind::Error => error_attrs,
                    NoticeKind::Info => info_attrs,
                };
                let notice_start = status_text.len();
                status_text.push_str(&notice.text);
                let notice_end = status_text.len();
                spans.push((notice_start..notice_end, notice_attrs));
            }

            // Apply attributes to the text buffer line
            let default_attrs = Attrs::new()
                .family(base_family(fam.as_deref()))
                .color(self.theme.ansi[8].to_glyphon());
            let mut attrs_list = glyphon::AttrsList::new(&default_attrs);
            for (range, attrs) in spans {
                attrs_list.add_span(range, &attrs);
            }

            let ending = glyphon::cosmic_text::LineEnding::default();
            if status_buffer.lines.is_empty() {
                status_buffer.lines.push(BufferLine::new(
                    &status_text,
                    ending,
                    attrs_list,
                    Shaping::Advanced,
                ));
            } else {
                status_buffer.lines[0].set_text(&status_text, ending, attrs_list);
            }
            status_buffer.lines.truncate(1);
            status_buffer.shape_until_scroll(&mut self.font_system, false);
        }

        // Top tabbar (tabbar/menubar) bands and text. The dropdown overlay is
        // handled separately via the image pass so it sits above pane text.
        let tabbar_texts = match tabbar {
            Some(c) => self.draw_tabbar(c, surface_w),
            None => Vec::new(),
        };

        let bg_count = all_bg_verts.len() as u32;
        let bg_bytes: Vec<u8> = all_bg_verts.iter().flat_map(|v| v.to_bytes()).collect();
        self.queue.write_buffer(&self.bg_buffer, 0, &bg_bytes);

        // An undecorated window has no OS-drawn frame, so the renderer draws its
        // own 1px outline around the outer edge. Kept in a separate vertex range
        // appended after `bg_bytes` and drawn last (see below), so it sits on top
        // of the tabbar strip, glyphs, and image blocks instead of being painted
        // over by them.
        let mut border_verts = Vec::new();
        if !self.decorated {
            let border = self.theme.window_border.as_linear();
            border_verts.extend_from_slice(&quad_vertices(
                0.0, 0.0, surface_w, 1.0, border, surface_w, surface_h,
            ));
            border_verts.extend_from_slice(&quad_vertices(
                0.0,
                surface_h - 1.0,
                surface_w,
                surface_h,
                border,
                surface_w,
                surface_h,
            ));
            border_verts.extend_from_slice(&quad_vertices(
                0.0, 0.0, 1.0, surface_h, border, surface_w, surface_h,
            ));
            border_verts.extend_from_slice(&quad_vertices(
                surface_w - 1.0,
                0.0,
                surface_w,
                surface_h,
                border,
                surface_w,
                surface_h,
            ));
        }
        let border_count = border_verts.len() as u32;
        let border_offset = bg_bytes.len() as wgpu::BufferAddress;
        if border_count > 0 {
            let border_bytes: Vec<u8> = border_verts.iter().flat_map(|v| v.to_bytes()).collect();
            self.queue
                .write_buffer(&self.bg_buffer, border_offset, &border_bytes);
        }

        // Cap braille dots to the buffer capacity (whole quads of 6 vertices) so
        // an unusually dense frame can't overrun it; dropped dots just don't draw.
        let max_dot_verts = (DOT_BUFFER_SIZE as usize / std::mem::size_of::<DotVertex>()) / 6 * 6;
        if all_dot_verts.len() > max_dot_verts {
            all_dot_verts.truncate(max_dot_verts);
        }
        let dot_count = all_dot_verts.len() as u32;
        let dot_bytes: Vec<u8> = all_dot_verts.iter().flat_map(|v| v.to_bytes()).collect();
        if dot_count > 0 {
            self.queue.write_buffer(&self.dot_buffer, 0, &dot_bytes);
        }

        // The top-tabbar strip (band + rounded tab pills) is composited before the
        // text pass so the tab cards sit under the tab titles.
        let tabbar_strip: Vec<ImagePlacement> = tabbar
            .and_then(|c| self.rasterize_tabbar_strip(c, surface_w))
            .into_iter()
            .collect();
        self.tabbar_strip_pass
            .prepare(&self.queue, &tabbar_strip, surface_w, surface_h);

        // The open dropdown and command palette are rasterized to textures and
        // drawn by the image pass (after the text pass) so they overlay content.
        let mut all_images: Vec<ImagePlacement> = images.to_vec();
        if let Some(c) = tabbar {
            all_images.extend(self.rasterize_dropdown(c, surface_w));
            if let Some(placement) = self.rasterize_url_tooltip(c, surface_w, surface_h) {
                all_images.push(placement);
            }
        }
        if let Some(p) = palette {
            all_images.extend(self.rasterize_palette(p, surface_w, surface_h));
        }
        if let Some(t) = toast {
            if let Some(placement) = self.rasterize_toast(t, surface_w) {
                all_images.push(placement);
            }
        }
        if let Some(wk) = which_key {
            all_images.extend(self.rasterize_which_key(wk, surface_w, surface_h));
        }
        self.image_pass
            .prepare(&self.queue, &all_images, surface_w, surface_h);

        self.glyph_quad_pass
            .prepare(&self.queue, &all_glyph_quads, surface_w, surface_h);

        let mut text_areas: Vec<TextArea> = text_buffers
            .iter()
            .zip(panes.iter())
            .map(|(buffer, pane)| {
                let fg = self.theme.foreground;
                let bg = self.theme.background;
                let default_color = if pane.dim {
                    let lerp = |v: u8, bv: u8| -> u8 {
                        (v as f32 + (bv as f32 - v as f32) * DIM_FACTOR).round() as u8
                    };
                    Color::rgba(lerp(fg.r, bg.r), lerp(fg.g, bg.g), lerp(fg.b, bg.b), 255)
                } else {
                    fg.to_glyphon()
                };
                TextArea {
                    buffer,
                    left: (pane.rect.x + crate::PANE_H_PAD).round(),
                    top: pane.rect.y.round(),
                    bounds: TextBounds {
                        left: (pane.rect.x + crate::PANE_H_PAD).round() as i32,
                        top: pane.rect.y.round() as i32,
                        right: (pane.rect.x + pane.rect.width - crate::PANE_H_PAD).round() as i32,
                        bottom: (pane.rect.y + pane.rect.height).round() as i32,
                    },
                    default_color,
                    scale: 1.0,
                    custom_glyphs: &[],
                }
            })
            .collect();

        if status.is_some() {
            text_areas.push(TextArea {
                buffer: &status_buffer,
                left: 0.0,
                top: (status_top
                    + 1.0
                    + (crate::STATUS_BAR_HEIGHT * self.cell_height - 1.0 - self.line_height) / 2.0)
                    .round(),
                bounds: TextBounds {
                    left: 0,
                    top: status_top.round() as i32,
                    right: surface_w as i32,
                    bottom: (status_top + crate::STATUS_BAR_HEIGHT * self.cell_height).round()
                        as i32,
                },
                default_color: self.theme.status_bar_fg.to_glyphon(),
                scale: 1.0,
                custom_glyphs: &[],
            });
        }

        for text in &tabbar_texts {
            text_areas.push(TextArea {
                buffer: &text.buffer,
                left: text.left.round(),
                top: text.top.round(),
                bounds: TextBounds {
                    left: text.bounds.left,
                    top: text.bounds.top,
                    right: text.bounds.right,
                    bottom: text.bounds.bottom,
                },
                default_color: text.color,
                scale: 1.0,
                custom_glyphs: &[],
            });
        }

        let prepared = self
            .text_renderer
            .prepare(
                &self.device,
                &self.queue,
                &mut self.font_system,
                &mut self.text_atlas,
                &self.viewport,
                text_areas,
                &mut self.swash_cache,
            )
            .is_ok();

        // Hand the buffers back for reuse next frame (preserves shape caches)
        // regardless of outcome, so a failed prepare doesn't also cost the
        // shape cache on top of the dropped frame.
        self.text_buffers = text_buffers;
        self.status_buffer = Some(status_buffer);

        // glyphon returns `AtlasFull` when ordinary heavy content (large
        // CJK/emoji dumps, a big font size, many panes) outgrows the glyph
        // atlas. That's reachable from real PTY content, not just a bug, so
        // skip this frame the same way `acquire_surface_texture` already
        // degrades on a transient surface error instead of panicking.
        if !prepared {
            return;
        }

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("winter frame"),
            });

        {
            let mut pass = encoder.begin_render_pass(&RenderPassDescriptor {
                label: Some("winter clear + bg"),
                color_attachments: &[Some(RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        // The sRGB surface expects a linear clear value, so decode
                        // the (sRGB) background to keep the displayed color exact.
                        load: LoadOp::Clear(wgpu::Color {
                            r: srgb_to_linear_f64(self.theme.background.r),
                            g: srgb_to_linear_f64(self.theme.background.g),
                            b: srgb_to_linear_f64(self.theme.background.b),
                            a: 1.0,
                        }),
                        store: StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });

            if bg_count > 0 {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(..));
                pass.draw(0..bg_count, 0..1);
            }

            // Anti-aliased braille dots blend over the cell backgrounds.
            if dot_count > 0 {
                pass.set_pipeline(&self.dot_pipeline);
                pass.set_vertex_buffer(0, self.dot_buffer.slice(..));
                pass.draw(0..dot_count, 0..1);
            }

            // Tabbar strip sits above the bg quads but below the text so the
            // rounded tab cards back the tab titles.
            self.tabbar_strip_pass.render(&mut pass);

            // Reachable only if `prepare` and this call disagree about the
            // atlas/viewport passed in, not from ordinary content; skip the
            // frame the same way a failed `prepare` above does rather than
            // panicking on it.
            if self
                .text_renderer
                .render(&self.text_atlas, &self.viewport, &mut pass)
                .is_err()
            {
                return;
            }

            // Fallback glyphs that needed a cell-width fit (e.g. Claude Code's
            // spinner): drawn as their own quads, not through cosmic-text, so
            // their correction never touched this row's ascent/descent.
            self.glyph_quad_pass.render(&mut pass);

            self.image_pass.render(&mut pass);

            // Drawn last so the window border sits on top of the tabbar strip,
            // glyphs, and image blocks instead of being painted over by them.
            if border_count > 0 {
                pass.set_pipeline(&self.bg_pipeline);
                pass.set_vertex_buffer(0, self.bg_buffer.slice(border_offset..));
                pass.draw(0..border_count, 0..1);
            }
        }

        self.queue.submit(std::iter::once(encoder.finish()));
        surface_texture.present();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Style;

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
            buffer.set_size(&mut font_system, Some(4000.0), Some(line_height));
            let attrs = Attrs::new().family(glyphon::cosmic_text::Family::Monospace);
            buffer.set_text(
                &mut font_system,
                &"w".repeat(80),
                &attrs,
                Shaping::Basic,
                None,
            );
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

    fn sample_menu_chrome(selected: Option<usize>) -> crate::tabbar::TopTabbar {
        use crate::tabbar::{ControlsSide, Menu, MenuItem, MenuStyle, TabLabel, TopTabbar};
        let leaf = |label: &str, shortcut: &str| MenuItem {
            children: Vec::new(),
            label: label.into(),
            shortcut: shortcut.into(),
        };
        TopTabbar {
            active_tab: 0,
            controls_side: ControlsSide::Right,
            menu_style: MenuStyle::Modern,
            menus: vec![Menu {
                title: "Menu".into(),
                items: vec![
                    leaf("New Tab", "Ctrl-Shift-T"),
                    leaf("Split Vertical", ""),
                    MenuItem {
                        children: vec![leaf("Dark", ""), leaf("Light", "")],
                        label: "Theme".into(),
                        shortcut: String::new(),
                    },
                ],
            }],
            open_menu: Some(0),
            open_submenu: None,
            selected_item: selected,
            selected_subitem: None,
            tabs: vec![TabLabel {
                title: "Terminal 1".into(),
                zoomed: false,
            }],
            tabbar_hover: crate::tabbar::TabbarHit::None,
            context_menu: None,
            url_tooltip: None,
            window_controls: false,
        }
    }

    fn sample_font_ctx() -> FontCtx<'static> {
        FontCtx {
            cell_h: 18.0,
            cell_w: 9.0,
            family: None,
            font_has_bold: true,
            font_size: 14.0,
            line_height: 18.0,
            normal_weight: None,
            bold_weight: None,
        }
    }

    #[test]
    fn test_which_key_rgba_is_centered_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1000.0;
        let surface_h = 800.0;

        let view = WhichKeyView {
            title: "g + ...".to_string(),
            items: vec![
                ("g".to_string(), "top of buffer".to_string()),
                ("v".to_string(), "restore visual".to_string()),
            ],
        };

        let image = which_key_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        // Centered horizontally: left margin plus card width / 2 is close to surface_w / 2
        let card_center_x = image.x + image.width as f32 / 2.0;
        assert!(
            (card_center_x - surface_w / 2.0).abs() < 50.0,
            "which-key card is roughly centered"
        );
        // Body is non-empty and opaque
        assert!(image.width > 100);
        assert!(image.height > 50);
        let center_idx = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[center_idx + 3], 255, "card interior is opaque");
    }

    #[test]
    fn test_dropdown_rgba_is_an_elevated_rounded_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let tabbar = sample_menu_chrome(None);

        let image = dropdown_rgba(&mut font_system, &mut swash, &ctx, &theme, &tabbar, 1000.0)
            .expect("menu is open");
        let pixel = |x: u32, y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            (
                image.rgba[i],
                image.rgba[i + 1],
                image.rgba[i + 2],
                image.rgba[i + 3],
            )
        };
        let margin = DROPDOWN_SHADOW as u32;

        // The outer corner is fully transparent: the panel does not fill the
        // shadow margin (this is a floating card, not a full-bleed rectangle).
        assert_eq!(pixel(0, 0).3, 0);
        // The panel's own top-left corner is rounded away (not fully opaque)...
        assert!(pixel(margin, margin).3 < 255);
        // ...while the top padding band is the opaque elevated surface color,
        // proving both the lighter menu_bg and the vertical padding are applied.
        let (r, g, b, a) = pixel(image.width / 2, margin + 2);
        assert_eq!(
            (r, g, b, a),
            (theme.menu_bg.r, theme.menu_bg.g, theme.menu_bg.b, 255)
        );
    }

    #[test]
    fn test_toast_rgba_is_a_top_right_pill_below_the_tabbar() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1000.0;
        let top_inset = 40.0;

        let image = toast_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            "Copied to clipboard",
            theme.ansi[4],
            surface_w,
            top_inset,
        );

        // Anchored to the right: the pill's right edge is near the surface edge,
        // sitting in the right half of the surface rather than centered.
        assert!(image.x > surface_w / 2.0, "toast sits in the right half");
        assert!(
            image.x + image.width as f32 <= surface_w,
            "not clipped right"
        );
        // Dropped below the tab bar: its top is at or beneath the tab-bar height.
        assert!(image.y >= top_inset - ctx.line_height, "below the tabbar");
        // The panel's padding band is the opaque elevated surface color.
        let i = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[i + 3], 255, "pill body is opaque");
    }

    #[test]
    fn test_dropdown_overlay_is_pixel_snapped() {
        // Real displays have fractional cell metrics; without snapping, the
        // overlay lands on sub-pixel boundaries and the linear sampler blurs it.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = FontCtx {
            cell_h: 19.4,
            cell_w: 9.6,
            family: None,
            font_has_bold: true,
            font_size: 14.0,
            line_height: 19.4,
            normal_weight: None,
            bold_weight: None,
        };
        let tabbar = sample_menu_chrome(None);
        let image = dropdown_rgba(&mut font_system, &mut swash, &ctx, &theme, &tabbar, 1003.0)
            .expect("menu is open");
        assert_eq!(image.x.fract(), 0.0, "overlay x must be whole pixels");
        assert_eq!(image.y.fract(), 0.0, "overlay y must be whole pixels");
    }

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

    #[test]
    fn test_xterm_256_first_16_are_ansi() {
        assert_eq!(xterm_256_to_rgb(0), (0, 0, 0));
        assert_eq!(xterm_256_to_rgb(1), (128, 0, 0));
        assert_eq!(xterm_256_to_rgb(7), (192, 192, 192));
        assert_eq!(xterm_256_to_rgb(15), (255, 255, 255));
    }

    #[test]
    fn test_xterm_256_cube() {
        let (r, g, b) = xterm_256_to_rgb(16 + 36 + 6 + 1);
        assert!(r > 0);
        assert!(g > 0);
        assert!(b > 0);
    }

    #[test]
    fn test_xterm_256_grey_ramp() {
        let (r, g, b) = xterm_256_to_rgb(232);
        assert_eq!(r, g);
        assert_eq!(g, b);
        assert!(r >= 8);
    }

    #[test]
    fn test_grid_color_default() {
        let theme = Theme::default();
        let (r, g, b) = grid_color_to_rgb(&GridColor::Default, &theme);
        assert_eq!((r, g, b), theme.background.as_linear());
    }

    #[test]
    fn test_cell_text_fg_keeps_the_cells_own_rgb_foreground() {
        // Regression: selected text used to be repainted with a fixed
        // `theme.selection_fg`, so a cell's own ANSI/RGB color (e.g.
        // syntax-highlighted output) changed when selected. Selection
        // should only add a background highlight.
        let theme = Theme::default();
        let cell = Cell {
            style: Style {
                foreground: GridColor::Rgb(RgbColor {
                    r: 10,
                    g: 200,
                    b: 30,
                }),
                ..Style::default()
            },
            ..Cell::default()
        };
        assert_eq!(cell_text_fg(Some(&cell), &theme), (10, 200, 30));
    }

    #[test]
    fn test_cell_text_fg_swaps_on_reverse_video() {
        let theme = Theme::default();
        let cell = Cell {
            style: Style {
                foreground: GridColor::Rgb(RgbColor {
                    r: 10,
                    g: 200,
                    b: 30,
                }),
                background: GridColor::Rgb(RgbColor { r: 5, g: 5, b: 5 }),
                reversed: true,
                ..Style::default()
            },
            ..Cell::default()
        };
        assert_eq!(cell_text_fg(Some(&cell), &theme), (5, 5, 5));
    }

    #[test]
    fn test_cell_text_fg_falls_back_to_theme_foreground_for_default_color() {
        let theme = Theme::default();
        let cell = Cell::default();
        assert_eq!(
            cell_text_fg(Some(&cell), &theme),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b)
        );
        assert_eq!(
            cell_text_fg(None, &theme),
            (theme.foreground.r, theme.foreground.g, theme.foreground.b)
        );
    }

    #[test]
    fn test_resolve_fg_linear_default() {
        let theme = Theme::default();
        let (r, g, b) = resolve_fg_linear(GridColor::Default, &theme);
        assert_eq!((r, g, b), theme.foreground.as_linear());
    }

    #[test]
    fn test_grid_color_rgb() {
        let theme = Theme::default();
        let (r, g, b) = grid_color_to_rgb(
            &GridColor::Rgb(RgbColor {
                r: 255,
                g: 128,
                b: 0,
            }),
            &theme,
        );
        assert!((r - 1.0).abs() < 0.01);
        assert!((g - 0.5).abs() < 0.01);
        assert!((b - 0.0).abs() < 0.01);
    }

    #[test]
    fn test_needs_dark_on_light_bold_flags_black_on_bright_highlight() {
        // A dark badge foreground (e.g. an SGR-colored log-level tag) on a
        // bright background is exactly the case the sRGB linear-light blend
        // thins out, so it must be flagged for the synthetic-bold fix.
        assert!(needs_dark_on_light_bold((0, 0, 0), (93, 162, 235)));
        // The default theme's selection colors are also dark-on-light.
        let theme = Theme::default();
        assert!(needs_dark_on_light_bold(
            (
                theme.selection_fg.r,
                theme.selection_fg.g,
                theme.selection_fg.b
            ),
            (
                theme.selection_bg.r,
                theme.selection_bg.g,
                theme.selection_bg.b
            ),
        ));
    }

    #[test]
    fn test_needs_dark_on_light_bold_ignores_light_on_dark_and_close_luminance() {
        // Light text on a dark background is the case linear-light blending
        // already renders crisp; it must not also get bolded.
        assert!(!needs_dark_on_light_bold((255, 255, 255), (0, 0, 0)));
        // Two colors within the margin are too close in luminance for the
        // thinning artifact to be visible, so no compensation is needed.
        assert!(!needs_dark_on_light_bold((100, 100, 100), (110, 110, 110)));
    }

    #[test]
    fn test_bg_vertex_bytes_roundtrip() {
        let v = BgVertex {
            x: 1.0,
            y: -1.0,
            r: 0.5,
            g: 0.25,
            b: 0.0,
        };
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), 20);
    }

    #[test]
    fn test_build_bg_vertices_empty_grid() {
        let grid = Grid::new(4, 2);
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn test_build_bg_vertices_cursor_only() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(verts.len(), 6);
    }

    #[test]
    fn test_build_bg_vertices_hollow_cursor_draws_an_outline_not_a_fill() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: true,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        // A filled cursor is one quad (6 vertices, see the sibling test above); a
        // hollow one is four thin outline strips (24 vertices) instead.
        assert_eq!(verts.len(), 24);
        let cursor_color = Theme::default().cursor_bg.as_linear();
        assert!(
            verts.iter().all(|v| (v.r, v.g, v.b) == cursor_color),
            "every vertex should belong to the outline strips"
        );
    }

    #[test]
    fn test_build_bg_vertices_unfocused_bar_cursor_fades_instead_of_outlining() {
        // A Bar cursor is too thin to read as an outline, so an unfocused window
        // keeps its filled strip but fades it toward the background — one quad,
        // in the dimmed color, not the four strips of a hollow cell outline and
        // not the full-strength cursor color.
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: true,
                cursor_shape: CursorShape::Bar,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(verts.len(), 6);
        let faded = lerp_to_bg(theme.cursor_bg.as_linear(), theme.background.as_linear());
        assert!(
            verts.iter().all(|v| (v.r, v.g, v.b) == faded),
            "the unfocused bar should fade toward the background"
        );
    }

    #[test]
    fn test_blend_over_composites_at_the_given_alpha() {
        assert_eq!(
            blend_over((1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 0.0),
            (0.0, 0.0, 0.0)
        );
        assert_eq!(
            blend_over((1.0, 1.0, 1.0), (0.0, 0.0, 0.0), 1.0),
            (1.0, 1.0, 1.0)
        );
        let (r, g, b) = blend_over((1.0, 0.0, 0.0), (0.0, 0.0, 1.0), 0.5);
        assert!((r - 0.5).abs() < 1e-6 && g.abs() < 1e-6 && (b - 0.5).abs() < 1e-6);
    }

    #[test]
    fn test_search_highlights_tint_current_and_other_matches_differently() {
        // The focused match gets `search_current_bg`, the rest `search_match_bg`,
        // and both are composited over the cell's background rather than painted
        // opaque, so the glyph's own foreground still reads on top.
        let mut grid = Grid::new(4, 1);
        grid.print('a');
        grid.print('b');
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[(0, 0), (0, 1)],
                search_current: &[(0, 1)],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let bits = |(r, g, b): (f32, f32, f32)| (r.to_bits(), g.to_bits(), b.to_bits());
        let drawn: std::collections::HashSet<(u32, u32, u32)> =
            verts.iter().map(|v| bits((v.r, v.g, v.b))).collect();
        let bg = theme.background.as_linear();

        assert!(
            drawn.contains(&bits(blend_over(
                theme.search_match_bg.as_linear(),
                bg,
                SEARCH_MATCH_ALPHA
            ))),
            "the non-current match should be tinted with search_match_bg"
        );
        assert!(
            drawn.contains(&bits(blend_over(
                theme.search_current_bg.as_linear(),
                bg,
                SEARCH_CURRENT_ALPHA
            ))),
            "the current match should be tinted with search_current_bg"
        );
        assert!(
            !drawn.contains(&bits(theme.search_match_bg.as_linear()))
                && !drawn.contains(&bits(theme.search_current_bg.as_linear())),
            "neither highlight should be drawn at full opacity"
        );
    }

    #[test]
    fn test_cursor_contrast_fg_repaints_only_text_that_clashes_with_the_cursor() {
        let theme = Theme::dark();
        let cursor = (theme.cursor_bg.r, theme.cursor_bg.g, theme.cursor_bg.b);

        // Text in (or near) the cursor's own color would be invisible inside the
        // block, so it's repainted in something that stands out.
        let fixed = cursor_contrast_fg(cursor, &theme).expect("identical color needs a repaint");
        assert!(color_distance(fixed, cursor) >= CURSOR_CONTRAST_MIN);

        // Ordinary text already contrasts and keeps its own color.
        let fg = (theme.foreground.r, theme.foreground.g, theme.foreground.b);
        assert_eq!(cursor_contrast_fg(fg, &theme), None);
    }

    #[test]
    fn test_cursor_contrast_fg_falls_back_when_cursor_fg_also_clashes() {
        // A theme whose cursor_fg is as lost in the cursor fill as the text is
        // still has to produce something readable.
        let mut theme = Theme::dark();
        theme.cursor_bg = Rgb::new(0, 0, 0);
        theme.cursor_fg = Rgb::new(4, 4, 4);
        assert_eq!(cursor_contrast_fg((0, 0, 0), &theme), Some((255, 255, 255)));

        theme.cursor_bg = Rgb::new(255, 255, 255);
        theme.cursor_fg = Rgb::new(250, 250, 250);
        assert_eq!(cursor_contrast_fg((255, 255, 255), &theme), Some((0, 0, 0)));
    }

    #[test]
    fn test_block_cursor_cell_follows_the_shell_cursor_without_a_nav_cursor() {
        // Insert mode has no nav cursor, so the covered cell tracks the shell's
        // own caret (shifted by the scroll offset, as the drawn quad is).
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, None, true, (3, 7), 0),
            Some((3, 7))
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, None, true, (3, 7), 5),
            Some((8, 7))
        );
        // Normal/Visual: the traversal cursor wins.
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, Some((1, 2)), true, (3, 7), 0),
            Some((1, 2))
        );
    }

    #[test]
    fn test_block_cursor_cell_is_none_for_thin_hidden_or_hollow_cursors() {
        // A bar or underline leaves the glyph's ink visible, a hidden cursor
        // covers nothing, and a hollow cursor's fill doesn't cover the glyph
        // either, so none of these need a repaint.
        assert_eq!(
            block_cursor_cell(CursorShape::Bar, false, None, true, (0, 0), 0),
            None
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Underline, false, None, true, (0, 0), 0),
            None
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Block, false, None, false, (0, 0), 0),
            None
        );
        assert_eq!(
            block_cursor_cell(CursorShape::Block, true, None, true, (0, 0), 0),
            None
        );
    }

    #[test]
    fn test_selection_highlight_covers_both_endpoints_including_the_cursor_cell() {
        // The Visual-mode selection runs anchor..cursor inclusive, so the cell the
        // cursor sits on is highlighted like the rest — three cells for a
        // three-column span, not two.
        let mut grid = Grid::new(8, 1);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 80.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: Some((0, 0, 0, 2)),
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let sel = theme.selection_bg.as_linear();
        // Six vertices per cell quad.
        let selected_cells = verts.iter().filter(|v| (v.r, v.g, v.b) == sel).count() / 6;
        assert_eq!(selected_cells, 3, "cols 0, 1 and 2 should all be selected");
    }

    #[test]
    fn test_cursor_line_paints_the_whole_row_but_spares_colored_cells() {
        // Normal mode washes the cursor's row with `cursor_line_bg`, across the
        // full pane width — except cells carrying their own background, which keep
        // it so program output isn't recolored.
        let mut grid = Grid::new(4, 2);
        let colored = crate::grid::Style {
            background: GridColor::Rgb(RgbColor { r: 9, g: 90, b: 9 }),
            ..Default::default()
        };
        grid.set_style(colored);
        grid.print('x');
        grid.set_style(crate::grid::Style::default());
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                cursor_line: Some((0, 0)),
                offset_x: 0.0,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let want = theme.cursor_line_bg.as_linear();
        let banded = verts.iter().filter(|v| (v.r, v.g, v.b) == want).count() / 6;
        assert_eq!(
            banded, 3,
            "3 of the row's 4 cells are washed; the SGR-colored one keeps its own bg"
        );

        // No cursor line (Insert mode) means no wash at all.
        let plain = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                cursor_line: None,
                offset_x: 0.0,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(
            plain.iter().filter(|v| (v.r, v.g, v.b) == want).count(),
            0,
            "no cursor line outside Normal/Visual"
        );
    }

    #[test]
    fn test_cursor_line_band_covers_a_soft_wrapped_line() {
        // A wrapped command line is one logical line, so the band covers both of
        // its rows — the span comes from `Grid::wrapped_row_span`.
        let mut grid = Grid::new(4, 3);
        for ch in "abcdefg".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                cursor_line: Some(grid.wrapped_row_span(1)),
                offset_x: 0.0,
                offset_y: 0.0,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let want = theme.cursor_line_bg.as_linear();
        let banded = verts.iter().filter(|v| (v.r, v.g, v.b) == want).count() / 6;
        assert_eq!(
            banded, 8,
            "both 4-column rows of the wrapped line are washed"
        );
    }

    #[test]
    fn test_find_label_boxes_use_the_theme_find_label_background() {
        // The `f`/`t` jump labels get their own light box, distinct from the amber
        // quick-select labels, so the two overlays can't be confused.
        let mut grid = Grid::new(4, 1);
        grid.print('a');
        let theme = Theme::default();
        let find_labels: &[(usize, usize, char)] = &[(0, 2, 's')];
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels,
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: true,
            },
            &mut Vec::new(),
        );

        let want = theme.find_label_bg.as_linear();
        let boxes = verts.iter().filter(|v| (v.r, v.g, v.b) == want).count() / 6;
        assert_eq!(boxes, 1, "one label box drawn in find_label_bg");
        assert_ne!(want, QUICK_SELECT_BG, "and it differs from quick-select's");
    }

    #[test]
    fn test_cursor_quad_bar_wrap_pending_clears_the_cell() {
        // Regression: a Bar cursor parked on the last-typed glyph (deferred
        // wrap) must not draw inside that glyph's cell on either edge, since
        // monospace ink commonly spans close to both edges.
        let (x0, _, x1, _) = cursor_quad(CursorShape::Bar, 100.0, 0.0, 10.0, 20.0, true);
        assert!(
            x0 >= 110.0,
            "bar must start at or past the cell's right edge, got x0={x0}"
        );
        assert!(x1 > x0);

        let (x0_no_wrap, _, _, _) = cursor_quad(CursorShape::Bar, 100.0, 0.0, 10.0, 20.0, false);
        assert_eq!(
            x0_no_wrap,
            100.0 + CURSOR_BAR_OFFSET,
            "without a pending wrap the bar sits slightly left of the cell's left edge"
        );
    }

    #[test]
    fn test_cursor_outline_quads_frames_the_cell_on_all_four_sides() {
        let [top, bottom, left, right] = cursor_outline_quads(100.0, 50.0, 10.0, 20.0, 2.0);
        assert_eq!(top, (100.0, 50.0, 110.0, 52.0));
        assert_eq!(bottom, (100.0, 68.0, 110.0, 70.0));
        assert_eq!(left, (100.0, 50.0, 102.0, 70.0));
        assert_eq!(right, (108.0, 50.0, 110.0, 70.0));
    }

    #[test]
    fn test_build_bg_vertices_offset_produces_same_count() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let with_offset = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 100.0,
                offset_y: 50.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        let without_offset = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert_eq!(with_offset.len(), without_offset.len());
    }

    #[test]
    fn test_selection_span_clamps_to_content_end() {
        // 10-col rows; row 0 has content through col 4, rows fully inside the
        // selection run to the last column but must clamp to that content end.
        let sel = Some((0, 2, 2, 9));
        // First row: starts at the anchor col, ends at the content end (4), not 9.
        assert_eq!(selection_span_on_row(sel, false, 0, 9, 4), Some((2, 4)));
        // Middle row fully selected: 0..=last_col clamped to its own content (6).
        assert_eq!(selection_span_on_row(sel, false, 1, 9, 6), Some((0, 6)));
        // Last row: ends at the anchor col (9) clamped to content end (3).
        assert_eq!(selection_span_on_row(sel, false, 2, 9, 3), Some((0, 3)));
    }

    #[test]
    fn test_selection_span_skips_rows_outside_and_empty_spans() {
        let sel = Some((1, 0, 1, 5));
        // Rows above/below the selection are never highlighted.
        assert_eq!(selection_span_on_row(sel, false, 0, 9, 9), None);
        assert_eq!(selection_span_on_row(sel, false, 2, 9, 9), None);
        // A selection that starts past the row's content highlights nothing.
        assert_eq!(
            selection_span_on_row(Some((0, 5, 0, 9)), false, 0, 9, 2),
            None
        );
        // No selection at all.
        assert_eq!(selection_span_on_row(None, false, 0, 9, 9), None);
    }

    #[test]
    fn test_selection_span_blockwise() {
        let sel = Some((0, 3, 2, 6));
        // Block selection on rows 0, 1, 2 between columns 3 and 6:
        assert_eq!(selection_span_on_row(sel, true, 0, 9, 8), Some((3, 6)));
        assert_eq!(selection_span_on_row(sel, true, 1, 9, 8), Some((3, 6)));
        assert_eq!(selection_span_on_row(sel, true, 2, 9, 5), Some((3, 5)));
        // Outside row:
        assert_eq!(selection_span_on_row(sel, true, 3, 9, 8), None);
    }

    #[test]
    fn test_build_bg_vertices_with_selection() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: Some((0, 0, 0, 1)),
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert!(verts.len() >= 12, "selection adds extra quads");
    }

    #[test]
    fn test_build_bg_vertices_selection_uses_absolute_row_not_viewport_row() {
        // Regression: `BgParams::selection` addresses rows absolutely (see
        // `Grid::to_absolute_row`), not by the currently-visible viewport
        // row. If the row were compared without converting it first, a
        // selection anchored to scrolled-back content would silently stop
        // highlighting anything once the view had scrolled far enough that
        // no viewport row number happened to equal the absolute one.
        let mut grid = Grid::new(4, 2);
        for line in ["AAAA", "BBBB", "CCCC", "DDDD", "EEEE", "FFFF", "GGGG"] {
            for ch in line.chars() {
                grid.print(ch);
            }
            grid.carriage_return();
            grid.line_feed();
        }
        grid.scroll_up_history(3);
        let abs_row0 = grid.to_absolute_row(0);
        assert_ne!(
            abs_row0, 0,
            "fixture needs the absolute row to diverge from the viewport row"
        );

        let theme = Theme::default();
        let params = |selection| BgParams {
            ch: 20.0,
            content_right: 40.0,
            cursor_unfocused: false,
            cursor_shape: CursorShape::Block,
            cw: 10.0,
            dim: false,
            draw_braille_dots: true,
            focused: true,
            hide_cursor: false,
            hovered_link: 0,
            labels: None,
            find_labels: &[],
            cursor_line: None,
            offset_x: 0.0,
            offset_y: 0.0,
            scroll_offset: grid.scroll_offset(),
            scrollback_len: grid.scrollback_len(),
            search_matches: &[],
            search_current: &[],
            sentence_spans: &[],
            selection,
            selection_block: false,
            surface_h: 600.0,
            surface_w: 800.0,
            theme: &theme,
            url_underline: true,
        };
        let selected = build_bg_vertices_offset(
            &grid,
            params(Some((abs_row0, 0, abs_row0, 3))),
            &mut Vec::new(),
        );
        let unselected = build_bg_vertices_offset(&grid, params(None), &mut Vec::new());

        assert!(
            selected.len() > unselected.len(),
            "a selection covering the current top row's absolute address should add highlight quads"
        );
    }

    #[test]
    fn test_build_bg_vertices_url_underline_toggle() {
        let mut grid = Grid::new(4, 2);
        grid.set_active_link(Some("https://example.com"));
        grid.print('a');
        grid.set_active_link(None);

        let with_underline = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        let without_underline = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: false,
            },
            &mut Vec::new(),
        );
        assert_eq!(
            with_underline.len(),
            without_underline.len() + 6,
            "url_underline #false should drop the link cell's underline quad"
        );
    }

    #[test]
    fn test_build_bg_vertices_with_labels() {
        let mut grid = Grid::new(4, 2);
        grid.print('a');
        let labels: &[(usize, usize, char)] = &[(0, 0, 's'), (1, 2, 'd')];
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 40.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: false,
                hovered_link: 0,
                labels: Some(labels),
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &[],
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &Theme::default(),
                url_underline: true,
            },
            &mut Vec::new(),
        );
        assert!(
            verts.len() >= 12,
            "label cells add extra quads beyond cursor"
        );
    }

    #[test]
    fn test_pane_rect_conversion() {
        let rect = PaneRect {
            x: 100.0,
            y: 50.0,
            width: 400.0,
            height: 300.0,
        };
        assert_eq!(rect.width, 400.0);
        assert_eq!(rect.height, 300.0);
    }

    // Two horizontally-split panes whose pixel-snapped rects are off by exactly
    // 1 px at the shared edge (top bottom = 411, bottom top = 410) must still
    // produce a horizontal divider. Reproduces the missing-divider bug caused by
    // `layout_rect_to_pane` rounding each field independently.
    #[test]
    fn test_horizontal_divider_drawn_with_one_pixel_rounding_gap() {
        let a = PaneRect {
            x: 0.0,
            y: 55.0,
            width: 800.0,
            height: 356.0,
        };
        let b = PaneRect {
            x: 0.0,
            y: 410.0,
            width: 800.0,
            height: 356.0,
        };
        assert_eq!(a.y + a.height, 411.0); // top pane bottom edge
        assert_eq!((a.y + a.height) - b.y, 1.0); // one-pixel rounding discrepancy
        let dv = compute_divider(a, b, 800.0, 800.0, (1.0, 1.0, 1.0), 1.0);
        assert!(dv.is_some(), "divider must be drawn despite the 1 px gap");
    }

    // Same check for a vertical split whose left/right shared edge disagrees by
    // 1 px after independent rounding.
    #[test]
    fn test_vertical_divider_drawn_with_one_pixel_rounding_gap() {
        let a = PaneRect {
            x: 0.0,
            y: 0.0,
            width: 401.0,
            height: 800.0,
        };
        let b = PaneRect {
            x: 400.0,
            y: 0.0,
            width: 400.0,
            height: 800.0,
        };
        assert_eq!((a.x + a.width) - b.x, 1.0); // one-pixel rounding discrepancy
        let dv = compute_divider(a, b, 800.0, 800.0, (1.0, 1.0, 1.0), 1.0);
        assert!(dv.is_some(), "divider must be drawn despite the 1 px gap");
    }

    #[test]
    fn test_build_bg_vertices_emits_quads_for_sentence_spans() {
        let mut grid = Grid::new(10, 2);
        grid.move_to(0, 0);
        for ch in "Hi! There!".chars() {
            grid.print(ch);
        }
        let theme = Theme::default();
        let spans = [(0, 0, 3, 0), (0, 4, 10, 1)];
        let verts = build_bg_vertices_offset(
            &grid,
            BgParams {
                ch: 20.0,
                content_right: 100.0,
                cursor_unfocused: false,
                cursor_shape: CursorShape::Block,
                cw: 10.0,
                dim: false,
                draw_braille_dots: true,
                focused: true,
                hide_cursor: true,
                hovered_link: 0,
                labels: None,
                find_labels: &[],
                offset_x: 0.0,
                offset_y: 0.0,
                cursor_line: None,
                scroll_offset: 0,
                scrollback_len: 0,
                search_matches: &[],
                search_current: &[],
                sentence_spans: &spans,
                selection: None,
                selection_block: false,
                surface_h: 600.0,
                surface_w: 800.0,
                theme: &theme,
                url_underline: false,
            },
            &mut Vec::new(),
        );
        // 3 cells for sentence 1 + 6 cells for sentence 2 = 9 tinted cells * 6 verts = 54 verts
        assert_eq!(verts.len(), (3 + 6) * 6);
    }
}

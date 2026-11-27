//! What resolves each pane for misty to draw: its cells, their colors, and
//! the shapes over them ([`paint`]), from the views the window builds each
//! frame ([`PaneView`]); the font the grid is drawn in and the cell it makes
//! ([`glyphs`]); and the textures of the pictures the window shows
//! ([`images`]), on misty's GPU.

use wgpu::{Device, Queue};

use crate::render::Themes;
use crate::render::image::ImagePass;
mod frame;
mod paint;
mod surface;
mod view;

pub use display::CellOverlay;
pub use paint::PanePaint;
pub use view::{
    GridStrip, PaneView, StatusBell, StatusHover, StatusNotice, StatusSearch, THIN_SPACE, cell_at,
    cell_box, grid_area, text_area,
};
mod background;
mod colors;
mod display;
mod glyphs;
mod images;
mod svg;

pub use glyphs::{FontConfig, bold_weight, clamp_font_size, regular_weight, start_font_load};

// ========================================================================
// Data Structures
// ========================================================================

/// GPU text renderer: owns the wgpu device/queue, glyph atlas, and render
/// pipelines. Designed to be created once and reused across frames.
pub struct GpuRenderer {
    /// How the window around the panes is laid out: dividers, the bars shown, decorations, and the popups' input row.
    window_layout: WindowLayout,
    /// The font in force and the cell it makes.
    fonts: FontState,
    /// Where the last frame drew what the pointer can act on, for the window to test a click or a hover against what is on screen.
    hits: FrameHits,
    device: Device,
    /// Misty's GPU, which the images' textures are made on, so a host
    /// drawing with misty draws them too.
    gpu: misty::Gpu,
    queue: Queue,
    /// The size of the frames rendered.
    size: FrameSize,
    cols: usize,
    rows: usize,
    /// Winter's theme and misty's built from it, kept together.
    themes: Themes,
    /// The textures of the pictures and the popups, by id.
    image_pass: ImagePass,
    /// Each popup's and toolbar's last raster, so a frame that changes
    /// nothing about one reuses the texture already uploaded for it.

    /// Bumped by every theme change, so cached rasters repaint in the new
    /// colors without comparing whole themes.
    theme_generation: u64,
    /// System fonts for SVG text, loaded lazily on first SVG with text and then
    /// reused (the scan costs ~150ms, so it is deferred off the startup path).
    svg_fontdb: Option<std::sync::Arc<resvg::usvg::fontdb::Database>>,
    /// Vello's renderer for scenes (a web page laid out by Blitz), on this
    /// device: `None` until the first scene, `Some(None)` once it failed to
    /// start.
    scene_renderer: Option<Option<vello::Renderer>>,
}

/// The size of the frames a renderer renders, in pixels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct FrameSize {
    height: u32,
    width: u32,
}

/// Where the last frame drew each part the pointer acts on: written as the frame is drawn, read until the next one replaces it.
#[derive(Default)]
pub struct FrameHits {
    /// The pictures the last frame drew in the content.
    pictures: Vec<crate::render::image::ImagePlacement>,
}

impl FrameHits {
    /// The pictures the last frame drew in the content, for a host drawing
    /// them itself (see [`GpuRenderer::image_texture`]).
    pub fn pictures(&self) -> &[crate::render::image::ImagePlacement] {
        &self.pictures
    }
}

/// The grid's font and the cell it makes.
struct FontState {
    cell_height: f32,
    cell_width: f32,
    font_family: Option<String>,
    /// Whether the grid draws its family's ligatures.
    ligatures: bool,
    logical_font_size: f32,
    scale_factor: f64,
}

/// How the window around the panes is laid out, as the settings in force have it.
struct WindowLayout {
    /// Whether the OS window manager draws its own frame around the window.
    /// When `false` (the undecorated "Modern" title bar style), the window
    /// draws its own 1px [`Theme::window_border`](winter_theme::Theme::window_border)
    /// outline over everything, through misty's `Frame::draw_window_outline`.
    decorated: bool,
    /// Thickness of the 1-D line drawn between adjacent panes (in logical pixels).
    divider_width: f32,
    /// How tall the field typed into is in every popup, in rows of the font.
    popup_input_height: f32,
    /// How tall the status bar is drawn, `None` while it is hidden.
    status_bar_height: Option<f32>,
    /// The window's opacity from the config, 0.0 to 1.0, which the theme's own
    /// [`Theme::background_alpha`](winter_theme::Theme::background_alpha) multiplies.
    window_opacity: f32,
}

// ========================================================================
// Tests
// ========================================================================

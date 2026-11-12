//! GPU text renderer: renders [`Grid`](winter_emulator::Grid)s to a wgpu surface using `glyphon` +
//! `cosmic-text` for glyph rasterization and `wgpu` for compositing.
//!
//! The renderer draws two layers per frame:
//! 1. **Background layer**: colored quads for cells with non-default
//!    backgrounds, plus the cursor.
//! 2. **Text layer**: glyphon renders shaped glyphs with per-cell foreground
//!    colors.
//!
//! Multi-pane rendering is supported: pass a slice of [`PaneView`] items, each
//! with a viewport rect and a grid reference. Each pane is clipped to its rect.

use glyphon::{FontSystem, SwashCache};
use wgpu::{Device, Queue};

use crate::image::ImagePass;
use crate::theme::Theme;
mod frame;
mod paint;
mod surface;
mod view;

pub use display::CellOverlay;
pub use paint::{PaintCell, PaintFill, PaintWidth, PanePaint};
pub use view::{
    GridStrip, NoticeKind, PaneRect, PaneView, StatusBar, StatusBell, StatusHover, StatusNotice,
    StatusPrompt, StatusSearch, StatusSegment, THIN_SPACE,
};
mod background;
mod colors;
mod display;
mod glyphs;
mod images;
mod svg;
mod widgets;

pub use glyphs::{clamp_font_size, start_font_load, FontConfig, FontLoad};
pub use widgets::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoSpan, InfoView, InputView, PanelLine, PanelSpan, PanelView, PickerItem, PickerPlacement,
    PickerView, SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView,
    TransientCommand, TransientSwitch, TransientView, WhichKeyView,
};

// ========================================================================
// Data Structures
// ========================================================================

/// GPU text renderer: owns the wgpu device/queue, glyph atlas, and render
/// pipelines. Designed to be created once and reused across frames.
pub struct GpuRenderer {
    /// How the window around the panes is laid out: dividers, the bars shown, decorations, and the popups' input row.
    chrome: ChromeLayout,
    /// The font in force and what it measures: family, sizes, weights, the cell it makes, and what it can draw.
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
    font_system: FontSystem,
    swash_cache: SwashCache,
    cols: usize,
    rows: usize,
    theme: Theme,
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
    pictures: Vec<crate::image::ImagePlacement>,
}

impl FrameHits {
    /// The pictures the last frame drew in the content, for a host drawing
    /// them itself (see [`GpuRenderer::image_texture`]).
    pub fn pictures(&self) -> &[crate::image::ImagePlacement] {
        &self.pictures
    }
}

/// The font in force and the cell it makes, with what it can draw itself and the glyphs drawn from a fallback.
struct FontState {
    bold_weight: Option<String>,
    cell_height: f32,
    cell_width: f32,
    font_family: Option<String>,
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
    /// Whether the active font actually contains Braille Patterns glyphs. When it
    /// does, braille is rendered from the font (crisp, anti-aliased, cell-aligned
    /// like btop in any other terminal); when it doesn't (e.g. the default
    /// DejaVu Sans Mono), braille is drawn procedurally as a dot grid instead.
    font_has_braille: bool,
    font_size: f32,
    /// When `false` (the default), text buffers use `Shaping::Basic` (no
    /// ligatures, each glyph at its nominal cell position); when `true`,
    /// `Shaping::Advanced` is used.
    ligatures: bool,
    line_height: f32,
    logical_font_size: f32,
    normal_weight: Option<String>,
    scale_factor: f64,
}

/// How the window around the panes is laid out, as the settings in force have it.
struct ChromeLayout {
    /// Whether the OS window manager draws its own frame around the window.
    /// When `false` (the undecorated "Modern" title bar style), the renderer
    /// draws its own 1px [`Theme::window_border`] outline instead.
    decorated: bool,
    /// Thickness of the 1-D line drawn between adjacent panes (in logical pixels).
    divider_width: f32,
    /// How tall the field typed into is in every popup, in rows of the font.
    popup_input_height: f32,
    status_bar_enabled: bool,
    /// The window's opacity from the config, 0.0 to 1.0, which the theme's own
    /// [`Theme::background_alpha`] multiplies.
    window_opacity: f32,
}

// ========================================================================
// Tests
// ========================================================================

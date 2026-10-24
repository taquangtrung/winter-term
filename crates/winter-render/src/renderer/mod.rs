//! GPU text renderer: renders [`Grid`]s to a wgpu surface using `glyphon` +
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

use glyphon::{FontSystem, SwashCache, TextAtlas, TextRenderer, Viewport};
use wgpu::{Device, Queue, RenderPipeline, Surface, SurfaceConfiguration};

use crate::glyph_quad::GlyphQuadPass;
use crate::image::{ImagePass, ImagePlacement};
use crate::theme::Theme;
mod frame;
mod surface;
mod view;

pub use display::CellOverlay;
use view::ToastRect;
pub use view::{
    NoticeKind, PaneRect, PaneView, StatusBar, StatusBell, StatusHover, StatusNotice, StatusPrompt,
    StatusSearch, StatusSegment, ToastCard, ToastHit, THIN_SPACE,
};
mod background;
mod colors;
mod display;
mod glyphs;
mod images;
mod parts;
mod raster;
mod svg;
mod widgets;

use glyphs::FallbackGlyphFit;
pub use glyphs::{clamp_font_size, start_font_load, FontConfig, FontLoad};
pub use widgets::{
    ButtonView, FindView, InputView, PaletteItem, PaletteView, PickerItem, PickerView, WhichKeyView,
};

// ========================================================================
// Data Structures
// ========================================================================

/// GPU text renderer: owns the wgpu device/queue, glyph atlas, and render
/// pipelines. Designed to be created once and reused across frames.
pub struct GpuRenderer {
    device: Device,
    queue: Queue,
    surface: Surface<'static>,
    config: SurfaceConfiguration,
    font_system: FontSystem,
    swash_cache: SwashCache,
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
    /// cell width or two (`is_wide`: a double-width glyph like most emoji
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
    /// Reused buffer for building glyph atlas keys during lookups, so the
    /// per-cell lookups of a frame allocate nothing.
    glyph_key_scratch: String,
    theme: Theme,
    /// Which ways the surface can combine its alpha with the desktop, as the
    /// platform listed them; the first is the one an opaque window uses.
    alpha_modes: Vec<wgpu::CompositeAlphaMode>,
    /// The window's opacity from the config, 0.0 to 1.0, which the theme's own
    /// [`Theme::background_alpha`] multiplies.
    window_opacity: f32,
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
    /// The strip's last paint, so a frame that changes nothing about the
    /// tabbar reuses the texture already uploaded for it. Repainting it was
    /// the most expensive thing an otherwise idle frame did.
    overlay_caches: raster::OverlayCaches,
    /// Bumped by every theme change, so cached rasters repaint in the new
    /// colors without comparing whole themes.
    theme_generation: u64,
    /// Whether the last frame drew any blinking text (SGR 5), which is what
    /// the host needs a blink timer running for.
    drew_blinking_text: bool,
    /// System fonts for SVG text, loaded lazily on first SVG with text and then
    /// reused (the scan costs ~150ms, so it is deferred off the startup path).
    svg_fontdb: Option<std::sync::Arc<resvg::usvg::fontdb::Database>>,
    /// Thickness of the 1-D line drawn between adjacent panes (in logical pixels).
    divider_width: f32,
    modern: bool,
    tabbar_enabled: bool,
    status_bar_enabled: bool,
    /// Where the status bar's right-hand parts were last drawn, for the
    /// pointer to find: clicks on the bell, and hovering over any of them.
    status_hits: Vec<parts::status::StatusHit>,
    /// The status bar's icons as this frame places them, drawn by the image
    /// pass over the slots the bar's text leaves blank for them.
    status_icons: Vec<ImagePlacement>,
    /// The artwork and pixel size last uploaded to each status icon texture,
    /// so an unchanged icon is not rasterized again every frame.
    status_icon_uploads:
        [Option<(std::sync::Arc<[u8]>, u32)>; raster::STATUS_ICON_TEXTURE_IDS.len()],
    /// Where each toast card was last drawn, newest first, for the pointer.
    toast_rects: Vec<ToastRect>,
    /// Where the find popup was last drawn, as `(x, y, width, height)`, while
    /// it is up.
    find_rect: Option<(f32, f32, f32, f32)>,
    /// Whether the OS window manager draws its own frame around the window.
    /// When `false` (the undecorated "Modern" title bar style), the renderer
    /// draws its own 1px [`Theme::window_border`] outline instead.
    decorated: bool,
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(crate) mod test_support {
    use super::glyphs::FontCtx;

    pub(super) fn sample_menu_chrome(selected: Option<usize>) -> crate::tabbar::TopTabbar {
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
                    leaf("New Tab", "C-S-n"),
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
            path: String::new(),
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
    pub(super) fn sample_font_ctx() -> FontCtx<'static> {
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
}

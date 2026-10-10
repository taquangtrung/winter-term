//! The terminal screen model: a 2D cell grid driven by VT sequences.
//!
//! This is the CPU side of the native text grid. [`Grid`] holds styled cells
//! and a cursor; [`Screen`] drives it from a byte stream via `vte` (printing,
//! cursor motion, SGR colors, erase, scroll). [`renderer::GpuRenderer`] draws a
//! `Grid` to a wgpu surface using `cosmic-text` + `glyphon` for glyph rendering.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod glyph_quad;
mod grid;
mod image;
mod markdown;
pub mod renderer;
mod screen;
mod tabbar;
mod theme;

pub use grid::{
    Cell, CellWidth, Color, CursorShape, EraseMode, Grid, RgbColor, RowRemap, Style, MAX_SCROLLBACK,
};
pub use image::ImagePlacement;
pub use renderer::{
    start_font_load, FontConfig, FontLoad, InputView, NoticeKind, PaletteItem, PaletteView,
    PaneRect, PaneView, StatusBar, StatusNotice, StatusSearch, WhichKeyView,
};
pub use screen::Screen;
pub use tabbar::{
    hit_test, hit_test_pane_strip, layout_pane_strip, tabbar_rows, ContextMenu, ControlsSide,
    Menu, MenuItem, MenuStyle, PaneStripEdges, PaneStripHit, PaneStripLayout, PaneTab, Region, TabLabel,
    TabbarHit, TopTabbar,
};
pub use theme::{Rgb as ThemeRgb, Theme};

/// Horizontal inset from each edge of a pane's rect to where its cell grid
/// actually starts/ends, in pixels. The single source of truth for this
/// margin, shared by column-count math and app-level window-width snapping.
pub const PANE_H_PAD: f32 = 2.0;

/// Height of the Modern-style tabbar, expressed as a multiple of cell height.
/// Classic style always uses exactly `tabbar_rows() * ch` (i.e. 2.0).
pub(crate) const MODERN_TABBAR_HEIGHT: f32 = 1.25;

/// Flat pixel top-up added on top of `MODERN_TABBAR_HEIGHT * cell_height`,
/// independent of font size (unlike that ratio). `tabbar::tab_top_inset_px`
/// adds the same amount to the tab pill's own top inset, so this space is
/// added purely above the tabs, and the pill/active-tab-highlight's own
/// rendered height never changes.
pub(crate) const TABBAR_EXTRA_HEIGHT_PX: f32 = 2.0;

/// The Modern-style tabbar's total pixel height. The single source of truth
/// for this value; every place that needs the tabbar's on-screen height
/// (window layout, hit-testing, the rasterized strip) must call this rather
/// than reimplementing `MODERN_TABBAR_HEIGHT * cell_height`, so they can
/// never drift apart.
pub fn modern_tabbar_height_px(cell_height: f32) -> f32 {
    MODERN_TABBAR_HEIGHT * cell_height + TABBAR_EXTRA_HEIGHT_PX
}

/// Gap between a pane strip's top edge (where its divider lies) and its tab
/// pills, in pixels.
pub(crate) const PANE_STRIP_TOP_PAD_PX: f32 = 1.0;

/// Height of the line along a pane strip's bottom edge that separates it from
/// the pane below, in pixels.
pub(crate) const PANE_STRIP_SEPARATOR_PX: f32 = 1.0;

/// Space between a pane strip's separator and the pane below it, in pixels,
/// left in the pane's own color.
pub(crate) const PANE_STRIP_SEPARATOR_INSET_PX: f32 = 1.0;

/// A pane strip's total pixel height: the pill height
/// ([`modern_tabbar_height_px`]) plus the gap above the pills and the
/// separator (with its inset) below them, so the pills stay as tall as the
/// title bar's.
pub fn pane_strip_height_px(cell_height: f32) -> f32 {
    modern_tabbar_height_px(cell_height)
        + PANE_STRIP_TOP_PAD_PX
        + PANE_STRIP_SEPARATOR_PX
        + PANE_STRIP_SEPARATOR_INSET_PX
}

/// Pixel space the Modern-style title bar adds above its tab pills, beyond
/// what a pane strip has.
pub(crate) const TITLEBAR_EXTRA_HEIGHT_PX: f32 = 2.0;

/// The Modern-style title bar's total pixel height: the pill height
/// ([`modern_tabbar_height_px`]) plus the padding the title bar keeps around
/// its tab pills, so a title bar pill is exactly as tall as a pane strip's.
pub fn modern_titlebar_height_px(cell_height: f32) -> f32 {
    modern_tabbar_height_px(cell_height)
        + tabbar::tab_top_inset_px(crate::tabbar::MenuStyle::Modern)
        + tabbar::TAB_BOTTOM_VPAD_PX
}

/// Height of the status bar, expressed as a multiple of cell height.
/// Set to 1.0 for a bar that is exactly one cell tall.
pub const STATUS_BAR_HEIGHT: f32 = 1.0;

/// Top edge of the status bar in pixels: the bar is exactly [`STATUS_BAR_HEIGHT`]
/// cell rows tall and flush with the surface's bottom edge. Anchoring it to the
/// bottom (instead of stacking it after the content rows) keeps it one row no
/// matter how much sub-cell slack the window height carries, or how many extra
/// pixels the Modern tabbar adds above: that slack collects above the bar as
/// plain background.
pub(crate) fn status_bar_top_px(surface_h: f32, cell_height: f32) -> f32 {
    (surface_h - STATUS_BAR_HEIGHT * cell_height).round()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_screen_new_dimensions() {
        let screen = Screen::new(80, 24);
        assert_eq!(screen.grid().cols(), 80);
        assert_eq!(screen.grid().rows(), 24);
    }

    #[test]
    fn test_status_bar_is_exactly_one_cell_row_flush_with_the_bottom() {
        // Whatever sub-cell slack the window height carries, the bar covers the
        // last cell row and nothing more: the slack lands above it.
        for surface_h in [600.0_f32, 607.0, 613.5] {
            let ch = 20.0;
            let top = status_bar_top_px(surface_h, ch);
            assert_eq!(
                (surface_h - top).round(),
                ch,
                "bar height at surface_h={surface_h} should be one cell row"
            );
        }
    }
}

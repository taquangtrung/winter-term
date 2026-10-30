//! The GPU renderer: draws a [`winter_core::Grid`] and the window chrome
//! around it to a wgpu surface, using `cosmic-text` + `glyphon` for glyph
//! rendering. The grid and the VT parsing that drives it live in `winter-core`;
//! this crate only draws them.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod glyph_quad;
mod image;
mod markdown;
pub mod renderer;
mod syntax;
mod tabbar;
mod theme;
mod theme_file;

pub use image::ImagePlacement;
pub use renderer::{
    clamp_font_size, start_font_load, ButtonView, CellOverlay, FindView, FontConfig, FontLoad,
    InputView, NoticeKind, PaneRect, PaneView, PickerItem, PickerView,
    StatusBar, StatusBell, StatusHover, StatusNotice, StatusPrompt, StatusSearch, StatusSegment,
    ToastCard, ToastHit, TokenInspectView, ToolbarItem, ToolbarTone, ToolbarView, WhichKeyView,
    CompletionRow, CompletionView, DropListRow, DropListView, InfoView, THIN_SPACE,
};
pub use syntax::{ScopeRule, SyntaxColors, SCOPE_COUNT, SCOPE_RULES};
pub use tabbar::{
    hit_test, hit_test_pane_strip, layout_pane_strip, tabbar_rows, ContextMenu, ControlsSide, Menu,
    MenuItem, MenuStyle, PaneStripDropTarget, PaneStripHit, PaneStripLayout,
    PaneTab, Region, TabLabel, TabbarHit, TopTabbar,
};
pub use theme::{GitColors, Rgb as ThemeRgb, Theme, PAGE_DIM_MIX};
pub use theme_file::{
    kdl_string, parse_theme_file, theme_kdl, ColorOverrides, KdlColors, ScalarColor, SCALAR_COLORS,
};

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

/// The status bar's border: a hairline along its top.
const STATUS_BAR_BORDER_PX: f32 = 1.0;

/// Room in the status bar above its row of contents, below the border.
pub const STATUS_BAR_PAD_TOP_PX: f32 = 0.0;

/// Room in the status bar below its row of contents, above the window's edge.
pub const STATUS_BAR_PAD_BOTTOM_PX: f32 = 2.0;

/// The status bar's whole height in whole pixels, so it and the panes above
/// it meet on one pixel edge: its border, its top padding, its one row of
/// contents, and its bottom padding.
pub fn status_bar_height_px(cell_height: f32) -> f32 {
    (STATUS_BAR_BORDER_PX + STATUS_BAR_PAD_TOP_PX + cell_height + STATUS_BAR_PAD_BOTTOM_PX).round()
}

/// How tall the field typed into is in every popup (a picker, the find box,
/// a question), in rows of the terminal's font, unless configured otherwise:
/// a line of text with room above and below it.
pub const DEFAULT_POPUP_INPUT_HEIGHT: f32 = 1.5;

/// The shortest and tallest a popup's field may be configured, in rows.
pub const POPUP_INPUT_HEIGHT_RANGE: (f32, f32) = (1.0, 4.0);

/// How far non-focused pane colors are lerped toward the background (0 = full,
/// 1 = fully dimmed). Public so what the host draws over a pane itself, such
/// as an editor's icons, can fade by the same amount.
pub const DIM_FACTOR: f32 = 0.4;

/// Top edge of the status bar in pixels: the bar is [`status_bar_height_px`]
/// tall and flush with the surface's bottom edge. Anchoring it to the
/// bottom (instead of stacking it after the content rows) keeps its height no
/// matter how much sub-cell slack the window height carries, or how many extra
/// pixels the Modern tabbar adds above: that slack collects above the bar as
/// plain background.
pub(crate) fn status_bar_top_px(surface_h: f32, cell_height: f32) -> f32 {
    (surface_h - status_bar_height_px(cell_height)).round()
}

/// The row the status bar's contents sit in (its text, icons, hover
/// highlight, and prompt cursor), as `(top, bottom)` pixels, for the bar
/// whose top edge is `status_top`: below its border and its top padding,
/// one cell row tall.
pub(crate) fn status_bar_content_px(status_top: f32, cell_height: f32) -> (f32, f32) {
    let top = status_top + STATUS_BAR_BORDER_PX + STATUS_BAR_PAD_TOP_PX;
    (top, top + cell_height)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_status_bar_is_its_border_paddings_and_row_flush_with_the_bottom() {
        // Whatever sub-cell slack the window height carries, the bar covers
        // its border, paddings, and row and nothing more: the slack lands
        // above it.
        let ch = 19.0;
        let tall = STATUS_BAR_BORDER_PX + STATUS_BAR_PAD_TOP_PX + ch + STATUS_BAR_PAD_BOTTOM_PX;
        for surface_h in [600.0_f32, 607.0, 613.5] {
            let top = status_bar_top_px(surface_h, ch);
            assert_eq!((surface_h - top).round(), tall, "at surface_h={surface_h}");
        }
    }

    #[test]
    fn test_the_status_bar_s_paddings_lie_about_its_contents() {
        let (top, ch) = (100.0, 19.0);
        let (contents_top, contents_bottom) = status_bar_content_px(top, ch);
        assert_eq!(contents_top - (top + STATUS_BAR_BORDER_PX), STATUS_BAR_PAD_TOP_PX, "under the border");
        assert_eq!(contents_bottom - contents_top, ch, "one row");
        let bar_bottom = top + status_bar_height_px(ch);
        assert_eq!(bar_bottom - contents_bottom, STATUS_BAR_PAD_BOTTOM_PX, "above the window's edge");
    }
}

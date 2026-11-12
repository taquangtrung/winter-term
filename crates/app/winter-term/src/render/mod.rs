//! The window's drawing layer under misty: each pane resolved into cells
//! and shapes ([`renderer`]), the pictures' textures, Markdown laid out for
//! a picture, and the title bar's styles. The theme and the syntax scopes
//! come from `winter-theme`, and the views of what floats over the panes
//! from `winter_model::view`; both are re-exported here, so the window
//! reaches everything it draws from through `crate::render`.

mod image;
mod markdown;
pub(crate) mod renderer;
mod tabbar;

pub use image::ImagePlacement;
pub use renderer::{
    clamp_font_size, start_font_load, CellOverlay, FontConfig, PaintCell, PaintFill, PaintWidth,
    PanePaint, PaneRect, StatusBar, StatusBell, StatusHover, StatusNotice, StatusPrompt,
    StatusSearch, StatusSegment, THIN_SPACE,
};
pub use tabbar::{ControlsSide, MenuStyle, Region};
pub use winter_model::view::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoSpan, InfoView, InputView, NoticeKind, PanelLine, PanelSpan, PanelView, PickerItem,
    PickerPlacement, PickerView, SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem,
    ToolbarTone, ToolbarView, TransientView, WhichKeyView,
};
pub use winter_theme::{
    kdl_string, parse_theme_file, theme_kdl, ColorOverrides, KdlColors, Rgb as ThemeRgb, Theme,
    DIM_FACTOR, PAGE_DIM_MIX, SCOPE_RULES, SENTENCE_TINT_ALPHAS,
};

/// Horizontal inset from each edge of a pane's rect to where its content
/// (cells, icons, pictures) actually starts/ends, in logical pixels: every
/// kind of tab takes the same margin. Scaled to the display by
/// [`pane_h_pad`].
pub const PANE_H_PAD: f32 = 2.0;

/// [`PANE_H_PAD`] in screen pixels at `scale_factor`, on whole pixels so the
/// text it insets starts on one.
pub fn pane_h_pad(scale_factor: f64) -> f32 {
    (PANE_H_PAD * scale_factor as f32).round()
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

/// Top edge of the status bar in pixels: the bar is [`status_bar_height_px`]
/// tall and flush with the surface's bottom edge. Anchoring it to the
/// bottom (instead of stacking it after the content rows) keeps its height no
/// matter how much sub-cell slack the window height carries, or how many extra
/// pixels the Modern tabbar adds above: that slack collects above the bar as
/// plain background.
pub(crate) fn status_bar_top_px(surface_h: f32, cell_height: f32) -> f32 {
    (surface_h - status_bar_height_px(cell_height)).round()
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
}

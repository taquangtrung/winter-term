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
mod theme;

pub use image::ImagePlacement;
pub use renderer::{
    CellOverlay, FontConfig, PanePaint, StatusBell, StatusHover, StatusNotice, StatusSearch,
    THIN_SPACE, bold_weight, cell_at, cell_box, clamp_font_size, grid_area, regular_weight,
    start_font_load, text_area,
};
pub use tabbar::{ControlsSide, MenuStyle, Region};
pub use theme::Themes;
pub(crate) use theme::mix;
pub use winter_model::view::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoView, InputView, NoticeKind, PanelView, PickerItem, PickerPlacement, PickerView,
    SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView,
    TransientView, WhichKeyView,
};
pub use winter_theme::{
    ColorOverrides, DIM_FACTOR, KdlColors, PAGE_DIM_MIX, Rgb as ThemeRgb, SCOPE_RULES,
    SENTENCE_TINT_ALPHAS, Theme, kdl_string, parse_theme_file, theme_kdl,
};

/// Horizontal inset from each edge of a pane's rect to where its content
/// (cells, icons, pictures) actually starts/ends, in logical pixels: every
/// kind of tab takes the same margin. Scaled to the display by
/// [`pane_h_pad`].
pub const PANE_H_PAD: f32 = 6.0;

/// How wide a pane's scrollbar is, in screen pixels: it runs down the pane
/// just inside [`PANE_H_PAD`] from its right edge.
pub const SCROLLBAR_WIDTH: f32 = 5.0;

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

//! What floats over the panes, each drawn in the chrome the shared
//! [`super::raster::chrome`] paints, from the views in [`view`]: the dialogs
//! ([`input`]) and the pane toolbars ([`toolbar`]). The find card, the token
//! inspector, the picker, the completion list, the info card, the drop
//! list, the menus, and the tooltips are misty's widgets now, drawn by the
//! app.

pub(in crate::renderer) mod button;
pub(in crate::renderer) mod input;
pub(in crate::renderer) mod toolbar;
mod view;

pub use view::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoSpan, InfoView, InputView, PanelLine, PanelSpan, PanelView, PickerItem, PickerPlacement,
    PickerView, SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView,
    TransientCommand, TransientSwitch, TransientView, WhichKeyView,
};

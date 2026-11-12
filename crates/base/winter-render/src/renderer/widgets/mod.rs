//! What floats over the panes, each drawn in the chrome the shared
//! [`super::raster::chrome`] paints (which also names the five classes),
//! from the views in [`view`]: the dialogs ([`input`]); the tooltips, a link's address ([`tooltip`]), and the
//! scopes under the cursor ([`token_inspect`]); and the find card
//! ([`find`]). The picker, the completion list, the info card, the drop
//! list, and the menus are misty's widgets now, drawn by the app.

pub(in crate::renderer) mod button;
pub(in crate::renderer) mod find;
pub(in crate::renderer) mod input;
pub(in crate::renderer) mod token_inspect;
pub(in crate::renderer) mod toolbar;
mod view;

pub use view::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoSpan, InfoView, InputView, PanelLine, PanelSpan, PanelView, PickerItem, PickerPlacement,
    PickerView, SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView,
    TransientCommand, TransientSwitch, TransientView, WhichKeyView,
};

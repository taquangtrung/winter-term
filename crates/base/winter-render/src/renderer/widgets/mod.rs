//! What floats over the panes, each drawn in the chrome the shared
//! [`super::raster::chrome`] paints (which also names the five classes),
//! from the views in [`view`]: the dialogs ([`input`]); the tooltips, a link's address ([`tooltip`]), and the
//! scopes under the cursor ([`token_inspect`]); and the menus ([`menu`]),
//! the find card ([`find`]). The picker, the completion list, the info
//! card, and the drop list are misty's widgets now, drawn by the app.

pub(in crate::renderer) mod button;
pub(in crate::renderer) mod find;
pub(in crate::renderer) mod input;
pub(in crate::renderer) mod menu;
pub(in crate::renderer) mod token_inspect;
pub(in crate::renderer) mod toolbar;
pub(in crate::renderer) mod tooltip;
pub(in crate::renderer) mod transient;
mod view;

pub use transient::TransientHit;
pub use view::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoSpan, InfoView, InputView, PanelLine, PanelSpan, PanelView, PickerItem, PickerPlacement,
    PickerView, SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView,
    TransientCommand, TransientSwitch, TransientView, WhichKeyView,
};

// ========================================================================
// Constants
// ========================================================================

/// Inset of the hover-highlight pill from the dropdown item-row edges, in pixels.
const MENU_HOVER_INSET: f32 = 4.0;

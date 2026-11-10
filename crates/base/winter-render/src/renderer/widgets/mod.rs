//! What floats over the panes, each drawn in the chrome the shared
//! [`super::raster::chrome`] paints (which also names the five classes),
//! from the views in [`view`]: the picker ([`picker`]); the dialogs
//! ([`input`]); the key-hint HUD ([`which_key`]); the popups at the cursor,
//! the fixes ([`popup`]) and the completions ([`completion`]); the tooltips,
//! a link's address ([`tooltip`]), what a name is ([`info`]), and the
//! scopes under the cursor ([`token_inspect`]); and the menus ([`menu`]),
//! the find card ([`find`]), and notification toasts ([`toast`]).

pub(in crate::renderer) mod button;
pub(in crate::renderer) mod completion;
pub(in crate::renderer) mod drop_list;
pub(in crate::renderer) mod find;
pub(in crate::renderer) mod info;
pub(in crate::renderer) mod input;
pub(in crate::renderer) mod menu;
pub(in crate::renderer) mod picker;
pub(in crate::renderer) mod popup;
pub(in crate::renderer) mod toast;
pub(in crate::renderer) mod token_inspect;
pub(in crate::renderer) mod toolbar;
pub(in crate::renderer) mod tooltip;
pub(in crate::renderer) mod transient;
mod view;
pub(in crate::renderer) mod which_key;

pub use picker::{PickerButtonHit, PickerRowHit};
pub use transient::TransientHit;
pub use view::{
    ButtonView, ColumnAlign, CompletionRow, CompletionView, DropListRow, DropListView, FindView,
    InfoSpan, InfoView, InputView, PanelLine, PanelSpan, PanelView, PickerItem, PickerPlacement,
    PickerView, SwitchKind, TokenInspectView, ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView,
    TransientCommand, TransientSwitch, TransientView, WhichKeyView,
};
pub use which_key::WhichKeyHit;

// ========================================================================
// Constants
// ========================================================================

/// Inset of the hover-highlight pill from the dropdown item-row edges, in pixels.
const MENU_HOVER_INSET: f32 = 4.0;

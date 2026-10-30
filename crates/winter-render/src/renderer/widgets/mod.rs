//! What floats over the panes, each drawn in the chrome the shared
//! [`super::raster::chrome`] paints (which also names the five classes),
//! from the views in [`view`]: the picker ([`picker`]); the dialogs
//! ([`input`]); the key-hint HUD ([`which_key`]); the popups at the cursor,
//! the fixes ([`popup`]) and the completions ([`completion`]); the tooltips,
//! a link's address ([`tooltip`]), what a name is ([`info`]), and the
//! scopes under the cursor ([`token_inspect`]); and the menus ([`menu`]),
//! the find card ([`find`]), and notification toasts ([`toast`]).

pub(in crate::renderer) mod button;
pub(in crate::renderer) mod find;
pub(in crate::renderer) mod input;
pub(in crate::renderer) mod menu;
pub(in crate::renderer) mod picker;
pub(in crate::renderer) mod popup;
pub(in crate::renderer) mod toast;
pub(in crate::renderer) mod token_inspect;
pub(in crate::renderer) mod completion;
pub(in crate::renderer) mod drop_list;
pub(in crate::renderer) mod info;
pub(in crate::renderer) mod toolbar;
pub(in crate::renderer) mod tooltip;
mod view;
pub(in crate::renderer) mod which_key;

pub use view::{
    ButtonView, FindView, InputView, PickerItem, PickerView,
    CompletionRow, CompletionView, DropListRow, DropListView, InfoView, TokenInspectView, ToolbarItem, ToolbarTone, ToolbarView,
    WhichKeyView,
};

// ========================================================================
// Constants
// ========================================================================

/// Inset of the hover-highlight pill from the dropdown item-row edges, in pixels.
const MENU_HOVER_INSET: f32 = 4.0;

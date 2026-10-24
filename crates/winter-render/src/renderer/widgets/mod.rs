//! What floats over the panes: the menus ([`menu`]), the command palette
//! ([`palette`]), the input dialog ([`input`]), the find popup ([`find`]),
//! the key-hint card ([`which_key`]), notification toasts ([`toast`]), and
//! the link tooltip ([`tooltip`]), each drawn on the elevated card the
//! shared [`super::raster::popup`] paints, from the views in [`view`].

pub(in crate::renderer) mod button;
pub(in crate::renderer) mod find;
pub(in crate::renderer) mod input;
pub(in crate::renderer) mod menu;
pub(in crate::renderer) mod palette;
pub(in crate::renderer) mod toast;
pub(in crate::renderer) mod tooltip;
mod view;
pub(in crate::renderer) mod which_key;

pub use view::{ButtonView, FindView, InputView, PaletteItem, PaletteView, WhichKeyView};

// ========================================================================
// Constants
// ========================================================================

/// Inset of the hover-highlight pill from the dropdown item-row edges, in pixels.
const MENU_HOVER_INSET: f32 = 4.0;

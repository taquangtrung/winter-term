//! Where input goes: each key press routed through the layers that may claim
//! it, the pointer routed to what lies under it, the overlay holding the
//! keyboard, multi-key sequences and the which-key hint, action dispatch,
//! which tab has focus, and input-method composition.

pub(crate) mod actions;
pub(crate) mod focus;
pub(in crate::window) mod ime;
pub(crate) mod keys;
pub(in crate::window) mod mouse;
pub(crate) mod overlay;
pub(crate) mod sequence;

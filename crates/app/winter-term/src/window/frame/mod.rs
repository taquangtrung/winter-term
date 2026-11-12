//! Composing each frame for the renderer: the panes' cell grids and view state,
//! the palette and which-key popups, the canvas's images, and the blink clock
//! that times carets and blinking text.
//! The drawing itself happens in `winter_render`.

pub(crate) mod blink;
pub(crate) mod canvas;
pub(crate) mod compose;
pub(in crate::window) mod frame_grid;
pub(in crate::window) mod inspect;
pub(in crate::window) mod menus;
mod overlays;
pub(in crate::window) mod pane_popups;
pub(in crate::window) mod panes;
pub(in crate::window) mod popups;
pub(in crate::window) mod settings;
pub(in crate::window) mod title_bar;
pub(in crate::window) mod transient;
pub(in crate::window) mod which_key;

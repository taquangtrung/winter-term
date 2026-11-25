//! Panes and the tabs in them: creating, splitting, and closing panes, opening
//! and cycling tabs, what each open tab holds, and the window geometry they
//! occupy.

pub(in crate::window) mod bands;
pub(crate) mod geometry;
pub(in crate::window) mod open_tabs;
pub(crate) mod panes;
pub(crate) mod tabs;

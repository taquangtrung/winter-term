//! Winter native app: winit window, GPU text renderer, interactive PTY panes,
//! split-tree layout, and interaction modes. The `Winter` binary is a thin entry
//! point that creates an [`app::App`] and runs the winit event loop.
//!
//! This library is published so the binary can be built from the registry, not
//! as an interface to build on. It carries no semver guarantee; pin an exact
//! version if you depend on it.

#![forbid(unsafe_code)]

pub mod app;
pub mod config;
pub mod control;
pub mod model;
pub mod mux;
pub(crate) mod paths;
pub mod session;
pub mod terminal;

pub use model::input::{resolve, Action, BlockNav, Key, KeyCode};
pub use model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
pub use model::mode::{Mode, ModeEvent};

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    #[test]
    fn test_lib_exports_mode() {
        let _ = super::Mode::default();
    }
}

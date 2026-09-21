//! Winter native app: winit window, GPU text renderer, interactive PTY panes,
//! split-tree layout, and interaction modes. The `Winter` binary is a thin entry
//! point that creates an [`app::App`] and runs the winit event loop.
//!
//! The architecture is layered on a Vim foundation ([`model::vim`]): the
//! vocabulary, text primitives, and key mapping every Vim surface shares.
//! The terminal's grid-based Normal mode stands on it, and the tool pages
//! ([`tools`]) stand on it in turn, reinterpreting the same motions over
//! their own content with their own keys overriding the defaults.
//!
//! This library is published so the binary can be built from the registry, not
//! as an interface to build on. It carries no semver guarantee; pin an exact
//! version if you depend on it.

#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod app;
pub mod config;
pub mod control;
pub(crate) mod icons;
pub mod model;
pub mod mux;
pub(crate) mod paths;
pub mod session;
pub mod terminal;
pub mod tools;

pub use model::input::{resolve, Action, BlockNav, Key, KeyCode};
pub use model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
pub use model::mode::{Mode, ModeEvent};

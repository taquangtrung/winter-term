//! Winter native app: winit window, GPU text renderer, terminal and editor tabs,
//! split-tree layout, and interaction modes. The `Winter` binary is a thin entry
//! point that creates an [`window::App`] and runs the winit event loop.
//!
//! The architecture is layered on a Vim foundation ([`winter_model::vim`]): the
//! vocabulary, text primitives, and key mapping every Vim surface shares.
//! The terminal's grid-based Normal mode stands on it, and the editors
//! ([`editors`]) stand on it in turn, reinterpreting the same motions over
//! their own content with their own keys overriding the defaults.
//!
//! This library is published so the binary can be built from the registry, not
//! as an interface to build on. It carries no semver guarantee; pin an exact
//! version if you depend on it.

#![deny(unsafe_code)]
#![deny(missing_docs)]

pub mod config;
pub mod control;
pub mod editors;
pub(crate) mod extensions;
#[cfg(feature = "wasm-extensions")]
pub use extensions::cli as extension_cli;
pub(crate) mod icons;
pub(crate) mod quota;
pub(crate) mod render;
pub mod session;
pub mod window;
pub mod workspace;

pub use winter_model::input::{resolve, Action, BlockNav, Key, KeyCode};
pub use winter_model::layout::{Direction, FocusDir, Layout, Rect, TabId};
pub use winter_model::mode::{Mode, ModeEvent};

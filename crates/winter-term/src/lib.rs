//! Winter native app: winit window, GPU text renderer, terminal and editor tabs,
//! split-tree layout, and interaction modes. The `Winter` binary is a thin entry
//! point that creates an [`window::App`] and runs the winit event loop.
//!
//! The architecture is layered on a Vim foundation ([`model::vim`]): the
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

pub(crate) mod bytes_text;
pub(crate) mod chart;
pub(crate) mod extensions;
pub mod config;
pub mod control;
pub(crate) mod crash_log;
pub mod editors;
pub(crate) mod icons;
pub mod model;
pub mod mux;
pub(crate) mod paths;
pub(crate) mod persist;
pub(crate) mod process;
pub mod project;
pub(crate) mod quota;
pub(crate) mod remote;
pub(crate) mod wake;
pub(crate) mod watch;
pub mod session;
pub mod terminal;
pub mod workspace;
pub mod window;

pub use model::input::{resolve, Action, BlockNav, Key, KeyCode};
pub use model::layout::{Direction, FocusDir, Layout, Rect, TabId};
pub use model::mode::{Mode, ModeEvent};

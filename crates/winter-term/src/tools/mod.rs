//! Tools: self-contained pages a pane can hold instead of a terminal.
//!
//! One module per tool. Everything in a tool stays pure and std-only except
//! the modules named for their side effects, so a tool's logic is testable
//! without a window, a filesystem, or a repository.
//!
//! - [`dir`]: a directory listing.
//! - [`editor`]: one file as editable text.
//! - [`git`]: the working tree's state.
//! - [`grep`]: the lines under a directory holding some text.
//! - [`keys`]: the command list and the chord bound to each command.
//! - [`pdf`]: one PDF document, drawn by a WebView the page owns.

pub mod dir;
pub mod editor;
pub mod git;
pub mod grep;
pub mod keys;
pub mod pdf;

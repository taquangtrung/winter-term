//! Winter's link between misty and the Cherry editor, which know nothing
//! of each other. Misty owns its key values and resolves no key presses;
//! this crate resolves them with Cherry's keymap, and edits text with
//! Cherry's modal editor:
//!
//! - [`keys`] converts misty's keys to Cherry's and back, without loss.
//! - [`clock`] gives Cherry timestamps on one clock with misty's
//!   `Instant`s.
//! - [`keymap`] binds a misty `CommandRegistry` into a
//!   `cherry_keys::Keymap` and turns a pending prefix's continuations into
//!   which-key rows.
//! - [`EditorView`] shows one view of a Cherry [`Editor`](cherry::Editor)
//!   as a misty widget, painted through the cell grid in the theme's
//!   colors.
//!
//! The Cherry crates are re-exported as [`cherry`] and [`cherry_keys`], and
//! with the `syntax` feature `cherry_syntax`, which highlights the editor's
//! text in the theme's syntax colors.

pub mod clock;
mod editor_view;
pub mod keymap;
pub mod keys;

pub use cherry;
pub use cherry_keys;
#[cfg(feature = "syntax")]
pub use cherry_syntax;
pub use editor_view::{EditorClipboard, EditorResponse, EditorView, HighlightStyle, LentEditor};

//! Keys for keyboard-driven apps, independent of any window toolkit: the
//! host converts its own key events into a [`Key`], and everything here
//! works on that.
//!
//! - [`chord`]: chords as Emacs notation writes them (`C-M-S-s-x`, `RET`,
//!   `<next>`): parsing a spec into keys, formatting keys back, and matching
//!   a pressed key against a bound one.
//! - [`sequence`]: bindings of two or more chords typed one after another,
//!   looked up as a prefix grows one key at a time.
//! - [`table`]: key tables, where each key is written once beside what it
//!   does, so dispatch, help, and rebinding all read the same entry.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod chord;
mod key;
pub mod sequence;
pub mod table;

pub use key::{Key, KeyCode, Mods};

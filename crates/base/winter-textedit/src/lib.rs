//! Modal text editing: Emacs keys while typing, Vim keys for everything else,
//! over text that knows nothing of files or panes.
//!
//! - [`buffer`]: the lines, the cursor over them, and every edit to either.
//! - [`field`]: a buffer whose lines are fixed, with the keys over it, for a
//!   one-line answer or a column of names.
//! - [`graphemes`]: what a reader sees as one character, and how wide it is.
//! - [`kill`]: the kill ring the Emacs kills and `C-y`/`M-y` share.
//! - [`modal`]: what each key does in each mode, and what it asks of the host.
//! - [`place`]: where a view puts the cursor's screen row.
//! - [`recall`]: earlier answers to a prompt's question, for the arrow keys.
//! - [`search`]: finding text in the lines.
//! - [`vim`]: the Vim vocabulary, text primitives, and grammar every modal
//!   surface resolves keys through.
//!
//! Keys come in as [`winter_keys::Key`]s, which the host converts from its
//! own window toolkit's events.

#![deny(missing_docs)]
#![forbid(unsafe_code)]

pub mod buffer;
pub mod field;
pub mod graphemes;
pub mod kill;
pub mod modal;
pub mod place;
pub mod recall;
pub mod search;
pub mod vim;

//! Winter's interaction model: modes, layout geometry, key resolution, the
//! palette, the settings page, the editors tabs hold, and the styled rows
//! they paint. The window that drives it is `winter-term`'s; nothing here
//! opens one.
//!
//! Beside the model sit the helpers every editor shares, so an editor built
//! in a crate of its own needs nothing of the app's:
//!
//! - [`background`]: work an editor runs on a thread of its own.
//! - [`list`]: the window over an editor's rows, and the cursor on them.
//! - [`text_file`]: reading a file into editable lines and writing them back.
//! - [`word_diff`]: the word-level diff that decorates a changed line.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod background;
pub mod button;
pub mod editor;
pub mod guard;
pub mod history;
pub mod input;
pub mod layout;
pub mod list;
pub mod mode;
pub mod notify;
pub mod paint;
pub mod path;
pub mod picture;
pub mod process;
pub mod settings_page;
pub mod spinner;
pub mod system;
pub mod text_file;
pub mod textedit;
pub mod units;
pub mod view;
pub mod word_diff;

pub use winter_textedit::vim;

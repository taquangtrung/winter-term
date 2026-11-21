//! Winter's OS plumbing, shared by the editor model and the app: where the
//! per-user files live and how they are written, the helper programs Winter
//! starts, the places files can be other than this user's own disk, and
//! waking the event loop from the threads that feed it.
//!
//! - [`bytes_text`]: bytes a program wrote, held as text without losing any.
//! - [`crash_log`]: every panic written with its backtrace to `crash.log`.
//! - [`edit_wait`]: answering a `winter --edit-wait` once its editor ends.
//! - [`paths`]: the private runtime, state, and config directories.
//! - [`persist`]: writing the app's own files so a write is never half done.
//! - [`process`]: starting helper programs, and signalling them.
//! - [`project`]: the project a directory is in, by its markers.
//! - [`remote`]: files as root, inside a container, or on a host over SSH.
//! - [`wake`]: waking the event loop when an answer arrives.
//! - [`watch`]: what the file watchers share.

#![deny(missing_docs)]
#![deny(unsafe_code)]

pub mod bytes_text;
pub mod crash_log;
pub mod edit_wait;
pub mod paths;
pub mod persist;
pub mod process;
pub mod project;
pub mod remote;
pub mod wake;
pub mod watch;

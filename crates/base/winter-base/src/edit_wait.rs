//! Answering a `winter --edit-wait`: the program blocked on an editor (git
//! waiting on a commit message) is told, once the editor ends, whether the
//! edit stands.

use std::io::Write;
#[cfg(unix)]
use std::os::unix::net::UnixStream;

#[cfg(windows)]
use uds_windows::UnixStream;

// ========================================================================
// Constants
// ========================================================================

/// The byte answering a waiting `--edit-wait`: the edit stands.
pub const ANSWER_ACCEPTED: u8 = 1;

/// The byte answering a waiting `--edit-wait`: the edit was discarded.
pub const ANSWER_REJECTED: u8 = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// Answers a waiting `winter --edit-wait` when dropped, so the program blocked
/// on the editor continues however the editing editor came to an end: closed,
/// replaced, or torn down with the app.
#[derive(Debug)]
pub struct EditWaiter {
    /// Whether the edit stands; `false` tells the waiter to abort what it was
    /// editing for.
    accepted: bool,
    stream: UnixStream,
}

// ========================================================================
// EditWaiter Implementations
// ========================================================================

impl EditWaiter {
    /// A waiter answered on `stream`, the edit standing unless rejected.
    pub fn new(stream: UnixStream) -> Self {
        Self {
            accepted: true,
            stream,
        }
    }

    /// Answer that the edit was thrown away, so the waiter exits with failure.
    pub fn reject(&mut self) {
        self.accepted = false;
    }
}

impl Drop for EditWaiter {
    fn drop(&mut self) {
        let answer = match self.accepted {
            true => ANSWER_ACCEPTED,
            false => ANSWER_REJECTED,
        };
        let _ = self.stream.write_all(&[answer]);
    }
}

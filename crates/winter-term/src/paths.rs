//! Shared OS runtime-directory resolution, used by every IPC socket path
//! (mux, control channel) so they agree on where to look.

// ========================================================================
// Runtime directory
// ========================================================================

/// The directory IPC sockets live in: `$XDG_RUNTIME_DIR`, then `$HOME/.run`,
/// then `%LOCALAPPDATA%`, then `%TEMP%`/`$TEMP`, or `None` if none resolve.
pub(crate) fn runtime_dir() -> Option<String> {
    std::env::var("XDG_RUNTIME_DIR")
        .or_else(|_| std::env::var("HOME").map(|h| format!("{h}/.run")))
        .or_else(|_| std::env::var("LOCALAPPDATA"))
        .or_else(|_| std::env::var("TEMP"))
        .ok()
}

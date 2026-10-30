//! The crash log: every panic, caught or not, written with its backtrace to
//! `crash.log` in the state directory, so a crash leaves something to read
//! after the window is gone, and a panic Winter recovered from can still be
//! looked into.

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::PanicHookInfo;

// ========================================================================
// Constants
// ========================================================================

/// The log's name in the state directory.
const FILE_NAME: &str = "crash.log";

/// How large the log grows before the next panic starts it afresh, so a
/// panic repeating for days never fills the disk.
const MAX_BYTES: u64 = 1024 * 1024;

// ========================================================================
// Functions
// ========================================================================

/// Where the log is written, when there is a state directory to put it in.
pub(crate) fn path() -> Option<std::path::PathBuf> {
    crate::paths::state_file(FILE_NAME)
}

/// Append the panic `info` describes to the log, with a backtrace. Never
/// panics itself, since a panic inside the panic hook aborts the process:
/// whatever goes wrong writing the log is let go.
pub(crate) fn append(info: &PanicHookInfo) {
    let Some(path) = path() else {
        return;
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let full = std::fs::metadata(&path).is_ok_and(|meta| meta.len() > MAX_BYTES);
    let file = OpenOptions::new()
        .create(true)
        .append(!full)
        .truncate(full)
        .write(true)
        .open(&path);
    let Ok(mut file) = file else {
        return;
    };
    let thread = std::thread::current();
    let entry = format!(
        "--- {} on thread {}\n{info}\n{}\n",
        chrono::Local::now().to_rfc3339(),
        thread.name().unwrap_or("unnamed"),
        Backtrace::force_capture(),
    );
    let _ = file.write_all(entry.as_bytes());
}

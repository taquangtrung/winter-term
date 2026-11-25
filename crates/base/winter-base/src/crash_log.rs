//! The crash log: every panic, caught or not, written with its backtrace to
//! `crash.log` in the state directory, so a crash leaves something to read
//! after the window is gone, and a panic Winter recovered from can still be
//! looked into. The window's watchdog writes here too when the window stops
//! answering. Nothing is written until the program turns the log on
//! ([`enable`]), so a library's tests and tools never write to the user's.

use std::backtrace::Backtrace;
use std::fs::OpenOptions;
use std::io::Write;
use std::panic::PanicHookInfo;
use std::sync::atomic::{AtomicBool, Ordering};

// ========================================================================
// Constants
// ========================================================================

/// The log's name in the state directory.
const FILE_NAME: &str = "crash.log";

/// How large the log grows before the next panic starts it afresh, so a
/// panic repeating for days never fills the disk.
const MAX_BYTES: u64 = 1024 * 1024;

/// Whether the program turned the log on.
static ENABLED: AtomicBool = AtomicBool::new(false);

// ========================================================================
// Functions
// ========================================================================

/// Turns the log on for the rest of the process: the program calls it as it
/// starts, and nothing else does.
pub fn enable() {
    ENABLED.store(true, Ordering::Relaxed);
}

/// Where the log is written, when there is a state directory to put it in.
pub fn path() -> Option<std::path::PathBuf> {
    crate::paths::state_file(FILE_NAME)
}

/// Append the panic `info` describes to the log, with a backtrace. Never
/// panics itself, since a panic inside the panic hook aborts the process:
/// whatever goes wrong writing the log is let go.
pub fn append(info: &PanicHookInfo) {
    let thread = std::thread::current();
    let heading = format!("on thread {}", thread.name().unwrap_or("unnamed"));
    append_entry(&heading, &format!("{info}\n{}", Backtrace::force_capture()));
}

/// Append an entry headed by the time and `heading`, `body` under it: a
/// panic, or the window found stuck. Never panics; whatever goes wrong
/// writing the log is let go.
pub fn append_entry(heading: &str, body: &str) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }
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
    let entry = format!(
        "--- {} {heading}\n{body}\n",
        chrono::Local::now().to_rfc3339(),
    );
    let _ = file.write_all(entry.as_bytes());
}

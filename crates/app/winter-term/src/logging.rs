//! Where the libraries' log messages go: standard error, each line headed
//! by the crate that said it (`misty: text not drawn: …`), for warnings and
//! errors only. misty reports through the `log` crate (a GPU fallback, text
//! or a picture not drawn, a callback that panicked), and without a logger
//! those would be dropped.
//!
//! While profiling is on ([`crate::diagnose::set_profiling`]), each line also
//! goes to `winter.log` in the state directory with the time it was said,
//! since a window started from the desktop has no standard error anyone
//! reads. The
//! file starts afresh past [`MAX_FILE_BYTES`], the one before kept as
//! `winter.log.old`, so it never fills the disk.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use log::{Level, LevelFilter, Log, Metadata, Record};

// ========================================================================
// Constants
// ========================================================================

/// The quietest messages written: warnings and errors.
const MAX_LEVEL: LevelFilter = LevelFilter::Warn;

/// The log file's name in the state directory.
const FILE_NAME: &str = "winter.log";

/// What a full log file is renamed to as the next one starts.
const OLD_SUFFIX: &str = "old";

/// How large the log file grows before it starts afresh.
const MAX_FILE_BYTES: u64 = 1024 * 1024;

/// The one logger, installed by [`init`].
static LOGGER: StderrLogger = StderrLogger;

/// The log file, once the program turns it on.
static FILE: Mutex<Option<LogFile>> = Mutex::new(None);

// ========================================================================
// Data Structures
// ========================================================================

/// Writes each message to standard error, headed by its crate, and to the
/// log file once there is one.
struct StderrLogger;

/// A log file being written, and how much it holds.
struct LogFile {
    /// The file, open to append.
    file: File,
    /// Where it is.
    path: PathBuf,
    /// How large it grows before starting afresh.
    max_bytes: u64,
    /// How many bytes it holds.
    written: u64,
}

// ========================================================================
// StderrLogger Implementations
// ========================================================================

impl Log for StderrLogger {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.level() <= MAX_LEVEL
    }

    fn log(&self, record: &Record<'_>) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let line = format_line(record.target(), record.level(), &record.args().to_string());
        // Nowhere to report a failed write to standard error.
        let _ = writeln!(std::io::stderr(), "{line}");
        if let Ok(mut file) = FILE.lock()
            && let Some(file) = file.as_mut()
        {
            file.write_line(&format!("{} {line}", chrono::Local::now().to_rfc3339()));
        }
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
    }
}

// ========================================================================
// LogFile Implementations
// ========================================================================

impl LogFile {
    /// The log file at `path`, open to append, starting afresh past
    /// `max_bytes`; `None` when it cannot be opened.
    fn open(path: &Path, max_bytes: u64) -> Option<Self> {
        if let Some(dir) = path.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .ok()?;
        let written = file.metadata().map_or(0, |meta| meta.len());
        Some(Self {
            file,
            path: path.to_path_buf(),
            max_bytes,
            written,
        })
    }

    /// Append `line`, first moving a full file aside to start afresh.
    fn write_line(&mut self, line: &str) {
        if self.written >= self.max_bytes {
            let old = self.path.with_extension(format!("log.{OLD_SUFFIX}"));
            let _ = std::fs::rename(&self.path, old);
            let fresh = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path);
            let Ok(file) = fresh else {
                return;
            };
            self.file = file;
            self.written = 0;
        }
        if writeln!(self.file, "{line}").is_ok() {
            self.written += line.len() as u64 + 1;
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Installs the logger, once; a second call keeps the first.
pub fn init() {
    if log::set_logger(&LOGGER).is_ok() {
        log::set_max_level(MAX_LEVEL);
    }
}

/// Open the log file, or close it. Only turning profiling on or off calls
/// it, so tests never write to the user's.
pub(crate) fn set_file(on: bool) {
    let Ok(mut file) = FILE.lock() else {
        return;
    };
    *file = match on {
        true => log_path().and_then(|path| LogFile::open(&path, MAX_FILE_BYTES)),
        false => None,
    };
}

/// Where the log file is written, when there is a state directory for it.
pub fn log_path() -> Option<PathBuf> {
    winter_base::paths::state_file(FILE_NAME)
}

/// The line written for a message of `level` from `target` (a module path,
/// `misty::render::gpu`): its crate, then the message, errors marked so.
fn format_line(target: &str, level: Level, message: &str) -> String {
    let source = target.split("::").next().unwrap_or(target);
    match level {
        Level::Error => format!("{source}: error: {message}"),
        _ => format!("{source}: {message}"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_line_is_headed_by_its_crate_and_an_error_is_marked() {
        assert_eq!(
            format_line("misty::render::gpu", Level::Warn, "trying OpenGL"),
            "misty: trying OpenGL"
        );
        assert_eq!(
            format_line("misty::app", Level::Error, "the app panicked"),
            "misty: error: the app panicked"
        );
        assert_eq!(format_line("winter_app", Level::Warn, "x"), "winter_app: x");
    }

    #[test]
    fn test_a_full_log_file_moves_aside_and_the_next_starts_afresh() {
        let dir = std::env::temp_dir().join(format!("winter-log-test-{}", std::process::id()));
        let path = dir.join(FILE_NAME);
        let _ = std::fs::remove_dir_all(&dir);
        let mut file = LogFile::open(&path, 10).expect("opened");
        file.write_line("first line");
        file.write_line("second");
        let old = std::fs::read_to_string(dir.join("winter.log.old")).expect("kept");
        let fresh = std::fs::read_to_string(&path).expect("written");
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(old, "first line\n");
        assert_eq!(fresh, "second\n");
    }

    #[test]
    fn test_only_warnings_and_errors_are_written() {
        let quiet = Metadata::builder().level(Level::Info).build();
        let loud = Metadata::builder().level(Level::Warn).build();
        assert!(!LOGGER.enabled(&quiet));
        assert!(LOGGER.enabled(&loud));
    }
}

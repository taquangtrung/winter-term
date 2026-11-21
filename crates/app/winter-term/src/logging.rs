//! Where the libraries' log messages go: standard error, each line headed
//! by the crate that said it (`misty: text not drawn: …`), for warnings and
//! errors only. misty reports through the `log` crate (a GPU fallback, text
//! or a picture not drawn, a callback that panicked), and without a logger
//! those would be dropped.

use std::io::Write;

use log::{Level, LevelFilter, Log, Metadata, Record};

// ========================================================================
// Constants
// ========================================================================

/// The quietest messages written: warnings and errors.
const MAX_LEVEL: LevelFilter = LevelFilter::Warn;

/// The one logger, installed by [`init`].
static LOGGER: StderrLogger = StderrLogger;

// ========================================================================
// Data Structures
// ========================================================================

/// Writes each message to standard error, headed by its crate.
struct StderrLogger;

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
    }

    fn flush(&self) {
        let _ = std::io::stderr().flush();
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
    fn test_only_warnings_and_errors_are_written() {
        let quiet = Metadata::builder().level(Level::Info).build();
        let loud = Metadata::builder().level(Level::Warn).build();
        assert!(!LOGGER.enabled(&quiet));
        assert!(LOGGER.enabled(&loud));
    }
}

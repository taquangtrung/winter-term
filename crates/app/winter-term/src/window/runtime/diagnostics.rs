//! The `Profiling:` commands: profiling turned on and off, and the account a
//! window gives of itself (see [`crate::diagnose`]) written to a file under
//! the state directory and opened in a tab. The account is gathered on a
//! thread of its own, since `gdb` walking the stacks takes a few seconds
//! and stops the whole window while it does; the window opens the file
//! once that thread wakes it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, TryRecvError};
use std::thread;

use winter_model::editor::OpenTarget;

use crate::diagnose::{self, ReportFiles};
use crate::window::App;
use crate::window::notice::WindowNotice;

// ========================================================================
// Constants
// ========================================================================

/// The folder in the state directory the written accounts go to.
const REPORTS_DIR: &str = "reports";

/// The thread gathering an account, as `/proc` and a debugger show it.
const THREAD_NAME: &str = "winter-diagnose";

// ========================================================================
// Data Structures
// ========================================================================

/// What a `Profiling:` command writes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Capture {
    /// The report for an issue: versions, system, GPU, programs, slow turns,
    /// stacks, settings, and the logs' last lines.
    Report,
    /// The slow turns and every thread's stack.
    Stacks,
}

/// What gathering an account came to: the file written, or why none was.
pub(in crate::window) type Gathered = Result<PathBuf, String>;

// ========================================================================
// Capture Implementations
// ========================================================================

impl Capture {
    /// What its file's name starts with.
    fn prefix(self) -> &'static str {
        match self {
            Capture::Report => "winter-report",
            Capture::Stacks => "winter-stacks",
        }
    }
}

// ========================================================================
// App: profiling
// ========================================================================

impl App {
    /// Turn profiling on, or off when it is on, and say which.
    pub(in crate::window) fn toggle_profiling(&mut self) {
        let on = !diagnose::profiling();
        diagnose::set_profiling(on);
        self.notify(WindowNotice::Profiling(on));
    }

    /// Gather the window's account of itself as `capture` says, on a thread
    /// of its own, to open once written ([`Self::collect_capture`]).
    pub(in crate::window) fn start_capture(&mut self, capture: Capture) {
        if self.runtime.capture.is_some() {
            self.notify(WindowNotice::AlreadyGathering);
            return;
        }
        let Some(dir) = crate::config::state_file(REPORTS_DIR) else {
            self.notify(WindowNotice::NoReportDir);
            return;
        };
        let files = ReportFiles {
            crash_log: winter_base::crash_log::path(),
            log: crate::logging::log_path(),
            settings: crate::config::config_dir().join("settings.kdl"),
        };
        let (tx, rx) = mpsc::channel();
        let waker = self.runtime.waker.clone();
        let spawned = thread::Builder::new()
            .name(THREAD_NAME.to_string())
            .spawn(move || {
                let _ = tx.send(write_capture(capture, &dir, &files));
                waker.wake();
            });
        match spawned {
            Ok(_) => {
                self.runtime.capture = Some(rx);
                self.notify(WindowNotice::Gathering);
            }
            Err(error) => self.notify(WindowNotice::GatherFailed(error.to_string())),
        }
    }

    /// Open the account gathered, once its thread has written it: called
    /// each turn, and costs nothing while none is gathered.
    pub(in crate::window) fn collect_capture(&mut self) {
        let Some(rx) = &self.runtime.capture else {
            return;
        };
        let gathered = match rx.try_recv() {
            Ok(gathered) => gathered,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => Err("the thread reading it stopped".to_string()),
        };
        self.runtime.capture = None;
        match gathered {
            Ok(path) => self.open_path_editor(OpenTarget {
                column: None,
                line: None,
                path,
            }),
            Err(why) => self.notify(WindowNotice::WriteFailed(why)),
        }
        self.dirty = true;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Gather the account `capture` asks for and write it to a new file in
/// `dir`, the report carrying `files`. Blocks while `gdb` walks the stacks.
fn write_capture(capture: Capture, dir: &Path, files: &ReportFiles) -> Gathered {
    let diagnosis = diagnose::take();
    let text = match capture {
        Capture::Report => diagnose::report(Ok(diagnosis), files),
        Capture::Stacks => diagnose::stacks_text(&diagnosis),
    };
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let path = dir.join(diagnose::stamped_name(capture.prefix()));
    std::fs::write(&path, text).map_err(|error| error.to_string())?;
    Ok(path)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::test_support::app_with_tabs;

    #[test]
    fn test_an_account_written_opens_in_a_tab_and_a_failure_says_why() {
        let mut app = app_with_tabs(1);
        let before = app.layout().members().len();
        let file =
            std::env::temp_dir().join(format!("winter-stacks-test-{}.txt", std::process::id()));
        std::fs::write(&file, "Threads:\n").expect("written");
        let (tx, rx) = mpsc::channel();
        app.runtime.capture = Some(rx);
        app.collect_capture();
        assert!(app.runtime.capture.is_some(), "nothing yet: still waiting");
        tx.send(Ok(file.clone())).expect("sent");
        app.collect_capture();
        let _ = std::fs::remove_file(&file);
        assert!(app.runtime.capture.is_none());
        assert_eq!(app.layout().members().len(), before + 1, "opened in a tab");

        let (tx, rx) = mpsc::channel::<Gathered>();
        app.runtime.capture = Some(rx);
        drop(tx);
        app.collect_capture();
        assert!(app.runtime.capture.is_none(), "a thread gone ends the wait");
    }
}

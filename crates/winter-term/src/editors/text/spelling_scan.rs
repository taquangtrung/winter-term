//! The spelling back-end's project half: the prose files of a project that
//! are not open, read from disk and checked, so a project's writing issues
//! take in more than the open buffers [`super::spelling`] checks as they
//! are edited.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use winter_lsp::diagnostics::{self, Backend};
use crate::editors::background::Background;
use crate::editors::spell;
use winter_lsp::protocol::Diagnostic;
use crate::project;

use super::spelling;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// How long a check of the project's prose files is taken as current
/// before it is made again: long enough to step through the issues without
/// reading every file at each step.
const FRESH_FOR: Duration = Duration::from_secs(10);

/// The most prose files read, past which a tree is too big to check at a
/// press.
const MAX_FILES: usize = 2000;

/// The largest prose file read.
const MAX_BYTES: u64 = 2 * 1024 * 1024;

/// The directories never looked in for prose: a build's output and fetched
/// packages.
const SKIPPED_DIRS: &[&str] = &[
    "target",
    "node_modules",
    "build",
    "dist",
    "out",
    "_build",
    "venv",
];

/// How often a scan running in the background is looked in on.
const SCAN_POLL: Duration = Duration::from_millis(100);

/// What the scanning thread is called.
const SCAN_THREAD: &str = "winter prose scan";

/// The prose kinds checked on disk.
const PROSE_EXTENSIONS: &[&str] = &["md", "markdown", "tex", "bib", "txt"];

// ========================================================================
// Data Structures
// ========================================================================

/// When the project's prose files were last checked, and the project.
#[derive(Debug, Default)]
pub(super) struct ProseScan {
    at: Option<Instant>,
    /// The dictionary's generation the files were checked at.
    generation: u64,
    root: Option<PathBuf>,
    /// A check still reading files off the window's thread, and the project
    /// it is reading: thousands of files read and checked on the window's
    /// thread froze it.
    running: Option<(PathBuf, Background<Vec<Checked>>)>,
}

/// One prose file checked, and what was found in it.
type Checked = (PathBuf, Vec<Diagnostic>);

// ========================================================================
// TextEditor: the project's prose
// ========================================================================

impl TextEditor {
    /// Check the spelling of the prose files under `root` that are not
    /// open, and publish what is found, unless they were just checked.
    pub(super) fn scan_prose(&mut self, root: &Path) {
        // A word taken as spelled right since makes the last check stale.
        let fresh = self.prose_scan.at.is_some_and(|at| at.elapsed() < FRESH_FOR)
            && self.prose_scan.generation == spell::generation();
        if fresh && self.prose_scan.root.as_deref() == Some(root) {
            return;
        }
        if self.prose_scan.running.as_ref().is_some_and(|(running, _)| running == root) {
            return;
        }
        // An open file's spelling is its buffer's, checked as it is edited,
        // or not at all where checking was turned off.
        let open: Vec<PathBuf> = self.docs.iter().map(|doc| doc.path.clone()).collect();
        let scanned = root.to_path_buf();
        let work = Background::start(SCAN_THREAD, move |cancel| check_prose(&scanned, &open, cancel));
        self.prose_scan.running = Some((root.to_path_buf(), work));
    }

    /// When a check running in the background is next looked in on.
    pub(super) fn prose_scan_next_tick(&self) -> Option<Instant> {
        self.prose_scan.running.as_ref().map(|_| Instant::now() + SCAN_POLL)
    }

    /// Publish what a check running in the background found, once it has.
    pub(super) fn take_prose_scan(&mut self) {
        let Some((root, work)) = self.prose_scan.running.take() else {
            return;
        };
        let Some(checked) = work.poll() else {
            self.prose_scan.running = Some((root, work));
            return;
        };
        for (path, found) in checked {
            diagnostics::publish(Backend::Spelling, &path, found);
        }
        diagnostics::forget_gone(Backend::Spelling, &root);
        self.prose_scan = ProseScan {
            at: Some(Instant::now()),
            generation: spell::generation(),
            root: Some(root),
            running: None,
        };
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Check the prose files under `root` other than the `open` ones, stopping
/// early when `cancel` is set.
fn check_prose(root: &Path, open: &[PathBuf], cancel: &AtomicBool) -> Vec<Checked> {
    let mut checked = Vec::new();
    for path in prose_files(root) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if open.contains(&path) {
            continue;
        }
        let (Some(prose), Ok(text)) = (spelling::prose_of(&path), std::fs::read_to_string(&path)) else {
            continue;
        };
        let lines: Vec<String> = text.lines().map(str::to_string).collect();
        let project = path.parent().and_then(project::marked_root);
        let found = spelling::find(&lines, prose, project.as_deref());
        checked.push((path, spelling::as_diagnostics(&found, &lines)));
    }
    checked
}

/// The prose files under `root`, hidden directories and build output left
/// out, in path order.
fn prose_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut dirs = vec![root.to_path_buf()];
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_str()) {
                    dirs.push(path);
                }
                continue;
            }
            let prose = path
                .extension()
                .and_then(|extension| extension.to_str())
                .is_some_and(|extension| {
                    PROSE_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
                });
            let small = entry.metadata().is_ok_and(|meta| meta.len() <= MAX_BYTES);
            if prose && small {
                files.push(path);
                if files.len() >= MAX_FILES {
                    files.sort();
                    return files;
                }
            }
        }
    }
    files.sort();
    files
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_project_scan_runs_off_the_window_thread_and_lands_on_a_tick() {
        // Reading and checking every prose file in the tree on the window's
        // thread froze it until the last was done.
        let dir = std::env::temp_dir().join(format!("winter-prose-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let other = dir.join("other.md");
        std::fs::write(&other, "Thiss sentense is mispelled.\n").expect("prose");
        let open = dir.join("open.md");
        std::fs::write(&open, "Fine.\n").expect("prose");
        let mut editor = TextEditor::new(open, None).expect("opens");
        editor.scan_prose(&dir);
        assert!(editor.prose_scan_next_tick().is_some(), "still running");
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.prose_scan.running.is_some() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            editor.take_prose_scan();
        }
        assert!(!diagnostics::of(Backend::Spelling, &other).is_empty(), "the other file was checked");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_prose_files_skip_hidden_and_build_directories() {
        let dir = std::env::temp_dir().join(format!("winter-problems-{}", std::process::id()));
        for sub in ["docs", ".git", "target"] {
            std::fs::create_dir_all(dir.join(sub)).expect("dirs");
        }
        for file in ["docs/a.md", ".git/b.md", "target/c.md", "d.tex", "e.rs"] {
            std::fs::write(dir.join(file), "x").expect("file");
        }
        let found = prose_files(&dir);
        assert_eq!(found, [dir.join("d.tex"), dir.join("docs/a.md")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

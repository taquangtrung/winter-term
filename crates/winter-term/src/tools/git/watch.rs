//! Noticing that a repository changed under the status view.
//!
//! The working tree is watched through the platform's file notifications
//! rather than read on a timer. Directories that only ever hold build output
//! are not watched at all, since setting up a watch on a tree like
//! `node_modules` is the expensive part, and inside git's own directory only
//! the files a status depends on count. Changes arrive in bursts (a checkout,
//! a build, a save and its backup), so the view is told once things have gone
//! quiet rather than once per event.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};

// ========================================================================
// Constants
// ========================================================================

/// How long the tree must be quiet after a change before the view is told.
const QUIET: Duration = Duration::from_secs(3);

/// How often the view looks for news while nothing is waiting out its quiet
/// time. Looking costs a channel read, not a git command.
const LOOK_EVERY: Duration = Duration::from_millis(500);

/// Directories of build output and dependencies, never watched: their churn
/// says nothing about the repository and they are by far the costliest to
/// watch.
const IGNORED_DIRS: [&str; 14] = [
    "node_modules",
    "dist",
    "dist-electron",
    "out",
    "build",
    "release",
    "target",
    ".venv",
    "__pycache__",
    ".next",
    ".turbo",
    "coverage",
    ".cache",
    "bower_components",
];

/// The files directly in git's directory that a status depends on: staging,
/// the checked-out branch, work in progress, and what a fetch brought.
const GIT_FILES: [&str; 5] = ["index", "HEAD", "ORIG_HEAD", "MERGE_HEAD", "FETCH_HEAD"];

/// Git's own file of packed references, and the directory of loose ones.
const PACKED_REFS: &str = "packed-refs";
const REFS_DIR: &str = "refs";

/// What git calls its directory inside a working tree.
const GIT_DIR: &str = ".git";

/// The suffix of the lock files git writes while it updates something.
const LOCK_SUFFIX: &str = ".lock";

// ========================================================================
// Data Structures
// ========================================================================

/// A running watch over one repository.
pub struct TreeWatch {
    /// When the last event arrived, while its quiet time is still running.
    changed_at: Option<Instant>,
    events: mpsc::Receiver<()>,
    /// When the view should next look for news.
    next_look: Instant,
    root: PathBuf,
    /// Top-level directories already under a watch.
    watched: HashSet<PathBuf>,
    /// Kept alive: dropping it stops the notifications.
    watcher: RecommendedWatcher,
}

impl std::fmt::Debug for TreeWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TreeWatch")
            .field("root", &self.root)
            .finish_non_exhaustive()
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Whether `event` is the file being changed rather than only looked at.
/// Git reading its own files to answer a status raises access events, which
/// would otherwise start another status.
fn is_content_change(kind: &EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        _ => true,
    }
}

/// Whether a change at `path` could alter what the view shows.
fn matters(path: &Path, root: &Path, git_dir: &Path) -> bool {
    if let Ok(inside) = path.strip_prefix(git_dir) {
        return git_change_matters(inside);
    }
    let Ok(inside) = path.strip_prefix(root) else {
        return false;
    };
    // The same directory under a different name is not git's, so only the
    // first component of a path says whether it is build output.
    let top = inside.components().next();
    let Some(top) = top.and_then(|part| part.as_os_str().to_str()) else {
        return true;
    };
    top != GIT_DIR && !IGNORED_DIRS.contains(&top)
}

/// Whether a change at `inside` (relative to git's directory) bears on a
/// status: the handful of files it is made of, and any loose reference.
fn git_change_matters(inside: &Path) -> bool {
    let name = inside.file_name().and_then(|name| name.to_str());
    if name.is_some_and(|name| name.ends_with(LOCK_SUFFIX)) {
        return false;
    }
    if inside.components().count() == 1 {
        return name.is_some_and(|name| GIT_FILES.contains(&name) || name == PACKED_REFS);
    }
    inside.starts_with(REFS_DIR)
}

/// The directories directly under `root` worth a watch of their own.
fn watchable_dirs(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_dir()))
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name != GIT_DIR && !IGNORED_DIRS.contains(&name.as_ref())
        })
        .map(|entry| entry.path())
        .collect()
}

// ========================================================================
// TreeWatch
// ========================================================================

impl TreeWatch {
    /// Start watching the repository at `root`, whose own files are in
    /// `git_dir`. `None` when the platform cannot be watched, which leaves the
    /// view to read on a timer instead.
    pub fn start(root: &Path, git_dir: Option<&Path>) -> Option<Self> {
        let git_dir = git_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.join(GIT_DIR));
        let (tx, events) = mpsc::channel();
        let (watch_root, watch_git) = (root.to_path_buf(), git_dir.clone());
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    return;
                };
                if is_content_change(&event.kind)
                    && event
                        .paths
                        .iter()
                        .any(|path| matters(path, &watch_root, &watch_git))
                {
                    let _ = tx.send(());
                }
            })
            .ok()?;

        // The top level of the tree for files and for directories appearing,
        // each directory under it for what is inside, and git's files.
        watcher.watch(root, RecursiveMode::NonRecursive).ok()?;
        let mut watched = HashSet::new();
        for dir in watchable_dirs(root) {
            if watcher.watch(&dir, RecursiveMode::Recursive).is_ok() {
                watched.insert(dir);
            }
        }
        watcher.watch(&git_dir, RecursiveMode::NonRecursive).ok()?;
        let _ = watcher.watch(&git_dir.join(REFS_DIR), RecursiveMode::Recursive);

        Some(Self {
            changed_at: None,
            events,
            next_look: Instant::now() + LOOK_EVERY,
            root: root.to_path_buf(),
            watched,
            watcher,
        })
    }

    /// When the view should next call [`Self::changed`].
    pub fn next_look(&self) -> Instant {
        self.next_look
    }

    /// Whether the tree has changed and then stayed quiet long enough that the
    /// view should read it again. True once per burst of changes.
    pub fn changed(&mut self, now: Instant) -> bool {
        self.next_look = now + LOOK_EVERY;
        let mut news = false;
        while self.events.try_recv().is_ok() {
            news = true;
        }
        if news {
            self.changed_at = Some(now);
            self.watch_new_dirs();
            return false;
        }
        match self.changed_at {
            Some(at) if now.duration_since(at) >= QUIET => {
                self.changed_at = None;
                true
            }
            _ => false,
        }
    }

    /// Put a watch on a top-level directory made since the last look.
    fn watch_new_dirs(&mut self) {
        for dir in watchable_dirs(&self.root) {
            if !self.watched.contains(&dir)
                && self.watcher.watch(&dir, RecursiveMode::Recursive).is_ok()
            {
                self.watched.insert(dir);
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const ROOT: &str = "/repo";
    const GIT: &str = "/repo/.git";

    fn matters_at(path: &str) -> bool {
        matters(Path::new(path), Path::new(ROOT), Path::new(GIT))
    }

    #[test]
    fn test_source_edits_matter_and_build_output_does_not() {
        assert!(matters_at("/repo/src/main.rs"));
        assert!(matters_at("/repo/README.md"));
        assert!(!matters_at("/repo/target/debug/winter"));
        assert!(!matters_at("/repo/node_modules/a/index.js"));
        assert!(
            matters_at("/repo/src/target/x.rs"),
            "only a top-level directory is build output"
        );
    }

    #[test]
    fn test_inside_git_only_the_files_a_status_reads_matter() {
        assert!(matters_at("/repo/.git/index"));
        assert!(matters_at("/repo/.git/HEAD"));
        assert!(matters_at("/repo/.git/refs/remotes/origin/main"));
        assert!(matters_at("/repo/.git/packed-refs"));
        assert!(!matters_at("/repo/.git/index.lock"));
        assert!(!matters_at("/repo/.git/objects/ab/cdef"));
        assert!(!matters_at("/repo/.git/logs/HEAD"));
        assert!(!matters_at("/repo/.git/refs/heads/main.lock"));
    }

    #[test]
    fn test_a_worktrees_git_directory_outside_the_tree_still_counts() {
        let inside = matters(
            Path::new("/main/.git/worktrees/w/index"),
            Path::new("/work/w"),
            Path::new("/main/.git/worktrees/w"),
        );
        assert!(inside);
    }

    #[test]
    fn test_reading_a_file_is_not_a_change() {
        assert!(!is_content_change(&EventKind::Access(AccessKind::Open(
            AccessMode::Any
        ))));
        assert!(is_content_change(&EventKind::Access(AccessKind::Close(
            AccessMode::Write
        ))));
        assert!(is_content_change(&EventKind::Create(
            notify::event::CreateKind::File
        )));
    }

    #[test]
    fn test_a_burst_of_writes_reports_once_after_it_goes_quiet() {
        let dir = std::env::temp_dir().join(format!(
            "winter-treewatch-test-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).expect("git dir");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        let mut watch = TreeWatch::start(&dir, None).expect("watch starts");
        // Let the platform start observing before the first write.
        std::thread::sleep(Duration::from_millis(150));

        std::fs::write(dir.join("src").join("a.rs"), "fn a() {}").expect("write");
        std::thread::sleep(Duration::from_millis(300));

        let start = Instant::now();
        assert!(!watch.changed(start), "the quiet time has only begun");
        assert!(!watch.changed(start + QUIET / 2), "still inside it");
        assert!(watch.changed(start + QUIET), "quiet long enough");
        assert!(!watch.changed(start + QUIET * 2), "reported once");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

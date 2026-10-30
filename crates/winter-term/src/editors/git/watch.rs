//! Noticing that a repository changed under the status view.
//!
//! The working tree is watched through the platform's file notifications
//! rather than read on a timer. Directories that only ever hold build output
//! are not watched at all, since setting up a watch on a tree like
//! `node_modules` is the expensive part, and inside git's own directory only
//! the files a status depends on count.
//!
//! Changes arrive in bursts (a checkout, a build, a save and its backup), and
//! a read per event would run git hundreds of times for one checkout. So a
//! burst is read twice at most: once just after it starts, which shows a save
//! or a commit at once, and once after it has gone quiet, which catches what
//! the first read was too early for. A burst that never goes quiet (a build,
//! a server writing a log) is read every so often all the same, and nothing is
//! read while git holds its index lock, half-way through writing. A change
//! inside git's own directory (a commit, a checkout, a fetch) asks for the
//! whole view to be read again; one in the working tree only for its status.
//! The watcher's thread wakes the window when there is news, so a quiet
//! repository costs nothing at all.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::editors::background::Background;
use crate::wake::wake_window;
use crate::watch::{is_content_change, Debounce, Pace};

// ========================================================================
// Constants
// ========================================================================

/// How a repository waits a burst out: read soon after it starts, so git or
/// an editor has finished the write the change is part of, and again once
/// the tree has been quiet for longer than a build pauses between files.
const PACE: Pace = Pace {
    lead: Duration::from_millis(200),
    max_wait: Duration::from_secs(5),
    quiet: Duration::from_millis(750),
};

/// How often a read waiting on git's index lock looks again.
const LOCK_RECHECK: Duration = Duration::from_millis(100);

/// The lock git holds on its index while it writes it.
const INDEX_LOCK: &str = "index.lock";

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

/// The thread a watch is set up on.
const WATCH_THREAD: &str = "winter git watch";

// ========================================================================
// Data Structures
// ========================================================================

/// What a change asks to be read again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Read {
    /// The whole view: a commit, a checkout, a fetch, a stash changed what
    /// git holds, not only the files.
    Full,
    /// Only the working tree's status: files changed, nothing git holds did.
    Status,
}

/// The changes being waited out, and whether any was inside git's
/// directory.
#[derive(Clone, Copy, Debug)]
struct Changes {
    debounce: Debounce,
    /// Whether a change waiting to be read was inside git's directory.
    git: bool,
}

/// A running watch over one repository.
pub struct TreeWatch {
    /// The changes being waited out, if any.
    changes: Changes,
    /// One per change, saying whether it was inside git's directory.
    events: mpsc::Receiver<bool>,
    git_dir: PathBuf,
    /// Set by the watcher's thread when it sends a change, so the view asks
    /// to look without draining the channel to find out.
    news: Arc<AtomicBool>,
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
// Changes
// ========================================================================

impl Changes {
    /// Nothing waited on yet.
    fn new() -> Self {
        Self {
            debounce: Debounce::new(PACE),
            git: false,
        }
    }

    /// Take in a change that arrived at `now`, inside git's directory when
    /// `in_git`.
    fn take_change(&mut self, now: Instant, in_git: bool) {
        self.debounce.take_change(now);
        self.git |= in_git;
    }

    /// What to read at `now`, if anything, noting it read. Nothing is read
    /// while git holds its lock (`locked`), unless the burst has waited as
    /// long as it may: a lock left behind by a crash must not freeze the
    /// view.
    fn decide(&mut self, now: Instant, locked: bool) -> Option<Read> {
        if !self.debounce.is_due(now) || (locked && !self.debounce.is_overdue(now)) {
            return None;
        }
        self.debounce.mark_read(now);
        let read = match self.git {
            true => Read::Full,
            false => Read::Status,
        };
        self.git = false;
        Some(read)
    }
}

// ========================================================================
// TreeWatch
// ========================================================================

impl TreeWatch {
    /// Start watching the repository at `root`, whose own files are in
    /// `git_dir`. `None` when the platform cannot be watched, which leaves the
    /// view read again only when asked.
    pub fn start(root: &Path, git_dir: Option<&Path>) -> Option<Self> {
        let git_dir = git_dir
            .map(Path::to_path_buf)
            .unwrap_or_else(|| root.join(GIT_DIR));
        let (tx, events) = mpsc::channel();
        let news = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&news);
        let (watch_root, watch_git) = (root.to_path_buf(), git_dir.clone());
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    return;
                };
                if !is_content_change(&event.kind) {
                    return;
                }
                let mattering = event
                    .paths
                    .iter()
                    .filter(|path| matters(path, &watch_root, &watch_git));
                let mut any = false;
                let mut in_git = false;
                for path in mattering {
                    any = true;
                    in_git |= path.starts_with(&watch_git);
                }
                if any && tx.send(in_git).is_ok() {
                    flag.store(true, Ordering::Release);
                    wake_window();
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
            changes: Changes::new(),
            events,
            git_dir,
            news,
            root: root.to_path_buf(),
            watched,
            watcher,
        })
    }

    /// [`Self::start`] on a thread of its own. Adding a recursive watch walks
    /// every directory under it, which in a large tree (a monorepo, a
    /// repository at `$HOME`) takes seconds the window must not spend
    /// frozen. The thread wakes the window once it has answered.
    pub fn start_in_background(root: &Path, git_dir: Option<&Path>) -> Background<Option<TreeWatch>> {
        let (root, git_dir) = (root.to_path_buf(), git_dir.map(Path::to_path_buf));
        Background::start(WATCH_THREAD, move |_| Self::start(&root, git_dir.as_deref()))
    }

    /// When the view should next call [`Self::changed`]: at once when the
    /// watcher has news, when a burst's next read falls due, and never while
    /// nothing has changed, since the watcher wakes the window itself.
    pub fn next_look(&self, now: Instant) -> Option<Instant> {
        if self.news.load(Ordering::Acquire) {
            return Some(now);
        }
        let due = self.changes.debounce.due_at()?;
        if self.locked() && !self.changes.debounce.is_overdue(now) {
            return Some(now + LOCK_RECHECK);
        }
        Some(due)
    }

    /// What should be read again now, if anything: the changes that arrived
    /// since the last call are taken in, and a read falls due as the burst
    /// they belong to says (see the module's own documentation).
    pub fn changed(&mut self, now: Instant) -> Option<Read> {
        // Cleared before the channel is read, so a change sent while it is
        // being read leaves the flag set for the next look.
        self.news.store(false, Ordering::Release);
        let mut arrived = false;
        while let Ok(in_git) = self.events.try_recv() {
            arrived = true;
            self.changes.take_change(now, in_git);
        }
        if arrived {
            self.watch_new_dirs();
        }
        let locked = self.locked();
        self.changes.decide(now, locked)
    }

    /// Whether git is half-way through writing its index.
    fn locked(&self) -> bool {
        self.git_dir.join(INDEX_LOCK).exists()
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

    const LEAD: Duration = PACE.lead;
    const MAX_WAIT: Duration = PACE.max_wait;
    const QUIET: Duration = PACE.quiet;

    /// A burst opened at `start` by a change inside git when `git`.
    fn burst_at(start: Instant, git: bool) -> Changes {
        let mut changes = Changes::new();
        changes.take_change(start, git);
        changes
    }

    #[test]
    fn test_a_change_is_read_just_after_it_starts_not_seconds_later() {
        let start = Instant::now();
        let mut burst = burst_at(start, false);
        assert_eq!(
            burst.decide(start, false),
            None,
            "the write may not be done"
        );
        assert_eq!(burst.decide(start + LEAD, false), Some(Read::Status));
        assert_eq!(
            burst.decide(start + LEAD * 2, false),
            None,
            "read once"
        );
    }

    #[test]
    fn test_changes_after_the_first_read_are_read_once_they_go_quiet() {
        let start = Instant::now();
        let mut burst = burst_at(start, false);
        burst.decide(start + LEAD, false);
        let later = start + LEAD + Duration::from_millis(100);
        burst.take_change(later, true);
        assert_eq!(
            burst.decide(later + QUIET / 2, false),
            None,
            "still busy"
        );
        assert_eq!(
            burst.decide(later + QUIET, false),
            Some(Read::Full),
            "a change inside git asks for everything"
        );
    }

    #[test]
    fn test_a_burst_that_never_goes_quiet_is_still_read_every_so_often() {
        let start = Instant::now();
        let mut burst = burst_at(start, false);
        burst.decide(start + LEAD, false);
        let mut now = start + LEAD;
        let mut reads = 0;
        while now < start + LEAD + MAX_WAIT * 2 {
            now += QUIET / 3;
            burst.take_change(now, false);
            if burst.decide(now, false).is_some() {
                reads += 1;
            }
        }
        assert_eq!(reads, 2, "once per MAX_WAIT, however busy");
    }

    #[test]
    fn test_nothing_is_read_while_git_holds_its_lock_unless_it_is_left_behind() {
        let start = Instant::now();
        let mut burst = burst_at(start, true);
        assert_eq!(burst.decide(start + LEAD, true), None, "mid-write");
        assert_eq!(burst.decide(start + LEAD, false), Some(Read::Full));

        let mut stale = burst_at(start, true);
        assert_eq!(
            stale.decide(start + MAX_WAIT, true),
            Some(Read::Full),
            "a lock a crash left must not freeze the view"
        );
    }

    #[test]
    fn test_a_change_long_after_the_last_burst_starts_a_new_one() {
        let start = Instant::now();
        let mut burst = burst_at(start, false);
        burst.decide(start + LEAD, false);
        let much_later = start + Duration::from_secs(60);
        burst.take_change(much_later, false);
        assert_eq!(
            burst.decide(much_later + LEAD, false),
            Some(Read::Status),
            "read at once again, not after the quiet time"
        );
    }

    #[test]
    fn test_a_write_on_disk_reaches_the_view_as_news() {
        let dir = std::env::temp_dir().join(format!(
            "winter-treewatch-test-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).expect("git dir");
        std::fs::create_dir_all(dir.join("src")).expect("src");
        let mut watch = TreeWatch::start(&dir, None).expect("watch starts");
        let idle = Instant::now();
        assert_eq!(watch.next_look(idle), None, "a quiet tree asks for nothing");
        // Let the platform start observing before the first write.
        std::thread::sleep(Duration::from_millis(150));

        std::fs::write(dir.join("src").join("a.rs"), "fn a() {}").expect("write");
        std::thread::sleep(Duration::from_millis(300));

        let now = Instant::now();
        assert_eq!(watch.next_look(now), Some(now), "news: look at once");
        assert_eq!(watch.changed(now), None, "too soon after the write");
        assert_eq!(watch.changed(now + LEAD), Some(Read::Status));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

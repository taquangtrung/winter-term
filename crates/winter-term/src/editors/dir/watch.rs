//! Watching the filesystem for changes under a directory listing.
//!
//! A directory listing is kept up to date through file notifications rather
//! than polling. The listing's root and each expanded directory are watched
//! non-recursively: unexpanded directories show only their own entry, so what
//! changes inside them does not alter the listing.
//!
//! Changes arrive in bursts (e.g. an editor writing a file and its temporary
//! backup, or a build producing multiple files). A burst is read after a
//! brief lead time to catch quick saves, and again once it goes quiet.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::wake::wake_window;
use crate::watch::{is_content_change, Debounce, Pace};

use super::tree::Folds;

// ========================================================================
// Constants
// ========================================================================

/// How a listing waits a burst out: read soon after it starts, so an editor
/// or an atomic write has finished, and again once the tree is quiet.
const PACE: Pace = Pace {
    lead: Duration::from_millis(150),
    max_wait: Duration::from_secs(2),
    quiet: Duration::from_millis(300),
};

// ========================================================================
// Data Structures
// ========================================================================

/// A running filesystem watcher over a directory and its expanded subdirectories.
pub struct DirWatch {
    /// The changes being waited out, if any.
    changes: Debounce,
    events: mpsc::Receiver<()>,
    news: Arc<AtomicBool>,
    root: PathBuf,
    watched: HashSet<PathBuf>,
    watcher: RecommendedWatcher,
}

impl std::fmt::Debug for DirWatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DirWatch")
            .field("root", &self.root)
            .field("watched", &self.watched)
            .finish_non_exhaustive()
    }
}

// ========================================================================
// DirWatch
// ========================================================================

impl DirWatch {
    /// Start watching `root` and any directories currently expanded in `folds`.
    /// Returns `None` if `root` is remote or if the filesystem watcher cannot
    /// start on this platform.
    pub fn start(root: &Path, folds: &Folds) -> Option<Self> {
        if crate::remote::parse(root).is_some() {
            return None;
        }
        let (tx, events) = mpsc::channel();
        let news = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&news);
        let mut watcher =
            notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
                let Ok(event) = result else {
                    return;
                };
                if !is_content_change(&event.kind) {
                    return;
                }
                if tx.send(()).is_ok() {
                    flag.store(true, Ordering::Release);
                    wake_window();
                }
            })
            .ok()?;

        let mut watched = HashSet::new();
        if watcher.watch(root, RecursiveMode::NonRecursive).is_ok() {
            watched.insert(root.to_path_buf());
        } else {
            return None;
        }

        for path in folds.expanded() {
            if path.starts_with(root) && watcher.watch(path, RecursiveMode::NonRecursive).is_ok() {
                watched.insert(path.clone());
            }
        }

        Some(Self {
            changes: Debounce::new(PACE),
            events,
            news,
            root: root.to_path_buf(),
            watched,
            watcher,
        })
    }

    /// When the view should next call [`Self::changed`]: at once when the
    /// watcher has news, when a burst's next read falls due, and never while
    /// nothing has changed, since the watcher wakes the window itself.
    pub fn next_look(&self, now: Instant) -> Option<Instant> {
        if self.news.load(Ordering::Acquire) {
            return Some(now);
        }
        self.changes.due_at()
    }

    /// Whether the directory listing should be reloaded at `now`: drains new
    /// changes from the channel and decides if a read falls due.
    pub fn changed(&mut self, now: Instant) -> bool {
        self.news.store(false, Ordering::Release);
        while let Ok(()) = self.events.try_recv() {
            self.changes.take_change(now);
        }
        self.changes.take_due(now)
    }

    /// Mark the change as still waiting to be read, if the editor was not ready
    /// to take it yet (e.g. while editing names in place).
    pub fn defer(&mut self) {
        self.changes.defer();
    }

    /// Synchronize the watched directory set with `root` and the current `folds`.
    pub fn sync_folds(&mut self, folds: &Folds) {
        self.watched.retain(|path| {
            if path == &self.root {
                return true;
            }
            if !folds.is_expanded(path) {
                let _ = self.watcher.unwatch(path);
                false
            } else {
                true
            }
        });
        for path in folds.expanded() {
            if path.starts_with(&self.root)
                && !self.watched.contains(path)
                && self.watcher.watch(path, RecursiveMode::NonRecursive).is_ok()
            {
                self.watched.insert(path.clone());
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

    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("winter-dirwatch-{tag}-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.0).ok();
        }
    }

    #[test]
    fn test_sync_folds_watches_newly_expanded_directories() {
        let tmp = TempTree::new("sync-folds");
        let root = tmp.path();
        let sub_a = root.join("sub_a");
        let sub_b = root.join("sub_b");
        std::fs::create_dir(&sub_a).expect("create sub_a");
        std::fs::create_dir(&sub_b).expect("create sub_b");

        let mut folds = Folds::new();
        folds.expand(&sub_a);

        let mut watch = DirWatch::start(root, &folds).expect("starts watch");
        assert!(watch.watched.contains(root));
        assert!(watch.watched.contains(&sub_a));
        assert!(!watch.watched.contains(&sub_b));

        // Switch expanded folder from sub_a to sub_b
        folds.toggle(&sub_a);
        folds.expand(&sub_b);
        watch.sync_folds(&folds);

        assert!(watch.watched.contains(root));
        assert!(!watch.watched.contains(&sub_a));
        assert!(watch.watched.contains(&sub_b));
    }
}

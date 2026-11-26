//! Directory sizes: each directory on screen walked for its total, one at a
//! time, the walks stopped when sizes are turned off. Totals are kept when the
//! root changes, so a directory stepped back into shows its sizes at once. A
//! total is walked again only when something says it changed (the watch, or a
//! reload), and shows its old value meanwhile.

use std::path::{Path, PathBuf};

use winter_model::editor::{DirTotal, EditorOutcome, JobRequest};

use super::DirEditor;
use super::tree::Row;

// ========================================================================
// Constants
// ========================================================================

/// How many totals are kept before a root change drops those away from it.
pub(super) const MAX_KEPT_SIZES: usize = 10_000;

// ========================================================================
// Data Structures
// ========================================================================

/// A directory's walked total.
#[derive(Clone, Copy, Debug)]
pub(super) struct SizeEntry {
    /// Bytes under the directory.
    pub(super) bytes: u64,
    /// Whether something said it changed since it was walked, so it is to be
    /// walked again.
    pub(super) changed: bool,
}

// ========================================================================
// DirEditor: sizes
// ========================================================================

impl DirEditor {
    /// The walked total for a directory row: `None` for a file, or while its
    /// first walk is still running. A total being walked again shows the old
    /// one until the new one lands.
    pub(super) fn dir_size(&self, row: &Row) -> Option<u64> {
        if !self.show_sizes || !row.entry.is_dir() {
            return None;
        }
        self.sizes.get(&row.entry.path).map(|entry| entry.bytes)
    }

    /// Whether `row` is the directory whose walk is under way, which wears
    /// the spinner.
    pub(super) fn is_walking(&self, row: &Row) -> bool {
        self.sizing.as_ref() == Some(&row.entry.path)
    }

    /// Turn directory sizes on or off. Turning them off asks for no more
    /// walks; the one under way finishes, and its answer asks for nothing.
    /// Cancelling it would cancel every job of the tab with it: a move or an
    /// archive half done, with nothing said.
    pub(super) fn toggle_sizes(&mut self) -> EditorOutcome {
        self.show_sizes = !self.show_sizes;
        if !self.show_sizes {
            self.sizing = None;
            return EditorOutcome::Consumed;
        }
        self.request_next_size()
    }

    /// Ask for the first directory on screen whose size is not known yet, or
    /// failing that the first whose total changed. One at a time, so a
    /// listing of a thousand directories does not start a thousand walks:
    /// each answer asks for the next, and nothing is asked while a walk is
    /// still under way.
    pub(super) fn request_next_size(&mut self) -> EditorOutcome {
        if !self.show_sizes || self.sizing.is_some() {
            return EditorOutcome::Consumed;
        }
        match self.next_unsized() {
            Some(path) => {
                self.sizing = Some(path.clone());
                EditorOutcome::Job(JobRequest::DirSize(path))
            }
            None => EditorOutcome::Consumed,
        }
    }

    /// The directory on screen to walk next: one never walked before one
    /// whose total changed, so what shows nothing fills in first.
    pub(super) fn next_unsized(&self) -> Option<PathBuf> {
        let dirs = || {
            self.entries()
                .filter(|row| row.entry.is_dir())
                .map(|row| &row.entry.path)
        };
        dirs()
            .find(|path| !self.sizes.contains_key(*path))
            .or_else(|| {
                dirs().find(|path| self.sizes.get(*path).is_some_and(|entry| entry.changed))
            })
            .cloned()
    }

    /// Keep the total a walk of `path` found, and those of the directories
    /// under it it summed on its way.
    pub(super) fn record_sizes(&mut self, path: PathBuf, bytes: u64, inner: Vec<DirTotal>) {
        let entry = SizeEntry {
            bytes,
            changed: false,
        };
        self.sizes.insert(path, entry);
        for total in inner {
            let entry = SizeEntry {
                bytes: total.bytes,
                changed: false,
            };
            self.sizes.insert(total.path, entry);
        }
    }

    /// Mark the totals over each of `paths` as changed, those of the
    /// directories holding it at any depth, and ask for walks: the rows keep
    /// their old totals until the new ones land.
    pub(super) fn mark_sizes_changed(&mut self, paths: &[PathBuf]) {
        for path in paths {
            for dir in path.ancestors() {
                if let Some(entry) = self.sizes.get_mut(dir) {
                    entry.changed = true;
                }
            }
        }
        self.sizes_due = self.show_sizes;
    }

    /// Mark the totals of the directories on screen as changed and ask for
    /// walks, for a reload: what changed in a folder shown closed reaches no
    /// watch, so asking again is how its total catches up.
    pub(super) fn mark_listed_sizes_changed(&mut self) {
        let listed: Vec<PathBuf> = self.entries().map(|row| row.entry.path.clone()).collect();
        for path in listed {
            if let Some(entry) = self.sizes.get_mut(&path) {
                entry.changed = true;
            }
        }
        self.sizes_due = self.show_sizes;
    }

    /// Once past [`MAX_KEPT_SIZES`], drop the totals away from `root`: keep
    /// those under it, and the rows of each directory above it, the way back
    /// up.
    pub(super) fn trim_sizes_around(&mut self, root: &Path) {
        if self.sizes.len() <= MAX_KEPT_SIZES {
            return;
        }
        self.sizes.retain(|path, _| {
            path.starts_with(root) || path.parent().is_some_and(|parent| root.starts_with(parent))
        });
    }
}

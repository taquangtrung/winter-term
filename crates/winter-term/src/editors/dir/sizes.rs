//! Directory sizes: each directory on screen walked for its total, one at a
//! time, the walks stopped when sizes are turned off.

use std::path::PathBuf;

use crate::model::editor::{EditorOutcome, JobRequest};

use super::tree::Row;
use super::DirEditor;

// ========================================================================
// DirEditor: sizes
// ========================================================================

impl DirEditor {
    /// The walked total for a directory row: `None` for a file, or while the
    /// walk is still running.
    pub(super) fn dir_size(&self, row: &Row) -> Option<u64> {
        if !self.show_sizes || !row.entry.is_dir() {
            return None;
        }
        self.sizes.get(&row.entry.path).copied()
    }

    /// Turn directory sizes on or off. Turning them off stops the walks still
    /// running, which is the whole reason they are cancellable.
    pub(super) fn toggle_sizes(&mut self) -> EditorOutcome {
        self.show_sizes = !self.show_sizes;
        if !self.show_sizes {
            return EditorOutcome::CancelJobs;
        }
        self.request_next_size()
    }

    /// Ask for the first directory on screen whose size is not known yet. One
    /// at a time, so a listing of a thousand directories does not start a
    /// thousand walks: each answer asks for the next.
    pub(super) fn request_next_size(&self) -> EditorOutcome {
        if !self.show_sizes {
            return EditorOutcome::Consumed;
        }
        match self.next_unsized() {
            Some(path) => EditorOutcome::Job(JobRequest::DirSize(path)),
            None => EditorOutcome::Consumed,
        }
    }

    pub(super) fn next_unsized(&self) -> Option<PathBuf> {
        self.rows
            .iter()
            .filter(|row| row.entry.is_dir())
            .map(|row| row.entry.path.clone())
            .find(|path| !self.sizes.contains_key(path))
    }
}

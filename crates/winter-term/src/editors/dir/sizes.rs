//! Directory sizes: each directory on screen walked for its total, one at a
//! time, the walks stopped when sizes are turned off.

use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::model::editor::{EditorOutcome, JobRequest};
use crate::model::spinner;

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

    /// The spinner frame for a directory row whose walk is under way at `now`,
    /// and `None` for every other row.
    pub(super) fn size_spinner(&self, row: &Row, now: SystemTime) -> Option<char> {
        if self.sizing.as_ref() != Some(&row.entry.path) {
            return None;
        }
        let elapsed = now.duration_since(UNIX_EPOCH).unwrap_or_default();
        Some(spinner::glyph(spinner::frame_at(elapsed)))
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

    /// Ask for the first directory on screen whose size is not known yet. One
    /// at a time, so a listing of a thousand directories does not start a
    /// thousand walks: each answer asks for the next, and nothing is asked
    /// while a walk is still under way.
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

    pub(super) fn next_unsized(&self) -> Option<PathBuf> {
        self.rows
            .iter()
            .filter(|row| row.entry.is_dir())
            .map(|row| row.entry.path.clone())
            .find(|path| !self.sizes.contains_key(path))
    }
}

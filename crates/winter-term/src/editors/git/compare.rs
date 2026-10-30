//! A comparison of two files that need not be in any repository's history: a
//! buffer against the file it was read from, against the clipboard, or
//! against another buffer, and two entries Dir has marked. `git diff
//! --no-index` reads them, and the view shows the diff the way it shows a
//! commit, so it folds and reads the same.

use std::path::{Path, PathBuf};

use crate::model::editor::{CompareRequest, JobRequest};

use crate::editors::git_exec;

// ========================================================================
// Data Structures
// ========================================================================

/// The two sides being compared, and the copies written only so they could
/// be, which go when the view does.
pub(super) struct Comparison {
    request: CompareRequest,
}

// ========================================================================
// Comparison
// ========================================================================

impl Comparison {
    pub(super) fn new(request: CompareRequest) -> Self {
        Self { request }
    }

    /// The request that reads the diff, again whenever `C-r` asks.
    pub(super) fn diff_request(&self) -> JobRequest {
        git_exec::compare(&self.root(), &self.request.old, &self.request.new)
    }

    /// Where the paths in the diff are read from: the root of the filesystem
    /// the old side is on, since git writes each path with its leading
    /// separator taken off.
    pub(super) fn root(&self) -> PathBuf {
        self.request
            .old
            .ancestors()
            .last()
            .map(Path::to_path_buf)
            .unwrap_or_default()
    }

    /// What the view is of, for its tab.
    pub(super) fn title(&self) -> &str {
        &self.request.title
    }

    /// The file `path` stands for: the file a copy of a buffer was written
    /// for, so a hunk opens the file being edited rather than the copy, and
    /// any other path as it is.
    pub(super) fn standing_for(&self, path: &Path) -> PathBuf {
        self.request
            .copies
            .iter()
            .find(|(copy, _)| copy == path)
            .and_then(|(_, real)| real.clone())
            .unwrap_or_else(|| path.to_path_buf())
    }
}

impl Drop for Comparison {
    fn drop(&mut self) {
        for (copy, _) in &self.request.copies {
            let _ = std::fs::remove_file(copy);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_copy_stands_for_its_file_and_goes_with_the_view() {
        let copy = std::env::temp_dir().join(format!("winter-compare-test-{}", std::process::id()));
        std::fs::write(&copy, "text").expect("a copy");
        let real = PathBuf::from("/src/main.rs");
        let comparison = Comparison::new(CompareRequest {
            copies: vec![(copy.clone(), Some(real.clone()))],
            new: copy.clone(),
            old: real.clone(),
            title: "main.rs".to_string(),
        });
        assert_eq!(comparison.standing_for(&copy), real);
        assert_eq!(comparison.standing_for(Path::new("/other")), PathBuf::from("/other"));
        assert_eq!(comparison.root(), PathBuf::from("/"));
        drop(comparison);
        assert!(!copy.exists(), "the copy is removed");
    }
}

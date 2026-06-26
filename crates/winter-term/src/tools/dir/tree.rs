//! Which directories a listing shows expanded, and how deep each row sits.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use super::entry::Entry;

// ========================================================================
// Data Structures
// ========================================================================

/// One row of the flattened tree.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Row {
    /// How many levels below the listing's root this row sits.
    pub depth: usize,
    /// The entry the row shows.
    pub entry: Entry,
    /// Whether this row's children are currently listed beneath it.
    pub expanded: bool,
}

/// The directories shown expanded in place.
#[derive(Clone, Debug, Default)]
pub struct Folds {
    expanded: HashSet<PathBuf>,
}

// ========================================================================
// Folds
// ========================================================================

impl Folds {
    /// A listing with everything collapsed.
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether `path`'s children are listed beneath it.
    pub fn is_expanded(&self, path: &Path) -> bool {
        self.expanded.contains(path)
    }

    /// Flip `path`, returning whether it is now expanded.
    pub fn toggle(&mut self, path: &Path) -> bool {
        if self.expanded.remove(path) {
            return false;
        }
        self.expanded.insert(path.to_path_buf());
        true
    }

    /// Expand `path`, whether or not it already was.
    pub fn expand(&mut self, path: &Path) {
        self.expanded.insert(path.to_path_buf());
    }

    /// Collapse everything.
    pub fn collapse_all(&mut self) {
        self.expanded.clear();
    }

    /// Collapse `root` and everything under it.
    pub fn collapse_under(&mut self, root: &Path) {
        self.expanded.retain(|path| !path.starts_with(root));
    }

    /// Forget anything outside `root`, so moving a listing's root does not
    /// carry folds that can never be seen again.
    pub fn retain_under(&mut self, root: &Path) {
        self.expanded.retain(|path| path.starts_with(root));
    }
}

// ========================================================================
// Row
// ========================================================================

impl Row {
    /// A row for `entry` at `depth`.
    pub fn new(depth: usize, entry: Entry, expanded: bool) -> Self {
        Self {
            depth,
            entry,
            expanded,
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
    fn test_toggle_expands_then_collapses() {
        let mut folds = Folds::new();
        let path = Path::new("/tmp/project/src");
        assert!(folds.toggle(path));
        assert!(folds.is_expanded(path));
        assert!(!folds.toggle(path));
        assert!(!folds.is_expanded(path));
    }

    #[test]
    fn test_collapse_under_closes_the_whole_subtree() {
        // Collapsing a directory has to take its open children with it, or
        // reopening it shows a tree the user thought they had closed.
        let mut folds = Folds::new();
        folds.expand(Path::new("/tmp/a"));
        folds.expand(Path::new("/tmp/a/b"));
        folds.expand(Path::new("/tmp/other"));
        folds.collapse_under(Path::new("/tmp/a"));
        assert!(!folds.is_expanded(Path::new("/tmp/a")));
        assert!(!folds.is_expanded(Path::new("/tmp/a/b")));
        assert!(folds.is_expanded(Path::new("/tmp/other")));
    }

    #[test]
    fn test_retain_under_drops_folds_outside_the_new_root() {
        // Descending into a sibling used to leave the old subtree's folds in
        // the set, so returning to a shared ancestor reopened directories the
        // user had collapsed.
        let mut folds = Folds::new();
        folds.toggle(Path::new("/tmp/a/keep"));
        folds.toggle(Path::new("/tmp/b/drop"));
        folds.retain_under(Path::new("/tmp/a"));
        assert!(folds.is_expanded(Path::new("/tmp/a/keep")));
        assert!(!folds.is_expanded(Path::new("/tmp/b/drop")));
    }
}

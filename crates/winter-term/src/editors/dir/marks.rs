//! Which entries an operation acts on: the marked set, or the cursor when
//! nothing is marked.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

// ========================================================================
// Data Structures
// ========================================================================

/// The entries the user has marked.
#[derive(Clone, Debug, Default)]
pub struct Marks {
    marked: HashSet<PathBuf>,
}

// ========================================================================
// Marks
// ========================================================================

impl Marks {
    /// A listing with nothing marked.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many entries are marked.
    pub fn len(&self) -> usize {
        self.marked.len()
    }

    /// Whether nothing is marked.
    pub fn is_empty(&self) -> bool {
        self.marked.is_empty()
    }

    /// Whether `path` is marked.
    pub fn contains(&self, path: &Path) -> bool {
        self.marked.contains(path)
    }

    /// Flip `path`, returning whether it is now marked.
    pub fn toggle(&mut self, path: &Path) -> bool {
        if self.marked.remove(path) {
            return false;
        }
        self.marked.insert(path.to_path_buf());
        true
    }

    /// Mark `path`.
    pub fn insert(&mut self, path: &Path) {
        self.marked.insert(path.to_path_buf());
    }

    /// Unmark `path`.
    pub fn remove(&mut self, path: &Path) {
        self.marked.remove(path);
    }

    /// Unmark everything.
    pub fn clear(&mut self) {
        self.marked.clear();
    }

    /// The marks that are still listed, in the order `listed` gives them, so an
    /// operation acts in the order the user sees rather than a hash order.
    pub fn in_order(&self, listed: &[PathBuf]) -> Vec<PathBuf> {
        listed
            .iter()
            .filter(|path| self.marked.contains(*path))
            .cloned()
            .collect()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What an operation acts on: every mark still listed, or `cursor` when nothing
/// is marked. An operation never silently acts on a mark scrolled out of sight
/// *and* the cursor entry, which is the mistake that deletes the wrong file.
pub fn targets(marks: &Marks, listed: &[PathBuf], cursor: Option<&Path>) -> Vec<PathBuf> {
    let marked = marks.in_order(listed);
    if !marked.is_empty() {
        return marked;
    }
    cursor
        .map(|path| vec![path.to_path_buf()])
        .unwrap_or_default()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    #[test]
    fn test_marks_take_precedence_over_the_cursor() {
        // With something marked, the cursor entry must not be swept in too.
        let mut marks = Marks::new();
        marks.insert(Path::new("a"));
        let listed = paths(&["a", "b", "c"]);
        assert_eq!(
            targets(&marks, &listed, Some(Path::new("c"))),
            paths(&["a"])
        );
    }

    #[test]
    fn test_the_cursor_is_the_target_when_nothing_is_marked() {
        let marks = Marks::new();
        let listed = paths(&["a", "b"]);
        assert_eq!(
            targets(&marks, &listed, Some(Path::new("b"))),
            paths(&["b"])
        );
    }

    #[test]
    fn test_marks_no_longer_listed_are_not_acted_on() {
        // A mark on an entry a filter or a reload removed is invisible; acting
        // on it would touch a file the user cannot see.
        let mut marks = Marks::new();
        marks.insert(Path::new("gone"));
        marks.insert(Path::new("here"));
        let listed = paths(&["here"]);
        assert_eq!(
            targets(&marks, &listed, Some(Path::new("here"))),
            paths(&["here"])
        );
    }

    #[test]
    fn test_targets_follow_the_listed_order() {
        let mut marks = Marks::new();
        marks.insert(Path::new("z"));
        marks.insert(Path::new("a"));
        let listed = paths(&["a", "m", "z"]);
        assert_eq!(marks.in_order(&listed), paths(&["a", "z"]));
    }

    #[test]
    fn test_an_empty_listing_with_no_cursor_has_no_targets() {
        let marks = Marks::new();
        assert!(targets(&marks, &[], None).is_empty());
    }
}

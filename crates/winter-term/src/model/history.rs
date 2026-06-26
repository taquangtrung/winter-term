//! A small generic undo/redo stack shared by the editable surfaces (the command
//! palette query and the prompt shadow buffer).
//!
//! The history holds a `current` snapshot plus a `past` stack (states reachable by
//! undo) and a `future` stack (states reachable by redo). Recording a new state
//! discards the future, the standard editor behaviour where a fresh edit forks
//! away any redo branch.

// ========================================================================
// Constants
// ========================================================================

/// How many past states to retain before dropping the oldest. Bounds memory on
/// long editing sessions; deeper-than-this undo is rare in a command line.
const HISTORY_DEPTH: usize = 200;

// ========================================================================
// Data Structures
// ========================================================================

/// An undo/redo stack over snapshots of an editable value.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditHistory<T> {
    current: T,
    future: Vec<T>,
    past: Vec<T>,
}

// ========================================================================
// Implementation
// ========================================================================

impl<T: Clone + PartialEq> EditHistory<T> {
    /// A fresh history whose only state is `initial`.
    pub fn new(initial: T) -> Self {
        Self {
            current: initial,
            future: Vec::new(),
            past: Vec::new(),
        }
    }

    /// The state most recently settled on.
    pub fn current(&self) -> &T {
        &self.current
    }

    /// Settle on `next` as a new undo point. A no-op when `next` equals the
    /// current state, so callers may record liberally. Recording forks away any
    /// redo branch and bounds the past to [`HISTORY_DEPTH`].
    pub fn record(&mut self, next: T) {
        if next == self.current {
            return;
        }
        let previous = std::mem::replace(&mut self.current, next);
        self.past.push(previous);
        if self.past.len() > HISTORY_DEPTH {
            self.past.remove(0);
        }
        self.future.clear();
    }

    /// Replace the current state in place, without creating a new undo point.
    /// Used to coalesce a run of edits (e.g. consecutive insertions) into one
    /// undoable step; like [`record`](Self::record) it forks away the redo branch.
    pub fn amend(&mut self, value: T) {
        self.current = value;
        self.future.clear();
    }

    /// Step back one state, returning it; `None` when there is nothing to undo.
    pub fn undo(&mut self) -> Option<&T> {
        let previous = self.past.pop()?;
        let current = std::mem::replace(&mut self.current, previous);
        self.future.push(current);
        Some(&self.current)
    }

    /// Step forward one undone state, returning it; `None` when there is nothing
    /// to redo.
    pub fn redo(&mut self) -> Option<&T> {
        let next = self.future.pop()?;
        let current = std::mem::replace(&mut self.current, next);
        self.past.push(current);
        Some(&self.current)
    }

    /// Drop all history and start over from `value`.
    pub fn reset(&mut self, value: T) {
        self.current = value;
        self.past.clear();
        self.future.clear();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_has_no_undo_or_redo() {
        let mut h = EditHistory::new(String::from("a"));
        assert_eq!(h.current(), "a");
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), None);
    }

    #[test]
    fn test_record_then_undo_returns_previous() {
        let mut h = EditHistory::new(String::from("a"));
        h.record(String::from("ab"));
        h.record(String::from("abc"));
        assert_eq!(h.undo().map(String::as_str), Some("ab"));
        assert_eq!(h.undo().map(String::as_str), Some("a"));
        assert_eq!(h.undo(), None);
    }

    #[test]
    fn test_redo_reverses_undo() {
        let mut h = EditHistory::new(0);
        h.record(1);
        h.record(2);
        h.undo();
        h.undo();
        assert_eq!(h.current(), &0);
        assert_eq!(h.redo(), Some(&1));
        assert_eq!(h.redo(), Some(&2));
        assert_eq!(h.redo(), None);
    }

    #[test]
    fn test_record_equal_to_current_is_noop() {
        let mut h = EditHistory::new(String::from("a"));
        h.record(String::from("a"));
        assert_eq!(
            h.undo(),
            None,
            "recording the current value adds no undo point"
        );
    }

    #[test]
    fn test_record_after_undo_clears_redo() {
        let mut h = EditHistory::new(0);
        h.record(1);
        h.undo();
        assert_eq!(h.current(), &0);
        h.record(9);
        assert_eq!(h.redo(), None, "a fresh edit forks away the redo branch");
        assert_eq!(h.undo(), Some(&0));
    }

    #[test]
    fn test_reset_drops_history() {
        let mut h = EditHistory::new(0);
        h.record(1);
        h.record(2);
        h.reset(7);
        assert_eq!(h.current(), &7);
        assert_eq!(h.undo(), None);
        assert_eq!(h.redo(), None);
    }

    #[test]
    fn test_past_is_bounded_to_history_depth() {
        let mut h = EditHistory::new(0);
        for n in 1..=(HISTORY_DEPTH + 50) {
            h.record(n);
        }
        let mut steps = 0;
        while h.undo().is_some() {
            steps += 1;
        }
        assert_eq!(
            steps, HISTORY_DEPTH,
            "no more than HISTORY_DEPTH undo points retained"
        );
    }
}

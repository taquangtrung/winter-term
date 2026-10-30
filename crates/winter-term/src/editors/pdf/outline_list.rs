//! The outline, open under the toolbar as a drop-down list: the sections,
//! folded to the top level but for the way down to the one being read, with
//! a cursor to pick one with.

use std::collections::HashSet;

use winter_render::DropListRow;

use super::outline::{Entry, Target};

// ========================================================================
// Constants
// ========================================================================

/// How far each level of nesting is indented, in columns.
const INDENT: usize = 2;

/// The cells an entry's fold mark takes before its title.
const MARK_CELLS: usize = 2;

/// The cells left of every row's mark, and right of its page.
const MARGIN: usize = 1;

/// The cells between a title and its page.
const PAGE_GAP: usize = 2;

/// The narrowest and widest the list is drawn, in cells.
const MIN_COLS: usize = 28;
const MAX_COLS: usize = 72;

// ========================================================================
// Data Structures
// ========================================================================

/// The open outline: which entry the cursor is on and which are unfolded,
/// by their index in the outline, and the first shown.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OutlineList {
    pub cursor: usize,
    unfolded: HashSet<usize>,
    /// The first of the shown entries at the list's top row.
    top: usize,
}

/// What a click on a row of the list landed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowHit {
    /// The fold mark of the entry at this index.
    Fold(usize),
    /// The entry at this index, anywhere else on its row.
    Entry(usize),
}

// ========================================================================
// OutlineList
// ========================================================================

impl OutlineList {
    /// The list opened on entry `at`, everything above it unfolded so it
    /// shows: the first entry when there is no `at`.
    pub fn open(entries: &[Entry], at: Option<usize>) -> Self {
        let cursor = at.unwrap_or(0);
        let mut list = Self {
            cursor,
            ..Self::default()
        };
        list.unfolded.extend(ancestors(entries, cursor));
        list
    }

    /// The indices of the entries shown: those no folded entry hides.
    pub fn shown(&self, entries: &[Entry]) -> Vec<usize> {
        let mut shown = Vec::new();
        // Entries deeper than this are under a folded one.
        let mut hidden_below: Option<usize> = None;
        for (index, entry) in entries.iter().enumerate() {
            if hidden_below.is_some_and(|depth| entry.depth > depth) {
                continue;
            }
            hidden_below = None;
            shown.push(index);
            if has_children(entries, index) && !self.unfolded.contains(&index) {
                hidden_below = Some(entry.depth);
            }
        }
        shown
    }

    /// Move the cursor `steps` shown entries down, up when negative.
    pub fn step(&mut self, entries: &[Entry], steps: isize) {
        let shown = self.shown(entries);
        let at = shown
            .iter()
            .position(|index| *index == self.cursor)
            .unwrap_or(0);
        let to = at
            .saturating_add_signed(steps)
            .min(shown.len().saturating_sub(1));
        if let Some(index) = shown.get(to) {
            self.cursor = *index;
        }
    }

    /// Unfold the entry under the cursor, or step into it once it is.
    pub fn unfold(&mut self, entries: &[Entry]) {
        if !has_children(entries, self.cursor) {
            return;
        }
        if !self.unfolded.insert(self.cursor) {
            self.step(entries, 1);
        }
    }

    /// Fold the entry under the cursor, or step out to its parent once it
    /// is.
    pub fn fold(&mut self, entries: &[Entry]) {
        if self.unfolded.remove(&self.cursor) {
            return;
        }
        if let Some(parent) = ancestors(entries, self.cursor).last() {
            self.cursor = *parent;
        }
    }

    /// Fold or unfold the entry at `index`.
    pub fn toggle(&mut self, index: usize) {
        if !self.unfolded.remove(&index) {
            self.unfolded.insert(index);
        }
    }

    /// The list `height` rows tall, scrolled so the cursor shows, and which
    /// of those rows the cursor is on.
    pub fn rows(&mut self, entries: &[Entry], height: usize) -> (Vec<DropListRow>, usize) {
        let shown = self.shown(entries);
        let at = shown
            .iter()
            .position(|index| *index == self.cursor)
            .unwrap_or(0);
        if at < self.top {
            self.top = at;
        } else if height > 0 && at >= self.top + height {
            self.top = at + 1 - height;
        }
        self.top = self.top.min(shown.len().saturating_sub(height));
        let rows = shown
            .iter()
            .skip(self.top)
            .take(height)
            .map(|index| self.row(entries, *index))
            .collect();
        (rows, at - self.top)
    }

    /// How many cells wide the list is drawn in a pane `cols` wide: room for
    /// its widest entry shown, within bounds.
    pub fn width(&self, entries: &[Entry], cols: usize) -> usize {
        let widest = self
            .shown(entries)
            .into_iter()
            .map(|index| {
                let entry = &entries[index];
                MARGIN + entry.depth * INDENT + MARK_CELLS + entry.title.chars().count() + PAGE_GAP + page_of(entry).len() + MARGIN
            })
            .max()
            .unwrap_or(0);
        widest.clamp(MIN_COLS, MAX_COLS).min(cols.saturating_sub(2)).max(1)
    }

    /// What row `row` of the list, as last drawn, holds at column `col` of
    /// it.
    pub fn hit(&self, entries: &[Entry], row: usize, col: usize) -> Option<RowHit> {
        let index = *self.shown(entries).get(self.top + row)?;
        let mark = MARGIN + entries[index].depth * INDENT;
        let on_mark = (mark..mark + MARK_CELLS).contains(&col);
        match on_mark && has_children(entries, index) {
            true => Some(RowHit::Fold(index)),
            false => Some(RowHit::Entry(index)),
        }
    }

    /// One entry's row: its mark indented to its depth, whether it folds,
    /// its title, and its page.
    fn row(&self, entries: &[Entry], index: usize) -> DropListRow {
        let entry = &entries[index];
        DropListRow {
            dim: entry.target.is_none(),
            fold: has_children(entries, index).then(|| self.unfolded.contains(&index)),
            label: entry.title.clone(),
            mark_col: MARGIN + entry.depth * INDENT,
            note: page_of(entry),
        }
    }
}

/// The page an entry points at, counted from one, or nothing.
fn page_of(entry: &Entry) -> String {
    entry
        .target
        .map(|target| (target.page + 1).to_string())
        .unwrap_or_default()
}

// ========================================================================
// Free functions
// ========================================================================

/// The entry the spot `at` is in: the last, in the outline's order, that
/// points at or before it. The outline's order is the reading order, which
/// a two-column page's coordinates are not, so a section is not taken for
/// later just for starting higher up the right column.
pub fn current(entries: &[Entry], at: Target) -> Option<usize> {
    entries.iter().rposition(|entry| {
        entry
            .target
            .is_some_and(|target| (target.page, target.y) <= (at.page, at.y))
    })
}

/// Whether the entry at `index` has entries under it.
fn has_children(entries: &[Entry], index: usize) -> bool {
    match (entries.get(index), entries.get(index + 1)) {
        (Some(entry), Some(next)) => next.depth > entry.depth,
        _ => false,
    }
}

/// The indices of the entries the one at `index` is under, outermost first.
fn ancestors(entries: &[Entry], index: usize) -> Vec<usize> {
    let Some(entry) = entries.get(index) else {
        return Vec::new();
    };
    let mut chain = Vec::new();
    let mut depth = entry.depth;
    for at in (0..index).rev() {
        if entries[at].depth < depth {
            chain.push(at);
            depth = entries[at].depth;
        }
    }
    chain.reverse();
    chain
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
pub(super) mod tests {
    use super::*;

    /// An entry `depth` deep titled `title`, pointing at `(page, y)`.
    pub(in super::super) fn entry(depth: usize, title: &str, page: usize, y: f32) -> Entry {
        Entry {
            depth,
            target: Some(Target { page, y }),
            title: title.to_string(),
        }
    }

    /// A two-level outline, laid out the way a two-column paper's is: 2.2
    /// starts on the same page as 2.1, higher up the right column.
    pub(in super::super) fn paper() -> Vec<Entry> {
        vec![
            entry(0, "1 Introduction", 0, 100.0),
            entry(0, "2 Background", 1, 80.0),
            entry(1, "2.1 Machine", 1, 400.0),
            entry(1, "2.2 Gas", 1, 120.0),
            entry(0, "3 Problem", 2, 300.0),
        ]
    }

    fn titles(list: &OutlineList, entries: &[Entry]) -> Vec<String> {
        list.shown(entries)
            .into_iter()
            .map(|index| entries[index].title.clone())
            .collect()
    }

    #[test]
    fn test_the_list_opens_unfolded_down_to_the_entry_being_read() {
        let entries = paper();
        let list = OutlineList::open(&entries, Some(2));
        assert_eq!(list.cursor, 2);
        assert_eq!(titles(&list, &entries).len(), 5, "2 unfolded to show 2.1");
        let list = OutlineList::open(&entries, Some(0));
        assert_eq!(
            titles(&list, &entries),
            ["1 Introduction", "2 Background", "3 Problem"],
            "folded to the top level otherwise"
        );
    }

    #[test]
    fn test_unfolding_shows_the_entries_under_it_and_then_steps_in() {
        let entries = paper();
        let mut list = OutlineList::open(&entries, Some(1));
        list.unfold(&entries);
        assert_eq!(titles(&list, &entries).len(), 5);
        assert_eq!(list.cursor, 1, "the first press only unfolds");
        list.unfold(&entries);
        assert_eq!(list.cursor, 2, "the second steps in");
        list.fold(&entries);
        assert_eq!(list.cursor, 1, "folding a leaf steps out to its parent");
        list.fold(&entries);
        assert_eq!(titles(&list, &entries).len(), 3, "then folds it");
    }

    #[test]
    fn test_the_cursor_steps_over_folded_entries() {
        let entries = paper();
        let mut list = OutlineList::open(&entries, Some(0));
        list.step(&entries, 2);
        assert_eq!(entries[list.cursor].title, "3 Problem");
        list.step(&entries, 5);
        assert_eq!(entries[list.cursor].title, "3 Problem", "stops at the last");
    }

    #[test]
    fn test_the_section_read_is_the_last_in_outline_order_before_the_spot() {
        let entries = paper();
        // Low on page 2's left column: 2.2 starts higher, but on the right.
        let at = Target { page: 1, y: 500.0 };
        assert_eq!(current(&entries, at), Some(3));
        let at = Target { page: 1, y: 90.0 };
        assert_eq!(current(&entries, at), Some(1));
        let at = Target { page: 0, y: 10.0 };
        assert_eq!(current(&entries, at), None, "ahead of every section");
    }

    #[test]
    fn test_a_click_on_a_fold_mark_folds_and_elsewhere_picks() {
        let entries = paper();
        let mut list = OutlineList::open(&entries, Some(0));
        let (rows, _) = list.rows(&entries, 10);
        assert_eq!(rows[1].fold, Some(false), "2 folds, and is folded");
        let mark = rows[1].mark_col;
        assert_eq!(list.hit(&entries, 1, mark), Some(RowHit::Fold(1)));
        assert_eq!(list.hit(&entries, 1, mark + 5), Some(RowHit::Entry(1)));
        assert_eq!(
            list.hit(&entries, 0, mark),
            Some(RowHit::Entry(0)),
            "a leaf"
        );
    }

    #[test]
    fn test_a_list_too_long_scrolls_to_keep_the_cursor_shown() {
        let entries = paper();
        let mut list = OutlineList::open(&entries, Some(2));
        let (rows, cursor_row) = list.rows(&entries, 2);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[cursor_row].label, "2.1 Machine");
        assert_eq!(rows[cursor_row].note, "2", "its page, from one");
    }
}

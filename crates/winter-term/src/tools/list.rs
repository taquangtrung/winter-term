//! The window a tool's rows are read through, and the cursor a list keeps on
//! them: the state every tool painting rows would otherwise keep, and get
//! subtly different, on its own.
//!
//! [`Viewport`] is the window alone: how tall the pane is, where it is
//! scrolled to, and whether the wheel is holding it there. A tool with a
//! cursor of its own (an editable buffer) or with none (a report read top to
//! bottom) uses just this. [`ListView`] adds the cursor over a list of rows,
//! kept on the same row by its key when the list is rebuilt under it, with
//! the search and the jump preview a list offers.

use crate::model::input::CursorMove;
use crate::model::paint::{wheel_scroll, window_anchor, wrap_window, RowWindow};
use crate::model::tool::{find_match, row_search_report, Notice, SearchReport};

// ========================================================================
// Data Structures
// ========================================================================

/// The rows a pane shows of a longer list: where it starts, how many fit,
/// and whether the wheel put it there.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    /// Whether the wheel, not the cursor, last placed the view: it then stays
    /// where it was put until a key brings the cursor back into it.
    held: bool,
    /// How many rows the pane showed at the last paint. Zero until the first
    /// paint, when there is nothing to page by.
    rows: usize,
    /// The first row showing.
    scroll: usize,
}

/// Where a paint lays its rows out, and whether the cursor is among them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Placement {
    /// Whether the window holds the cursor, which is when a tool paints its
    /// cursor band.
    pub shows_cursor: bool,
    /// The rows to paint.
    pub window: RowWindow,
}

/// A cursor over a list of rows, each named by a key of type `K`: the key is
/// what keeps the cursor on the same row when the list is read again and
/// rows come or go above it.
#[derive(Clone, Debug)]
pub struct ListView<K> {
    /// The row the cursor is on.
    cursor: usize,
    /// Where the cursor and the scroll were when a jump preview started, put
    /// back if it is cancelled.
    jump_origin: Option<(usize, usize)>,
    /// One key per row, in order.
    keys: Vec<K>,
    /// The last text searched for, repeated by the next and previous keys.
    search: String,
    /// The search last run or repeated, until the host takes it to highlight.
    searched: Option<SearchReport>,
    /// The window over the rows.
    view: Viewport,
}

// ========================================================================
// Viewport
// ========================================================================

impl Viewport {
    /// A window at the top of its rows, not yet painted.
    pub fn new() -> Self {
        Self::default()
    }

    /// How many rows the pane showed at the last paint.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Note how many rows the pane shows, for the paging motions, which key
    /// handling needs between paints.
    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
    }

    /// The first row showing.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// Start the window at `scroll`, as a paint or a jump decides.
    pub fn set_scroll(&mut self, scroll: usize) {
        self.scroll = scroll;
    }

    /// The first row showing, lent to a text editor that scrolls it itself.
    pub fn scroll_mut(&mut self) -> &mut usize {
        &mut self.scroll
    }

    /// Turn the wheel `lines` rows, positive upwards, over `total` rows. The
    /// view then stays there until [`Viewport::release`].
    pub fn wheel(&mut self, lines: isize, total: usize) {
        self.scroll = wheel_scroll(self.scroll, lines, total, self.rows);
        self.held = true;
    }

    /// A key was pressed: the cursor decides where the view is again.
    pub fn release(&mut self) {
        self.held = false;
    }

    /// Move the window `delta` rows over `total` rows, for a view with no
    /// cursor, stopping at the last start that still fills the pane.
    pub fn scroll_by(&mut self, delta: isize, total: usize) {
        self.scroll_to(self.scroll.saturating_add_signed(delta), total);
    }

    /// Start the window at `at`, clamped the way [`Viewport::scroll_by`] is.
    pub fn scroll_to(&mut self, at: usize, total: usize) {
        self.scroll = at.min(total.saturating_sub(self.rows));
    }

    /// Lay `total` rows out in `visible` screen rows around `cursor`, each
    /// `height(row)` screen rows tall, and start the window there.
    pub fn place(
        &mut self,
        cursor: usize,
        total: usize,
        visible: usize,
        height: impl Fn(usize) -> usize,
    ) -> Placement {
        let anchor = window_anchor(self.held, self.scroll, cursor, visible);
        let window = wrap_window(self.scroll, anchor, total, visible, height);
        self.scroll = window.start;
        Placement {
            shows_cursor: anchor == cursor,
            window,
        }
    }
}

// ========================================================================
// ListView
// ========================================================================

impl<K: Clone + PartialEq> ListView<K> {
    // --------------------------------------------------------------------
    // Rows
    // --------------------------------------------------------------------

    /// A cursor over no rows yet.
    pub fn new() -> Self {
        Self {
            cursor: 0,
            jump_origin: None,
            keys: Vec::new(),
            search: String::new(),
            searched: None,
            view: Viewport::new(),
        }
    }

    /// Take a new list of rows, one key each. The cursor stays on the row
    /// whose key it was on; where that row is gone it stays at the same
    /// index, clamped to the list, which lands on the row that took its place.
    pub fn set_keys(&mut self, keys: Vec<K>) {
        let selected = self.keys.get(self.cursor).cloned();
        self.keys = keys;
        if let Some(index) = selected.and_then(|key| self.position(&key)) {
            self.cursor = index;
        }
        self.clamp();
    }

    /// The same, but leaving the cursor at its index whatever row is there
    /// now: for a list read again as a new one, a new search's answers.
    pub fn reset_keys(&mut self, keys: Vec<K>) {
        self.keys = keys;
        self.clamp();
    }

    /// The key of the row the cursor is on.
    pub fn selected(&self) -> Option<&K> {
        self.keys.get(self.cursor)
    }

    /// Where the row named `key` is.
    pub fn position(&self, key: &K) -> Option<usize> {
        self.keys.iter().position(|candidate| candidate == key)
    }

    /// The row the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Put the cursor on row `at`, clamped to the list.
    pub fn set_cursor(&mut self, at: usize) {
        self.cursor = at;
        self.clamp();
    }

    /// Put the cursor on the row named `key`, where there is one.
    pub fn select(&mut self, key: &K) -> bool {
        match self.position(key) {
            Some(index) => {
                self.cursor = index;
                true
            }
            None => false,
        }
    }

    /// The window over the rows.
    pub fn view(&self) -> &Viewport {
        &self.view
    }

    /// The window over the rows, to scroll or place.
    pub fn view_mut(&mut self) -> &mut Viewport {
        &mut self.view
    }

    /// Lay the rows out around the cursor (see [`Viewport::place`]).
    pub fn place(&mut self, visible: usize, height: impl Fn(usize) -> usize) -> Placement {
        let (cursor, total) = (self.cursor, self.keys.len());
        self.view.place(cursor, total, visible, height)
    }

    fn clamp(&mut self) {
        self.cursor = self.cursor.min(self.keys.len().saturating_sub(1));
    }

    // --------------------------------------------------------------------
    // Motions
    // --------------------------------------------------------------------

    /// Move the cursor `delta` rows, clamped to the list.
    pub fn move_by(&mut self, delta: isize) {
        let last = self.keys.len().saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// Carry out one of the motions that mean the same over any list of
    /// rows: a row, a page, either end, or a place on the screen. `false`
    /// for the rest (words, paragraphs, columns), which each tool reads over
    /// its own rows.
    pub fn apply_motion(&mut self, motion: CursorMove) -> bool {
        let last = self.keys.len().saturating_sub(1);
        let rows = self.view.rows;
        let half = (rows / 2).max(1) as isize;
        let page = rows.max(1) as isize;
        let scroll = self.view.scroll;
        match motion {
            CursorMove::Down => self.move_by(1),
            CursorMove::Up => self.move_by(-1),
            CursorMove::Top | CursorMove::LineStart | CursorMove::FirstNonBlank => self.cursor = 0,
            CursorMove::Bottom | CursorMove::LineEnd => self.cursor = last,
            CursorMove::HalfPageDown => self.move_by(half),
            CursorMove::HalfPageUp => self.move_by(-half),
            CursorMove::PageDown => self.move_by(page),
            CursorMove::PageUp => self.move_by(-page),
            CursorMove::ScreenTop => self.cursor = scroll.min(last),
            CursorMove::ScreenMiddle => self.cursor = (scroll + rows / 2).min(last),
            CursorMove::ScreenBottom => {
                self.cursor = scroll.saturating_add(rows.saturating_sub(1)).min(last);
            }
            // Words, paragraphs, and columns mean something different over
            // each tool's rows, so the tool reads those itself.
            _ => return false,
        }
        true
    }

    // --------------------------------------------------------------------
    // Search
    // --------------------------------------------------------------------

    /// Remember `query` and move to the next row whose label holds it.
    pub fn search_for(&mut self, labels: &[impl AsRef<str>], query: &str) -> Option<Notice> {
        self.search_with(labels, query, true)
    }

    /// Remember `query` and step to the next or previous row holding it, for
    /// a search one list carries on from another.
    pub fn search_with(
        &mut self,
        labels: &[impl AsRef<str>],
        query: &str,
        forward: bool,
    ) -> Option<Notice> {
        self.set_search(query);
        self.search_step(labels, forward)
    }

    /// Remember `query` as the last search without moving, for a list whose
    /// search was run over another.
    pub fn set_search(&mut self, query: &str) {
        self.search = query.to_string();
    }

    /// Move to the next or previous row holding the last search, by the
    /// rows' `labels`. What to tell the reader comes back, when the search
    /// went nowhere.
    pub fn search_step(&mut self, labels: &[impl AsRef<str>], forward: bool) -> Option<Notice> {
        if self.search.is_empty() {
            return Some(Notice::error("no search"));
        }
        let found = find_match(labels, &self.search, self.cursor, forward);
        if let Some(index) = found {
            self.cursor = index;
        }
        self.searched = Some(row_search_report(labels, &self.search, self.cursor, false));
        match found {
            Some(_) => None,
            None => Some(Notice::error(format!("not found: {}", self.search))),
        }
    }

    /// The last text searched for.
    pub fn search(&self) -> &str {
        &self.search
    }

    /// The search run or repeated since the host last asked, taken.
    pub fn take_search(&mut self) -> Option<SearchReport> {
        self.searched.take()
    }

    // --------------------------------------------------------------------
    // Jumps
    // --------------------------------------------------------------------

    /// Preview a jump to row `target`, remembering where the cursor was the
    /// first time so a cancelled jump goes back there.
    pub fn jump_to(&mut self, target: usize) {
        if self.jump_origin.is_none() {
            self.jump_origin = Some((self.cursor, self.view.scroll));
        }
        self.land(target);
    }

    /// Go back to where the cursor was before the preview.
    pub fn jump_cancel(&mut self) {
        if let Some((cursor, scroll)) = self.jump_origin.take() {
            self.set_cursor(cursor);
            self.view.scroll = scroll;
        }
    }

    /// Settle on row `target`.
    pub fn jump_confirm(&mut self, target: usize) {
        self.jump_origin = None;
        self.land(target);
    }

    /// Whether a jump preview is under way.
    #[cfg(test)]
    pub fn is_jumping(&self) -> bool {
        self.jump_origin.is_some()
    }

    /// Put the cursor on `target` with it in the middle of the pane.
    fn land(&mut self, target: usize) {
        self.set_cursor(target);
        self.view.scroll = target.saturating_sub(self.view.rows / 2);
    }
}

impl<K: Clone + PartialEq> Default for ListView<K> {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn list(keys: &[&'static str]) -> ListView<&'static str> {
        let mut view = ListView::new();
        view.set_keys(keys.to_vec());
        view
    }

    #[test]
    fn test_the_cursor_follows_its_row_when_rows_appear_above_it() {
        let mut view = list(&["a", "b", "c"]);
        view.set_cursor(1);
        view.set_keys(vec!["new", "a", "b", "c"]);
        assert_eq!(view.selected(), Some(&"b"), "still on b, now one lower");
    }

    #[test]
    fn test_a_row_that_went_leaves_the_cursor_on_the_one_that_took_its_place() {
        let mut view = list(&["a", "b", "c"]);
        view.set_cursor(1);
        view.set_keys(vec!["a", "c"]);
        assert_eq!(view.selected(), Some(&"c"));
        view.set_cursor(1);
        view.set_keys(vec!["a"]);
        assert_eq!(view.cursor(), 0, "clamped to what is left");
    }

    #[test]
    fn test_resetting_the_rows_keeps_the_index_not_the_row() {
        let mut view = list(&["a", "b", "c"]);
        view.set_cursor(1);
        view.reset_keys(vec!["x", "a", "b"]);
        assert_eq!(view.selected(), Some(&"a"));
    }

    #[test]
    fn test_paging_moves_by_the_pane_and_stops_at_the_ends() {
        let mut view = list(&["a"; 30]);
        view.view_mut().set_rows(10);
        assert!(view.apply_motion(CursorMove::PageDown));
        assert_eq!(view.cursor(), 10);
        view.apply_motion(CursorMove::HalfPageDown);
        assert_eq!(view.cursor(), 15);
        view.apply_motion(CursorMove::Bottom);
        view.apply_motion(CursorMove::Down);
        assert_eq!(view.cursor(), 29, "no row past the last");
        view.apply_motion(CursorMove::Top);
        view.apply_motion(CursorMove::Up);
        assert_eq!(view.cursor(), 0);
        assert!(
            !view.apply_motion(CursorMove::WordForward),
            "the tool's own"
        );
    }

    #[test]
    fn test_a_search_steps_through_its_matches_and_reports_a_miss() {
        let labels = ["alpha", "beta", "alphabet"];
        let mut view = list(&["0", "1", "2"]);
        assert!(view.search_step(&labels, true).is_some(), "no search yet");
        assert_eq!(view.search_for(&labels, "alpha"), None);
        assert_eq!(view.cursor(), 2, "the next match after the cursor");
        view.search_step(&labels, true);
        assert_eq!(view.cursor(), 0, "wrapping round");
        let report = view.take_search().expect("a report");
        assert_eq!((report.index, report.total), (1, 2));
        assert!(view.search_for(&labels, "gamma").is_some(), "not found");
        assert_eq!(view.cursor(), 0, "a miss leaves the cursor alone");
    }

    #[test]
    fn test_a_cancelled_jump_goes_back_where_it_started() {
        let mut view = list(&["a"; 50]);
        view.view_mut().set_rows(10);
        view.set_cursor(3);
        view.jump_to(40);
        view.jump_to(45);
        assert_eq!((view.cursor(), view.view().scroll()), (45, 40));
        view.jump_cancel();
        assert_eq!((view.cursor(), view.view().scroll()), (3, 0));
        assert!(!view.is_jumping());
    }

    #[test]
    fn test_the_wheel_holds_the_view_until_a_key_releases_it() {
        let mut view = list(&["a"; 50]);
        view.view_mut().set_rows(10);
        view.view_mut().wheel(-20, 50);
        let held = view.place(10, |_| 1);
        assert!(!held.shows_cursor, "the cursor scrolled out of view");
        assert_eq!(held.window.start, 20);
        view.view_mut().release();
        let placed = view.place(10, |_| 1);
        assert!(placed.shows_cursor);
        assert_eq!(placed.window.start, 0);
    }

    #[test]
    fn test_a_view_with_no_cursor_scrolls_no_further_than_fills_the_pane() {
        let mut view = Viewport::new();
        view.set_rows(10);
        view.scroll_by(100, 25);
        assert_eq!(view.scroll(), 15);
        view.scroll_by(-100, 25);
        assert_eq!(view.scroll(), 0);
    }
}

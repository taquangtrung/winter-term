//! The window an editor's rows are read through, and the cursor a list keeps on
//! them: the state every editor painting rows would otherwise keep, and get
//! subtly different, on its own.
//!
//! [`Viewport`] is the window alone: how tall the pane is, where it is
//! scrolled to, and whether the wheel is holding it there. An editor with a
//! cursor of its own (an editable buffer) or with none (a report read top to
//! bottom) uses just this. [`ListView`] adds the cursor over a list of rows,
//! kept on the same row by its key when the list is rebuilt under it, with
//! the search and the jump preview a list offers. [`RowSearch`] is the
//! search alone, for an editor whose rows are no keyed list (a build's
//! growing output).

use crate::model::editor::{find_match, row_search_report, JumpStep, Notice, SearchReport};
use crate::model::input::CursorMove;
use crate::model::paint::{place_view, CursorRows, Frame, Place, RowWindow, Shown, ViewAim};

// ========================================================================
// Data Structures
// ========================================================================

/// The rows a pane shows of a longer list: where it starts, as a row and
/// how many of its screen rows are scrolled past, how many fit, and whether
/// the wheel put it there. Every move of it counts screen rows, so a list
/// whose rows wrap scrolls a screen row at a time, and one whose rows do not
/// a row at a time. What needs the rows' heights (the wheel, a placement) is
/// asked for between paints and carried out at the next, which knows them.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    /// Whether the wheel, not the cursor, last placed the view: it then stays
    /// where it was put until a key brings the cursor back into it.
    held: bool,
    /// How many screen rows the view keeps above and below the cursor's row
    /// as it follows it, where the rows allow.
    margin: usize,
    /// Where the next paint is to put the cursor's row, as `zt`, `zz`, `zb`,
    /// or a jump asked.
    place: Option<Place>,
    /// How many screen rows the pane showed at the last paint. Zero until the
    /// first paint, when there is nothing to page by.
    rows: usize,
    /// The first row showing.
    scroll: usize,
    /// The rows on the pane's top, middle, and bottom screen rows at the
    /// last paint.
    shown: Shown,
    /// How many screen rows of the first row showing are scrolled past.
    skip: usize,
    /// The screen rows the wheel has turned since the last paint, positive
    /// upwards.
    wheel: isize,
}

/// What a list of rows was last searched for, and the search the host is
/// still to take to highlight.
#[derive(Clone, Debug, Default)]
pub struct RowSearch {
    /// The last text searched for, repeated by the next and previous keys.
    query: String,
    /// The search last run or repeated, until the host takes it.
    report: Option<SearchReport>,
}

/// Where a paint lays its rows out, and whether the cursor is among them
/// (see [`Placement::apply`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Placement {
    /// Whether the window holds the cursor, which is when an editor paints its
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
    /// Where the cursor and the window were when a jump preview started, put
    /// back if it is cancelled.
    jump_origin: Option<(usize, Viewport)>,
    /// One key per row, in order.
    keys: Vec<K>,
    /// The last search, repeated by the next and previous keys.
    search: RowSearch,
    /// The window over the rows.
    view: Viewport,
}

/// Where one tab's cursor and window over a list were, kept while another
/// tab on the same list is the one acted on. The rows are the list's, read
/// again under either tab, so this keeps the cursor's key rather than the
/// rows themselves.
#[derive(Clone, Debug)]
pub struct ListPlace<K> {
    /// The row the cursor was on, for where its key is gone.
    cursor: usize,
    /// The key of that row.
    key: Option<K>,
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

    /// How many screen rows the pane showed at the last paint.
    pub fn rows(&self) -> usize {
        self.rows
    }

    /// Note how many screen rows the pane shows, for the paging motions,
    /// which key handling needs between paints.
    pub fn set_rows(&mut self, rows: usize) {
        self.rows = rows;
    }

    /// The first row showing.
    pub fn scroll(&self) -> usize {
        self.scroll
    }

    /// The rows on the pane's top, middle, and bottom screen rows at the
    /// last paint, which `H`, `M`, and `L` go to.
    pub fn shown(&self) -> Shown {
        self.shown
    }

    /// Start the window at the top of row `scroll`, as a paint or a jump
    /// decides.
    pub fn set_scroll(&mut self, scroll: usize) {
        (self.scroll, self.skip) = (scroll, 0);
    }

    /// Turn the wheel `lines` screen rows, positive upwards. The view then
    /// stays there until [`Viewport::release`].
    pub fn wheel(&mut self, lines: isize) {
        self.wheel += lines;
        self.held = true;
    }

    /// Keep `margin` screen rows above and below the cursor's row from the
    /// next paint on.
    pub fn set_margin(&mut self, margin: usize) {
        self.margin = margin;
    }

    /// Have the next paint put the cursor's row at `place` in the pane.
    pub fn request(&mut self, place: Place) {
        self.place = Some(place);
    }

    /// A key was pressed: the cursor decides where the view is again.
    pub fn release(&mut self) {
        self.held = false;
    }

    /// Move the window `delta` rows over `total` rows, for a view with no
    /// cursor and rows that never wrap, stopping at the last start that
    /// still fills the pane.
    pub fn scroll_by(&mut self, delta: isize, total: usize) {
        self.scroll_to(self.scroll.saturating_add_signed(delta), total);
    }

    /// Start the window at `at`, clamped the way [`Viewport::scroll_by`] is.
    pub fn scroll_to(&mut self, at: usize, total: usize) {
        self.set_scroll(at.min(total.saturating_sub(self.rows)));
    }

    /// Lay `total` rows out in `visible` screen rows around `cursor`, each
    /// `height(row)` screen rows tall, and start the window there: where
    /// the wheel turned it to, or where a placement asked, or else moved the
    /// least that shows the cursor's row, whole where it fits, with the
    /// margin round it.
    pub fn place(
        &mut self,
        cursor: usize,
        total: usize,
        visible: usize,
        height: impl Fn(usize) -> usize,
    ) -> Placement {
        let aim = match (self.place.take(), self.held) {
            (Some(place), _) => ViewAim::Place(place),
            (None, true) => ViewAim::Held,
            (None, false) => ViewAim::Follow,
        };
        // The cursor is its whole row: first screen row to last.
        let rows = match total {
            0 => CursorRows { first: (0, 0), last: (0, 0) },
            _ => CursorRows {
                first: (cursor, 0),
                last: (cursor, height(cursor).max(1) - 1),
            },
        };
        let wheel = std::mem::take(&mut self.wheel);
        let window = place_view((self.scroll, self.skip), wheel, aim, rows, total, visible, self.margin, height);
        (self.scroll, self.skip) = (window.start, window.skip);
        self.shown = window.shown;
        Placement {
            shows_cursor: window.holds(cursor) && window.cursor < visible,
            window,
        }
    }
}

// ========================================================================
// Placement
// ========================================================================

impl Placement {
    /// `frame` with the window's rows laid in from frame row `first` (the
    /// rows above it, a heading, each one screen row): the screen rows of
    /// the first scrolled past left unpainted, and the cursor's row banded
    /// where it shows.
    pub fn apply(&self, frame: Frame, first: usize) -> Frame {
        self.band(frame.with_skipped(first, self.window.skip), first)
    }

    /// `frame` with the cursor's row banded where it shows, for rows laid in
    /// from screen row `first` by an editor that drops the screen rows
    /// scrolled past itself, each of its rows painting one screen row.
    pub fn band(&self, frame: Frame, first: usize) -> Frame {
        match self.shows_cursor {
            true => frame.with_cursor_line(first + self.window.cursor),
            false => frame,
        }
    }
}

// ========================================================================
// RowSearch
// ========================================================================

impl RowSearch {
    /// Remember `query` as the last search, without moving.
    pub fn set(&mut self, query: &str) {
        self.query = query.to_string();
    }

    /// The last text searched for.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The row of `labels` after `cursor` (before it, unless `forward`)
    /// holding the last search, wrapping round, noting the search for the
    /// host to highlight. What to tell the reader when there is none, or no
    /// search to repeat.
    pub fn step(&mut self, labels: &[impl AsRef<str>], cursor: usize, forward: bool) -> Result<usize, Notice> {
        if self.query.is_empty() {
            return Err(Notice::error("no search"));
        }
        let found = find_match(labels, &self.query, cursor, forward);
        self.report = Some(row_search_report(labels, &self.query, found.unwrap_or(cursor), false));
        found.ok_or_else(|| Notice::error(format!("not found: {}", self.query)))
    }

    /// The search run or repeated since the host last asked, taken.
    pub fn take_report(&mut self) -> Option<SearchReport> {
        self.report.take()
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
            search: RowSearch::default(),
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

    /// Every row's key, in the list's order.
    pub fn keys(&self) -> &[K] {
        &self.keys
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

    /// Where the cursor and the window are, for a tab to keep while another
    /// tab on the list is acted on.
    pub fn saved_place(&self) -> ListPlace<K> {
        ListPlace {
            cursor: self.cursor,
            key: self.selected().cloned(),
            view: self.view.clone(),
        }
    }

    /// Put the cursor and the window back where [`ListView::saved_place`]
    /// found them: on the same row by its key, or where that row is gone, at
    /// the same index, as [`ListView::set_keys`] keeps it.
    pub fn restore_place(&mut self, place: ListPlace<K>) {
        self.view = place.view;
        self.cursor = place.cursor;
        if let Some(key) = place.key {
            self.select(&key);
        }
        self.clamp();
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
    /// rows: a row, a page, either end, a place on the screen, or the
    /// cursor's row placed in the pane. `false`
    /// for the rest (words, paragraphs, columns), which each editor reads over
    /// its own rows.
    pub fn apply_motion(&mut self, motion: CursorMove) -> bool {
        let last = self.keys.len().saturating_sub(1);
        let rows = self.view.rows;
        let half = (rows / 2).max(1) as isize;
        let page = rows.max(1) as isize;
        let shown = self.view.shown;
        match motion {
            CursorMove::Down => self.move_by(1),
            CursorMove::Up => self.move_by(-1),
            CursorMove::Top => self.cursor = 0,
            CursorMove::Bottom => self.cursor = last,
            CursorMove::HalfPageDown => self.move_by(half),
            CursorMove::HalfPageUp => self.move_by(-half),
            CursorMove::PageDown => self.move_by(page),
            CursorMove::PageUp => self.move_by(-page),
            CursorMove::ScreenTop => self.cursor = shown.top.min(last),
            CursorMove::ScreenMiddle => self.cursor = shown.middle.min(last),
            CursorMove::ScreenBottom => self.cursor = shown.bottom.min(last),
            CursorMove::LineToBottom => self.view.request(Place::Bottom),
            CursorMove::LineToCenter => self.view.request(Place::Middle),
            CursorMove::LineToTop => self.view.request(Place::Top),
            // Words, paragraphs, and columns mean something different over
            // each editor's rows, so the editor reads those itself.
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
        self.search.set(query);
    }

    /// Move to the next or previous row holding the last search, by the
    /// rows' `labels`. What to tell the reader comes back, when the search
    /// went nowhere.
    pub fn search_step(&mut self, labels: &[impl AsRef<str>], forward: bool) -> Option<Notice> {
        match self.search.step(labels, self.cursor, forward) {
            Ok(index) => {
                self.cursor = index;
                None
            }
            Err(notice) => Some(notice),
        }
    }

    /// The last text searched for.
    pub fn search(&self) -> &str {
        self.search.query()
    }

    /// The search run or repeated since the host last asked, taken.
    pub fn take_search(&mut self) -> Option<SearchReport> {
        self.search.take_report()
    }

    // --------------------------------------------------------------------
    // Jumps
    // --------------------------------------------------------------------

    /// Preview a jump to row `target`, remembering where the cursor was the
    /// first time so a cancelled jump goes back there.
    pub fn jump_to(&mut self, target: usize) {
        if self.jump_origin.is_none() {
            self.jump_origin = Some((self.cursor, self.view.clone()));
        }
        self.land(target);
    }

    /// Go back to where the cursor was before the preview.
    pub fn jump_cancel(&mut self) {
        if let Some((cursor, view)) = self.jump_origin.take() {
            self.set_cursor(cursor);
            self.view = view;
        }
    }

    /// Settle on row `target`.
    pub fn jump_confirm(&mut self, target: usize) {
        self.jump_origin = None;
        self.land(target);
    }

    /// Take one step through a jump, as an editor showing the list is
    /// offered it.
    pub fn on_jump(&mut self, step: JumpStep) {
        match step {
            JumpStep::Cancel => self.jump_cancel(),
            JumpStep::Confirm(target) => self.jump_confirm(target),
            JumpStep::Preview(target) => self.jump_to(target),
        }
    }

    /// Whether a jump preview is under way.
    #[cfg(test)]
    pub fn is_jumping(&self) -> bool {
        self.jump_origin.is_some()
    }

    /// Put the cursor on `target`, to show in the middle of the pane.
    fn land(&mut self, target: usize) {
        self.set_cursor(target);
        self.view.request(Place::Middle);
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
    fn test_a_restored_place_finds_its_row_in_rows_read_again_since() {
        let mut view = list(&["a", "b", "c"]);
        view.set_cursor(1);
        let kept = view.saved_place();
        view.set_cursor(2);
        view.set_keys(vec!["new", "a", "b", "c"]);
        view.restore_place(kept.clone());
        assert_eq!(view.selected(), Some(&"b"), "on b, though it moved down");
        view.set_keys(vec!["a"]);
        view.restore_place(kept);
        assert_eq!(view.cursor(), 0, "b gone: its index, clamped");
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
            "the editor's own"
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
        view.place(10, |_| 1);
        assert_eq!((view.cursor(), view.view().scroll()), (45, 40), "the target in the middle");
        view.jump_cancel();
        view.place(10, |_| 1);
        assert_eq!((view.cursor(), view.view().scroll()), (3, 0));
        assert!(!view.is_jumping());
    }

    #[test]
    fn test_the_wheel_holds_the_view_until_a_key_releases_it() {
        let mut view = list(&["a"; 50]);
        view.view_mut().set_rows(10);
        view.view_mut().wheel(-20);
        let held = view.place(10, |_| 1);
        assert!(!held.shows_cursor, "the cursor scrolled out of view");
        assert_eq!(held.window.start, 20);
        view.view_mut().release();
        let placed = view.place(10, |_| 1);
        assert!(placed.shows_cursor);
        assert_eq!(placed.window.start, 0);
    }

    #[test]
    fn test_over_wrapped_rows_the_view_moves_by_screen_rows() {
        // Rows of three screen rows in a 10-row pane. Regression: the wheel
        // counted each row as one screen row and stepped a whole row a turn,
        // and the cursor past the bottom scrolled by every row of the top one.
        let mut view = list(&["a"; 20]);
        let height = |_: usize| 3;
        view.view_mut().wheel(-1);
        let placed = view.place(10, height);
        assert_eq!((placed.window.start, placed.window.skip), (0, 1), "one screen row down");
        assert!(placed.shows_cursor, "the cursor's row still partly shows");
        view.view_mut().release();
        view.set_cursor(3);
        let placed = view.place(10, height);
        assert_eq!((placed.window.start, placed.window.skip), (0, 2), "its last screen row on the bottom");
        assert_eq!(placed.window.cursor, 7);
        assert_eq!(placed.window.shown, Shown { bottom: 3, middle: 2, top: 0 });
        // A placement counts screen rows too: `zt`, then `zb`.
        assert!(view.apply_motion(CursorMove::LineToTop));
        assert_eq!(view.place(10, height).window.start, 3);
        view.apply_motion(CursorMove::LineToBottom);
        let placed = view.place(10, height);
        assert_eq!((placed.window.start, placed.window.skip), (0, 0), "nine screen rows above its top");
        // And `L` goes to the row on the bottom screen row.
        view.place(10, height);
        view.apply_motion(CursorMove::ScreenBottom);
        assert_eq!(view.cursor(), 3);
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

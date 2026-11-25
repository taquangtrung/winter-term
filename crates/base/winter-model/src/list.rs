//! The window an editor's rows are read through, and the cursor a list keeps on
//! them: the state every editor painting rows would otherwise keep, and get
//! subtly different, on its own.
//!
//! [`Viewport`] is the window alone: how tall the pane is, where it is
//! scrolled to, whether the wheel is holding it there, and which screen row
//! of a wrapped cursor row the cursor is on, where `j` and `k` go by screen
//! rows ([`ViewportSettings::screen_row_motions`]). An editor with a
//! cursor of its own (an editable buffer) or with none (a report read top to
//! bottom) uses just this. [`ListView`] adds the cursor over a list of rows,
//! kept on the same row by its key when the list is rebuilt under it, with
//! the search and the jump preview a list offers. [`RowCursor`] is the
//! cursor and its window with no key per row, for an editor whose rows are
//! no keyed list (a build's growing output); [`RowSearch`] is the search
//! alone, for the same.
//!
//! Every cursor over rows is a [`RowCursor`], alone or inside a
//! [`ListView`], and moves through [`RowCursor::apply_motion`]: there is no
//! other way to step one, so a motion means the same in every editor. Each
//! page hands its windows to the host (`Page::visit_viewports`), which tells
//! them the window's [`ViewportSettings`] before each paint.

use crate::editor::{
    JumpStep, Page, ScrollPosition, SearchReport, find_match, row_search_report,
};
use crate::input::CursorMove;
use crate::notice::SearchMiss;
use crate::paint::{
    CursorRows, Frame, Place, RowWindow, Shown, ViewAim, place_view, scroll_position_rows,
    top_at_screen_row,
};

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
    /// How many screen rows each row the last paint laid out took, from row
    /// `laid_out_from` on: what the scrollbar measures by between paints.
    /// Only those rows are measured, since measuring a row may mean painting
    /// it, so every other row counts as one screen row.
    laid_out: Vec<usize>,
    /// The first row `laid_out` holds the height of.
    laid_out_from: usize,
    /// The screen row of row `line_row` the cursor is on, while it is the
    /// cursor's: any other cursor row is on its first.
    line: usize,
    /// The row `line` is a screen row of.
    line_row: usize,
    /// How many screen rows the view keeps above and below the cursor's row
    /// as it follows it, where the rows allow.
    margin: usize,
    /// Where the next paint is to put the cursor's row, as `zt`, `zz`, `zb`,
    /// or a jump asked.
    place: Option<Place>,
    /// How many screen rows the pane showed at the last paint. Zero until the
    /// first paint, when there is nothing to page by.
    rows: usize,
    /// Whether `j` and `k` go by screen rows through a wrapped row, the
    /// cursor then one of its screen rows rather than all of them.
    screen_rows: bool,
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

/// The window's settings every [`Viewport`] follows, told before each
/// paint.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ViewportSettings {
    /// How many screen rows the view keeps above and below the cursor as it
    /// follows it, where the rows allow: the window's `scroll-margin`.
    pub margin: usize,
    /// Whether `j` and `k` go by screen rows through a wrapped row, as
    /// Vim's `gj` and `gk` do: the window's `screen-row-motions`.
    pub screen_row_motions: bool,
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
    /// Whether a jump preview is actively targeting the list.
    pub is_jumping: bool,
    /// Whether the band covers its screen row alone, not the rest of a
    /// wrapped row: where the cursor is one screen row of it.
    band_alone: bool,
    /// The screen row the cursor band goes on, counted from the window's
    /// first painted screen row.
    band_row: usize,
    /// Whether the window holds the cursor, which is when an editor paints its
    /// cursor band.
    pub shows_cursor: bool,
    /// The rows to paint.
    pub window: RowWindow,
}

/// A cursor over rows and the window they are read through, with no key
/// per row: the one way any editor keeps a cursor on rows and moves it.
#[derive(Clone, Debug, Default)]
pub struct RowCursor {
    /// The row the cursor is on.
    cursor: usize,
    /// The window over the rows.
    view: Viewport,
}

/// A cursor over a list of rows, each named by a key of type `K`: the key is
/// what keeps the cursor on the same row when the list is read again and
/// rows come or go above it.
#[derive(Clone, Debug)]
pub struct ListView<K> {
    /// Where the cursor and the window were when a jump preview started, put
    /// back if it is cancelled.
    jump_origin: Option<RowCursor>,
    /// One key per row, in order.
    keys: Vec<K>,
    /// The cursor and the window over the rows.
    rows: RowCursor,
    /// The last search, repeated by the next and previous keys.
    search: RowSearch,
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

    /// Follow the window's `settings` from the next paint on: only the host
    /// tells them, through [`configure_viewports`].
    pub(crate) fn configure(&mut self, settings: ViewportSettings) {
        self.margin = settings.margin;
        self.screen_rows = settings.screen_row_motions;
    }

    /// The row `j` (`forward`) or `k` takes the cursor on row `cursor` of
    /// `total` to by screen rows, keeping which of its screen rows it is on:
    /// the next one of a wrapped row, else the first of the next row (the
    /// last of the one before, going up). `None` where motions go by rows.
    fn step_screen_row(&mut self, cursor: usize, total: usize, forward: bool) -> Option<usize> {
        if !self.screen_rows || total == 0 {
            return None;
        }
        let line = self.line_of(cursor);
        let height = self.height_of(cursor);
        let (row, line) = match forward {
            true if line + 1 < height => (cursor, line + 1),
            true if cursor + 1 < total => (cursor + 1, 0),
            false if line > 0 => (cursor, line - 1),
            false if cursor > 0 => (cursor - 1, self.height_of(cursor - 1) - 1),
            _ => (cursor, line),
        };
        (self.line_row, self.line) = (row, line);
        Some(row)
    }

    /// The screen row of row `cursor` the cursor is on.
    fn line_of(&self, cursor: usize) -> usize {
        match self.screen_rows && self.line_row == cursor {
            true => self.line,
            false => 0,
        }
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
        let at = self.scroll.saturating_add_signed(delta);
        self.set_scroll(at.min(total.saturating_sub(self.rows)));
        self.held = true;
    }

    /// Start the window `at` screen rows down `total` rows, as the scrollbar
    /// measures them (see [`Viewport::scroll_position`]), held inside the
    /// last screenful.
    pub fn scroll_to(&mut self, at: usize, total: usize) {
        (self.scroll, self.skip) =
            top_at_screen_row(at, total, self.rows, |row| self.height_of(row));
        self.held = true;
    }

    /// Where the viewport sits in `total` rows, for the scrollbar, in screen
    /// rows as the last paint laid them out.
    pub fn scroll_position(&self, total: usize) -> Option<ScrollPosition> {
        scroll_position_rows((self.scroll, self.skip), total, self.rows, |row| {
            self.height_of(row)
        })
    }

    /// How many screen rows `row` took at the last paint: one for a row it
    /// did not lay out.
    fn height_of(&self, row: usize) -> usize {
        row.checked_sub(self.laid_out_from)
            .and_then(|at| self.laid_out.get(at))
            .copied()
            .unwrap_or(1)
    }

    /// Lay `total` rows out in `visible` screen rows around `cursor`, each
    /// `height(row)` screen rows tall, and start the window there: where
    /// the wheel turned it to, or where a placement asked, or else moved the
    /// least that shows the cursor's row, whole where it fits, with the
    /// margin round it. Only a [`RowCursor`] places one around a cursor, so
    /// no editor keeps a cursor of its own beside a window.
    fn place(
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
        // The cursor is its whole row, first screen row to last, unless it
        // goes by screen rows: then it is the one it is on.
        let line = self.line_of(cursor).min(height(cursor).max(1) - 1);
        let rows = match (total, self.screen_rows) {
            (0, _) => CursorRows {
                first: (0, 0),
                last: (0, 0),
            },
            (_, true) => CursorRows {
                first: (cursor, line),
                last: (cursor, line),
            },
            (_, false) => CursorRows {
                first: (cursor, 0),
                last: (cursor, height(cursor).max(1) - 1),
            },
        };
        let wheel = std::mem::take(&mut self.wheel);
        let window = place_view(
            (self.scroll, self.skip),
            wheel,
            aim,
            rows,
            total,
            visible,
            self.margin,
            &height,
        );
        (self.scroll, self.skip) = (window.start, window.skip);
        self.laid_out_from = window.start;
        self.laid_out.clear();
        self.laid_out
            .extend((window.start..window.start + window.count).map(&height));
        self.shown = window.shown;
        // A whole row bands from where it starts showing; one screen row of
        // it shows only if not scrolled past, the first row showing having
        // dropped those.
        let past = match cursor == window.start {
            true => window.skip,
            false => 0,
        };
        let band_row = match self.screen_rows {
            true => (window.cursor + line).checked_sub(past),
            false => Some(window.cursor),
        };
        Placement {
            band_alone: self.screen_rows,
            band_row: band_row.unwrap_or(0),
            is_jumping: false,
            shows_cursor: window.holds(cursor) && band_row.is_some_and(|row| row < visible),
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
    /// scrolled past itself, each of its rows painting one screen row: the
    /// screen row the cursor is on alone, where it goes by screen rows.
    pub fn band(&self, frame: Frame, first: usize) -> Frame {
        match (self.shows_cursor, self.is_jumping) {
            (true, true) => frame.with_jump_target(first + self.window.cursor),
            (true, false) => {
                let frame = frame.with_cursor_line(first + self.band_row);
                match self.band_alone {
                    true => frame.with_cursor_line_alone(),
                    false => frame,
                }
            }
            (false, _) => frame,
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
    pub fn step(
        &mut self,
        labels: &[impl AsRef<str>],
        cursor: usize,
        forward: bool,
    ) -> Result<usize, SearchMiss> {
        if self.query.is_empty() {
            return Err(SearchMiss::NoSearch);
        }
        let found = find_match(labels, &self.query, cursor, forward);
        self.report = Some(row_search_report(
            labels,
            &self.query,
            found.unwrap_or(cursor),
            false,
        ));
        found.ok_or_else(|| SearchMiss::NotFound(self.query.clone()))
    }

    /// The search run or repeated since the host last asked, taken.
    pub fn take_report(&mut self) -> Option<SearchReport> {
        self.report.take()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Tell every window over `page`'s rows the window's `settings`, before
/// each paint: the one way they reach a [`Viewport`].
pub fn configure_viewports(page: &mut dyn Page, settings: ViewportSettings) {
    page.visit_viewports(&mut |viewport| viewport.configure(settings));
}

// ========================================================================
// RowCursor
// ========================================================================

impl RowCursor {
    /// A cursor on the first row, its window not yet painted.
    pub fn new() -> Self {
        Self::default()
    }

    /// The row the cursor is on.
    pub fn cursor(&self) -> usize {
        self.cursor
    }

    /// Put the cursor on row `at` of `total`, clamped to them.
    pub fn set_cursor(&mut self, at: usize, total: usize) {
        self.cursor = at.min(total.saturating_sub(1));
    }

    /// The window over the rows.
    pub fn view(&self) -> &Viewport {
        &self.view
    }

    /// The window over the rows, to scroll or place.
    pub fn view_mut(&mut self) -> &mut Viewport {
        &mut self.view
    }

    /// Step the cursor `delta` whole rows of `total`, clamped to them: for an
    /// action that goes on to the next row (marking one), never a motion,
    /// which goes through [`RowCursor::apply_motion`].
    pub fn step_rows(&mut self, delta: isize, total: usize) {
        let last = total.saturating_sub(1);
        self.cursor = self.cursor.saturating_add_signed(delta).min(last);
    }

    /// Carry out one of the motions that mean the same over any `total`
    /// rows: a row (a screen row of a wrapped one, where the window says
    /// so), a page, either end, a place on the screen, or the cursor's row
    /// placed in the pane. `false` for the rest (words, paragraphs,
    /// columns), which each editor reads over its own rows.
    pub fn apply_motion(&mut self, motion: CursorMove, total: usize) -> bool {
        let last = total.saturating_sub(1);
        let rows = self.view.rows;
        let half = (rows / 2).max(1) as isize;
        let page = rows.max(1) as isize;
        let shown = self.view.shown;
        match motion {
            CursorMove::Down | CursorMove::Up => {
                let forward = motion == CursorMove::Down;
                match self.view.step_screen_row(self.cursor, total, forward) {
                    Some(row) => self.cursor = row,
                    None => self.step_rows(if forward { 1 } else { -1 }, total),
                }
            }
            CursorMove::Top => self.cursor = 0,
            CursorMove::Bottom => self.cursor = last,
            CursorMove::HalfPageDown => self.step_rows(half, total),
            CursorMove::HalfPageUp => self.step_rows(-half, total),
            CursorMove::PageDown => self.step_rows(page, total),
            CursorMove::PageUp => self.step_rows(-page, total),
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

    /// Lay `total` rows out around the cursor (see [`Viewport::place`]).
    pub fn place(
        &mut self,
        total: usize,
        visible: usize,
        height: impl Fn(usize) -> usize,
    ) -> Placement {
        self.view.place(self.cursor, total, visible, height)
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
            jump_origin: None,
            keys: Vec::new(),
            rows: RowCursor::new(),
            search: RowSearch::default(),
        }
    }

    /// Take a new list of rows, one key each. The cursor stays on the row
    /// whose key it was on; where that row is gone it stays at the same
    /// index, clamped to the list, which lands on the row that took its place.
    pub fn set_keys(&mut self, keys: Vec<K>) {
        let selected = self.keys.get(self.rows.cursor).cloned();
        self.keys = keys;
        if let Some(index) = selected.and_then(|key| self.position(&key)) {
            self.rows.cursor = index;
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
        self.keys.get(self.rows.cursor)
    }

    /// Where the row named `key` is.
    pub fn position(&self, key: &K) -> Option<usize> {
        self.keys.iter().position(|candidate| candidate == key)
    }

    /// The row the cursor is on.
    pub fn cursor(&self) -> usize {
        self.rows.cursor
    }

    /// Put the cursor on row `at`, clamped to the list.
    pub fn set_cursor(&mut self, at: usize) {
        self.rows.cursor = at;
        self.clamp();
    }

    /// Put the cursor on the row named `key`, where there is one.
    pub fn select(&mut self, key: &K) -> bool {
        match self.position(key) {
            Some(index) => {
                self.rows.cursor = index;
                true
            }
            None => false,
        }
    }

    /// The window over the rows.
    pub fn view(&self) -> &Viewport {
        &self.rows.view
    }

    /// The window over the rows, to scroll or place.
    pub fn view_mut(&mut self) -> &mut Viewport {
        &mut self.rows.view
    }

    /// Lay the rows out around the cursor (see [`Viewport::place`]).
    pub fn place(&mut self, visible: usize, height: impl Fn(usize) -> usize) -> Placement {
        let mut placement = self.rows.place(self.keys.len(), visible, height);
        placement.is_jumping = self.jump_origin.is_some();
        placement
    }

    /// Where the list view sits in its rows, for the scrollbar.
    pub fn scroll_position(&self) -> Option<ScrollPosition> {
        self.rows.view.scroll_position(self.keys.len())
    }

    /// Scroll to row `at`.
    pub fn scroll_to(&mut self, at: usize) {
        self.rows.view.scroll_to(at, self.keys.len());
    }

    /// Where the cursor and the window are, for a tab to keep while another
    /// tab on the list is acted on.
    pub fn saved_place(&self) -> ListPlace<K> {
        ListPlace {
            cursor: self.rows.cursor,
            key: self.selected().cloned(),
            view: self.rows.view.clone(),
        }
    }

    /// Put the cursor and the window back where [`ListView::saved_place`]
    /// found them: on the same row by its key, or where that row is gone, at
    /// the same index, as [`ListView::set_keys`] keeps it.
    pub fn restore_place(&mut self, place: ListPlace<K>) {
        self.rows.view = place.view;
        self.rows.cursor = place.cursor;
        if let Some(key) = place.key {
            self.select(&key);
        }
        self.clamp();
    }

    fn clamp(&mut self) {
        self.rows.cursor = self.rows.cursor.min(self.keys.len().saturating_sub(1));
    }

    // --------------------------------------------------------------------
    // Motions
    // --------------------------------------------------------------------

    /// Step the cursor `delta` whole rows (see [`RowCursor::step_rows`]).
    pub fn step_rows(&mut self, delta: isize) {
        self.rows.step_rows(delta, self.keys.len());
    }

    /// Carry out a motion over the rows (see [`RowCursor::apply_motion`]).
    pub fn apply_motion(&mut self, motion: CursorMove) -> bool {
        self.rows.apply_motion(motion, self.keys.len())
    }

    // --------------------------------------------------------------------
    // Search
    // --------------------------------------------------------------------

    /// Remember `query` and move to the next row whose label holds it.
    pub fn search_for(&mut self, labels: &[impl AsRef<str>], query: &str) -> Option<SearchMiss> {
        self.search_with(labels, query, true)
    }

    /// Remember `query` and step to the next or previous row holding it, for
    /// a search one list carries on from another.
    pub fn search_with(
        &mut self,
        labels: &[impl AsRef<str>],
        query: &str,
        forward: bool,
    ) -> Option<SearchMiss> {
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
    pub fn search_step(&mut self, labels: &[impl AsRef<str>], forward: bool) -> Option<SearchMiss> {
        match self.search.step(labels, self.rows.cursor, forward) {
            Ok(index) => {
                self.rows.cursor = index;
                None
            }
            Err(miss) => Some(miss),
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
            self.jump_origin = Some(self.rows.clone());
        }
        self.set_cursor(target);
        self.rows.view.request(Place::CenterInUncovered);
    }

    /// Go back to where the cursor was before the preview.
    pub fn jump_cancel(&mut self) {
        if let Some(rows) = self.jump_origin.take() {
            self.rows = rows;
            self.clamp();
        }
    }

    /// Settle on row `target`.
    pub fn jump_confirm(&mut self, target: usize) {
        self.jump_origin = None;
        self.set_cursor(target);
        self.rows.view.request(Place::Middle);
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
    #[cfg(any(test, feature = "test-support"))]
    pub fn is_jumping(&self) -> bool {
        self.jump_origin.is_some()
    }
}

impl<K: Clone + PartialEq> Default for ListView<K> {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Test support
// ========================================================================

/// What every editor drawing rows through a [`Viewport`] tests its motions
/// by.
#[cfg(any(test, feature = "test-support"))]
pub mod test_support {
    use super::*;
    use crate::input::KeyCode;
    use crate::input::test_support::key;

    /// Assert that `page`, painted `rows` by `cols` with wrapping on, its
    /// cursor on a row taking more than one screen row, moves as the
    /// window's `screen-row-motions` says: off, `j` passes the whole row;
    /// on, `j` bands its next screen row alone and `k` the one it left. Every editor with a cursor over rows calls it in its tests, so
    /// none moves its own way.
    pub fn assert_steps_screen_rows(page: &mut dyn Page, rows: usize, cols: usize) {
        let band = |page: &mut dyn Page, screen_row_motions: bool| {
            let settings = ViewportSettings {
                margin: 0,
                screen_row_motions,
            };
            configure_viewports(page, settings);
            let frame = page.content(rows, cols, true);
            (frame.cursor_line.expect("a cursor band"), frame.cursor_line_alone)
        };
        let press = |page: &mut dyn Page, code: char| {
            page.on_key(&key(KeyCode::Char(code)));
        };
        let (start, _) = band(page, false);
        press(page, 'j');
        let (passed, alone) = band(page, false);
        assert!(
            passed > start + 1,
            "the cursor's row takes one screen row (banded at {start}, then {passed}): \
             the test needs one that wraps"
        );
        assert!(!alone, "off, the band covers the whole row");
        press(page, 'k');
        assert_eq!(band(page, false).0, start, "off, k comes back to the row");
        press(page, 'j');
        press(page, 'k');
        assert_eq!(band(page, true).0, start);
        press(page, 'j');
        assert_eq!(band(page, true), (start + 1, true), "on, j bands the next screen row alone");
        press(page, 'k');
        assert_eq!(band(page, true), (start, true), "on, k bands the first again");
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
    fn test_screen_row_motions_step_through_a_wrapped_row_and_band_its_screen_row() {
        // Row b wraps onto three screen rows; a and c take one each.
        let height = |row: usize| if row == 1 { 3 } else { 1 };
        let mut view = list(&["a", "b", "c"]);
        view.view_mut().configure(ViewportSettings {
            margin: 0,
            screen_row_motions: true,
        });
        let down = |view: &mut ListView<&'static str>| {
            view.apply_motion(CursorMove::Down);
            let placed = view.place(10, height);
            (view.cursor(), placed.band_row, placed.band_alone)
        };
        view.place(10, height);
        assert_eq!(down(&mut view), (1, 1, true), "b's first screen row");
        assert_eq!(down(&mut view), (1, 2, true), "still b, its second");
        assert_eq!(down(&mut view), (1, 3, true));
        assert_eq!(down(&mut view), (2, 4, true), "on to c");
        view.apply_motion(CursorMove::Up);
        assert_eq!(view.cursor(), 1);
        assert_eq!(view.place(10, height).band_row, 3, "b's last screen row going up");
        view.apply_motion(CursorMove::Top);
        view.apply_motion(CursorMove::Down);
        assert_eq!(view.place(10, height).band_row, 1, "any other move lands on a first");
    }

    #[test]
    fn test_without_screen_row_motions_a_wrapped_row_is_one_stop_banded_whole() {
        let height = |row: usize| if row == 1 { 3 } else { 1 };
        let mut view = list(&["a", "b", "c"]);
        view.place(10, height);
        view.apply_motion(CursorMove::Down);
        view.apply_motion(CursorMove::Down);
        let placed = view.place(10, height);
        assert_eq!(view.cursor(), 2, "b passed in one step");
        assert_eq!((placed.band_row, placed.band_alone), (4, false));
    }

    #[test]
    fn test_a_search_steps_through_its_matches_and_reports_a_miss() {
        let labels = ["alpha", "beta", "alphabet"];
        let mut view = list(&["0", "1", "2"]);
        assert_eq!(view.search_step(&labels, true), Some(SearchMiss::NoSearch));
        assert_eq!(view.search_for(&labels, "alpha"), None);
        assert_eq!(view.cursor(), 2, "the next match after the cursor");
        view.search_step(&labels, true);
        assert_eq!(view.cursor(), 0, "wrapping round");
        let report = view.take_search().expect("a report");
        assert_eq!((report.index, report.total), (1, 2));
        assert_eq!(
            view.search_for(&labels, "gamma"),
            Some(SearchMiss::NotFound("gamma".to_string()))
        );
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
        assert_eq!(
            (view.cursor(), view.view().scroll()),
            (45, 40),
            "the target in the middle"
        );
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
        assert_eq!(
            (placed.window.start, placed.window.skip),
            (0, 1),
            "one screen row down"
        );
        assert!(placed.shows_cursor, "the cursor's row still partly shows");
        view.view_mut().release();
        view.set_cursor(3);
        let placed = view.place(10, height);
        assert_eq!(
            (placed.window.start, placed.window.skip),
            (0, 2),
            "its last screen row on the bottom"
        );
        assert_eq!(placed.window.cursor, 7);
        assert_eq!(
            placed.window.shown,
            Shown {
                bottom: 3,
                middle: 2,
                top: 0
            }
        );
        // A placement counts screen rows too: `zt`, then `zb`.
        assert!(view.apply_motion(CursorMove::LineToTop));
        assert_eq!(view.place(10, height).window.start, 3);
        view.apply_motion(CursorMove::LineToBottom);
        let placed = view.place(10, height);
        assert_eq!(
            (placed.window.start, placed.window.skip),
            (0, 0),
            "nine screen rows above its top"
        );
        // And `L` goes to the row on the bottom screen row.
        view.place(10, height);
        view.apply_motion(CursorMove::ScreenBottom);
        assert_eq!(view.cursor(), 3);
    }

    #[test]
    fn test_the_scrollbar_counts_the_screen_rows_a_paint_laid_out() {
        // The first four rows wrap to three screen rows each in a 10-row
        // pane; the other sixteen were not laid out and count one each.
        // Regression: the scrollbar counted every row as one screen row.
        let mut view = list(&["a"; 20]);
        view.view_mut().set_rows(10);
        view.place(10, |row| if row < 4 { 3 } else { 1 });
        let pos = view.scroll_position().unwrap();
        assert_eq!((pos.offset, pos.visible, pos.total), (0, 10, 28));
        // A drag to screen row 4 lands one screen row into the second row.
        view.scroll_to(4);
        assert_eq!(view.view().scroll(), 1);
        assert_eq!(view.scroll_position().unwrap().offset, 4);
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

//! [`PageView`]: a page drawn by misty as a view, laid out and styled as
//! misty lays out an app's window (columns, rows, forms, tables, controls)
//! in the page's pane. A page holds one, built from the state the view
//! reads, and answers the view hooks of [`Page`](super::Page) through it:
//! [`Page::is_view`](super::Page::is_view) with `true`, and
//! [`Page::draw_view`](super::Page::draw_view),
//! [`Page::view_input`](super::Page::view_input), and
//! [`Page::view_about_to_wait`](super::Page::view_about_to_wait) by calling
//! the methods of the same names here.
//!
//! A page whose state is the page itself, too woven through its methods to
//! move into a [`PageView`], implements [`OwnView`] instead and keeps the
//! view in a [`ViewSlot`].
//!
//! The view's callbacks change the state alone; what the page asks of the
//! window, it keeps in the state and hands over from
//! [`Page::take_requests`](super::Page::take_requests).
//!
//! A page's keys are its keymap's, a block of `keybindings.kdl` the user
//! may rebind, so by default the view takes the pointer alone: a button
//! clicked keeps no keyboard focus that would take `RET` from the page's
//! own `RET`. A page whose view holds controls typed into (a form) makes
//! it with [`PageView::with_keys`] instead, its focused control taking the
//! keys first.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use misty::{AccessAction, EventCx, FocusId, Frame, HostedView, Input, Rect, Theme, ViewHost};
use misty::{ScrollStep, WheelDelta, WheelInput};

// ========================================================================
// Constants
// ========================================================================

/// The id every page view's ids are made under, a child of it each.
const PAGE_VIEWS: FocusId = FocusId::new("winter.page");

/// How many pixels of a smooth wheel scroll a [`RowWindow`] a row.
pub const PIXELS_PER_ROW: f32 = 24.0;

// ========================================================================
// Globals
// ========================================================================

/// How many page views were made, so each takes ids no other one has: two
/// panes showing pages of one kind keep their focus apart.
static MADE: AtomicU64 = AtomicU64::new(0);

// ========================================================================
// Data Structures
// ========================================================================

/// A page's view and the state it is built from.
#[derive(Debug)]
pub struct PageView<S> {
    /// What lasts of the view between frames: its widgets, focus, scroll.
    host: ViewHost<S>,
    /// Whether the view takes keys, or the pointer alone.
    keys: bool,
    /// The state the view reads and its callbacks change.
    state: S,
}

// ========================================================================
// PageView Implementations
// ========================================================================

impl<S: HostedView> PageView<S> {
    /// A view of `state` taking the pointer alone, the keys left to the
    /// page's keymap; its ids apart from every other page view's.
    pub fn new(state: S) -> Self {
        let made = MADE.fetch_add(1, Ordering::Relaxed);
        Self {
            host: ViewHost::new(PAGE_VIEWS.child(made)),
            keys: false,
            state,
        }
    }

    /// A view of `state` whose focused control takes the keys first, what
    /// it leaves going on to the page's keymap.
    pub fn with_keys(state: S) -> Self {
        Self {
            keys: true,
            ..Self::new(state)
        }
    }

    /// The state the view is built from.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// The same, to change; the next frame shows the change.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }

    /// Draw the view over `area` of `frame` in `theme`.
    pub fn draw_view(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        self.host.draw(&self.state, frame, area, theme);
    }

    /// Hand `input` to the view: whether it used it. A key, to a view
    /// taking the pointer alone, is never used.
    pub fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> bool {
        let key = matches!(input, Input::Key(_) | Input::KeyReleased(_) | Input::Ime(_));
        if key && !self.keys {
            return false;
        }
        self.host.handle(&mut self.state, input, cx)
    }

    /// Do what a screen reader's `action` asks of the view: whether one of
    /// its elements took it.
    pub fn access_action(&mut self, action: AccessAction) -> bool {
        self.host.access_action(&mut self.state, action)
    }

    /// Step the view's animations and timers: when it next needs a step.
    pub fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.host.about_to_wait(&mut self.state, cx)
    }

    /// Give the keyboard to the view's element named `name` once it is
    /// next drawn.
    pub fn focus(&mut self, name: impl Into<String>) {
        self.host.focus(name);
    }

    /// Move the keyboard `delta` controls on (back for a negative
    /// `delta`), as Tab and Shift+Tab do, for the page's own motions.
    pub fn move_focus(&mut self, delta: isize) {
        self.host.move_focus(delta);
    }

    /// Scroll the view's scrolling container named `name` by `step`, for
    /// the page's own motions; whether it moved.
    pub fn scroll(&mut self, name: &str, step: ScrollStep) -> bool {
        self.host.scroll(name, step)
    }

    /// How far down the scrolling container named `name` is scrolled.
    pub fn offset(&mut self, name: &str) -> Option<f32> {
        self.host.offset(name)
    }

    /// Where the element named `name` starts down its scrolling content.
    pub fn top_of(&self, name: &str) -> Option<f32> {
        self.host.top_of(name)
    }

    /// Scroll the element named `name` to the top of its scrolling
    /// content; whether that moved it.
    pub fn reveal_top(&mut self, name: &str) -> bool {
        self.host.reveal_top(name)
    }
}

// ========================================================================
// ViewSlot
// ========================================================================

/// Where a page that is its own view's state ([`OwnView`]) keeps the view:
/// for a page whose state is the page itself, too woven through its
/// methods to move into a [`PageView`]. The page lends itself to the view
/// for each call, the view taken out of the slot meanwhile.
#[derive(Debug)]
pub struct ViewSlot<S> {
    /// The view, but while it is lent the page.
    host: Option<ViewHost<S>>,
    /// The id the view's ids are made under.
    id: FocusId,
}

impl<S: HostedView> ViewSlot<S> {
    /// A slot for a view taking the pointer alone, the keys left to the
    /// page's keymap, as [`PageView::new`]'s does; its ids apart from every
    /// other page view's.
    pub fn new() -> Self {
        let id = PAGE_VIEWS.child(MADE.fetch_add(1, Ordering::Relaxed));
        Self {
            host: Some(ViewHost::new(id)),
            id,
        }
    }

    /// The view, taken out to lend the page to it: a fresh one should a
    /// panic have left the slot empty.
    fn take(&mut self) -> ViewHost<S> {
        self.host.take().unwrap_or_else(|| ViewHost::new(self.id))
    }
}

impl<S: HostedView> Default for ViewSlot<S> {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Traits
// ========================================================================

/// A page that is its own view's state: it builds the view from itself
/// ([`HostedView::view`]) and keeps the view in a [`ViewSlot`], and its
/// [`Page`](super::Page) view hooks call [`OwnView::draw_own_view`],
/// [`OwnView::own_view_input`], and [`OwnView::own_view_about_to_wait`]. As with a [`PageView::new`], the view takes the pointer alone.
pub trait OwnView: HostedView {
    /// Where the page keeps its view.
    fn view_slot(&mut self) -> &mut ViewSlot<Self>;

    /// Draw the view over `area` of `frame` in `theme`.
    fn draw_own_view(&mut self, frame: &mut Frame, area: Rect, theme: &Theme) {
        let mut host = self.view_slot().take();
        host.draw(self, frame, area, theme);
        self.view_slot().host = Some(host);
    }

    /// Hand `input` to the view, a key never: whether it used it.
    fn own_view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> bool {
        if matches!(input, Input::Key(_) | Input::KeyReleased(_) | Input::Ime(_)) {
            return false;
        }
        let mut host = self.view_slot().take();
        let used = host.handle(self, input, cx);
        self.view_slot().host = Some(host);
        used
    }

    /// Do what a screen reader's `action` asks of the view: whether one of
    /// its elements took it.
    fn own_access_action(&mut self, action: AccessAction) -> bool {
        let mut host = self.view_slot().take();
        let used = host.access_action(self, action);
        self.view_slot().host = Some(host);
        used
    }

    /// Step the view's animations and timers: when it next needs a step.
    fn own_view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        let mut host = self.view_slot().take();
        let due = host.about_to_wait(self, cx);
        self.view_slot().host = Some(host);
        due
    }
}

// ========================================================================
// RowWindow
// ========================================================================

/// The rows of a long list a page view builds: those from its top that fit
/// the pane, so a view over hundreds of rows builds, lays out, and draws a
/// screenful each frame rather than every row. The page moves the cursor
/// with its keys and calls [`RowWindow::follow`] after, which scrolls the
/// least that keeps the cursor in sight; the wheel scrolls the window away
/// from it ([`RowWindow::wheel`]) until the next key.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RowWindow {
    /// How many rows fit the pane.
    fits: usize,
    /// Whether the wheel put the window where it is, away from the cursor.
    held: bool,
    /// The wheel's turn not yet a whole row, in rows.
    pending: f32,
    /// The first row built.
    top: usize,
}

impl RowWindow {
    /// Note how many rows fit the pane, at least one.
    pub fn set_fits(&mut self, fits: usize) {
        self.fits = fits.max(1);
    }

    /// The rows to build of `total`, from the top.
    pub fn range(&self, total: usize) -> std::ops::Range<usize> {
        let top = self.top.min(total.saturating_sub(1));
        top..(top + self.fits.max(1)).min(total)
    }

    /// Scroll the least that shows row `cursor` of `total`, unless the
    /// wheel holds the window; a key releases it ([`RowWindow::release`]).
    pub fn follow(&mut self, cursor: usize, total: usize) {
        let fits = self.fits.max(1);
        if !self.held {
            if cursor < self.top {
                self.top = cursor;
            } else if cursor >= self.top + fits {
                self.top = cursor + 1 - fits;
            }
        }
        self.top = self.top.min(total.saturating_sub(fits));
    }

    /// A key was pressed: the cursor decides where the window is again.
    pub fn release(&mut self) {
        (self.held, self.pending) = (false, 0.0);
    }

    /// Turn the window by `wheel` over `total` rows: a notch a row, a
    /// smooth wheel's pixels a row each [`PIXELS_PER_ROW`]; positive turns
    /// scroll toward the top. The window then stays until a key.
    pub fn wheel(&mut self, wheel: &WheelInput, total: usize) {
        let rows = match wheel.delta {
            WheelDelta::Lines(lines) => lines.y,
            WheelDelta::Pixels(pixels) => pixels.y / PIXELS_PER_ROW,
            // A kind of turn misty adds later scrolls nothing.
            _ => 0.0,
        };
        self.pending -= rows;
        let whole = self.pending.trunc();
        self.pending -= whole;
        let last = total.saturating_sub(self.fits.max(1));
        self.top = self.top.saturating_add_signed(whole as isize).min(last);
        self.held = true;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::{Mods, Point};

    use super::*;

    fn window(fits: usize) -> RowWindow {
        let mut window = RowWindow::default();
        window.set_fits(fits);
        window
    }

    fn wheel(delta: WheelDelta) -> WheelInput {
        WheelInput {
            delta,
            mods: Mods::NONE,
            position: Point::new(0.0, 0.0),
        }
    }

    #[test]
    fn test_the_window_scrolls_the_least_that_keeps_the_cursor_in_sight() {
        let mut rows = window(10);
        rows.follow(14, 100);
        assert_eq!(rows.range(100), 5..15, "the cursor on the last row");
        rows.follow(3, 100);
        assert_eq!(rows.range(100), 3..13, "and on the first");
        rows.follow(99, 100);
        assert_eq!(rows.range(100), 90..100);
        assert_eq!(window(10).range(4), 0..4, "a short list whole");
    }

    #[test]
    fn test_the_wheel_holds_the_window_away_from_the_cursor_until_a_key() {
        let mut rows = window(10);
        rows.wheel(&wheel(WheelDelta::Lines(Point::new(0.0, -3.0))), 100);
        rows.follow(0, 100);
        assert_eq!(rows.range(100), 3..13, "held by the wheel");
        rows.release();
        rows.follow(0, 100);
        assert_eq!(rows.range(100), 0..10, "back to the cursor");
    }

    #[test]
    fn test_a_smooth_wheels_pixels_add_up_to_whole_rows() {
        let mut rows = window(10);
        let half = WheelDelta::Pixels(Point::new(0.0, -PIXELS_PER_ROW / 2.0));
        rows.wheel(&wheel(half), 100);
        assert_eq!(rows.range(100), 0..10, "half a row yet");
        rows.wheel(&wheel(half), 100);
        assert_eq!(rows.range(100), 1..11);
    }
}

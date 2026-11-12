//! Winter as misty's application: misty's inputs dispatched to Winter's
//! handlers, a frame drawn into a texture misty lays over the window, and
//! the work done between them before the loop sleeps until the next deadline.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use misty::{AppContext, Application, Frame, Input, WindowId};
use winit::dpi::PhysicalSize;

use winter_model::guard::{describe_panic, install_panic_hook};
use winter_model::input;
use winter_model::mode::Mode;

use crate::window::appearance::theme_for;
use crate::window::input::misty as misty_input;
use crate::window::input::sequence::is_hint_redraw_due;
use crate::window::{App, CURSOR_BLINK_PERIOD};

// ========================================================================
// Constants
// ========================================================================

const PTY_POLL_INTERVAL: Duration = Duration::from_millis(16);
/// Poll interval used once `PTY_ACTIVE_WINDOW` has passed with no output, input,
/// or other window activity. `about_to_wait` still has no way to be woken
/// directly by a pane's PTY-reader thread, so it can't stop polling outright;
/// backing off to this from the 16ms cadence once idle is what keeps a
/// sitting-there terminal from holding the CPU out of its deeper idle states
/// around the clock.
const PTY_POLL_INTERVAL_IDLE: Duration = Duration::from_millis(250);
/// How long since the last output/input/window activity before `about_to_wait`
/// backs off to `PTY_POLL_INTERVAL_IDLE`. Long enough that normal think-time
/// between commands doesn't visibly change responsiveness, short enough that
/// a command finishing and printing while the user has stepped away still
/// shows up promptly once they're back.
const PTY_ACTIVE_WINDOW: Duration = Duration::from_secs(2);

/// How many panics the window carries on from within [`PANIC_WINDOW`]
/// before it lets the next one take it down: past that it is panicking on
/// every frame, and carrying on would only spin.
const MAX_PANICS_IN_WINDOW: usize = 5;
const PANIC_WINDOW: Duration = Duration::from_secs(10);

thread_local! {
    /// When the window last carried on from a panic, within [`PANIC_WINDOW`].
    static RECENT_PANICS: RefCell<Vec<Instant>> = const { RefCell::new(Vec::new()) };
}

// ========================================================================
// Free functions
// ========================================================================

/// Whether `about_to_wait` should back off to `PTY_POLL_INTERVAL_IDLE`:
/// `PTY_ACTIVE_WINDOW` has elapsed since `last_activity` as of `now`.
fn is_poll_idle(last_activity: Instant, now: Instant) -> bool {
    now.saturating_duration_since(last_activity) >= PTY_ACTIVE_WINDOW
}

/// Whether the blink timer should be running: it is configured on *and* the
/// window has focus. An unfocused window draws its cursor in the steady "not
/// receiving keystrokes" form, which does not consult the blink phase, so
/// flipping it there cannot change a pixel and every redraw it asked for was
/// waste a backgrounded window paid around the clock.
fn is_blink_active(blink_configured: bool, window_focused: bool) -> bool {
    blink_configured && window_focused
}

// ========================================================================
// App: Application
// ========================================================================

impl Application for App {
    type Message = ();

    fn init(&mut self, cx: &mut AppContext<'_, ()>, window: WindowId) {
        self.connect_waker(cx.waker());
        self.survive(|app| app.init_window(cx, window));
        self.flush_window(cx);
    }

    fn input(&mut self, cx: &mut AppContext<'_, ()>, _window: WindowId, input: &Input) {
        self.survive(|app| app.handle_input(cx, input));
        self.flush_window(cx);
    }

    fn about_to_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        if let Some(window) = &self.window {
            window.note_geometry(cx.window_geometry(window.id()));
        }
        let mut wakeup = None;
        self.survive(|app| wakeup = app.prepare_wait(cx));
        let window = self.window.as_ref().map(|window| window.id());
        if let Some(window) = window {
            let cards = self.cards_deadline(&mut cx.event_cx(window));
            let tooltip = self.chrome.tooltip.about_to_wait(&mut cx.event_cx(window));
            let strips = self.strips_deadline(&mut cx.event_cx(window));
            wakeup = [wakeup, cards, tooltip, strips].into_iter().flatten().min();
        }
        self.flush_window(cx);
        wakeup
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        self.survive(|app| app.draw_into(frame));
    }
}

// ========================================================================
// App: the handlers, run under the window's guard
// ========================================================================

impl App {
    /// Run `handler` so that a panic in it, wherever in the window it was
    /// raised, is written to the crash log, told as an error notification,
    /// and the window goes on: losing every terminal in it to one bug is
    /// worse than carrying on from a frame that did not finish. Only a panic
    /// repeating many times over a few seconds, which carrying on would only
    /// spin on, is let take the window down.
    fn survive(&mut self, handler: impl FnOnce(&mut Self)) {
        install_panic_hook();
        let Err(payload) = panic::catch_unwind(AssertUnwindSafe(|| handler(self))) else {
            return;
        };
        let now = Instant::now();
        let repeats = RECENT_PANICS.with(|recent| {
            let mut recent = recent.borrow_mut();
            recent.retain(|at| now.duration_since(*at) < PANIC_WINDOW);
            recent.push(now);
            recent.len()
        });
        if repeats > MAX_PANICS_IN_WINDOW {
            panic::resume_unwind(payload);
        }
        let why = describe_panic(payload.as_ref());
        let log = winter_base::crash_log::path()
            .map(|path| format!("; details in {}", path.display()))
            .unwrap_or_default();
        self.set_error(format!(
            "Winter hit an internal error and carried on: {why}{log}"
        ));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// Hand what the window was asked to misty.
    fn flush_window(&mut self, cx: &mut AppContext<'_, ()>) {
        if let Some(window) = &self.window {
            window.flush(cx);
        }
    }

    /// Draw a frame into `frame`: the renderer draws into a texture the size
    /// of the window, which the frame then lays over the whole window. A new
    /// size or scale resizes the renderer and the panes first.
    fn draw_into(&mut self, frame: &mut Frame) {
        let bounds = frame.bounds();
        let size = PhysicalSize::new(bounds.width as u32, bounds.height as u32);
        if size.width == 0 || size.height == 0 {
            return;
        }
        let scale_factor = frame.scale_factor();
        if let Some(window) = &self.window {
            window.note_metrics(frame.cell_metrics());
        }
        let changed = self
            .window
            .as_ref()
            .is_some_and(|window| window.note_frame(size, scale_factor, frame.is_maximized()));
        if changed {
            self.resize_window(size, scale_factor);
        }
        self.render_frame();
        self.sync_strips();
        self.draw_panes(frame);
        self.draw_strips(frame);
        self.draw_settings(frame);
        self.draw_title_bar(frame);
        self.draw_status_bar(frame);
        self.draw_pane_popups(frame);
        self.draw_which_key(frame);
        self.draw_transient(frame);
        self.draw_cards(frame);
        self.draw_picker(frame);
        self.draw_menus(frame);
        self.draw_tooltip(frame);
    }

    /// Resize the renderer and the panes to a window of `size` at
    /// `scale_factor`.
    fn resize_window(&mut self, size: PhysicalSize<u32>, scale_factor: f64) {
        let resized = self
            .renderer
            .as_mut()
            .and_then(|renderer| renderer.resize(size.width, size.height, scale_factor));
        if resized.is_some() {
            self.resize_all_panes();
            self.dirty = true;
            self.runtime.note_geometry_change(Instant::now());
        }
    }

    fn handle_input(&mut self, cx: &mut AppContext<'_, ()>, input: &Input) {
        // Every input counts as activity for the `about_to_wait` poll
        // back-off.
        self.runtime.last_activity = Instant::now();
        let window = self.window.as_ref().map(|window| window.id());
        if let Some(window) = window {
            self.input.modifiers = misty_input::winit_modifiers(cx.event_cx(window).mods());
        }
        let pointer = matches!(
            input,
            Input::PointerButton(_) | Input::PointerMoved(_) | Input::PointerLeft | Input::Wheel(_)
        );
        // Where the pointer is, kept even when a misty widget takes the move:
        // the tooltip and the title bar's hits read it.
        if let Some(point) = input.position() {
            self.input.pointer.cursor_pos = (point.x, point.y);
        }
        // The bar lights its buttons, and switches the open menu, before
        // an open menu takes the pointer moving.
        let moved = matches!(input, Input::PointerMoved(_));
        let picked = window.is_some_and(|window| {
            let mut ecx = cx.event_cx(window);
            moved && self.offer_to_title_bar(input, &mut ecx)
                || self.offer_to_menus(input, &mut ecx)
                || pointer
                    && (!moved && self.offer_to_title_bar(input, &mut ecx)
                        || self.point_at_picker(input, &mut ecx)
                        || self.point_at_transient(input, &mut ecx)
                        || self.point_at_which_key(input, &mut ecx)
                        || self.point_at_cards(input, &mut ecx)
                        || self.point_at_status_bar(input, &mut ecx)
                        || self.offer_to_strips(input, &mut ecx))
                || self.offer_to_settings(input, &mut ecx)
        });
        if picked {
            self.quit_if_requested(cx);
        }
        match input {
            _ if picked => {}
            Input::Key(key) => self.on_key(*key, true, cx),
            Input::KeyReleased(key) => self.on_key(*key, false, cx),
            Input::Ime(ime) => self.on_ime(ime, cx),

            Input::PointerButton(button) => {
                let (state, winit_button) = misty_input::winit_button(button);
                self.on_cursor_moved(misty_input::winit_position(button.position));
                self.on_mouse_input(state, winit_button, cx);
            }
            Input::PointerMoved(point) => self.on_cursor_moved(misty_input::winit_position(*point)),
            Input::PointerLeft => self.on_cursor_left(),
            Input::Wheel(wheel) => {
                self.on_cursor_moved(misty_input::winit_position(wheel.position));
                self.on_mouse_wheel(misty_input::winit_wheel(wheel.delta));
            }

            // A dialog is answered first: closing the window from its title
            // bar or the window manager would put the quit question over it.
            Input::CloseRequested => {
                if !self.chrome.dialog_open() {
                    self.quit(cx);
                }
            }

            Input::FocusGained | Input::FocusLost => {
                let focused = *input == Input::FocusGained;
                if !focused {
                    self.input.keys.prefix = input::PendingPrefix::None;
                    self.input.clear_sequence();
                    self.input.keys.forwarded.clear();
                    self.chrome.tab_drag = None;
                }
                if self.input.focus.window != focused {
                    self.input.focus.window = focused;
                    // The blink timer is paused while unfocused, so
                    // `blink.next_flip` is stale by the time focus returns.
                    // Restart it from the visible phase, the same way a key
                    // press does, rather than letting the elapsed deadline
                    // blink the cursor off the instant the window is clicked.
                    if self.config.cursor.blink && focused {
                        self.blink.phase = true;
                        self.blink.next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
                    }
                    if focused {
                        self.tell_editors_of_focus();
                    }
                    self.dirty = true;
                }
            }

            Input::AppearanceChanged(appearance) => {
                if let Some(window) = &self.window {
                    window.note_color_scheme(appearance.color_scheme);
                }
                if let Some(renderer) = &mut self.renderer {
                    if self.config.theme == crate::config::ThemeSetting::Auto {
                        let mut colors = theme_for(appearance.color_scheme);
                        self.config.colors.apply(&mut colors);
                        renderer.set_theme(colors);
                        self.sync_translucency();
                        self.dirty = true;
                    }
                }
            }

            Input::FileDropped(_) | Input::FileHovered(_) | Input::FileHoverCancelled => {}
        }
        // What was typed into an open picker's query reaches it now, with
        // the window's context.
        if let Some(window) = window {
            self.type_into_picker(&mut cx.event_cx(window));
        }
        // A key, a click, or the window's own focus may have moved the
        // keyboard; the programs that asked hear where it went.
        self.sync_focus_reports();
        // Or started an editor's prefix, whose hint waits from now.
        self.sync_editor_hint();
    }

    /// The work done between events before the loop sleeps until the next
    /// deadline.
    fn prepare_wait(&mut self, cx: &mut AppContext<'_, ()>) -> Option<Instant> {
        if self.reload_config_if_changed() {
            self.dirty = true;
        }

        self.serve_control_requests();
        if self.runtime.exit.reload_pending {
            self.runtime.exit.reload_pending = false;
            self.reload(cx);
            return None;
        }

        self.reap_dead_panes();
        // What extensions' slow work handed back, run on this thread; the
        // queries their pickers settled on; what the editors show that
        // extensions have not heard of; and a repaint an extension asked for
        // from another thread.
        self.run_extension_replies();
        self.tell_extension_queries();
        let extension_tick = self.watch_for_extensions();
        if crate::extensions::take_repaint() {
            self.dirty = true;
        }
        // A tab closed on its own hands the keyboard on.
        self.sync_focus_reports();
        if self.quit_if_requested(cx) {
            return None;
        }
        let any_output = self.drain_all_panes();

        if any_output {
            self.runtime.last_activity = Instant::now();
            // A Vim prompt edit just echoed back: re-seed the nav cursor onto the
            // shell's new cursor position so it tracks the edited line.
            if self.term.nav_resync_pending {
                let focused = self.layout().focused();
                if self.open_tabs.mode(&focused) == Some(Mode::Normal) {
                    self.init_nav_cursor(focused);
                }
                self.term.nav_resync_pending = false;
            }
            self.dirty = true;
            self.update_window_title();
        }

        // Toasts whose time is up come down; the loop wakes for the next.
        let toast_tick = self.expire_toasts(Instant::now());

        // Blinking text runs on its own timer, for as long as some is on
        // screen.
        if self
            .blink
            .text
            .tick(Instant::now(), self.is_text_blink_active())
        {
            self.dirty = true;
        }

        // Flip cursor blink phase when the timer fires, requesting a redraw to
        // show the updated state.
        if is_blink_active(self.config.cursor.blink, self.input.focus.window) {
            let now = Instant::now();
            if now >= self.blink.next_flip {
                self.blink.phase = !self.blink.phase;
                self.blink.next_flip = now + CURSOR_BLINK_PERIOD;
                self.dirty = true;
            }
        }

        // One frame to show the which-key hint once its delay is up, and no
        // more: the frame that drew it is what shows it from then on.
        if let Some(deadline) = self.hint_deadline() {
            if is_hint_redraw_due(deadline, Instant::now(), self.runtime.last_frame_at) {
                self.dirty = true;
            }
        }

        self.auto_scroll_selection();
        if self.runtime.take_geometry_save_due(Instant::now()) {
            self.save_app_state();
        }

        // Answers from editor jobs. Each worker wakes the loop as it answers,
        // so a running job needs no faster poll to be heard.
        if self.deliver_finished_jobs() {
            self.dirty = true;
        }
        // A preview following a file being written takes its text.
        if self.feed_followers() {
            self.dirty = true;
        }
        // An editor a panic stopped while painting is told of at once, not
        // at the next key its tab hears.
        if self.announce_stopped_editors() {
            self.dirty = true;
        }
        // What the language servers said, handed to the editors it is for.
        if self.deliver_lsp() {
            self.dirty = true;
        }
        if self.quota.as_mut().is_some_and(|quota| quota.drain()) {
            self.dirty = true;
        }
        // Editors that keep themselves current (the process monitor) ask for
        // their next refresh here, and the loop wakes for the earliest.
        let editor_tick = self.tick_editors(Instant::now());
        // What the editors asked of the extensions' inline sources, as typing
        // paused or a key asked.
        if self.call_extension_inlines() {
            self.dirty = true;
        }
        let picker_tick = self.tick_picker(Instant::now());
        if self.sync_progress() {
            self.dirty = true;
        }
        let spinner_tick = self.update_spinners(Instant::now());

        // Typing into a search or an editor's question, handed on once it
        // pauses.
        let search_tick = self.settle_search(Instant::now());
        let prompt_tick = self.settle_prompt_input(Instant::now());

        // Turn the app-wide "what is drawn no longer matches the state" flag
        // into the one redraw request it stands for. This is the only place a
        // redraw is requested: every change sets `dirty` and leaves the frame
        // to this. Taken rather than read, so one request is made per batch
        // of changes; winit coalesces a redraw already pending.
        if let Some(window) = &self.window {
            if std::mem::take(&mut self.dirty) {
                window.request_redraw();
            }
        }

        let now = Instant::now();
        let poll_interval = if is_poll_idle(self.runtime.last_activity, now) {
            PTY_POLL_INTERVAL_IDLE
        } else {
            PTY_POLL_INTERVAL
        };
        let next_poll = now + poll_interval;
        let mut wakeup = if is_blink_active(self.config.cursor.blink, self.input.focus.window) {
            next_poll.min(self.blink.next_flip)
        } else {
            next_poll
        };
        if let Some(deadline) = self.blink.text.deadline(self.is_text_blink_active()) {
            wakeup = wakeup.min(deadline);
        }
        for deadline in [search_tick, prompt_tick].into_iter().flatten() {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = editor_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = extension_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = self.step_wheel(now) {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = picker_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = spinner_tick {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = toast_tick {
            wakeup = wakeup.min(deadline);
        }
        // A yank's highlight goes out on its own deadline, and the loop wakes
        // for it rather than leaving it lit until the next PTY poll.
        if let Some(deadline) = self.tick_focus_flash(now) {
            wakeup = wakeup.min(deadline);
        }
        if let Some(deadline) = self.expire_yank_flash() {
            wakeup = wakeup.min(deadline);
        }
        // If any terminal is holding back synchronized output, wake up promptly
        // when its hold deadline expires so the frame is released without lag.
        for (_, terminal) in self.open_tabs.terminals() {
            if let Some(deadline) = terminal.sync_hold_deadline() {
                wakeup = wakeup.min(deadline);
            }
        }
        if let Some(deadline) = self.hint_deadline().filter(|deadline| *deadline > now) {
            wakeup = wakeup.min(deadline);
        }
        if let Some(due) = self.runtime.config_watch.reload_due {
            wakeup = wakeup.min(due);
        }
        if let Some(due) = self.runtime.geometry_save_due {
            wakeup = wakeup.min(due);
        }
        Some(wakeup)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_poll_idle_false_before_active_window_elapses() {
        let last_activity = Instant::now();
        let now = last_activity + PTY_ACTIVE_WINDOW - Duration::from_millis(1);
        assert!(!is_poll_idle(last_activity, now));
    }

    #[test]
    fn test_is_poll_idle_true_once_active_window_elapses() {
        let last_activity = Instant::now();
        let now = last_activity + PTY_ACTIVE_WINDOW;
        assert!(is_poll_idle(last_activity, now));
    }

    #[test]
    fn test_blink_needs_both_config_and_window_focus() {
        // Dropping either term is the regression this guards. Losing the focus
        // term puts a backgrounded window back on a 530ms redraw it cannot
        // show, which is what kept `PTY_POLL_INTERVAL_IDLE` unreachable;
        // losing the config term blinks for users who turned blinking off.
        assert!(is_blink_active(true, true));
        assert!(!is_blink_active(true, false));
        assert!(!is_blink_active(false, true));
        assert!(!is_blink_active(false, false));
    }

    #[test]
    fn test_a_panic_in_a_handler_is_told_and_the_window_carries_on_until_it_repeats_without_end() {
        let mut app = App::new();
        app.survive(|_| panic!("handler gone wrong"));
        let told = app
            .chrome
            .notifier
            .log
            .iter()
            .any(|entry| entry.text.contains("carried on: handler gone wrong"));
        assert!(told, "told as an error notification");
        app.survive(|app| app.dirty = true);
        assert!(app.dirty, "and the window still runs its handlers");

        for _ in 1..MAX_PANICS_IN_WINDOW {
            app.survive(|_| panic!("again"));
        }
        let last = panic::catch_unwind(AssertUnwindSafe(|| {
            app.survive(|_| panic!("once too often"))
        }));
        assert!(
            last.is_err(),
            "a panic on every frame is let take the window down"
        );
    }
}

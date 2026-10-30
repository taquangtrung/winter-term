//! The winit event loop: the window events dispatched to their handlers, and
//! the work done between them before the loop sleeps until the next deadline.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow};
use winit::window::WindowId;
use winter_render::Theme;

use crate::model::guard::{describe_panic, install_panic_hook};
use crate::model::input;
use crate::model::mode::Mode;

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
// App: ApplicationHandler
// ========================================================================

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        self.init_window(event_loop);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, window_id: WindowId, event: WindowEvent) {
        self.survive(|app| app.handle_window_event(event_loop, window_id, event));
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.survive(|app| app.prepare_wait(event_loop));
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
        let log = crate::crash_log::path().map(|path| format!("; details in {}", path.display())).unwrap_or_default();
        self.set_error(format!("Winter hit an internal error and carried on: {why}{log}"));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    fn handle_window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Every window event except a repaint counts as activity for the
        // `about_to_wait` poll back-off. `RedrawRequested` is excluded because
        // the app is what asks for those: the cursor blink alone requests one
        // roughly twice a second, so counting them kept `last_activity`
        // permanently fresher than `PTY_ACTIVE_WINDOW` and pinned the loop to
        // `PTY_POLL_INTERVAL` forever, leaving `PTY_POLL_INTERVAL_IDLE`
        // unreachable with the default config.
        if !matches!(event, WindowEvent::RedrawRequested) {
            self.runtime.last_activity = Instant::now();
        }
        match event {
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    let mut size_changed = false;
                    if let Some(renderer) = &mut self.renderer {
                        if let Some(window) = &self.window {
                            let scale_factor = window.scale_factor();
                            if renderer
                                .resize(size.width, size.height, scale_factor)
                                .is_some()
                            {
                                size_changed = true;
                            }
                        }
                    }
                    if size_changed {
                        self.resize_all_panes();
                        self.dirty = true;
                        // Windows reports a bogus small size (the legacy "iconic"
                        // size, not 0x0) when the window is minimized; skip
                        // persisting it so a later restart doesn't restore a
                        // near-unusable window.
                        let minimized = self
                            .window
                            .as_ref()
                            .and_then(|w| w.is_minimized())
                            .unwrap_or(false);
                        if !minimized {
                            self.runtime.note_geometry_change(Instant::now());
                        }
                    }
                }
            }

            WindowEvent::Moved(_) => {
                let minimized = self
                    .window
                    .as_ref()
                    .and_then(|w| w.is_minimized())
                    .unwrap_or(false);
                if !minimized {
                    self.runtime.note_geometry_change(Instant::now());
                }
            }

            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                let size = self.window.as_ref().map(|w| w.inner_size());
                if let Some(size) = size {
                    let mut changed = false;
                    if let Some(renderer) = &mut self.renderer {
                        if renderer
                            .resize(size.width, size.height, scale_factor)
                            .is_some()
                        {
                            changed = true;
                        }
                    }
                    if changed {
                        self.resize_all_panes();
                        self.dirty = true;
                    }
                }
            }

            WindowEvent::ModifiersChanged(mods) => {
                self.input.modifiers = mods;
                // winit always fires ModifiersChanged after the synthesized
                // KeyboardInput::Pressed events it issues on XI_FocusIn, so
                // this is the right moment to stop suppressing. X11-only quirk:
                // on other platforms `suppress_synthesized_keys` is never set,
                // so this reset is a no-op there.
                #[cfg(target_os = "linux")]
                {
                    self.input.suppress_synthesized_keys = false;
                }
            }

            WindowEvent::KeyboardInput { event, .. } => self.on_keyboard_input(event, event_loop),
            WindowEvent::Ime(ime) => self.on_ime(ime, event_loop),

            WindowEvent::MouseInput { state, button, .. } => {
                self.on_mouse_input(state, button, event_loop)
            }

            WindowEvent::CursorMoved { position, .. } => self.on_cursor_moved(position),
            WindowEvent::CursorLeft { .. } => self.on_cursor_left(),

            WindowEvent::MouseWheel { delta, .. } => self.on_mouse_wheel(delta),

            WindowEvent::RedrawRequested => {
                self.render_frame();
            }

            WindowEvent::CloseRequested => {
                self.quit(event_loop);
            }

            WindowEvent::Focused(focused) => {
                // Suppressing synthesized presses on focus-in is an X11-only
                // workaround (see the `ModifiersChanged` arm above): winit's
                // Windows and macOS backends never synthesize those presses,
                // and never fire a compensating `ModifiersChanged` either, so
                // setting this flag there would swallow every keystroke until
                // a modifier key happened to be pressed.
                #[cfg(target_os = "linux")]
                if focused {
                    self.input.suppress_synthesized_keys = true;
                }
                if !focused {
                    self.input.keys.prefix = input::PendingPrefix::None;
                    self.input.clear_sequence();
                    self.input.keys.forwarded.clear();
                    self.tabs.drag_start = None;
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
                    self.dirty = true;
                }
            }

            WindowEvent::ThemeChanged(theme) => {
                if let Some(renderer) = &mut self.renderer {
                    if self.config.theme == crate::config::ThemeSetting::Auto {
                        let mut colors = match theme {
                            winit::window::Theme::Dark => Theme::dark(),
                            winit::window::Theme::Light => Theme::light(),
                        };
                        self.config.colors.apply(&mut colors);
                        renderer.set_theme(colors);
                        self.sync_translucency();
                        self.dirty = true;
                    }
                }
            }

            _ => {}
        }
        // A key, a click, or the window's own focus may have moved the
        // keyboard; the programs that asked hear where it went.
        self.sync_focus_reports();
        // Or started an editor's prefix, whose hint waits from now.
        self.sync_editor_hint();
    }

    /// The work done between events before the loop sleeps until the next
    /// deadline.
    fn prepare_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.reload_config_if_changed() {
            self.dirty = true;
        }

        self.serve_control_requests();
        if self.runtime.exit.reload_pending {
            self.runtime.exit.reload_pending = false;
            self.reload(event_loop);
            return;
        }

        self.reap_dead_panes();
        // What extensions' slow work handed back, run on this thread; what
        // the editors show that extensions have not heard of; and a repaint
        // an extension asked for from another thread.
        self.run_extension_replies();
        let extension_tick = self.watch_for_extensions();
        if crate::extensions::take_repaint() {
            self.dirty = true;
        }
        // A tab closed on its own hands the keyboard on.
        self.sync_focus_reports();
        if self.quit_if_requested(event_loop) {
            return;
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
        let live_search_tick = self.tick_live_search(Instant::now());
        let progress = self.host.job_progress_line(self.layout().focused());
        if progress != self.job_progress {
            self.job_progress = progress;
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
        if let Some(deadline) = live_search_tick {
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
        event_loop.set_control_flow(ControlFlow::WaitUntil(wakeup));
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
        let told = app.chrome.notifier.log.iter().any(|entry| entry.text.contains("carried on: handler gone wrong"));
        assert!(told, "told as an error notification");
        app.survive(|app| app.dirty = true);
        assert!(app.dirty, "and the window still runs its handlers");

        for _ in 1..MAX_PANICS_IN_WINDOW {
            app.survive(|_| panic!("again"));
        }
        let last = panic::catch_unwind(AssertUnwindSafe(|| app.survive(|_| panic!("once too often"))));
        assert!(last.is_err(), "a panic on every frame is let take the window down");
    }
}

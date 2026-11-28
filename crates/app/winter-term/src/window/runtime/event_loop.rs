//! Winter as misty's application: misty's inputs dispatched to Winter's
//! handlers, a frame drawn into a texture misty lays over the window, and
//! the work done between them before the loop sleeps until the next deadline.

use std::cell::RefCell;
use std::panic::{self, AssertUnwindSafe};
use std::time::{Duration, Instant};

use misty::{
    AccessAction, AppContext, Application, Frame, HangWatch, Input, KeyScheme, MeasureCx, Widget,
    WindowId,
};

use winter_model::guard::{describe_panic, install_panic_hook};
use winter_model::input;
use winter_model::mode::Mode;

use crate::watchdog;
use crate::window::appearance::{theme_for, ui_theme};
use crate::window::input::sequence::is_hint_redraw_due;
use crate::window::notice::{InternalError, WindowNotice};
use crate::window::{App, CURSOR_BLINK_PERIOD};

// ========================================================================
// Constants
// ========================================================================

/// How often the loop reads mux terminals while one is open, since their
/// server's writes wake nothing (see `prepare_wait`).
const MUX_POLL_INTERVAL: Duration = Duration::from_millis(16);

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

/// What `input` is, for the watchdog: its kind alone, never which key.
fn input_kind(input: &Input) -> &'static str {
    match input {
        Input::Key(_) => "input: key",
        Input::KeyReleased(_) => "input: key release",
        Input::Ime(_) => "input: input method",
        Input::PointerButton(_) => "input: click",
        Input::PointerMoved(_) | Input::PointerLeft => "input: pointer",
        Input::Wheel(_) => "input: wheel",
        Input::FileDropped(_) | Input::FileHovered(_) | Input::FileHoverCancelled => {
            "input: file drop"
        }
        Input::FocusGained | Input::FocusLost => "input: focus",
        Input::AppearanceChanged(_) => "input: appearance",
        Input::CloseRequested => "input: close",
        // A kind misty adds later.
        _ => "input: other",
    }
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
    type Message = crate::window::WindowMessage;

    fn message(&mut self, cx: &mut AppContext<'_, Self::Message>, message: Self::Message) {
        watchdog::busy("message");
        match message {
            crate::window::WindowMessage::Nothing => {}
            crate::window::WindowMessage::ShowTab(tab) => {
                self.survive(|app| app.show_tab(tab));
                if let Some(window) = self.window.as_ref().map(|window| window.id()) {
                    cx.event_cx(window).focus_window();
                }
            }
        }
        self.flush_window(cx);
    }

    fn init(&mut self, cx: &mut AppContext<'_, crate::window::WindowMessage>, window: WindowId) {
        watchdog::busy("init");
        self.connect_waker(cx.waker());
        self.survive(|app| app.init_window(cx, window));
        self.flush_window(cx);
    }

    fn input(
        &mut self,
        cx: &mut AppContext<'_, crate::window::WindowMessage>,
        _window: WindowId,
        input: &Input,
    ) {
        watchdog::busy(input_kind(input));
        self.survive(|app| app.handle_input(cx, input));
        self.flush_window(cx);
    }

    fn about_to_wait(
        &mut self,
        cx: &mut AppContext<'_, crate::window::WindowMessage>,
    ) -> Option<Instant> {
        watchdog::busy("about_to_wait");
        if let Some(window) = &self.window {
            window.note_geometry(cx.event_cx(window.id()).geometry());
        }
        let mut wakeup = None;
        self.survive(|app| wakeup = app.prepare_wait(cx));
        let window = self.window.as_ref().map(|window| window.id());
        if let Some(window) = window {
            let cards = self.cards_deadline(&mut cx.event_cx(window));
            let tooltip = self.parts.tooltip.about_to_wait(&mut cx.event_cx(window));
            let strips = self.strips_deadline(&mut cx.event_cx(window));
            let pages = self.page_views_deadline(&mut cx.event_cx(window));
            let texts = self.text_editors_deadline(&mut cx.event_cx(window));
            let scrollbars = self.scrollbars_deadline(&mut cx.event_cx(window));
            wakeup = [wakeup, cards, tooltip, strips, pages, texts, scrollbars]
                .into_iter()
                .flatten()
                .min();
        }
        self.flush_window(cx);
        // The window waits next: the turn is over.
        watchdog::idle(|| self.turn_detail());
        wakeup
    }

    /// A hang is reported to the crash log, and Winter started again if it
    /// lasts (see [`crate::watchdog`]).
    fn hang_watch(&self) -> HangWatch {
        watchdog::hang_watch()
    }

    /// Winter is keyboard-first: misty's lists, fields, and controls take
    /// the Vim and Emacs keys, as Winter's own views do.
    fn key_scheme(&self) -> KeyScheme {
        KeyScheme::VIM_AND_EMACS
    }

    fn render(&mut self, _window: WindowId, frame: &mut Frame) {
        watchdog::busy("render");
        self.survive(|app| app.draw_into(frame));
    }

    /// A screen reader's action goes to what described the node it names:
    /// a pane's strip, else a page drawn as a view.
    fn access_action(
        &mut self,
        _cx: &mut AppContext<'_, crate::window::WindowMessage>,
        _window: WindowId,
        action: AccessAction,
    ) {
        watchdog::busy("access_action");
        self.survive(|app| {
            if app.strips_access_action(action) || app.page_views_access_action(action) {
                app.dirty = true;
            }
        });
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
        self.notify(WindowNotice::InternalError(InternalError {
            log: winter_base::crash_log::path(),
            why: why.to_string(),
        }));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }

    /// What the window's focused tab is, for the watchdog's account of a
    /// slow turn: a terminal and the program in its foreground, or a page's
    /// kind. Never its contents.
    fn turn_detail(&self) -> String {
        let tab = self.layout().focused();
        if let Some(terminal) = self.open_tabs.terminal(&tab) {
            return match terminal.title_facts().program {
                Some(program) => format!("terminal running {program}"),
                None => "terminal".to_string(),
            };
        }
        match self.open_tabs.page(&tab) {
            Some(slot) => format!("{:?}", slot.kind()),
            None => "no tab".to_string(),
        }
    }

    /// Hand what the window was asked to misty.
    fn flush_window(&mut self, cx: &mut AppContext<'_, crate::window::WindowMessage>) {
        if let Some(window) = &self.window {
            window.flush(cx);
        }
    }

    /// Draw a frame into `frame`: the renderer draws into a texture the size
    /// of the window, which the frame then lays over the whole window. A new
    /// size or scale resizes the renderer and the panes first.
    fn draw_into(&mut self, frame: &mut Frame) {
        let started = Instant::now();
        self.draw_window(frame);
        crate::diagnose::note_frame(started.elapsed());
    }

    /// Build the window's frame into `frame`: the panes, the bars and popups around
    /// them, and whatever lies over them.
    fn draw_window(&mut self, frame: &mut Frame) {
        let bounds = frame.bounds();
        let (width, height) = (bounds.width as u32, bounds.height as u32);
        if width == 0 || height == 0 {
            return;
        }
        let scale_factor = frame.scale_factor();
        if let Some(window) = &self.window {
            let theme = ui_theme(self.renderer.as_ref());
            window.note_measure(MeasureCx::new(frame, theme));
        }
        let changed = self.window.as_ref().is_some_and(|window| {
            window.note_frame((width, height), scale_factor, frame.is_maximized())
        });
        if changed {
            self.resize_window((width, height), scale_factor);
        }
        self.render_frame();
        self.sync_strips();
        self.draw_panes(frame);
        self.draw_text_editors(frame);
        self.draw_toolbars(frame);
        // Before the strips, whose active tab's accent rises over its
        // bottom border.
        self.draw_title_bar(frame);
        self.draw_strips(frame);
        self.draw_page_views(frame);
        self.draw_find(frame);
        self.draw_token_card(frame);
        self.draw_status_bar(frame);
        // After the pages and the status bar, whose own edges would cover
        // the focused pane's bottom border.
        self.draw_separators(frame);
        self.draw_pane_popups(frame);
        self.draw_which_key(frame);
        self.draw_transient(frame);
        self.draw_cards(frame);
        self.draw_picker(frame);
        self.draw_dialog(frame);
        self.draw_menus(frame);
        self.draw_tooltip(frame);
        self.draw_window_outline(frame);
    }

    /// Draw the window's own outline over everything, when the system draws
    /// no frame for it.
    fn draw_window_outline(&self, frame: &mut Frame) {
        let Some(renderer) = &self.renderer else {
            return;
        };
        if renderer.is_decorated() {
            return;
        }
        let border = renderer.theme().window_border;
        frame.draw_window_outline(misty::Rgb::new(border.r, border.g, border.b));
    }

    /// Resize the renderer and the panes to a window `width` by `height`
    /// physical pixels at `scale_factor`.
    fn resize_window(&mut self, (width, height): (u32, u32), scale_factor: f64) {
        let resized = self
            .renderer
            .as_mut()
            .and_then(|renderer| renderer.resize(width, height, scale_factor));
        if resized.is_some() {
            self.resize_all_panes();
            self.dirty = true;
            self.runtime.note_geometry_change(Instant::now());
        }
    }

    fn handle_input(
        &mut self,
        cx: &mut AppContext<'_, crate::window::WindowMessage>,
        input: &Input,
    ) {
        let window = self.window.as_ref().map(|window| window.id());
        if let Some(window) = window {
            self.input.modifiers = cx.event_cx(window).mods();
        }
        let pointer = matches!(
            input,
            Input::PointerButton(_) | Input::PointerMoved(_) | Input::PointerLeft | Input::Wheel(_)
        );
        // A key, a press, or a scroll puts the tooltip away until the pointer
        // rests on something else.
        let pressed = matches!(input, Input::PointerButton(button) if button.pressed);
        if (matches!(input, Input::Key(_) | Input::Wheel(_)) || pressed)
            && self.parts.tooltip.dismiss()
        {
            self.dirty = true;
        }
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
                        || self.point_at_dialog(input, &mut ecx)
                        || self.point_at_picker(input, &mut ecx)
                        || self.point_at_transient(input, &mut ecx)
                        || self.point_at_which_key(input, &mut ecx)
                        || self.point_at_cards(input, &mut ecx)
                        || self.point_at_status_bar(input, &mut ecx)
                        || self.offer_to_strips(input, &mut ecx))
                || self.offer_to_page_views(input, &mut ecx)
                || self.offer_to_text_editors(input, &mut ecx)
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
                self.on_cursor_moved(button.position);
                self.on_mouse_input(button.pressed, button.button, cx);
            }
            Input::PointerMoved(point) => self.on_cursor_moved(*point),
            Input::PointerLeft => self.on_cursor_left(),
            Input::Wheel(wheel) => {
                self.on_cursor_moved(wheel.position);
                self.on_mouse_wheel(wheel.delta);
            }

            // A dialog is answered first: closing the window from its title
            // bar or the window manager would put the quit question over it.
            Input::CloseRequested => {
                if !self.parts.dialog_open() {
                    self.quit(cx);
                }
            }

            Input::FocusGained | Input::FocusLost => {
                let focused = *input == Input::FocusGained;
                if !focused {
                    self.input.keys.prefix = input::PendingPrefix::None;
                    self.input.clear_sequence();
                    self.input.keys.forwarded.clear();
                    self.parts.tab_drag = None;
                    self.on_cursor_left();
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
                if let Some(renderer) = &mut self.renderer
                    && self.config.theme == crate::config::ThemeSetting::Auto
                {
                    let mut colors = theme_for(appearance.color_scheme);
                    self.config.colors.apply(&mut colors);
                    renderer.set_theme(colors);
                    self.sync_translucency();
                    self.dirty = true;
                }
            }

            Input::FileDropped(_) | Input::FileHovered(_) | Input::FileHoverCancelled => {}
            // Input misty adds later is none Winter acts on yet.
            _ => {}
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
    fn prepare_wait(
        &mut self,
        cx: &mut AppContext<'_, crate::window::WindowMessage>,
    ) -> Option<Instant> {
        if self.reload_config_if_changed() {
            self.dirty = true;
        }

        self.serve_control_requests();
        self.collect_capture();
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
        if let Some(deadline) = self.hint_deadline()
            && is_hint_redraw_due(deadline, Instant::now(), self.runtime.last_frame_at)
        {
            self.dirty = true;
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
        // What the managed agents' thread found: a worktree made, the agents
        // listed. It wakes the loop as it answers.
        if self.take_agent_work() {
            self.dirty = true;
        }
        // What the language servers said, handed to the editors it is for.
        if self.deliver_lsp() {
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
        // of changes; misty coalesces a redraw already pending.
        if let Some(window) = &self.window
            && std::mem::take(&mut self.dirty)
        {
            window.request_redraw();
        }

        // Nothing is polled: each source wakes the loop when it has
        // something, or names the moment it next needs a turn here.
        let now = Instant::now();
        let blink = is_blink_active(self.config.cursor.blink, self.input.focus.window)
            .then_some(self.blink.next_flip);
        let text_blink = self.blink.text.deadline(self.is_text_blink_active());
        let wheel = self.step_wheel(now);
        let focus_flash = self.tick_focus_flash(now);
        let yank_flash = self.expire_yank_flash();
        let hint = self.hint_deadline().filter(|deadline| *deadline > now);
        let sync_hold = self
            .open_tabs
            .terminals()
            .filter_map(|(_, terminal)| terminal.sync_hold_deadline())
            .min();
        // A mux terminal's socket is read on this thread, and nothing wakes
        // the loop when its server writes: the one source still polled.
        let mux = self
            .open_tabs
            .terminals()
            .any(|(_, terminal)| terminal.mux_session().is_some())
            .then(|| now + MUX_POLL_INTERVAL);
        [
            blink,
            text_blink,
            search_tick,
            prompt_tick,
            editor_tick,
            extension_tick,
            wheel,
            picker_tick,
            spinner_tick,
            toast_tick,
            focus_flash,
            yank_flash,
            sync_hold,
            hint,
            self.runtime.config_watch.reload_due,
            self.runtime.geometry_save_due,
            self.auto_scroll_deadline(),
            self.host.jobs.next_long_at(),
            mux,
        ]
        .into_iter()
        .flatten()
        .min()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_quiet_window_asks_for_no_wakeup() {
        let mut window = misty::testing::TestApp::new(App::new(), "Winter", 800, 600);
        window.update(|app| app.config.cursor.blink = false);
        for _ in 0..3 {
            window.about_to_wait();
        }
        assert_eq!(
            window.about_to_wait(),
            None,
            "nothing to wake for, so no poll"
        );
    }

    #[test]
    fn test_blink_needs_both_config_and_window_focus() {
        // Dropping either term is the regression this guards. Losing the focus
        // term puts a backgrounded window back on a 530ms redraw it cannot
        // show;
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
            .parts
            .notifier
            .log()
            .iter()
            .any(|entry| entry.text().contains("carried on: handler gone wrong"));
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

/// Frame timings of a window of terminals drawn by the real renderer: one
/// pane, four panes, and four panes whose last line changes every frame, as
/// an agent's status line does. Ignored by `cargo test`; run them with
/// `cargo test --release -p winter-term bench_frame -- --ignored --nocapture`.
#[cfg(test)]
mod bench {
    use std::time::{Duration, Instant};

    use portable_pty::CommandBuilder;
    use winter_emulator::MAX_SCROLLBACK;
    use winter_model::layout::Direction;
    use winter_terminal::session::Terminal;

    use crate::render::renderer::{FontConfig, GpuRenderer};
    use crate::window::App;

    /// The window's size, in pixels.
    const SIZE: (u32, u32) = (1920, 1080);

    /// How many frames each timing averages over.
    const FRAMES: usize = 60;

    /// The cells each terminal is filled to.
    const GRID: (usize, usize) = (200, 60);

    /// A window of `panes` terminals, each its screen full of text, drawn by
    /// a renderer on a headless GPU; `None` where there is no GPU.
    fn window(panes: usize) -> Option<App> {
        let gpu = misty::testing::gpu()?;
        let mut app = App::new();
        let font = FontConfig {
            family: None,
            size: 13.0,
        };
        app.renderer = Some(GpuRenderer::new(gpu.clone(), SIZE.0, SIZE.1, 1.0, font));
        app.gpu = Some(gpu);
        for _ in 1..panes {
            let id = app.alloc_tab_id();
            app.layout_mut().split(Direction::Vertical, 0.5, id);
        }
        for id in app.layout().shown_tabs() {
            let mut terminal =
                Terminal::with_command(GRID.0, GRID.1, CommandBuilder::new("cat"), MAX_SCROLLBACK)
                    .expect("a terminal");
            for row in 0..GRID.1 {
                let line =
                    format!("{row:>4} fn render_frame(&mut self) {{ let x = {row} * 2; }}\r\n");
                terminal.feed_program_output(line.as_bytes());
            }
            app.open_tabs.insert_terminal(id, terminal);
        }
        Some(app)
    }

    /// The mean and slowest of `FRAMES` frames of `app`, each first changed
    /// by `change`: drawn into a plain frame (`gpu` false, the CPU's share),
    /// or rendered on the GPU and waited for, as a window's frame is, with
    /// nothing read back.
    fn time(app: &mut App, gpu: bool, mut change: impl FnMut(&mut App, usize)) -> (f64, f64) {
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        if gpu {
            let times = misty::testing::time_frames(SIZE.0, SIZE.1, FRAMES, |frame, number| {
                change(app, number);
                app.dirty = true;
                app.draw_into(frame);
            });
            return times.map_or((0.0, 0.0), |times| (ms(times.mean), ms(times.max)));
        }
        let window = misty::testing::TestWindow::new(SIZE.0, SIZE.1);
        let (mut total, mut slowest) = (Duration::ZERO, Duration::ZERO);
        for number in 0..FRAMES {
            change(app, number);
            app.dirty = true;
            let start = Instant::now();
            let mut frame = window.frame();
            app.draw_into(&mut frame);
            let took = start.elapsed();
            total += took;
            slowest = slowest.max(took);
        }
        (ms(total) / FRAMES as f64, ms(slowest))
    }

    /// Writes a changing last line into every pane, as a status line turns.
    fn turn(app: &mut App, frame: usize) {
        let ids = app.layout().shown_tabs();
        for id in ids {
            if let Some(terminal) = app.open_tabs.terminal_mut(&id) {
                let glyph = ['|', '/', '-', '\\'][frame % 4];
                terminal.feed_program_output(format!("\r{glyph} working {frame}").as_bytes());
            }
        }
    }

    #[test]
    #[ignore = "a benchmark: run with --ignored in a release build"]
    fn bench_frame_terminal_panes() {
        for (name, panes, changing) in [
            ("one pane, still", 1, false),
            ("four panes, still", 4, false),
            ("four panes, a line turning", 4, true),
        ] {
            let Some(mut app) = window(panes) else {
                eprintln!("bench frame: no GPU, skipped");
                return;
            };
            let probe = misty::testing::TestWindow::new(SIZE.0, SIZE.1);
            let mut frame = probe.frame();
            app.draw_into(&mut frame);
            let drawn = misty::testing::drawn_text(&frame);
            let lines = drawn
                .iter()
                .filter(|text| text.contains("render_frame"))
                .count();
            eprintln!(
                "bench frame {name}: {lines} terminal lines drawn of {}",
                drawn.len()
            );
            let change = |app: &mut App, frame| {
                if changing {
                    turn(app, frame);
                }
            };
            let (cpu, cpu_max) = time(&mut app, false, change);
            let (full, full_max) = time(&mut app, true, change);
            eprintln!(
                "bench frame {name}: CPU {cpu:.2} ms (max {cpu_max:.2}), \
                 with GPU {full:.2} ms (max {full_max:.2})"
            );
        }
    }
}

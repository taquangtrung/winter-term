//! Handlers for the winit window events the event loop dispatches.

use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey, NativeKeyCode, PhysicalKey};
use winit::window::CursorIcon;

use crate::model::input::{self, Action, KeyCode, Mods};
use crate::model::layout::TabId;
use crate::model::mode::{Mode, ModeEvent};
use crate::model::paint::Point;
use crate::model::textedit::modal::EditOutcome;

use super::overlay::Overlay;
use super::prompt_edit::PromptShadow;
use super::strip::StripHit;
use super::App;
use super::Selection;
use super::{
    APPROX_CELL_HEIGHT, CURSOR_BLINK_PERIOD, SCROLLBAR_CLICK_WIDTH, SCROLL_LINES_PER_WHEEL_NOTCH,
};

// ========================================================================
// Constants
// ========================================================================

/// Longest gap between two presses that still reads as a double click.
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

/// The letter that, with Ctrl and Shift, pastes into whatever takes input,
/// whatever else the keymap binds paste to.
const PASTE_LETTER: char = 'v';

/// Farthest, in pixels, a second press may land from the first and still
/// count as a double click.
const DOUBLE_CLICK_SLOP: f32 = 5.0;

/// Programs whose own modes run on Escape (the vi family, Helix, Kakoune,
/// Snow) or that read it as Meta (Emacs). In Insert mode a bare Escape goes
/// straight to one of these; any other program, such as an agent CLI, leaves
/// the first Escape to Winter. Matched against the foreground process name.
const ESCAPE_OWNING_PROGRAMS: &[&str] = &[
    "emacs", "helix", "hx", "kak", "nvim", "snow", "vi", "view", "vim", "vimdiff",
];

// ========================================================================
// Data Structures
// ========================================================================

/// One key press on its way through the layers that may take it, with what
/// each layer reads about where it landed.
struct KeyPress {
    focused: TabId,
    key: input::Key,
    /// The winit key it came from, which the palette and settings page read
    /// for text entry.
    logical: Key,
    /// The focused terminal's mode; the default for a tool, which has none.
    mode: Mode,
    physical: PhysicalKey,
    tool_focused: bool,
}

/// What a layer did with a key.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum KeyRoute {
    /// Taken: no layer after it sees the key.
    Handled,
    /// Not this layer's: the next one is offered it.
    Pass,
    /// Taken, and the app is to quit.
    Quit,
}

/// One layer of key routing.
type KeyLayer = fn(&mut App, &KeyPress) -> KeyRoute;

// ========================================================================
// KeyRoute Implementations
// ========================================================================

impl KeyRoute {
    /// A layer's yes or no, as a route.
    fn taken(taken: bool) -> Self {
        match taken {
            true => Self::Handled,
            false => Self::Pass,
        }
    }

    /// What [`App::route_sequence_key`] reported: `None` when the sequence
    /// did not take the key, else whether it asked to quit.
    fn of_sequence(fed: Option<bool>) -> Self {
        match fed {
            None => Self::Pass,
            Some(false) => Self::Handled,
            Some(true) => Self::Quit,
        }
    }
}

// ========================================================================
// App: window-event handlers
// ========================================================================

impl App {
    /// Route one key press or release: the chrome overlays first (command
    /// palette, settings page, which-key), then the focused pane's modal
    /// keymap, and finally the PTY.
    pub(crate) fn on_keyboard_input(&mut self, event: KeyEvent, event_loop: &ActiveEventLoop) {
        // A key held down after opening a sequence auto-repeats; the repeats
        // are the same press, not the next chord of the sequence.
        if event.repeat && self.is_sequence_pending() {
            return;
        }
        if self.handle_key(event.state, event.logical_key, event.physical_key) {
            self.quit(event_loop);
        }
    }

    /// The keyboard handler proper, split from [`Self::on_keyboard_input`] so
    /// tests can drive it without an `ActiveEventLoop` (winit's `KeyEvent`
    /// has no public constructor). Returns `true` when the app should quit,
    /// standing in for the former inline `quit(event_loop); return;` sites.
    pub(super) fn handle_key(
        &mut self,
        state: ElementState,
        logical_key: Key,
        physical_key: PhysicalKey,
    ) -> bool {
        // winit synthesizes KeyboardInput::Pressed events for every
        // physically-held key on XI_FocusIn (handle_pressed_keys). Swallow
        // those here so e.g. the Tab from M-TAB never reaches the PTY.
        if state == ElementState::Pressed && self.suppress_synthesized_keys {
            return false;
        }

        // Windows-only: drop a key event that raced ahead of this
        // window's own focus-gain notification (see
        // `is_pre_focus_key_leak`'s doc for why this can happen).
        #[cfg(target_os = "windows")]
        if is_pre_focus_key_leak(state, self.window.as_ref().is_some_and(|w| w.has_focus())) {
            return false;
        }

        let mods_state = self.modifiers.state();
        let code = winit_key_to_code(&logical_key, &physical_key);
        let key = input::Key {
            alt: mods_state.alt_key(),
            code,
            ctrl: mods_state.control_key(),
            shift: mods_state.shift_key(),
            sup: mods_state.super_key(),
        }
        .canonical();
        self.route_key(state, key, logical_key, physical_key)
    }

    /// Type `text` an input method committed: each character goes through
    /// the same routing as an unmodified key press and release, so it reaches
    /// a shell, a tool, or the Normal-mode keymap exactly as typing it on a
    /// keyboard would, whatever modifiers happen to be held while the
    /// candidate is picked. Returns `true` when the app should quit.
    pub(crate) fn type_committed_text(&mut self, text: &str) -> bool {
        let physical = PhysicalKey::Unidentified(NativeKeyCode::Unidentified);
        for ch in text.chars() {
            let logical = Key::Character(ch.to_string().into());
            let key = input::Key {
                alt: false,
                code: winit_key_to_code(&logical, &physical),
                ctrl: false,
                shift: false,
                sup: false,
            }
            .canonical();
            if self.route_key(ElementState::Pressed, key, logical.clone(), physical) {
                return true;
            }
            self.route_key(ElementState::Released, key, logical, physical);
        }
        false
    }

    /// Whether `key` only moves the open palette's selection (`Up`, `Down`,
    /// `C-n`, `C-p`), leaving its query as it was.
    fn moves_palette_selection(&self, key: &input::Key) -> bool {
        if self.palette().is_none() || key.alt || key.sup {
            return false;
        }
        match key.glyph() {
            input::KeyCode::Up | input::KeyCode::Down => !key.ctrl,
            input::KeyCode::Char('n' | 'p') => key.ctrl,
            _ => false,
        }
    }

    /// Route `key` (with the winit key it came from, which the palette and
    /// settings page read for text entry) through the overlays, the focused
    /// pane's keymap, and on to the PTY. Returns `true` when the app should
    /// quit.
    fn route_key(
        &mut self,
        state: ElementState,
        key: input::Key,
        logical_key: Key,
        physical_key: PhysicalKey,
    ) -> bool {
        if state == ElementState::Released {
            self.route_key_release(&key);
            return false;
        }

        // A key winit cannot name, such as Ctrl or Shift going down, types
        // nothing and means nothing. Left to go on, it reaches whatever is
        // taking text, which writes the NUL it decodes to as a stray glyph.
        if key.is_unidentified() {
            return false;
        }

        // Reset blink to the "visible" phase on every key press so the
        // cursor is immediately shown as feedback for the action. A key that
        // only moves the palette's selection leaves the query's caret
        // blinking: nothing was typed, and arrowing down a list would
        // otherwise hold it solid the whole way.
        if self.config.cursor.blink && !self.moves_palette_selection(&key) {
            self.blink_phase = true;
            self.blink_next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
        }

        let focused = self.layout().focused();
        let press = KeyPress {
            focused,
            key,
            logical: logical_key,
            // `mode` is the terminal's, and only means anything when the tab
            // is not a tool, which takes its own keys rather than being in a
            // mode.
            mode: self.open_tabs.mode(&focused).unwrap_or_default(),
            physical: physical_key,
            tool_focused: self.open_tabs.is_tool(&focused),
        };
        // Each layer takes the key or passes it on, in this order: the first
        // to take it is the only one that sees it.
        let layers: [KeyLayer; 13] = [
            Self::route_paste_key,
            Self::route_rename_key,
            Self::route_pending_sequence_key,
            Self::route_modal_key,
            Self::route_dismissing_escape,
            Self::route_global_key,
            Self::route_palette_key,
            Self::route_tool_cursor_layer,
            Self::route_exit_shown_key,
            Self::route_sequence_start_key,
            Self::route_tool_key,
            Self::route_bare_escape,
            Self::route_keymap_key,
        ];
        for layer in layers {
            match layer(self, &press) {
                KeyRoute::Pass => continue,
                KeyRoute::Handled => return self.finish_key(),
                KeyRoute::Quit => return true,
            }
        }
        false
    }

    /// What every key that was taken leads to, done once here rather than by
    /// each layer: a quit asked for on the way, else a fresh title and frame.
    fn finish_key(&mut self) -> bool {
        if std::mem::take(&mut self.exit_requested) {
            return true;
        }
        self.update_window_title();
        self.dirty = true;
        false
    }

    /// Only a key whose press the program was sent hears its release, and it
    /// hears it in the tab the press went to: the release of a chord Winter
    /// kept (a window command, a key sequence) would reach the program as a
    /// key it never saw go down, and one sent wherever the focus has moved
    /// since would reach a program that never saw it at all.
    fn route_key_release(&mut self, key: &input::Key) {
        let Some(tab) = self.forwarded_presses.remove(&key.code) else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal_mut(&tab) else {
            return;
        };
        let bytes = input::encode_release(key, terminal.kitty_flags());
        if !bytes.is_empty() {
            terminal.write(&bytes);
            self.dirty = true;
        }
    }

    // --------------------------------------------------------------------
    // Key layers
    // --------------------------------------------------------------------

    /// The paste chord pastes into whatever takes input, ahead of every layer
    /// that would otherwise take it as a key: a dialog's field, the palette,
    /// a tool, or the terminal.
    fn route_paste_key(&mut self, press: &KeyPress) -> KeyRoute {
        if !self.is_paste_key(&press.key) {
            return KeyRoute::Pass;
        }
        self.paste_from_clipboard();
        KeyRoute::Handled
    }

    /// Whether `key` pastes: `C-S-v` always, and whatever the keymap binds
    /// to paste besides.
    fn is_paste_key(&self, key: &input::Key) -> bool {
        let fixed = key.ctrl
            && key.shift
            && !key.alt
            && !key.sup
            && matches!(key.glyph(), KeyCode::Char(c) if c.eq_ignore_ascii_case(&PASTE_LETTER));
        fixed || self.keymap.global_action(key) == Some(Action::Paste)
    }

    /// While a tab rename is in progress it takes every key: Enter names the
    /// tab the rename was started on, Escape or `C-g` cancels, and the rest
    /// edit the name as any field's Emacs keys do.
    fn route_rename_key(&mut self, press: &KeyPress) -> KeyRoute {
        let Some(Overlay::Rename(input)) = &mut self.overlay else {
            return KeyRoute::Pass;
        };
        match input.field.on_key(&press.key.typed(), 1) {
            EditOutcome::Submit => {
                let tab = input.tab;
                let name = input.field.text().to_string();
                self.overlay = None;
                self.tabs.set_name(tab, Some(name));
            }
            EditOutcome::Cancel => self.overlay = None,
            _ => {}
        }
        KeyRoute::Handled
    }

    /// A sequence part-way typed takes the next key ahead of everything, the
    /// global chords included: continuing it is the key's only reading.
    fn route_pending_sequence_key(&mut self, press: &KeyPress) -> KeyRoute {
        if !self.is_sequence_pending() {
            return KeyRoute::Pass;
        }
        KeyRoute::of_sequence(self.route_sequence_key(press.focused, &press.key))
    }

    /// A question, a name being typed, or the settings page owns the keyboard
    /// while it is up, ahead of the window chords: a filename being typed
    /// must not trigger one, and no key reaches the PTY underneath.
    fn route_modal_key(&mut self, press: &KeyPress) -> KeyRoute {
        match &mut self.overlay {
            Some(Overlay::Close(_)) => self.handle_close_prompt_key(&press.key),
            Some(Overlay::ToolPrompt(_)) => self.handle_prompt_key(&press.key),
            Some(Overlay::Password(_)) => self.handle_password_key(&press.key),
            Some(Overlay::ZaiKey(_)) => self.handle_zai_key_key(&press.key),
            Some(Overlay::ThemeName(input)) => match input.on_key(&press.key.typed(), 1) {
                EditOutcome::Submit => {
                    let name = input.text().to_string();
                    self.overlay = None;
                    self.create_named_theme(&name);
                }
                EditOutcome::Cancel => self.overlay = None,
                _ => {}
            },
            // Edits apply live, Enter and Escape close it.
            Some(Overlay::Settings(_)) => {
                if let Some(mut page) = self.take_settings_page() {
                    if self.handle_settings_input(&mut page, &press.logical, &press.key) {
                        self.overlay = Some(Overlay::Settings(page));
                    } else {
                        self.on_settings_closed();
                    }
                }
            }
            Some(Overlay::Palette(_) | Overlay::Rename(_)) | None => return KeyRoute::Pass,
        }
        KeyRoute::Handled
    }

    /// Escape puts away an open menu, then a mouse-drag selection, before
    /// anything else acts on it.
    fn route_dismissing_escape(&mut self, press: &KeyPress) -> KeyRoute {
        if self.menus.open.is_some() && press.key.glyph() == KeyCode::Escape {
            self.close_menu();
            return KeyRoute::Handled;
        }
        if escape_clears_selection(&press.key, press.mode, self.selection.span.is_some()) {
            self.selection.span = None;
            return KeyRoute::Handled;
        }
        KeyRoute::Pass
    }

    /// Global app shortcuts (settings, font size, tab/pane management,
    /// palette toggles): configurable via the `window` block in
    /// keybindings.kdl, and checked here so they intercept even while the
    /// palette would otherwise own the key. Window-layout chords
    /// (split/close/focus/scroll/zoom) are resolved later, once nothing else
    /// claims the key.
    fn route_global_key(&mut self, press: &KeyPress) -> KeyRoute {
        let global = self
            .keymap
            .global_action(&press.key)
            .filter(|action| !self.is_copy_yielded_to_tool(action, press.focused));
        let Some(action) = global else {
            return KeyRoute::Pass;
        };
        self.handle_action(action, press.focused);
        KeyRoute::Handled
    }

    /// The open palette takes every key that is not a global chord.
    fn route_palette_key(&mut self, press: &KeyPress) -> KeyRoute {
        if self.palette().is_none() {
            return KeyRoute::Pass;
        }
        self.press_palette_key(&press.logical, &press.physical, press.focused);
        KeyRoute::Handled
    }

    /// The tool text cursor takes every key while it is up, ahead of the
    /// tool itself: sharing keys would move the selection and the listing
    /// at once. See `tool_cursor`.
    fn route_tool_cursor_layer(&mut self, press: &KeyPress) -> KeyRoute {
        KeyRoute::taken(self.handle_tool_cursor_key(&press.key))
    }

    /// A command that failed keeps its tab, its exit written below its
    /// output, until a key: this key, whatever it is, closes it.
    fn route_exit_shown_key(&mut self, press: &KeyPress) -> KeyRoute {
        let shown = self
            .open_tabs
            .terminal(&press.focused)
            .is_some_and(|terminal| terminal.is_exit_shown());
        if !shown {
            return KeyRoute::Pass;
        }
        self.close_tab_now(press.focused, super::tool::Closing::Forget);
        KeyRoute::Handled
    }

    /// A key opening a sequence (the window's, or the focused tool's own
    /// block's) is the sequence's, ahead of the tool and the mode keymaps
    /// alike, so `M-g M-h` reads the same in a tool as in a terminal.
    fn route_sequence_start_key(&mut self, press: &KeyPress) -> KeyRoute {
        KeyRoute::of_sequence(self.route_sequence_key(press.focused, &press.key))
    }

    /// A tool owns the keys it binds, and only those: anything it declines
    /// falls through to the window chords, so the vim vocabulary reads the
    /// same in a tool as in a terminal. `v` or `V` in a tool that did not
    /// bind it starts a text cursor selecting by character or by whole line,
    /// the way each starts a Visual selection in a terminal.
    fn route_tool_key(&mut self, press: &KeyPress) -> KeyRoute {
        if self.offer_key_to_tool(press.focused, &press.key) {
            return KeyRoute::Handled;
        }
        let key = &press.key;
        let glyph = key.glyph();
        let starts_cursor = press.tool_focused
            && !key.ctrl
            && !key.alt
            && !key.sup
            && matches!(glyph, KeyCode::Char('v' | 'V'));
        if starts_cursor {
            self.start_tool_cursor(press.focused);
            // The cursor starts selecting by character; `V` is what turns a
            // selection linewise once it is up.
            if glyph == KeyCode::Char('V') {
                self.handle_tool_cursor_key(key);
            }
        }
        KeyRoute::taken(starts_cursor)
    }

    /// In Insert mode the first bare Escape switches to Normal mode, unless
    /// the program in the pane runs its own modes on Escape (vim, Helix) or
    /// the shell is mid tab-completion; those get the Escape directly. In
    /// Normal mode, a bare Escape first goes to whatever Winter is holding
    /// open (a half-typed command, a search, a quick-select); only when there
    /// is none, and a program is running (or the shell is mid
    /// tab-completion), is it written to the PTY. So an agent CLI like
    /// Claude Code gets its Escape with a second press.
    fn route_bare_escape(&mut self, press: &KeyPress) -> KeyRoute {
        let focused = press.focused;
        let bare_esc = press.key.glyph() == KeyCode::Escape && press.key.mods() == Mods::NONE;
        if !bare_esc || press.tool_focused {
            return KeyRoute::Pass;
        }
        if press.mode == Mode::Insert {
            let terminal = self.open_tabs.terminal(&focused);
            // Every process in the foreground group, not only its leader:
            // `make dev` leads the group the editor it runs sits in.
            let foreground = terminal
                .map(|p| p.foreground_group_names())
                .unwrap_or_default();
            let pending_tab_completion = self
                .open_tabs
                .terminal_tab_mut(&focused)
                .is_some_and(|tab| tab.pending_tab_completion);
            if escape_kept_in_insert(&foreground, pending_tab_completion) {
                return KeyRoute::Pass;
            }
            let switch = input::Action::SwitchMode(Mode::Insert.apply(ModeEvent::EnterNormal));
            self.handle_action(switch, focused);
            return KeyRoute::Handled;
        }
        if press.mode != Mode::Normal || self.escape_has_winter_target() {
            return KeyRoute::Pass;
        }
        let pending_tab_completion = self
            .open_tabs
            .terminal_tab_mut(&focused)
            .is_some_and(|tab| std::mem::take(&mut tab.pending_tab_completion));
        let has_foreground_process = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.has_foreground_process());
        if !escape_forwarded_to_pty(has_foreground_process, pending_tab_completion) {
            return KeyRoute::Pass;
        }
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&[0x1b]);
            self.forwarded_presses.insert(press.key.code, focused);
        }
        KeyRoute::Handled
    }

    /// The focused tab's own keymap: the window chords, the terminal's modal
    /// keys, and finally the bytes the key sends to the PTY.
    fn route_keymap_key(&mut self, press: &KeyPress) -> KeyRoute {
        let focused = press.focused;
        let key = &press.key;
        let terminal = self.open_tabs.terminal(&focused);
        let at_prompt = terminal.is_some_and(|p| p.is_at_prompt());
        let kitty_flags = terminal.map(|p| p.kitty_flags()).unwrap_or(0);
        let modify_other_keys = terminal.and_then(|p| p.modify_other_keys());
        let is_alt_screen = terminal.is_some_and(|p| p.grid().is_alt_screen());
        let app_cursor_keys = terminal.is_some_and(|p| p.app_cursor_keys());
        let prev_pending = self.pending;
        let action = match press.tool_focused {
            true => input::resolve_tool(key, &self.keymap),
            false => input::resolve_with(
                press.mode,
                key,
                &mut self.pending,
                &self.keymap,
                kitty_flags,
                modify_other_keys,
                is_alt_screen,
            ),
        };
        let action = input::apply_cursor_key_mode(action, app_cursor_keys, kitty_flags);
        if forwarded_to_pty(&action) {
            self.forwarded_presses.insert(key.code, focused);
        }
        if self.pending != prev_pending {
            self.pending_since = if self.pending.hint().is_some() {
                Some(Instant::now())
            } else {
                None
            };
        } else if self.pending == input::PendingPrefix::None {
            self.pending_since = None;
        }
        // Keep the per-pane prompt shadow in step with this key so
        // `C-/`/`C-\` can replay edits. Only forwarded keys mutate the line
        // (`apply_insert_key` models them or desyncs on the unmodeled ones);
        // `Edit`/undo/redo update it in their handlers. Mode switches, focus
        // and tab commands leave this line untouched, so the shadow is
        // preserved (undo still works in Normal mode).
        if !press.tool_focused && press.mode == Mode::Insert && forwarded_to_pty(&action) {
            // Deliberately does NOT queue an automatic switch to Normal mode
            // once a submitted command's new prompt settles: Insert mode
            // always resumes after running a command, exactly as it was
            // before that command ran; only an explicit mode-switch chord
            // enters Normal mode.
            self.record_insert_key(focused, key, &action, at_prompt);
        }
        self.handle_action(action, focused);
        KeyRoute::Handled
    }

    /// Whether the global copy chord, with nothing selected, should pass to
    /// the tool in tab `focused` instead: with nothing to copy it
    /// would do nothing, and a tool may bind the same chord (the Git tool
    /// commits on `C-S-c`). A tool that declines it still reaches the
    /// ordinary keymap, where it resolves to the same harmless copy.
    fn is_copy_yielded_to_tool(&self, action: &input::Action, focused: TabId) -> bool {
        *action == input::Action::Copy
            && self.palette().is_none()
            && self.open_tabs.is_tool(&focused)
            && self.selected_text().is_none()
    }

    /// A left press on grid cell `(row, col)` of `tab`, at the pointer's
    /// current pixel position. It anchors any drag that follows at the press
    /// rather than at the first motion event (see `PointerState::press_cell`)
    /// and turns a quick second press into a word selection. The traversal
    /// cursor is left where it is: only a drag moves it, so a plain click in
    /// Normal mode does not make the block cursor jump to the pointer.
    fn press_grid_cell(&mut self, tab: TabId, row: usize, col: usize) {
        let (x, y) = self.pointer.cursor_pos;
        self.pointer.press_cell = self
            .selection_grid(tab)
            .map(|grid| (tab, grid.to_absolute_row(row), col));
        let now = Instant::now();
        if let Some((prev_time, prev_x, prev_y)) = self.pointer.last_click {
            let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
            if now.duration_since(prev_time) < DOUBLE_CLICK_INTERVAL && dist < DOUBLE_CLICK_SLOP {
                self.select_word_at(tab, row, col);
            }
        }
        self.pointer.last_click = Some((now, x, y));
    }

    /// Route a mouse button press or release. The tabbar and any open menu
    /// take precedence over the panes, so a click on chrome never reaches a
    /// pane underneath it.
    pub(crate) fn on_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        event_loop: &ActiveEventLoop,
    ) {
        let focused = self.layout().focused();

        // Tabbar/menubar clicks take precedence over the panes (including
        // mouse-tracking apps), and any click resolves an open menu.
        if let (ElementState::Pressed, MouseButton::Left) = (state, button) {
            let (x, y) = self.pointer.cursor_pos;
            if let Some(direction) = self.edge_resize_direction(x, y) {
                if let Some(window) = &self.window {
                    let _ = window.drag_resize_window(direction);
                }
                return;
            }
            if self.menus.open.is_none() && self.press_notifications(x, y) {
                return;
            }
            // A click on the find popup is the popup's, not the pane's under it.
            if self.renderer.as_ref().is_some_and(|r| r.find_at(x, y)) {
                return;
            }
            if (self.menus.open.is_some() || y < self.top_chrome_height())
                && self.handle_tabbar_click(x, y)
            {
                if self.exit_requested {
                    self.exit_requested = false;
                    self.quit(event_loop);
                    return;
                }
                self.dirty = true;
                return;
            }
        }

        // A press on a pane's strip of tabs belongs to the strip: it shows,
        // closes, or opens a tab rather than reaching the pane below it. A
        // split divider running past the strip still wins, so the drag that
        // resizes panes can start anywhere along it.
        if state == ElementState::Pressed
            && matches!(button, MouseButton::Left | MouseButton::Middle)
        {
            let (x, y) = self.pointer.cursor_pos;
            let on_divider = self
                .layout()
                .divider_at(x, y, self.content_viewport())
                .is_some();
            if !on_divider && self.handle_strip_press(x, y, button == MouseButton::Middle) {
                if self.exit_requested {
                    self.exit_requested = false;
                    self.quit(event_loop);
                    return;
                }
                if button == MouseButton::Left {
                    self.pointer.mouse_down = true;
                }
                self.dirty = true;
                return;
            }
        }

        // Ctrl+click on a hyperlinked cell or printed URL/file opens it.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.modifiers.state().control_key()
        {
            let (x, y) = self.pointer.cursor_pos;
            if self.open_file_at_pixel(x, y) {
                self.dirty = true;
                return;
            }
            // A `file://` link was the file above, if it is a local one; the
            // OS is handed only the schemes safe to hand it.
            if let Some(url) = &self.pointer.hovered_url {
                if crate::terminal::session::is_os_openable(url) {
                    let _ = open::that_detached(url);
                    return;
                }
            }
        }

        // Plain clicks always drive Winter's own selection, even over a
        // full-screen app (vim, htop) that has turned on mouse reporting.
        // Hold Shift to forward the click to that app's mouse handling instead.
        // A program that was handed a press is handed its release too, even
        // if Shift went up first, or it would see the button held forever.
        let completes_forwarded_press =
            state == ElementState::Released && self.pointer.forwarded_button.is_some();
        let mouse_active = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_tracking())
            && (self.modifiers.state().shift_key() || completes_forwarded_press);

        if mouse_active {
            self.forward_mouse_event(state, button, focused);
            return;
        }

        // Right-click: paste if configured, otherwise open the context menu.
        if state == ElementState::Pressed && button == MouseButton::Right {
            if self.config.paste_on_right_click {
                self.paste_from_clipboard();
            } else {
                let (x, y) = self.pointer.cursor_pos;
                self.open_context_menu(x, y);
                self.dirty = true;
            }
            return;
        }

        // Any left-press dismisses an open context menu before other handling.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.menus.context_pos.is_some()
        {
            let (x, y) = self.pointer.cursor_pos;
            let surface_w = self.viewport_rect().width;
            if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
                let tabbar = self.build_top_tabbar();
                let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
                if let winter_render::TabbarHit::ContextMenuItem(i) = hit {
                    if let Some(action) = self.context_entry(i) {
                        self.close_context_menu();
                        self.run_context_action(action);
                        self.dirty = true;
                        return;
                    }
                }
            }
            self.close_context_menu();
        }

        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => {
                self.pointer.mouse_down = true;
                self.selection.span = None;

                let (x, y) = self.pointer.cursor_pos;

                // Divider click: start a drag; skip scrollbar/focus changes.
                let on_divider = {
                    let viewport = self.content_viewport();
                    self.layout().divider_at(x, y, viewport).is_some()
                };
                if on_divider {
                    self.pointer.divider_drag = Some((x, y));
                } else {
                    // Scrollbar click: right-edge strip of a pane navigates scrollback.
                    'scroll: for (id, rect) in self.pane_rects() {
                        let pr = Self::layout_rect_to_pane(rect);
                        let sb_x = pr.x + pr.width - SCROLLBAR_CLICK_WIDTH;
                        if x >= sb_x && x <= pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                            if let Some(terminal) = self.open_tabs.terminal_mut(&id) {
                                let sbl = terminal.grid().scrollback_len();
                                if sbl > 0 {
                                    let rows = terminal.grid().rows();
                                    let total = (rows + sbl) as f32;
                                    let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                                    let top_virtual = (frac * total) as usize;
                                    let new_offset = sbl.saturating_sub(top_virtual);
                                    terminal.grid_mut().set_scroll_offset(new_offset);
                                    self.pointer.scrollbar_drag = Some(id);
                                    self.dirty = true;
                                    break 'scroll;
                                }
                            }
                        }
                    }

                    if let Some((tab_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        self.set_focus(tab_id);
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        // A tool tab owns what is drawn there, so the click is
                        // its to answer before the grid's.
                        let at = Point {
                            col,
                            drag: false,
                            row,
                        };
                        if self.offer_mouse_to_tool(tab_id, at) {
                            self.pointer.press_cell = None;
                            self.dirty = true;
                            return;
                        }
                        self.press_grid_cell(tab_id, row, col);
                    }
                }

                self.dirty = true;
            }
            (ElementState::Released, MouseButton::Left) => {
                self.pointer.mouse_down = false;
                self.pointer.divider_drag = None;
                self.pointer.press_cell = None;
                self.pointer.scrollbar_drag = None;
                self.finalize_tab_drag();
                self.copy_selection();
                self.copy_selection_to_primary();
                if let Some(window) = &self.window {
                    window.set_cursor(CursorIcon::Default);
                }
                self.dirty = true;
            }
            (ElementState::Pressed, MouseButton::Middle) => {
                self.paste_from_primary();
            }
            _ => {}
        }
    }
    /// Track the pointer: hover highlights, drag-selection extension, split
    /// dragging, and the resize-edge cursor shape.
    /// Light the part of the status bar's right side the pointer is on, and
    /// put out the one it left.
    fn hover_status_bar(&mut self, x: f32, y: f32) {
        let hover = match self.status_bar_visible() {
            true => self
                .renderer
                .as_ref()
                .and_then(|renderer| renderer.status_hover_at(x, y)),
            false => None,
        };
        if hover != self.status_hover {
            self.status_hover = hover;
            self.dirty = true;
        }
    }

    pub(crate) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let x = position.x as f32;
        let y = position.y as f32;
        self.pointer.cursor_pos = (x, y);
        // A toast stays up while the pointer rests on it.
        self.hover_toasts(x, y);
        self.hover_status_bar(x, y);

        // Divider drag: highest priority, blocks selection and PTY forwarding.
        if self.pointer.mouse_down {
            if let Some((prev_x, prev_y)) = self.pointer.divider_drag {
                let dx = x - prev_x;
                let dy = y - prev_y;
                let viewport = self.content_viewport();
                if self
                    .layout_mut()
                    .drag_divider(prev_x, prev_y, dx, dy, viewport)
                {
                    self.pointer.divider_drag = Some((x, y));
                    self.resize_all_panes();
                    self.dirty = true;
                }
                return;
            }

            // Scrollbar drag: update scroll position proportionally.
            if let Some(sb_pane_id) = self.pointer.scrollbar_drag {
                if let Some((_, rect)) = self
                    .pane_rects()
                    .into_iter()
                    .find(|(id, _)| *id == sb_pane_id)
                {
                    let pr = Self::layout_rect_to_pane(rect);
                    if let Some(terminal) = self.open_tabs.terminal_mut(&sb_pane_id) {
                        let sbl = terminal.grid().scrollback_len();
                        if sbl > 0 {
                            let rows = terminal.grid().rows();
                            let total = (rows + sbl) as f32;
                            let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                            let top_virtual = (frac * total) as usize;
                            terminal
                                .grid_mut()
                                .set_scroll_offset(sbl.saturating_sub(top_virtual));
                            self.dirty = true;
                        }
                    }
                }
                return;
            }
        }

        if self.menus.open.is_some() {
            self.update_menu_hover(x, y);
        }
        if self.menus.context_pos.is_some() {
            self.update_context_menu_hover(x, y);
        }

        // Update tabbar hover state so the renderer can show hover highlights.
        if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
            let surface_w = self.viewport_rect().width;
            let tabbar = self.build_top_tabbar();
            let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
            if hit != self.tabs.hover {
                self.tabs.hover = hit;
                self.dirty = true;
            }
        }
        // The pane strips' hover, which also carries a tab's full title as a
        // tooltip, since a strip may have had to cut it short.
        let strip_before = self.tabs.strip_hover;
        self.update_strip_hover(x, y);
        if self.tabs.strip_hover != strip_before {
            self.tabs.hover_pos = match self.tabs.strip_hover {
                StripHit::Tab(_) => Some(self.pointer.cursor_pos),
                _ => None,
            };
            self.update_window_title();
            self.dirty = true;
        }

        // With Shift held (Winter's "this goes to the app" chord), motion
        // reaches a mouse-tracking program: drags under drag tracking
        // (?1002), and bare motion too under any-event tracking (?1003).
        let focused = self.layout().focused();
        let reports_motion = self.open_tabs.terminal(&focused).is_some_and(|p| {
            if self.pointer.forwarded_button.is_some() {
                p.mouse_drag_tracking()
            } else {
                p.mouse_any_event()
            }
        });
        if reports_motion && self.modifiers.state().shift_key() {
            self.forward_mouse_motion(focused);
            return;
        }

        let is_tab_dragging = self.tabs.drag_start.is_some_and(|(_, sx, sy)| {
            ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() >= super::strip::DRAG_THRESHOLD_PX
        });
        if self.tabs.drag_start.is_some() {
            self.dirty = true;
        }

        if self.pointer.mouse_down && self.tabs.drag_start.is_none() {
            // Resolved against the pane the drag started in, not just the one
            // under the pointer, so running off an edge keeps selecting.
            if let Some((tab_id, row, col)) = self.drag_cell_at(x, y) {
                let at = Point {
                    col,
                    drag: true,
                    row,
                };
                if self.offer_mouse_to_tool(tab_id, at) {
                    self.dirty = true;
                    return;
                }
                // Selection rows are absolute (see `Grid::to_absolute_row`)
                // so they keep naming the same line if auto-scroll (or a
                // wheel scroll) moves the view mid-drag.
                let abs_row = self
                    .selection_grid(tab_id)
                    .map(|grid| grid.to_absolute_row(row))
                    .unwrap_or(row);
                if let Some(sel) = &mut self.selection.span {
                    sel.end_row = abs_row;
                    sel.end_col = col;
                    sel.tab = tab_id;
                } else if let Some((start_row, start_col)) =
                    self.pointer.drag_anchor(tab_id, (abs_row, col))
                {
                    self.selection.span = Some(Selection {
                        block: false,
                        start_row,
                        start_col,
                        end_row: abs_row,
                        end_col: col,
                        tab: tab_id,
                    });
                }
                // The traversal cursor rides along with the drag's
                // live end, so the block cursor keeps following the
                // mouse while the selection extends.
                self.track_nav_cursor_to_mouse(tab_id, row, col);
                self.dirty = true;
            }
        }

        // Update hovered hyperlink and cursor icon (divider resize, tab drag, or pointer).
        let new_url = self.hovered_link_at(x, y);
        self.pointer.hovered_url = new_url;
        let vp = self.content_viewport();
        let icon = if let Some(direction) = self.edge_resize_direction(x, y) {
            CursorIcon::from(direction)
        } else if is_tab_dragging {
            CursorIcon::Grabbing
        } else {
            match self.layout().divider_at(x, y, vp) {
                Some(crate::model::layout::Direction::Vertical) => CursorIcon::EwResize,
                Some(crate::model::layout::Direction::Horizontal) => CursorIcon::NsResize,
                None => {
                    if self.pointer.hovered_url.is_some()
                        || matches!(
                            self.tabs.strip_hover,
                            super::strip::StripHit::Tab(_)
                                | super::strip::StripHit::Close(_)
                                | super::strip::StripHit::NewTab(_)
                        )
                    {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    }
                }
            }
        };
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }
    /// Scroll the pane under the pointer (the focused one when the pointer is
    /// over no pane), or forward the wheel to its PTY when a full-screen
    /// application has asked for mouse reporting. Focus stays where it was.
    pub(crate) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let scroll_lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                (y * SCROLL_LINES_PER_WHEEL_NOTCH).round() as isize
            }
            MouseScrollDelta::PixelDelta(pos) => (-pos.y / APPROX_CELL_HEIGHT as f64) as isize,
        };

        let (x, y) = self.pointer.cursor_pos;
        let focused = self
            .pane_at_pixel(x, y)
            .map(|(id, _)| id)
            .unwrap_or_else(|| self.layout().focused());
        if scroll_lines != 0 && self.offer_scroll_to_tool(focused, scroll_lines) {
            self.dirty = true;
            return;
        }
        if self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_tracking())
        {
            self.drop_selection_in_tab(focused);
            self.forward_mouse_scroll(scroll_lines, focused);
            return;
        }

        if scroll_lines != 0 {
            let alt_screen = self
                .open_tabs
                .terminal(&focused)
                .is_some_and(|p| p.grid().is_alt_screen());
            if alt_screen {
                self.drop_selection_in_tab(focused);
            }
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                // Alt-screen apps (vim, less, etc.) own their viewport: send
                // arrow keys so they respond to the scroll gesture instead of
                // us scrolling their non-existent scrollback.
                if alt_screen {
                    let arrow = if scroll_lines > 0 {
                        b"\x1b[A" as &[u8]
                    } else {
                        b"\x1b[B"
                    };
                    let count = scroll_lines.unsigned_abs();
                    for _ in 0..count {
                        terminal.write(arrow);
                    }
                } else {
                    let grid = terminal.grid_mut();
                    if scroll_lines > 0 {
                        grid.scroll_up_history(scroll_lines as usize);
                    } else {
                        grid.scroll_down_history((-scroll_lines) as usize);
                    }
                }
            }
            self.dirty = true;
        }
    }

    /// Bookkeeping for one key forwarded to the PTY while the terminal is in
    /// Insert mode: keep the prompt shadow in step so `C-/`/`C-\\` can
    /// replay edits, and accumulate the exact bytes into the Insert session's
    /// `.`-repeat run (finalized when the pane leaves Insert: see
    /// [`App::track_change`]). Extracted from the keyboard handler so tests
    /// can drive the same path the event loop does.
    pub(crate) fn record_insert_key(
        &mut self,
        focused: TabId,
        key: &input::Key,
        action: &Action,
        at_prompt: bool,
    ) {
        if let Some(tab) = self.open_tabs.terminal_tab_mut(&focused) {
            tab.prompt_shadow
                .get_or_insert_with(PromptShadow::default)
                .apply_insert_key(key, at_prompt);
            // Tracks whether the tab may now be mid the shell's own
            // tab-completion, so a following bare Escape can cancel it -
            // see `escape_forwarded_to_pty`.
            tab.pending_tab_completion = key.glyph() == input::KeyCode::Tab;
        }
        if let Action::SendBytes(bytes) = action {
            self.vim
                .tab_mut(focused)
                .insert_session
                .get_or_insert_with(Default::default)
                .run
                .extend_from_slice(bytes);
        }
    }

    /// Whether Winter itself has something open that a bare Escape in Normal
    /// mode would put away: a half-typed command or count, a search, a
    /// quick-select, find labels. While it does, Escape is Winter's and never
    /// reaches the program underneath.
    pub(crate) fn escape_has_winter_target(&self) -> bool {
        self.pending != input::PendingPrefix::None
            || self.search.query.is_some()
            || self.quick_select.is_some()
            || self.find_labels.is_some()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Recover a character from `physical`'s key position when the logical key
/// couldn't resolve one. On some keyboard layouts, Windows' `ToUnicode`
/// returns no character at all for certain Ctrl+Alt digit/letter combos
/// (the same class of layout quirk that defines `AltGr`), so winit reports
/// `Key::Unidentified` even though a physical digit or letter was pressed.
/// Covers only the keys chord specs actually use.
fn physical_key_fallback(physical: &PhysicalKey) -> Option<char> {
    use winit::keyboard::KeyCode as Physical;
    let PhysicalKey::Code(code) = physical else {
        return None;
    };
    Some(match code {
        Physical::Digit0 => '0',
        Physical::Digit1 => '1',
        Physical::Digit2 => '2',
        Physical::Digit3 => '3',
        Physical::Digit4 => '4',
        Physical::Digit5 => '5',
        Physical::Digit6 => '6',
        Physical::Digit7 => '7',
        Physical::Digit8 => '8',
        Physical::Digit9 => '9',
        Physical::KeyA => 'a',
        Physical::KeyB => 'b',
        Physical::KeyC => 'c',
        Physical::KeyD => 'd',
        Physical::KeyE => 'e',
        Physical::KeyF => 'f',
        Physical::KeyG => 'g',
        Physical::KeyH => 'h',
        Physical::KeyI => 'i',
        Physical::KeyJ => 'j',
        Physical::KeyK => 'k',
        Physical::KeyL => 'l',
        Physical::KeyM => 'm',
        Physical::KeyN => 'n',
        Physical::KeyO => 'o',
        Physical::KeyP => 'p',
        Physical::KeyQ => 'q',
        Physical::KeyR => 'r',
        Physical::KeyS => 's',
        Physical::KeyT => 't',
        Physical::KeyU => 'u',
        Physical::KeyV => 'v',
        Physical::KeyW => 'w',
        Physical::KeyX => 'x',
        Physical::KeyY => 'y',
        Physical::KeyZ => 'z',
        _ => return None,
    })
}

pub(crate) fn winit_key_to_code(key: &Key, physical: &PhysicalKey) -> KeyCode {
    match key.as_ref() {
        Key::Character(c) => KeyCode::Char(c.chars().next().unwrap_or('\0')),
        Key::Named(NamedKey::Enter) => KeyCode::Enter,
        Key::Named(NamedKey::Backspace) => KeyCode::Backspace,
        Key::Named(NamedKey::Tab) => KeyCode::Tab,
        Key::Named(NamedKey::Escape) => KeyCode::Escape,
        Key::Named(NamedKey::ArrowUp) => KeyCode::Up,
        Key::Named(NamedKey::ArrowDown) => KeyCode::Down,
        Key::Named(NamedKey::ArrowLeft) => KeyCode::Left,
        Key::Named(NamedKey::ArrowRight) => KeyCode::Right,
        Key::Named(NamedKey::Space) => KeyCode::Space,
        Key::Named(NamedKey::Home) => KeyCode::Home,
        Key::Named(NamedKey::End) => KeyCode::End,
        Key::Named(NamedKey::PageUp) => KeyCode::PageUp,
        Key::Named(NamedKey::PageDown) => KeyCode::PageDown,
        Key::Named(NamedKey::Insert) => KeyCode::Insert,
        Key::Named(NamedKey::Delete) => KeyCode::Delete,
        Key::Named(NamedKey::F1) => KeyCode::F(1),
        Key::Named(NamedKey::F2) => KeyCode::F(2),
        Key::Named(NamedKey::F3) => KeyCode::F(3),
        Key::Named(NamedKey::F4) => KeyCode::F(4),
        Key::Named(NamedKey::F5) => KeyCode::F(5),
        Key::Named(NamedKey::F6) => KeyCode::F(6),
        Key::Named(NamedKey::F7) => KeyCode::F(7),
        Key::Named(NamedKey::F8) => KeyCode::F(8),
        Key::Named(NamedKey::F9) => KeyCode::F(9),
        Key::Named(NamedKey::F10) => KeyCode::F(10),
        Key::Named(NamedKey::F11) => KeyCode::F(11),
        Key::Named(NamedKey::F12) => KeyCode::F(12),
        Key::Named(NamedKey::F13) => KeyCode::F(13),
        Key::Named(NamedKey::F14) => KeyCode::F(14),
        Key::Named(NamedKey::F15) => KeyCode::F(15),
        Key::Named(NamedKey::F16) => KeyCode::F(16),
        Key::Named(NamedKey::F17) => KeyCode::F(17),
        Key::Named(NamedKey::F18) => KeyCode::F(18),
        Key::Named(NamedKey::F19) => KeyCode::F(19),
        Key::Named(NamedKey::F20) => KeyCode::F(20),
        Key::Named(NamedKey::F21) => KeyCode::F(21),
        Key::Named(NamedKey::F22) => KeyCode::F(22),
        Key::Named(NamedKey::F23) => KeyCode::F(23),
        Key::Named(NamedKey::F24) => KeyCode::F(24),
        _ => physical_key_fallback(physical).map_or(KeyCode::Char('\0'), KeyCode::Char),
    }
}

/// Whether `action` actually put bytes on the pty. A key winit cannot identify
/// (e.g. a synthesized focus-in event for an already-held modifier) resolves to
/// an empty `SendBytes`, which must not be folded into the prompt shadow as a
/// real keystroke: doing so would wrongly mark an untouched, truly empty line
/// as non-empty and desync undo/redo tracking until the next `Enter`.
pub(crate) fn forwarded_to_pty(action: &Action) -> bool {
    matches!(action, Action::SendBytes(bytes) if !bytes.is_empty())
}

/// Whether a bare Escape (no modifiers) in Insert mode should be written to the
/// PTY instead of switching to Normal mode: the foreground program runs its own
/// modes on Escape, or the shell is mid tab-completion and needs the real ESC
/// byte to close it.
fn escape_kept_in_insert(foreground: &[String], pending_tab_completion: bool) -> bool {
    pending_tab_completion
        || foreground
            .iter()
            .any(|name| ESCAPE_OWNING_PROGRAMS.contains(&name.as_str()))
}

/// Whether a bare Escape (no modifiers) in Normal mode, which Winter has
/// nothing to do with, should be written to the PTY as `0x1b`: a foreground
/// process owns the pane (a full-screen app, an agent CLI), or the pane is mid
/// the shell's own tab-completion (e.g. zsh's menu-select), shell-internal
/// state this app has no other way to observe, so it needs the real ESC byte
/// to close itself.
pub(crate) fn escape_forwarded_to_pty(
    has_foreground_process: bool,
    pending_tab_completion: bool,
) -> bool {
    has_foreground_process || pending_tab_completion
}

/// Whether a plain Escape should clear a mouse-drag selection instead of
/// falling through to its usual mode-switch/PTY-forward handling. Visual
/// mode is excluded: there Escape already clears the selection via the
/// Normal-mode switch, together with the mode's own anchor state.
fn escape_clears_selection(key: &input::Key, mode: Mode, has_selection: bool) -> bool {
    key.glyph() == KeyCode::Escape
        && !key.ctrl
        && !key.alt
        && !key.shift
        && mode != Mode::Visual
        && has_selection
}

/// Whether a `KeyboardInput` event should be dropped as a Windows-only race:
/// Win32 can dispatch a queued `WM_KEYDOWN` for the window that is about to
/// gain focus before it dispatches the matching `WM_SETFOCUS`, so the first
/// keystroke of an OS-level window switch (e.g. M-TAB's `Tab`) arrives
/// while `winit`'s cached focus state still reads unfocused.
#[cfg(target_os = "windows")]
fn is_pre_focus_key_leak(state: ElementState, window_has_focus: bool) -> bool {
    state == ElementState::Pressed && !window_has_focus
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::overlay::RenameInput;
    use crate::app::test_support::*;
    use crate::model::mode::Mode;
    use crate::terminal::session::Terminal;
    use winit::keyboard::{KeyCode as WinitKeyCode, NamedKey};

    #[test]
    fn test_only_keys_that_move_the_palette_selection_leave_its_caret_blinking() {
        let (mut app, _) = app_pane_and_id();
        let key = |code, ctrl| input::Key {
            alt: false,
            code,
            ctrl,
            shift: false,
            sup: false,
        };
        let down = key(KeyCode::Down, false);
        assert!(!app.moves_palette_selection(&down), "no palette, no list");
        app.overlay = Some(Overlay::Palette(crate::app::overlay::PaletteOverlay {
            palette: crate::model::palette::Palette::open(&app.keymap),
            pick: None,
        }));
        assert!(app.moves_palette_selection(&down));
        assert!(app.moves_palette_selection(&key(KeyCode::Up, false)));
        assert!(app.moves_palette_selection(&key(KeyCode::Char('n'), true)));
        assert!(!app.moves_palette_selection(&key(KeyCode::Char('n'), false)), "typed");
        assert!(!app.moves_palette_selection(&key(KeyCode::Backspace, false)));
    }

    #[test]
    fn test_a_click_leaves_the_block_cursor_and_a_drag_moves_it() {
        // Regression: a plain click in Normal mode parked the block cursor
        // under the pointer, so it seemed to appear wherever you clicked.
        let (mut app, id) = app_pane_and_id();
        app.open_tabs.set_mode(&id, Mode::Normal);
        app.set_nav_cursor(id, (1, 1));

        app.press_grid_cell(id, 3, 7);
        assert_eq!(app.nav_cursor(id), Some((1, 1)), "a click keeps the cursor");
        assert!(app.pointer.press_cell.is_some(), "a drag can still anchor");

        app.track_nav_cursor_to_mouse(id, 4, 2);
        assert_eq!(app.nav_cursor(id), Some((4, 2)), "a drag moves it");
    }

    #[test]
    fn test_the_wheel_scrolls_the_pane_under_the_pointer_not_the_focused_one() {
        let (mut app, left) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&left)
            .expect("pane")
            .grid_mut()
            .scroll_up(20);
        let right = app.alloc_tab_id();
        app.layout_mut()
            .split(crate::model::layout::Direction::Vertical, 0.5, right);
        let mut terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        terminal.grid_mut().scroll_up(20);
        app.open_tabs.insert_terminal(right, terminal);
        app.open_tabs.set_mode(&right, Mode::Insert);

        // Focus is on `right` after the split; point at `left`.
        assert_eq!(app.layout().focused(), right);
        let rect = app.pane_rect(left).expect("left is shown");
        app.pointer.cursor_pos = (rect.x + 1.0, rect.y + 1.0);

        app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, 1.0));

        let offset = |app: &App, id| app.open_tabs.terminal(&id).unwrap().grid().scroll_offset();
        assert!(
            offset(&app, left) > 0,
            "the pane under the pointer scrolled"
        );
        assert_eq!(offset(&app, right), 0, "the focused pane stayed put");
        assert_eq!(app.layout().focused(), right, "focus did not move");
    }

    /// A pane with a real PTY running `cat`, mirroring how a full-screen app
    /// holds the keyboard: a foreground process (cat) plus the alt screen.
    fn app_pane_and_id() -> (App, crate::model::layout::TabId) {
        let mut app = App::new();
        // Isolate from the developer's own ~/.config keybindings.
        app.keymap = crate::model::input::Keymap::default();
        let id = app.layout().shown_tabs()[0];
        let terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs.set_mode(&id, Mode::Insert);
        (app, id)
    }

    #[test]
    fn test_v_and_capital_v_in_a_tool_select_by_character_and_by_line() {
        // `V` used to do nothing until a `v` had started the cursor, so the
        // linewise selection Vim starts with one key took two here.
        let select = |typed: &str| {
            let mut app = App::new();
            let tab = app.show_tool(
                crate::app::tool::ToolKind::Dir,
                Box::new(crate::tools::dir::DirTool::new(std::env::temp_dir())),
            );
            let mut grid = winter_render::Grid::new(20, 2);
            for ch in "  alpha".chars() {
                grid.print(ch);
            }
            if let Some(slot) = app.open_tabs.tool_mut(&tab) {
                slot.painted = Some(grid);
                slot.cursor_line = Some(0);
            }
            app.type_committed_text(typed);
            app.selected_text()
        };
        assert_eq!(select("V").as_deref(), Some("  alpha"), "the whole line");
        assert_eq!(select("v").as_deref(), Some("a"), "the one character");
    }

    #[test]
    fn test_only_a_key_whose_press_reached_the_program_sends_its_release() {
        let (mut app, _) = app_pane_and_id();
        let route = |app: &mut App, state, key: input::Key| {
            let (logical, physical) = typed('a');
            app.route_key(state, key, logical, physical)
        };
        let a = input::Key::plain(KeyCode::Char('a'));
        route(&mut app, ElementState::Pressed, a);
        assert!(app.forwarded_presses.contains_key(&KeyCode::Char('a')));
        route(&mut app, ElementState::Released, a);
        assert!(app.forwarded_presses.is_empty());

        // `M-g` opens a window sequence: Winter keeps the press, so the
        // program must not hear the release either.
        let alt_g = input::Key::with_alt(KeyCode::Char('g'));
        route(&mut app, ElementState::Pressed, alt_g);
        assert!(app.is_sequence_pending());
        assert!(!app.forwarded_presses.contains_key(&KeyCode::Char('g')));
    }

    /// The key a plain character `c` is, as winit hands it over.
    fn typed(c: char) -> (Key, PhysicalKey) {
        (
            Key::Character(c.to_string().into()),
            PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
        )
    }

    #[test]
    fn test_a_tool_search_stays_lit_after_enter_until_esc_and_n_lights_it_again() {
        let dir = std::env::temp_dir().join(format!("winter-tool-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        for name in ["alpha.txt", "beta.txt", "alphabet.md"] {
            std::fs::write(dir.join(name), "").expect("write");
        }
        let mut app = App::new();
        app.keymap = crate::model::input::Keymap::default();
        app.show_tool(
            super::super::tool::ToolKind::Dir,
            Box::new(crate::tools::dir::DirTool::new(dir.clone())),
        );
        let press = |app: &mut App, (logical, physical): (Key, PhysicalKey)| {
            assert!(!app.handle_key(ElementState::Pressed, logical, physical));
        };

        for c in "/alpha".chars() {
            press(&mut app, typed(c));
        }
        press(
            &mut app,
            (
                Key::Named(NamedKey::Enter),
                PhysicalKey::Code(WinitKeyCode::Enter),
            ),
        );
        assert!(app.tool_prompt().is_none(), "Enter answered the question");
        assert_eq!(app.search.query.as_deref(), Some("alpha"), "still lit");
        // From `alpha.txt`, the search looks past the cursor first.
        assert_eq!((app.search.match_index, app.search.match_total), (2, 2));

        press(&mut app, esc());
        assert_eq!(app.search.query, None, "Esc put the highlight away");

        press(&mut app, typed('n'));
        assert_eq!(app.search.query.as_deref(), Some("alpha"), "n lit it again");
        assert_eq!((app.search.match_index, app.search.match_total), (1, 2));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn esc() -> (Key, PhysicalKey) {
        (
            Key::Named(NamedKey::Escape),
            PhysicalKey::Code(WinitKeyCode::Escape),
        )
    }

    /// Whether the pane's terminal has echoed an ESC byte the program on it
    /// was sent, waiting up to `wait` for it to arrive.
    fn echoed_escape(app: &mut App, id: crate::model::layout::TabId, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            let terminal = app.open_tabs.terminal_mut(&id).expect("pane");
            terminal.drain_output();
            if terminal.grid().to_text().contains("^[") {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn test_text_an_input_method_commits_is_typed_into_the_terminal() {
        // Composed Vietnamese, as an input method commits it: precomposed
        // letters beyond ASCII, plus an uppercase one, with Ctrl still held
        // from picking the candidate. It must arrive as the text itself.
        let (mut app, id) = app_pane_and_id();
        app.modifiers = winit::keyboard::ModifiersState::CONTROL.into();
        assert!(!app.type_committed_text("Tiếng Việt"));
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut echoed = String::new();
        while Instant::now() < deadline && !echoed.contains("Tiếng Việt") {
            let terminal = app.open_tabs.terminal_mut(&id).expect("pane");
            terminal.drain_output();
            echoed = terminal.grid().to_text();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(echoed.contains("Tiếng Việt"), "echoed: {echoed:?}");
    }

    #[test]
    fn test_first_escape_switches_to_normal_in_a_program_without_modes() {
        // `cat` stands in for an agent CLI like Claude Code: a full-screen
        // program with no modes of its own, so Winter takes the first Escape.
        let (mut app, id) = app_pane_and_id();
        app.config.status_bar.enabled = false;
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(
            app.open_tabs.mode(&id).unwrap(),
            Mode::Normal,
            "Winter takes the first Escape"
        );
        assert!(app.notice_text().is_none());
        assert!(
            !echoed_escape(&mut app, id, Duration::from_millis(300)),
            "the program was sent nothing"
        );
    }

    #[test]
    fn test_alt_escape_switches_to_normal_mode() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.modifiers = winit::keyboard::ModifiersState::ALT.into();

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
    }

    #[test]
    fn test_escape_in_normal_mode_reaches_the_program_when_winter_has_nothing_to_cancel() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.open_tabs.set_mode(&id, Mode::Normal);

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
        assert!(
            echoed_escape(&mut app, id, Duration::from_secs(3)),
            "the program received the Escape"
        );
    }

    #[test]
    fn test_escape_in_normal_mode_cancels_a_half_typed_command_before_the_program_sees_it() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.open_tabs.set_mode(&id, Mode::Normal);
        app.pending = crate::model::input::PendingPrefix::Count(3);

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.pending, crate::model::input::PendingPrefix::None);
        assert!(
            !echoed_escape(&mut app, id, Duration::from_millis(300)),
            "Winter spent this Escape"
        );
    }

    #[test]
    fn test_mode_switch_is_silent_while_the_status_bar_is_shown() {
        let (mut app, id) = app_pane_and_id();
        app.config.status_bar.enabled = true;
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        // The entry chord switches even inside a full-screen app, and raises
        // no notice.
        app.modifiers = winit::keyboard::ModifiersState::ALT.into();
        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));
        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
        assert!(app.notice_text().is_none());
    }

    #[test]
    fn test_a_modifier_going_down_types_nothing_into_the_text_being_edited() {
        // Regression: Ctrl alone decodes to the NUL character, which every
        // text field that took a character wrote in as a stray glyph.
        let (mut app, _id) = app_pane_and_id();
        let tab = app.layout().focused();
        app.open_overlay(Overlay::Rename(RenameInput::new(tab, "ab")));
        let pressed = app.handle_key(
            ElementState::Pressed,
            Key::Named(NamedKey::Control),
            PhysicalKey::Code(WinitKeyCode::ControlLeft),
        );
        assert!(!pressed);
        assert_eq!(
            app.rename_input().map(|input| input.field.text()),
            Some("ab")
        );
    }

    #[test]
    fn test_a_rename_names_the_tab_it_was_started_on_though_the_focus_moved() {
        // Regression: the name went to whichever tab was focused at Enter,
        // so a click on another pane mid-rename renamed that one instead.
        let (mut app, renamed) = app_pane_and_id();
        app.run_command("rename_tab", renamed);
        let other = app.alloc_tab_id();
        app.layout_mut().add_tab(other);
        assert_ne!(app.layout().focused(), renamed, "the focus moved on");
        for c in ['h', 'i'] {
            let (logical, physical) = typed(c);
            app.handle_key(ElementState::Pressed, logical, physical);
        }
        app.handle_key(
            ElementState::Pressed,
            Key::Named(NamedKey::Enter),
            PhysicalKey::Code(WinitKeyCode::Enter),
        );
        assert_eq!(app.tabs.name(renamed), Some("hi"));
        assert_eq!(app.tabs.name(other), None);
    }

    #[test]
    fn test_a_tab_name_is_edited_at_its_caret_with_the_emacs_keys() {
        let (mut app, renamed) = app_pane_and_id();
        app.tabs.set_name(renamed, Some("hello".to_string()));
        app.run_command("rename_tab", renamed);
        let (logical, physical) = typed('a');
        let ctrl_a = input::Key {
            ctrl: true,
            ..input::Key::plain(KeyCode::Char('a'))
        };
        app.route_key(ElementState::Pressed, ctrl_a, logical, physical);
        let (logical, physical) = typed('x');
        app.route_key(
            ElementState::Pressed,
            input::Key::plain(KeyCode::Char('x')),
            logical,
            physical,
        );
        assert_eq!(
            app.rename_input().map(|input| input.field.text()),
            Some("xhello")
        );
        assert_eq!(
            app.strip_title(renamed),
            "x\u{2502}hello",
            "the caret where it is"
        );
    }

    #[test]
    fn test_a_release_goes_to_the_tab_its_press_went_to() {
        // Regression: the release went to whichever tab was focused when it
        // came, a key-up for a key that program never saw go down.
        let (mut app, pressed_in) = app_pane_and_id();
        let a = input::Key::plain(KeyCode::Char('a'));
        let (logical, physical) = typed('a');
        app.route_key(ElementState::Pressed, a, logical.clone(), physical);
        assert_eq!(
            app.forwarded_presses.get(&KeyCode::Char('a')),
            Some(&pressed_in)
        );
        let other = app.alloc_tab_id();
        app.layout_mut().add_tab(other);
        app.route_key(ElementState::Released, a, logical, physical);
        assert!(
            app.forwarded_presses.is_empty(),
            "the release was routed, though to a tab no longer focused"
        );
    }

    #[test]
    fn test_alt_escape_switches_to_normal_at_a_bare_prompt() {
        // A real shell sitting at its prompt: primary screen, no foreground
        // child, M-ESC switches modes.
        let mut app = App::new();
        app.keymap = crate::model::input::Keymap::default();
        let id = app.layout().shown_tabs()[0];
        let mut terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("bash"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        for _ in 0..150 {
            terminal.drain_output();
            if !terminal.has_foreground_process() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs.set_mode(&id, Mode::Insert);
        app.modifiers = winit::keyboard::ModifiersState::ALT.into();
        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));
        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
    }

    #[test]
    fn test_winit_key_to_code_chars() {
        let placeholder = PhysicalKey::Code(winit::keyboard::KeyCode::KeyA);
        assert_eq!(
            winit_key_to_code(&Key::Character("a".into()), &placeholder),
            KeyCode::Char('a')
        );
        assert_eq!(
            winit_key_to_code(&Key::Named(NamedKey::Enter), &placeholder),
            KeyCode::Enter
        );
        assert_eq!(
            winit_key_to_code(&Key::Named(NamedKey::Space), &placeholder),
            KeyCode::Space
        );
    }

    #[test]
    fn test_winit_key_to_code_falls_back_to_physical_key_when_logical_is_unidentified() {
        // Windows reports no character at all for some Ctrl+Alt digit/letter
        // combos on certain keyboard layouts; the physical key position must
        // still resolve so those chords aren't silently lost.
        let unidentified =
            Key::Unidentified(winit::keyboard::NativeKeyCode::Windows(0x0032).into());
        let physical = PhysicalKey::Code(winit::keyboard::KeyCode::Digit2);
        assert_eq!(
            winit_key_to_code(&unidentified, &physical),
            KeyCode::Char('2')
        );
    }

    #[test]
    fn test_forwarded_to_pty_rejects_empty_send_bytes() {
        // A key winit could not identify encodes to zero bytes (see
        // `winit_key_to_code`'s `Char('\0')` fallback); it must not count as
        // forwarded, or the prompt shadow would wrongly desync/mutate for a key
        // that never reached the shell.
        assert!(!forwarded_to_pty(&Action::SendBytes(Vec::new())));
        assert!(forwarded_to_pty(&Action::SendBytes(vec![0x04])));
        assert!(!forwarded_to_pty(&Action::Ignore));
    }

    #[test]
    fn test_escape_in_insert_is_kept_only_by_modal_programs_or_a_pending_completion() {
        let group =
            |names: &[&str]| -> Vec<String> { names.iter().map(|name| name.to_string()).collect() };
        assert!(escape_kept_in_insert(&group(&["nvim"]), false));
        assert!(escape_kept_in_insert(&group(&["hx"]), false));
        assert!(escape_kept_in_insert(&group(&["snow"]), false));
        assert!(!escape_kept_in_insert(&group(&["claude"]), false));
        assert!(!escape_kept_in_insert(&[], false));
        assert!(escape_kept_in_insert(&[], true));
        // `make dev` leads the foreground group, with the editor inside it.
        assert!(escape_kept_in_insert(
            &group(&["make", "sh", "snow"]),
            false
        ));
        assert!(!escape_kept_in_insert(&group(&["make", "cargo"]), false));
    }

    #[test]
    fn test_escape_forwarded_to_pty_on_foreground_process_or_pending_completion() {
        // Regression: a bare Escape used to switch straight to Normal mode at
        // any bare shell prompt, even one mid the shell's own tab-completion
        // (e.g. zsh's menu-select) - the completion was left open with Normal
        // mode active underneath it, since the real ESC byte never reached
        // the shell to cancel it.
        assert!(escape_forwarded_to_pty(true, false));
        assert!(escape_forwarded_to_pty(false, true));
        assert!(escape_forwarded_to_pty(true, true));
        assert!(!escape_forwarded_to_pty(false, false));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_pre_focus_key_leak_swallows_press_before_focus() {
        // Regression: M-TAB into the window can dispatch the queued
        // Tab KeyboardInput before the Focused(true) event, so `has_focus()`
        // still reads false when the leaked press arrives.
        assert!(is_pre_focus_key_leak(ElementState::Pressed, false));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_pre_focus_key_leak_ignores_press_once_focused() {
        assert!(!is_pre_focus_key_leak(ElementState::Pressed, true));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn test_is_pre_focus_key_leak_ignores_release_events() {
        // A Released event never reaches the PTY on its own (see
        // `encode_release`), so it must not be swallowed even if it arrives
        // while `has_focus()` still reads false.
        assert!(!is_pre_focus_key_leak(ElementState::Released, false));
    }

    #[test]
    fn test_escape_clears_selection_outside_visual_mode() {
        let key = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert!(escape_clears_selection(&key, Mode::Insert, true));
        assert!(escape_clears_selection(&key, Mode::Normal, true));
    }

    #[test]
    fn test_escape_clears_selection_false_without_a_selection() {
        let key = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert!(!escape_clears_selection(&key, Mode::Insert, false));
    }

    #[test]
    fn test_escape_clears_selection_false_in_visual_mode() {
        // Regression: Visual mode's own Escape handling (the Normal-mode
        // switch in `handle_action`) already clears the selection together
        // with the visual anchor; double-handling it here would be redundant
        // and, if the ordering ever changed, could race that clear.
        let key = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
            sup: false,
        };
        assert!(!escape_clears_selection(&key, Mode::Visual, true));
    }

    #[test]
    fn test_escape_clears_selection_false_with_modifiers() {
        let shift_escape = input::Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: true,
            sup: false,
        };
        assert!(!escape_clears_selection(&shift_escape, Mode::Insert, true));
    }

    #[test]
    fn test_scrolling_a_full_screen_app_drops_the_stale_selection() {
        // Regression: a selection names absolute grid rows, which follow the
        // text through Winter's own scrolling but cannot follow a full-screen
        // app scrolling its own content. The wheel goes to the app, the app
        // repaints, the grid rows never move, and the highlight was left
        // sitting over whatever the app painted there: copying it returned
        // text the user never pointed at.
        let mut app = App::new();
        // The default tab lays out a single pane, `TabId(0)`.
        let id = TabId(0);
        let mut terminal = pane_with_scrollback();
        terminal.grid_mut().enter_alt_screen();
        // DECSET ?1000: the app asks for mouse reporting, so the wheel is
        // forwarded to it rather than scrolling Winter's own view.
        terminal.grid_mut().set_private_mode(1000, true);
        app.open_tabs.insert_terminal(id, terminal);
        app.selection.span = Some(Selection {
            block: false,
            start_row: 3,
            start_col: 0,
            end_row: 5,
            end_col: 10,
            tab: id,
        });

        app.on_mouse_wheel(winit::event::MouseScrollDelta::LineDelta(0.0, -1.0));

        assert!(
            app.selection.span.is_none(),
            "the text under the highlight is about to change, so the selection goes"
        );
    }

    #[test]
    fn test_ctrl_shift_v_pastes_whichever_case_the_letter_arrives_in() {
        let app = App::new();
        let chord = |code: char, ctrl: bool, shift: bool| input::Key {
            alt: false,
            code: KeyCode::Char(code),
            ctrl,
            shift,
            sup: false,
        };
        assert!(app.is_paste_key(&chord('v', true, true)));
        assert!(
            app.is_paste_key(&chord('V', true, true)),
            "the capital winit hands over"
        );
        assert!(
            !app.is_paste_key(&chord('v', true, false)),
            "C-v is the terminal's"
        );
        assert!(!app.is_paste_key(&chord('c', true, true)));
    }
}

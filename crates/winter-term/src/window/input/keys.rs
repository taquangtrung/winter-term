//! Where a key goes: each press routed through the layers that may claim it
//! (a rename being typed, a pending sequence, the overlay, the palette, the
//! focused editor, the keymap) before what is left reaches the program in
//! the terminal, and its release sent after it.

use std::time::Instant;

use winit::event::{ElementState, KeyEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey, NativeKeyCode, PhysicalKey};

use crate::model::input::{self, Action, KeyCode, Mods};
use crate::model::layout::TabId;
use crate::model::mode::{Mode, ModeEvent};
use crate::model::textedit::modal::EditOutcome;

use crate::window::editor::Closing;
use crate::window::input::overlay::Overlay;
use crate::window::terminal::prompt_edit::PromptShadow;
use crate::window::{App, CURSOR_BLINK_PERIOD};

// ========================================================================
// Constants
// ========================================================================

/// The letter that, with Ctrl and Shift, pastes into whatever takes input,
/// whatever else the keymap binds paste to.
const PASTE_LETTER: char = 'v';

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

/// One layer of key routing.
type KeyLayer = fn(&mut App, &KeyPress) -> KeyRoute;

/// One key press on its way through the layers that may take it, with what
/// each layer reads about where it landed.
struct KeyPress {
    focused: TabId,
    key: input::Key,
    /// The winit key it came from, which the palette and settings page read
    /// for text entry.
    logical: Key,
    /// The focused terminal's mode; the default for an editor, which has none.
    mode: Mode,
    physical: PhysicalKey,
    editor_focused: bool,
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

pub(in crate::window) fn winit_key_to_code(key: &Key, physical: &PhysicalKey) -> KeyCode {
    match key.as_ref() {
        Key::Character(c) => match c.chars().next().unwrap_or('\0') {
            ctrl @ '\x01'..='\x1a' => KeyCode::Char(((ctrl as u8 - 1) + b'a') as char),
            ch => KeyCode::Char(ch),
        },
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
pub(in crate::window) fn forwarded_to_pty(action: &Action) -> bool {
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
pub(in crate::window) fn escape_forwarded_to_pty(
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
    key.chord() == (Mods::NONE, KeyCode::Escape) && mode != Mode::Visual && has_selection
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
// App: key routing
// ========================================================================

impl App {
    /// Route one key press or release: the chrome overlays first (command
    /// palette, settings page, which-key), then the focused pane's modal
    /// keymap, and finally the PTY.
    pub(in crate::window) fn on_keyboard_input(
        &mut self,
        event: KeyEvent,
        event_loop: &ActiveEventLoop,
    ) {
        // A key held down after opening a sequence auto-repeats; the repeats
        // are the same press, not the next chord of the sequence.
        if event.repeat && self.input.is_sequence_pending() {
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
    pub(in crate::window) fn handle_key(
        &mut self,
        state: ElementState,
        logical_key: Key,
        physical_key: PhysicalKey,
    ) -> bool {
        // winit synthesizes KeyboardInput::Pressed events for every
        // physically-held key on XI_FocusIn (handle_pressed_keys). Swallow
        // those here so e.g. the Tab from M-TAB never reaches the PTY.
        if state == ElementState::Pressed && self.input.suppress_synthesized_keys {
            return false;
        }

        // Windows-only: drop a key event that raced ahead of this
        // window's own focus-gain notification (see
        // `is_pre_focus_key_leak`'s doc for why this can happen).
        #[cfg(target_os = "windows")]
        if is_pre_focus_key_leak(state, self.window.as_ref().is_some_and(|w| w.has_focus())) {
            return false;
        }

        let mods_state = self.input.modifiers.state();
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
    /// a shell, an editor, or the Normal-mode keymap exactly as typing it on a
    /// keyboard would, whatever modifiers happen to be held while the
    /// candidate is picked. Returns `true` when the app should quit.
    pub(in crate::window) fn type_committed_text(&mut self, text: &str) -> bool {
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
        self.chrome.picker().is_some()
            && matches!(
                key.chord(),
                (Mods::NONE, KeyCode::Up | KeyCode::Down) | (Mods::CTRL, KeyCode::Char('n' | 'p'))
            )
    }

    /// Route `key` (with the winit key it came from, which the palette and
    /// settings page read for text entry) through the overlays, the focused
    /// pane's keymap, and on to the PTY. Returns `true` when the app should
    /// quit.
    pub(in crate::window) fn route_key(
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
            self.blink.phase = true;
            self.blink.next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
        }

        let focused = self.layout().focused();
        let press = KeyPress {
            focused,
            key,
            logical: logical_key,
            // `mode` is the terminal's, and only means anything when the tab
            // is not an editor, which takes its own keys rather than being in a
            // mode.
            mode: self.open_tabs.mode(&focused).unwrap_or_default(),
            physical: physical_key,
            editor_focused: self.open_tabs.is_editor(&focused),
        };

        // If the previous action was macro replay and user presses 'e', repeat the replay.
        if self.macro_state.last_was_replay {
            if key.chord() == (Mods::NONE, KeyCode::Char('e')) {
                self.replay_macro(None);
                return self.finish_key();
            } else {
                self.macro_state.last_was_replay = false;
            }
        }

        // In normal mode, 'q' while recording stops recording
        if self.macro_state.is_recording()
            && press.mode == Mode::Normal
            && !press.editor_focused
            && key.chord() == (Mods::NONE, KeyCode::Char('q'))
        {
            self.stop_macro_recording();
            return self.finish_key();
        }

        // Buffer the key if currently recording
        if self.macro_state.is_recording() && self.macro_state.replay_depth == 0 {
            self.macro_state.buffer.push(key);
        }

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
            Self::route_editor_cursor_layer,
            Self::route_exit_shown_key,
            Self::route_sequence_start_key,
            Self::route_editor_key,
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
        if self.runtime.exit.take_requested() {
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
        let forwarded = self.input.keys.forwarded.remove(&key.code); // key-exact: the physical key a press went out on
        let Some(tab) = forwarded else {
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

    /// The paste chord pastes into whatever takes input, ahead of every layer
    /// that would otherwise take it as a key: a dialog's field, the palette,
    /// an editor, or the terminal.
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
        let fixed = key.chord() == (Mods::CTRL_SHIFT, KeyCode::Char(PASTE_LETTER));
        fixed || self.keymap.global_action(key) == Some(Action::Paste)
    }

    /// While a tab rename is in progress it takes every key: Enter names the
    /// tab the rename was started on, Escape or `C-g` cancels, and the rest
    /// edit the name as any field's Emacs keys do.
    fn route_rename_key(&mut self, press: &KeyPress) -> KeyRoute {
        let Some(Overlay::Rename(input)) = &mut self.chrome.overlay else {
            return KeyRoute::Pass;
        };
        match input.field.on_key(&press.key.typed(), 1) {
            EditOutcome::Submit => {
                let tab = input.tab;
                let name = input.field.text().to_string();
                self.chrome.overlay = None;
                self.tabs.set_name(tab, Some(name));
            }
            EditOutcome::Cancel => self.chrome.overlay = None,
            _ => {}
        }
        KeyRoute::Handled
    }

    /// A sequence part-way typed takes the next key ahead of everything, the
    /// global chords included: continuing it is the key's only reading.
    fn route_pending_sequence_key(&mut self, press: &KeyPress) -> KeyRoute {
        if !self.input.is_sequence_pending() {
            return KeyRoute::Pass;
        }
        KeyRoute::of_sequence(self.route_sequence_key(press.focused, &press.key))
    }

    /// A question, a name being typed, or the settings page owns the keyboard
    /// while it is up, ahead of the window chords: a filename being typed
    /// must not trigger one, and no key reaches the PTY underneath.
    fn route_modal_key(&mut self, press: &KeyPress) -> KeyRoute {
        match &mut self.chrome.overlay {
            Some(Overlay::Close(_)) => self.handle_close_prompt_key(&press.key),
            Some(Overlay::EditorPrompt(_)) => self.handle_prompt_key(&press.key),
            Some(Overlay::Password(_)) => self.handle_password_key(&press.key),
            Some(Overlay::ZaiKey(_)) => self.handle_zai_key_key(&press.key),
            Some(Overlay::ThemeName(input)) => match input.on_key(&press.key.typed(), 1) {
                EditOutcome::Submit => {
                    let name = input.text().to_string();
                    self.chrome.overlay = None;
                    self.create_named_theme(&name);
                }
                EditOutcome::Cancel => self.chrome.overlay = None,
                _ => {}
            },
            // Edits apply live, Enter and Escape close it.
            Some(Overlay::Settings(_)) => {
                if let Some(mut page) = self.chrome.take_settings_page() {
                    if self.handle_settings_input(&mut page, &press.logical, &press.key) {
                        self.chrome.overlay = Some(Overlay::Settings(page));
                    } else {
                        self.on_settings_closed();
                    }
                }
            }
            Some(Overlay::Picker(_) | Overlay::Rename(_)) | None => return KeyRoute::Pass,
        }
        KeyRoute::Handled
    }

    /// Escape puts away an open menu, then a mouse-drag selection, before
    /// anything else acts on it.
    fn route_dismissing_escape(&mut self, press: &KeyPress) -> KeyRoute {
        let escape = press.key.chord() == (Mods::NONE, KeyCode::Escape);
        if self.chrome.menus.open.is_some() && escape {
            self.close_menu();
            return KeyRoute::Handled;
        }
        if self.host.token_inspect.is_some() && escape {
            self.dismiss_token_inspect();
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
            .filter(|action| !self.is_copy_yielded_to_editor(action, press.focused));
        let Some(action) = global else {
            return KeyRoute::Pass;
        };
        self.handle_action(action, press.focused);
        KeyRoute::Handled
    }

    /// The open palette takes every key that is not a global chord.
    fn route_palette_key(&mut self, press: &KeyPress) -> KeyRoute {
        if self.chrome.picker().is_none() {
            return KeyRoute::Pass;
        }
        self.press_palette_key(&press.logical, &press.physical, press.focused);
        KeyRoute::Handled
    }

    /// The editor text cursor takes every key while it is up, ahead of the
    /// editor itself: sharing keys would move the selection and the listing
    /// at once. See `editor_cursor`.
    fn route_editor_cursor_layer(&mut self, press: &KeyPress) -> KeyRoute {
        KeyRoute::taken(self.handle_editor_cursor_key(&press.key))
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
        self.close_tab_now(press.focused, Closing::Forget);
        KeyRoute::Handled
    }

    /// A key opening a sequence (the window's, or the focused editor's own
    /// block's) is the sequence's, ahead of the editor and the mode keymaps
    /// alike, so `M-g M-h` reads the same in an editor as in a terminal.
    fn route_sequence_start_key(&mut self, press: &KeyPress) -> KeyRoute {
        KeyRoute::of_sequence(self.route_sequence_key(press.focused, &press.key))
    }

    /// An editor owns the keys it binds, and only those: anything it declines
    /// falls through to the window chords, so the vim vocabulary reads the
    /// same in an editor as in a terminal. `v` or `V` in an editor that did not
    /// bind it starts a text cursor selecting by character or by whole line,
    /// the way each starts a Visual selection in a terminal.
    fn route_editor_key(&mut self, press: &KeyPress) -> KeyRoute {
        if self.offer_key_to_editor(press.focused, &press.key) {
            return KeyRoute::Handled;
        }
        // A motion along a row the editor did not claim moves the column
        // cursor every editor gets on its cursor row.
        if press.editor_focused && self.editor_column_key(press.focused, &press.key) {
            return KeyRoute::Handled;
        }
        let key = &press.key;
        let chord = key.chord();
        let starts_cursor =
            press.editor_focused && matches!(chord, (Mods::NONE, KeyCode::Char('v' | 'V')));
        if starts_cursor {
            self.start_editor_cursor(press.focused);
            // The cursor starts selecting by character; `V` is what turns a
            // selection linewise once it is up.
            if chord == (Mods::NONE, KeyCode::Char('V')) {
                self.handle_editor_cursor_key(key);
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
        let bare_esc = press.key.chord() == (Mods::NONE, KeyCode::Escape);
        if !bare_esc || press.editor_focused {
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
            let code = press.key.code; // key-exact: the physical key whose release follows
            self.input.keys.forwarded.insert(code, focused);
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
        let prev_pending = self.input.keys.prefix;
        let action = match press.editor_focused {
            true => input::resolve_editor(key, &self.keymap),
            false => input::resolve_with(
                press.mode,
                key,
                &mut self.input.keys.prefix,
                &self.keymap,
                kitty_flags,
                modify_other_keys,
                is_alt_screen,
            ),
        };
        let action = input::apply_cursor_key_mode(action, app_cursor_keys, kitty_flags);
        if forwarded_to_pty(&action) {
            let code = key.code; // key-exact: the physical key whose release follows
            self.input.keys.forwarded.insert(code, focused);
        }
        if self.input.keys.prefix != prev_pending {
            self.input.keys.prefix_since = if self.input.keys.prefix.hint().is_some() {
                Some(Instant::now())
            } else {
                None
            };
        } else if self.input.keys.prefix == input::PendingPrefix::None {
            self.input.keys.prefix_since = None;
        }
        // Keep the per-pane prompt shadow in step with this key so
        // `C-/`/`C-\` can replay edits. Only forwarded keys mutate the line
        // (`apply_insert_key` models them or desyncs on the unmodeled ones);
        // `Edit`/undo/redo update it in their handlers. Mode switches, focus
        // and tab commands leave this line untouched, so the shadow is
        // preserved (undo still works in Normal mode).
        if !press.editor_focused && press.mode == Mode::Insert && forwarded_to_pty(&action) {
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
    /// the editor in tab `focused` instead: with nothing to copy it
    /// would do nothing, and an editor may bind the same chord (the Git editor
    /// commits on `C-S-c`). An editor that declines it still reaches the
    /// ordinary keymap, where it resolves to the same harmless copy.
    fn is_copy_yielded_to_editor(&self, action: &input::Action, focused: TabId) -> bool {
        *action == input::Action::Copy
            && self.chrome.picker().is_none()
            && self.open_tabs.is_editor(&focused)
            && self.selected_text().is_none()
    }

    /// Bookkeeping for one key forwarded to the PTY while the terminal is in
    /// Insert mode: keep the prompt shadow in step so `C-/`/`C-\\` can
    /// replay edits, and accumulate the exact bytes into the Insert session's
    /// `.`-repeat run (finalized when the pane leaves Insert: see
    /// [`App::track_change`]). Extracted from the keyboard handler so tests
    /// can drive the same path the event loop does.
    pub(in crate::window) fn record_insert_key(
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
            tab.pending_tab_completion = key.chord() == (Mods::NONE, KeyCode::Tab);
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
    pub(in crate::window) fn escape_has_winter_target(&self) -> bool {
        self.input.keys.prefix != input::PendingPrefix::None
            || self.search.query.is_some()
            || self.chrome.labels.are_up()
            || self.host.token_inspect.is_some()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use winit::keyboard::{KeyCode as WinitKeyCode, NamedKey};

    use crate::model::mode::Mode;
    use crate::terminal::session::Terminal;
    use crate::window::input::overlay::RenameInput;
    use crate::window::test_support::*;

    use super::*;

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
        app.chrome.overlay = Some(Overlay::Picker(
            crate::window::input::overlay::PickerOverlay {
                picker: crate::model::picker::Picker::open(crate::window::commands::builtin_commands(&app.keymap)),
                pick: None,
            },
        ));
        assert!(app.moves_palette_selection(&down));
        assert!(app.moves_palette_selection(&key(KeyCode::Up, false)));
        assert!(app.moves_palette_selection(&key(KeyCode::Char('n'), true)));
        assert!(
            !app.moves_palette_selection(&input::Key::with_ctrl_shift(KeyCode::Char('n'))),
            "C-S-n is not C-n"
        );
        assert!(
            !app.moves_palette_selection(&key(KeyCode::Char('n'), false)),
            "typed"
        );
        assert!(!app.moves_palette_selection(&key(KeyCode::Backspace, false)));
    }

    #[test]
    fn test_v_and_capital_v_in_an_editor_select_by_character_and_by_line() {
        // `V` used to do nothing until a `v` had started the cursor, so the
        // linewise selection Vim starts with one key took two here.
        let select = |typed: &str| {
            let mut app = App::new();
            let tab = app.show_editor(
                crate::window::editor::EditorKind::Notifications,
                Box::new(crate::editors::notifications::NotificationsEditor::new(
                    Vec::new(),
                    0,
                )),
            );
            let mut grid = winter_core::Grid::new(20, 2);
            for ch in "  alpha".chars() {
                grid.print(ch);
            }
            if let Some(slot) = app.open_tabs.editor_mut(&tab) {
                slot.view.painted = Some(grid);
                slot.view.cursor_line = Some(0);
            }
            app.type_committed_text(typed);
            app.selected_text()
        };
        assert_eq!(select("V").as_deref(), Some("  alpha"), "the whole line");
        assert_eq!(select("v").as_deref(), Some("a"), "the one character");
    }

    #[test]
    fn test_dir_editor_cursor_stays_at_leftmost_column_when_moving_by_jk() {
        let dir = std::env::temp_dir().join(format!("winter-dir-jk-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let mut app = App::new();
        let tab = app.show_editor(
            crate::window::editor::EditorKind::Dir,
            Box::new(crate::editors::dir::DirEditor::new(dir.clone())),
        );
        let mut grid = winter_core::Grid::new(40, 3);
        // Row 0 has 2 leading spaces; row 1 has 7 leading spaces
        for ch in "  folder/".chars() {
            grid.print(ch);
        }
        grid.move_to(1, 0);
        for ch in "       file.txt".chars() {
            grid.print(ch);
        }
        if let Some(slot) = app.open_tabs.editor_mut(&tab) {
            slot.view.painted = Some(grid);
            slot.view.cursor_line = Some(0);
        }

        // On row 0, column is at the leftmost column 0
        assert_eq!(app.editor_column(tab, 0), Some(0));

        // Move down by 'j'
        let j = input::Key::plain(KeyCode::Char('j'));
        let (logical, physical) = typed('j');
        app.route_key(ElementState::Pressed, j, logical.clone(), physical);
        app.route_key(ElementState::Released, j, logical, physical);

        // On row 1, column still stays at column 0 instead of jumping to 7
        assert_eq!(app.editor_column(tab, 1), Some(0));

        // Move back up by 'k'
        let k = input::Key::plain(KeyCode::Char('k'));
        let (logical, physical) = typed('k');
        app.route_key(ElementState::Pressed, k, logical.clone(), physical);
        app.route_key(ElementState::Released, k, logical, physical);

        // On row 0, column still stays at column 0
        assert_eq!(app.editor_column(tab, 0), Some(0));

        let _ = std::fs::remove_dir_all(&dir);
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
        assert!(app.input.keys.forwarded.contains_key(&KeyCode::Char('a')));
        route(&mut app, ElementState::Released, a);
        assert!(app.input.keys.forwarded.is_empty());

        // `M-g` opens a window sequence: Winter keeps the press, so the
        // program must not hear the release either.
        let alt_g = input::Key::with_alt(KeyCode::Char('g'));
        route(&mut app, ElementState::Pressed, alt_g);
        assert!(app.input.is_sequence_pending());
        assert!(!app.input.keys.forwarded.contains_key(&KeyCode::Char('g')));
    }

    #[test]
    fn test_ctrl_alt_digit_closes_that_pane_from_an_editor_pane_too() {
        // 1 | 2 over 3, with the focus in 2: the chord closing pane 3 was
        // offered to the focused editor first and never reached the window.
        let mut app = App::new();
        app.keymap = crate::model::input::Keymap::default();
        let dir = std::env::temp_dir();
        app.show_editor(
            crate::window::editor::EditorKind::Dir,
            Box::new(crate::editors::dir::DirEditor::new(dir)),
        );
        app.split_pane(crate::model::layout::Direction::Vertical);
        app.split_pane(crate::model::layout::Direction::Horizontal);
        let panes = app.layout().shown_tabs();
        assert_eq!(panes.len(), 3);
        let (two, three) = (panes[1], panes[2]);
        app.set_focus(two);
        let ctrl_alt_3 = input::Key {
            alt: true,
            code: KeyCode::Char('3'),
            ctrl: true,
            shift: false,
            sup: false,
        };
        let (logical, physical) = typed('3');
        app.route_key(ElementState::Pressed, ctrl_alt_3, logical, physical);
        let left = app.layout().shown_tabs();
        assert_eq!(left.len(), 2, "pane 3 closed: {left:?}");
        assert!(!left.contains(&three));
        // The same path focuses a pane by its number.
        let alt_1 = input::Key::with_alt(KeyCode::Char('1'));
        let (logical, physical) = typed('1');
        app.route_key(ElementState::Pressed, alt_1, logical, physical);
        assert_eq!(app.layout().focused(), left[0], "M-1 went to pane 1");
    }

    /// The key a plain character `c` is, as winit hands it over.
    fn typed(c: char) -> (Key, PhysicalKey) {
        (
            Key::Character(c.to_string().into()),
            PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified),
        )
    }

    #[test]
    fn test_an_editor_search_stays_lit_after_enter_until_esc_and_n_lights_it_again() {
        let dir = std::env::temp_dir().join(format!("winter-tool-search-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        for name in ["alpha.txt", "beta.txt", "alphabet.md"] {
            std::fs::write(dir.join(name), "").expect("write");
        }
        let mut app = App::new();
        app.keymap = crate::model::input::Keymap::default();
        app.show_editor(
            crate::window::editor::EditorKind::Dir,
            Box::new(crate::editors::dir::DirEditor::new(dir.clone())),
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
        assert!(app.chrome.editor_prompt().is_none(), "Enter answered the question");
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
        app.input.modifiers = winit::keyboard::ModifiersState::CONTROL.into();
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
        assert!(app.chrome.notice_text().is_none());
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
        app.input.modifiers = winit::keyboard::ModifiersState::ALT.into();

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
        app.input.keys.prefix = crate::model::input::PendingPrefix::Count(3);

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.input.keys.prefix, crate::model::input::PendingPrefix::None);
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
        app.input.modifiers = winit::keyboard::ModifiersState::ALT.into();
        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));
        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
        assert!(app.chrome.notice_text().is_none());
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
            app.chrome.rename_input().map(|input| input.field.text()),
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
            app.chrome.rename_input().map(|input| input.field.text()),
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
            app.input.keys.forwarded.get(&KeyCode::Char('a')),
            Some(&pressed_in)
        );
        let other = app.alloc_tab_id();
        app.layout_mut().add_tab(other);
        app.route_key(ElementState::Released, a, logical, physical);
        assert!(
            app.input.keys.forwarded.is_empty(),
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
            winter_core::MAX_SCROLLBACK,
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
        app.input.modifiers = winit::keyboard::ModifiersState::ALT.into();
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
            winit_key_to_code(&Key::Character("\x18".into()), &placeholder),
            KeyCode::Char('x')
        );
        assert_eq!(
            winit_key_to_code(&Key::Character("\x11".into()), &placeholder),
            KeyCode::Char('q')
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

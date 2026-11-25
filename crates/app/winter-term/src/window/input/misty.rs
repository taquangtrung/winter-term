//! Misty's keys, as Winter's handlers take them: a key as Winter's own
//! [`input::Key`]. Winter's key goes back to misty's with
//! [`winter_field::misty_key`].

use misty::{AppContext, Key, KeyCode, Mods};

use winter_model::input;

use crate::window::App;

// ========================================================================
// App: keys from misty
// ========================================================================

impl App {
    /// Route a key misty reports pressed (`pressed`) or released, quitting
    /// when the key asked to. A key held down after opening a sequence
    /// auto-repeats; the repeats are the same press, not the next chord of
    /// the sequence.
    pub(in crate::window) fn on_key(
        &mut self,
        key: Key,
        pressed: bool,
        cx: &mut AppContext<'_, crate::window::WindowMessage>,
    ) {
        let repeat = self
            .window
            .as_ref()
            .is_some_and(|window| cx.event_cx(window.id()).is_key_repeat());
        if pressed && repeat && self.input.is_sequence_pending() {
            return;
        }
        // Windows-only: drop a key that raced ahead of the window's own
        // focus-gain notification (see `is_pre_focus_key_leak`).
        #[cfg(target_os = "windows")]
        if super::keys::is_pre_focus_key_leak(pressed, self.input.focus.window) {
            return;
        }
        let Some(key) = winter_key(key) else {
            return;
        };
        if self.route_key(pressed, key) {
            self.quit(cx);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Winter's key for misty's `key`; `None` for a key code Winter has no
/// name for.
pub(crate) fn winter_key(key: Key) -> Option<input::Key> {
    let code = match key.code {
        KeyCode::Backspace => input::KeyCode::Backspace,
        KeyCode::Char(' ') => input::KeyCode::Space,
        KeyCode::Char(c) => input::KeyCode::Char(c),
        KeyCode::Delete => input::KeyCode::Delete,
        KeyCode::Down => input::KeyCode::Down,
        KeyCode::End => input::KeyCode::End,
        KeyCode::Enter => input::KeyCode::Enter,
        KeyCode::Esc => input::KeyCode::Escape,
        KeyCode::F(n) => input::KeyCode::F(n),
        KeyCode::Home => input::KeyCode::Home,
        KeyCode::Insert => input::KeyCode::Insert,
        KeyCode::Left => input::KeyCode::Left,
        KeyCode::PageDown => input::KeyCode::PageDown,
        KeyCode::PageUp => input::KeyCode::PageUp,
        KeyCode::Right => input::KeyCode::Right,
        KeyCode::Tab => input::KeyCode::Tab,
        KeyCode::Up => input::KeyCode::Up,
        // A key code misty adds later has no Winter name yet.
        _ => return None,
    };
    Some(
        input::Key {
            alt: key.mods.contains(Mods::ALT),
            code,
            ctrl: key.mods.contains(Mods::CTRL),
            shift: key.mods.contains(Mods::SHIFT),
            sup: key.mods.contains(Mods::SUPER),
        }
        .canonical(),
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_space_and_escape_become_winters_own_names_with_their_modifiers() {
        let space = winter_key(Key::new(KeyCode::Char(' '), Mods::CTRL)).expect("a key");
        assert_eq!((space.code, space.ctrl), (input::KeyCode::Space, true));
        let escape = winter_key(Key::new(KeyCode::Esc, Mods::NONE)).expect("a key");
        assert_eq!(escape.code, input::KeyCode::Escape);
        let meta_x = winter_key(Key::new(KeyCode::Char('x'), Mods::ALT)).expect("a key");
        assert_eq!((meta_x.code, meta_x.alt), (input::KeyCode::Char('x'), true));
    }
}

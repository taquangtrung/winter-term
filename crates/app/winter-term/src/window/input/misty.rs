//! Misty's input, as Winter's handlers take it: a key as Winter's own
//! [`input::Key`], and the pointer's buttons, place, wheel, and the
//! modifiers held as the winit values the pointer code was written for.

use misty::{AppContext, ButtonInput, Key, KeyCode, Mods, Point, PointerButton, WheelDelta};
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, Modifiers, MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;

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
        cx: &mut AppContext<'_, ()>,
    ) {
        let repeat = self
            .window
            .as_ref()
            .is_some_and(|window| cx.event_cx(window.id()).is_key_repeat());
        if pressed && repeat && self.input.is_sequence_pending() {
            return;
        }
        let state = match pressed {
            true => ElementState::Pressed,
            false => ElementState::Released,
        };
        // Windows-only: drop a key that raced ahead of the window's own
        // focus-gain notification (see `is_pre_focus_key_leak`).
        #[cfg(target_os = "windows")]
        if super::keys::is_pre_focus_key_leak(state, self.input.focus.window) {
            return;
        }
        if self.route_key(state, winter_key(key)) {
            self.quit(cx);
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Winter's key for misty's `key`.
pub(in crate::window) fn winter_key(key: Key) -> input::Key {
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
    };
    input::Key {
        alt: key.mods.contains(Mods::ALT),
        code,
        ctrl: key.mods.contains(Mods::CTRL),
        shift: key.mods.contains(Mods::SHIFT),
        sup: key.mods.contains(Mods::SUPER),
    }
    .canonical()
}

/// Misty's key for Winter's `key`.
pub(in crate::window) fn misty_key(key: &input::Key) -> Key {
    let code = match key.code {
        input::KeyCode::Backspace => KeyCode::Backspace,
        input::KeyCode::Char(c) => KeyCode::Char(c),
        input::KeyCode::Delete => KeyCode::Delete,
        input::KeyCode::Down => KeyCode::Down,
        input::KeyCode::End => KeyCode::End,
        input::KeyCode::Enter => KeyCode::Enter,
        input::KeyCode::Escape => KeyCode::Esc,
        input::KeyCode::F(n) => KeyCode::F(n),
        input::KeyCode::Home => KeyCode::Home,
        input::KeyCode::Insert => KeyCode::Insert,
        input::KeyCode::Left => KeyCode::Left,
        input::KeyCode::PageDown => KeyCode::PageDown,
        input::KeyCode::PageUp => KeyCode::PageUp,
        input::KeyCode::Right => KeyCode::Right,
        input::KeyCode::Space => KeyCode::Char(' '),
        input::KeyCode::Tab => KeyCode::Tab,
        input::KeyCode::Up => KeyCode::Up,
    };
    let mut mods = Mods::NONE;
    for (held, flag) in [
        (key.alt, Mods::ALT),
        (key.ctrl, Mods::CTRL),
        (key.shift, Mods::SHIFT),
        (key.sup, Mods::SUPER),
    ] {
        if held {
            mods |= flag;
        }
    }
    Key::new(code, mods)
}

/// The winit modifiers for misty's `mods`.
pub(in crate::window) fn winit_modifiers(mods: Mods) -> Modifiers {
    let mut state = ModifiersState::empty();
    for (held, flag) in [
        (Mods::ALT, ModifiersState::ALT),
        (Mods::CTRL, ModifiersState::CONTROL),
        (Mods::SHIFT, ModifiersState::SHIFT),
        (Mods::SUPER, ModifiersState::SUPER),
    ] {
        if mods.contains(held) {
            state |= flag;
        }
    }
    Modifiers::from(state)
}

/// The winit press or release and button of misty's `button`.
pub(in crate::window) fn winit_button(button: &ButtonInput) -> (ElementState, MouseButton) {
    let state = match button.pressed {
        true => ElementState::Pressed,
        false => ElementState::Released,
    };
    let button = match button.button {
        PointerButton::Back => MouseButton::Back,
        PointerButton::Forward => MouseButton::Forward,
        PointerButton::Left => MouseButton::Left,
        PointerButton::Middle => MouseButton::Middle,
        PointerButton::Other(n) => MouseButton::Other(n),
        PointerButton::Right => MouseButton::Right,
    };
    (state, button)
}

/// The winit position of misty's `point`, both in physical pixels.
pub(in crate::window) fn winit_position(point: Point) -> PhysicalPosition<f64> {
    PhysicalPosition::new(f64::from(point.x), f64::from(point.y))
}

/// The winit wheel movement of misty's `delta`.
pub(in crate::window) fn winit_wheel(delta: WheelDelta) -> MouseScrollDelta {
    match delta {
        WheelDelta::Lines(lines) => MouseScrollDelta::LineDelta(lines.x, lines.y),
        WheelDelta::Pixels(pixels) => MouseScrollDelta::PixelDelta(PhysicalPosition::new(
            f64::from(pixels.x),
            f64::from(pixels.y),
        )),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_space_and_escape_become_winters_own_names_with_their_modifiers() {
        let space = winter_key(Key::new(KeyCode::Char(' '), Mods::CTRL));
        assert_eq!((space.code, space.ctrl), (input::KeyCode::Space, true));
        let escape = winter_key(Key::new(KeyCode::Esc, Mods::NONE));
        assert_eq!(escape.code, input::KeyCode::Escape);
        let meta_x = winter_key(Key::new(KeyCode::Char('x'), Mods::ALT));
        assert_eq!((meta_x.code, meta_x.alt), (input::KeyCode::Char('x'), true));
    }

    #[test]
    fn test_modifiers_held_carry_over_one_for_one() {
        let held = winit_modifiers(Mods::CTRL | Mods::SHIFT).state();
        assert!(held.control_key() && held.shift_key());
        assert!(!held.alt_key() && !held.super_key());
    }
}

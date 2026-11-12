//! The text editor's keys: Winter's key values as Viento's, which owns
//! every editing key; and the key tables the Keys page lists for it.

use winter_model::input::table::Listed;
use winter_model::input::{Key, KeyCode};

use viento_keys::{Key as VientoKey, KeyCode as VientoCode, Mods};

// ========================================================================
// Constants
// ========================================================================

/// Every key table of the text editor, for the Keys page to list: none of
/// Winter's own, as Viento binds every editing key.
pub const KEY_TABLES: &[&dyn Listed] = &[];

// ========================================================================
// Functions
// ========================================================================

/// `key` as Viento names it.
pub(crate) fn to_viento(key: &Key) -> VientoKey {
    let code = match key.code {
        KeyCode::Backspace => VientoCode::Backspace,
        KeyCode::Char(c) => VientoCode::Char(c),
        KeyCode::Delete => VientoCode::Delete,
        KeyCode::Down => VientoCode::Down,
        KeyCode::End => VientoCode::End,
        KeyCode::Enter => VientoCode::Enter,
        KeyCode::Escape => VientoCode::Esc,
        KeyCode::F(number) => VientoCode::F(number),
        KeyCode::Home => VientoCode::Home,
        KeyCode::Insert => VientoCode::Insert,
        KeyCode::Left => VientoCode::Left,
        KeyCode::PageDown => VientoCode::PageDown,
        KeyCode::PageUp => VientoCode::PageUp,
        KeyCode::Right => VientoCode::Right,
        KeyCode::Space => VientoCode::Char(' '),
        KeyCode::Tab => VientoCode::Tab,
        KeyCode::Up => VientoCode::Up,
    };
    let held = [
        (key.alt, Mods::ALT),
        (key.ctrl, Mods::CTRL),
        (key.shift, Mods::SHIFT),
        (key.sup, Mods::SUPER),
    ];
    let mods = held
        .into_iter()
        .filter(|(down, _)| *down)
        .fold(Mods::NONE, |mods, (_, held)| mods | held);
    VientoKey::new(code, mods)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keys_keep_their_modifiers_and_space_and_escape_take_vientos_names() {
        let key = |code, ctrl, alt| Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        };
        let parsed = |spec| viento_keys::parse_key(spec).expect("a valid key");
        assert_eq!(
            to_viento(&key(KeyCode::Char('x'), true, true)),
            parsed("C-M-x")
        );
        assert_eq!(
            to_viento(&key(KeyCode::Space, true, false)),
            parsed("C-SPC")
        );
        assert_eq!(
            to_viento(&key(KeyCode::Escape, false, false)),
            parsed("<esc>")
        );
    }
}

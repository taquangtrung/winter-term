//! Conversions between misty's keys and `cherry-keys`' keys, for an app
//! that resolves its keymap with Cherry. Both normalize a chord the same
//! way, so a key converts both ways without loss.

use misty::key::{Key, KeyCode, Mods};

// ========================================================================
// Constants
// ========================================================================

/// Each misty modifier beside the Cherry modifier it is.
const MODS: [(Mods, cherry::keys::Mods); 4] = [
    (Mods::ALT, cherry::keys::Mods::ALT),
    (Mods::CTRL, cherry::keys::Mods::CTRL),
    (Mods::SHIFT, cherry::keys::Mods::SHIFT),
    (Mods::SUPER, cherry::keys::Mods::SUPER),
];

// ========================================================================
// Functions
// ========================================================================

/// `key`, a misty key, as the Cherry key it is.
pub fn to_cherry(key: Key) -> cherry::keys::Key {
    cherry::keys::Key::new(code_to_cherry(key.code), mods_to_cherry(key.mods))
}

/// `key`, a Cherry key, as the misty key it is.
pub fn from_cherry(key: cherry::keys::Key) -> Key {
    Key::new(code_from_cherry(key.code), mods_from_cherry(key.mods))
}

/// `mods`, misty modifiers, as the Cherry modifiers they are.
pub fn mods_to_cherry(mods: Mods) -> cherry::keys::Mods {
    MODS.iter()
        .filter(|(ours, _)| mods.contains(*ours))
        .fold(cherry::keys::Mods::NONE, |held, (_, theirs)| held | *theirs)
}

/// `mods`, Cherry modifiers, as the misty modifiers they are.
fn mods_from_cherry(mods: cherry::keys::Mods) -> Mods {
    MODS.iter()
        .filter(|(_, theirs)| mods.contains(*theirs))
        .fold(Mods::NONE, |held, (ours, _)| held | *ours)
}

/// `code`, a misty key code, as the Cherry key code it is.
fn code_to_cherry(code: KeyCode) -> cherry::keys::KeyCode {
    match code {
        KeyCode::Backspace => cherry::keys::KeyCode::Backspace,
        KeyCode::Char(c) => cherry::keys::KeyCode::Char(c),
        KeyCode::Delete => cherry::keys::KeyCode::Delete,
        KeyCode::Down => cherry::keys::KeyCode::Down,
        KeyCode::End => cherry::keys::KeyCode::End,
        KeyCode::Enter => cherry::keys::KeyCode::Enter,
        KeyCode::Esc => cherry::keys::KeyCode::Esc,
        KeyCode::F(n) => cherry::keys::KeyCode::F(n),
        KeyCode::Home => cherry::keys::KeyCode::Home,
        KeyCode::Insert => cherry::keys::KeyCode::Insert,
        KeyCode::Left => cherry::keys::KeyCode::Left,
        KeyCode::PageDown => cherry::keys::KeyCode::PageDown,
        KeyCode::PageUp => cherry::keys::KeyCode::PageUp,
        KeyCode::Right => cherry::keys::KeyCode::Right,
        KeyCode::Tab => cherry::keys::KeyCode::Tab,
        KeyCode::Up => cherry::keys::KeyCode::Up,
    }
}

/// `code`, a Cherry key code, as the misty key code it is.
fn code_from_cherry(code: cherry::keys::KeyCode) -> KeyCode {
    match code {
        cherry::keys::KeyCode::Backspace => KeyCode::Backspace,
        cherry::keys::KeyCode::Char(c) => KeyCode::Char(c),
        cherry::keys::KeyCode::Delete => KeyCode::Delete,
        cherry::keys::KeyCode::Down => KeyCode::Down,
        cherry::keys::KeyCode::End => KeyCode::End,
        cherry::keys::KeyCode::Enter => KeyCode::Enter,
        cherry::keys::KeyCode::Esc => KeyCode::Esc,
        cherry::keys::KeyCode::F(n) => KeyCode::F(n),
        cherry::keys::KeyCode::Home => KeyCode::Home,
        cherry::keys::KeyCode::Insert => KeyCode::Insert,
        cherry::keys::KeyCode::Left => KeyCode::Left,
        cherry::keys::KeyCode::PageDown => KeyCode::PageDown,
        cherry::keys::KeyCode::PageUp => KeyCode::PageUp,
        cherry::keys::KeyCode::Right => KeyCode::Right,
        cherry::keys::KeyCode::Tab => KeyCode::Tab,
        cherry::keys::KeyCode::Up => KeyCode::Up,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::key::parse_sequence;

    use super::*;

    #[test]
    fn test_keys_convert_both_ways_without_loss() {
        let keys = parse_sequence("a G ? C-S-f M-? S-<tab> C-M-S-s-<f24> <esc> <space>")
            .expect("valid keys");
        for key in keys {
            let theirs = to_cherry(key);
            assert_eq!(theirs.to_string(), key.to_string());
            assert_eq!(from_cherry(theirs), key);
        }
    }
}

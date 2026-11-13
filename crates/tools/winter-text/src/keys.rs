//! The text editor's keys: Winter's key values as Cherry's, which owns
//! every editing key; and the key tables the Keys page lists for it, of
//! Winter's own keys over the text (the inline suggestions').

use winter_model::input::table::{Binding, KeyTable, Listed};
use winter_model::input::{Key, KeyCode};

use cherry_keys::{Key as CherryKey, KeyCode as CherryCode, Mods};

// ========================================================================
// Constants
// ========================================================================

/// Every key table of the text editor, for the Keys page to list; Cherry
/// binds every editing key.
pub const KEY_TABLES: &[&dyn Listed] = &[&INLINE_KEYS];

/// The keys over an inline suggestion, ahead of Cherry's: ghost text's
/// while typing, a next edit's while its card shows.
pub(crate) const INLINE_KEYS: KeyTable<InlineAct> = KeyTable {
    block: "inline",
    bindings: &[
        Binding {
            action: InlineAct::Ask,
            id: "ask",
            keys: &["C-\\"],
            label: "Ask for Ghost Text",
        },
        Binding {
            action: InlineAct::TakeAll,
            id: "take_all",
            keys: &["TAB", "C-RET"],
            label: "Take the Suggestion",
        },
        Binding {
            action: InlineAct::TakeWord,
            id: "take_word",
            keys: &["M-f", "M-<right>", "C-,"],
            label: "Take the Ghost Text's Next Word",
        },
        Binding {
            action: InlineAct::TakeLine,
            id: "take_line",
            keys: &["C-."],
            label: "Take the Ghost Text's Line",
        },
        Binding {
            action: InlineAct::Next,
            id: "next",
            keys: &["M-n"],
            label: "Next Ghost Text",
        },
        Binding {
            action: InlineAct::Previous,
            id: "previous",
            keys: &["M-p"],
            label: "Previous Ghost Text",
        },
        Binding {
            action: InlineAct::PutAway,
            id: "put_away",
            keys: &["C-g"],
            label: "Put the Suggestion Away",
        },
        Binding {
            action: InlineAct::Leave,
            id: "leave",
            keys: &["ESC"],
            label: "Put the Suggestion Away, then Escape",
        },
    ],
    configurable: true,
    title: "Text: Inline Suggestions",
};

// ========================================================================
// Data Structures
// ========================================================================

/// What a key does to the inline suggestion on screen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum InlineAct {
    /// Ask for ghost text where none came by itself.
    Ask,
    /// Put the suggestion away, the key going on to Cherry.
    Leave,
    /// Show the next ghost text suggestion.
    Next,
    /// Show the ghost text suggestion before.
    Previous,
    /// Put the suggestion away.
    PutAway,
    /// Take all of the suggestion.
    TakeAll,
    /// Take the rest of the ghost text's line.
    TakeLine,
    /// Take the ghost text's next word.
    TakeWord,
}

// ========================================================================
// Functions
// ========================================================================

/// `key` as Cherry names it.
pub(crate) fn to_cherry(key: &Key) -> CherryKey {
    let code = match key.code {
        KeyCode::Backspace => CherryCode::Backspace,
        KeyCode::Char(c) => CherryCode::Char(c),
        KeyCode::Delete => CherryCode::Delete,
        KeyCode::Down => CherryCode::Down,
        KeyCode::End => CherryCode::End,
        KeyCode::Enter => CherryCode::Enter,
        KeyCode::Escape => CherryCode::Esc,
        KeyCode::F(number) => CherryCode::F(number),
        KeyCode::Home => CherryCode::Home,
        KeyCode::Insert => CherryCode::Insert,
        KeyCode::Left => CherryCode::Left,
        KeyCode::PageDown => CherryCode::PageDown,
        KeyCode::PageUp => CherryCode::PageUp,
        KeyCode::Right => CherryCode::Right,
        KeyCode::Space => CherryCode::Char(' '),
        KeyCode::Tab => CherryCode::Tab,
        KeyCode::Up => CherryCode::Up,
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
    CherryKey::new(code, mods)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_keys_keep_their_modifiers_and_space_and_escape_take_cherry_s_names() {
        let key = |code, ctrl, alt| Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        };
        let parsed = |spec| cherry_keys::parse_key(spec).expect("a valid key");
        assert_eq!(
            to_cherry(&key(KeyCode::Char('x'), true, true)),
            parsed("C-M-x")
        );
        assert_eq!(
            to_cherry(&key(KeyCode::Space, true, false)),
            parsed("C-SPC")
        );
        assert_eq!(
            to_cherry(&key(KeyCode::Escape, false, false)),
            parsed("<esc>")
        );
    }
}

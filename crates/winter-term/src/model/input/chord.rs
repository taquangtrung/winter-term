//! Chords as `keybindings.kdl` writes them: parsing a spec into keys,
//! formatting keys back, and matching a pressed key against a bound one.

use super::sequence::MAX_SEQUENCE_LEN;
use super::{Key, KeyCode};

// ========================================================================
// Constants
// ========================================================================

/// The highest function key a chord can name.
const MAX_FUNCTION_KEY: u8 = 24;

// ========================================================================
// Formatting
// ========================================================================

/// Format a [`Key`] in Emacs notation (e.g. `"C-M-S-t"`): `C-` for Ctrl, `M-`
/// for Alt, `S-` for Shift, `s-` for Super, in that order, then the key as Emacs names it
/// (`RET`, `TAB`, `SPC`, `ESC`, `DEL`, `<up>`, `<f5>`). A letter is always the
/// lowercase letter, so Shift-s reads `S-s` and never a bare `S`.
pub fn format_key(key: &Key) -> String {
    let key = key.canonical();
    let mut s = String::new();
    if key.ctrl {
        s.push_str("C-");
    }
    if key.alt {
        s.push_str("M-");
    }
    // A shifted symbol carries its Shift in the glyph: `M-?`, not `M-S-?`.
    let glyph_is_shifted = matches!(key.code, KeyCode::Char(c) if is_shifted_symbol(c));
    if key.shift && !glyph_is_shifted {
        s.push_str("S-");
    }
    if key.sup {
        s.push_str("s-");
    }
    match key.code {
        KeyCode::Char(c) => s.push(c),
        KeyCode::Backspace => s.push_str("DEL"),
        KeyCode::Delete => s.push_str("<delete>"),
        KeyCode::Down => s.push_str("<down>"),
        KeyCode::End => s.push_str("<end>"),
        KeyCode::Enter => s.push_str("RET"),
        KeyCode::Escape => s.push_str("ESC"),
        KeyCode::F(n) => {
            s.push_str("<f");
            s.push_str(&n.to_string());
            s.push('>');
        }
        KeyCode::Home => s.push_str("<home>"),
        KeyCode::Insert => s.push_str("<insert>"),
        KeyCode::Left => s.push_str("<left>"),
        KeyCode::PageDown => s.push_str("<next>"),
        KeyCode::PageUp => s.push_str("<prior>"),
        KeyCode::Right => s.push_str("<right>"),
        KeyCode::Space => s.push_str("SPC"),
        KeyCode::Tab => s.push_str("TAB"),
        KeyCode::Up => s.push_str("<up>"),
    }
    s
}
/// Format a sequence of chords the way a binding spec writes it, each in
/// [`format_key`]'s notation and separated by spaces (`"M-g M-h"`).
pub fn format_sequence(keys: &[Key]) -> String {
    keys.iter().map(format_key).collect::<Vec<_>>().join(" ")
}
/// Parse a key-binding spec of one or more chords (e.g. `"C-h"` or
/// `"M-g M-h"`) into its keys. Returns `None` on any unrecognized chord or a
/// length outside `1..=MAX_SEQUENCE_LEN`.
pub(super) fn parse_chord_sequence(spec: &str) -> Option<Vec<Key>> {
    let keys = spec
        .split_whitespace()
        .map(parse_chord)
        .collect::<Option<Vec<Key>>>()?;
    if (1..=MAX_SEQUENCE_LEN).contains(&keys.len()) {
        Some(keys)
    } else {
        None
    }
}
/// Look up `key` in a chord table, retrying with the unshifted glyph if the
/// first lookup misses. winit reports `logical_key`, which applies the Shift
/// character transformation (`S-\` -> `'|'`, `S--` -> `'_'`,
/// `S-o` -> `'O'`, ...); binding specs use the physical key label, so a
/// held-Shift chord that misses on the transformed glyph retries on the
/// untransformed one.
pub(super) fn find_chord<T: Clone>(entries: &[(Key, T)], key: &Key) -> Option<T> {
    let find = |key: &Key| {
        entries
            .iter()
            .find(|(chord, _)| chord == key)
            .map(|(_, action)| action.clone())
    };
    find(key).or_else(|| find(&unshifted(key)?))
}
/// Whether the pressed key is the bound chord, as typed or else unshifted:
/// the one-chord form of [`find_chord`]'s matching, for a sequence's chords.
pub(super) fn is_chord_match(bound: &Key, pressed: &Key) -> bool {
    bound == pressed || unshifted(pressed).as_ref() == Some(bound)
}
/// `key` with its shifted glyph turned back into the physical key it was
/// typed on (`'|'` to `'\\'`), Shift still held; `None` when Shift is not
/// held or the glyph is no shifted one.
fn unshifted(key: &Key) -> Option<Key> {
    let KeyCode::Char(c) = key.glyph() else {
        return None;
    };
    let base = unshift_char(c).filter(|_| key.shift)?;
    Some(Key {
        code: KeyCode::Char(base),
        ..*key
    })
}
/// Whether `c` is a symbol typed with Shift (`?`, `+`, `(`), so its Shift is
/// in the glyph. Capital letters are not: they fold to lowercase plus Shift.
pub(super) fn is_shifted_symbol(c: char) -> bool {
    !c.is_ascii_uppercase() && unshift_char(c).is_some()
}
/// Map a Shift-modified character back to its physical key for US QWERTY,
/// so binding specs like `"S+M+\\"` match the winit `logical_key` `'|'`.
pub(super) fn unshift_char(c: char) -> Option<char> {
    Some(match c {
        'A'..='Z' => c.to_ascii_lowercase(),
        '!' => '1',
        '@' => '2',
        '#' => '3',
        '$' => '4',
        '%' => '5',
        '^' => '6',
        '&' => '7',
        '*' => '8',
        '(' => '9',
        ')' => '0',
        '_' => '-',
        '+' => '=',
        '{' => '[',
        '}' => ']',
        '|' => '\\',
        ':' => ';',
        '"' => '\'',
        '<' => ',',
        '>' => '.',
        '?' => '/',
        '~' => '`',
        _ => return None,
    })
}
/// Parse a single chord into a [`Key`], in Emacs notation (`"C-M-S-n"`,
/// `"C-RET"`, `"M-<left>"`, `"C-x"`): `C-` Ctrl, `M-` Alt, `S-` Shift, `s-` Super, in any
/// order, then the key. The key is a single character, an Emacs name (`RET`,
/// `TAB`, `SPC`, `ESC`, `DEL` for Backspace, `<up>`, `<next>`, `<f5>`), or
/// one of the plain names (`Space`, `Enter`, `PageDown`, ...). `s-` is Super.
///
/// A letter is kept as the lowercase letter with Shift held, however it is
/// spelled.
pub(super) fn parse_chord(token: &str) -> Option<Key> {
    parse_emacs_chord(token).map(|key| key.canonical())
}
/// The Emacs spelling of a chord: see [`parse_chord`].
fn parse_emacs_chord(token: &str) -> Option<Key> {
    let mut key = Key {
        alt: false,
        code: KeyCode::Space,
        ctrl: false,
        shift: false,
        sup: false,
    };
    let mut rest = token;
    loop {
        let mut chars = rest.chars();
        match (chars.next(), chars.next(), chars.as_str()) {
            (Some(modifier), Some('-'), tail) if !tail.is_empty() => {
                match modifier {
                    'C' => key.ctrl = true,
                    'M' => key.alt = true,
                    'S' => key.shift = true,
                    's' => key.sup = true,
                    _ => return None,
                }
                rest = tail;
            }
            _ => break,
        }
    }
    key.code = parse_key_code(
        rest.strip_prefix('<')
            .and_then(|name| name.strip_suffix('>'))
            .unwrap_or(rest),
    )?;
    Some(key)
}
/// Parse a key name into a [`KeyCode`]: a single character is a `Char`, and
/// named keys (`Space`, `Enter`, `F1`, ...) map case-insensitively.
pub(super) fn parse_key_code(name: &str) -> Option<KeyCode> {
    // Emacs' `DEL` is the Backspace key; `Del` and `Delete` are Delete.
    if name == "DEL" {
        return Some(KeyCode::Backspace);
    }
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(KeyCode::Char(first));
    }
    Some(match name.to_ascii_lowercase().as_str() {
        "backspace" => KeyCode::Backspace,
        "delete" | "del" => KeyCode::Delete,
        "down" => KeyCode::Down,
        "end" => KeyCode::End,
        "enter" | "ret" | "return" => KeyCode::Enter,
        "escape" | "esc" => KeyCode::Escape,
        "home" => KeyCode::Home,
        "insert" => KeyCode::Insert,
        "left" => KeyCode::Left,
        "next" | "pagedown" => KeyCode::PageDown,
        "pageup" | "prior" => KeyCode::PageUp,
        "right" => KeyCode::Right,
        "space" | "spc" => KeyCode::Space,
        "tab" => KeyCode::Tab,
        "up" => KeyCode::Up,
        other => return parse_function_key(other),
    })
}
/// Parse an `f1`..`f24` function-key name into [`KeyCode::F`].
pub(super) fn parse_function_key(name: &str) -> Option<KeyCode> {
    let number: u8 = name.strip_prefix('f')?.parse().ok()?;
    (1..=MAX_FUNCTION_KEY)
        .contains(&number)
        .then_some(KeyCode::F(number))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::super::{Keymap, WindowAction};
    use super::*;

    #[test]
    fn test_parse_chord_modifiers_and_named_keys() {
        assert_eq!(
            parse_chord("C-S-SPC"),
            Some(Key {
                alt: false,
                code: KeyCode::Space,
                ctrl: true,
                shift: true,
                sup: false,
            })
        );
        assert_eq!(parse_chord("<f5>").map(|k| k.code), Some(KeyCode::F(5)));
        assert_eq!(parse_chord("v").map(|k| k.code), Some(KeyCode::Char('v')));
        assert_eq!(parse_chord("M-S--").map(|k| (k.code, k.alt)), Some((KeyCode::Char('-'), true)));
        assert_eq!(parse_chord("C-+").map(|k| k.code), Some(KeyCode::Char('+')));
        // The retired `+` spelling is no chord.
        assert_eq!(parse_chord("C+S+Space"), None);
    }
    #[test]
    fn test_parse_chord_sequence_length_bounds() {
        assert_eq!(parse_chord_sequence("C-w v").map(|k| k.len()), Some(2));
        assert_eq!(parse_chord_sequence("C-h").map(|k| k.len()), Some(1));
        assert_eq!(parse_chord_sequence(""), None);
        assert_eq!(parse_chord_sequence("a b c d").map(|k| k.len()), Some(4));
        assert_eq!(parse_chord_sequence("a b c d e"), None);
    }
    #[test]
    fn test_every_displayed_chord_parses_back_to_the_same_key() {
        // What the app shows is what a config can say: the two notations are
        // one, for letters, symbols, and every named key.
        let codes = [
            KeyCode::Backspace,
            KeyCode::Char('-'),
            KeyCode::Char('+'),
            KeyCode::Char('n'),
            KeyCode::Delete,
            KeyCode::Down,
            KeyCode::Enter,
            KeyCode::Escape,
            KeyCode::Char('?'),
            KeyCode::F(5),
            KeyCode::F(24),
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Space,
            KeyCode::Tab,
        ];
        for code in codes {
            for bits in 0..16u8 {
                let key = Key {
                    alt: bits & 1 != 0,
                    code,
                    ctrl: bits & 2 != 0,
                    shift: bits & 4 != 0,
                    sup: bits & 8 != 0,
                };
                let shown = format_key(&key);
                assert_eq!(parse_chord(&shown), Some(key.canonical()), "{shown}");
            }
        }
    }

    #[test]
    fn test_emacs_chords_name_keys_and_modifiers_one_way() {
        assert_eq!(parse_chord("DEL").map(|k| k.code), Some(KeyCode::Backspace));
        assert_eq!(parse_chord("Del").map(|k| k.code), Some(KeyCode::Delete));
        // Shift on a letter is the flag, however it is written.
        assert_eq!(parse_chord("C-S-R"), parse_chord("C-S-r"));
        // A shifted symbol has its Shift in the glyph, so `M-?` is what
        // Alt-Shift-/ sends, and `C-+` is Ctrl-Shift-=.
        let alt_question = Key {
            shift: true,
            ..Key::with_alt(KeyCode::Char('?'))
        };
        assert_eq!(parse_chord("M-?"), Some(alt_question));
        assert_eq!(format_key(&alt_question), "M-?");
        let mut bindings = HashMap::new();
        bindings.insert("M-?".to_string(), "zoom_pane".to_string());
        let keymap = Keymap::from_config(Some(&bindings), None);
        assert_eq!(
            keymap.direct_action(&alt_question, false),
            Some(WindowAction::Zoom.to_action())
        );
        let super_x = parse_chord("s-x").expect("s- is Super");
        assert!(super_x.sup && !super_x.ctrl && !super_x.alt && !super_x.shift);
        assert_eq!(
            parse_chord("C-M-S-s-x").map(|k| format_key(&k)),
            Some("C-M-S-s-x".into())
        );
        assert_ne!(Some(super_x), parse_chord("M-x"), "Super is not Alt");
        assert_eq!(parse_chord("C-"), None);
    }

    #[test]
    fn test_format_key_writes_emacs_notation_in_c_m_s_order() {
        let chord = Key {
            alt: true,
            code: KeyCode::Char('n'),
            ctrl: true,
            shift: true,
            sup: false,
        };
        assert_eq!(format_key(&chord), "C-M-S-n");
        // A capital is the lowercase letter with Shift, never a bare `S`.
        assert_eq!(format_key(&Key::plain(KeyCode::Char('S'))), "S-s");
        assert_eq!(format_key(&Key::plain(KeyCode::Char('s'))), "s");
        assert_eq!(format_key(&Key::with_ctrl(KeyCode::Enter)), "C-RET");
        assert_eq!(format_key(&Key::with_alt(KeyCode::Backspace)), "M-DEL");
        assert_eq!(format_key(&Key::plain(KeyCode::PageDown)), "<next>");
        assert_eq!(
            format_key(&Key {
                sup: true,
                ..Key::plain(KeyCode::Char('x'))
            }),
            "s-x"
        );
    }
}

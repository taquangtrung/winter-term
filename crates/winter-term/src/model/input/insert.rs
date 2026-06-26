//! Insert mode: encoding key events into the bytes the PTY expects.

use super::{Action, Key, KeyCode, Keymap, Mods};
use crate::model::mode::{Mode, ModeEvent};

// ========================================================================
// Insert mode: key-to-byte encoding
// ========================================================================

pub(super) const CONTROL_MASK: u8 = 0x1f;
pub(super) const DELETE: u8 = 0x7f;
pub(super) const ESCAPE: u8 = 0x1b;
pub(super) const CARRIAGE_RETURN: u8 = b'\r';
/// What Ctrl with Space, `@` or `2` sends: NUL.
const NUL: u8 = 0x00;
/// The control bytes Ctrl gives the symbol keys that have one, as xterm
/// sends them: `C-[` is ESC, `C-\` FS, `C-]` GS, `C-^` RS, `C-_` US, `C-?`
/// DEL, with the digit row standing in for them as on a VT220.
const CONTROL_SYMBOLS: &[(char, u8)] = &[
    (' ', NUL),
    ('2', NUL),
    ('3', ESCAPE),
    ('4', 0x1c),
    ('5', 0x1d),
    ('6', 0x1e),
    ('7', 0x1f),
    ('8', DELETE),
    ('/', 0x1f),
    ('?', DELETE),
    ('@', NUL),
    ('[', ESCAPE),
    ('\\', 0x1c),
    (']', 0x1d),
    ('^', 0x1e),
    ('_', 0x1f),
];
/// The kitty keyboard flag asking for repeat and release events as well as
/// presses.
const KITTY_REPORT_EVENT_TYPES: u32 = 2;
/// What opens every escape sequence a key is sent as.
const CSI: &[u8] = b"\x1b[";
/// Shift+Tab's legacy shape, which kitty keeps when not every key is escaped.
const BACKTAB: &[u8] = b"\x1b[Z";
/// The kitty keyboard flag asking for every key as an escape code, the
/// ones that type text included (`0b1000`).
const KITTY_REPORT_ALL_KEYS: u32 = 8;
/// The kitty protocol's private-use codepoint for F13; F14.. follow it.
const KITTY_F13: u32 = 57376;
/// xterm's `CSI n ~` numbers for F13 through F20, in order.
const XTERM_F13_TO_F20: [u8; 8] = [25, 26, 28, 29, 31, 32, 33, 34];
pub(super) fn resolve_insert(
    key: &Key,
    flags: u32,
    modify_other_keys: Option<i64>,
    window: &Keymap,
    is_alt_screen: bool,
) -> Action {
    // Window direct chords intercept in every mode so pane management works
    // regardless of what the terminal is doing. Scroll-type chords are bypassed
    // when a full-screen TUI app owns the pane.
    if let Some(action) = window.direct_action(key, is_alt_screen) {
        return action;
    }
    if let Some(a) = window.specific_index_action(key) {
        return a;
    }
    if is_entry_chord(key) {
        return Action::SwitchMode(Mode::Insert.apply(ModeEvent::EnterNormal));
    }
    // Configurable line edits and prompt undo/redo (defaults: `C-DEL` →
    // delete word back, `C-/` → undo, `C-\` → redo). Claiming the undo/redo
    // chords also stops them reaching the PTY as raw control bytes 0x1f / 0x1c.
    if let Some(binding) = window.edit_binding(key) {
        return binding.to_action();
    }
    // Escape always goes to the PTY. Context-aware mode switching (e.g. at the
    // shell prompt) is handled one layer up in the application event loop.
    Action::SendBytes(encode(key, flags, modify_other_keys))
}
pub(super) fn is_entry_chord(key: &Key) -> bool {
    key.mods() == Mods::ALT && key.glyph() == KeyCode::Escape
}
/// Encode a key as the bytes a terminal program expects on the PTY.
/// `flags` is the active Kitty keyboard protocol bitmask for the pane
/// (0 = legacy xterm encoding). `modify_other_keys` is the xterm
/// modifyOtherKeys mode (`None` = off, `Some(1)` or `Some(2)`).
pub(super) fn encode(key: &Key, flags: u32, modify_other_keys: Option<i64>) -> Vec<u8> {
    if flags != 0 {
        encode_kitty(key, flags)
    } else {
        encode_xterm(key, modify_other_keys)
    }
}
/// Encode a key-release event for the Kitty protocol: `CSI codepoint ;
/// modifier : 3 u`, the modifier written as `1` when there is none, or the
/// legacy shape with the same `: 3` for the arrows and their kin.
///
/// Nothing is sent unless the program asked for event types (flag 2), and,
/// as the protocol says, not for a key whose press went out as the plain text
/// or legacy byte it types (a letter, Space, Enter, Tab, Backspace) unless it
/// asked for every key as an escape code too (flag 8). A program that never
/// asked to see those releases (pi, asking for 7) reads one as a key of its
/// own, or drops the keystroke it arrives beside.
pub fn encode_release(key: &Key, flags: u32) -> Vec<u8> {
    if flags & KITTY_REPORT_EVENT_TYPES == 0 {
        return Vec::new();
    }
    let press = encode_kitty(key, flags);
    let escaped = press.starts_with(CSI) && press != BACKTAB;
    if !escaped && flags & KITTY_REPORT_ALL_KEYS == 0 {
        return Vec::new();
    }
    encode_kitty_release(key)
}
/// `action` as the cursor keys send it under DECCKM (application cursor
/// keys, `CSI ? 1 h`): an unmodified arrow, Home or End (`CSI A`..`CSI D`,
/// `CSI H`, `CSI F`) goes out in its `SS3` form (`ESC O A`), which is what
/// `kcuu1` and friends in the `xterm-256color` terminfo name once a program
/// enables keypad mode. Modified keys, and every other action, are unchanged,
/// as in xterm. Only the legacy encoding has this mode; the kitty protocol
/// keeps its own forms.
pub fn apply_cursor_key_mode(action: Action, app_cursor_keys: bool, kitty_flags: u32) -> Action {
    match action {
        Action::SendBytes(bytes) if app_cursor_keys && kitty_flags == 0 => {
            Action::SendBytes(match bytes.as_slice() {
                [ESCAPE, b'[', final_byte @ (b'A'..=b'D' | b'F' | b'H')] => ss3(*final_byte),
                _ => bytes,
            })
        }
        other => other,
    }
}
/// xterm modifier byte: 1 + shift + 2*alt + 4*ctrl + 8*super. Value 1 means no
/// modifier.
pub(super) fn xterm_modifier(key: &Key) -> u8 {
    1 + (key.shift as u8) + 2 * (key.alt as u8) + 4 * (key.ctrl as u8) + 8 * (key.sup as u8)
}
/// Prepend an ESC byte (Alt prefix convention).
pub(super) fn esc_prefix(mut bytes: Vec<u8>) -> Vec<u8> {
    bytes.insert(0, ESCAPE);
    bytes
}
/// Navigation key: bare CSI when no modifier, `\e[1;NX` with one.
pub(super) fn nav_csi_xterm(final_byte: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        csi(final_byte)
    } else {
        format!("\x1b[1;{m}{}", final_byte as char).into_bytes()
    }
}
/// Tilde-form key: `\e[k~` bare, `\e[k;N~` with modifier.
pub(super) fn tilde_xterm(param: u8, m: u8) -> Vec<u8> {
    if m == 1 {
        csi_param(param, b'~')
    } else {
        format!("\x1b[{param};{m}~").into_bytes()
    }
}
/// xterm encoding for function keys F1-F12 with optional modifier.
pub(super) fn encode_f_xterm(n: u8, m: u8) -> Vec<u8> {
    match n {
        1 => {
            if m > 1 {
                format!("\x1b[1;{m}P").into_bytes()
            } else {
                ss3(b'P')
            }
        }
        2 => {
            if m > 1 {
                format!("\x1b[1;{m}Q").into_bytes()
            } else {
                ss3(b'Q')
            }
        }
        3 => {
            if m > 1 {
                format!("\x1b[1;{m}R").into_bytes()
            } else {
                ss3(b'R')
            }
        }
        4 => {
            if m > 1 {
                format!("\x1b[1;{m}S").into_bytes()
            } else {
                ss3(b'S')
            }
        }
        5 => tilde_xterm(15, m),
        6 => tilde_xterm(17, m),
        7 => tilde_xterm(18, m),
        8 => tilde_xterm(19, m),
        9 => tilde_xterm(20, m),
        10 => tilde_xterm(21, m),
        11 => tilde_xterm(23, m),
        12 => tilde_xterm(24, m),
        13..=20 => tilde_xterm(XTERM_F13_TO_F20[usize::from(n) - 13], m),
        _ => Vec::new(),
    }
}
pub(super) fn encode_xterm(key: &Key, modify_other_keys: Option<i64>) -> Vec<u8> {
    // Legacy xterm has no way to say Super on a key with no parameter slot, so
    // such a chord sends nothing rather than the bare character.
    let carries_modifier = matches!(
        key.glyph(),
        KeyCode::Delete
            | KeyCode::Down
            | KeyCode::End
            | KeyCode::F(_)
            | KeyCode::Home
            | KeyCode::Insert
            | KeyCode::Left
            | KeyCode::PageDown
            | KeyCode::PageUp
            | KeyCode::Right
            | KeyCode::Up
    );
    if key.sup && modify_other_keys.is_none() && !carries_modifier {
        return Vec::new();
    }
    let m = xterm_modifier(key);
    let bytes = match key.glyph() {
        KeyCode::Backspace => vec![DELETE],
        KeyCode::Char('\0') => return Vec::new(),
        KeyCode::Char(c) => match key.ctrl.then(|| control_byte(c)).flatten() {
            Some(byte) => vec![byte],
            None => c.to_string().into_bytes(),
        },
        KeyCode::Delete => tilde_xterm(3, m),
        KeyCode::Down => nav_csi_xterm(b'B', m),
        KeyCode::End => {
            if m > 1 {
                format!("\x1b[1;{m}F").into_bytes()
            } else {
                csi(b'F')
            }
        }
        KeyCode::Enter => vec![CARRIAGE_RETURN],
        KeyCode::Escape => vec![ESCAPE],
        KeyCode::F(n) => encode_f_xterm(n, m),
        KeyCode::Home => {
            if m > 1 {
                format!("\x1b[1;{m}H").into_bytes()
            } else {
                csi(b'H')
            }
        }
        KeyCode::Insert => tilde_xterm(2, m),
        KeyCode::Left => nav_csi_xterm(b'D', m),
        KeyCode::PageDown => tilde_xterm(6, m),
        KeyCode::PageUp => tilde_xterm(5, m),
        KeyCode::Right => nav_csi_xterm(b'C', m),
        KeyCode::Space => match key.ctrl {
            true => vec![NUL],
            false => vec![b' '],
        },
        KeyCode::Tab => {
            if key.shift {
                vec![ESCAPE, b'[', b'Z'] // backtab / reverse-tab
            } else {
                vec![b'\t']
            }
        }
        KeyCode::Up => nav_csi_xterm(b'A', m),
    };

    // xterm modifyOtherKeys: when an app has enabled it (CSI > 4;N m) and the
    // key has at least one modifier, encode character keys as
    // `\x1b[27;<modifier>;<codepoint>~` instead of the legacy `\x1b<char>`.
    // This is unambiguous: there is no ESC prefix that could be parsed as a
    // standalone Escape, so M-S-e/H/L arrive intact. Mirrors WezTerm.
    //
    // Mode 1 excludes a few well-known keys from the extended encoding (the
    // same set xterm excludes); mode 2 applies it to all modified chars.
    if let Some(mode) = modify_other_keys {
        if let (KeyCode::Char(c), true) =
            (key.glyph(), (key.shift || key.ctrl || key.alt || key.sup))
        {
            let cp = if key.ctrl && c.is_ascii_alphabetic() {
                (c.to_ascii_uppercase() as u32) & 0x1f
            } else {
                c as u32
            };
            let mode1_excluded = mode == 1 && matches!(c, 'c' | 'd' | '\x1b' | '\x7f' | '\x08');
            if !mode1_excluded {
                return format!("\x1b[27;{m};{cp}~").into_bytes();
            }
        }
    }

    // Alt prefix: prepend ESC (never on bare Escape to avoid double-ESC).
    if key.alt && !matches!(key.glyph(), KeyCode::Escape | KeyCode::Char('\0')) {
        esc_prefix(bytes)
    } else {
        bytes
    }
}
/// The control byte Ctrl turns `c` into, if it has one: a letter's C0 code,
/// or one of the [`CONTROL_SYMBOLS`].
fn control_byte(c: char) -> Option<u8> {
    if c.is_ascii_alphabetic() {
        return Some((c.to_ascii_uppercase() as u8) & CONTROL_MASK);
    }
    CONTROL_SYMBOLS
        .iter()
        .find(|(symbol, _)| *symbol == c)
        .map(|(_, byte)| *byte)
}
/// Kitty modifier value: 1 + shift + 2*alt + 4*ctrl + 8*super. Value 1 means no
/// modifier.
pub(super) fn kitty_modifier(key: &Key) -> u32 {
    1 + (key.shift as u32) + 2 * (key.alt as u32) + 4 * (key.ctrl as u32) + 8 * (key.sup as u32)
}
/// `CSI codepoint u` or `CSI codepoint ; modifier u` (omit `;1`).
/// A key the kitty protocol leaves in its legacy shape rather than giving it a
/// codepoint: the arrows, Home and End, Insert, Delete, the tool keys, and the
/// function keys. `number` and `final_byte` are the legacy `CSI number ~` or
/// `CSI 1 final` pair, and `modifier` is 1 for none. A press drops what is
/// default (`CSI A`, `CSI 5 ~`) and carries a modifier as `CSI 1 ; m A`; a
/// release adds the `:3` event type to the modifier field, which is then always
/// written. Encoding these as private-use codepoints, as the other keys are,
/// leaves a program that reads `CSI A` for Up with nothing it recognises.
pub(super) fn kitty_legacy_key(
    number: u32,
    final_byte: u8,
    modifier: u32,
    release: bool,
) -> Vec<u8> {
    let last = final_byte as char;
    match (release, modifier) {
        (false, 1) if final_byte != b'~' => format!("\x1b[{last}").into_bytes(),
        (false, 1) => format!("\x1b[{number}~").into_bytes(),
        (false, _) => format!("\x1b[{number};{modifier}{last}").into_bytes(),
        (true, _) => format!("\x1b[{number};{modifier}:3{last}").into_bytes(),
    }
}
/// The legacy shape of a key kitty does not give a codepoint to, as the
/// `(number, final byte)` pair [`kitty_legacy_key`] writes, or `None` for the
/// keys that do have one.
fn kitty_legacy_shape(code: KeyCode) -> Option<(u32, u8)> {
    Some(match code {
        KeyCode::Insert => (2, b'~'),
        KeyCode::Delete => (3, b'~'),
        KeyCode::Left => (1, b'D'),
        KeyCode::Right => (1, b'C'),
        KeyCode::Up => (1, b'A'),
        KeyCode::Down => (1, b'B'),
        KeyCode::PageUp => (5, b'~'),
        KeyCode::PageDown => (6, b'~'),
        KeyCode::Home => (1, b'H'),
        KeyCode::End => (1, b'F'),
        KeyCode::F(1) => (1, b'P'),
        KeyCode::F(2) => (1, b'Q'),
        KeyCode::F(3) => (13, b'~'),
        KeyCode::F(4) => (1, b'S'),
        KeyCode::F(5) => (15, b'~'),
        KeyCode::F(6) => (17, b'~'),
        KeyCode::F(7) => (18, b'~'),
        KeyCode::F(8) => (19, b'~'),
        KeyCode::F(9) => (20, b'~'),
        KeyCode::F(10) => (21, b'~'),
        KeyCode::F(11) => (23, b'~'),
        KeyCode::F(12) => (24, b'~'),
        _ => return None,
    })
}
pub(super) fn kitty_csi(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}u").into_bytes()
    }
}
/// Release variant: `CSI codepoint ; modifier : 3 u`. The modifier field is
/// written even when it is `1` (none): the event type rides on it, and
/// `CSI 97::3u` would read as an empty alternate key and a base-layout key of
/// `3`, a press.
pub(super) fn kitty_csi_release(codepoint: u32, modifier: u32) -> Vec<u8> {
    format!("\x1b[{codepoint};{modifier}:3u").into_bytes()
}
/// Kitty release encoding: same key mapping as `encode_kitty` but with the
/// `: 3` event-type suffix, always as a full sequence. Which keys get one at
/// all is [`encode_release`]'s to decide.
pub(super) fn encode_kitty_release(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.glyph() {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => kitty_csi_release(base_codepoint(c, key.shift), m),
        KeyCode::Space => kitty_csi_release(32, m),
        KeyCode::Tab => kitty_csi_release(9, m),
        KeyCode::Enter => kitty_csi_release(13, m),
        KeyCode::Escape => kitty_csi_release(27, m),
        KeyCode::Backspace => kitty_csi_release(127, m),
        KeyCode::F(n @ 13..=24) => kitty_csi_release(KITTY_F13 + u32::from(n) - 13, m),
        code => match kitty_legacy_shape(code) {
            Some((number, last)) => kitty_legacy_key(number, last, m, true),
            None => Vec::new(),
        },
    }
}
/// The base Unicode codepoint of a character key (lowercase for ASCII alpha
/// so C-S-a uses codepoint 97, not 65).
pub(super) fn base_codepoint(c: char, shift: bool) -> u32 {
    if shift && c.is_ascii_uppercase() {
        c.to_ascii_lowercase() as u32
    } else {
        c as u32
    }
}
/// Encode a key press under the kitty keyboard protocol, given the `flags`
/// the program pushed (never 0 here). Every flag set disambiguates: Escape and
/// every modified key become `CSI ... u`, while text, Enter, Tab and Backspace
/// keep their legacy bytes. [`KITTY_REPORT_ALL_KEYS`] turns those into escape
/// codes too.
pub(super) fn encode_kitty(key: &Key, flags: u32) -> Vec<u8> {
    let m = kitty_modifier(key);
    let all_keys = flags & KITTY_REPORT_ALL_KEYS != 0;
    // The keys that keep their legacy bytes when unmodified (and, for text,
    // under a lone Shift, which the glyph already carries).
    let legacy = |bytes: Vec<u8>, codepoint: u32, plain: bool| match plain && !all_keys {
        true => bytes,
        false => kitty_csi(codepoint, m),
    };
    match key.glyph() {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => legacy(
            c.to_string().into_bytes(),
            base_codepoint(c, key.shift),
            m == 1 || m == 2,
        ),
        KeyCode::Space => legacy(vec![b' '], u32::from(b' '), m == 1),
        KeyCode::Tab if m == 2 && !all_keys => vec![ESCAPE, b'[', b'Z'], // backtab
        KeyCode::Tab => legacy(vec![b'\t'], u32::from(b'\t'), m == 1),
        KeyCode::Enter => legacy(vec![CARRIAGE_RETURN], u32::from(CARRIAGE_RETURN), m == 1),
        // Telling Escape apart from the start of a sequence is what the
        // protocol is for, so it is never sent bare.
        KeyCode::Escape => kitty_csi(u32::from(ESCAPE), m),
        KeyCode::Backspace => legacy(vec![DELETE], u32::from(DELETE), m == 1),
        KeyCode::F(n @ 13..=24) => kitty_csi(KITTY_F13 + u32::from(n) - 13, m),
        code => match kitty_legacy_shape(code) {
            Some((number, last)) => kitty_legacy_key(number, last, m, false),
            None => Vec::new(),
        },
    }
}
pub(super) fn csi(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', final_byte]
}
/// `param` is written as its decimal ASCII digits (`3` -> `b"3"`), not the raw
/// byte value: `3u8` alone is the ETX control character, and a raw control
/// byte embedded mid-sequence gets caught by the pty's line discipline as a
/// signal (e.g. `3` is `C-c`/`VINTR`) instead of reaching the app as part
/// of the escape sequence.
pub(super) fn csi_param(param: u8, final_byte: u8) -> Vec<u8> {
    let mut bytes = vec![ESCAPE, b'['];
    bytes.extend(param.to_string().into_bytes());
    bytes.push(final_byte);
    bytes
}
pub(super) fn ss3(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'O', final_byte]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{resolve, FocusDir, PendingPrefix};
    use super::*;
    use crate::model::input::test_support::*;

    #[test]
    fn test_insert_sends_printable_bytes() {
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Char('a'))),
            Action::SendBytes(vec![b'a'])
        );
    }
    #[test]
    fn test_insert_encodes_control_chars() {
        let ctrl_c = Key {
            ctrl: true,
            ..key(KeyCode::Char('c'))
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_c),
            Action::SendBytes(vec![0x03])
        );
    }
    #[test]
    fn test_insert_encodes_arrow_keys() {
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Up)),
            Action::SendBytes(vec![0x1b, b'[', b'A'])
        );
    }
    #[test]
    fn test_delete_key_sends_ascii_tilde_sequence() {
        // Regression: `csi_param` used to splice the raw byte value 3 into the
        // sequence instead of the ASCII digit '3', producing
        // `\x1b[\x03~`, embedding a literal C-c (ETX) that a pty's line
        // discipline intercepts as SIGINT, so a bare Delete keypress could
        // kill the foreground program instead of forward-deleting.
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Delete)),
            Action::SendBytes(b"\x1b[3~".to_vec())
        );
    }
    #[test]
    fn test_tilde_form_keys_use_ascii_digits_for_their_param() {
        // Every `tilde_xterm` key must carry its CSI parameter as ASCII
        // digits, including the multi-digit function-key params (e.g. F5 is
        // `15`, two bytes `b"15"`, not the single raw byte 15).
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Insert)),
            Action::SendBytes(b"\x1b[2~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::PageUp)),
            Action::SendBytes(b"\x1b[5~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::PageDown)),
            Action::SendBytes(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::F(5))),
            Action::SendBytes(b"\x1b[15~".to_vec())
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::F(12))),
            Action::SendBytes(b"\x1b[24~".to_vec())
        );
    }
    #[test]
    fn test_esc_is_always_sent_to_pty_in_insert_mode() {
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Insert, &key(KeyCode::Escape), &mut pending, 0),
            Action::SendBytes(vec![0x1b])
        );
    }
    #[test]
    fn test_entry_chord_switches_to_normal() {
        let alt_esc = Key {
            alt: true,
            ..key(KeyCode::Escape)
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &alt_esc),
            Action::SwitchMode(Mode::Normal)
        );
        // C-S-SPC is not an entry chord; it goes to the program.
        let ctrl_shift_space = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Space)
        };
        assert_ne!(
            resolve_simple(Mode::Insert, &ctrl_shift_space),
            Action::SwitchMode(Mode::Normal)
        );
    }
    #[test]
    fn test_alt_i_selects_the_paragraph_in_normal_mode() {
        let alt_i = Key {
            alt: true,
            ..key(KeyCode::Char('i'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &alt_i),
            Action::SelectParagraph
        );
        // Alt-Shift-i is another chord, and is not this one.
        let alt_shift_i = Key {
            alt: true,
            shift: true,
            sup: false,
            ..key(KeyCode::Char('I'))
        };
        assert_eq!(resolve_simple(Mode::Normal, &alt_shift_i), Action::Ignore);
        // M-i in Insert mode still belongs to the shell.
        assert_ne!(
            resolve_simple(Mode::Insert, &alt_i),
            Action::SelectParagraph
        );
    }
    #[test]
    fn test_alt_hjkl_moves_pane_focus() {
        let alt = |c: char| Key {
            alt: true,
            shift: false,
            sup: false,
            ..key(KeyCode::Char(c))
        };
        let cases = [
            (alt('h'), FocusDir::Left),
            (alt('j'), FocusDir::Down),
            (alt('k'), FocusDir::Up),
            (alt('l'), FocusDir::Right),
        ];
        for (k, dir) in cases {
            assert_eq!(resolve_simple(Mode::Normal, &k), Action::FocusPane(dir));
            assert_eq!(resolve_simple(Mode::Insert, &k), Action::FocusPane(dir));
            // Regression: `resolve_visual` used to skip the window keymap
            // entirely, so pane-focus chords either did nothing or, worse,
            // were shadowed by Visual's own `h`/`j`/`k`/`l` motions instead
            // of moving focus.
            assert_eq!(resolve_simple(Mode::Visual, &k), Action::FocusPane(dir));
        }
    }
    #[test]
    fn test_ctrl_digit_goes_to_tab() {
        for n in 1usize..=9 {
            let k = Key {
                ctrl: true,
                alt: false,
                shift: false,
                sup: false,
                code: KeyCode::Char(char::from_digit(n as u32, 10).unwrap()),
            };
            assert_eq!(resolve_simple(Mode::Normal, &k), Action::GotoTab(n));
            assert_eq!(resolve_simple(Mode::Insert, &k), Action::GotoTab(n));
        }
    }
    #[test]
    fn test_alt_digit_focuses_pane_by_default() {
        for n in 1usize..=9 {
            let k = Key {
                ctrl: false,
                alt: true,
                shift: false,
                sup: false,
                code: KeyCode::Char(char::from_digit(n as u32, 10).unwrap()),
            };
            assert_eq!(
                resolve_simple(Mode::Normal, &k),
                Action::FocusPaneByIndex(n)
            );
            assert_eq!(
                resolve_simple(Mode::Insert, &k),
                Action::FocusPaneByIndex(n)
            );
            assert_eq!(
                resolve_simple(Mode::Visual, &k),
                Action::FocusPaneByIndex(n)
            );
        }
    }
    #[test]
    fn test_ctrl_alt_digit_closes_pane_by_default() {
        for n in 1usize..=9 {
            let k = Key {
                ctrl: true,
                alt: true,
                shift: false,
                sup: false,
                code: KeyCode::Char(char::from_digit(n as u32, 10).unwrap()),
            };
            assert_eq!(
                resolve_simple(Mode::Normal, &k),
                Action::ClosePaneByIndex(n)
            );
            assert_eq!(
                resolve_simple(Mode::Insert, &k),
                Action::ClosePaneByIndex(n)
            );
        }
    }
    #[test]
    fn test_insert_sends_shell_control_chars_to_pty() {
        // Every Ctrl+letter that the shell binds (readline / vi mode) must reach
        // the PTY as its control byte, never be claimed by window bindings.
        let ctrl = |c: char| Key {
            ctrl: true,
            ..key(KeyCode::Char(c))
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('a')),
            Action::SendBytes(vec![0x01])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('e')),
            Action::SendBytes(vec![0x05])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('h')),
            Action::SendBytes(vec![0x08])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('j')),
            Action::SendBytes(vec![0x0a])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('k')),
            Action::SendBytes(vec![0x0b])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('l')),
            Action::SendBytes(vec![0x0c])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('u')),
            Action::SendBytes(vec![0x15])
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl('w')),
            Action::SendBytes(vec![0x17])
        );
    }
    #[test]
    fn test_ctrl_slash_is_prompt_undo_in_both_modes() {
        let undo = Key {
            ctrl: true,
            ..key(KeyCode::Char('/'))
        };
        assert_eq!(resolve_simple(Mode::Insert, &undo), Action::PromptUndo);
        assert_eq!(resolve_simple(Mode::Normal, &undo), Action::PromptUndo);
    }
    #[test]
    fn test_ctrl_backslash_is_prompt_redo_in_both_modes() {
        let redo = Key {
            ctrl: true,
            ..key(KeyCode::Char('\\'))
        };
        assert_eq!(resolve_simple(Mode::Insert, &redo), Action::PromptRedo);
        assert_eq!(resolve_simple(Mode::Normal, &redo), Action::PromptRedo);
    }
    #[test]
    fn test_plain_slash_is_not_undo() {
        // `/` without Ctrl still starts a block search in Normal mode and is
        // forwarded to the PTY in Insert mode.
        assert_eq!(
            resolve_simple(Mode::Insert, &key(KeyCode::Char('/'))),
            Action::SendBytes(vec![b'/'])
        );
        let mut pending = PendingPrefix::None;
        assert_eq!(
            resolve(Mode::Normal, &key(KeyCode::Char('/')), &mut pending, 0),
            Action::SearchStart
        );
    }
    #[test]
    fn test_xterm_ctrl_i_equals_tab_ambiguity() {
        // In legacy xterm mode, C-i and Tab produce the same bytes.
        let tab = resolve_simple(Mode::Insert, &key(KeyCode::Tab));
        let ctrl_i = resolve_simple(
            Mode::Insert,
            &Key {
                ctrl: true,
                ..key(KeyCode::Char('i'))
            },
        );
        assert_eq!(tab, ctrl_i);
    }
    #[test]
    fn test_kitty_ctrl_i_differs_from_tab() {
        // In Kitty mode, Tab still produces \t but C-i is disambiguated.
        let tab = kitty(KeyCode::Tab);
        let ctrl_i = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Char('i'))
        });
        assert_ne!(tab, ctrl_i);
        assert_eq!(tab, Action::SendBytes(vec![b'\t']));
        // C-i: codepoint 105 ('i'), modifier 5 (ctrl=4, +1 base).
        assert_eq!(ctrl_i, Action::SendBytes(b"\x1b[105;5u".to_vec()));
    }
    #[test]
    fn test_kitty_shift_enter() {
        let shift_enter = kitty_key(Key {
            shift: true,
            sup: false,
            ..key(KeyCode::Enter)
        });
        // Codepoint 13 (CR), modifier 2 (shift).
        assert_eq!(shift_enter, Action::SendBytes(b"\x1b[13;2u".to_vec()));
    }
    #[test]
    fn test_kitty_bare_enter_unchanged() {
        assert_eq!(kitty(KeyCode::Enter), Action::SendBytes(vec![b'\r']));
    }
    #[test]
    fn test_kitty_ctrl_escape() {
        let ctrl_esc = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Escape)
        });
        // Codepoint 27 (ESC), modifier 5 (ctrl).
        assert_eq!(ctrl_esc, Action::SendBytes(b"\x1b[27;5u".to_vec()));
    }
    #[test]
    fn test_kitty_bare_escape_is_disambiguated() {
        // Regression: a bare Escape went out as the lone ESC byte, which is
        // exactly the ambiguity the protocol's first flag exists to remove.
        assert_eq!(
            kitty(KeyCode::Escape),
            Action::SendBytes(b"\x1b[27u".to_vec())
        );
    }
    #[test]
    fn test_kitty_report_all_keys_sends_text_keys_as_escape_codes() {
        let all_keys = |key: Key| encode(&key, KITTY_REPORT_ALL_KEYS | 1, None);
        assert_eq!(all_keys(key(KeyCode::Char('a'))), b"\x1b[97u".to_vec());
        assert_eq!(
            all_keys(Key {
                shift: true,
                ..key(KeyCode::Char('A'))
            }),
            b"\x1b[97;2u".to_vec()
        );
        assert_eq!(all_keys(key(KeyCode::Enter)), b"\x1b[13u".to_vec());
        assert_eq!(all_keys(key(KeyCode::Tab)), b"\x1b[9u".to_vec());
        assert_eq!(all_keys(key(KeyCode::Backspace)), b"\x1b[127u".to_vec());
        assert_eq!(all_keys(key(KeyCode::Space)), b"\x1b[32u".to_vec());
        // Without the flag, text keeps its bytes.
        assert_eq!(encode(&key(KeyCode::Char('a')), 1, None), b"a".to_vec());
        assert_eq!(encode(&key(KeyCode::Enter), 1, None), b"\r".to_vec());
    }
    #[test]
    fn test_ctrl_on_a_symbol_key_sends_its_control_byte() {
        // Regression: only Ctrl+letter was a control byte; Ctrl+Space sent a
        // space and Ctrl+] a bracket.
        let ctrl = |code: KeyCode| {
            encode(
                &Key {
                    ctrl: true,
                    ..key(code)
                },
                0,
                None,
            )
        };
        assert_eq!(ctrl(KeyCode::Space), vec![0x00]);
        for (c, byte) in [
            ('@', 0x00),
            ('2', 0x00),
            ('[', 0x1b),
            ('\\', 0x1c),
            (']', 0x1d),
            ('^', 0x1e),
            ('6', 0x1e),
            ('_', 0x1f),
            ('/', 0x1f),
            ('?', 0x7f),
        ] {
            assert_eq!(ctrl(KeyCode::Char(c)), vec![byte], "C-{c}");
        }
        // A symbol with no control byte is sent as it is.
        assert_eq!(ctrl(KeyCode::Char(',')), b",".to_vec());
        // Alt still prefixes ESC.
        let ctrl_alt_space = Key {
            alt: true,
            ctrl: true,
            ..key(KeyCode::Space)
        };
        assert_eq!(encode(&ctrl_alt_space, 0, None), vec![ESCAPE, 0x00]);
    }
    #[test]
    fn test_kitty_ctrl_left_arrow() {
        let ctrl_left = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Left)
        });
        // Left keeps its legacy shape, with modifier 5 (ctrl).
        assert_eq!(ctrl_left, Action::SendBytes(b"\x1b[1;5D".to_vec()));
    }
    #[test]
    fn test_kitty_printable_no_modifier_passthrough() {
        assert_eq!(kitty(KeyCode::Char('a')), Action::SendBytes(vec![b'a']));
        assert_eq!(kitty(KeyCode::Char('Z')), Action::SendBytes(vec![b'Z']));
    }
    #[test]
    fn test_kitty_ctrl_printable() {
        // C-a: codepoint 97 ('a'), modifier 5 (ctrl).
        let ctrl_a = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Char('a'))
        });
        assert_eq!(ctrl_a, Action::SendBytes(b"\x1b[97;5u".to_vec()));
    }
    #[test]
    fn test_kitty_f1_through_f4() {
        assert_eq!(kitty(KeyCode::F(1)), Action::SendBytes(b"\x1b[P".to_vec()));
        assert_eq!(kitty(KeyCode::F(4)), Action::SendBytes(b"\x1b[S".to_vec()));
    }
    #[test]
    fn test_kitty_arrows_keep_their_legacy_shape() {
        // Regression: with the protocol on, a bare Up went out as
        // `CSI 57352 u`, a codepoint no program reads as an arrow, so a menu
        // answered to `j` and `C-n` but not to the arrow keys.
        assert_eq!(kitty(KeyCode::Up), Action::SendBytes(b"\x1b[A".to_vec()));
        assert_eq!(kitty(KeyCode::Down), Action::SendBytes(b"\x1b[B".to_vec()));
        assert_eq!(kitty(KeyCode::Home), Action::SendBytes(b"\x1b[H".to_vec()));
        assert_eq!(
            kitty(KeyCode::Delete),
            Action::SendBytes(b"\x1b[3~".to_vec())
        );
        assert_eq!(
            kitty(KeyCode::PageDown),
            Action::SendBytes(b"\x1b[6~".to_vec())
        );
        assert_eq!(
            kitty(KeyCode::F(5)),
            Action::SendBytes(b"\x1b[15~".to_vec())
        );
    }
    #[test]
    fn test_a_text_key_reports_no_release_unless_every_key_is_escaped() {
        // pi asks for 7 (disambiguate, event types, alternate keys): typing a
        // letter must send the letter and nothing after it.
        let a = key(KeyCode::Char('a'));
        assert_eq!(encode_release(&a, 7), Vec::<u8>::new());
        for plain in [
            KeyCode::Enter,
            KeyCode::Tab,
            KeyCode::Backspace,
            KeyCode::Space,
        ] {
            assert_eq!(
                encode_release(&key(plain), 7),
                Vec::<u8>::new(),
                "{plain:?}"
            );
        }
        assert_eq!(
            encode_release(&a, 15),
            b"\x1b[97;1:3u".to_vec(),
            "with every key escaped, the modifier field carries the event type"
        );
    }

    #[test]
    fn test_a_key_pressed_as_an_escape_code_reports_its_release() {
        let ctrl_a = Key {
            ctrl: true,
            ..key(KeyCode::Char('a'))
        };
        assert_eq!(encode_release(&ctrl_a, 7), b"\x1b[97;5:3u".to_vec());
        assert_eq!(
            encode_release(&key(KeyCode::Escape), 7),
            b"\x1b[27;1:3u".to_vec()
        );
        let shift_tab = Key {
            shift: true,
            ..key(KeyCode::Tab)
        };
        assert_eq!(
            encode_release(&shift_tab, 7),
            Vec::<u8>::new(),
            "legacy backtab"
        );
        assert_eq!(
            encode_release(&ctrl_a, 1),
            Vec::<u8>::new(),
            "no event types asked"
        );
    }

    #[test]
    fn test_kitty_release_of_a_legacy_key_carries_the_event_type() {
        assert_eq!(
            encode_release(&key(KeyCode::Up), 2),
            b"\x1b[1;1:3A".to_vec()
        );
        let ctrl_delete = Key {
            ctrl: true,
            ..key(KeyCode::Delete)
        };
        assert_eq!(encode_release(&ctrl_delete, 2), b"\x1b[3;5:3~".to_vec());
    }
    #[test]
    fn test_kitty_shift_tab_backtab() {
        let shift_tab = kitty_key(Key {
            shift: true,
            sup: false,
            ..key(KeyCode::Tab)
        });
        assert_eq!(shift_tab, Action::SendBytes(vec![ESCAPE, b'[', b'Z']));
    }
    #[test]
    fn test_scroll_keybindings_resolve() {
        let shift_alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: true,
            sup: false,
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('h')),
            Action::ScrollPageUp
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('l')),
            Action::ScrollPageDown
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('j')),
            Action::ScrollLineDown
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('k')),
            Action::ScrollLineUp
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('a')),
            Action::ScrollToTop
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('e')),
            Action::ScrollToBottom
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt(',')),
            Action::ScrollToTop
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('.')),
            Action::ScrollToBottom
        );

        // Also check that it intercepts in Insert mode
        assert_eq!(
            resolve_simple(Mode::Insert, &shift_alt('h')),
            Action::ScrollPageUp
        );
    }
    #[test]
    fn test_tab_switching_keybindings_resolve() {
        let ctrl_pageup = Key {
            alt: false,
            code: KeyCode::PageUp,
            ctrl: true,
            shift: false,
            sup: false,
        };
        let ctrl_pagedown = Key {
            alt: false,
            code: KeyCode::PageDown,
            ctrl: true,
            shift: false,
            sup: false,
        };
        let ctrl_bracket = |c: char| Key {
            alt: false,
            code: KeyCode::Char(c),
            ctrl: true,
            shift: false,
            sup: false,
        };
        assert_eq!(resolve_simple(Mode::Normal, &ctrl_pageup), Action::PrevTab);
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_pagedown),
            Action::NextTab
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_bracket('[')),
            Action::PrevTab
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_bracket(']')),
            Action::NextTab
        );

        // Also check that it intercepts in Insert mode
        assert_eq!(resolve_simple(Mode::Insert, &ctrl_pageup), Action::PrevTab);
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_pagedown),
            Action::NextTab
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_bracket('[')),
            Action::PrevTab
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_bracket(']')),
            Action::NextTab
        );
    }
    #[test]
    fn test_shift_alt_letter_encodes_esc_prefix_in_legacy_mode() {
        // M-S-e in legacy (flags=0) mode produces \x1bE (ESC + uppercase
        // letter). The shift is encoded as the uppercase character; the alt is
        // the ESC prefix. This is the correct xterm encoding but is timing-
        // ambiguous (can parse as Escape then E).
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            sup: false,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, None);
        assert_eq!(bytes, b"\x1bE".to_vec());
    }
    #[test]
    fn test_shift_alt_letter_encodes_csi_u_in_kitty_mode() {
        // M-S-e with Kitty flags active produces \x1b[101;4u: the
        // unambiguous CSI-u encoding (codepoint 101='e', modifier 4=shift+alt).
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            sup: false,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 1, None); // flag 1 = disambiguate
        assert_eq!(bytes, b"\x1b[101;4u".to_vec());
    }
    #[test]
    fn test_shift_alt_h_l_encode_correctly_in_kitty_mode() {
        // Verify M-S-h and M-S-l as well.
        let mk = |c: char| Key {
            alt: true,
            shift: true,
            sup: false,
            ctrl: false,
            code: KeyCode::Char(c),
        };
        assert_eq!(encode(&mk('H'), 1, None), b"\x1b[104;4u".to_vec());
        assert_eq!(encode(&mk('L'), 1, None), b"\x1b[108;4u".to_vec());
    }
    #[test]
    fn test_modify_other_keys_shift_alt_e_uses_27_format() {
        // When modifyOtherKeys is active (mode 2), M-S-e produces
        // `\x1b[27;4;69~` (modifier 4 = shift+alt, codepoint 69 = 'E').
        // This is unambiguous: no ESC prefix that could parse as standalone Escape.
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            sup: false,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, Some(2));
        assert_eq!(bytes, b"\x1b[27;4;69~".to_vec());
    }
    #[test]
    fn test_modify_other_keys_disabled_falls_back_to_esc_prefix() {
        // Without modifyOtherKeys, M-S-e falls back to the legacy \x1bE.
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            sup: false,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, None);
        assert_eq!(bytes, b"\x1bE".to_vec());
    }
    #[test]
    fn test_modify_other_keys_unmodified_char_not_affected() {
        // A bare character (no modifiers) is never affected by modifyOtherKeys.
        let plain_e = Key {
            alt: false,
            shift: false,
            sup: false,
            ctrl: false,
            code: KeyCode::Char('e'),
        };
        let bytes = encode(&plain_e, 0, Some(2));
        assert_eq!(bytes, b"e".to_vec());
    }

    #[test]
    fn test_a_window_sequence_leader_is_left_to_the_app_in_insert_mode() {
        // The app resolves window sequences ahead of the mode keymaps, so the
        // mode keymap never sees `M-g`; if it does, it is just a chord.
        let win = Keymap::default();
        let mut pending = PendingPrefix::None;
        let alt_g = Key::with_alt(KeyCode::Char('g'));
        super::super::resolve_with(Mode::Insert, &alt_g, &mut pending, &win, 0, None, false);
        assert_eq!(pending, PendingPrefix::None);
    }
    #[test]
    fn test_a_super_chord_is_not_typed_into_the_shell_as_its_bare_character() {
        let super_x = Key {
            sup: true,
            ..key(KeyCode::Char('x'))
        };
        assert_eq!(encode(&super_x, 0, None), Vec::<u8>::new());
        // Kitty can say Super, so it is sent with its modifier (1 + 8).
        assert_eq!(encode(&super_x, 1, None), b"\x1b[120;9u".to_vec());
        // A key with a parameter slot carries it.
        let super_up = Key {
            sup: true,
            ..key(KeyCode::Up)
        };
        assert_eq!(encode(&super_up, 0, None), b"\x1b[1;9A".to_vec());
    }

    #[test]
    fn test_kitty_tells_every_key_and_modifier_set_apart() {
        // A program that asks for the kitty protocol is owed every chord as
        // its own sequence: no two sets of modifiers on a key may collide.
        let mut codes = vec![
            KeyCode::Backspace,
            KeyCode::Delete,
            KeyCode::Down,
            KeyCode::End,
            KeyCode::Enter,
            KeyCode::Escape,
            KeyCode::Home,
            KeyCode::Insert,
            KeyCode::Left,
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Right,
            KeyCode::Space,
            KeyCode::Tab,
            KeyCode::Up,
        ];
        codes.extend((1..=24).map(KeyCode::F));
        codes.extend("a1,/".chars().map(KeyCode::Char));
        for code in codes {
            let mut seen = std::collections::HashMap::new();
            for bits in 0..16u8 {
                let key = Key {
                    alt: bits & 1 != 0,
                    code,
                    ctrl: bits & 2 != 0,
                    shift: bits & 4 != 0,
                    sup: bits & 8 != 0,
                };
                // A lone Shift on a symbol arrives as the shifted glyph, never as
                // the unshifted one with the flag, so that pair is not a key.
                let unreal = matches!(code, KeyCode::Char(c) if !c.is_ascii_alphabetic())
                    && key.shift
                    && !key.ctrl
                    && !key.alt
                    && !key.sup;
                if unreal {
                    continue;
                }
                let bytes = encode(&key, 1, None);
                assert!(!bytes.is_empty(), "{key:?} sends nothing");
                if let Some(other) = seen.insert(bytes.clone(), key) {
                    panic!("{key:?} and {other:?} both send {bytes:?}");
                }
            }
        }
    }

    #[test]
    fn test_application_cursor_keys_send_ss3_for_unmodified_keys_only() {
        let send = |bytes: &[u8]| Action::SendBytes(bytes.to_vec());
        for (normal, application) in [
            (b"\x1b[A", b"\x1bOA"),
            (b"\x1b[D", b"\x1bOD"),
            (b"\x1b[H", b"\x1bOH"),
            (b"\x1b[F", b"\x1bOF"),
        ] {
            assert_eq!(
                apply_cursor_key_mode(send(normal), true, 0),
                send(application)
            );
            assert_eq!(
                apply_cursor_key_mode(send(normal), false, 0),
                send(normal),
                "mode off"
            );
        }
        // A modified arrow keeps its CSI form, as in xterm.
        assert_eq!(
            apply_cursor_key_mode(send(b"\x1b[1;5A"), true, 0),
            send(b"\x1b[1;5A")
        );
        // The kitty protocol has its own forms.
        assert_eq!(
            apply_cursor_key_mode(send(b"\x1b[A"), true, 1),
            send(b"\x1b[A")
        );
        assert_eq!(
            apply_cursor_key_mode(Action::Ignore, true, 0),
            Action::Ignore
        );
    }
}

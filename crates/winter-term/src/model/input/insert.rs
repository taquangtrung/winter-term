//! Insert mode: encoding key events into the bytes the PTY expects.

use super::WindowKeymap;
use super::{Action, Key, KeyCode};
use crate::model::mode::{Mode, ModeEvent};

// ========================================================================
// Insert mode: key-to-byte encoding
// ========================================================================

pub(super) const CONTROL_MASK: u8 = 0x1f;
pub(super) const DELETE: u8 = 0x7f;
pub(super) const ESCAPE: u8 = 0x1b;
pub(super) const CARRIAGE_RETURN: u8 = b'\r';
// Kitty keyboard protocol: functional-key codepoints.
// https://sw.kovidgoyal.net/kitty/keyboard-protocol/#functional-key-definitions
pub(super) const KP_INSERT: u32 = 57348;
pub(super) const KP_DELETE: u32 = 57349;
pub(super) const KP_LEFT: u32 = 57350;
pub(super) const KP_RIGHT: u32 = 57351;
pub(super) const KP_UP: u32 = 57352;
pub(super) const KP_DOWN: u32 = 57353;
pub(super) const KP_PAGE_UP: u32 = 57354;
pub(super) const KP_PAGE_DOWN: u32 = 57355;
pub(super) const KP_HOME: u32 = 57356;
pub(super) const KP_END: u32 = 57357;
pub(super) const KP_F1: u32 = 57364;
pub(super) fn resolve_insert(
    key: &Key,
    flags: u32,
    modify_other_keys: Option<i64>,
    window: &WindowKeymap,
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
    // Configurable line edits and prompt undo/redo (defaults: `Ctrl-Backspace` →
    // delete word back, `Ctrl-/` → undo, `Ctrl-\` → redo). Claiming the undo/redo
    // chords also stops them reaching the PTY as raw control bytes 0x1f / 0x1c.
    if let Some(binding) = window.edit_binding(key) {
        return binding.to_action();
    }
    // Escape always goes to the PTY. Context-aware mode switching (e.g. at the
    // shell prompt) is handled one layer up in the application event loop.
    Action::SendBytes(encode(key, flags, modify_other_keys))
}
pub(super) fn resolve_block_focus(
    key: &Key,
    flags: u32,
    window: &WindowKeymap,
    is_alt_screen: bool,
) -> Action {
    if key.code == KeyCode::Escape {
        return Action::SwitchMode(Mode::BlockFocus.apply(ModeEvent::Escape));
    }
    if let Some(action) = window.direct_action(key, is_alt_screen) {
        return action;
    }
    Action::ForwardToBlock(encode(key, flags, None))
}
pub(super) fn is_entry_chord(key: &Key) -> bool {
    key.ctrl && key.shift && key.code == KeyCode::Space
}
/// Encode a key as the bytes a terminal program expects on the PTY.
/// `flags` is the active Kitty keyboard protocol bitmask for the pane
/// (0 = legacy xterm encoding). `modify_other_keys` is the xterm
/// modifyOtherKeys mode (`None` = off, `Some(1)` or `Some(2)`).
pub(super) fn encode(key: &Key, flags: u32, modify_other_keys: Option<i64>) -> Vec<u8> {
    if flags != 0 {
        encode_kitty(key)
    } else {
        encode_xterm(key, modify_other_keys)
    }
}
/// Encode a key-release event for the Kitty protocol.
/// Returns bytes only when `flags & 2 != 0` (bit 1: report event types).
/// Release sequences use `: 3` as the event-type sub-field:
///   `CSI codepoint :: 3 u`  (no modifier)
///   `CSI codepoint ; modifier : 3 u`  (with modifier)
pub fn encode_release(key: &Key, flags: u32) -> Vec<u8> {
    if flags & 2 == 0 {
        return Vec::new();
    }
    encode_kitty_release(key)
}
/// xterm modifier byte: 1 + shift + 2*alt + 4*ctrl. Value 1 means no modifier.
pub(super) fn xterm_modifier(key: &Key) -> u8 {
    1 + (key.shift as u8) + 2 * (key.alt as u8) + 4 * (key.ctrl as u8)
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
        _ => Vec::new(),
    }
}
pub(super) fn encode_xterm(key: &Key, modify_other_keys: Option<i64>) -> Vec<u8> {
    let m = xterm_modifier(key);
    let bytes = match key.code {
        KeyCode::Backspace => vec![DELETE],
        KeyCode::Char('\0') => return Vec::new(),
        KeyCode::Char(c) => {
            if key.ctrl && c.is_ascii_alphabetic() {
                vec![(c.to_ascii_uppercase() as u8) & CONTROL_MASK]
            } else {
                c.to_string().into_bytes()
            }
        }
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
        KeyCode::Space => vec![b' '],
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
    // standalone Escape, so Shift+Alt+E/H/L arrive intact. Mirrors WezTerm.
    //
    // Mode 1 excludes a few well-known keys from the extended encoding (the
    // same set xterm excludes); mode 2 applies it to all modified chars.
    if let Some(mode) = modify_other_keys {
        if let (KeyCode::Char(c), true) = (key.code, (key.shift || key.ctrl || key.alt)) {
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
    if key.alt && !matches!(key.code, KeyCode::Escape | KeyCode::Char('\0')) {
        esc_prefix(bytes)
    } else {
        bytes
    }
}
/// Kitty modifier value: 1 + shift + 2*alt + 4*ctrl. Value 1 means no modifier.
pub(super) fn kitty_modifier(key: &Key) -> u32 {
    1 + (key.shift as u32) + 2 * (key.alt as u32) + 4 * (key.ctrl as u32)
}
/// `CSI codepoint u` or `CSI codepoint ; modifier u` (omit `;1`).
pub(super) fn kitty_csi(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}u").into_bytes()
    }
}
/// Release variant: `CSI codepoint :: 3 u` (no modifier) or
/// `CSI codepoint ; modifier : 3 u` (with modifier).
pub(super) fn kitty_csi_release(codepoint: u32, modifier: u32) -> Vec<u8> {
    if modifier == 1 {
        format!("\x1b[{codepoint}::3u").into_bytes()
    } else {
        format!("\x1b[{codepoint};{modifier}:3u").into_bytes()
    }
}
/// Kitty release encoding: same key mapping as `encode_kitty` but with the
/// `: 3` event-type suffix. Keys that map to raw bytes on press (bare chars,
/// bare Tab, bare Enter, etc.) get full CSI sequences on release so the app
/// can distinguish press from release.
pub(super) fn encode_kitty_release(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.code {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => kitty_csi_release(base_codepoint(c, key.shift), m),
        KeyCode::Space => kitty_csi_release(32, m),
        KeyCode::Tab => kitty_csi_release(9, m),
        KeyCode::Enter => kitty_csi_release(13, m),
        KeyCode::Escape => kitty_csi_release(27, m),
        KeyCode::Backspace => kitty_csi_release(127, m),
        KeyCode::Insert => kitty_csi_release(KP_INSERT, m),
        KeyCode::Delete => kitty_csi_release(KP_DELETE, m),
        KeyCode::Left => kitty_csi_release(KP_LEFT, m),
        KeyCode::Right => kitty_csi_release(KP_RIGHT, m),
        KeyCode::Up => kitty_csi_release(KP_UP, m),
        KeyCode::Down => kitty_csi_release(KP_DOWN, m),
        KeyCode::PageUp => kitty_csi_release(KP_PAGE_UP, m),
        KeyCode::PageDown => kitty_csi_release(KP_PAGE_DOWN, m),
        KeyCode::Home => kitty_csi_release(KP_HOME, m),
        KeyCode::End => kitty_csi_release(KP_END, m),
        KeyCode::F(n @ 1..=12) => kitty_csi_release(KP_F1 + (n as u32 - 1), m),
        KeyCode::F(_) => Vec::new(),
    }
}
/// The base Unicode codepoint of a character key (lowercase for ASCII alpha
/// so Ctrl+Shift+a uses codepoint 97, not 65).
pub(super) fn base_codepoint(c: char, shift: bool) -> u32 {
    if shift && c.is_ascii_uppercase() {
        c.to_ascii_lowercase() as u32
    } else {
        c as u32
    }
}
pub(super) fn encode_kitty(key: &Key) -> Vec<u8> {
    let m = kitty_modifier(key);
    match key.code {
        KeyCode::Char('\0') => Vec::new(),
        KeyCode::Char(c) => {
            // No modifier or lone Shift: send raw UTF-8. Shift is already
            // encoded in the char winit provides ('A' for Shift+a).
            if m == 1 || m == 2 {
                return c.to_string().into_bytes();
            }
            kitty_csi(base_codepoint(c, key.shift), m)
        }
        KeyCode::Space => {
            if m == 1 {
                vec![b' ']
            } else {
                kitty_csi(32, m)
            }
        }
        KeyCode::Tab => {
            if m == 1 {
                vec![b'\t']
            } else if key.shift && !key.ctrl && !key.alt {
                vec![ESCAPE, b'[', b'Z'] // backtab
            } else {
                kitty_csi(9, m)
            }
        }
        KeyCode::Enter => {
            if m == 1 {
                vec![CARRIAGE_RETURN]
            } else {
                kitty_csi(13, m)
            }
        }
        KeyCode::Escape => {
            if m == 1 {
                vec![ESCAPE]
            } else {
                kitty_csi(27, m)
            }
        }
        KeyCode::Backspace => {
            if m == 1 {
                vec![DELETE]
            } else {
                kitty_csi(127, m)
            }
        }
        KeyCode::Insert => kitty_csi(KP_INSERT, m),
        KeyCode::Delete => kitty_csi(KP_DELETE, m),
        KeyCode::Left => kitty_csi(KP_LEFT, m),
        KeyCode::Right => kitty_csi(KP_RIGHT, m),
        KeyCode::Up => kitty_csi(KP_UP, m),
        KeyCode::Down => kitty_csi(KP_DOWN, m),
        KeyCode::PageUp => kitty_csi(KP_PAGE_UP, m),
        KeyCode::PageDown => kitty_csi(KP_PAGE_DOWN, m),
        KeyCode::Home => kitty_csi(KP_HOME, m),
        KeyCode::End => kitty_csi(KP_END, m),
        KeyCode::F(n @ 1..=12) => kitty_csi(KP_F1 + (n as u32 - 1), m),
        KeyCode::F(_) => Vec::new(),
    }
}
pub(super) fn csi(final_byte: u8) -> Vec<u8> {
    vec![ESCAPE, b'[', final_byte]
}
/// `param` is written as its decimal ASCII digits (`3` -> `b"3"`), not the raw
/// byte value: `3u8` alone is the ETX control character, and a raw control
/// byte embedded mid-sequence gets caught by the pty's line discipline as a
/// signal (e.g. `3` is `Ctrl-C`/`VINTR`) instead of reaching the app as part
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
        // `\x1b[\x03~`, embedding a literal Ctrl-C (ETX) that a pty's line
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
        let chord = Key {
            ctrl: true,
            shift: true,
            ..key(KeyCode::Space)
        };
        assert_eq!(
            resolve_simple(Mode::Insert, &chord),
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
        // Shift transforms the glyph on some layouts; both reach the same action.
        let alt_shift_i = Key {
            alt: true,
            shift: true,
            ..key(KeyCode::Char('I'))
        };
        assert_eq!(
            resolve_simple(Mode::Normal, &alt_shift_i),
            Action::SelectParagraph
        );
        // Alt-i in Insert mode still belongs to the shell.
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
        // In legacy xterm mode, Ctrl+I and Tab produce the same bytes.
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
        // In Kitty mode, Tab still produces \t but Ctrl+I is disambiguated.
        let tab = kitty(KeyCode::Tab);
        let ctrl_i = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Char('i'))
        });
        assert_ne!(tab, ctrl_i);
        assert_eq!(tab, Action::SendBytes(vec![b'\t']));
        // Ctrl+I: codepoint 105 ('i'), modifier 5 (ctrl=4, +1 base).
        assert_eq!(ctrl_i, Action::SendBytes(b"\x1b[105;5u".to_vec()));
    }
    #[test]
    fn test_kitty_shift_enter() {
        let shift_enter = kitty_key(Key {
            shift: true,
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
    fn test_kitty_bare_escape_unchanged() {
        assert_eq!(kitty(KeyCode::Escape), Action::SendBytes(vec![ESCAPE]));
    }
    #[test]
    fn test_kitty_ctrl_left_arrow() {
        let ctrl_left = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Left)
        });
        // KP_LEFT = 57350, modifier 5 (ctrl).
        assert_eq!(ctrl_left, Action::SendBytes(b"\x1b[57350;5u".to_vec()));
    }
    #[test]
    fn test_kitty_printable_no_modifier_passthrough() {
        assert_eq!(kitty(KeyCode::Char('a')), Action::SendBytes(vec![b'a']));
        assert_eq!(kitty(KeyCode::Char('Z')), Action::SendBytes(vec![b'Z']));
    }
    #[test]
    fn test_kitty_ctrl_printable() {
        // Ctrl+A: codepoint 97 ('a'), modifier 5 (ctrl).
        let ctrl_a = kitty_key(Key {
            ctrl: true,
            ..key(KeyCode::Char('a'))
        });
        assert_eq!(ctrl_a, Action::SendBytes(b"\x1b[97;5u".to_vec()));
    }
    #[test]
    fn test_kitty_f1_through_f4() {
        assert_eq!(
            kitty(KeyCode::F(1)),
            Action::SendBytes(b"\x1b[57364u".to_vec())
        );
        assert_eq!(
            kitty(KeyCode::F(4)),
            Action::SendBytes(b"\x1b[57367u".to_vec())
        );
    }
    #[test]
    fn test_kitty_shift_tab_backtab() {
        let shift_tab = kitty_key(Key {
            shift: true,
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
        };
        let ctrl_pagedown = Key {
            alt: false,
            code: KeyCode::PageDown,
            ctrl: true,
            shift: false,
        };
        let shift_alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: true,
        };
        assert_eq!(resolve_simple(Mode::Normal, &ctrl_pageup), Action::PrevTab);
        assert_eq!(
            resolve_simple(Mode::Normal, &ctrl_pagedown),
            Action::NextTab
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt('[')),
            Action::PrevTab
        );
        assert_eq!(
            resolve_simple(Mode::Normal, &shift_alt(']')),
            Action::NextTab
        );

        // Also check that it intercepts in Insert mode
        assert_eq!(resolve_simple(Mode::Insert, &ctrl_pageup), Action::PrevTab);
        assert_eq!(
            resolve_simple(Mode::Insert, &ctrl_pagedown),
            Action::NextTab
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &shift_alt('[')),
            Action::PrevTab
        );
        assert_eq!(
            resolve_simple(Mode::Insert, &shift_alt(']')),
            Action::NextTab
        );
    }
    #[test]
    fn test_shift_alt_letter_encodes_esc_prefix_in_legacy_mode() {
        // Shift+Alt+E in legacy (flags=0) mode produces \x1bE (ESC + uppercase
        // letter). The shift is encoded as the uppercase character; the alt is
        // the ESC prefix. This is the correct xterm encoding but is timing-
        // ambiguous (can parse as Escape then E).
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, None);
        assert_eq!(bytes, b"\x1bE".to_vec());
    }
    #[test]
    fn test_shift_alt_letter_encodes_csi_u_in_kitty_mode() {
        // Shift+Alt+E with Kitty flags active produces \x1b[101;4u: the
        // unambiguous CSI-u encoding (codepoint 101='e', modifier 4=shift+alt).
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 1, None); // flag 1 = disambiguate
        assert_eq!(bytes, b"\x1b[101;4u".to_vec());
    }
    #[test]
    fn test_shift_alt_h_l_encode_correctly_in_kitty_mode() {
        // Verify Shift+Alt+H and Shift+Alt+L as well.
        let mk = |c: char| Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char(c),
        };
        assert_eq!(encode(&mk('H'), 1, None), b"\x1b[104;4u".to_vec());
        assert_eq!(encode(&mk('L'), 1, None), b"\x1b[108;4u".to_vec());
    }
    #[test]
    fn test_modify_other_keys_shift_alt_e_uses_27_format() {
        // When modifyOtherKeys is active (mode 2), Shift+Alt+E produces
        // `\x1b[27;4;69~` (modifier 4 = shift+alt, codepoint 69 = 'E').
        // This is unambiguous: no ESC prefix that could parse as standalone Escape.
        let shift_alt_e = Key {
            alt: true,
            shift: true,
            ctrl: false,
            code: KeyCode::Char('E'),
        };
        let bytes = encode(&shift_alt_e, 0, Some(2));
        assert_eq!(bytes, b"\x1b[27;4;69~".to_vec());
    }
    #[test]
    fn test_modify_other_keys_disabled_falls_back_to_esc_prefix() {
        // Without modifyOtherKeys, Shift+Alt+E falls back to the legacy \x1bE.
        let shift_alt_e = Key {
            alt: true,
            shift: true,
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
            ctrl: false,
            code: KeyCode::Char('e'),
        };
        let bytes = encode(&plain_e, 0, Some(2));
        assert_eq!(bytes, b"e".to_vec());
    }
}

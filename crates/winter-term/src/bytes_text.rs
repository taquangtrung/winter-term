//! Bytes a program wrote, held as text without losing any. A file name or a
//! file's content need not be UTF-8 (a Latin-1 source file, a name written
//! on another system), and read as lossy text its odd bytes became U+FFFD:
//! a path that names no file, a patch whose lines match nothing. Each byte
//! that is not part of UTF-8 is held instead as a character of its own in a
//! private-use range, and turned back into that byte where the text goes
//! back out to a program, as an argument or as its input.

use std::borrow::Cow;
use std::ffi::OsString;
use std::path::PathBuf;

// ========================================================================
// Constants
// ========================================================================

/// The character a lone byte is held as is this plus the byte: the last 256
/// code points of plane 16's private use area.
const ESCAPE_BASE: u32 = 0x10_FF00;

// ========================================================================
// Functions
// ========================================================================

/// `bytes` as text, every byte that is not UTF-8 held as a character of its
/// own. A character of the escape range itself is held as its bytes, so
/// [`encode`] gives back exactly what came in.
pub(crate) fn decode(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len());
    for chunk in bytes.utf8_chunks() {
        for ch in chunk.valid().chars() {
            match is_escape(ch) {
                true => {
                    let mut utf8 = [0; 4];
                    ch.encode_utf8(&mut utf8).bytes().for_each(|byte| text.push(escape(byte)));
                }
                false => text.push(ch),
            }
        }
        chunk.invalid().iter().for_each(|byte| text.push(escape(*byte)));
    }
    text
}

/// The bytes `text` holds, each escaped byte (see [`decode`]) as itself.
pub(crate) fn encode(text: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len());
    for ch in text.chars() {
        match is_escape(ch) {
            true => bytes.push((u32::from(ch) - ESCAPE_BASE) as u8),
            false => {
                let mut utf8 = [0; 4];
                bytes.extend_from_slice(ch.encode_utf8(&mut utf8).as_bytes());
            }
        }
    }
    bytes
}

/// `text` as it is shown: each byte held for want of UTF-8 written `\xNN`,
/// as git and `ls -b` write one, where it would paint as a glyph the font
/// does not have.
pub(crate) fn display(text: &str) -> Cow<'_, str> {
    if !text.chars().any(is_escape) {
        return Cow::Borrowed(text);
    }
    let mut shown = String::with_capacity(text.len());
    for ch in text.chars() {
        match is_escape(ch) {
            true => shown.push_str(&format!("\\x{:02X}", u32::from(ch) - ESCAPE_BASE)),
            false => shown.push(ch),
        }
    }
    Cow::Owned(shown)
}

/// `text` as an argument for a program, with the bytes it holds.
pub(crate) fn to_os(text: &str) -> OsString {
    os_from_bytes(encode(text))
}

/// `text` as a path, with the bytes it holds: a name git reported, which
/// may not be UTF-8.
pub(crate) fn to_path(text: &str) -> PathBuf {
    PathBuf::from(to_os(text))
}

/// Whether `ch` is in the range a lone byte is held as.
fn is_escape(ch: char) -> bool {
    u32::from(ch) >= ESCAPE_BASE
}

/// The character `byte` is held as.
fn escape(byte: u8) -> char {
    char::from_u32(ESCAPE_BASE + u32::from(byte)).unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The OS string whose bytes are `bytes`, as a file name on this system is
/// any bytes but `/` and NUL.
#[cfg(unix)]
fn os_from_bytes(bytes: Vec<u8>) -> OsString {
    use std::os::unix::ffi::OsStringExt;
    OsString::from_vec(bytes)
}

/// The OS string `bytes` read as UTF-8 where a name is text.
#[cfg(not(unix))]
fn os_from_bytes(bytes: Vec<u8>) -> OsString {
    OsString::from(String::from_utf8_lossy(&bytes).into_owned())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bytes_that_are_not_utf8_come_back_exactly() {
        // Read lossily, `caf\xe9` came back as `caf\u{fffd}`: three bytes
        // where there was one, and no file of that name.
        let bytes = b"caf\xe9 \xff\xfe ok \xc3\xa9";
        let text = decode(bytes);
        assert!(text.starts_with("caf"));
        assert!(text.ends_with(" ok \u{e9}"), "UTF-8 stays as it is: {text:?}");
        assert_eq!(encode(&text), bytes);
    }

    #[test]
    fn test_a_character_of_the_escape_range_itself_survives_too() {
        let bytes = "\u{10ff41}A".as_bytes();
        assert_eq!(encode(&decode(bytes)), bytes);
    }

    #[test]
    fn test_a_byte_held_for_want_of_utf8_is_shown_as_its_value() {
        // Drawn as it was held, it painted as the font's missing glyph.
        assert_eq!(display(&decode(b"caf\xe9.rs")), "caf\\xE9.rs");
        assert!(matches!(display("plain é"), Cow::Borrowed("plain é")));
    }

    #[cfg(unix)]
    #[test]
    fn test_a_name_that_is_not_utf8_is_handed_back_as_its_own_bytes() {
        use std::os::unix::ffi::OsStrExt;
        let path = to_path(&decode(b"src/caf\xe9.rs"));
        assert_eq!(path.as_os_str().as_bytes(), b"src/caf\xe9.rs");
    }
}

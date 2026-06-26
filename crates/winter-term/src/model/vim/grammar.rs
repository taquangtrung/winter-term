//! The Vim grammar both modal surfaces share: the terminal's Normal and Visual
//! modes (`model::input`) and the text editor's (`model::textedit::modal`).
//!
//! Each surface keeps its own state machine, since one resolves keys into
//! actions on a live grid and the other edits a buffer in place. What a key
//! *means* in the grammar is decided here, once: how a count is typed and how
//! large it may grow, which keys are operators, which name a register or a
//! mark, and what the `g` and `z` motions are. A rule changed here changes on
//! both surfaces, so the two cannot drift apart.

use super::motion::CursorMove;
use crate::model::input::{Key, KeyCode};

// ========================================================================
// Constants
// ========================================================================

/// The largest count a `5j`-style prefix accumulates; further digits clamp
/// rather than overflow. Vim's own cap is far larger, but a count is spent
/// by repeating a command, and a runaway `99999999x` costs a walk of that
/// many steps for no editing value.
pub const MAX_COUNT: usize = 9999;

/// The radix count digits are read in.
const COUNT_RADIX: u32 = 10;

/// The motions a `g` prefix leads to on every surface: the key, the motion,
/// and what the key hints call it. A surface adds its own `g` commands
/// beside these (`gt` for tabs, `gb` for the open files).
pub const GOTO_MOTIONS: &[(char, CursorMove, &str)] = &[
    ('g', CursorMove::Top, "the first line"),
    ('e', CursorMove::WordEndBack, "the end of the word before"),
    (
        'E',
        CursorMove::WordEndBackBig,
        "the end of the WORD before",
    ),
    (
        '_',
        CursorMove::LastNonBlank,
        "the last character with ink on it",
    ),
];

/// The `z` placements of the cursor's line in the view.
pub const LINE_PLACEMENTS: &[(char, CursorMove, &str)] = &[
    ('z', CursorMove::LineToCenter, "this line to the middle"),
    ('t', CursorMove::LineToTop, "this line to the top"),
    ('b', CursorMove::LineToBottom, "this line to the bottom"),
];

/// The register that takes what is written to it and keeps none of it.
pub const BLACK_HOLE_REGISTER: char = '_';

/// The registers named by a symbol rather than a letter or digit: the system
/// clipboard (`+`), the primary selection (`*`), and the unnamed one (`"`).
const SYMBOL_REGISTERS: [char; 3] = ['+', '*', '"'];

/// The mark that names where the last jump started, however it is spelled.
pub const JUMP_MARK: char = '\'';

/// The marks kept without being set: the last change, where Insert was left,
/// and the two ends of the last yank.
const AUTOMATIC_MARKS: [char; 4] = ['.', '^', '[', ']'];

// ========================================================================
// Data Structures
// ========================================================================

/// The operators: commands that wait for a motion or an object saying how
/// far they reach.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OperatorKind {
    /// `c`: take the text and start typing in its place.
    Change,
    /// `d`: take the text out.
    Delete,
    /// `>`: push the lines it covers one level right.
    Indent,
    /// `<`: pull them one level back.
    Outdent,
    /// `y`: copy the text, leaving it where it is.
    Yank,
}

// ========================================================================
// Grammar
// ========================================================================

/// The count after typing `key` on top of `count` (the digits so far), or
/// `None` when the key is no count digit there. A leading `0` is not a
/// digit but the motion to the line's start; a `0` after other digits
/// extends them. A modified key is never a digit: it is a chord.
pub fn push_count_digit(key: &Key, count: Option<usize>) -> Option<usize> {
    if key.ctrl || key.alt || key.sup {
        return None;
    }
    let KeyCode::Char(c) = key.glyph() else {
        return None;
    };
    let digit = c.to_digit(COUNT_RADIX)? as usize;
    match (count, digit) {
        (None, 0) => None,
        (None, digit) => Some(digit),
        (Some(count), digit) => Some(
            count
                .saturating_mul(COUNT_RADIX as usize)
                .saturating_add(digit)
                .min(MAX_COUNT),
        ),
    }
}

/// The operator `key` opens, if it is one.
pub fn operator_of(key: &Key) -> Option<OperatorKind> {
    match key.glyph() {
        KeyCode::Char('c') => Some(OperatorKind::Change),
        KeyCode::Char('d') => Some(OperatorKind::Delete),
        KeyCode::Char('>') => Some(OperatorKind::Indent),
        KeyCode::Char('<') => Some(OperatorKind::Outdent),
        KeyCode::Char('y') => Some(OperatorKind::Yank),
        _ => None,
    }
}

/// Whether `c` names a register after `"`: a letter (upper case appends),
/// a digit, or one of the [`SYMBOL_REGISTERS`]. The [`BLACK_HOLE_REGISTER`]
/// is named apart, since only a surface that edits text has a use for it.
pub fn is_register_name(c: char) -> bool {
    c.is_ascii_alphanumeric() || SYMBOL_REGISTERS.contains(&c)
}

/// Whether `m` may set a mark named `c`: the letters only, since the other
/// marks are kept on their own.
pub fn is_settable_mark(c: char) -> bool {
    c.is_ascii_lowercase()
}

/// The mark a key names after `'` or `` ` ``, or `None` where it names none.
///
/// A letter is its own mark, the one `m` set. The rest are the marks kept on
/// their own as Vim does, under the name Vim gives them (see
/// [`AUTOMATIC_MARKS`]), and the [`JUMP_MARK`], spelled either `` ` `` or `'`,
/// both of which Vim accepts.
pub fn mark_name(c: char) -> Option<char> {
    match c {
        '`' | JUMP_MARK => Some(JUMP_MARK),
        c if is_settable_mark(c) || AUTOMATIC_MARKS.contains(&c) => Some(c),
        _ => None,
    }
}

/// The value `key` stands for in a command `table` of `(key, value, hint)`.
pub fn table_entry<T: Copy>(table: &[(char, T, &str)], key: &Key) -> Option<T> {
    let KeyCode::Char(c) = key.glyph() else {
        return None;
    };
    table
        .iter()
        .find(|(entry, _, _)| *entry == c)
        .map(|(_, value, _)| *value)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn digit(c: char) -> Key {
        Key::plain(KeyCode::Char(c))
    }

    #[test]
    fn test_a_count_clamps_instead_of_overflowing() {
        // Regression: the editor's count multiplied without a cap, so twenty
        // digits overflowed (a panic in debug builds) and a long count ran a
        // command that many times.
        let mut count = None;
        for _ in 0..25 {
            count = push_count_digit(&digit('9'), count);
        }
        assert_eq!(count, Some(MAX_COUNT));
    }

    #[test]
    fn test_a_leading_zero_is_no_count_but_a_later_one_is() {
        assert_eq!(push_count_digit(&digit('0'), None), None);
        assert_eq!(push_count_digit(&digit('0'), Some(2)), Some(20));
        assert_eq!(
            push_count_digit(&Key::with_ctrl(KeyCode::Char('3')), None),
            None
        );
        assert_eq!(push_count_digit(&digit('x'), Some(2)), None);
    }

    #[test]
    fn test_marks_name_the_jump_either_way_and_the_automatic_ones() {
        assert_eq!(mark_name('`'), Some(JUMP_MARK));
        assert_eq!(mark_name('\''), Some(JUMP_MARK));
        assert_eq!(mark_name('.'), Some('.'));
        assert_eq!(mark_name('q'), Some('q'));
        assert_eq!(mark_name('Q'), None);
        assert!(!is_settable_mark('.'));
    }
}

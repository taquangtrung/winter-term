//! The Vim word and line primitives: pure functions over a row of
//! characters, shared by every Vim surface in Winter — the terminal grid's
//! Normal mode, the dir name editor — as the foundation's text layer.

use super::motion::FindChar;

// ========================================================================
// Word classification
// ========================================================================

/// A character's word class, à la Vim. Blanks (class 0) separate words. With
/// `big` false: keyword runs (alphanumerics and `_`, class 1) are distinct
/// from punctuation runs (class 2). With `big` true: any non-blank is class 1,
/// so only whitespace breaks a WORD. Text objects (`iw`/`aw`/`iW`/`aW`) and the
/// word search classify with this; the motions classify with [`motion_class`]
/// instead.
pub(crate) fn char_class(c: char, big: bool) -> u8 {
    if c == '\0' || c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// The class a *motion* step sees. Punctuation is never worth landing on, so
/// the two sizes differ in what they do with it, not in whether they stop on
/// it: the small motions (`w`/`b`/`e`/`ge`) cross it the way they cross a
/// blank, leaving `foo.bar` two words; the big ones (`W`/`B`/`E`/`gE`) take it
/// into the word beside it, leaving `foo.bar` one WORD.
fn motion_class(c: char, big: bool) -> u8 {
    if big {
        char_class(c, true)
    } else if c.is_alphanumeric() || c == '_' {
        1
    } else {
        0
    }
}

// ========================================================================
// Single-line word motion primitives
// ========================================================================

/// The start column of the next word at or after `col` (Vim `w`/`W`), or `None`
/// when the rest of the line holds no further word. Neither size lands on
/// punctuation (see [`motion_class`]).
pub(crate) fn next_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col;
    let here = line.get(i).map(|c| motion_class(*c, big)).unwrap_or(0);
    if here != 0 {
        while i < line.len() && motion_class(line[i], big) == here {
            i += 1;
        }
    }
    while i < line.len() && motion_class(line[i], big) == 0 {
        i += 1;
    }
    (i < line.len()).then_some(i)
}

/// The start column of the previous word before `col` (Vim `b`/`B`), or `None`
/// when nothing precedes it on the line.
pub(crate) fn prev_word_start(line: &[char], col: usize, big: bool) -> Option<usize> {
    if col == 0 {
        return None;
    }
    let mut i = col - 1;
    while i > 0 && motion_class(line[i], big) == 0 {
        i -= 1;
    }
    if motion_class(line[i], big) == 0 {
        return None;
    }
    let class = motion_class(line[i], big);
    while i > 0 && motion_class(line[i - 1], big) == class {
        i -= 1;
    }
    Some(i)
}

/// The end column of the next word after `col` (Vim `e`/`E`), or `None` when the
/// rest of the line holds no further word. The small motion's word ends on its
/// last word-based character, so `e` over `foo.` stops on the second `o`; the
/// big one has the period inside the word, so `E` stops on it.
pub(crate) fn word_end(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col + 1;
    while i < line.len() && motion_class(line[i], big) == 0 {
        i += 1;
    }
    if i >= line.len() {
        return None;
    }
    let class = motion_class(line[i], big);
    while i + 1 < line.len() && motion_class(line[i + 1], big) == class {
        i += 1;
    }
    Some(i)
}

/// The landing column for a Vim char-search (`f`/`F`/`t`/`T`) on `line` from
/// `col`. `forward` searches right of the cursor, else left; `till` stops one
/// cell short of the match. Returns `None` when there is no match, or when a
/// `till` search would not move (target already adjacent).
pub(crate) fn find_char(line: &[char], col: usize, find: FindChar) -> Option<usize> {
    let FindChar { ch, forward, till } = find;
    let target = if forward {
        (col + 1..line.len()).find(|&i| line[i] == ch)?
    } else {
        (0..col).rev().find(|&i| line[i] == ch)?
    };
    let landing = if !till {
        target
    } else if forward {
        target.checked_sub(1)?
    } else {
        target + 1
    };
    (landing != col).then_some(landing)
}

/// The column of the first non-blank character (Vim `^`), or 0 for a blank line.
pub(crate) fn first_non_blank(line: &[char]) -> usize {
    line.iter()
        .position(|c| char_class(*c, false) != 0)
        .unwrap_or(0)
}

/// The column of the last non-blank character (Vim `g_`), or 0 for a blank line.
pub(crate) fn last_non_blank(line: &[char]) -> usize {
    line.iter()
        .rposition(|c| char_class(*c, false) != 0)
        .unwrap_or(0)
}

/// The end column of the word before `col` (Vim `ge`/`gE`), or `None` when
/// nothing precedes it on the line.
pub(crate) fn prev_word_end(line: &[char], col: usize, big: bool) -> Option<usize> {
    let mut i = col.checked_sub(1)?;
    // Step back off the word the cursor is inside, then over the blanks, and
    // over the punctuation the small motion crosses (see [`motion_class`]).
    let here = motion_class(*line.get(col).unwrap_or(&' '), big);
    if here != 0 {
        while i > 0 && motion_class(line[i], big) == here {
            i -= 1;
        }
        if motion_class(line[i], big) == here {
            return None;
        }
    }
    while motion_class(line[i], big) == 0 {
        i = i.checked_sub(1)?;
    }
    Some(i)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_small_motions_cross_punctuation_the_way_they_cross_a_blank() {
        // f0 o1 o2 .3 b4 a5 r6 _7 b8 a9 z10 ,11 ' '12 q13 u14 x15
        let line: Vec<char> = "foo.bar_baz, qux".chars().collect();

        // `w`: the period and the comma are stepped over, never landed on.
        assert_eq!(next_word_start(&line, 0, false), Some(4), "not the period");
        assert_eq!(next_word_start(&line, 4, false), Some(13), "nor the comma");
        assert_eq!(next_word_start(&line, 13, false), None);

        // `b`: the same boundaries, walked backwards.
        assert_eq!(prev_word_start(&line, 13, false), Some(4));
        assert_eq!(prev_word_start(&line, 4, false), Some(0));
        assert_eq!(prev_word_start(&line, 0, false), None);

        // `e`: a word ends on its last word-based character.
        assert_eq!(word_end(&line, 0, false), Some(2), "the second `o`");
        assert_eq!(word_end(&line, 2, false), Some(10), "the `z`, over the dot");

        // `ge`: back to that end, over the comma.
        assert_eq!(prev_word_end(&line, 13, false), Some(10));
    }

    #[test]
    fn test_the_big_motions_take_punctuation_into_the_word_beside_it() {
        let line: Vec<char> = "foo.bar_baz, qux".chars().collect();

        // `W`/`B`: only a blank breaks a WORD, so the run up to it is one.
        assert_eq!(next_word_start(&line, 0, true), Some(13));
        assert_eq!(next_word_start(&line, 13, true), None);
        assert_eq!(prev_word_start(&line, 13, true), Some(0));

        // `E`/`gE`: the trailing comma is inside the WORD it follows.
        assert_eq!(word_end(&line, 0, true), Some(11), "the comma");
        assert_eq!(prev_word_end(&line, 13, true), Some(11));
    }

    #[test]
    fn test_a_char_search_never_matches_the_cell_it_starts_on() {
        // f0 o1 o2 ' '3 b4 a5 r6 ' '7 f8 o9 o10
        let line: Vec<char> = "foo bar foo".chars().collect();
        let f = |ch, forward, till| FindChar { ch, forward, till };

        // `f` from the first `f` reaches the second, not the one under it.
        assert_eq!(find_char(&line, 0, f('f', true, false)), Some(8));
        // `F` likewise looks strictly left, never at the cursor.
        assert_eq!(find_char(&line, 8, f('f', false, false)), Some(0));
        assert_eq!(find_char(&line, 0, f('z', true, false)), None, "no match");
        assert_eq!(
            find_char(&line, 0, f('o', false, false)),
            None,
            "nothing left"
        );
    }

    #[test]
    fn test_a_till_search_stops_one_short_and_declines_to_stay_put() {
        let line: Vec<char> = "foo bar foo".chars().collect();
        let f = |ch, forward, till| FindChar { ch, forward, till };

        // `t` lands before the target, `T` after it.
        assert_eq!(find_char(&line, 0, f('b', true, true)), Some(3));
        assert_eq!(find_char(&line, 8, f('f', false, true)), Some(1));
        // A `t` onto the cell already occupied is no move, so it is declined
        // rather than reported as a jump that goes nowhere.
        assert_eq!(find_char(&line, 3, f('b', true, true)), None);
        assert_eq!(find_char(&line, 1, f('f', false, true)), None);
    }

    #[test]
    fn test_the_line_ends_skip_the_indent_and_the_trailing_blanks() {
        assert_eq!(first_non_blank(&"   hi".chars().collect::<Vec<_>>()), 3);
        assert_eq!(first_non_blank(&"".chars().collect::<Vec<_>>()), 0);
        assert_eq!(last_non_blank(&"hi   ".chars().collect::<Vec<_>>()), 1);
    }
}

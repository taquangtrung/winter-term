//! The Vim word and line primitives: pure functions over a row of
//! characters, shared by every Vim surface in Winter — the terminal grid's
//! Normal mode, the dir name editor — as the foundation's text layer.

// ========================================================================
// Word classification
// ========================================================================

/// A character's word class, à la Vim. Blanks (class 0) separate words. With
/// `big` false (`w`/`b`/`e`): keyword runs (alphanumerics and `_`, class 1) are
/// distinct from punctuation runs (class 2). With `big` true (`W`/`B`/`E`):
/// any non-blank is class 1, so only whitespace breaks a WORD. Text objects
/// (`iw`/`aw`/`iW`/`aW`) and the word search classify with this; the motions
/// classify with [`motion_class`] instead.
pub(crate) fn char_class(c: char, big: bool) -> u8 {
    if c == '\0' || c.is_whitespace() {
        0
    } else if big || c.is_alphanumeric() || c == '_' {
        1
    } else {
        2
    }
}

/// The class a *motion* step sees. It matches [`char_class`] for the small
/// motions (`w`/`b`/`e`); for the big ones (`W`/`B`/`E`/`gE`) the word-based
/// characters — alphanumerics and `_` — are the only ones worth landing on,
/// so punctuation is crossed the way whitespace is: `W` over `foo .bar`
/// lands on the `b`, never on the `.`.
fn motion_class(c: char, big: bool) -> u8 {
    if !big {
        char_class(c, false)
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
/// when the rest of the line holds no further word. The big motion crosses
/// punctuation along with the whitespace (see [`motion_class`]).
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
/// rest of the line holds no further word. The big motion's word ends on its
/// last word-based character, so `E` over `foo.` stops on the second `o`, not
/// the period.
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
    // Step back off the word the cursor is inside, then over the blanks —
    // and, for the big motion, the punctuation (see [`motion_class`]).
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
    fn test_the_big_motions_land_on_word_based_characters_only() {
        // Punctuation is crossed the way whitespace is: `W` over `foo .bar`
        // lands on the `b`, never the `.`; `E` ends a word on its last
        // word-based character; `B` steps back the same way.
        let line: Vec<char> = "foo .bar_baz, qux".chars().collect();

        assert_eq!(
            next_word_start(&line, 0, true),
            Some(5),
            "the period is skipped"
        );
        assert_eq!(
            next_word_start(&line, 5, true),
            Some(14),
            "the comma with it"
        );

        assert_eq!(
            word_end(&line, 0, true),
            Some(2),
            "the second `o`, not the blank"
        );
        assert_eq!(word_end(&line, 5, true), Some(11), "the `z`, not the comma");

        assert_eq!(prev_word_start(&line, 14, true), Some(5));
        assert_eq!(prev_word_start(&line, 5, true), Some(0));
    }

    #[test]
    fn test_vim_word_motions_on_a_line() {
        // f o o , _ b a r _ b a z _ q u x   (_ = space)
        let line: Vec<char> = "foo, bar_baz qux".chars().collect();

        // `w`: word starts, treating punctuation as its own word.
        assert_eq!(next_word_start(&line, 0, false), Some(3)); // foo -> ','
        assert_eq!(next_word_start(&line, 3, false), Some(5)); // ',' -> 'bar_baz'
        assert_eq!(next_word_start(&line, 5, false), Some(13)); // 'bar_baz' -> 'qux'
        assert_eq!(next_word_start(&line, 13, false), None); // nothing after 'qux'

        // `b`: previous word starts.
        assert_eq!(prev_word_start(&line, 13, false), Some(5));
        assert_eq!(prev_word_start(&line, 5, false), Some(3));
        assert_eq!(prev_word_start(&line, 0, false), None);

        // `e`: word ends.
        assert_eq!(word_end(&line, 0, false), Some(2)); // end of 'foo'
        assert_eq!(word_end(&line, 2, false), Some(3)); // the ',' is a 1-char word
        assert_eq!(word_end(&line, 5, false), Some(11)); // end of 'bar_baz'

        // `^`: first non-blank.
        assert_eq!(first_non_blank(&"   hi".chars().collect::<Vec<_>>()), 3);
        assert_eq!(first_non_blank(&"".chars().collect::<Vec<_>>()), 0);
    }
}

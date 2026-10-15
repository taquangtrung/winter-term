//! The word-level diff that decorates a hunk's changed lines: which stretches
//! of each removed and added line are the edit itself, and which the edit left
//! alone, so a changed line can be painted with its edit standing out rather
//! than the whole line shouting at once.
//!
//! The shape of this follows Magic's diff decorator: each run of removed lines
//! is paired with the added run under it, the pairing is split into groups so
//! reflowed text is compared against the right counterpart, and each group is
//! diffed word by word. The no-newline marker never takes part: it carries no
//! text of either side.

use std::ops::Range;

// ========================================================================
// Constants
// ========================================================================

/// How alike two lines must be, from nothing to exactly alike, before the
/// pairing step treats them as versions of one another rather than unrelated
/// text.
const PAIR_SIMILARITY: f64 = 0.5;

/// The most tokens either side of a pairing may run to before the word diff
/// gives up on it and leaves its lines wholly the edit, the way they painted
/// before word diffs existed. Past this the comparison table stops being worth
/// building on every rebuild, and such a pairing is a rewrite, whose lines are
/// all edit anyway.
const TOKEN_LIMIT: usize = 1024;

// ========================================================================
// Data Structures
// ========================================================================

/// One stretch of a changed line: its text, and whether the word diff counts
/// it as part of the edit rather than as text the edit left alone.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Segment {
    /// Whether the edit touched this stretch.
    pub changed: bool,
    /// The stretch's text, verbatim.
    pub text: String,
}

/// One pairing of removed lines with the added lines they were replaced by,
/// as ranges into each side's run.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Pairing {
    /// Which added lines the pairing covers.
    added: Range<usize>,
    /// Which removed lines the pairing covers.
    removed: Range<usize>,
}

/// Which side of a word diff a run of text belongs to.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Side {
    /// Only in the added text.
    Added,
    /// In both texts.
    Common,
    /// Only in the removed text.
    Removed,
}

/// One run of tokens the word diff treats alike, with adjoining tokens of one
/// kind already joined into one stretch of text.
#[derive(Debug)]
struct Change {
    /// Which side the run belongs to.
    side: Side,
    /// The run's text.
    text: String,
}

/// The class a character falls into when tokenizing, which decides where one
/// token ends and the next begins.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Class {
    /// A character that is neither word nor whitespace, standing alone.
    Other,
    /// Whitespace other than a newline.
    Space,
    /// A word character: a letter, a digit, or the underscore.
    Word,
}

// ========================================================================
// Functions
// ========================================================================

/// Split every line given into its word-level segments, one list per line in
/// the order they came, each segment carrying the line's text after its diff
/// marker. A line the word diff has nothing to say about — context, a
/// deletion with nothing replacing it, a marker — comes back as one segment,
/// wholly the edit, and paints exactly as the whole line always did.
pub fn decorate(lines: &[String]) -> Vec<Vec<Segment>> {
    let mut marked: Vec<Vec<Segment>> =
        lines.iter().map(|line| whole_line(content(line))).collect();
    let mut index = 0;
    while index < lines.len() {
        if !lines[index].starts_with('-') {
            index += 1;
            continue;
        }
        let removed_start = index;
        while index < lines.len() && lines[index].starts_with('-') {
            index += 1;
        }
        let removed_end = index;
        // The no-newline marker can sit between the removed run and the added
        // run under it, so it is stepped over rather than treated as the end
        // of the removed run's pairing.
        while index < lines.len() && lines[index].starts_with('\\') {
            index += 1;
        }
        let added_start = index;
        while index < lines.len() && lines[index].starts_with('+') {
            index += 1;
        }
        let added_end = index;
        if added_start == added_end {
            continue;
        }
        let removed: Vec<&str> = lines[removed_start..removed_end]
            .iter()
            .map(|line| content(line))
            .collect();
        let added: Vec<&str> = lines[added_start..added_end]
            .iter()
            .map(|line| content(line))
            .collect();
        // The added run sits below the removed one, so the two halves the
        // pairings are filled into never overlap and can be borrowed apart.
        let (removed_rows, added_rows) = marked.split_at_mut(added_start);
        for pairing in pairings(&removed, &added) {
            let Pairing {
                added: added_range,
                removed: removed_range,
            } = pairing;
            diff_pairing(
                &removed[removed_range.clone()],
                &added[added_range.clone()],
                &mut removed_rows[removed_start..removed_end][removed_range],
                &mut added_rows[..added_end - added_start][added_range],
            );
        }
    }
    marked
}

/// A line's text after its diff marker, which the marker's own span carries.
fn content(line: &str) -> &str {
    line.get(1..).unwrap_or_default()
}

/// A line the word diff leaves whole: one segment, all of it the edit.
fn whole_line(text: &str) -> Vec<Segment> {
    vec![Segment {
        changed: true,
        text: text.to_string(),
    }]
}

/// Pair a run of removed lines with the added run that replaced it. Equal
/// counts correspond one to one, the common case of a line tweaked in place.
/// Unequal counts mean text reflowed across a different number of lines, so
/// lines are paired where they are alike enough to be versions of one another,
/// and the lines in between join whichever pairing next collects a confident
/// pair rather than being dropped: text that could not be lined up is usually
/// part of a neighboring line's paragraph, not an unrelated change.
fn pairings(removed: &[&str], added: &[&str]) -> Vec<Pairing> {
    if removed.len() == added.len() {
        return (0..removed.len())
            .map(|index| Pairing {
                added: index..index + 1,
                removed: index..index + 1,
            })
            .collect();
    }
    let mut pairings = Vec::new();
    let mut pending_removed = None;
    let mut pending_added = None;
    let mut r = 0;
    let mut a = 0;
    while r < removed.len() && a < added.len() {
        if line_similarity(removed[r], added[a]) >= PAIR_SIMILARITY {
            pairings.push(Pairing {
                removed: pending_removed.unwrap_or(r)..r + 1,
                added: pending_added.unwrap_or(a)..a + 1,
            });
            pending_removed = None;
            pending_added = None;
            r += 1;
            a += 1;
        } else if removed.len() - r > added.len() - a {
            pending_removed = pending_removed.or(Some(r));
            r += 1;
        } else {
            pending_added = pending_added.or(Some(a));
            a += 1;
        }
    }
    while r < removed.len() {
        pending_removed = pending_removed.or(Some(r));
        r += 1;
    }
    while a < added.len() {
        pending_added = pending_added.or(Some(a));
        a += 1;
    }
    match (pending_removed, pending_added) {
        (None, None) => {}
        // Leftovers on one side extend the last pairing, when there is one:
        // a pairing with an empty side has nothing to compare against, so it
        // would report the whole other side as changed even where it is not.
        (Some(start), None) => match pairings.last_mut() {
            Some(last) => last.removed.end = removed.len(),
            None => pairings.push(Pairing {
                removed: start..removed.len(),
                added: added.len()..added.len(),
            }),
        },
        (None, Some(start)) => match pairings.last_mut() {
            Some(last) => last.added.end = added.len(),
            None => pairings.push(Pairing {
                removed: removed.len()..removed.len(),
                added: start..added.len(),
            }),
        },
        // Leftovers on both sides mean nothing matched confidently anywhere,
        // so the whole run is one pairing and is compared as one text.
        (Some(removed_start), Some(added_start)) => pairings.push(Pairing {
            removed: removed_start..removed.len(),
            added: added_start..added.len(),
        }),
    }
    pairings
}

/// How alike two lines are, from nothing to exactly alike: how much of the
/// longer line a shared prefix and a shared suffix cover. Cheap, and enough
/// to tell a tweaked version of a line from an unrelated one.
fn line_similarity(a: &str, b: &str) -> f64 {
    if a == b {
        return 1.0;
    }
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let longest = a.len().max(b.len());
    if longest == 0 {
        return 1.0;
    }
    let shortest = a.len().min(b.len());
    let mut prefix = 0;
    while prefix < shortest && a[prefix] == b[prefix] {
        prefix += 1;
    }
    let mut suffix = 0;
    while suffix < shortest - prefix && a[a.len() - 1 - suffix] == b[b.len() - 1 - suffix] {
        suffix += 1;
    }
    (prefix + suffix) as f64 / longest as f64
}

/// Word-diff one pairing and hand each of its lines its segments. The lines of
/// each side are joined with newlines before diffing, so a paragraph that
/// reflowed across a different number of lines still diffs against the right
/// text, and the diff's runs are split back apart onto the lines they came
/// from. One rule keeps the result readable, learned from Magic's decorator:
/// an edit's own bordering whitespace is trimmed out of the edit, since it is
/// the ordinary separator around preserved text — unless the whitespace is
/// itself the whole edit, an indentation or a spacing change.
fn diff_pairing(
    removed: &[&str],
    added: &[&str],
    out_removed: &mut [Vec<Segment>],
    out_added: &mut [Vec<Segment>],
) {
    for line in out_removed.iter_mut() {
        line.clear();
    }
    for line in out_added.iter_mut() {
        line.clear();
    }
    let old_text = removed.join("\n");
    let new_text = added.join("\n");
    let old = tokenize(&old_text);
    let new = tokenize(&new_text);
    if old.len() > TOKEN_LIMIT || new.len() > TOKEN_LIMIT {
        for (text, line) in removed.iter().zip(out_removed.iter_mut()) {
            *line = whole_line(text);
        }
        for (text, line) in added.iter().zip(out_added.iter_mut()) {
            *line = whole_line(text);
        }
        return;
    }
    let changes = diff_tokens(&old, &new);
    let mut removed_line = 0;
    let mut added_line = 0;
    for (index, change) in changes.iter().enumerate() {
        let previous = index.checked_sub(1).and_then(|p| changes.get(p));
        let following = changes.get(index + 1);
        let removed_run = change.side == Side::Removed;
        let added_run = change.side == Side::Added;
        let whitespace_edit = change.side != Side::Common && is_whitespace(&change.text);
        let previous_common = previous.is_none_or(|p| p.side == Side::Common);
        let following_common = following.is_none_or(|f| f.side == Side::Common);
        let pieces: Vec<&str> = change.text.split('\n').collect();
        let last = pieces.len() - 1;
        for (position, piece) in pieces.iter().enumerate() {
            // Common text advances both sides; removed and added text advance
            // only its own, never both.
            if !added_run && removed_line < out_removed.len() {
                mark_piece(
                    &mut out_removed[removed_line],
                    piece,
                    removed_run,
                    whitespace_edit,
                    position == 0 && previous_common,
                    position == last && following_common,
                );
            }
            if !removed_run && added_line < out_added.len() {
                mark_piece(
                    &mut out_added[added_line],
                    piece,
                    added_run,
                    whitespace_edit,
                    position == 0 && previous_common,
                    position == last && following_common,
                );
            }
            if position != last {
                if !added_run {
                    removed_line += 1;
                }
                if !removed_run {
                    added_line += 1;
                }
            }
        }
    }
}

/// Lay one piece of one change onto the line it belongs to, as the edit or as
/// text the edit left alone. A literal edit's bordering whitespace is trimmed
/// out of the highlight and left as ordinary text; an all-whitespace edit
/// keeps everything, since the whitespace is then the point.
fn mark_piece(
    line: &mut Vec<Segment>,
    piece: &str,
    edit: bool,
    whitespace_edit: bool,
    trim_start: bool,
    trim_end: bool,
) {
    if piece.is_empty() {
        return;
    }
    if !edit {
        push_segment(line, piece, false);
        return;
    }
    let start = if !whitespace_edit && trim_start {
        piece.len() - piece.trim_start().len()
    } else {
        0
    };
    let end = if !whitespace_edit && trim_end {
        piece.trim_end().len()
    } else {
        piece.len()
    };
    if end <= start {
        push_segment(line, piece, false);
        return;
    }
    push_segment(line, &piece[..start], false);
    push_segment(line, &piece[start..end], true);
    push_segment(line, &piece[end..], false);
}

/// Add one stretch to a line's segments, joining it with the previous stretch
/// when they agree on whether the edit touched them.
fn push_segment(line: &mut Vec<Segment>, text: &str, changed: bool) {
    if text.is_empty() {
        return;
    }
    match line.last_mut() {
        Some(last) if last.changed == changed => last.text.push_str(text),
        _ => line.push(Segment {
            changed,
            text: text.to_string(),
        }),
    }
}

/// Split text into the tokens a word diff compares: runs of word characters,
/// runs of whitespace other than a newline, every other character on its own,
/// and each newline as a token of its own, so a token never spans a line
/// break. This matches how jsdiff's word diff with spaces tokenizes, which
/// Magic's decorator diffs with.
fn tokenize(text: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    for (index, fragment) in text.split('\n').enumerate() {
        if index > 0 {
            tokens.push("\n");
        }
        tokens.extend(tokenize_fragment(fragment));
    }
    tokens
}

/// One newline-free stretch of text, as runs of one class with every
/// stand-alone character its own run.
fn tokenize_fragment(fragment: &str) -> Vec<&str> {
    let mut tokens = Vec::new();
    let mut start = 0;
    let mut class = None;
    for (offset, character) in fragment.char_indices() {
        let current = class_of(character);
        match class {
            Some(previous) if previous == current && current != Class::Other => {}
            Some(_) => {
                tokens.push(&fragment[start..offset]);
                start = offset;
            }
            None => {}
        }
        class = Some(current);
    }
    if class.is_some() {
        tokens.push(&fragment[start..]);
    }
    tokens
}

/// The class a character falls into when tokenizing.
fn class_of(character: char) -> Class {
    if is_word(character) {
        Class::Word
    } else if character.is_whitespace() {
        Class::Space
    } else {
        Class::Other
    }
}

/// Whether a character builds a word: letters, digits, and the underscore.
fn is_word(character: char) -> bool {
    character.is_alphanumeric() || character == '_'
}

/// Whether a stretch of text is nothing but whitespace, the runs the trim
/// rules turn on.
fn is_whitespace(text: &str) -> bool {
    !text.is_empty() && text.chars().all(char::is_whitespace)
}

/// Diff two token slices into common, removed, and added runs, with adjoining
/// tokens of one kind joined. A plain longest-common-subsequence walk: a
/// pairing's lines are short, and the token limit keeps the table bounded.
fn diff_tokens(old: &[&str], new: &[&str]) -> Vec<Change> {
    let columns = new.len() + 1;
    let mut lengths = vec![0u32; (old.len() + 1) * columns];
    for i in (0..old.len()).rev() {
        for j in (0..new.len()).rev() {
            lengths[i * columns + j] = if old[i] == new[j] {
                lengths[(i + 1) * columns + j + 1] + 1
            } else {
                lengths[(i + 1) * columns + j].max(lengths[i * columns + j + 1])
            };
        }
    }
    let mut changes = Vec::new();
    let mut i = 0;
    let mut j = 0;
    while i < old.len() && j < new.len() {
        let side = if old[i] == new[j] {
            Side::Common
        } else if lengths[(i + 1) * columns + j] >= lengths[i * columns + j + 1] {
            Side::Removed
        } else {
            Side::Added
        };
        let token = match side {
            Side::Common => {
                let token = old[i];
                i += 1;
                j += 1;
                token
            }
            Side::Removed => {
                let token = old[i];
                i += 1;
                token
            }
            Side::Added => {
                let token = new[j];
                j += 1;
                token
            }
        };
        push_token(&mut changes, side, token);
    }
    while i < old.len() {
        push_token(&mut changes, Side::Removed, old[i]);
        i += 1;
    }
    while j < new.len() {
        push_token(&mut changes, Side::Added, new[j]);
        j += 1;
    }
    changes
}

/// Add one token to the change runs, joining it with the previous run when it
/// belongs to the same side, so a change reads as one stretch of text.
fn push_token(changes: &mut Vec<Change>, side: Side, token: &str) {
    match changes.last_mut() {
        Some(change) if change.side == side => change.text.push_str(token),
        _ => changes.push(Change {
            side,
            text: token.to_string(),
        }),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn to_lines(raw: &[&str]) -> Vec<String> {
        raw.iter().map(|line| line.to_string()).collect()
    }

    fn reassembled(segments: &[Segment]) -> String {
        segments
            .iter()
            .map(|segment| segment.text.as_str())
            .collect()
    }

    fn unchanged(text: &str) -> Segment {
        Segment {
            changed: false,
            text: text.to_string(),
        }
    }

    fn changed(text: &str) -> Segment {
        Segment {
            changed: true,
            text: text.to_string(),
        }
    }

    #[test]
    fn test_only_the_edited_words_of_a_replaced_line_are_marked() {
        let decorated = decorate(&to_lines(&[
            "-    println!(\"old\");",
            "+    println!(\"new\");",
        ]));
        assert_eq!(
            decorated[0],
            vec![
                unchanged("    println!(\""),
                changed("old"),
                unchanged("\");"),
            ]
        );
        assert_eq!(
            decorated[1],
            vec![
                unchanged("    println!(\""),
                changed("new"),
                unchanged("\");"),
            ]
        );
    }

    #[test]
    fn test_a_line_reassembles_from_its_segments_exactly() {
        // The trim rules decide highlighting, never text: a stretch
        // dropped or duplicated there would corrupt the painted line.
        let lines = [
            "-    println!(\"old\");",
            "+    println!(\"new\");",
            "-a  b",
            "+a x  b",
            "-cat end",
            "+dog fin",
            "-one two",
            "-three unmatched",
            "-four five",
            "+one two three",
            "+four five",
        ];
        let decorated = decorate(&to_lines(&lines));
        for (line, segments) in lines.iter().zip(&decorated) {
            assert_eq!(reassembled(segments), &line[1..], "got {segments:?}");
        }
    }

    #[test]
    fn test_an_unmatched_line_joins_the_pairing_of_the_next_confident_match() {
        // Three removed lines reflowing into two added ones: the middle
        // removed line matches nothing one to one, and must still read as
        // removed rather than silently rendering as unchanged.
        let decorated = decorate(&to_lines(&[
            "-one two",
            "-three unmatched",
            "-four five",
            "+one two three",
            "+four five",
        ]));
        assert_eq!(decorated[1], vec![changed("three unmatched")]);
        assert_eq!(decorated[2], vec![unchanged("four five")]);
        assert_eq!(decorated[4], vec![unchanged("four five")]);
    }

    #[test]
    fn test_the_space_between_two_edits_stays_unchanged() {
        // A word changed on each side of one space: the space is text both
        // sides share, and marking it would blur where each edit ends.
        let decorated = decorate(&to_lines(&["-cat end", "+dog fin"]));
        assert_eq!(
            decorated[0],
            vec![changed("cat"), unchanged(" "), changed("end")]
        );
        assert_eq!(
            decorated[1],
            vec![changed("dog"), unchanged(" "), changed("fin")]
        );
    }

    #[test]
    fn test_an_edits_bordering_whitespace_stays_ordinary_text() {
        // The space a removed word took with it is the separator around
        // preserved text, not the edit itself.
        let decorated = decorate(&to_lines(&["-let total count", "+let count"]));
        assert_eq!(
            decorated[0],
            vec![unchanged("let "), changed("total"), unchanged(" count")]
        );
        assert_eq!(decorated[1], vec![unchanged("let count")]);
    }

    #[test]
    fn test_a_whitespace_only_edit_keeps_its_highlight() {
        // A spacing change is the whole edit; trimming its borders would
        // erase it entirely.
        let decorated = decorate(&to_lines(&["-a  b", "+a b"]));
        assert_eq!(
            decorated[0],
            vec![unchanged("a"), changed("  "), unchanged("b")]
        );
        assert_eq!(
            decorated[1],
            vec![unchanged("a"), changed(" "), unchanged("b")]
        );
    }

    #[test]
    fn test_a_run_without_a_counterpart_stays_wholly_changed() {
        let decorated = decorate(&to_lines(&[" keep", "-gone", "+fresh", "+extra"]));
        assert_eq!(decorated[0], vec![changed("keep")]);
        assert_eq!(decorated[1], vec![changed("gone")]);
        assert_eq!(decorated[2], vec![changed("fresh")]);
        assert_eq!(decorated[3], vec![changed("extra")]);
    }

    #[test]
    fn test_the_no_newline_marker_does_not_hide_a_pairing() {
        // The marker sits between the removed line and the added line it
        // pairs with; read as part of the run, it would leave the pair
        // undiffed and both lines wholly highlighted.
        let decorated = decorate(&to_lines(&[
            "-old text",
            "\\ No newline at end of file",
            "+new text",
        ]));
        assert_eq!(decorated[0], vec![changed("old"), unchanged(" text")]);
        assert_eq!(decorated[2], vec![changed("new"), unchanged(" text")]);
    }

    #[test]
    fn test_an_oversized_pairing_falls_back_to_whole_lines() {
        let mut long = String::new();
        for _ in 0..1100 {
            long.push_str("w ");
        }
        let decorated = decorate(&to_lines(&[&format!("-{long}"), &format!("+{long}v")]));
        assert_eq!(decorated[0], vec![changed(&long)]);
        assert_eq!(decorated[1], vec![changed(&format!("{long}v"))]);
    }
}

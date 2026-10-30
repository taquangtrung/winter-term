//! The scanning every extractor shares: a mask of the bytes still kept,
//! the passes that drop more of them, and the small matchers the passes
//! are built from.
//!
//! A pass sees each kept run on its own, so no match spans text an earlier
//! pass dropped. After a match the pass rescans from where it ended, and
//! that point counts as a start of text again, as it does for the regex
//! passes in magic-vscode, which reslice the text after every match.

use std::iter;
use std::ops::Range;

use super::Segment;

// ========================================================================
// Data Structures
// ========================================================================

/// What became of each byte of a text: kept as prose, or dropped either
/// outright or in a way that still parts the words on either side.
pub(super) struct Mask<'a> {
    states: Vec<ByteState>,
    text: &'a str,
}

/// One byte's fate.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ByteState {
    /// Dropped outright: the prose on either side joins, as `{P}rogram`
    /// reads `Program` once its braces go.
    Dropped,
    /// Kept as prose.
    Kept,
    /// Dropped where the source put a space or a placeholder in its place,
    /// so the prose on either side stays apart.
    Spaced,
}

/// A match found by a pass: the byte ranges to drop and where the match
/// ends, both relative to the text the pass scanned.
pub(super) struct Hit {
    drops: Vec<Range<usize>>,
    end: usize,
    is_spaced: bool,
}

/// The prose of a text joined up, as the spell checker reads it, with
/// the way back to the text's own offsets.
pub(crate) struct ProseText {
    /// For each byte of `text`, the offset of the byte it came from; an
    /// inserted space maps to the start of the dropped run it stands for.
    offsets: Vec<usize>,
    /// The kept bytes in order, a space where dropped text parted words.
    text: String,
}

// ========================================================================
// Mask
// ========================================================================

impl<'a> Mask<'a> {
    /// Every byte of `text` kept, for markup that passes then thin out.
    pub(super) fn kept_all(text: &'a str) -> Self {
        Self {
            states: vec![ByteState::Kept; text.len()],
            text,
        }
    }

    /// No byte of `text` kept, for code whose comments are then kept; the
    /// code between two comments parts them.
    pub(super) fn kept_none(text: &'a str) -> Self {
        Self {
            states: vec![ByteState::Spaced; text.len()],
            text,
        }
    }

    /// Keeps the bytes of `range`.
    pub(super) fn keep(&mut self, range: Range<usize>) {
        self.states[range].fill(ByteState::Kept);
    }

    /// Runs one pass: within each kept run, finds the first position where
    /// `rule` matches, drops what the hit says, and rescans from its end.
    /// `rule` gets the rest of the run and a position in it.
    pub(super) fn apply(&mut self, rule: impl Fn(&str, usize) -> Option<Hit>) {
        let text = self.text;
        for run in self.runs() {
            let mut from = run.start;
            while let Some(hay) = text.get(from..run.end) {
                let Some((at, hit)) = first_hit(hay, &rule) else {
                    break;
                };
                let state = if hit.is_spaced {
                    ByteState::Spaced
                } else {
                    ByteState::Dropped
                };
                for drop in hit.drops {
                    self.states[from + drop.start..from + drop.end].fill(state);
                }
                // Every rule consumes at least the byte it matched at.
                from += hit.end.max(at + 1);
            }
        }
    }

    /// The kept runs, in order.
    pub(super) fn segments(&self) -> Vec<Segment> {
        self.runs()
            .into_iter()
            .map(|run| Segment {
                end: run.end,
                start: run.start,
            })
            .collect()
    }

    /// The kept text joined up, one space standing for each dropped run
    /// that parts words.
    pub(super) fn prose_text(&self) -> ProseText {
        let mut offsets = Vec::with_capacity(self.text.len());
        let mut text = String::with_capacity(self.text.len());
        let mut cursor = 0;
        for run in self.runs() {
            let gap = &self.states[cursor..run.start];
            if gap.contains(&ByteState::Spaced) && !text.is_empty() {
                offsets.push(cursor);
                text.push(' ');
            }
            offsets.extend(run.clone());
            text.push_str(&self.text[run.clone()]);
            cursor = run.end;
        }
        ProseText { offsets, text }
    }

    /// The maximal runs of kept bytes.
    fn runs(&self) -> Vec<Range<usize>> {
        let mut runs = Vec::new();
        let mut start = None;
        for (index, &state) in self.states.iter().enumerate() {
            match (state == ByteState::Kept, start) {
                (true, None) => start = Some(index),
                (false, Some(run_start)) => {
                    runs.push(run_start..index);
                    start = None;
                }
                (true, Some(_)) | (false, None) => {}
            }
        }
        if let Some(run_start) = start {
            runs.push(run_start..self.states.len());
        }
        runs
    }
}

/// The first position in `hay` where `rule` matches, with its hit.
fn first_hit(hay: &str, rule: &impl Fn(&str, usize) -> Option<Hit>) -> Option<(usize, Hit)> {
    hay.char_indices()
        .find_map(|(at, _)| rule(hay, at).map(|hit| (at, hit)))
}

// ========================================================================
// Hit
// ========================================================================

impl Hit {
    /// A match ending at `end` that drops exactly `drops`.
    pub(super) fn new(drops: impl IntoIterator<Item = Range<usize>>, end: usize) -> Self {
        Self {
            drops: drops.into_iter().collect(),
            end,
            is_spaced: false,
        }
    }

    /// A match dropped whole.
    pub(super) fn dropping(range: Range<usize>) -> Self {
        Self::new(iter::once(range.clone()), range.end)
    }

    /// A match dropped except for `keeps`, which are in order and inside
    /// it, as when markup around prose goes and the prose stays.
    pub(super) fn sparing(
        range: Range<usize>,
        keeps: impl IntoIterator<Item = Range<usize>>,
    ) -> Self {
        let mut drops = Vec::new();
        let mut cursor = range.start;
        for keep in keeps {
            drops.push(cursor..keep.start);
            cursor = keep.end;
        }
        drops.push(cursor..range.end);
        drops.retain(|drop| !drop.is_empty());
        Self::new(drops, range.end)
    }

    /// This hit with its dropped text parting the words around it, where
    /// the source put a space or a placeholder in its place.
    pub(super) fn spaced(self) -> Self {
        Self {
            is_spaced: true,
            ..self
        }
    }
}

// ========================================================================
// ProseText
// ========================================================================

impl ProseText {
    /// The joined prose.
    pub(crate) fn text(&self) -> &str {
        &self.text
    }

    /// The range of the original text that `range`, a nonempty range of
    /// the joined prose, came from.
    pub(crate) fn original_range(&self, range: Range<usize>) -> Range<usize> {
        self.offsets[range.start]..self.offsets[range.end - 1] + 1
    }
}

// ========================================================================
// Matchers
// ========================================================================

/// Whether `c` is a regex word character, ASCII only as in JavaScript.
pub(super) fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The character just before `at`, if any.
pub(super) fn char_before(hay: &str, at: usize) -> Option<char> {
    hay[..at].chars().next_back()
}

/// Whether `at` opens the text or follows a non-word character, the
/// `(^|(?<=\W))` guard of the source regexes.
pub(super) fn is_after_non_word(hay: &str, at: usize) -> bool {
    char_before(hay, at).is_none_or(|c| !is_word_char(c))
}

/// Whether the text ends at `at` or a non-word character follows, the
/// `((?=\W)|$)` guard of the source regexes.
pub(super) fn is_before_non_word(hay: &str, at: usize) -> bool {
    hay[at..].chars().next().is_none_or(|c| !is_word_char(c))
}

/// Whether the byte at `at` is `byte`.
pub(super) fn is_byte_at(hay: &str, at: usize, byte: u8) -> bool {
    hay.as_bytes().get(at) == Some(&byte)
}

/// The first `needle` at or after `at`.
pub(super) fn find_from(hay: &str, at: usize, needle: &str) -> Option<usize> {
    hay.get(at..)?.find(needle).map(|index| at + index)
}

/// Where the line holding `at` ends, at its line break or the text's end.
pub(super) fn line_end(hay: &str, at: usize) -> usize {
    find_from(hay, at, "\n").unwrap_or(hay.len())
}

/// Past the whitespace starting at `at`.
pub(super) fn whitespace_end(hay: &str, at: usize) -> usize {
    hay[at..]
        .char_indices()
        .find(|&(_, c)| !c.is_whitespace())
        .map_or(hay.len(), |(index, _)| at + index)
}

/// Past the ASCII letters starting at `at`.
pub(super) fn letters_end(hay: &str, at: usize) -> usize {
    hay[at..]
        .bytes()
        .position(|byte| !byte.is_ascii_alphabetic())
        .map_or(hay.len(), |index| at + index)
}

/// Past an identifier at `at`: a letter, then letters and digits.
pub(super) fn identifier_end(hay: &str, at: usize) -> Option<usize> {
    if !hay.as_bytes().get(at)?.is_ascii_alphabetic() {
        return None;
    }
    let end = hay[at..]
        .bytes()
        .position(|byte| !byte.is_ascii_alphanumeric())
        .map_or(hay.len(), |index| at + index);
    Some(end)
}

/// Past a group opened by `open` at `at` and closed by the first `close`
/// after it, as `\{[^}]*\}` matches.
pub(super) fn group_end(hay: &str, at: usize, open: u8, close: u8) -> Option<usize> {
    if !is_byte_at(hay, at, open) {
        return None;
    }
    let close_index = hay.as_bytes()[at + 1..]
        .iter()
        .position(|&byte| byte == close)?;
    Some(at + 1 + close_index + 1)
}

/// Past `$...$` math opening at `at`, closed on the same line by a `$`
/// that ends the text or precedes a non-word character.
pub(super) fn inline_math_end(hay: &str, at: usize) -> Option<usize> {
    if !is_byte_at(hay, at, b'$') || !is_after_non_word(hay, at) {
        return None;
    }
    let end = line_end(hay, at);
    hay[at + 1..end]
        .match_indices('$')
        .map(|(index, _)| at + 1 + index + 1)
        .find(|&close_end| is_before_non_word(hay, close_end))
}

/// Past a bare `http://` or `https://` URL starting a word at `at`; it
/// runs to whitespace or a closing parenthesis.
pub(super) fn url_end(hay: &str, at: usize) -> Option<usize> {
    if !is_after_non_word(hay, at) {
        return None;
    }
    let rest = &hay[at..];
    let scheme = ["https://", "http://"]
        .into_iter()
        .find(|scheme| rest.starts_with(scheme))?;
    let body_start = at + scheme.len();
    let end = hay[body_start..]
        .char_indices()
        .find(|&(_, c)| c.is_whitespace() || c == ')')
        .map_or(hay.len(), |(index, _)| body_start + index);
    (end > body_start).then_some(end)
}

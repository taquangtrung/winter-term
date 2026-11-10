//! Highlights: phrases lit in colors of their own wherever they stand in the
//! file, as Emacs' hi-lock marks them, and the sentence wash the window's
//! `View: Toggle Sentence Highlight` turns on for every pane, laying each
//! sentence on a faint band of the two tones a terminal's sentences take, so
//! where one ends and the next starts reads at a glance. Both last for the
//! session; neither is written to the file.

use std::cell::RefCell;

use winter_model::editor::{EditorOutcome, Notice};
use winter_model::paint::Style;

use super::{markdown, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// How many highlight colors there are before they are used again.
const HIGHLIGHT_COLORS: u8 = 10;

/// How many tones the sentences alternate between, as a terminal's do.
const SENTENCE_TONES: u8 = 2;

/// What ends a sentence, before the space after it.
const SENTENCE_ENDS: [char; 3] = ['.', '!', '?'];

/// What may close a sentence after its end mark: `"Yes."` and `(see above.)`.
const SENTENCE_CLOSERS: [char; 4] = ['"', '\'', ')', ']'];

// ========================================================================
// Data Structures
// ========================================================================

/// One file's highlights.
#[derive(Debug, Default)]
pub(super) struct Highlights {
    /// The phrases lit, each with its color.
    phrases: Vec<Phrase>,
    /// Whether the sentences are washed, as the window says they are.
    sentences: bool,
    /// Where each line's sentences run, and the revision of the text they
    /// were found in: worked out again only when the text changes, since the
    /// paint asks on every frame.
    washed: RefCell<Option<(usize, Vec<Vec<Band>>)>>,
}

/// A phrase lit wherever it stands.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Phrase {
    /// Which highlight color it takes.
    color: u8,
    text: String,
}

/// Columns of a line on one band: from `start` up to `end`, in `style`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Band {
    pub(super) end: usize,
    pub(super) start: usize,
    pub(super) style: Style,
}

// ========================================================================
// Highlights Implementations
// ========================================================================

impl Highlights {
    /// Light `text` in the first color no phrase has, or the next in turn
    /// once all are taken. Says whether it was lit: a phrase lit already is
    /// not lit twice.
    fn light(&mut self, text: &str) -> bool {
        if self.phrases.iter().any(|phrase| phrase.text == text) {
            return false;
        }
        let color = (0..HIGHLIGHT_COLORS)
            .find(|color| self.phrases.iter().all(|phrase| phrase.color != *color))
            .unwrap_or(self.phrases.len() as u8 % HIGHLIGHT_COLORS);
        self.phrases.push(Phrase {
            color,
            text: text.to_string(),
        });
        true
    }

    /// The bands of line `index` of `lines`, the text at `revision`: each lit
    /// phrase where it stands, over the sentence wash where that is on.
    pub(super) fn bands(&self, lines: &[String], revision: usize, index: usize) -> Vec<Band> {
        let mut bands = phrase_bands(&self.phrases, &lines[index]);
        if self.sentences {
            let mut washed = self.washed.borrow_mut();
            if washed.as_ref().is_none_or(|(at, _)| *at != revision) {
                *washed = Some((revision, sentence_bands(lines)));
            }
            if let Some((_, by_line)) = washed.as_ref() {
                bands.extend(by_line.get(index).into_iter().flatten().copied());
            }
        }
        bands
    }
}

// ========================================================================
// TextEditor: highlights
// ========================================================================

impl TextEditor {
    /// Light every occurrence of the selection, or of the word under the
    /// cursor, in a color of its own.
    pub(super) fn highlight_text(&mut self) -> EditorOutcome {
        let row = self.buffer().row();
        let line: Vec<char> = self.buffer().lines()[row].chars().collect();
        let range = match self.selection() {
            Some(selection)
                if selection.start.0 == selection.end.0
                    && !selection.linewise
                    && !selection.block =>
            {
                Some((selection.start.1, (selection.end.1 + 1).min(line.len())))
            }
            Some(_) => return self.report("select within one line"),
            None => markdown::word_at(&self.buffer().lines()[row], self.buffer().col()),
        };
        let Some((start, end)) = range.filter(|(start, end)| start < end) else {
            return self.report("no word under the cursor");
        };
        let text: String = line[start..end].iter().collect();
        self.leave_to_normal();
        match self.doc_mut().highlights.light(&text) {
            true => EditorOutcome::Consumed,
            false => self.report(&format!("\"{text}\" is highlighted already")),
        }
    }

    /// Put out the phrase lit under the cursor.
    pub(super) fn delete_highlight(&mut self) -> EditorOutcome {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let line = self.buffer().lines()[row].clone();
        let phrases = &self.doc().highlights.phrases;
        let under = phrases.iter().position(|phrase| {
            occurrences(&line, &phrase.text).any(|(start, end)| (start..end).contains(&col))
        });
        match under {
            Some(at) => {
                self.doc_mut().highlights.phrases.remove(at);
                EditorOutcome::Consumed
            }
            None => self.report("no highlight under the cursor"),
        }
    }

    /// Put out every phrase lit in the file.
    pub(super) fn delete_all_highlights(&mut self) -> EditorOutcome {
        let count = std::mem::take(&mut self.doc_mut().highlights.phrases).len();
        let said = match count {
            0 => "nothing is highlighted".to_string(),
            1 => "1 highlight put out".to_string(),
            count => format!("{count} highlights put out"),
        };
        self.modal.report(Notice::info(said));
        EditorOutcome::Consumed
    }

    /// Wash every open file's sentences, or stop, as the window's sentence
    /// highlight says, before each paint.
    pub fn set_sentence_wash(&mut self, on: bool) {
        for doc in &mut self.docs {
            doc.highlights.sentences = on;
        }
    }

    /// The bands line `index` is painted on.
    pub(super) fn highlight_bands(&self, index: usize) -> Vec<Band> {
        let doc = self.doc();
        doc.highlights
            .bands(doc.buffer.lines(), doc.buffer.revision(), index)
    }

    /// Say why the key did nothing, and nothing else.
    fn report(&mut self, text: &str) -> EditorOutcome {
        self.modal.report(Notice::refusal(text));
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Where each of `phrases` stands in `line`, on its color's band.
fn phrase_bands(phrases: &[Phrase], line: &str) -> Vec<Band> {
    phrases
        .iter()
        .flat_map(|phrase| {
            occurrences(line, &phrase.text).map(|(start, end)| Band {
                end,
                start,
                style: Style::Highlight(phrase.color),
            })
        })
        .collect()
}

/// The column ranges `text` stands at in `line`, as it is written: case
/// counts, and two occurrences never overlap.
fn occurrences<'a>(line: &'a str, text: &'a str) -> impl Iterator<Item = (usize, usize)> + 'a {
    let width = text.chars().count();
    line.match_indices(text)
        .filter(move |_| width > 0)
        .map(move |(at, _)| {
            let start = line[..at].chars().count();
            (start, start + width)
        })
}

/// Each line's sentences, as bands of their turn's color. A sentence runs
/// from its first character to the mark that ends it (`.`, `!`, `?`, with
/// any quote or bracket that closes after it) where a space or the line's
/// end follows; a blank line ends one too, as a paragraph does.
fn sentence_bands(lines: &[String]) -> Vec<Vec<Band>> {
    let mut by_line = vec![Vec::new(); lines.len()];
    let mut turn: u8 = 0;
    let mut open = false;
    for (index, line) in lines.iter().enumerate() {
        let chars: Vec<char> = line.chars().collect();
        if chars.iter().all(|c| c.is_whitespace()) {
            if open {
                turn = (turn + 1) % SENTENCE_TONES;
                open = false;
            }
            continue;
        }
        let mut start = match open {
            true => 0,
            false => chars.iter().position(|c| !c.is_whitespace()).unwrap_or(0),
        };
        let mut at = start;
        while at < chars.len() {
            if SENTENCE_ENDS.contains(&chars[at]) {
                let mut end = at + 1;
                while chars.get(end).is_some_and(|c| SENTENCE_CLOSERS.contains(c)) {
                    end += 1;
                }
                if chars.get(end).is_none_or(|c| c.is_whitespace()) {
                    by_line[index].push(Band {
                        end,
                        start,
                        style: Style::Sentence(turn),
                    });
                    turn = (turn + 1) % SENTENCE_TONES;
                    open = false;
                    start = chars[end..]
                        .iter()
                        .position(|c| !c.is_whitespace())
                        .map_or(chars.len(), |gap| end + gap);
                    at = start;
                    continue;
                }
            }
            at += 1;
        }
        if start < chars.len() {
            by_line[index].push(Band {
                end: chars.len(),
                start,
                style: Style::Sentence(turn),
            });
            open = true;
        }
    }
    by_line
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &[&str]) -> Vec<String> {
        text.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn test_each_phrase_takes_a_color_of_its_own_and_is_lit_once() {
        let mut highlights = Highlights::default();
        assert!(highlights.light("foo"));
        assert!(highlights.light("bar"));
        assert!(!highlights.light("foo"), "lit already");
        let bands = phrase_bands(&highlights.phrases, "foo and bar, foo");
        let at: Vec<(usize, usize, Style)> = bands
            .iter()
            .map(|band| (band.start, band.end, band.style))
            .collect();
        assert_eq!(
            at,
            [
                (0, 3, Style::Highlight(0)),
                (13, 16, Style::Highlight(0)),
                (8, 11, Style::Highlight(1))
            ]
        );
        assert!(
            phrase_bands(&highlights.phrases, "Foo").is_empty(),
            "as it is written"
        );
    }

    #[test]
    fn test_a_freed_color_is_the_next_one_taken() {
        let mut highlights = Highlights::default();
        highlights.light("a");
        highlights.light("b");
        highlights.phrases.remove(0);
        highlights.light("c");
        assert_eq!(
            highlights.phrases[1].color, 0,
            "the color `a` had, not a third"
        );
    }

    #[test]
    fn test_sentences_run_across_lines_and_end_at_their_mark_or_a_blank_line() {
        // Washed one line at a time, a sentence broken over two lines read
        // as two.
        let text = lines(&["One. Two runs", "on here! (Three.)", "", "Four"]);
        let bands = sentence_bands(&text);
        let spans = |row: usize| -> Vec<(usize, usize, Style)> {
            bands[row]
                .iter()
                .map(|band| (band.start, band.end, band.style))
                .collect()
        };
        assert_eq!(
            spans(0),
            [(0, 4, Style::Sentence(0)), (5, 13, Style::Sentence(1))]
        );
        assert_eq!(
            spans(1),
            [(0, 8, Style::Sentence(1)), (9, 17, Style::Sentence(0))]
        );
        assert!(spans(2).is_empty());
        assert_eq!(
            spans(3),
            [(0, 4, Style::Sentence(1))],
            "the blank line ended nothing open"
        );
        assert!(
            sentence_bands(&lines(&["e.g.x"]))[0].len() == 1,
            "no space after the mark, no end"
        );
    }
}

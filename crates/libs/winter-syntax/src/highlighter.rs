//! The grammars as a Viento [`Highlighter`]: a document's lines walked top
//! down a budget at a time, as a line's colors depend on every line above
//! it, each line's runs kept until an edit above or in it makes them
//! untrue. A run's [`Style`] is numbered as a [`HighlightId`] the window
//! draws it by ([`highlight_styles`]); the numbers are the process's, the
//! same for every document.
//!
//! A line's state holds the grammar engine's match registers, which may
//! not cross threads, while Viento's highlighters must be able to: the
//! states are kept on the thread that colors, by the highlighter's number,
//! and a highlighter moved to another thread starts its walk over there.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, PoisonError};

use viento::{ChangeSet, Document, HighlightId, HighlightSpan, Highlighter, Position, Preparation};
use winter_model::paint::Style;

use super::{highlight, next_state, Grammar, LineState};

// ========================================================================
// Constants
// ========================================================================

/// The styles numbered so far, each id its index.
static NUMBERED: Mutex<Vec<Style>> = Mutex::new(Vec::new());

/// The number the next highlighter's walk is kept under.
static NEXT_WALK: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Where each line walked so far begins, by highlighter, from the top:
    /// a walk's `[i]` is what the lines above line `i` left open.
    static WALKS: RefCell<HashMap<u64, Vec<LineState>>> = RefCell::new(HashMap::new());
}

// ========================================================================
// Data Structures
// ========================================================================

/// A line's runs: their chars from the line's start, and their style's
/// number.
type LineRuns = Vec<(Range<usize>, HighlightId)>;

/// One document's coloring by its grammar.
#[derive(Debug)]
pub struct SyntaxHighlighter {
    /// The grammar the document is read with.
    grammar: Grammar,
    /// Each line's runs worked out so far, from the top: its chars from
    /// the line's start, and their style's number.
    runs: Vec<Option<LineRuns>>,
    /// The number its walk is kept under on the coloring thread.
    walk: u64,
}

// ========================================================================
// SyntaxHighlighter Implementations
// ========================================================================

impl SyntaxHighlighter {
    /// A highlighter reading its document with `grammar`, nothing walked.
    pub fn new(grammar: Grammar) -> Self {
        Self {
            grammar,
            runs: Vec::new(),
            walk: NEXT_WALK.fetch_add(1, Ordering::Relaxed),
        }
    }

    /// `act` on the states of this highlighter's walk on this thread, the
    /// first line's at least.
    fn with_states<R>(&self, act: impl FnOnce(&mut Vec<LineState>) -> R) -> R {
        WALKS.with(|walks| {
            let mut walks = walks.borrow_mut();
            let states = walks.entry(self.walk).or_default();
            if states.is_empty() {
                states.push(LineState::start(self.grammar));
            }
            act(states)
        })
    }

    /// The runs of line `index` of `document`, working them out when the
    /// state it begins in is known and they are not yet.
    fn color_line(&mut self, document: &Document, index: usize) {
        if self.runs.get(index).is_some_and(Option::is_some) {
            return;
        }
        let Some(line) = document.line_text(index) else {
            return;
        };
        let Some(styles) = self.with_states(|states| Some(highlight(&line, states.get(index)?).0))
        else {
            return;
        };
        if self.runs.len() <= index {
            self.runs.resize(index + 1, None);
        }
        self.runs[index] = Some(runs_of(&styles));
    }
}

impl Highlighter for SyntaxHighlighter {
    fn edit(&mut self, changes: &ChangeSet, document: &Document) {
        let Some(first) = changes.first_change() else {
            return;
        };
        let line = document
            .char_to_position(first.min(document.len_chars()))
            .map_or(0, |at| at.line);
        self.with_states(|states| states.truncate(line + 1));
        self.runs.truncate(line);
    }

    fn prepare(&mut self, document: &Document, range: Range<usize>, lines: usize) -> Preparation {
        let line_of = |offset: usize| {
            document
                .char_to_position(offset.min(document.len_chars()))
                .map_or(0, |at| at.line)
        };
        let (top, bottom) = (line_of(range.start), line_of(range.end));
        let mut budget = lines;
        // The lines above the ones asked for are walked for their state
        // alone; theirs are worked out when they come into sight.
        let walked = self.with_states(|states| {
            while states.len() <= bottom && budget > 0 {
                let index = states.len() - 1;
                let Some(line) = document.line_text(index) else {
                    break;
                };
                let next = next_state(&line, &states[index]);
                states.push(next);
                budget -= 1;
            }
            states.len()
        });
        for index in top..=bottom {
            if budget == 0 {
                break;
            }
            if self.runs.get(index).is_none_or(Option::is_none) && index < walked {
                self.color_line(document, index);
                budget -= 1;
            }
        }
        let done = (top..=bottom.min(document.len_lines().saturating_sub(1)))
            .all(|index| self.runs.get(index).is_some_and(Option::is_some));
        match done {
            true => Preparation::Ready,
            false => Preparation::Pending,
        }
    }

    fn highlights(&self, document: &Document, range: Range<usize>) -> Vec<HighlightSpan> {
        let line_of = |offset: usize| {
            document
                .char_to_position(offset.min(document.len_chars()))
                .map_or(0, |at| at.line)
        };
        let mut spans = Vec::new();
        for line in line_of(range.start)..=line_of(range.end) {
            let Some(Some(runs)) = self.runs.get(line) else {
                continue;
            };
            let Some(start) = document.position_to_char(Position { column: 0, line }) else {
                continue;
            };
            for (run, id) in runs {
                let chars = start + run.start..start + run.end;
                if chars.start < range.end && chars.end > range.start {
                    spans.push(HighlightSpan::new(chars, *id));
                }
            }
        }
        spans
    }
}

impl Drop for SyntaxHighlighter {
    fn drop(&mut self) {
        // A walk kept on another thread is left there; it is only memory.
        let _ = WALKS.try_with(|walks| walks.borrow_mut().remove(&self.walk));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The style each number stands for, numbers in order: what the window
/// resolves against its theme for the view to draw. Numbers are only ever
/// added, so a view handed these draws every id it has seen so far.
pub fn highlight_styles() -> Vec<Style> {
    NUMBERED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// The number `style` is drawn by, numbering it if it has none yet: a
/// run's, or any other text a view shows in one of the window's styles (a
/// gutter's marks, a diff's lines).
pub fn highlight_id(style: Style) -> HighlightId {
    let mut numbered = NUMBERED.lock().unwrap_or_else(PoisonError::into_inner);
    let index = match numbered.iter().position(|known| *known == style) {
        Some(index) => index,
        None => {
            numbered.push(style);
            numbered.len() - 1
        }
    };
    HighlightId::new(u16::try_from(index).unwrap_or(u16::MAX))
}

/// The runs of a line whose chars are styled `styles`: each stretch of one
/// style other than plain text, numbered.
fn runs_of(styles: &[Style]) -> LineRuns {
    let mut runs = Vec::new();
    let mut start = 0;
    while start < styles.len() {
        let style = styles[start];
        let end = styles[start..]
            .iter()
            .position(|next| *next != style)
            .map_or(styles.len(), |length| start + length);
        if style != Style::Normal {
            runs.push((start..end, highlight_id(style)));
        }
        start = end;
    }
    runs
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use viento::Edit;

    use super::*;

    /// The style each char of `line` of `document` is drawn in, by
    /// `highlighter`, plain where it names none.
    fn styles_of(highlighter: &SyntaxHighlighter, document: &Document, line: usize) -> Vec<Style> {
        let start = document
            .position_to_char(Position { column: 0, line })
            .expect("the line");
        let text = document.line_text(line).expect("the line");
        let numbered = highlight_styles();
        let mut styles = vec![Style::Normal; text.chars().count()];
        for span in highlighter.highlights(document, start..start + styles.len()) {
            for offset in span.range {
                styles[offset - start] = numbered[usize::from(span.highlight.index())];
            }
        }
        styles
    }

    /// The styles the grammar gives `line` read after `above`, as Winter
    /// colors a line today.
    fn walked(grammar: Grammar, above: &[&str], line: &str) -> Vec<Style> {
        let mut state = LineState::start(grammar);
        for line in above {
            state = next_state(line, &state);
        }
        highlight(line, &state).0
    }

    #[test]
    fn test_a_prepared_line_takes_the_styles_the_grammar_walks_it_in() {
        let grammar = crate::named("Rust").expect("the Rust grammar");
        let document = Document::new("/* open\nstill */ fn x() {}\n");
        let mut highlighter = SyntaxHighlighter::new(grammar);
        let all = 0..document.len_chars();
        assert_eq!(highlighter.prepare(&document, all, 10), Preparation::Ready);
        assert_eq!(
            styles_of(&highlighter, &document, 1),
            walked(grammar, &["/* open"], "still */ fn x() {}"),
            "the comment's end, then code"
        );
    }

    #[test]
    fn test_a_small_budget_leaves_work_pending_until_the_lines_are_walked() {
        let grammar = crate::named("Rust").expect("the Rust grammar");
        let text = "fn a() {}\n".repeat(20);
        let document = Document::new(&text);
        let mut highlighter = SyntaxHighlighter::new(grammar);
        let last = document
            .position_to_char(Position {
                column: 0,
                line: 19,
            })
            .expect("line 19");
        let mut rounds = 0;
        while highlighter.prepare(&document, last..last + 9, 4) == Preparation::Pending {
            rounds += 1;
            assert!(rounds < 10, "work runs out");
        }
        assert!(rounds >= 4, "19 lines above walked 4 at a time");
        assert_eq!(
            styles_of(&highlighter, &document, 19),
            walked(grammar, &[], "fn a() {}")
        );
    }

    #[test]
    fn test_an_edit_forgets_the_lines_from_the_one_it_touched() {
        let grammar = crate::named("Rust").expect("the Rust grammar");
        let document = Document::new("let a = 1;\nlet b = 2;\n");
        let mut highlighter = SyntaxHighlighter::new(grammar);
        highlighter.prepare(&document, 0..document.len_chars(), 10);
        let edit = Edit::new(0..0, "/*");
        let changes = ChangeSet::from_edits(document.len_chars(), [edit]).expect("a change");
        let mut opened = document.clone();
        changes.apply(&mut opened).expect("applies");
        highlighter.edit(&changes, &opened);
        highlighter.prepare(&opened, 0..opened.len_chars(), 10);
        assert_eq!(
            styles_of(&highlighter, &opened, 1),
            walked(grammar, &["/*let a = 1;"], "let b = 2;"),
            "line 1 is in the comment now"
        );
    }
}

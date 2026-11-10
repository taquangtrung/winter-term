//! Peek, as VS Code's: where a name is defined (`M-F12`), or every place it
//! is used (`S-M-F12`), shown in a card under the cursor, a few lines of it
//! with the place marked, without leaving the file. `RET` goes there,
//! `M-n` and `M-p` step through several, and any other key puts the card
//! away and does what it does.

use std::path::Path;

use winter_lsp::protocol::{self, Location};
use winter_model::editor::{EditorOutcome, Notice};
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::input::Key;
use winter_model::paint::Style;
use winter_syntax::{Grammar, LineState};

use super::lsp::Place;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// How many lines a peek shows above the place, and below it.
const ABOVE: usize = 2;
const BELOW: usize = 8;

/// What marks the place's line, and every other.
const MARK: &str = "> ";
const UNMARKED: &str = "  ";

/// The keys a peek's card answers while it shows; any other puts it away.
pub(crate) const PEEK_KEYS: KeyTable<PeekAct> = KeyTable {
    block: "editor.peek",
    bindings: &[
        Binding {
            action: PeekAct::Go,
            id: "go",
            keys: &["RET"],
            label: "Go to the place the peek shows",
        },
        Binding {
            action: PeekAct::Next,
            id: "next",
            keys: &["M-n"],
            label: "The next place",
        },
        Binding {
            action: PeekAct::Previous,
            id: "previous",
            keys: &["M-p"],
            label: "The place before",
        },
    ],
    configurable: true,
    title: "Peek",
};

// ========================================================================
// Data Structures
// ========================================================================

/// What a key does to a peek.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PeekAct {
    Go,
    Next,
    Previous,
}

/// The places a peek is showing, and which of them.
#[derive(Debug, Default)]
pub(super) struct Peek {
    at: usize,
    places: Vec<Place>,
}

// ========================================================================
// TextEditor: peeking
// ========================================================================

impl TextEditor {
    /// Show `found` in a card, the first of them, or say there is none of
    /// `what`.
    pub(super) fn take_peek(&mut self, what: &str, found: &[Location]) -> EditorOutcome {
        if found.is_empty() {
            self.modal.report(Notice::info(format!("no {what} found")));
            return EditorOutcome::Consumed;
        }
        let places = found
            .iter()
            .map(|location| Place {
                at: location.range.start,
                path: location.path.clone(),
            })
            .collect();
        self.peek = Some(Peek { at: 0, places });
        self.show_peek();
        EditorOutcome::Consumed
    }

    /// What a key does while a peek shows: `RET` goes to the place, `M-n`
    /// and `M-p` step between places. `None` for any other key, which puts
    /// the peek away.
    pub(super) fn peek_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        if self.peek.is_some() && !self.has_card() {
            self.peek = None;
        }
        let peek = self.peek.as_mut()?;
        let count = peek.places.len();
        match table::find(&PEEK_KEYS, key) {
            Some(PeekAct::Go) => {
                let place = peek.places[peek.at].clone();
                self.peek = None;
                self.close_card();
                self.go_to(&place, true);
                Some(EditorOutcome::Consumed)
            }
            Some(PeekAct::Next) => {
                peek.at = (peek.at + 1) % count;
                self.show_peek();
                Some(EditorOutcome::Consumed)
            }
            Some(PeekAct::Previous) => {
                peek.at = (peek.at + count - 1) % count;
                self.show_peek();
                Some(EditorOutcome::Consumed)
            }
            None => {
                self.peek = None;
                None
            }
        }
    }

    /// Open the card on the place the peek is at.
    fn show_peek(&mut self) {
        let Some(peek) = &self.peek else {
            return;
        };
        let place = peek.places[peek.at].clone();
        let (count, at) = (peek.places.len(), peek.at);
        let lines = self.lines_of(&place.path);
        let (row, _) = protocol::place_of(&lines, place.at);
        let first = row.saturating_sub(ABOVE);
        let last = (row + BELOW).min(lines.len().saturating_sub(1));
        let width = (last + 1).to_string().len();
        let mut state = grammar_of(&place.path, &lines).map(LineState::start);
        for line in lines.iter().take(first) {
            state = state.map(|state| winter_syntax::next_state(line, &state));
        }
        let mut text = Vec::new();
        let mut styles = Vec::new();
        for (index, line) in lines.iter().enumerate().take(last + 1).skip(first) {
            let (mark, gutter) = match index == row {
                true => (MARK, Style::Accent),
                false => (UNMARKED, Style::Dim),
            };
            let number = format!("{mark}{:>width$}  ", index + 1);
            let mut line_styles = vec![gutter; number.chars().count()];
            if let Some(now) = &state {
                let (code, next) = winter_syntax::highlight(line, now);
                line_styles.extend(code);
                state = Some(next);
            }
            text.push(format!("{number}{line}"));
            styles.push(line_styles);
        }
        let shown = self.shown_path(&place.path);
        let which = match count {
            1 => String::new(),
            n => format!("  {}/{n}, M-n M-p the others", at + 1),
        };
        let footer = format!("{shown}:{}{which}  RET goes there", row + 1);
        self.show_code_card(text.join("\n"), styles, footer);
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The language `path` is in, by its name or else by its first line.
fn grammar_of(path: &Path, lines: &[String]) -> Option<Grammar> {
    winter_syntax::of_path(path).or_else(|| {
        lines
            .first()
            .and_then(|line| winter_syntax::of_first_line(line))
    })
}

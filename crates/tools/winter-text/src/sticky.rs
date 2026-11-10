//! Sticky scroll, as VS Code's: the first lines of what the top of the view
//! is inside, pinned over the pane's first rows, dim, outermost first. In
//! code those are the definitions the grammar names (a function, an `impl`,
//! a class) whose bodies run on past the top; in Markdown and LaTeX, the
//! headings the text under the top is filed under. A click on one goes to
//! it.
//!
//! Only what is above the top is read: the definitions among the lines
//! already colored, so a long file costs no more to scroll through.

use winter_model::paint::{GutterMark, Row, Span, Style};

use super::markdown;
use super::structure;
use super::writing::Prose;
use super::{latex, TextEditor};

// ========================================================================
// Constants
// ========================================================================

/// The most lines pinned, and the least view they leave under them.
const MAX_STICKY: usize = 3;
const MIN_ROWS_UNDER: usize = 8;

/// How many definitions above the top are looked at, nearest first, for
/// those still open there.
const LOOK_BACK: usize = 2_000;

// ========================================================================
// Data Structures
// ========================================================================

/// The lines last pinned, and the text's revision and view's top they were
/// found for.
#[derive(Debug, Default)]
pub(super) struct Sticky {
    found: Option<((usize, usize), Vec<usize>)>,
}

// ========================================================================
// TextEditor: sticky scroll
// ========================================================================

impl TextEditor {
    /// The lines to pin over a view whose top is line `top` and which shows
    /// `rows` rows, outermost first.
    pub(super) fn sticky_lines(&mut self, top: usize, rows: usize) -> Vec<usize> {
        if top == 0 || rows < MIN_ROWS_UNDER + 1 || !self.sticky_on {
            return Vec::new();
        }
        let key = (self.buffer().revision(), top);
        if let Some((found, lines)) = &self.doc().sticky.found {
            if *found == key {
                return lines.clone();
            }
        }
        let mut lines = match self.prose() {
            Some(Prose::Markdown) => heading_chain(
                markdown::headings(self.buffer().lines())
                    .iter()
                    .map(|heading| (heading.row, heading.level)),
                top,
            ),
            Some(Prose::Latex) => heading_chain(
                self.buffer()
                    .lines()
                    .iter()
                    .enumerate()
                    .filter_map(|(row, line)| latex::section_depth(line).map(|depth| (row, depth))),
                top,
            ),
            None => self.open_definitions(top),
        };
        let keep = lines.len().saturating_sub(MAX_STICKY);
        lines.drain(..keep);
        self.doc_mut().sticky.found = Some((key, lines.clone()));
        lines
    }

    /// The definitions named above line `top` whose bodies run on past it,
    /// outermost first, among the lines the grammar has read.
    fn open_definitions(&self, top: usize) -> Vec<usize> {
        let doc = self.doc();
        let lines = doc.buffer.lines();
        let read = doc.states.len().min(top);
        let mut open: Vec<usize> = (0..read)
            .rev()
            .filter(|row| winter_syntax::defines(&lines[*row], &doc.states[*row]))
            .take(LOOK_BACK)
            .filter(|row| structure::end_of(lines, *row) >= top)
            .collect();
        open.reverse();
        open
    }

    /// Lay the lines `pinned` over the first of `rows`, dim, each its first
    /// screen row painted as it is, and their numbers over the first of
    /// `gutter`'s rows.
    pub(super) fn pin_rows(
        &self,
        rows: &mut [Row],
        gutter: &mut [Vec<Option<GutterMark>>],
        pinned: &[usize],
        text_cols: usize,
    ) {
        for (at, line) in pinned.iter().enumerate() {
            let Some(slot) = rows.get_mut(at) else {
                return;
            };
            // A pinned line's number recedes with its text.
            if let Some(lanes) = gutter.get_mut(at) {
                let number = GutterMark::Text(Span::new(Style::Dim, (line + 1).to_string()));
                *lanes = self.line_marks(*line, number);
            }
            let Some(mut painted) = self.text_rows(*line, text_cols, false).into_iter().next()
            else {
                continue;
            };
            for span in &mut painted {
                // The text recedes; an indentation guide is already faint and
                // keeps the color it has in the lines under it.
                if span.style != Style::IndentGuide {
                    span.style = Style::Dim;
                }
                span.lit = false;
                span.selected = false;
                span.squiggle = None;
            }
            *slot = painted;
        }
    }

    /// Show the pinned lines, or stop.
    pub(super) fn toggle_sticky(&mut self) {
        self.sticky_on = !self.sticky_on;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The headings `headings` (each its line and its depth, top to bottom)
/// files line `top` under: the last above it at each depth, under the one
/// before it.
fn heading_chain(headings: impl Iterator<Item = (usize, usize)>, top: usize) -> Vec<usize> {
    let mut chain: Vec<(usize, usize)> = Vec::new();
    for (row, depth) in headings.take_while(|(row, _)| *row < top) {
        while chain.last().is_some_and(|(_, last)| *last >= depth) {
            chain.pop();
        }
        chain.push((row, depth));
    }
    chain.into_iter().map(|(row, _)| row).collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_headings_a_line_is_filed_under_are_the_last_at_each_depth() {
        let headings = [(0, 1), (3, 2), (8, 2), (10, 3), (20, 1)];
        assert_eq!(heading_chain(headings.into_iter(), 12), [0, 8, 10]);
        assert_eq!(heading_chain(headings.into_iter(), 9), [0, 8]);
        assert_eq!(heading_chain(headings.into_iter(), 25), [20]);
    }
}

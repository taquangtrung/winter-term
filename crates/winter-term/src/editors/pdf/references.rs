//! A numbered bibliography found by its text, for the citation links whose
//! destinations a PDF no longer names.
//!
//! hyperref names each citation's destination `cite.<key>`, and publishers'
//! pipelines often write the PDF again without the table those names are
//! listed in, keeping the links. The reference list is still on its pages:
//! each entry starts a line with its `[N]`, so the link's own number finds
//! it, as magic-vscode's viewer does.

use std::collections::HashMap;

use super::outline::Target;
use super::text::PageText;

// ========================================================================
// Constants
// ========================================================================

/// The most digits an entry's number runs to.
const MAX_DIGITS: usize = 3;

/// The fewest entries counting up from `[1]` taken for a bibliography: two
/// in a row turn up in prose by chance.
const MIN_RUN: usize = 3;

// ========================================================================
// Data Structures
// ========================================================================

/// A line starting with `[N]`: an entry of the bibliography, or prose that
/// happens to start a line with a citation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Marker {
    pub number: usize,
    /// The top of its line, in points down the page.
    pub y: f32,
}

// ========================================================================
// Functions
// ========================================================================

/// The lines of `text` that start with `[N]`, down the page.
pub fn markers(text: &PageText) -> Vec<Marker> {
    text.lines
        .iter()
        .filter_map(|line| {
            let number = leading_number(&line.text)?;
            let (_, y, _, _) = line.bounds()?;
            Some(Marker { number, y })
        })
        .collect()
}

/// The bibliography among `found`, each marker with its page in the order
/// the document reads: the longest run counting up one at a time from
/// `[1]`, the later of two as long, since a bibliography follows the prose
/// citing it. Each entry's number maps to where it starts; none at all for
/// a run too short to be sure of.
pub fn bibliography(found: impl IntoIterator<Item = (usize, Marker)>) -> HashMap<usize, Target> {
    let mut best: Vec<(usize, Marker)> = Vec::new();
    let mut run: Vec<(usize, Marker)> = Vec::new();
    for (page, marker) in found {
        if marker.number == run.len() + 1 {
            run.push((page, marker));
        } else if marker.number == 1 {
            run = vec![(page, marker)];
        } else {
            continue;
        }
        if run.len() >= best.len() {
            best.clone_from(&run);
        }
    }
    if best.len() < MIN_RUN {
        return HashMap::new();
    }
    best.into_iter()
        .map(|(page, marker)| (marker.number, Target { page, y: marker.y }))
        .collect()
}

/// The entry number a citation link's text names: its last run of digits,
/// as `[12]` or the `12` of `Smith [12]`.
pub fn cited_number(text: &str) -> Option<usize> {
    let mut runs = text.split(|ch: char| !ch.is_ascii_digit());
    let last = runs.rfind(|run| !run.is_empty() && run.len() <= MAX_DIGITS)?;
    last.parse().ok().filter(|number| *number > 0)
}

/// The characters of `text` whose middles fall in `bounds`, as
/// `(left, top, width, height)` in points, line after line.
pub fn text_in(text: &PageText, bounds: (f32, f32, f32, f32)) -> String {
    let (left, top, width, height) = bounds;
    text.lines
        .iter()
        .flat_map(|line| line.text.iter().zip(&line.boxes))
        .filter(|(_, at)| {
            let (x, y, w, h) = at.bounds();
            let (x, y) = (x + w / 2.0, y + h / 2.0);
            (left..=left + width).contains(&x) && (top..=top + height).contains(&y)
        })
        .map(|(ch, _)| *ch)
        .collect()
}

/// The `N` a line opens with as `[N]`, spaces before it let be.
fn leading_number(line: &[char]) -> Option<usize> {
    let text: String = line.iter().collect();
    let (digits, _) = text.trim_start().strip_prefix('[')?.split_once(']')?;
    let numeric = digits.chars().all(|ch| ch.is_ascii_digit());
    match (numeric, digits.len()) {
        (true, 1..=MAX_DIGITS) => digits.parse().ok(),
        _ => None,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::text::tests::words;
    use super::*;

    fn page(lines: &[&str]) -> PageText {
        let glyphs = lines
            .iter()
            .enumerate()
            .flat_map(|(row, line)| words(line, 40.0, 60.0 + row as f32 * 14.0, 10.0))
            .collect();
        PageText::new(glyphs)
    }

    fn marker(number: usize) -> Marker {
        Marker { number, y: 0.0 }
    }

    #[test]
    fn test_only_a_line_opening_with_a_bracketed_number_is_a_marker() {
        let text = page(&["[1] A. Author, a paper.", "as [2] shows", "[12]Tight", "[1234] long"]);
        let numbers: Vec<usize> = markers(&text).iter().map(|m| m.number).collect();
        assert_eq!(numbers, [1, 12]);
    }

    #[test]
    fn test_the_bibliography_is_the_longest_run_up_from_one_and_the_later_of_two() {
        // Prose starting lines with [1] and [2], then a list of four, then
        // a stray [3] that breaks no run.
        let found = [(0, 1), (0, 2), (5, 1), (5, 2), (5, 3), (6, 4), (6, 3)]
            .map(|(page, number)| (page, marker(number)));
        let entries = bibliography(found);
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[&1].page, 5);
        assert_eq!(entries[&4].page, 6);
    }

    #[test]
    fn test_a_run_too_short_is_no_bibliography() {
        let found = [(0, 1), (0, 2)].map(|(page, number)| (page, marker(number)));
        assert!(bibliography(found).is_empty());
    }

    #[test]
    fn test_a_citation_names_its_last_run_of_digits() {
        assert_eq!(cited_number("[12]"), Some(12));
        // Text clipped loosely around the link brings in what is before it.
        assert_eq!(cited_number("t [3[22]"), Some(22));
        assert_eq!(cited_number("Smith"), None);
        assert_eq!(cited_number("[0]"), None);
    }

    #[test]
    fn test_the_text_in_a_box_is_the_characters_whose_middles_it_holds() {
        let text = page(&["see [7] here"]);
        // `words` sets each character 5 points wide from x = 40: `[7]` is
        // the fifth to seventh, 60 to 75.
        assert_eq!(text_in(&text, (60.0, 40.0, 15.0, 30.0)), "[7]");
    }
}

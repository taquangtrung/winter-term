//! A page's text cut into sentences, for each to be tinted its own color.
//!
//! A sentence ends at a `.`, `!`, or `?` (and any closing quote or bracket
//! after it) followed by a space or the line's end, unless what comes next
//! starts in lower case: the `e.g. the` of prose runs on.

use super::text::PageText;

// ========================================================================
// Constants
// ========================================================================

/// What ends a sentence.
const ENDS: [char; 3] = ['.', '!', '?'];

/// What may close a sentence after its end: `."` and `.)` end it too.
const CLOSERS: [char; 5] = ['"', '\'', ')', ']', '\u{201d}'];

// ========================================================================
// Data Structures
// ========================================================================

/// The part of one line a sentence covers: characters `from` to `to`,
/// both counted, of line `line`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Run {
    pub from: usize,
    pub line: usize,
    pub to: usize,
}

/// A character of the page, by line and place: `None` for the break
/// between two lines, which reads as a space.
type At = Option<(usize, usize)>;

// ========================================================================
// Functions
// ========================================================================

/// The sentences of `text`, down the page, each the runs of the lines it
/// covers.
pub fn sentences(text: &PageText) -> Vec<Vec<Run>> {
    let mut chars: Vec<(At, char)> = Vec::new();
    for (line, shown) in text.lines.iter().enumerate() {
        if !chars.is_empty() {
            chars.push((None, ' '));
        }
        chars.extend(shown.text.iter().enumerate().map(|(at, ch)| (Some((line, at)), *ch)));
    }
    let mut found = Vec::new();
    let mut start: Option<usize> = None;
    let mut index = 0;
    while index < chars.len() {
        let ch = chars[index].1;
        if start.is_none() && !ch.is_whitespace() {
            start = Some(index);
        }
        let mut end = index;
        if ENDS.contains(&ch) {
            while chars.get(end + 1).is_some_and(|(_, next)| CLOSERS.contains(next)) {
                end += 1;
            }
        }
        let closes = ENDS.contains(&ch) && ends_here(&chars, end);
        if let (true, Some(from)) = (closes || end + 1 == chars.len(), start) {
            found.push(runs(&chars[from..=end]));
            start = None;
        }
        index = end + 1;
    }
    found
}

/// Whether a sentence's end at `end` is one: a space or the page's end
/// follows it, and what comes after that does not start in lower case.
fn ends_here(chars: &[(At, char)], end: usize) -> bool {
    match chars.get(end + 1) {
        None => true,
        Some((_, next)) if next.is_whitespace() => !chars[end + 1..]
            .iter()
            .map(|(_, ch)| *ch)
            .find(|ch| !ch.is_whitespace())
            .is_some_and(char::is_lowercase),
        Some(_) => false,
    }
}

/// The runs `chars` covers, one a line, spaces at either end of a line
/// left out.
fn runs(chars: &[(At, char)]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (at, ch) in chars {
        let Some((line, at)) = *at else {
            continue;
        };
        if ch.is_whitespace() {
            continue;
        }
        match runs.last_mut() {
            Some(run) if run.line == line => run.to = at,
            _ => runs.push(Run {
                from: at,
                line,
                to: at,
            }),
        }
    }
    runs
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

    /// The text of each sentence of `lines`, its lines joined by `|`.
    fn texts(lines: &[&str]) -> Vec<String> {
        let text = page(lines);
        sentences(&text)
            .iter()
            .map(|runs| {
                runs.iter()
                    .map(|run| text.lines[run.line].text[run.from..=run.to].iter().collect::<String>())
                    .collect::<Vec<_>>()
                    .join("|")
            })
            .collect()
    }

    #[test]
    fn test_a_sentence_ends_at_its_stop_and_runs_on_over_a_line_break() {
        assert_eq!(
            texts(&["One here. Two runs", "on over a line! Three?"]),
            ["One here.", "Two runs|on over a line!", "Three?"]
        );
    }

    #[test]
    fn test_an_abbreviation_followed_by_lower_case_ends_nothing() {
        assert_eq!(texts(&["See e.g. the proof. Done."]), ["See e.g. the proof.", "Done."]);
    }

    #[test]
    fn test_a_closing_quote_or_bracket_goes_with_its_sentence() {
        assert_eq!(texts(&["He said \"no.\" Then (it ended.) Fin"]), [
            "He said \"no.\"",
            "Then (it ended.)",
            "Fin"
        ]);
    }

    #[test]
    fn test_a_number_s_decimal_point_ends_nothing() {
        assert_eq!(texts(&["It is 3.5 wide. Next."]), ["It is 3.5 wide.", "Next."]);
    }
}

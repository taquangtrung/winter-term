//! A reStructuredText document's outline: its section titles, each a line
//! underlined (and perhaps overlined) with one punctuation character at
//! least as long as it. A title's level is its style's: the first style
//! met is the top, the next new one below it, as reStructuredText reads
//! them.

use winter_model::editor::JumpTarget;

use super::entry;

// ========================================================================
// Constants
// ========================================================================

/// The characters a title's underline may be drawn with.
const ADORNMENTS: &str = "!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~";

// ========================================================================
// Functions
// ========================================================================

/// The outline of the reStructuredText document `lines`.
pub(super) fn outline(lines: &[String]) -> Vec<JumpTarget> {
    let mut outline = Vec::new();
    // Each title style met, in order: its character, and whether overlined.
    let mut styles: Vec<(char, bool)> = Vec::new();
    for (row, line) in lines.iter().enumerate() {
        let title = line.trim_end();
        if title.is_empty() || title.starts_with(char::is_whitespace) || adornment(title).is_some()
        {
            continue;
        }
        let Some(under) = lines
            .get(row + 1)
            .and_then(|next| adornment(next.trim_end()))
        else {
            continue;
        };
        if under.1 < title.chars().count() {
            continue;
        }
        let over = row
            .checked_sub(1)
            .and_then(|above| adornment(lines[above].trim_end()))
            .is_some_and(|over| over.0 == under.0);
        let style = (under.0, over);
        let level = match styles.iter().position(|known| *known == style) {
            Some(level) => level,
            None => {
                styles.push(style);
                styles.len() - 1
            }
        };
        outline.push(entry(row, level, title.trim()));
    }
    outline
}

/// The character a line is drawn with throughout, and how long it is,
/// where it is an adornment: one punctuation character, repeated.
fn adornment(line: &str) -> Option<(char, usize)> {
    let first = line.chars().next().filter(|c| ADORNMENTS.contains(*c))?;
    let count = line.chars().count();
    (count >= 2 && line.chars().all(|c| c == first)).then_some((first, count))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use super::super::tests::{labels, lines};

    #[test]
    fn test_titles_take_their_style_s_level_and_short_or_indented_lines_are_not_titles() {
        let doc = lines(concat!(
            "=====\n",
            "Guide\n",
            "=====\n",
            "\n",
            "Install\n",
            "-------\n",
            "\n",
            "Too long a title\n",
            "----\n",
            "\n",
            "    Indented\n",
            "    --------\n",
            "\n",
            "Usage\n",
            "-----\n",
            "\n",
            "Options\n",
            "=======\n",
        ));
        assert_eq!(
            labels(&outline(&doc)),
            [
                "    2  Guide",
                "    5    Install",
                "   14    Usage",
                "   17      Options",
            ]
        );
    }
}

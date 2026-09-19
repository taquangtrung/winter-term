//! Finding text in the file: what was last looked for, and where the next
//! occurrence of it lies.

// ========================================================================
// Data Structures
// ========================================================================

/// A search over the file's lines, taken literally: the editor looks for the
/// text that was typed, not a pattern. A regular expression is what `$EDITOR`
/// is one key away for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Search {
    /// Whether the search ran down the file, which is the way `n` repeats it.
    pub forward: bool,
    /// The text looked for.
    pub pattern: String,
}

// ========================================================================
// Search
// ========================================================================

impl Search {
    /// A search for `pattern`, running down the file unless `forward` says
    /// otherwise.
    pub fn new(pattern: String, forward: bool) -> Self {
        Self { forward, pattern }
    }

    /// Where the next match lies from `at`, going `forward` or back, wrapping
    /// around the file once. `None` when the text is nowhere in it.
    ///
    /// A pattern typed in lower case matches either case, and one typed with
    /// a capital in it matches exactly, so searching for a name finds it and
    /// searching for a word does not care how it was written.
    pub fn next(
        &self,
        lines: &[String],
        at: (usize, usize),
        forward: bool,
    ) -> Option<(usize, usize)> {
        if self.pattern.is_empty() || lines.is_empty() {
            return None;
        }
        let folded = !self.pattern.chars().any(char::is_uppercase);
        let needle: Vec<char> = match folded {
            true => self.pattern.to_lowercase().chars().collect(),
            false => self.pattern.chars().collect(),
        };
        let (row, col) = at;
        let total = lines.len();
        // Every line once, from the cursor's, and the cursor's line again at
        // the end so a match behind the cursor on it is found on the wrap.
        for step in 0..=total {
            let row = match forward {
                true => (row + step) % total,
                false => (row + total - step % total) % total,
            };
            let line: Vec<char> = match folded {
                true => lines[row].to_lowercase().chars().collect(),
                false => lines[row].chars().collect(),
            };
            let hits = (0..line.len().saturating_sub(needle.len() - 1))
                .filter(|start| line[*start..].starts_with(&needle));
            let found = match (step, forward) {
                (0, true) => hits.filter(|start| *start > col).min(),
                (0, false) => hits.filter(|start| *start < col).max(),
                (_, true) => hits.min(),
                (_, false) => hits.max(),
            };
            if let Some(found) = found {
                return Some((row, found));
            }
        }
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(of: &[&str]) -> Vec<String> {
        of.iter().map(|line| line.to_string()).collect()
    }

    #[test]
    fn test_a_search_walks_on_to_the_next_match_and_wraps_around_the_file() {
        let lines = lines(&["one two", "three", "two again"]);
        let search = Search::new("two".to_string(), true);
        assert_eq!(search.next(&lines, (0, 0), true), Some((0, 4)));
        assert_eq!(search.next(&lines, (0, 4), true), Some((2, 0)));
        assert_eq!(search.next(&lines, (2, 0), true), Some((0, 4)), "wrapped");
        assert_eq!(search.next(&lines, (2, 0), false), Some((0, 4)), "back");
    }

    #[test]
    fn test_a_lower_case_pattern_matches_either_case_and_a_capital_exactly() {
        let lines = lines(&["Widget", "widget"]);
        assert_eq!(
            Search::new("widget".to_string(), true).next(&lines, (0, 0), true),
            Some((1, 0)),
            "from the first line, the next of either case"
        );
        assert_eq!(
            Search::new("Widget".to_string(), true).next(&lines, (1, 0), true),
            Some((0, 0)),
            "a capital matches only the one written that way"
        );
    }

    #[test]
    fn test_text_that_is_nowhere_in_the_file_is_no_match_rather_than_a_loop() {
        let lines = lines(&["one", "two"]);
        assert_eq!(
            Search::new("three".to_string(), true).next(&lines, (0, 0), true),
            None
        );
        // A pattern longer than every line must not index past one.
        assert_eq!(
            Search::new("a very long pattern".to_string(), true).next(&lines, (0, 0), true),
            None
        );
    }
}

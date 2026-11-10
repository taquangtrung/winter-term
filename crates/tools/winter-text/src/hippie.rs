//! Hippie Expand (dabbrev-expand): expanding the word before the cursor
//! by cycling through completions found in the active buffer and other open buffers.
//!
//! Ordered closest occurrences before the cursor first (walking backwards),
//! then closest occurrences after the cursor (walking forwards), then other open
//! documents in first-occurrence order, and finally the original prefix typed.

use std::collections::HashSet;

// ========================================================================
// Data Structures
// ========================================================================

/// An in-progress hippie expansion session cycling through candidate words.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct HippieSession {
    /// The document row the expansion is taking place on.
    pub row: usize,
    /// The column where the word prefix begins.
    pub start_col: usize,
    /// The length in characters of the text currently showing in the buffer.
    pub current_len: usize,
    /// The original prefix typed before expansion started.
    pub original_prefix: String,
    /// All candidate completions, ordered nearest-first.
    pub candidates: Vec<String>,
    /// The current index in the cycle (0..=candidates.len(), where
    /// `candidates.len()` represents the original prefix).
    pub index: usize,
}

impl HippieSession {
    /// The text the session is currently showing: one of the candidates, or
    /// (once every candidate has been cycled through) the original prefix.
    pub fn current_text(&self) -> &str {
        if self.index < self.candidates.len() {
            &self.candidates[self.index]
        } else {
            &self.original_prefix
        }
    }

    /// Advance the cycle one step forward, wrapping back to 0 after the original prefix.
    pub fn advance(&mut self) {
        let total = self.candidates.len() + 1;
        self.index = (self.index + 1) % total;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Whether `c` is treated as part of a word for hippie expansion.
pub(super) fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// The run of word characters immediately before the cursor on a line,
/// and the character column where it begins.
pub(super) fn find_word_prefix(line_before_cursor: &str) -> Option<(usize, String)> {
    let chars: Vec<char> = line_before_cursor.chars().collect();
    if chars.is_empty() {
        return None;
    }
    let end = chars.len();
    if !is_word_char(chars[end - 1]) {
        return None;
    }
    let mut start = end;
    while start > 0 && is_word_char(chars[start - 1]) {
        start -= 1;
    }
    if start == end {
        return None;
    }
    let prefix: String = chars[start..end].iter().collect();
    Some((start, prefix))
}

/// Extract all words and their starting character columns from a single line.
fn words_in_line(line: &str) -> Vec<(usize, String)> {
    let chars: Vec<char> = line.chars().collect();
    let mut words = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        if !is_word_char(chars[at]) {
            at += 1;
            continue;
        }
        let begin = at;
        while at < chars.len() && is_word_char(chars[at]) {
            at += 1;
        }
        words.push((begin, chars[begin..at].iter().collect()));
    }
    words
}

/// Every word in `lines` starting with `prefix` and longer than it,
/// ordered closest before `(cursor_row, cursor_col)` first (walking backwards),
/// then closest after (walking forwards).
pub(super) fn find_expansion_candidates(
    lines: &[String],
    cursor_row: usize,
    cursor_col: usize,
    prefix: &str,
) -> Vec<String> {
    let mut before = Vec::new();
    let mut after = Vec::new();

    for (row, line) in lines.iter().enumerate() {
        for (col, word) in words_in_line(line) {
            if row == cursor_row && col + word.chars().count() == cursor_col {
                // The exact prefix or word being expanded at the cursor.
                continue;
            }
            if !word.starts_with(prefix) || word.len() <= prefix.len() {
                continue;
            }
            let is_before = if row < cursor_row {
                true
            } else if row > cursor_row {
                false
            } else {
                col < cursor_col
            };

            if is_before {
                before.push((row, col, word));
            } else {
                after.push((row, col, word));
            }
        }
    }

    // Nearest before cursor first (walking backwards: highest row first, then highest col)
    before.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| b.1.cmp(&a.1)));
    // Nearest after cursor first (walking forwards: lowest row first, then lowest col)
    after.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for (_, _, word) in before.into_iter().chain(after) {
        if seen.insert(word.clone()) {
            candidates.push(word);
        }
    }

    candidates
}

/// Candidates from another document, ordered by first occurrence.
pub(super) fn find_other_document_candidates(lines: &[String], prefix: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();

    for line in lines {
        for (_, word) in words_in_line(line) {
            if word.starts_with(prefix) && word.len() > prefix.len() && seen.insert(word.clone()) {
                candidates.push(word);
            }
        }
    }

    candidates
}

/// Merges candidate lists into one, in the order given, dropping duplicates.
pub(super) fn merge_candidate_lists(lists: Vec<Vec<String>>) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut merged = Vec::new();

    for list in lists {
        for word in list {
            if seen.insert(word.clone()) {
                merged.push(word);
            }
        }
    }

    merged
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_word_prefix() {
        assert_eq!(find_word_prefix(""), None);
        assert_eq!(find_word_prefix("   "), None);
        assert_eq!(
            find_word_prefix("hello world"),
            Some((6, "world".to_string()))
        );
        assert_eq!(
            find_word_prefix("let x = foo_bar"),
            Some((8, "foo_bar".to_string()))
        );
        assert_eq!(find_word_prefix("hello,"), None);
    }

    #[test]
    fn test_find_expansion_candidates_ordering() {
        let lines = vec![
            "distant_before_match".to_string(),
            "close_before_match".to_string(),
            "prefix_target prefix_".to_string(),
            "close_after_match".to_string(),
            "distant_after_match".to_string(),
        ];
        // Cursor at row 2, col 21 ("prefix_")
        let candidates = find_expansion_candidates(&lines, 2, 21, "prefix_");
        assert_eq!(candidates, vec!["prefix_target"]);

        let match_lines = vec![
            "match_three".to_string(),
            "match_two".to_string(),
            "mat".to_string(),
            "match_four".to_string(),
            "match_five".to_string(),
        ];
        let candidates = find_expansion_candidates(&match_lines, 2, 3, "mat");
        // Row 1 ("match_two") is closer before row 2 than row 0 ("match_three")
        // Row 3 ("match_four") is closer after row 2 than row 4 ("match_five")
        assert_eq!(
            candidates,
            vec!["match_two", "match_three", "match_four", "match_five"]
        );
    }

    #[test]
    fn test_session_advance_and_cycling() {
        let mut session = HippieSession {
            row: 0,
            start_col: 0,
            current_len: 3,
            original_prefix: "foo".to_string(),
            candidates: vec!["foobar".to_string(), "foobaz".to_string()],
            index: 0,
        };

        assert_eq!(session.current_text(), "foobar");
        session.advance();
        assert_eq!(session.current_text(), "foobaz");
        session.advance();
        // Index is now 2 == candidates.len(), which wraps back to original prefix
        assert_eq!(session.current_text(), "foo");
        session.advance();
        // Wraps back to candidate 0
        assert_eq!(session.current_text(), "foobar");
    }

    #[test]
    fn test_merge_candidate_lists() {
        let list1 = vec!["apple".to_string(), "banana".to_string()];
        let list2 = vec![
            "banana".to_string(),
            "cherry".to_string(),
            "date".to_string(),
        ];
        let merged = merge_candidate_lists(vec![list1, list2]);
        assert_eq!(merged, vec!["apple", "banana", "cherry", "date"]);
    }
}

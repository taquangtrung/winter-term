//! Interactive search and replace (`M-%` / `query-replace`).

use crate::model::editor::{match_spans, EditorOutcome, Notice, PromptMode, PromptRequest};

use super::TextEditor;

pub(super) const ASK_QUERY_REPLACE_SEARCH: &str = "query-replace-search";
pub(super) const ASK_QUERY_REPLACE_TARGET: &str = "query-replace-target";
pub(super) const ASK_QUERY_REPLACE_ACTION: &str = "query-replace-action";

/// State of an in-progress interactive query-replace session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct QueryReplaceState {
    /// Pattern being searched.
    pub search: String,
    /// Replacement string.
    pub replacement: String,
    /// Number of replacements made so far.
    pub replaced: usize,
    /// Current match row.
    pub match_row: usize,
    /// Current match start column.
    pub match_col: usize,
}

impl TextEditor {
    /// Begin interactive query-replace: prompt for search pattern.
    pub(super) fn start_query_replace(&mut self) -> EditorOutcome {
        if let Some(refused) = self.refuse_edits() {
            return refused;
        }
        let initial = self
            .selection()
            .and_then(|sel| {
                if sel.start.0 == sel.end.0 {
                    let line = self.buffer().lines().get(sel.start.0)?;
                    let chars: Vec<char> = line.chars().collect();
                    let from = sel.start.1.min(chars.len());
                    let to = (sel.end.1 + 1).min(chars.len());
                    (from < to).then(|| chars[from..to].iter().collect())
                } else {
                    None
                }
            })
            .unwrap_or_default();
        EditorOutcome::Prompt(PromptRequest {
            initial,
            label: "Query replace: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_QUERY_REPLACE_SEARCH,
        })
    }

    /// User answered search pattern question.
    pub(super) fn on_query_replace_search(&mut self, answer: Option<String>) -> EditorOutcome {
        let Some(search) = answer.filter(|s| !s.is_empty()) else {
            return EditorOutcome::Consumed;
        };
        // Store search in query_replace partially
        self.query_replace = Some(QueryReplaceState {
            search: search.clone(),
            replacement: String::new(),
            replaced: 0,
            match_row: 0,
            match_col: 0,
        });
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!("Replace '{search}' with: "),
            mode: PromptMode::Text,
            tag: ASK_QUERY_REPLACE_TARGET,
        })
    }

    /// User answered replacement target question.
    pub(super) fn on_query_replace_target(
        &mut self,
        answer: Option<String>,
    ) -> EditorOutcome {
        let Some(mut state) = self.query_replace.take() else {
            return EditorOutcome::Consumed;
        };
        let Some(replacement) = answer else {
            return EditorOutcome::Consumed;
        };
        state.replacement = replacement;

        let (row, col) = (self.buffer().row(), self.buffer().col());
        let lines = self.buffer().lines();
        let first_match = find_match_after(lines, &state.search, (row, col));
        let Some((m_row, m_col)) = first_match else {
            self.modal
                .report(Notice::info(format!("'{}' not found", state.search)));
            return EditorOutcome::Consumed;
        };

        self.buffer_mut().move_to(m_row, m_col);
        state.match_row = m_row;
        state.match_col = m_col;

        let search = state.search.clone();
        let replacement = state.replacement.clone();
        self.query_replace = Some(state);

        self.prompt_query_replace_action(&search, &replacement)
    }

    /// Show the action prompt for the current match.
    fn prompt_query_replace_action(&self, search: &str, replacement: &str) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!("Query replace '{search}' with '{replacement}': "),
            mode: PromptMode::QueryReplace,
            tag: ASK_QUERY_REPLACE_ACTION,
        })
    }

    /// User responded to query-replace action prompt (`y`, `n`, `!`, `.`, `q`).
    pub(super) fn on_query_replace_action(&mut self, answer: Option<String>) -> EditorOutcome {
        let Some(mut state) = self.query_replace.take() else {
            return EditorOutcome::Consumed;
        };
        let choice = answer.as_deref().unwrap_or("q");
        match choice {
            "y" => {
                self.replace_at(
                    state.match_row,
                    state.match_col,
                    &state.search,
                    &state.replacement,
                );
                state.replaced += 1;
                let s_char_len = state.replacement.chars().count();
                let next_start = (state.match_row, state.match_col + s_char_len);
                let next = find_match_after(self.buffer().lines(), &state.search, next_start);
                if let Some((next_row, next_col)) = next {
                    state.match_row = next_row;
                    state.match_col = next_col;
                    self.buffer_mut().move_to(next_row, next_col);
                    let outcome =
                        self.prompt_query_replace_action(&state.search, &state.replacement);
                    self.query_replace = Some(state);
                    outcome
                } else {
                    let total = state.replaced;
                    let word = if total == 1 { "occurrence" } else { "occurrences" };
                    self.modal
                        .report(Notice::info(format!("Replaced {total} {word}")));
                    EditorOutcome::Consumed
                }
            }
            "n" => {
                let next_start = (state.match_row, state.match_col + 1);
                let next = find_match_after(self.buffer().lines(), &state.search, next_start);
                if let Some((next_row, next_col)) = next {
                    state.match_row = next_row;
                    state.match_col = next_col;
                    self.buffer_mut().move_to(next_row, next_col);
                    let outcome =
                        self.prompt_query_replace_action(&state.search, &state.replacement);
                    self.query_replace = Some(state);
                    outcome
                } else {
                    let total = state.replaced;
                    let word = if total == 1 { "occurrence" } else { "occurrences" };
                    self.modal
                        .report(Notice::info(format!("Replaced {total} {word}")));
                    EditorOutcome::Consumed
                }
            }
            "." => {
                self.replace_at(
                    state.match_row,
                    state.match_col,
                    &state.search,
                    &state.replacement,
                );
                state.replaced += 1;
                let total = state.replaced;
                let word = if total == 1 { "occurrence" } else { "occurrences" };
                self.modal
                    .report(Notice::info(format!("Replaced {total} {word}")));
                EditorOutcome::Consumed
            }
            "!" => {
                self.replace_at(
                    state.match_row,
                    state.match_col,
                    &state.search,
                    &state.replacement,
                );
                state.replaced += 1;
                let mut next_start = (
                    state.match_row,
                    state.match_col + state.replacement.chars().count(),
                );
                while let Some((next_row, next_col)) =
                    find_match_after(self.buffer().lines(), &state.search, next_start)
                {
                    self.replace_at(next_row, next_col, &state.search, &state.replacement);
                    state.replaced += 1;
                    next_start = (next_row, next_col + state.replacement.chars().count());
                }
                let total = state.replaced;
                let word = if total == 1 { "occurrence" } else { "occurrences" };
                self.modal
                    .report(Notice::info(format!("Replaced {total} {word}")));
                EditorOutcome::Consumed
            }
            _ => {
                let total = state.replaced;
                let word = if total == 1 { "occurrence" } else { "occurrences" };
                self.modal
                    .report(Notice::info(format!("Replaced {total} {word}")));
                EditorOutcome::Consumed
            }
        }
    }

    /// Perform a single string replacement on a line at `(row, col)`.
    fn replace_at(&mut self, row: usize, col: usize, search: &str, replacement: &str) {
        let Some(line) = self.buffer().lines().get(row) else {
            return;
        };
        let mut chars: Vec<char> = line.chars().collect();
        let search_len = search.chars().count();
        let from = col.min(chars.len());
        let to = (col + search_len).min(chars.len());
        chars.splice(from..to, replacement.chars());
        let new_line: String = chars.into_iter().collect();
        self.buffer_mut().replace_rows(row, 1, vec![new_line]);
        self.buffer_mut().move_to(row, from);
    }
}

/// Find match strictly at or after `at = (start_row, start_col)` down through the document.
fn find_match_after(
    lines: &[String],
    pattern: &str,
    at: (usize, usize),
) -> Option<(usize, usize)> {
    if pattern.is_empty() || lines.is_empty() {
        return None;
    }
    let (start_row, start_col) = at;
    if let Some(line) = lines.get(start_row) {
        let hits = match_spans(line, pattern);
        if let Some((col, _)) = hits.into_iter().find(|(col, _)| *col >= start_col) {
            return Some((start_row, col));
        }
    }
    for (row, line) in lines.iter().enumerate().skip(start_row + 1) {
        let hits = match_spans(line, pattern);
        if let Some((col, _)) = hits.into_iter().next() {
            return Some((row, col));
        }
    }
    None
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_match_after_finds_in_same_line_and_next_lines() {
        let lines = vec![
            "hello world hello".to_string(),
            "no match here".to_string(),
            "say hello again".to_string(),
        ];
        // At start of first line
        assert_eq!(find_match_after(&lines, "hello", (0, 0)), Some((0, 0)));
        // Past first occurrence on row 0
        assert_eq!(find_match_after(&lines, "hello", (0, 1)), Some((0, 12)));
        // Past second occurrence on row 0
        assert_eq!(find_match_after(&lines, "hello", (0, 13)), Some((2, 4)));
        // Past third occurrence on row 2
        assert_eq!(find_match_after(&lines, "hello", (2, 5)), None);
    }
}

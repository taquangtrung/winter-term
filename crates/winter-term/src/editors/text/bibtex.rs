//! Sorting a BibTeX file's entries, as magic-vscode sorts them: by citation
//! key or by year, either way round. What stands before the first entry (a
//! comment, `@string` and `@preamble` blocks) stays where it is, and each
//! entry keeps its own text, the blank line after it included.

use crate::model::editor::{EditorOutcome, Notice};

use super::TextEditor;

// ========================================================================
// Data Structures
// ========================================================================

/// What the entries are sorted by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SortBy {
    Key,
    Year,
}

/// One entry: its lines, its citation key, and its year where it has one.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    key: String,
    lines: Vec<String>,
    year: Option<u32>,
}

// ========================================================================
// TextEditor: BibTeX
// ========================================================================

impl TextEditor {
    /// Sort the buffer's entries by `by`, the earliest or smallest first
    /// when `ascending` is set.
    pub(super) fn sort_bibtex(&mut self, by: SortBy, ascending: bool) -> EditorOutcome {
        let is_bib = self
            .doc()
            .path
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("bib"));
        if !is_bib {
            self.modal.report(Notice::info("BibTeX: not a .bib file"));
            return EditorOutcome::Consumed;
        }
        let lines = self.buffer().lines().to_vec();
        let (head, mut entries) = entries_of(&lines);
        if entries.len() < 2 {
            return EditorOutcome::Consumed;
        }
        // A stable sort, so entries alike keep their order.
        match by {
            SortBy::Key => entries.sort_by_key(|entry| entry.key.to_lowercase()),
            SortBy::Year => entries.sort_by_key(|entry| entry.year.unwrap_or(u32::MAX)),
        }
        if !ascending {
            entries.reverse();
        }
        let mut sorted = head;
        let count = entries.len();
        for mut entry in entries {
            while entry.lines.last().is_some_and(|line| line.trim().is_empty()) {
                entry.lines.pop();
            }
            sorted.extend(entry.lines);
            sorted.push(String::new());
        }
        sorted.pop();
        if sorted != lines {
            let total = lines.len();
            self.buffer_mut().replace_rows(0, total, sorted);
        }
        self.modal.report(Notice::info(format!("sorted {count} entries")).unlogged());
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What stands before the first entry, and the entries, each from its `@`
/// line to the line before the next one. `@string`, `@preamble`, and
/// `@comment` blocks are not entries: they stay with what is before them.
fn entries_of(lines: &[String]) -> (Vec<String>, Vec<Entry>) {
    let starts: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, line)| entry_key(line).is_some())
        .map(|(row, _)| row)
        .collect();
    let Some(&first) = starts.first() else {
        return (lines.to_vec(), Vec::new());
    };
    let head = lines[..first].to_vec();
    let entries = starts
        .iter()
        .enumerate()
        .map(|(at, &start)| {
            let end = starts.get(at + 1).copied().unwrap_or(lines.len());
            let text = lines[start..end].to_vec();
            Entry {
                key: entry_key(&lines[start]).unwrap_or_default(),
                year: text.iter().find_map(|line| year_of(line)),
                lines: text,
            }
        })
        .collect();
    (head, entries)
}

/// The citation key an entry's first line names: `@article{key,`.
fn entry_key(line: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix('@')?;
    let open = rest.find(['{', '('])?;
    let kind = rest[..open].trim().to_ascii_lowercase();
    if matches!(kind.as_str(), "string" | "preamble" | "comment") || kind.is_empty() {
        return None;
    }
    let key = rest[open + 1..].split(',').next()?.trim();
    (!key.is_empty()).then(|| key.to_string())
}

/// The year a `year = ...` field line gives.
fn year_of(line: &str) -> Option<u32> {
    let (name, value) = line.split_once('=')?;
    if !name.trim().eq_ignore_ascii_case("year") {
        return None;
    }
    let digits: String = value.chars().skip_while(|c| !c.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn test_entries_are_read_with_their_keys_and_years_after_the_head() {
        let text = lines("% refs\n@string{x = \"y\"}\n\n@article{zed,\n  year = {2020},\n}\n\n@book{alpha,\n  Year = 1999\n}\n");
        let (head, entries) = entries_of(&text);
        assert_eq!(head, ["% refs", "@string{x = \"y\"}", ""]);
        let read: Vec<(&str, Option<u32>)> = entries.iter().map(|entry| (entry.key.as_str(), entry.year)).collect();
        assert_eq!(read, [("zed", Some(2020)), ("alpha", Some(1999))]);
    }
}

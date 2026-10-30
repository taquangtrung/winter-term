//! Occurrences of the word under the cursor: lit wherever they show, stepped
//! between with `M-n` and `M-p`, and changed all at once, as magic-vscode's
//! occurrence mode and Emacs' iedit change them. `C-;` starts the change:
//! what is typed goes into every occurrence together, `Tab` and `S-Tab`
//! step between them, and `Esc` or `RET` ends it.

use crate::model::editor::{EditorOutcome, Notice};
use crate::model::input::{Key, KeyCode, Mods};

use super::complete::is_word_char;
use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The shortest word lit where the cursor rests on it: a lone letter is
/// everywhere, and lighting it says nothing.
const MIN_LIT_CHARS: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// A change going into every occurrence at once: where each one starts (a
/// row and a column), how long each is now, what each now reads, which one
/// the cursor is in, and how far into it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Occur {
    at: usize,
    offset: usize,
    regions: Vec<(usize, usize)>,
    text: Vec<char>,
}

// ========================================================================
// TextEditor: lighting
// ========================================================================

impl TextEditor {
    /// The columns of line `index` an occurrence covers, half-open, and
    /// whether they are being changed (drawn louder) rather than only lit.
    pub(super) fn lit_columns(&self, index: usize, line: &str) -> (Vec<(usize, usize)>, bool) {
        if let Some(occur) = &self.occur {
            let len = occur.text.len();
            let spans = occur
                .regions
                .iter()
                .filter(|(row, _)| *row == index)
                .map(|(_, col)| (*col, col + len))
                .collect();
            return (spans, true);
        }
        if !self.occurrences || self.modal.is_typing() || self.modal.is_selecting() {
            return (Vec::new(), false);
        }
        let Some(word) = self.word_under_cursor().filter(|word| word.len() >= MIN_LIT_CHARS) else {
            return (Vec::new(), false);
        };
        let chars: Vec<char> = line.chars().collect();
        // The one the cursor is in is where the eye already is.
        let row = self.buffer().row();
        let col = self.buffer().col();
        let others = whole_words(&chars, &word)
            .into_iter()
            .filter(|(start, end)| index != row || !(*start..=*end).contains(&col))
            .collect();
        (others, false)
    }

    /// The word the cursor is on, as characters.
    fn word_under_cursor(&self) -> Option<Vec<char>> {
        let line: Vec<char> = self.line_of(self.buffer().row()).chars().collect();
        let (start, end) = word_at(&line, self.buffer().col())?;
        Some(line[start..end].to_vec())
    }

    /// Light the occurrences of the word under the cursor, or stop.
    pub(super) fn toggle_occurrences(&mut self) -> EditorOutcome {
        self.occurrences = !self.occurrences;
        let state = if self.occurrences { "on" } else { "off" };
        self.modal.report(Notice::info(format!("occurrences lit {state}")).unlogged());
        EditorOutcome::Consumed
    }

    /// On to the next occurrence of the word under the cursor, or back to
    /// the one before, round from the end.
    pub(super) fn step_occurrence(&mut self, forward: bool) -> EditorOutcome {
        let Some(word) = self.word_under_cursor() else {
            self.modal.report(Notice::info("no word under the cursor").unlogged());
            return EditorOutcome::Consumed;
        };
        let found = self.occurrences_of(&word, false);
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let line: Vec<char> = self.line_of(row).chars().collect();
        let here = word_at(&line, col).map_or((row, col), |(start, _)| (row, start));
        let next = match forward {
            true => found.iter().find(|at| **at > here).or(found.first()),
            false => found.iter().rev().find(|at| **at < here).or(found.last()),
        };
        if let Some(&(row, col)) = next {
            self.buffer_mut().move_to(row, col);
        }
        let count = found.len();
        let place = found.iter().position(|at| Some(at) == next).map_or(0, |at| at + 1);
        self.modal.report(Notice::info(format!("occurrence {place} of {count}")).unlogged());
        EditorOutcome::Consumed
    }

    /// Where `word` occurs in the buffer, as whole words, or anywhere when
    /// `anywhere` is set (a selection's text, which need not be a word).
    fn occurrences_of(&self, word: &[char], anywhere: bool) -> Vec<(usize, usize)> {
        self.buffer()
            .lines()
            .iter()
            .enumerate()
            .flat_map(|(row, line)| {
                let chars: Vec<char> = line.chars().collect();
                let found = match anywhere {
                    true => literal(&chars, word),
                    false => whole_words(&chars, word),
                };
                found.into_iter().map(move |(start, _)| (row, start))
            })
            .collect()
    }
}

// ========================================================================
// TextEditor: changing every occurrence
// ========================================================================

impl TextEditor {
    /// Start changing every occurrence of the word under the cursor, or of
    /// the text a selection on one line covers.
    pub(super) fn start_occur(&mut self) -> EditorOutcome {
        if self.modal.is_read_only() {
            self.modal.report(Notice::info("this buffer is read-only"));
            return EditorOutcome::Consumed;
        }
        let selected = self.selection().filter(|selection| !selection.linewise && selection.start.0 == selection.end.0);
        let (word, anywhere, start) = match selected {
            Some(selection) => {
                let line: Vec<char> = self.line_of(selection.start.0).chars().collect();
                let end = (selection.end.1 + 1).min(line.len());
                (line[selection.start.1..end].to_vec(), true, (selection.start.0, selection.start.1))
            }
            None => {
                let row = self.buffer().row();
                let line: Vec<char> = self.line_of(row).chars().collect();
                let Some((from, to)) = word_at(&line, self.buffer().col()) else {
                    self.modal.report(Notice::info("no word under the cursor to change").unlogged());
                    return EditorOutcome::Consumed;
                };
                (line[from..to].to_vec(), false, (row, from))
            }
        };
        if word.is_empty() {
            return EditorOutcome::Consumed;
        }
        let regions = self.occurrences_of(&word, anywhere);
        let at = regions.iter().position(|region| *region == start).unwrap_or(0);
        let offset = match (self.buffer().row(), self.buffer().col()) {
            (row, col) if row == start.0 && col >= start.1 => (col - start.1).min(word.len()),
            _ => 0,
        };
        self.modal.clear_selection();
        self.leave_to_normal();
        let count = regions.len();
        self.occur = Some(Occur {
            at,
            offset,
            regions,
            text: word,
        });
        self.place_occur_cursor();
        self.modal.report(
            Notice::info(format!("changing {count} occurrences: type, Tab to the next, Esc to finish")).unlogged(),
        );
        EditorOutcome::Consumed
    }

    /// A key while every occurrence is being changed: what it does to them,
    /// or `None` for a key that ends the change and acts as it would have.
    pub(super) fn occur_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        let occur = self.occur.as_mut()?;
        let count = occur.regions.len();
        match key.chord() {
            (Mods::NONE, KeyCode::Escape | KeyCode::Enter) => {
                self.finish_occur();
                return Some(EditorOutcome::Consumed);
            }
            (Mods::CTRL, KeyCode::Char(';' | 'g')) => {
                self.finish_occur();
                return Some(EditorOutcome::Consumed);
            }
            (Mods::NONE, KeyCode::Tab) => occur.at = (occur.at + 1) % count.max(1),
            (Mods::SHIFT, KeyCode::Tab) => occur.at = (occur.at + count.max(1) - 1) % count.max(1),
            (Mods::NONE, KeyCode::Left) => occur.offset = occur.offset.saturating_sub(1),
            (Mods::NONE, KeyCode::Right) => occur.offset = (occur.offset + 1).min(occur.text.len()),
            (Mods::NONE, KeyCode::Home) | (Mods::CTRL, KeyCode::Char('a')) => occur.offset = 0,
            (Mods::NONE, KeyCode::End) | (Mods::CTRL, KeyCode::Char('e')) => {
                occur.offset = occur.text.len()
            }
            (Mods::NONE, KeyCode::Backspace) if occur.offset > 0 => {
                let old = occur.text.len();
                occur.offset -= 1;
                let offset = occur.offset;
                occur.text.remove(offset);
                self.rewrite_occurrences(old);
            }
            (Mods::NONE, KeyCode::Backspace) => {}
            (Mods::NONE, KeyCode::Delete) if occur.offset < occur.text.len() => {
                let old = occur.text.len();
                let offset = occur.offset;
                occur.text.remove(offset);
                self.rewrite_occurrences(old);
            }
            (Mods::NONE, KeyCode::Delete) => {}
            (Mods::NONE, KeyCode::Char(c)) => {
                let old = occur.text.len();
                let offset = occur.offset;
                occur.text.insert(offset, c);
                occur.offset += 1;
                self.rewrite_occurrences(old);
            }
            (Mods::NONE, KeyCode::Space) => {
                let old = occur.text.len();
                let offset = occur.offset;
                occur.text.insert(offset, ' ');
                occur.offset += 1;
                self.rewrite_occurrences(old);
            }
            _ => {
                self.finish_occur();
                return None;
            }
        }
        self.place_occur_cursor();
        Some(EditorOutcome::Consumed)
    }

    /// Stop changing the occurrences, the cursor where it is.
    fn finish_occur(&mut self) {
        if self.occur.take().is_some() {
            self.buffer_mut().clamp(false);
        }
    }

    /// Put each occurrence's text, `old` characters long, in as it now
    /// reads, as one change.
    fn rewrite_occurrences(&mut self, old: usize) {
        let Some(occur) = self.occur.clone() else {
            return;
        };
        let new = occur.text.len();
        let Some(first) = occur.regions.first().map(|(row, _)| *row) else {
            return;
        };
        let last = occur.regions.last().map_or(first, |(row, _)| *row);
        let text: String = occur.text.iter().collect();
        let mut lines: Vec<String> = self.buffer().lines()[first..=last].to_vec();
        let mut shifted = Vec::with_capacity(occur.regions.len());
        let mut row_shift = 0isize;
        let mut current_row = usize::MAX;
        for &(row, col) in &occur.regions {
            if row != current_row {
                current_row = row;
                row_shift = 0;
            }
            let start = (col as isize + row_shift) as usize;
            let chars: Vec<char> = lines[row - first].chars().collect();
            let end = (start + old).min(chars.len());
            let rebuilt: String = chars[..start.min(chars.len())]
                .iter()
                .chain(text.chars().collect::<Vec<_>>().iter())
                .chain(chars[end..].iter())
                .collect();
            lines[row - first] = rebuilt;
            shifted.push((row, start));
            row_shift += new as isize - old as isize;
        }
        if let Some(occur) = self.occur.as_mut() {
            occur.regions = shifted;
        }
        let count = last - first + 1;
        self.buffer_mut().replace_rows(first, count, lines);
    }

    /// Put the cursor where it is in the occurrence it is in.
    fn place_occur_cursor(&mut self) {
        let Some(occur) = &self.occur else {
            return;
        };
        let Some(&(row, col)) = occur.regions.get(occur.at) else {
            return;
        };
        let offset = occur.offset.min(occur.text.len());
        self.buffer_mut().move_to_typing(row, col + offset);
    }

    /// What the status bar says while every occurrence is being changed.
    pub(super) fn occur_status(&self) -> Option<String> {
        let occur = self.occur.as_ref()?;
        Some(format!("occurrence {} of {}", occur.at + 1, occur.regions.len()))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The word `col` is on in `line`, as a half-open range, or the one ending
/// just before it.
fn word_at(line: &[char], col: usize) -> Option<(usize, usize)> {
    let col = match line.get(col).is_some_and(|c| is_word_char(*c)) {
        true => col,
        false if col > 0 && line.get(col - 1).is_some_and(|c| is_word_char(*c)) => col - 1,
        false => return None,
    };
    let start = (0..=col).rev().take_while(|at| is_word_char(line[*at])).last()?;
    let end = (col..line.len()).take_while(|at| is_word_char(line[*at])).last()? + 1;
    Some((start, end))
}

/// Where `word` stands in `line` as a whole word, half-open.
fn whole_words(line: &[char], word: &[char]) -> Vec<(usize, usize)> {
    literal(line, word)
        .into_iter()
        .filter(|(start, end)| {
            let before = start.checked_sub(1).and_then(|at| line.get(at));
            let after = line.get(*end);
            !before.is_some_and(|c| is_word_char(*c)) && !after.is_some_and(|c| is_word_char(*c))
        })
        .collect()
}

/// Where `word` stands in `line`, half-open, the ones that would overlap
/// an earlier one left out.
fn literal(line: &[char], word: &[char]) -> Vec<(usize, usize)> {
    let mut found = Vec::new();
    if word.is_empty() || word.len() > line.len() {
        return found;
    }
    let mut at = 0;
    while at + word.len() <= line.len() {
        if line[at..at + word.len()] == *word {
            found.push((at, at + word.len()));
            at += word.len();
        } else {
            at += 1;
        }
    }
    found
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    #[test]
    fn test_only_whole_words_are_occurrences() {
        let line = chars("val value val_x (val)");
        assert_eq!(whole_words(&line, &chars("val")), [(0, 3), (17, 20)]);
        assert_eq!(literal(&chars("aaaa"), &chars("aa")), [(0, 2), (2, 4)]);
    }

    #[test]
    fn test_the_word_under_the_cursor_or_just_before_it() {
        let line = chars("let foo = 1;");
        assert_eq!(word_at(&line, 5), Some((4, 7)));
        assert_eq!(word_at(&line, 7), Some((4, 7)), "just past the word");
        assert_eq!(word_at(&line, 8), None);
    }
}

//! Ghost text: a source's suggestion drawn dim at the cursor as you type,
//! its first line in the cursor's and the rest on rows of their own under
//! it, an overlay the file's text makes room for, as copilot.el draws it.
//!
//! While ghost text shows, `Tab` (or `C-RET`) takes all of it, `M-f`,
//! `M-<right>`, or `C-,` its next word, `C-.` its next line, `M-n` and `M-p`
//! step through what else the source offered, and `C-g` hides it (as `Esc`
//! does on its way back to Normal mode). Typing what it shows keeps it
//! showing.

use winter_lsp::protocol::{self, Range, TextEdit};
use winter_model::editor::EditorOutcome;
use winter_model::input::{Key, KeyCode, Mods};
use winter_model::paint::{Row, Style, TAB_WIDTH};

use super::{end_of, OfferKind, Suggestion};
use crate::{runs, Cell, TextEditor};

// ========================================================================
// Data Structures
// ========================================================================

/// The ghost text offered: every suggestion, which is shown, the row it is
/// for, and the source it came from.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Ghost {
    pub(super) at: usize,
    pub(super) items: Vec<Suggestion>,
    pub(super) row: usize,
    /// The source's place in the editor's list.
    pub(super) source: usize,
    /// Which suggestions the source has been told were shown.
    pub(super) told: Vec<usize>,
}

// ========================================================================
// TextEditor: ghost text
// ========================================================================

impl TextEditor {
    /// The ghost text to draw at the cursor: what the suggestion shown puts
    /// in past what is already typed, less any of the line's own text after
    /// the cursor it ends with. `None` where none shows, or where what is
    /// typed has parted from it.
    pub(crate) fn ghost_text(&self) -> Option<String> {
        let ghost = self.inline.ghost.as_ref()?;
        let item = ghost.items.get(ghost.at)?;
        let row = self.buffer().row();
        if row != ghost.row {
            return None;
        }
        let line: Vec<char> = self.buffer().lines().get(row)?.chars().collect();
        let col = self.buffer().col().min(line.len());
        if item.start > col {
            return None;
        }
        let typed: String = line[item.start..col].iter().collect();
        let rest = item.insert.strip_prefix(&typed)?;
        let tail: String = line[col..item.end.max(col).min(line.len())]
            .iter()
            .collect();
        let rest = rest.strip_suffix(&tail).unwrap_or(rest);
        (!rest.is_empty()).then(|| rest.to_string())
    }

    /// A key ghost text gives a meaning of its own, while it shows.
    pub(super) fn ghost_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        match key.chord() {
            (Mods::NONE, KeyCode::Tab) | (Mods::CTRL, KeyCode::Enter) => {
                Some(self.take_whole_ghost())
            }
            (Mods::ALT, KeyCode::Char('f') | KeyCode::Right) | (Mods::CTRL, KeyCode::Char(',')) => {
                Some(self.take_ghost_part(false))
            }
            (Mods::CTRL, KeyCode::Char('.')) => Some(self.take_ghost_part(true)),
            (Mods::ALT, KeyCode::Char('n')) => Some(self.step_ghost(true)),
            (Mods::ALT, KeyCode::Char('p')) => Some(self.step_ghost(false)),
            (Mods::CTRL, KeyCode::Char('g')) => {
                self.inline.ghost = None;
                Some(EditorOutcome::Consumed)
            }
            (Mods::NONE, KeyCode::Escape) => {
                self.inline.ghost = None;
                None
            }
            _ => None,
        }
    }

    /// Put the ghost text away and tell its source the suggestion shown was
    /// taken.
    fn finish_ghost(&mut self) {
        let Some(ghost) = self.inline.ghost.take() else {
            return;
        };
        if let Some(item) = ghost.items.get(ghost.at) {
            self.tell_taken(ghost.source, OfferKind::Ghost, &item.receipt);
        }
    }

    /// Take the whole suggestion shown: what the ghost text shows put in at
    /// the cursor (the line's own text after it being what the suggestion
    /// ends with), the cursor after it, and its source told.
    fn take_whole_ghost(&mut self) -> EditorOutcome {
        let Some(rest) = self.ghost_text() else {
            return EditorOutcome::Consumed;
        };
        self.put_in(&rest);
        self.finish_ghost();
        EditorOutcome::Consumed
    }

    /// Put `text` in at the cursor, the cursor after it, as one edit.
    fn put_in(&mut self, text: &str) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let at = protocol::position_in(self.buffer().lines(), row, col);
        let index = self.at.min(self.docs.len() - 1);
        self.apply_to_buffer(
            index,
            &[TextEdit {
                new_text: text.to_string(),
                range: Range { end: at, start: at },
            }],
        );
        let (end_row, end_col) = end_of(row, col, text);
        self.buffer_mut().move_to_typing(end_row, end_col);
    }

    /// Take the ghost text's next word (`line` unset) or the rest of its
    /// line, the line after where it is at a line's end: what is left keeps
    /// showing, on the row the cursor ends on.
    fn take_ghost_part(&mut self, line: bool) -> EditorOutcome {
        let Some(ghost) = self.ghost_text() else {
            return EditorOutcome::Consumed;
        };
        let chars: Vec<char> = ghost.chars().collect();
        let end = match line {
            true => match chars.first() {
                Some('\n') => 1 + chars[1..].iter().take_while(|c| **c != '\n').count(),
                _ => chars.iter().take_while(|c| **c != '\n').count(),
            },
            false => {
                // Spaces before the word, then the word, as Emacs' `M-f`.
                let spaces = chars.iter().take_while(|c| c.is_whitespace()).count();
                let word = chars[spaces..]
                    .iter()
                    .take_while(|c| !c.is_whitespace())
                    .count();
                (spaces + word).max(1)
            }
        };
        let piece: String = chars[..end.min(chars.len())].iter().collect();
        let left: String = chars[end.min(chars.len())..].iter().collect();
        let tail = self.ghost_tail();
        self.put_in(&piece);
        if piece.contains('\n') {
            self.rebase_ghost(&left, tail);
        }
        if self.ghost_text().is_none() {
            self.finish_ghost();
        }
        EditorOutcome::Consumed
    }

    /// How many characters of the line after the cursor the suggestion
    /// shown ends with, and so leaves out of its ghost text.
    fn ghost_tail(&self) -> usize {
        let Some(item) = self
            .inline
            .ghost
            .as_ref()
            .and_then(|ghost| ghost.items.get(ghost.at))
        else {
            return 0;
        };
        let col = self.buffer().col();
        item.end
            .max(col)
            .min(self.buffer().line_len_of(self.buffer().row()))
            - col
    }

    /// After part of the ghost text was taken onto a new row: the suggestion
    /// shown, alone, for the row the cursor is now on, `left` still to come
    /// and the `tail` characters after the cursor what it ends with.
    fn rebase_ghost(&mut self, left: &str, tail: usize) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let line: Vec<char> = self
            .buffer()
            .lines()
            .get(row)
            .map(|line| line.chars().collect())
            .unwrap_or_default();
        let Some(ghost) = self.inline.ghost.as_mut() else {
            return;
        };
        let Some(shown) = ghost.items.get(ghost.at) else {
            return;
        };
        let end = (col + tail).min(line.len());
        let typed: String = line[..col.min(line.len())].iter().collect();
        let after: String = line[col.min(end)..end].iter().collect();
        ghost.items = vec![Suggestion {
            end,
            insert: format!("{typed}{left}{after}"),
            receipt: shown.receipt.clone(),
            start: 0,
        }];
        ghost.at = 0;
        ghost.row = row;
        ghost.told = vec![0];
    }

    /// The next suggestion the source offered, or the one before, round.
    fn step_ghost(&mut self, forward: bool) -> EditorOutcome {
        if let Some(ghost) = self.inline.ghost.as_mut() {
            let count = ghost.items.len().max(1);
            ghost.at = match forward {
                true => (ghost.at + 1) % count,
                false => (ghost.at + count - 1) % count,
            };
        }
        self.tell_shown();
        EditorOutcome::Consumed
    }

    /// The ghost text's first line as cells for line `index`, and where in
    /// the line's cells it goes: the cursor's.
    pub(crate) fn ghost_cells(&self, index: usize) -> Option<(usize, Vec<Cell>)> {
        if index != self.buffer().row() || !self.modal.is_typing() {
            return None;
        }
        let ghost = self.ghost_text()?;
        let first = ghost
            .split('\n')
            .next()
            .unwrap_or_default()
            .replace('\t', &" ".repeat(TAB_WIDTH));
        let cells = first
            .chars()
            .map(|c| (c, Style::Dim, None, false))
            .collect();
        let col = self.buffer().col();
        let at = self.painted_cells_before(index, col);
        Some((at, cells))
    }

    /// The ghost text's lines past its first, each drawn dim on a screen
    /// row of its own under line `index`. None but on the cursor's line
    /// while typing.
    pub(crate) fn ghost_lines(&self, index: usize) -> Vec<String> {
        if index != self.buffer().row() || !self.modal.is_typing() {
            return Vec::new();
        }
        let Some(ghost) = self.ghost_text() else {
            return Vec::new();
        };
        ghost
            .split('\n')
            .skip(1)
            .map(|line| line.replace('\t', &" ".repeat(TAB_WIDTH)))
            .collect()
    }

    /// The rows the ghost text's later lines are drawn on under line
    /// `index`: each line from display column `from` on, as far as `cols`
    /// cells.
    pub(crate) fn ghost_rows(&self, index: usize, cols: usize, from: usize) -> Vec<Row> {
        self.ghost_lines(index)
            .iter()
            .map(|line| {
                let cells: Vec<Cell> = line
                    .chars()
                    .skip(from)
                    .map(|c| (c, Style::Dim, None, false))
                    .collect();
                runs(&cells, cols, None)
            })
            .collect()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;

    use serde_json::Value;
    use winter_model::editor::Page;
    use winter_model::paint::row_text;

    /// An editor over `text` in Insert mode at the end of its first line, a
    /// source having offered `insert` over the first `end` characters of
    /// it. `tag` names the file, so tests run side by side.
    fn offered(tag: &str, text: &str, insert: &str, end: usize) -> (TextEditor, PathBuf) {
        let path =
            std::env::temp_dir().join(format!("winter-ghost-{tag}-{}.txt", std::process::id()));
        std::fs::write(&path, text).expect("temp file");
        let mut editor = TextEditor::new(path.clone(), None).expect("opens");
        editor.on_key(&key(KeyCode::Char('A'), false, false));
        editor.inline.ghost = Some(Ghost {
            at: 0,
            items: vec![Suggestion {
                end,
                insert: insert.to_string(),
                receipt: Value::Null,
                start: 0,
            }],
            row: 0,
            source: 0,
            told: Vec::new(),
        });
        (editor, path)
    }

    fn key(code: KeyCode, ctrl: bool, alt: bool) -> Key {
        Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_the_ghost_text_is_what_the_suggestion_adds_past_what_is_typed() {
        let (mut editor, path) = offered("typed", "    fo", "    foo(bar)", 6);
        assert_eq!(editor.ghost_text().as_deref(), Some("o(bar)"));
        editor.on_key(&key(KeyCode::Char('o'), false, false));
        assert_eq!(
            editor.ghost_text().as_deref(),
            Some("(bar)"),
            "typing what it shows keeps it"
        );
        editor.on_key(&key(KeyCode::Char('x'), false, false));
        assert_eq!(editor.ghost_text(), None, "typing past it puts it away");
        assert!(editor.inline.ghost.is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_the_ghost_text_leaves_out_the_line_s_own_text_after_the_cursor() {
        let (mut editor, path) = offered("tail", "foo()", "foo(x, y)", 5);
        editor.on_key(&key(KeyCode::Left, false, false));
        assert_eq!(editor.ghost_text().as_deref(), Some("x, y"));
        let painted = editor.content(4, 40, false);
        let first = row_text(&painted.rows[0]);
        assert!(
            first.contains("foo(x, y)"),
            "drawn at the cursor, the `)` after it: {first}"
        );
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_the_ghost_text_s_later_lines_are_rows_under_the_cursor_s_not_a_card() {
        let (mut editor, path) = offered("rows", "let\nend", "let a = 1;\nlet b = 2;", 3);
        let painted = editor.content(6, 40, false);
        let texts: Vec<String> = painted.rows.iter().map(row_text).collect();
        assert!(
            texts[0].contains("let a = 1;"),
            "first line in the cursor's: {texts:?}"
        );
        assert!(
            texts[1].contains("let b = 2;"),
            "the next on a row of its own: {texts:?}"
        );
        assert!(
            texts[2].contains("end"),
            "the file's next line moved down past it: {texts:?}"
        );
        assert!(editor.popups().card.is_none(), "no card for ghost text");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_word_then_the_rest_of_a_suggestion_are_taken() {
        let (mut editor, path) = offered("take", "let", "let total = sum(xs);\nprint(total)", 3);
        editor.on_key(&key(KeyCode::Char('f'), false, true));
        assert_eq!(editor.buffer().lines()[0], "let total");
        assert_eq!(
            editor.ghost_text().as_deref(),
            Some(" = sum(xs);\nprint(total)")
        );
        editor.on_key(&key(KeyCode::Tab, false, false));
        assert_eq!(
            editor.buffer().lines(),
            ["let total = sum(xs);", "print(total)"]
        );
        assert_eq!(
            (editor.buffer().row(), editor.buffer().col()),
            (1, 12),
            "after what went in"
        );
        assert!(editor.inline.ghost.is_none());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_a_line_taken_leaves_the_rest_showing_on_the_next_row() {
        let (mut editor, path) = offered("line", "a", "ab\ncd\nef", 1);
        editor.on_key(&key(KeyCode::Char('.'), true, false));
        editor.on_key(&key(KeyCode::Char('.'), true, false));
        assert_eq!(editor.buffer().lines(), ["ab", "cd"]);
        assert_eq!(
            editor.ghost_text().as_deref(),
            Some("\nef"),
            "the rest still shows on the new row"
        );
        editor.on_key(&key(KeyCode::Tab, false, false));
        assert_eq!(editor.buffer().lines(), ["ab", "cd", "ef"]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn test_c_g_hides_the_ghost_text_and_tab_then_indents_as_ever() {
        let (mut editor, path) = offered("hide", "x", "xyz", 1);
        editor.on_key(&key(KeyCode::Char('g'), true, false));
        assert_eq!(editor.ghost_text(), None);
        assert_eq!(editor.buffer().lines()[0], "x", "C-g took nothing");
        let _ = std::fs::remove_file(path);
    }
}

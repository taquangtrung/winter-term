//! Text with a fixed number of lines and the keys that edit it, for the
//! places that are not a file: a one-line answer to a question, or a column of
//! names, one per listed entry, being renamed in place.

use crate::model::input::Key;
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::{EditMode, EditOutcome, EditView, Marks, ModalEditor};
use crate::model::tool::ToolNotice;

// ========================================================================
// Data Structures
// ========================================================================

/// A field of text and the modal keys over it. Its lines are fixed, so
/// `Enter` while typing answers with [`EditOutcome::Submit`] rather than
/// breaking a line.
#[derive(Clone, Debug)]
pub struct TextField {
    buffer: TextBuffer,
    marks: Marks,
    modal: ModalEditor,
    /// The first line in view, which nothing outside the field reads.
    scroll: usize,
}

// ========================================================================
// TextField
// ========================================================================

impl TextField {
    /// A one-line field holding `text`, typed into from its end, where `Esc`
    /// withdraws the answer.
    pub fn line(text: &str) -> Self {
        let mut buffer = TextBuffer::new(vec![text.to_string()]).with_fixed_lines();
        buffer.move_line_end(true);
        Self {
            buffer,
            marks: Marks::default(),
            modal: ModalEditor::insert_only(),
            scroll: 0,
        }
    }

    /// A field of `lines` in Normal mode, the cursor on the last character
    /// of line `row`, which is where a rewrite of its tail starts.
    pub fn rows(lines: Vec<String>, row: usize) -> Self {
        let mut buffer = TextBuffer::new(lines).with_fixed_lines();
        buffer.move_to(row, usize::MAX);
        Self {
            buffer,
            marks: Marks::default(),
            modal: ModalEditor::new().with_mode(EditMode::Normal),
            scroll: 0,
        }
    }

    /// The text, one string per line.
    pub fn lines(&self) -> &[String] {
        self.buffer.lines()
    }

    /// The first line, which is the whole of a one-line field.
    pub fn text(&self) -> &str {
        self.buffer.lines().first().map_or("", String::as_str)
    }

    /// The line the cursor is on.
    pub fn row(&self) -> usize {
        self.buffer.row()
    }

    /// The character the cursor is on, or sits before while typing.
    pub fn col(&self) -> usize {
        self.buffer.col()
    }

    /// The keys' state, for what the host shows of it: the mode and what is
    /// part-typed.
    pub fn modal(&self) -> &ModalEditor {
        &self.modal
    }

    /// Replace a one-line field's text, the caret at its end, as one step
    /// `u` can walk back.
    pub fn set_text(&mut self, text: &str) {
        self.buffer.replace_rows(0, 1, vec![text.to_string()]);
        self.buffer.move_line_end(true);
    }

    /// Take what the last command reported, for the status bar.
    pub fn take_notice(&mut self) -> Option<ToolNotice> {
        self.modal.take_notice()
    }

    /// Put the cursor on line `row`, as the host does when something other
    /// than a key in the field moved it there.
    pub fn move_to_row(&mut self, row: usize) {
        if row != self.buffer.row() {
            let col = self.buffer.col();
            self.buffer.move_to(row, col);
        }
    }

    /// One key, in a view `viewport` lines tall.
    pub fn on_key(&mut self, key: &Key, viewport: usize) -> EditOutcome {
        let view = EditView {
            buffer: &mut self.buffer,
            marks: &mut self.marks,
            others: &[],
            scroll: &mut self.scroll,
            viewport,
        };
        self.modal.on_key(view, key)
    }

    /// Put text from the clipboard in at the cursor. Only its first line
    /// fits, since the field's lines are fixed.
    pub fn paste(&mut self, text: &str) {
        let first = text.lines().next().unwrap_or_default().to_string();
        let view = EditView {
            buffer: &mut self.buffer,
            marks: &mut self.marks,
            others: &[],
            scroll: &mut self.scroll,
            viewport: 1,
        };
        self.modal.paste_text(view, first);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::input::KeyCode;

    fn key(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn ctrl(c: char) -> Key {
        Key {
            ctrl: true,
            ..key(KeyCode::Char(c))
        }
    }

    fn alt(c: char) -> Key {
        Key {
            alt: true,
            ..key(KeyCode::Char(c))
        }
    }

    fn type_text(field: &mut TextField, text: &str) {
        for c in text.chars() {
            field.on_key(&key(KeyCode::Char(c)), 1);
        }
    }

    #[test]
    fn test_a_line_field_edits_with_the_emacs_chords_and_answers_on_enter() {
        let mut field = TextField::line("hello world");
        field.on_key(&ctrl('a'), 1);
        field.on_key(&alt('f'), 1);
        type_text(&mut field, ",");
        assert_eq!(field.text(), "hello, world");

        field.on_key(&ctrl('e'), 1);
        field.on_key(&ctrl('w'), 1);
        assert_eq!(field.text(), "hello, ");
        field.on_key(&ctrl('y'), 1);
        assert_eq!(field.text(), "hello, world", "the killed word yanks back");

        assert_eq!(field.on_key(&key(KeyCode::Enter), 1), EditOutcome::Submit);
        assert_eq!(field.on_key(&key(KeyCode::Escape), 1), EditOutcome::Cancel);
        assert_eq!(field.lines().len(), 1, "Enter never breaks the line");
    }

    #[test]
    fn test_a_pasted_line_lands_at_the_caret_and_the_caret_steps_past_it() {
        let mut field = TextField::line("ab");
        field.on_key(&ctrl('b'), 1);
        field.paste("XY\nignored");
        assert_eq!(field.text(), "aXYb");
        assert_eq!(field.col(), 3, "after what was pasted, before the b");
    }

    #[test]
    fn test_rows_keep_their_count_under_vim_line_commands() {
        let mut field = TextField::rows(vec!["one".into(), "two".into()], 0);
        type_text(&mut field, "dd");
        assert_eq!(field.lines(), ["", "two"], "a deleted line is emptied");
        type_text(&mut field, "jyykp");
        assert_eq!(
            field.lines(),
            ["two", "two"],
            "a yanked line pastes as text"
        );
        assert_eq!(
            field.on_key(&key(KeyCode::Char('o')), 1),
            EditOutcome::Consumed
        );
        assert_eq!(field.lines().len(), 2, "no line opens");
    }
}

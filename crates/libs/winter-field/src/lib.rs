//! Winter's text fields: a prompt's answer, a search box, a rename, each a
//! single-line Cherry document with a view of its own, in one Cherry editor
//! every field of the window's thread shares. Cherry edits them with Emacs
//! keys, as a text field of any GUI is edited: Enter answers, Escape and
//! `C-g` withdraw the answer, and the keys that move between lines (the
//! arrows up and down, the page keys) are left to whatever holds it.
//!
//! [`Rows`] is a field of several lines, one per listed entry, renamed in
//! place (wdired's names, grep's matched lines): a Cherry document with the
//! Vim modes, its lines fixed, so Enter while typing answers rather than
//! breaking a line, a key that deletes whole lines (`dd`) empties them in
//! place, and any other key that would add or take away a line is taken
//! back.
//!
//! What a field copies goes to the system clipboard, and is reported too
//! ([`EditOutcome::Yank`]), for the window to keep in its clipboard history.

#![deny(missing_docs)]
#![deny(unsafe_code)]

use std::cell::RefCell;
use std::fmt;
use std::sync::{Arc, Mutex};

use cherry::keys::{Key as CherryKey, KeyCode as CherryCode, Mods};
use cherry::{
    Clipboard, ClipboardSelection, DocumentId, Edit, Editor, EditorOptions, Handled, HostRequest,
    NoticeLevel, ViewId,
};
use misty::SystemClipboard;
use winter_cherry::{clock, EditorClipboard};
use winter_model::editor::Notice;
use winter_model::input::{Key, KeyCode};
use winter_model::textedit::modal::{EditMode, EditOutcome};

// ========================================================================
// Constants
// ========================================================================

/// The keys a field leaves to whatever holds it, pressed alone: they move
/// between lines, and a field has one (a prompt walks its earlier answers
/// with them, a list moves its selection).
const VERTICAL_KEYS: [KeyCode; 4] = [
    KeyCode::Down,
    KeyCode::PageDown,
    KeyCode::PageUp,
    KeyCode::Up,
];

/// What is said when a key would add or take away a line of [`Rows`].
const FIXED_LINES: &str = "one line per entry: lines are not added or taken away here";

thread_local! {
    /// The window thread's editor every field is a document of.
    static FIELDS: RefCell<FieldEditor> = RefCell::new(FieldEditor::new());
}

// ========================================================================
// Data Structures
// ========================================================================

/// A one-line field of text and the keys that edit it.
pub struct Field {
    /// The field's single-line document.
    document: DocumentId,
    /// What the last key reported, for the status bar.
    notice: Option<Notice>,
    /// The field's view of it.
    view: ViewId,
}

/// A field of several fixed lines, one per listed entry, and the Vim keys
/// that edit them.
pub struct Rows {
    /// The field's document.
    document: DocumentId,
    /// How many lines it has, which no key changes.
    lines: usize,
    /// What the last key reported, for the status bar.
    notice: Option<Notice>,
    /// The field's view of it.
    view: ViewId,
}

/// The editor every field is a document of, and what its clipboard was last
/// handed, not yet reported.
struct FieldEditor {
    copied: Arc<Mutex<Option<String>>>,
    editor: Editor,
}

/// The system clipboard as the field editor's, noting what it copies.
struct NotingClipboard {
    clipboard: EditorClipboard,
    copied: Arc<Mutex<Option<String>>>,
}

// ========================================================================
// Field Implementations
// ========================================================================

impl Field {
    /// A one-line field holding `text`, typed into from its end.
    pub fn line(text: &str) -> Self {
        with(|fields| {
            let document = fields.editor.open_single_line_document(text);
            let view = fields
                .editor
                .new_view(document)
                .expect("a document just opened takes a view");
            let end = fields
                .editor
                .document(document)
                .map_or(0, |text| text.len_chars());
            fields.editor.set_cursor(view, end);
            Self {
                document,
                notice: None,
                view,
            }
        })
    }

    /// The text.
    pub fn text(&self) -> String {
        with(|fields| {
            fields
                .editor
                .document(self.document)
                .and_then(|text| text.line_text(0))
                .unwrap_or_default()
        })
    }

    /// The character the cursor sits before.
    pub fn col(&self) -> usize {
        with(|fields| {
            fields
                .editor
                .view(self.view)
                .map_or(0, cherry::View::cursor)
        })
    }

    /// Replaces the text, the cursor at its end, as one step `C-/` can walk
    /// back.
    pub fn set_text(&mut self, text: &str) {
        with(|fields| {
            let length = fields
                .editor
                .document(self.document)
                .map_or(0, |text| text.len_chars());
            let edit = Edit::new(0..length, text);
            fields.editor.apply_edits(self.view, vec![edit]);
            let end = fields
                .editor
                .document(self.document)
                .map_or(0, |text| text.len_chars());
            fields.editor.set_cursor(self.view, end);
        });
    }

    /// Takes what the last key reported, for the status bar.
    pub fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    /// One key: what it asks of whatever holds the field.
    pub fn on_key(&mut self, key: &Key) -> EditOutcome {
        let alone = !(key.alt || key.ctrl || key.shift || key.sup);
        if alone && VERTICAL_KEYS.contains(&key.code) {
            return EditOutcome::Ignored;
        }
        let (handled, requests, copied) = with(|fields| {
            let handled = fields
                .editor
                .handle_key(self.view, to_cherry(key), clock::now());
            let requests = fields.editor.take_requests();
            let copied = fields
                .copied
                .lock()
                .ok()
                .and_then(|mut copied| copied.take());
            (handled, requests, copied)
        });
        let mut outcome = match handled {
            Handled::Ignored => EditOutcome::Ignored,
            _ => EditOutcome::Consumed,
        };
        for request in requests {
            match request {
                HostRequest::Cancel(_) => outcome = EditOutcome::Cancel,
                HostRequest::Submit(_) => outcome = EditOutcome::Submit,
                HostRequest::Notice(notice) => {
                    self.notice = Some(match notice.level {
                        NoticeLevel::Error => Notice::error(notice.message),
                        NoticeLevel::Warning => Notice::warning(notice.message),
                        _ => Notice::info(notice.message),
                    });
                }
                _ => {}
            }
        }
        match copied {
            Some(text) if outcome == EditOutcome::Consumed => EditOutcome::Yank(text),
            _ => outcome,
        }
    }

    /// Puts text from the clipboard in at the cursor: its first line, since
    /// the field is one line.
    pub fn paste(&mut self, text: &str) {
        let text = text.lines().next().unwrap_or_default();
        with(|fields| {
            let Some(cursor) = fields.editor.view(self.view).map(cherry::View::cursor) else {
                return;
            };
            let edit = Edit::new(cursor..cursor, text);
            fields.editor.apply_edits(self.view, vec![edit]);
        });
    }
}

impl Clone for Field {
    /// A field of its own, holding the same text, its cursor in the same
    /// place.
    fn clone(&self) -> Self {
        let mut field = Self::line(&self.text());
        let cursor = self.col();
        with(|fields| fields.editor.set_cursor(field.view, cursor));
        field.notice = self.notice.clone();
        field
    }
}

impl fmt::Debug for Field {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Field")
            .field("text", &self.text())
            .field("col", &self.col())
            .finish()
    }
}

impl Drop for Field {
    fn drop(&mut self) {
        with(|fields| {
            fields.editor.close_view(self.view);
            fields.editor.close_document(self.document);
        });
    }
}

// ========================================================================
// Rows Implementations
// ========================================================================

impl Rows {
    /// A field of `lines` in `mode`, the cursor on the last character of
    /// line `row`, which is where a rewrite of its tail starts.
    pub fn new(lines: Vec<String>, row: usize, mode: EditMode) -> Self {
        let count = lines.len().max(1);
        let rows = with(|fields| {
            let document = fields.editor.open_document(&lines.join("\n"));
            let view = fields
                .editor
                .new_view(document)
                .expect("a document just opened takes a view");
            Self {
                document,
                lines: count,
                notice: None,
                view,
            }
        });
        rows.put_cursor(row, usize::MAX);
        if mode == EditMode::Insert {
            with(|fields| {
                let _ = fields.editor.run_command(rows.view, "enter-insert", None);
            });
        }
        rows
    }

    /// The text, one string per line.
    pub fn lines(&self) -> Vec<String> {
        with(|fields| {
            let Some(text) = fields.editor.document(self.document) else {
                return Vec::new();
            };
            (0..text.len_lines())
                .filter_map(|line| text.line_text(line))
                .collect()
        })
    }

    /// The line the cursor is on.
    pub fn row(&self) -> usize {
        self.place().0
    }

    /// The character the cursor is on, or sits before while typing.
    pub fn col(&self) -> usize {
        self.place().1
    }

    /// The mode, as the status bar names it.
    pub fn mode(&self) -> EditMode {
        match self.cherry_mode() {
            Some(cherry::Mode::Insert | cherry::Mode::CommandLine) => EditMode::Insert,
            Some(cherry::Mode::Replace) => EditMode::Replace,
            Some(cherry::Mode::Visual | cherry::Mode::VisualBlock | cherry::Mode::VisualLine) => {
                EditMode::Visual
            }
            _ => EditMode::Normal,
        }
    }

    /// Whether keys type text into the field (not into its `:` line).
    pub fn is_typing(&self) -> bool {
        matches!(
            self.cherry_mode(),
            Some(cherry::Mode::Insert | cherry::Mode::Replace)
        )
    }

    /// Whether nothing is part-typed: Normal mode, no key of a sequence
    /// waiting on the rest.
    pub fn is_idle(&self) -> bool {
        let pending = with(|fields| {
            fields
                .editor
                .view(self.view)
                .is_some_and(|view| !view.pending_keys().is_empty())
        });
        self.cherry_mode() == Some(cherry::Mode::Normal) && !pending
    }

    /// The view's mode, as Cherry has it.
    fn cherry_mode(&self) -> Option<cherry::Mode> {
        with(|fields| fields.editor.view(self.view).map(cherry::View::mode))
    }

    /// Puts the cursor on line `row`, as the host does when something other
    /// than a key in the field moved it there.
    pub fn move_to_row(&mut self, row: usize) {
        let (at, column) = self.place();
        if row != at {
            self.put_cursor(row, column);
        }
    }

    /// Takes what the last key reported, for the status bar.
    pub fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    /// One key: what it asks of whatever holds the field.
    pub fn on_key(&mut self, key: &Key) -> EditOutcome {
        let alone = !(key.alt || key.ctrl || key.shift || key.sup);
        if alone && key.code == KeyCode::Enter && self.is_typing() {
            return EditOutcome::Submit;
        }
        let before = self.lines();
        let (handled, requests, copied) = with(|fields| {
            let handled = fields
                .editor
                .handle_key(self.view, to_cherry(key), clock::now());
            let requests = fields.editor.take_requests();
            let copied = fields
                .copied
                .lock()
                .ok()
                .and_then(|mut copied| copied.take());
            (handled, requests, copied)
        });
        self.keep_lines(&before);
        let mut outcome = match handled {
            Handled::Ignored => EditOutcome::Ignored,
            _ => EditOutcome::Consumed,
        };
        let mut saved = false;
        for request in requests {
            match request {
                HostRequest::Notice(notice) => {
                    self.notice = Some(match notice.level {
                        NoticeLevel::Error => Notice::error(notice.message),
                        NoticeLevel::Warning => Notice::warning(notice.message),
                        _ => Notice::info(notice.message),
                    });
                }
                HostRequest::Save(_) => {
                    saved = true;
                    outcome = EditOutcome::Save;
                }
                HostRequest::Quit(quit) => {
                    outcome = EditOutcome::Quit {
                        save: saved && !quit.force,
                    };
                }
                HostRequest::Cancel(_) => outcome = EditOutcome::Cancel,
                HostRequest::Submit(_) => outcome = EditOutcome::Submit,
                _ => {}
            }
        }
        match copied {
            Some(text) if outcome == EditOutcome::Consumed => EditOutcome::Yank(text),
            _ => outcome,
        }
    }

    /// Puts text from the clipboard in at the cursor: its first line, since
    /// the field's lines are fixed.
    pub fn paste(&mut self, text: &str) {
        let text = text.lines().next().unwrap_or_default();
        // Typing, it goes in where the cursor sits; otherwise after the
        // character the cursor is on, as `p` puts.
        let (row, column) = self.place();
        let typing = self.is_typing();
        with(|fields| {
            let Some(document) = fields.editor.document(self.document) else {
                return;
            };
            let length = document
                .line_text(row)
                .map_or(0, |line| line.chars().count());
            let column = match typing {
                true => column,
                false => (column + 1).min(length),
            };
            let Some(at) = document.position_to_char(cherry::Position { column, line: row }) else {
                return;
            };
            let edit = Edit::new(at..at, text);
            fields.editor.apply_edits(self.view, vec![edit]);
        });
    }

    /// Keeps the lines fixed after a key, the text having read `before`:
    /// whole lines it deleted are put back empty, and any other change to
    /// how many lines there are is taken back, saying why.
    fn keep_lines(&mut self, before: &[String]) {
        let after = self.lines();
        if after.len() == self.lines {
            return;
        }
        with(|fields| {
            let _ = fields.editor.run_command(self.view, "undo", None);
        });
        let Some(deleted) = deleted_lines(before, &after) else {
            self.notice = Some(Notice::warning(FIXED_LINES));
            return;
        };
        let emptied = vec![String::new(); deleted.len()].join("\n");
        with(|fields| {
            let Some(text) = fields.editor.document(self.document) else {
                return;
            };
            let last = deleted.end - 1;
            let from = text.position_to_char(cherry::Position {
                column: 0,
                line: deleted.start,
            });
            let to = text.line_text(last).and_then(|line| {
                let column = line.chars().count();
                text.position_to_char(cherry::Position { column, line: last })
            });
            if let (Some(from), Some(to)) = (from, to) {
                let edit = Edit::new(from..to, emptied);
                fields.editor.apply_edits(self.view, vec![edit]);
            }
        });
        self.put_cursor(deleted.start, 0);
    }

    /// Where the cursor is: its line, and its column in chars.
    fn place(&self) -> (usize, usize) {
        with(|fields| {
            let cursor = fields.editor.view(self.view)?.cursor();
            let at = fields
                .editor
                .document(self.document)?
                .char_to_position(cursor)?;
            Some((at.line, at.column))
        })
        .unwrap_or_default()
    }

    /// Puts the cursor on `row` at `column`, each clamped to the text, a
    /// column past the end landing on the line's last character.
    fn put_cursor(&self, row: usize, column: usize) {
        with(|fields| {
            let Some(text) = fields.editor.document(self.document) else {
                return;
            };
            let row = row.min(text.len_lines().saturating_sub(1));
            let length = text.line_text(row).map_or(0, |line| line.chars().count());
            let column = column.min(length.saturating_sub(1));
            let at = text
                .position_to_char(cherry::Position { column, line: row })
                .unwrap_or(0);
            fields.editor.set_cursor(self.view, at);
        });
    }
}

impl fmt::Debug for Rows {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Rows")
            .field("lines", &self.lines())
            .field("row", &self.row())
            .finish()
    }
}

impl Drop for Rows {
    fn drop(&mut self) {
        with(|fields| {
            fields.editor.close_view(self.view);
            fields.editor.close_document(self.document);
        });
    }
}

// ========================================================================
// FieldEditor Implementations
// ========================================================================

impl FieldEditor {
    /// An editor with Cherry's options, its clipboard the system's.
    fn new() -> Self {
        let copied = Arc::new(Mutex::new(None));
        let mut editor = Editor::new(EditorOptions::default());
        editor.set_clipboard(NotingClipboard {
            clipboard: EditorClipboard::new(SystemClipboard::new()),
            copied: Arc::clone(&copied),
        });
        Self { copied, editor }
    }
}

// ========================================================================
// NotingClipboard Implementations
// ========================================================================

impl Clipboard for NotingClipboard {
    fn read(&mut self, selection: ClipboardSelection) -> Option<String> {
        self.clipboard.read(selection)
    }

    fn write(&mut self, selection: ClipboardSelection, text: &str) {
        self.clipboard.write(selection, text);
        if let Ok(mut copied) = self.copied.lock() {
            *copied = Some(text.to_string());
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The lines `after` lost from `before` where a key deleted whole lines,
/// those around them as they were; `None` for any other change.
fn deleted_lines(before: &[String], after: &[String]) -> Option<std::ops::Range<usize>> {
    let lost = before
        .len()
        .checked_sub(after.len())
        .filter(|lost| *lost > 0)?;
    let start = before
        .iter()
        .zip(after)
        .position(|(was, now)| was != now)
        .unwrap_or(after.len());
    let kept = before[..start] == after[..start] && before[start + lost..] == after[start..];
    kept.then_some(start..start + lost)
}

/// `act` on the window thread's field editor.
fn with<R>(act: impl FnOnce(&mut FieldEditor) -> R) -> R {
    FIELDS.with(|fields| act(&mut fields.borrow_mut()))
}

/// `key` as Cherry names it.
pub fn to_cherry(key: &Key) -> CherryKey {
    let code = match key.code {
        KeyCode::Backspace => CherryCode::Backspace,
        KeyCode::Char(c) => CherryCode::Char(c),
        KeyCode::Delete => CherryCode::Delete,
        KeyCode::Down => CherryCode::Down,
        KeyCode::End => CherryCode::End,
        KeyCode::Enter => CherryCode::Enter,
        KeyCode::Escape => CherryCode::Esc,
        KeyCode::F(number) => CherryCode::F(number),
        KeyCode::Home => CherryCode::Home,
        KeyCode::Insert => CherryCode::Insert,
        KeyCode::Left => CherryCode::Left,
        KeyCode::PageDown => CherryCode::PageDown,
        KeyCode::PageUp => CherryCode::PageUp,
        KeyCode::Right => CherryCode::Right,
        KeyCode::Space => CherryCode::Char(' '),
        KeyCode::Tab => CherryCode::Tab,
        KeyCode::Up => CherryCode::Up,
    };
    let held = [
        (key.alt, Mods::ALT),
        (key.ctrl, Mods::CTRL),
        (key.shift, Mods::SHIFT),
        (key.sup, Mods::SUPER),
    ];
    let mods = held
        .into_iter()
        .filter(|(down, _)| *down)
        .fold(Mods::NONE, |mods, (_, held)| mods | held);
    CherryKey::new(code, mods)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// `code` pressed, with Ctrl when `ctrl` is set.
    fn press(code: KeyCode, ctrl: bool) -> Key {
        Key {
            alt: false,
            code,
            ctrl,
            shift: false,
            sup: false,
        }
    }

    /// Each character of `typed` pressed in `field`.
    fn type_in(field: &mut Field, typed: &str) {
        for c in typed.chars() {
            field.on_key(&press(KeyCode::Char(c), false));
        }
    }

    /// Each key of `typed` pressed in `rows`, each a plain character.
    fn type_rows(rows: &mut Rows, typed: &str) -> Vec<EditOutcome> {
        typed
            .chars()
            .map(|c| rows.on_key(&press(KeyCode::Char(c), false)))
            .collect()
    }

    #[test]
    fn test_rows_rename_a_line_in_place_and_keep_their_count() {
        let names = vec!["alpha.txt".to_string(), "beta.txt".to_string()];
        let mut rows = Rows::new(names, 1, EditMode::Normal);
        assert_eq!((rows.row(), rows.col()), (1, 7), "on the last character");
        assert!(rows.is_idle());
        type_rows(&mut rows, "0");
        type_rows(&mut rows, "c");
        assert!(!rows.is_idle(), "an operator waits on its motion");
        type_rows(&mut rows, "wgamma");
        assert!(rows.is_typing());
        assert_eq!(
            rows.on_key(&press(KeyCode::Enter, false)),
            EditOutcome::Submit,
            "Enter answers rather than breaking the line"
        );
        assert_eq!(rows.lines(), ["alpha.txt", "gamma.txt"]);
        rows.on_key(&press(KeyCode::Escape, false));
        type_rows(&mut rows, "kJ");
        assert_eq!(rows.lines(), ["alpha.txt", "gamma.txt"], "no lines joined");
        assert!(rows.take_notice().is_some(), "and why said");
        type_rows(&mut rows, "dd");
        assert_eq!(rows.lines(), ["", "gamma.txt"], "a deleted line left empty");
    }

    #[test]
    fn test_rows_save_with_w() {
        let mut rows = Rows::new(vec!["a".to_string()], 0, EditMode::Normal);
        let outcomes = type_rows(&mut rows, ":w");
        assert_eq!(outcomes.last(), Some(&EditOutcome::Consumed));
        assert_eq!(
            rows.on_key(&press(KeyCode::Enter, false)),
            EditOutcome::Save
        );
    }

    #[test]
    fn test_keys_keep_their_modifiers_and_space_and_escape_take_cherry_s_names() {
        let key = |code, ctrl, alt| Key {
            alt,
            code,
            ctrl,
            shift: false,
            sup: false,
        };
        let parsed = |spec| cherry::keys::parse_key(spec).expect("a valid key");
        assert_eq!(
            to_cherry(&key(KeyCode::Char('x'), true, true)),
            parsed("C-M-x")
        );
        assert_eq!(
            to_cherry(&key(KeyCode::Space, true, false)),
            parsed("C-SPC")
        );
        assert_eq!(
            to_cherry(&key(KeyCode::Escape, false, false)),
            parsed("<esc>")
        );
    }

    #[test]
    fn test_a_field_is_typed_into_from_its_end_and_enter_answers() {
        let mut field = Field::line("file");
        type_in(&mut field, ".rs");
        assert_eq!(field.text(), "file.rs");
        assert_eq!(field.col(), 7);
        field.on_key(&press(KeyCode::Char('a'), true));
        type_in(&mut field, "my_");
        assert_eq!(
            field.text(),
            "my_file.rs",
            "C-a goes to the start, as Emacs"
        );
        assert_eq!(
            field.on_key(&press(KeyCode::Enter, false)),
            EditOutcome::Submit
        );
    }

    #[test]
    fn test_escape_withdraws_and_the_arrows_are_left_to_the_holder() {
        let mut field = Field::line("query");
        assert_eq!(
            field.on_key(&press(KeyCode::Up, false)),
            EditOutcome::Ignored
        );
        assert_eq!(
            field.on_key(&press(KeyCode::Escape, false)),
            EditOutcome::Cancel
        );
        assert_eq!(field.text(), "query", "the text left as it was");
    }

    #[test]
    fn test_set_text_replaces_it_and_paste_takes_the_first_line() {
        let mut field = Field::line("old");
        field.set_text("new");
        assert_eq!((field.text().as_str(), field.col()), ("new", 3));
        field.paste(" a\nb");
        assert_eq!(field.text(), "new a");
        let copy = field.clone();
        drop(field);
        assert_eq!(copy.text(), "new a", "a clone outlives its original");
    }
}

//! Winter's text fields: a prompt's answer, a search box, a rename, each a
//! single-line Cherry document with a view of its own, in one Cherry editor
//! every field of the window's thread shares. Cherry edits them with Emacs
//! keys, as a text field of any GUI is edited: Enter answers, Escape and
//! `C-g` withdraw the answer, and the keys that move between lines (the
//! arrows up and down, the page keys) are left to whatever holds it.
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
use winter_model::textedit::modal::EditOutcome;

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

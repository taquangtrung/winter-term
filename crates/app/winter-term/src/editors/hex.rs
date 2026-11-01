//! Hex: a file's bytes, sixteen to a row, as an offset, the bytes in hex, and
//! the characters they read as.
//! `C-x C-q` turns editing on: a hex digit writes the half of the byte under
//! the cursor, `u` takes the last byte written back, and `C-s` (or `w`)
//! writes the file.

use std::path::{Path, PathBuf};

use winter_model::editor::{
    menu_groups, present, Editor, EditorMenuItem, EditorOutcome, EditorStatus, Notice, Page,
    PromptMode, PromptReply, PromptRequest, ScrollPosition,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::input::{Key, KeyCode, Mods};
use winter_model::list::ListView;
use winter_model::paint::{row_height, Caret, Frame, IconKind, Point, Row, Span, Style};
use winter_model::text_file;
use winter_model::vim::nav::{VimKey, VimNav};

// ========================================================================
// Constants
// ========================================================================

/// How many bytes each row shows.
const PER_ROW: usize = 16;

/// The columns before a row's first byte: the offset and a gap.
const OFFSET_COLS: usize = 10;

/// The largest file read, past which a hex view of it is no use to scroll.
const MAX_BYTES: u64 = 256 * 1024 * 1024;

/// The question an offset to go to answers.
const ASK_OFFSET: &str = "hex-offset";

/// The viewer's own keys, ahead of the row motions every list shares.
const KEYS: KeyTable<Act> = KeyTable {
    block: "hex",
    bindings: &[
        Binding {
            action: Act::ByteBefore,
            id: "byte_before",
            keys: &["h", "<left>"],
            label: "The byte before",
        },
        Binding {
            action: Act::ByteAfter,
            id: "byte_after",
            keys: &["l", "<right>"],
            label: "The byte after",
        },
        Binding {
            action: Act::RowStart,
            id: "row_start",
            keys: &["0"],
            label: "The row's first byte",
        },
        Binding {
            action: Act::RowEnd,
            id: "row_end",
            keys: &["$"],
            label: "The row's last byte",
        },
        Binding {
            action: Act::Top,
            id: "first_byte",
            keys: &["g g"],
            label: "The file's first byte",
        },
        Binding {
            action: Act::GotoOffset,
            id: "goto_offset",
            keys: &["g o", "M-g M-c"],
            label: "Go to an offset (decimal, or 0x hex)\u{2026}",
        },
        Binding {
            action: Act::ToggleEditing,
            id: "toggle_editing",
            keys: &["C-x C-q"],
            label: "Turn editing on, or off",
        },
        Binding {
            action: Act::Undo,
            id: "undo_byte",
            keys: &["u"],
            label: "While editing: take the last byte written back",
        },
        Binding {
            action: Act::Save,
            id: "save",
            keys: &["C-s", "w"],
            label: "Write the file",
        },
    ],
    configurable: true,
    title: "Hex",
};

/// Every key table of this surface, for the Keys editor to list.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

/// What typing a hex digit does while editing, for the help: typed text, not
/// a key of the table.
const DIGITS_HELP: (&str, &str) = (
    "0-9  a-f",
    "While editing: write the half of the byte under the cursor",
);

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the viewer's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    ByteAfter,
    ByteBefore,
    GotoOffset,
    RowEnd,
    RowStart,
    Save,
    ToggleEditing,
    Top,
    Undo,
}

/// A file's bytes in a tab.
#[derive(Debug)]
pub struct HexEditor {
    bytes: Vec<u8>,
    /// The byte the cursor is on.
    cursor: usize,
    /// Whether digits write bytes.
    editing: bool,
    /// Whether the next digit writes the byte's low half, the high one
    /// having been written.
    low: bool,
    list: ListView<usize>,
    nav: VimNav,
    notice: Option<Notice>,
    path: PathBuf,
    /// Whether the file's bytes were read. A file too large or unreadable
    /// shows nothing, and writing that nothing back would empty it.
    loaded: bool,
    /// The keys of a sequence typed so far (`C-x`, `g`), while they wait for
    /// the rest.
    pending: Vec<Key>,
    /// Each byte written, and what it was, the latest last.
    undo: Vec<(usize, u8)>,
    /// Whether the bytes differ from the file's.
    dirty: bool,
}

/// What one tab's view of the bytes keeps while another tab on them is the
/// one acted on: the byte the cursor is on and the rows scrolled to. The
/// bytes, what was written, and whether digits write are the file's, so
/// every tab on it shares them.
#[derive(Clone, Debug)]
struct HexView {
    cursor: usize,
    list: ListView<usize>,
    low: bool,
}

// ========================================================================
// HexEditor
// ========================================================================

impl HexEditor {
    /// The bytes of the file at `path`.
    pub fn new(path: PathBuf) -> Self {
        let (bytes, notice) = match std::fs::metadata(&path) {
            Ok(meta) if meta.len() > MAX_BYTES => (
                None,
                Some(Notice::error("hex: the file is too large to show")),
            ),
            _ => match std::fs::read(&path) {
                Ok(bytes) => (Some(bytes), None),
                Err(e) => (None, Some(Notice::error(format!("hex: {e}")))),
            },
        };
        let loaded = bytes.is_some();
        let bytes = bytes.unwrap_or_default();
        let mut list = ListView::new();
        list.set_keys((0..bytes.len().div_ceil(PER_ROW).max(1)).collect());
        Self {
            bytes,
            cursor: 0,
            editing: false,
            low: false,
            list,
            nav: VimNav::new(),
            loaded,
            notice,
            path,
            pending: Vec::new(),
            undo: Vec::new(),
            dirty: false,
        }
    }

    /// Put the cursor on byte `at`, the row following it.
    fn move_to(&mut self, at: usize) {
        self.cursor = at.min(self.bytes.len().saturating_sub(1));
        self.low = false;
        self.list.set_cursor(self.cursor / PER_ROW);
    }

    /// Write `digit` into the half of the byte under the cursor it goes to,
    /// moving on to the next byte once both halves are written.
    fn write_digit(&mut self, digit: u8) {
        let Some(byte) = self.bytes.get_mut(self.cursor) else {
            return;
        };
        if !self.low {
            self.undo.push((self.cursor, *byte));
        }
        *byte = match self.low {
            false => (digit << 4) | (*byte & 0x0f),
            true => (*byte & 0xf0) | digit,
        };
        self.dirty = true;
        match self.low {
            false => self.low = true,
            true => {
                let next = self.cursor + 1;
                self.move_to(next);
            }
        }
    }

    /// Write the bytes back to the file.
    fn save(&mut self) {
        if !self.loaded {
            self.notice = Some(Notice::error(
                "hex: the file was never read, so there is nothing to write",
            ));
            return;
        }
        match text_file::write_user_file(&self.path, &self.bytes) {
            Ok(_) => {
                self.dirty = false;
                self.notice = Some(Notice::info(format!("wrote {} bytes", self.bytes.len())));
            }
            Err(e) => self.notice = Some(Notice::error(format!("hex: {e}"))),
        }
    }

    /// A row's spans: its offset, its bytes (a gap after the eighth), and
    /// what they read as.
    fn row(&self, index: usize) -> Row {
        let start = index * PER_ROW;
        let end = (start + PER_ROW).min(self.bytes.len());
        let mut row = vec![Span::new(Style::Dim, format!("{start:08x}  "))];
        let mut hex = String::new();
        for (at, byte) in self.bytes[start..end].iter().enumerate() {
            if at == PER_ROW / 2 {
                hex.push(' ');
            }
            hex.push_str(&format!("{byte:02x} "));
        }
        let width = PER_ROW * 3 + 1;
        row.push(Span::plain(format!("{hex:width$}")));
        let text: String = self.bytes[start..end]
            .iter()
            .map(|byte| match byte.is_ascii_graphic() || *byte == b' ' {
                true => *byte as char,
                false => '.',
            })
            .collect();
        row.push(Span::new(Style::Dim, " │"));
        row.push(Span::plain(text));
        row.push(Span::new(Style::Dim, "│"));
        row
    }

    /// The column the cursor's byte, or its half being written, is drawn at.
    fn caret_col(&self) -> usize {
        let at = self.cursor % PER_ROW;
        let gap = usize::from(at >= PER_ROW / 2);
        OFFSET_COLS + at * 3 + gap + usize::from(self.low)
    }

    /// An offset typed as a number, decimal or `0x` hex.
    fn parse_offset(text: &str) -> Option<usize> {
        let text = text.trim();
        match text.strip_prefix("0x").or_else(|| text.strip_prefix("0X")) {
            Some(hex) => usize::from_str_radix(hex, 16).ok(),
            None => text.parse().ok(),
        }
    }
}

impl Page for HexEditor {
    /// Going to an offset, and turning editing on or off; while editing,
    /// taking the last byte back and writing the file.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        let key = |act: Act, label: &str| EditorMenuItem::bound_as(&KEYS, act, label);
        let toggle = match self.editing {
            true => "Turn Editing Off",
            false => "Turn Editing On",
        };
        let editing = present([
            key(Act::Undo, "Undo Byte").filter(|_| self.editing && !self.undo.is_empty()),
            key(Act::Save, "Write File").filter(|_| self.dirty),
        ]);
        menu_groups([
            present([key(Act::GotoOffset, "Go to Offset…")]),
            present([key(Act::ToggleEditing, toggle).filter(|_| self.loaded)]),
            editing,
        ])
    }

    fn title(&self) -> String {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        format!("Hex {name}")
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::Editor { kind: "hex" })
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        self.list.view_mut().set_rows(rows);
        let total = self.list_len();
        let placed = self.list.place(rows, |_| row_height("", cols, wrap, 0));
        let window = placed.window;
        let shown: Vec<Row> = (window.start..(window.start + window.count).min(total))
            .map(|index| self.row(index))
            .collect();
        placed.apply(Frame::new(shown), 0)
    }

    fn caret(&self) -> Option<Caret> {
        Some(Caret {
            col: self.caret_col(),
            insert: self.editing,
        })
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        // key-table: a hex digit while editing is typed text, not a command.
        if let (true, (Mods::NONE, KeyCode::Char(c))) =
            (self.editing && self.pending.is_empty(), key.chord())
        {
            if c.is_ascii_hexdigit() {
                self.write_digit(c.to_digit(16).unwrap_or(0) as u8);
                return EditorOutcome::Consumed;
            }
        }
        self.pending.push(*key);
        match table::lookup(&KEYS, &self.pending) {
            TableMatch::Prefix => return EditorOutcome::Consumed,
            TableMatch::Exact(act) => {
                self.pending.clear();
                if act != Act::Undo || self.editing {
                    return self.run(act);
                }
            }
            // The rest of a sequence that is none of the table's is dropped.
            TableMatch::Miss if self.pending.len() > 1 => {
                self.pending.clear();
                return EditorOutcome::Consumed;
            }
            TableMatch::Miss => self.pending.clear(),
        }
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                self.list.apply_motion(motion);
                let row = self.list.cursor();
                let at = row * PER_ROW + self.cursor % PER_ROW;
                self.cursor = at.min(self.bytes.len().saturating_sub(1));
                self.low = false;
            }
            VimKey::Pending => {}
            VimKey::Unhandled => return EditorOutcome::Ignored,
        }
        EditorOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        if let (ASK_OFFSET, Some(answer)) = (reply.tag, reply.answer) {
            match Self::parse_offset(&answer).filter(|at| *at < self.bytes.len().max(1)) {
                Some(at) => self.move_to(at),
                None => self.notice = Some(Notice::info("no such offset").unlogged()),
            }
        }
        EditorOutcome::Consumed
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.list.view_mut().set_margin(rows);
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.list.view_mut().wheel(lines);
        EditorOutcome::Consumed
    }

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.list.scroll_position()
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.list.scroll_to(offset);
        EditorOutcome::Consumed
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        let row = self.list.view().scroll() + at.row;
        let col = at.col.saturating_sub(OFFSET_COLS);
        let byte = (col - usize::from(col > (PER_ROW / 2) * 3)) / 3;
        if byte < PER_ROW {
            self.move_to(row * PER_ROW + byte);
        }
        EditorOutcome::Consumed
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let (keys, label) = DIGITS_HELP;
        let mut rows = table::help(&KEYS);
        rows.push(KeyHelp {
            keys: keys.to_string(),
            label: label.to_string(),
        });
        rows.extend(VimNav::help());
        rows
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn status(&self) -> EditorStatus {
        let byte = self.bytes.get(self.cursor).copied().unwrap_or(0);
        let mut segments = vec![
            format!("0x{:x} ({})", self.cursor, self.cursor),
            format!("{byte:02x} = {byte}"),
            format!("{} bytes", self.bytes.len()),
        ];
        if self.editing {
            segments.insert(0, "editing".to_string());
        }
        EditorStatus {
            mode: None,
            segments,
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }
}

impl Editor for HexEditor {
    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn holds_file(&self, path: &Path) -> bool {
        self.path == path
    }

    fn address(&self) -> Option<String> {
        Some(winter_model::path::abbreviate_home(&self.path))
    }

    fn snapshot(&self) -> Option<winter_model::editor::OpenTarget> {
        Some(winter_model::editor::OpenTarget::file(self.path.clone()))
    }

    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(HexView {
            cursor: self.cursor,
            list: self.list.clone(),
            low: self.low,
        }))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        let Ok(view) = state.downcast::<HexView>() else {
            return;
        };
        self.cursor = view.cursor;
        self.list = view.list;
        self.low = view.low;
    }
}

impl HexEditor {
    /// Carry out `act`, a key of the viewer's own.
    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::ByteAfter => self.move_to(self.cursor + 1),
            Act::ByteBefore => self.move_to(self.cursor.saturating_sub(1)),
            Act::GotoOffset => {
                return EditorOutcome::Prompt(PromptRequest {
                    initial: String::new(),
                    label: "Go to offset: ".to_string(),
                    mode: PromptMode::Text,
                    tag: ASK_OFFSET,
                    title: None,
                });
            }
            Act::RowEnd => self.move_to(self.cursor - self.cursor % PER_ROW + PER_ROW - 1),
            Act::RowStart => self.move_to(self.cursor - self.cursor % PER_ROW),
            Act::Save => self.save(),
            Act::ToggleEditing => {
                self.editing = !self.editing;
                let state = if self.editing { "on" } else { "off" };
                self.notice = Some(Notice::info(format!("editing {state}")).unlogged());
            }
            Act::Top => self.move_to(0),
            Act::Undo => match self.undo.pop() {
                Some((at, old)) => {
                    self.bytes[at] = old;
                    self.move_to(at);
                    self.dirty = !self.undo.is_empty();
                }
                None => self.notice = Some(Notice::info("nothing to take back").unlogged()),
            },
        }
        EditorOutcome::Consumed
    }

    /// How many rows the bytes take.
    fn list_len(&self) -> usize {
        self.bytes.len().div_ceil(PER_ROW).max(1)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use winter_model::paint::row_text;

    fn press(editor: &mut HexEditor, code: KeyCode, ctrl: bool) {
        editor.on_key(&Key {
            ctrl,
            ..Key::plain(code)
        });
    }

    #[test]
    fn test_writing_a_file_that_was_never_read_leaves_it_alone() {
        // Writing the empty bytes of a file too large or unreadable to show
        // would wipe it to nothing.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            let dir =
                std::env::temp_dir().join(format!("winter-hex-unread-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("dir");
            let path = dir.join("locked.bin");
            std::fs::write(&path, b"keep me").expect("file");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).expect("chmod");
            let mut editor = HexEditor::new(path.clone());
            press(&mut editor, KeyCode::Char('w'), false);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644))
                .expect("chmod back");
            assert_eq!(std::fs::read(&path).expect("reads"), b"keep me");
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[test]
    fn test_a_row_shows_the_offset_the_bytes_and_what_they_read_as() {
        let dir = std::env::temp_dir().join(format!("winter-hex-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("a.bin");
        std::fs::write(&path, b"Hello\x00\x01world, again!").expect("file");
        let mut editor = HexEditor::new(path.clone());
        let rows: Vec<String> = editor
            .content(5, 80, false)
            .rows
            .iter()
            .map(row_text)
            .collect();
        assert!(
            rows[0].starts_with("00000000  48 65 6c 6c 6f 00 01 77  6f"),
            "{}",
            rows[0]
        );
        assert!(rows[0].ends_with("│Hello..world, ag│"), "{}", rows[0]);
        assert!(rows[1].starts_with("00000010  61 69 6e 21"));
        press(&mut editor, KeyCode::Char('l'), false);
        assert_eq!(editor.caret().map(|caret| caret.col), Some(13));
        press(&mut editor, KeyCode::Char('4'), false);
        assert_eq!(editor.bytes[1], b'e', "not editing yet");
        press(&mut editor, KeyCode::Char('x'), true);
        press(&mut editor, KeyCode::Char('q'), true);
        press(&mut editor, KeyCode::Char('4'), false);
        press(&mut editor, KeyCode::Char('1'), false);
        assert_eq!(editor.bytes[1], b'A');
        assert_eq!(editor.cursor, 2, "on to the next byte");
        press(&mut editor, KeyCode::Char('u'), false);
        assert_eq!(editor.bytes[1], b'e');
        press(&mut editor, KeyCode::Char('4'), false);
        press(&mut editor, KeyCode::Char('1'), false);
        press(&mut editor, KeyCode::Char('s'), true);
        assert_eq!(std::fs::read(&path).expect("written")[..2], *b"HA");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_tab_s_cursor_comes_back_but_a_byte_written_in_another_stays() {
        // Two tabs on one file: each keeps its own cursor, and both see the
        // same bytes.
        let dir = std::env::temp_dir().join(format!("winter-hex-view-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("a.bin");
        std::fs::write(&path, [0u8; 64]).expect("file");
        let mut editor = HexEditor::new(path);
        let first = editor.save_view().expect("a view to keep");
        press(&mut editor, KeyCode::Char('j'), false);
        press(&mut editor, KeyCode::Char('x'), true);
        press(&mut editor, KeyCode::Char('q'), true);
        press(&mut editor, KeyCode::Char('f'), false);
        assert_eq!(editor.cursor, PER_ROW, "the other tab went down a row");
        editor.restore_view(first);
        assert_eq!(editor.cursor, 0, "and this one is where it was");
        assert_eq!(editor.list.cursor(), 0);
        assert_eq!(editor.bytes[PER_ROW], 0xf0, "the byte the other tab wrote");
        assert!(editor.editing, "editing is the file's, not the tab's");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_an_offset_is_read_in_decimal_or_hex() {
        assert_eq!(HexEditor::parse_offset("0x1f"), Some(31));
        assert_eq!(HexEditor::parse_offset("31"), Some(31));
        assert_eq!(HexEditor::parse_offset("zz"), None);
    }

    /// A viewer on a file of `bytes` in a directory of its own, `name`d.
    fn viewer(name: &str, bytes: &[u8]) -> (HexEditor, PathBuf) {
        let dir = std::env::temp_dir().join(format!("winter-hex-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("a.bin");
        std::fs::write(&path, bytes).expect("file");
        (HexEditor::new(path.clone()), path)
    }

    #[test]
    fn test_the_tables_keys_edit_take_back_and_write_a_byte() {
        let (mut editor, path) = viewer("keys", b"\x00\x00");
        press(&mut editor, KeyCode::Char('l'), false);
        assert_eq!(editor.cursor, 1);
        // `C-x C-q` is a sequence: the first key waits for the second.
        press(&mut editor, KeyCode::Char('x'), true);
        assert!(!editor.editing);
        press(&mut editor, KeyCode::Char('q'), true);
        assert!(editor.editing);
        // A hex digit, even `0`, is typed while editing, not the row-start key.
        press(&mut editor, KeyCode::Char('f'), false);
        press(&mut editor, KeyCode::Char('0'), false);
        assert_eq!(editor.bytes[1], 0xf0);
        press(&mut editor, KeyCode::Char('u'), false);
        assert_eq!(editor.bytes[1], 0x00, "taken back");
        press(&mut editor, KeyCode::Char('a'), false);
        press(&mut editor, KeyCode::Char('w'), false);
        assert_eq!(std::fs::read(&path).expect("reads")[1] >> 4, 0xa, "written");
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn test_g_o_asks_for_an_offset_and_a_rebound_key_does_instead() {
        let (mut editor, path) = viewer("goto", b"abc");
        let g = Key::plain(KeyCode::Char('g'));
        assert_eq!(editor.on_key(&g), EditorOutcome::Consumed);
        assert!(matches!(
            editor.on_key(&Key::plain(KeyCode::Char('o'))),
            EditorOutcome::Prompt(_)
        ));
        let ids = HashMap::from([("goto_offset".to_string(), vec!["C-g".to_string()])]);
        table::set_overrides(HashMap::from([("hex".to_string(), ids)]));
        let ctrl_g = Key {
            ctrl: true,
            ..Key::plain(KeyCode::Char('g'))
        };
        assert!(
            matches!(editor.on_key(&ctrl_g), EditorOutcome::Prompt(_)),
            "the user's key"
        );
        editor.on_key(&g);
        assert_eq!(
            editor.on_key(&Key::plain(KeyCode::Char('o'))),
            EditorOutcome::Consumed,
            "the default is gone"
        );
        table::set_overrides(HashMap::new());
        let _ = std::fs::remove_dir_all(path.parent().expect("dir"));
    }

    #[test]
    fn test_every_key_of_the_viewer_is_spelled_right() {
        assert_eq!(table::invalid_specs(&KEYS), Vec::<&str>::new());
    }
}

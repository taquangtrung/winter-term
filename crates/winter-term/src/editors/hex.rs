//! Hex: a file's bytes, sixteen to a row, as an offset, the bytes in hex, and
//! the characters they read as, as magic-vscode's hex editor shows them.
//! `C-x C-q` turns editing on: a hex digit writes the half of the byte under
//! the cursor, `u` takes the last byte written back, and `C-s` (or `w`)
//! writes the file.

use std::path::{Path, PathBuf};

use crate::editors::list::ListView;
use crate::editors::text_file;
use crate::model::editor::{
    Editor, EditorOutcome, EditorStatus, Notice, PromptMode, PromptReply, PromptRequest,
};
use crate::model::input::table::KeyHelp;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::{row_height, Caret, Frame, IconKind, Point, Row, Span, Style};
use crate::model::vim::nav::{VimKey, VimNav};

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

/// The keys, for the help the Keys view shows.
const HELP: &[(&str, &str)] = &[
    ("h  l  ←  →", "The byte before, the byte after"),
    ("j  k  C-d  C-u  gg  G", "Rows"),
    ("0  $", "The row's first byte, its last"),
    ("g o  M-g M-c", "Go to an offset (decimal, or 0x hex)..."),
    ("C-x C-q", "Turn editing on, or off"),
    ("0-9  a-f", "While editing: write the half of the byte under the cursor"),
    ("u", "While editing: take the last byte written back"),
    ("C-s  w", "Write the file"),
];

// ========================================================================
// Data Structures
// ========================================================================

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
    /// The first key of `C-x C-q`, `g o`, or `M-g M-c`, while it waits.
    pending: Option<Key>,
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
            Ok(meta) if meta.len() > MAX_BYTES => (None, Some(Notice::error("hex: the file is too large to show"))),
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
            pending: None,
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
            self.notice = Some(Notice::error("hex: the file was never read, so there is nothing to write"));
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

impl Editor for HexEditor {
    fn title(&self) -> String {
        let name = self.path.file_name().map_or_else(|| self.path.display().to_string(), |name| name.to_string_lossy().into_owned());
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
        let shown: Vec<Row> = (window.start..(window.start + window.count).min(total)).map(|index| self.row(index)).collect();
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
        if let Some(first) = self.pending.take() {
            match (first.chord(), key.chord()) {
                ((Mods::CTRL, KeyCode::Char('x')), (Mods::CTRL, KeyCode::Char('q'))) => {
                    self.editing = !self.editing;
                    let state = if self.editing { "on" } else { "off" };
                    self.notice = Some(Notice::info(format!("editing {state}")).unlogged());
                }
                ((Mods::NONE, KeyCode::Char('g')), (Mods::NONE, KeyCode::Char('o')))
                | ((Mods::ALT, KeyCode::Char('g')), (Mods::ALT, KeyCode::Char('c'))) => {
                    return EditorOutcome::Prompt(PromptRequest {
                        initial: String::new(),
                        label: "Go to offset: ".to_string(),
                        mode: PromptMode::Text,
                        tag: ASK_OFFSET,
                    });
                }
                ((Mods::NONE, KeyCode::Char('g')), (Mods::NONE, KeyCode::Char('g'))) => self.move_to(0),
                _ => {}
            }
            return EditorOutcome::Consumed;
        }
        match key.chord() {
            (Mods::CTRL, KeyCode::Char('x')) | (Mods::NONE | Mods::ALT, KeyCode::Char('g')) => self.pending = Some(*key),
            (Mods::CTRL, KeyCode::Char('s')) | (Mods::NONE, KeyCode::Char('w')) => self.save(),
            (Mods::NONE, KeyCode::Char('u')) if self.editing => match self.undo.pop() {
                Some((at, old)) => {
                    self.bytes[at] = old;
                    self.move_to(at);
                    self.dirty = !self.undo.is_empty();
                }
                None => self.notice = Some(Notice::info("nothing to take back").unlogged()),
            },
            (Mods::NONE, KeyCode::Char(c)) if self.editing && c.is_ascii_hexdigit() => {
                self.write_digit(c.to_digit(16).unwrap_or(0) as u8);
            }
            (Mods::NONE, KeyCode::Char('h') | KeyCode::Left) => {
                let at = self.cursor.saturating_sub(1);
                self.move_to(at);
            }
            (Mods::NONE, KeyCode::Char('l') | KeyCode::Right) => {
                let at = self.cursor + 1;
                self.move_to(at);
            }
            (Mods::NONE, KeyCode::Char('0')) => {
                let at = self.cursor - self.cursor % PER_ROW;
                self.move_to(at);
            }
            (Mods::NONE, KeyCode::Char('$')) => {
                let at = self.cursor - self.cursor % PER_ROW + PER_ROW - 1;
                self.move_to(at);
            }
            _ => match self.nav.row_key(key) {
                VimKey::Motion(motion) => {
                    self.list.apply_motion(motion);
                    let row = self.list.cursor();
                    let at = row * PER_ROW + self.cursor % PER_ROW;
                    self.cursor = at.min(self.bytes.len().saturating_sub(1));
                    self.low = false;
                }
                VimKey::Pending => {}
                VimKey::Unhandled => return EditorOutcome::Ignored,
            },
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
        HELP.iter()
            .map(|(keys, label)| KeyHelp {
                keys: keys.to_string(),
                label: label.to_string(),
            })
            .collect()
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
        EditorStatus { mode: None, segments }
    }

    fn is_dirty(&self) -> bool {
        self.dirty
    }

    fn holds_file(&self, path: &Path) -> bool {
        self.path == path
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn address(&self) -> Option<String> {
        Some(crate::model::path::abbreviate_home(&self.path))
    }

    fn snapshot(&self) -> Option<crate::model::editor::OpenTarget> {
        Some(crate::model::editor::OpenTarget::file(self.path.clone()))
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
    use super::*;
    use crate::model::paint::row_text;

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

            let dir = std::env::temp_dir().join(format!("winter-hex-unread-{}", std::process::id()));
            std::fs::create_dir_all(&dir).expect("dir");
            let path = dir.join("locked.bin");
            std::fs::write(&path, b"keep me").expect("file");
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o200)).expect("chmod");
            let mut editor = HexEditor::new(path.clone());
            press(&mut editor, KeyCode::Char('w'), false);
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod back");
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
        let rows: Vec<String> = editor.content(5, 80, false).rows.iter().map(row_text).collect();
        assert!(rows[0].starts_with("00000000  48 65 6c 6c 6f 00 01 77  6f"), "{}", rows[0]);
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
}

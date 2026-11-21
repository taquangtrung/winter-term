//! Hex: a file's bytes, sixteen to a row, as an offset, the bytes in hex, and
//! the characters they read as.
//! `C-x C-q` turns editing on: a hex digit writes the half of the byte under
//! the cursor, `u` takes the last byte written back, and `C-s` (or `w`)
//! writes the file.
//!
//! Misty draws the viewer as a view: the rows of bytes that fit around the
//! cursor, the cursor's byte lit in both columns, beside an inspector card
//! reading the bytes at the cursor as numbers and text (under the rows in
//! a narrow pane). A click on a byte puts the cursor there; the keys are
//! the viewer's own.

use std::ops::Range;
use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::layout::Align;
use misty::view::{Corners, Extent, badge, column, divider, item, page, row, section, text};
use misty::view::{Press, Space};
use misty::{
    AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect, Theme, ThemeToken,
};
use misty::{WheelDelta, WheelInput};

use winter_model::editor::{
    Editor, EditorMenuItem, EditorOutcome, EditorStatus, Notice, OwnView, PIXELS_PER_ROW, Page,
    PromptMode, PromptReply, PromptRequest, ViewSlot, menu_groups, present,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::input::{Key, KeyCode, Mods};
use winter_model::list::ListView;
use winter_model::paint::{Frame, IconKind, Row};
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

/// The text columns a pane needs to hold the inspector beside the rows
/// rather than under them.
const BESIDE_COLS: usize = 130;

/// How wide the inspector is beside the rows, in text columns.
const INSPECTOR_COLS: f32 = 40.0;

/// The pane's text rows the heading, the column headings, the padding, and
/// the footer take around the rows; and the inspector under them.
const CHROME_ROWS: usize = 9;
const INSPECTOR_ROWS: usize = 18;

/// What the inspector's byte and words mean, for their tooltips.
const BYTE_ABOUT: &str = "The byte in hex and in decimal, then read as a signed number";
const WORD_ABOUT: &str = "low byte first (little-endian) / high byte first (big-endian)";

/// What the heading's badges mean, for their tooltips.
const EDITING_ABOUT: &str = "Typing hex digits writes the byte at the cursor";
const READ_ONLY_ABOUT: &str = "The bytes are only shown; C-x C-q edits them";
const MODIFIED_ABOUT: &str = "Changed since it was read; w writes it";

/// The blank text columns at each side of a row.
const ROW_PAD_COLS: usize = 1;

/// What the headings over the rows say.
const HEADING: &str = "offset    00 01 02 03 04 05 06 07  08 09 0a 0b 0c 0d 0e 0f   ascii";

/// What stands for a byte that reads as no character.
const UNPRINTABLE: char = '.';

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
    /// Whether the pane is wide enough for the inspector beside the rows.
    beside: bool,
    /// How wide a text column was drawn last, in pixels, so a click finds
    /// its byte.
    cell_px: f32,
    /// The rows built and drawn: those around the cursor that fit.
    shown: Range<usize>,
    /// The page's view, which the viewer is the state of.
    view: ViewSlot<HexEditor>,
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
                Some(Notice::refusal("hex: the file is too large to show")),
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
            beside: false,
            cell_px: 0.0,
            shown: 0..0,
            view: ViewSlot::new(),
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
            self.notice = Some(Notice::refusal(
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

    /// Row `index` as the view shows it: its offset, its bytes (a gap after
    /// the eighth), and what they read as, the cursor's byte lit in both
    /// columns; a click on a byte puts the cursor on it.
    fn row_view(&self, index: usize) -> Element<'_, Self> {
        let start = index * PER_ROW;
        let end = (start + PER_ROW).min(self.bytes.len());
        let bytes = &self.bytes[start..end];
        let cursor = (start..end)
            .contains(&self.cursor)
            .then(|| self.cursor - start);
        let hex: Vec<String> = bytes
            .iter()
            .enumerate()
            .map(|(at, byte)| match at == PER_ROW / 2 {
                true => format!(" {byte:02x} "),
                false => format!("{byte:02x} "),
            })
            .collect();
        let chars: Vec<String> = bytes
            .iter()
            .map(|byte| shown_char(*byte).to_string())
            .collect();
        let pad = (PER_ROW * 3 + 1).saturating_sub(hex.iter().map(String::len).sum());
        let mut parts = vec![text(format!("{start:08x}  ")).muted().into()];
        parts.extend(self.lit_run(&hex, cursor, " ".repeat(pad)));
        parts.push(text(" \u{2502}").muted().into());
        parts.extend(self.lit_run(&chars, cursor, String::new()));
        parts.push(text("\u{2502}").muted().into());
        row(parts)
            .gap(Space::Cells(0.0))
            .align(Align::Start)
            .padding(ROW_PAD_COLS as f32, 0.0)
            .on_pointer_press(move |hex: &mut Self, press: Press| hex.click(index, press.point.x))
    }

    /// `pieces` as text, the one at `lit` (a byte's, of a row's bytes) on
    /// the cursor's color, with `tail` after.
    fn lit_run<'a>(
        &self,
        pieces: &[String],
        lit: Option<usize>,
        tail: String,
    ) -> Vec<Element<'a, Self>> {
        let Some(at) = lit else {
            return vec![text(pieces.concat() + &tail).into()];
        };
        let piece = &pieces[at];
        let shown = piece.trim();
        let lead = &piece[..piece.find(shown).unwrap_or(0)];
        let trail = &piece[lead.len() + shown.len()..];
        let tone = match self.editing {
            true => ThemeToken::Accent,
            false => ThemeToken::Selection,
        };
        vec![
            text(pieces[..at].concat() + lead).into(),
            row([text(shown.to_string()).into()])
                .width(Extent::Auto)
                .background(tone)
                .corners(Corners::Control),
            text(trail.to_string() + &pieces[at + 1..].concat() + &tail).into(),
        ]
    }

    /// A click on row `index`, `x` pixels from its left edge: the cursor
    /// goes to the byte there.
    fn click(&mut self, index: usize, x: f32) {
        self.list.view_mut().release();
        if self.cell_px <= 0.0 {
            return;
        }
        let col = (x / self.cell_px) as usize;
        if let Some(byte) = byte_at_col(col.saturating_sub(ROW_PAD_COLS)) {
            self.move_to(index * PER_ROW + byte);
        }
    }

    /// The card reading the bytes at the cursor: the offset, the byte in
    /// each base and as a character, and the words starting there both
    /// ways round; then the file's size and state.
    fn inspector(&self) -> Element<'_, Self> {
        let byte = self.bytes.get(self.cursor).copied().unwrap_or(0);
        let fact = |label: &'static str, value: String| item(label).trailing(text(value));
        let mut card = section("Inspector")
            .item(fact(
                "Offset",
                format!("0x{:x}  ({})", self.cursor, self.cursor),
            ))
            .item(
                fact("Byte", format!("0x{byte:02x}  {byte}  ({})", byte as i8)).tooltip(BYTE_ABOUT),
            )
            .item(fact("Binary", format!("{byte:08b}")))
            .item(fact("Character", shown_char(byte).to_string()));
        for (label, len) in [("16-bit LE / BE", 2), ("32-bit LE / BE", 4)] {
            if let Some((le, be)) = word_at(&self.bytes, self.cursor, len) {
                let about = format!("The {len} bytes from the cursor as a number: {WORD_ABOUT}");
                card = card.item(fact(label, format!("{le} / {be}")).tooltip(about));
            }
        }
        let state = match (self.editing, self.dirty) {
            (_, true) => "modified: w writes it",
            (true, false) => "editing: hex digits write the byte",
            (false, false) => "read only: C-x C-q edits",
        };
        let file = section("File")
            .item(fact("Size", format!("{} bytes", self.bytes.len())))
            .item(
                item(winter_model::path::abbreviate_home(&self.path))
                    .description(state)
                    .tooltip(self.path.display().to_string()),
            );
        column([card.into(), file.into()]).gap(Space::Cells(0.5))
    }

    /// Turn the rows by `wheel`, the cursor staying where it is.
    fn wheel(&mut self, wheel: &WheelInput) {
        let lines = match wheel.delta {
            WheelDelta::Lines(lines) => lines.y,
            WheelDelta::Pixels(pixels) => pixels.y / PIXELS_PER_ROW,
            // A kind of turn misty adds later scrolls nothing.
            _ => 0.0,
        };
        self.list.view_mut().wheel(lines.round() as isize);
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

impl HostedView for HexEditor {
    /// The heading saying whether the bytes are written and changed, over
    /// the rows that fit and the inspector beside them or under them.
    fn view(&self) -> Element<'_, Self> {
        let mut rows = vec![
            row([text(HEADING).muted().into()]).padding(ROW_PAD_COLS as f32, 0.0),
            divider(),
        ];
        let total = self.list_len();
        let shown = self.shown.start.min(total)..self.shown.end.min(total);
        rows.extend(shown.map(|index| self.row_view(index)));
        let rows = column(rows)
            .gap(Space::Cells(0.0))
            .grow(1.0)
            .on_scroll(|hex: &mut Self, wheel| hex.wheel(&wheel));
        let body = match self.beside {
            true => row([
                rows,
                column([self.inspector()])
                    .width(Extent::Cells(INSPECTOR_COLS))
                    .shrink(0.0),
            ])
            .align(Align::Start)
            .gap(Space::Cells(2.0)),
            false => column([rows, self.inspector()]).gap(Space::Cells(0.5)),
        };
        let mode = match self.editing {
            true => badge("editing", ThemeToken::Accent).tooltip(EDITING_ABOUT),
            false => badge("read only", ThemeToken::Muted).tooltip(READ_ONLY_ABOUT),
        };
        let mut shown = page(self.title()).action(mode);
        if self.dirty {
            let modified = badge("modified", ThemeToken::Warning).tooltip(MODIFIED_ABOUT);
            shown = shown.action(modified);
        }
        shown.content(body).into()
    }
}

impl OwnView for HexEditor {
    fn view_slot(&mut self) -> &mut ViewSlot<Self> {
        &mut self.view
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

    /// Misty draws the page; the grid under it is empty, its size kept to
    /// place the rows that fit round the cursor and the inspector.
    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.beside = cols >= BESIDE_COLS;
        let under = if self.beside { 0 } else { INSPECTOR_ROWS };
        let viewport = rows.saturating_sub(CHROME_ROWS + under).max(1);
        self.list.view_mut().set_rows(viewport);
        let placed = self.list.place(viewport, |_| 1);
        let window = placed.window;
        self.shown = window.start..window.start + window.count;
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.cell_px = frame.cell_metrics().width;
        self.draw_own_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        self.own_view_input(input, cx)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        self.own_access_action(action)
            .then_some(EditorOutcome::Consumed)
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.own_view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        // key-table: a hex digit while editing is typed text, not a command.
        if let (true, (Mods::NONE, KeyCode::Char(c))) =
            (self.editing && self.pending.is_empty(), key.chord())
            && c.is_ascii_hexdigit()
        {
            self.write_digit(c.to_digit(16).unwrap_or(0) as u8);
            return EditorOutcome::Consumed;
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

    fn crumb_path(&self) -> Option<PathBuf> {
        Some(self.path.clone())
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
// Functions
// ========================================================================

/// `byte` as the character column shows it: itself where it reads as a
/// character, [`UNPRINTABLE`] where it does not.
fn shown_char(byte: u8) -> char {
    match byte.is_ascii_graphic() || byte == b' ' {
        true => byte as char,
        false => UNPRINTABLE,
    }
}

/// Which of a row's bytes text column `col` of the row is on, in the hex
/// column (a gap after the eighth byte) or the character column; `None`
/// over the offset, the gaps, and the borders.
fn byte_at_col(col: usize) -> Option<usize> {
    let hex = col.checked_sub(OFFSET_COLS)?;
    let gap = PER_ROW / 2 * 3;
    let hex_cols = PER_ROW * 3 + 1;
    if hex < hex_cols {
        let byte = (hex - usize::from(hex > gap)) / 3;
        return (byte < PER_ROW).then_some(byte);
    }
    // The character column starts past the hex and its border.
    let chars = hex.checked_sub(hex_cols + 2)?;
    (chars < PER_ROW).then_some(chars)
}

/// The `len` bytes of `bytes` from `at` read as a number, least
/// significant first and most significant first; `None` past the end.
fn word_at(bytes: &[u8], at: usize, len: usize) -> Option<(u64, u64)> {
    let word = bytes.get(at..at.checked_add(len)?)?;
    let mut le = [0u8; 8];
    let mut be = [0u8; 8];
    le[..len].copy_from_slice(word);
    be[8 - len..].copy_from_slice(word);
    Some((u64::from_le_bytes(le), u64::from_be_bytes(be)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use misty::testing::TestWindow;

    use super::*;
    use crate::editors::view_tests::{drawn, text_of};

    fn press(editor: &mut HexEditor, code: KeyCode, ctrl: bool) {
        editor.on_key(&Key {
            ctrl,
            ..Key::plain(code)
        });
    }

    #[test]
    fn test_a_column_of_a_row_is_on_the_byte_drawn_there() {
        assert_eq!(byte_at_col(0), None, "the offset");
        assert_eq!(byte_at_col(OFFSET_COLS), Some(0));
        assert_eq!(byte_at_col(OFFSET_COLS + 3), Some(1));
        // The eighth byte, then the gap, then the ninth.
        assert_eq!(byte_at_col(OFFSET_COLS + 7 * 3), Some(7));
        assert_eq!(byte_at_col(OFFSET_COLS + 8 * 3 + 1), Some(8));
        let chars = OFFSET_COLS + PER_ROW * 3 + 1 + 2;
        assert_eq!(byte_at_col(chars), Some(0), "the character column");
        assert_eq!(byte_at_col(chars + PER_ROW - 1), Some(PER_ROW - 1));
        assert_eq!(byte_at_col(chars + PER_ROW), None, "the border");
    }

    #[test]
    fn test_a_word_reads_both_ways_round_and_not_past_the_end() {
        let bytes = [0x01, 0x02, 0x03, 0x04];
        assert_eq!(word_at(&bytes, 0, 2), Some((0x0201, 0x0102)));
        assert_eq!(word_at(&bytes, 0, 4), Some((0x0403_0201, 0x0102_0304)));
        assert_eq!(word_at(&bytes, 3, 2), None);
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
        let window = TestWindow::new(1400, 900);
        let shown = text_of(&drawn(&mut editor, &window));
        assert!(shown.contains("00000000  "), "{shown}");
        assert!(shown.contains("65 6c 6c 6f 00 01 77  6f"), "{shown}");
        assert!(shown.contains("ello..world, ag"), "{shown}");
        assert!(shown.contains("00000010  "), "{shown}");
        assert!(shown.contains("61 69 6e 21"), "{shown}");
        assert!(
            shown.contains("0x48  72"),
            "the inspector reads the first byte"
        );
        // A click on the second byte's hex, from the row's left edge, puts
        // the cursor there.
        let col = ROW_PAD_COLS + OFFSET_COLS + 3 + 1;
        editor.click(0, editor.cell_px * col as f32 + 1.0);
        assert_eq!(editor.cursor, 1);
        assert!(text_of(&drawn(&mut editor, &window)).contains("0x65  101"));
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

    #[test]
    fn test_every_control_says_what_it_is() {
        let (mut editor, _) = viewer("named", b"\x00\x01\x02");
        let frame = crate::editors::view_tests::drawn(&mut editor, &TestWindow::new(1200, 800));
        let unnamed = misty::testing::unnamed_controls(&frame);
        assert!(unnamed.is_empty(), "{unnamed:?}");
    }
}

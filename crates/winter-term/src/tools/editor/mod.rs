//! Editor: one file as editable text in a pane, opened from whichever tool
//! was pointing at it.
//!
//! - [`buffer`]: the text and the cursor over it.
//! - [`file`]: reading a file in and writing it back.
//! - [`keys`]: what every key does to either.
//! - [`search`]: finding text in the file.
//! - [`syntax`]: which run of characters is a comment, a string, a keyword.

pub mod buffer;
pub mod file;
pub mod keys;
pub mod search;
pub mod syntax;

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};

use crate::model::input::Key;
use crate::model::page::{
    row_height, wrap_window, wrapped_lines, OpenTarget, Page, PageCaret, PageContent, PageHint,
    PageOutcome, PagePoint, PageRow, PageSpan, PageStyle, PromptMode, PromptReply, PromptRequest,
};
use crate::model::vim::motion::{CursorMove, FindChar};
use crate::model::vim::nav::VimNav;

use buffer::{Register, TextBuffer};
use file::{FileShape, FileStamp};
use keys::{name_of_kind, Operator, Pending};
use search::Search;
use syntax::{LineState, Syntax, Token};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question the editor asks.
const ASK_BUFFER: &str = "buffer";
const ASK_DISCARD: &str = "discard";
const ASK_OVERWRITE: &str = "overwrite";
const ASK_SEARCH: &str = "search";
const ASK_SEARCH_BACK: &str = "search-back";

/// What marks a buffer holding edits that are not on disk, in the header
/// and in the list of open buffers.
const DIRTY_MARK: &str = " [+]";

/// Rows of header above the first line of text.
const HEADER_ROWS: usize = 1;

/// Width the line numbers are padded to, and the gap after them, so the text
/// starts in the same column whether the file has ten lines or ten thousand.
const NUMBER_WIDTH: usize = 4;
const NUMBER_GAP: &str = " ";

/// How many columns a tab occupies on screen. Display only: what is in the
/// file stays one tab character.
const TAB_WIDTH: usize = 4;

/// What a screenful means to the paging keys, as a fraction of the pane.
const HALF_PAGE: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// The files open for editing, the keys that act on them, and everything the
/// two share: one yank reaches every buffer, as it does in Vim.
#[derive(Debug)]
pub struct EditorPage {
    /// Where a Visual selection started, for as long as there is one.
    anchor: Option<Anchor>,
    /// Which of the open files the keys act on.
    at: usize,
    /// A count being typed ahead of a command, as digits so far.
    count: Option<usize>,
    /// Every file open in this editor, in the order they were opened.
    docs: Vec<Document>,
    /// The last char search, for `;` and `,` to do again.
    find: Option<FindChar>,
    /// The keys of the last command that changed the text, for `.`.
    last_change: Vec<Key>,
    /// What the last command reported, shown in the header until the next key.
    message: Option<String>,
    mode: EditMode,
    /// Buffer indices, most recently shown first, so a recency walk and the
    /// buffer list both read in the order the files were last worked on.
    mru: Vec<usize>,
    /// Cursor into the recency order while a walk is in progress, so repeated
    /// steps go on through it instead of restarting from the current buffer.
    mru_walk: Option<usize>,
    nav: VimNav,
    /// An operator waiting for the motion or object that says how far it
    /// reaches.
    operator: Option<Operator>,
    /// What each screen row of the last painted frame holds, so a click lands
    /// on the character under the pointer.
    painted: Vec<Painted>,
    /// A command part-way through being typed, waiting on its next key.
    pending: Option<Pending>,
    /// The keys of the command being typed, as they are pressed.
    record: Vec<Key>,
    /// The register the next command reads or writes, from a `"` prefix.
    register: Option<char>,
    /// The registers every buffer's yanks and deletes go to.
    registers: HashMap<char, Register>,
    /// Whether the keys arriving are a replay of the last change, so the
    /// replay is not recorded as a change of its own.
    replaying: bool,
    /// The text revision the command being typed started from, which is how a
    /// command that changed something is told from one that did not.
    revision: usize,
    /// What the last `/` or `?` looked for, which every buffer searches by.
    search: Option<Search>,
    /// How many rows of text the pane last had room for, so the paging keys
    /// can move by what is actually on screen.
    viewport: usize,
}

/// One open file: its text, where the cursor and the window sit in it, and
/// what it looked like on disk when it was read.
#[derive(Debug)]
struct Document {
    buffer: TextBuffer,
    /// The first column painted, for lines wider than the pane.
    hscroll: usize,
    /// Where `''` goes: where the cursor was before the last jump.
    jump: Option<(usize, usize)>,
    /// The marks `m` has set in this file, as the position each was set at.
    marks: HashMap<char, (usize, usize)>,
    path: PathBuf,
    scroll: usize,
    shape: FileShape,
    stamp: FileStamp,
    /// What each line begins inside, one entry per line worked out so far.
    /// Kept because a comment opened on one line colors every line until it
    /// closes, so a line cannot be colored without knowing what precedes it.
    states: Vec<LineState>,
    /// How the file's language spells the things worth coloring, or `None`
    /// for a file whose language this does not know.
    syntax: Option<&'static Syntax>,
}

/// Which of the Vim modes has the keyboard.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum EditMode {
    /// Typing goes into the text.
    Insert,
    /// Keys are commands.
    #[default]
    Normal,
    /// Typing goes into the text, over what is already there.
    Replace,
    /// Keys are commands, and they act on what is selected.
    Visual,
}

/// Where a Visual selection was started, which is the end of it the cursor is
/// not on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Anchor {
    col: usize,
    /// Whether the selection covers whole lines (`V` rather than `v`).
    linewise: bool,
    row: usize,
}

/// What one screen row of a painted frame holds: which line of the file, and
/// the display column its text begins at, which is the fold a wrapped line
/// picks up from or the scroll a wide one is read through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Painted {
    line: usize,
    offset: usize,
}

// ========================================================================
// EditorPage: state
// ========================================================================

impl EditorPage {
    /// Open `path`'s contents for editing, landing on `line` when whatever
    /// asked for the file knew which one mattered.
    pub fn new(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        Ok(Self {
            anchor: None,
            at: 0,
            count: None,
            docs: vec![Document::open(path, line)?],
            find: None,
            last_change: Vec::new(),
            message: None,
            mode: EditMode::Normal,
            mru: vec![0],
            mru_walk: None,
            nav: VimNav::new(),
            operator: None,
            painted: Vec::new(),
            pending: None,
            record: Vec::new(),
            register: None,
            registers: HashMap::new(),
            replaying: false,
            revision: 0,
            search: None,
            viewport: 0,
        })
    }

    /// The file the keys are acting on.
    fn doc(&self) -> &Document {
        &self.docs[self.at.min(self.docs.len() - 1)]
    }

    /// The same file, to change.
    fn doc_mut(&mut self) -> &mut Document {
        let at = self.at.min(self.docs.len() - 1);
        &mut self.docs[at]
    }

    /// Open `path` as another buffer of this editor, or go to it where it is
    /// already open, which is what makes a second file a second buffer rather
    /// than a second editor.
    pub fn open(&mut self, path: PathBuf, line: Option<usize>) {
        if let Some(at) = self.docs.iter().position(|doc| doc.path == path) {
            self.select_buffer(at);
            if let Some(line) = line {
                self.buffer_mut().move_to_line(line.saturating_sub(1));
            }
            return;
        }
        match Document::open(path, line) {
            Ok(doc) => {
                self.docs.push(doc);
                self.select_buffer(self.docs.len() - 1);
            }
            Err(e) => self.message = Some(format!("{e}")),
        }
    }

    /// Show buffer `at` as a deliberate choice: it becomes the most recently
    /// used, which also ends any recency walk in progress.
    pub(super) fn select_buffer(&mut self, at: usize) {
        self.touch_mru(at);
        self.show(at);
    }

    /// Move `at` to the front of the recency order, ending any walk.
    fn touch_mru(&mut self, at: usize) {
        self.mru.retain(|&i| i != at);
        self.mru.insert(0, at);
        self.mru_walk = None;
    }

    /// Step through the buffers in recency order: `forward` comes back toward
    /// the most recent, its opposite goes further back. Walking over one does
    /// not make it recent, so repeated steps go on rather than bouncing.
    pub(super) fn recent_buffer(&mut self, forward: bool) {
        let count = self.docs.len();
        if count <= 1 {
            return;
        }
        // Guard against any drift from open/close bookkeeping: a malformed
        // order is rebuilt with the buffer being read most-recent.
        if self.mru.len() != count {
            self.mru = (0..count).collect();
            self.touch_mru(self.at);
        }
        let cursor = self.mru_walk.unwrap_or(0);
        let next = match forward {
            true => (cursor + count - 1) % count,
            false => (cursor + 1) % count,
        };
        self.mru_walk = Some(next);
        self.show(self.mru[next]);
    }

    /// Put buffer `at` in front of the keys, with nothing half-typed carried
    /// over from the one being left. The recency order is untouched, so a walk
    /// passing through keeps its place.
    fn show(&mut self, at: usize) {
        // The unnamed register follows the eye rather than staying with the
        // file it was filled from: a yank in one buffer puts in the next.
        let register = self.buffer().register().clone();
        self.at = at.min(self.docs.len().saturating_sub(1));
        self.anchor = None;
        self.count = None;
        self.operator = None;
        self.pending = None;
        self.mode = EditMode::Normal;
        self.revision = self.buffer().revision();
        self.buffer_mut().set_register(register);
        self.buffer_mut().clamp(false);
        // Coming back to a buffer is coming back to the file: one written to
        // while it was in the background is re-read, as on_resume does it.
        self.reread();
    }

    /// How many files are open in this editor.
    pub(super) fn buffer_count(&self) -> usize {
        self.docs.len()
    }

    /// Which of them the keys are acting on.
    pub(super) fn buffer_at(&self) -> usize {
        self.at
    }

    /// How each open buffer reads in a list: its path, and whether it holds
    /// edits that are not on disk.
    pub(super) fn buffer_list(&self) -> Vec<String> {
        // Most recently used first, so the file being bounced to and from sits
        // at the top rather than wherever it happened to be opened. An index
        // the order has drifted out of step with is dropped rather than
        // panicking on it, and anything the order missed is appended.
        let named = |doc: &Document| match doc.buffer.is_dirty() {
            true => format!("{}{DIRTY_MARK}", doc.path.display()),
            false => doc.path.display().to_string(),
        };
        let mut listed: Vec<String> = self
            .mru
            .iter()
            .filter_map(|&i| self.docs.get(i).map(named))
            .collect();
        listed.extend(
            self.docs
                .iter()
                .enumerate()
                .filter(|(i, _)| !self.mru.contains(i))
                .map(|(_, doc)| named(doc)),
        );
        listed
    }

    /// The text under the cursor.
    fn buffer(&self) -> &TextBuffer {
        &self.doc().buffer
    }

    /// The same text, to change.
    fn buffer_mut(&mut self) -> &mut TextBuffer {
        &mut self.doc_mut().buffer
    }

    /// The count typed before a command, defaulting to one, and spent in the
    /// reading: a count belongs to the command that follows it.
    fn take_count(&mut self) -> usize {
        self.count.take().unwrap_or(1)
    }

    /// Write the text back, unless something else has written to the file
    /// since it was opened, which is worth a question rather than a save.
    fn save(&mut self) -> PageOutcome {
        if file::is_stale(&self.doc().path, &self.doc().stamp) {
            return PageOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: format!(
                    "{} changed on disk. Overwrite? (y/n) ",
                    name_of(&self.doc().path)
                ),
                mode: PromptMode::Confirm,
                tag: ASK_OVERWRITE,
            });
        }
        self.write()
    }

    /// Write the text back whatever the file on disk now says.
    fn write(&mut self) -> PageOutcome {
        match file::save(&self.doc().path, self.buffer().lines(), self.doc().shape) {
            Ok(stamp) => {
                self.doc_mut().stamp = stamp;
                self.buffer_mut().mark_saved();
                let wrote = lines_written(self.buffer());
                self.message = Some(format!("wrote {wrote}"));
            }
            Err(e) => self.message = Some(format!("could not write: {e}")),
        }
        PageOutcome::Consumed
    }

    /// Close the buffer being read, asking first when that would drop edits
    /// that were never written: the one thing an editor cannot let a stray
    /// key do. The page goes with the last buffer.
    fn leave(&mut self) -> PageOutcome {
        if !self.buffer().is_dirty() {
            return self.close_buffer();
        }
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!(
                "{} has unsaved edits. Discard? (y/n) ",
                name_of(&self.doc().path)
            ),
            mode: PromptMode::Confirm,
            tag: ASK_DISCARD,
        })
    }

    /// Drop the buffer being read and show the one before it, or close the
    /// page where it was the only one open.
    fn close_buffer(&mut self) -> PageOutcome {
        if self.docs.len() <= 1 {
            return PageOutcome::Close;
        }
        let gone = name_of(&self.docs.remove(self.at).path);
        // The indices above the hole all shift down by one, so the order has
        // to be renumbered or a later walk lands on the wrong file.
        let closed = self.at;
        self.mru.retain(|&i| i != closed);
        for i in self.mru.iter_mut() {
            if *i > closed {
                *i -= 1;
            }
        }
        self.select_buffer(self.at.saturating_sub(1));
        self.message = Some(format!("closed {gone}"));
        PageOutcome::Consumed
    }

    /// Hand the file to `$EDITOR` in a tab of its own, at the line under the
    /// cursor, for the work this editor deliberately cannot do. Unsaved edits
    /// would not be there, so they go to disk first.
    fn escalate(&mut self) -> PageOutcome {
        if self.buffer().is_dirty() {
            self.write();
        }
        PageOutcome::SpawnEditor(OpenTarget::at_line(
            self.doc().path.clone(),
            self.buffer().row() + 1,
        ))
    }
}

// ========================================================================
// Document
// ========================================================================

impl Document {
    /// Read `path` in, landing on `line` where whatever asked for the file
    /// knew which one mattered.
    fn open(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        let loaded = file::load(&path)?;
        let mut buffer = TextBuffer::new(loaded.lines);
        if let Some(line) = line {
            buffer.move_to_line(line.saturating_sub(1));
        }
        let syntax = syntax::of_path(&path);
        Ok(Self {
            buffer,
            hscroll: 0,
            jump: None,
            marks: HashMap::new(),
            path,
            scroll: 0,
            shape: loaded.shape,
            stamp: loaded.stamp,
            states: vec![LineState::default()],
            syntax,
        })
    }
}

// ========================================================================
// EditorPage: painting
// ========================================================================

impl EditorPage {
    /// What the header says: the file, whether it has unsaved edits, where the
    /// cursor is, and whatever the last command reported.
    fn header_row(&self) -> PageRow {
        let mut spans = vec![PageSpan::new(PageStyle::Header, name_of(&self.doc().path))];
        if self.buffer().is_dirty() {
            spans.push(PageSpan::new(PageStyle::Accent, DIRTY_MARK));
        }
        if self.docs.len() > 1 {
            spans.push(PageSpan::new(
                PageStyle::Dim,
                format!("  {} of {}", self.at + 1, self.docs.len()),
            ));
        }
        spans.push(PageSpan::new(
            PageStyle::Dim,
            format!(
                "  {}:{}  {} lines",
                self.buffer().row() + 1,
                self.buffer().col() + 1,
                self.buffer().lines().len()
            ),
        ));
        if let Some(name) = mode_name(self.mode) {
            spans.push(PageSpan::new(PageStyle::Accent, format!("  {name}")));
        }
        if let Some(register) = self.register {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  \"{register}")));
        }
        if let Some(message) = &self.message {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  {message}")));
        }
        spans
    }

    /// One line of the file: its number, then as much of its text as the pane
    /// has room for. The caret is the host's to draw, on the cell this page
    /// reports, so it takes the shape and the color every other caret in the
    /// window has.
    fn text_row(&self, index: usize, cols: usize) -> PageRow {
        let number = PageSpan::new(
            PageStyle::Dim,
            format!("{:>NUMBER_WIDTH$}{NUMBER_GAP}", index + 1),
        );
        let line = self.line_of(index);
        let colors = self.colors_of(index, &line);
        // Tabs are opened out for the screen, and every column they open into
        // carries what the tab itself was, so a run of color survives one.
        let mut cells: Vec<(char, Token)> = Vec::new();
        for (offset, c) in line.chars().enumerate() {
            let token = colors.get(offset).copied().unwrap_or_default();
            match c == '\t' {
                true => cells.extend(std::iter::repeat_n((' ', token), TAB_WIDTH)),
                false => cells.push((c, token)),
            }
        }
        let mut row = vec![number];
        let from = cells.len().min(self.doc().hscroll);
        let marked = self
            .selected_columns(index)
            .map(|(start, end)| (start.saturating_sub(from), end.saturating_sub(from)));
        row.extend(runs(&cells[from..], cols, marked));
        row
    }

    /// The display columns of line `index` a Visual selection covers, as a
    /// half-open range, or `None` where it covers none of them.
    fn selected_columns(&self, index: usize) -> Option<(usize, usize)> {
        let selection = self.selection()?;
        if index < selection.start.0 || index > selection.end.0 {
            return None;
        }
        let width = expanded(&self.line_of(index)).len();
        if selection.linewise {
            return Some((0, width.max(1)));
        }
        let start = match index == selection.start.0 {
            true => self.display_of(index, selection.start.1),
            false => 0,
        };
        let end = match index == selection.end.0 {
            true => self.display_of(index, selection.end.1 + 1),
            false => width.max(1),
        };
        Some((start, end))
    }

    /// What each character of a line is, for the language the file is in.
    /// A file in no language this knows is one long run of plain text.
    fn colors_of(&self, index: usize, line: &str) -> Vec<Token> {
        let Some(syntax) = self.doc().syntax else {
            return Vec::new();
        };
        let chars: Vec<char> = line.chars().collect();
        let state = self.doc().states.get(index).copied().unwrap_or_default();
        syntax::highlight(&chars, syntax, state).tokens
    }

    /// Work out what every line up to `upto` leaves open, as far as it is not
    /// already known. A line's color depends on every line above it, so this
    /// is the price of the first look at a part of the file; what it learns is
    /// kept until an edit makes it untrue.
    fn learn_states(&mut self, upto: usize) {
        if let Some(changed) = self.buffer_mut().take_change() {
            self.doc_mut().states.truncate(changed + 1);
        }
        let Some(syntax) = self.doc().syntax else {
            return;
        };
        if let Some((open, close)) = syntax.block() {
            // Taken out so the lines can be read while it is written to, and
            // one buffer of characters serves every line rather than one
            // allocation apiece.
            let mut states = std::mem::take(&mut self.doc_mut().states);
            let lines = self.buffer().lines();
            let mut chars: Vec<char> = Vec::new();
            while states.len() <= upto && states.len() < lines.len() {
                let index = states.len() - 1;
                let line = &lines[index];
                let state = states[index];
                // Only a line that opens or closes one of these can leave the
                // next line anywhere new, so the rest are passed over without
                // being read character by character.
                let next = match line.contains(open) || line.contains(close) {
                    true => {
                        chars.clear();
                        chars.extend(line.chars());
                        syntax::next_state(&chars, syntax, state)
                    }
                    false => state,
                };
                states.push(next);
            }
            self.doc_mut().states = states;
            return;
        }
        // Nothing in the language runs past its own line, so every line
        // begins where the last one did.
        self.doc_mut().states.resize(upto + 1, LineState::default());
    }

    /// The painted cell the cursor is on: the gutter's width, plus how far
    /// into the line it sits once tabs are opened out and the scroll taken
    /// off.
    fn caret_col(&self) -> usize {
        NUMBER_WIDTH + NUMBER_GAP.len() + self.display_col().saturating_sub(self.doc().hscroll)
    }

    fn line_of(&self, index: usize) -> String {
        self.buffer()
            .lines()
            .get(index)
            .cloned()
            .unwrap_or_default()
    }

    /// The column the cursor paints at, which is not the column it edits at
    /// whenever a tab precedes it.
    fn display_col(&self) -> usize {
        self.display_of(self.buffer().row(), self.buffer().col())
    }

    /// The column character `col` of line `index` paints at, once the tabs
    /// before it are opened out.
    fn display_of(&self, index: usize, col: usize) -> usize {
        self.line_of(index)
            .chars()
            .take(col)
            .map(|c| if c == '\t' { TAB_WIDTH } else { 1 })
            .sum()
    }

    /// The character of line `index` painted at display column `display`,
    /// which is what a click on that cell is pointing at.
    fn char_at_display(&self, index: usize, display: usize) -> usize {
        let mut at = 0;
        for (col, c) in self.line_of(index).chars().enumerate() {
            if at >= display {
                return col;
            }
            at += if c == '\t' { TAB_WIDTH } else { 1 };
        }
        self.buffer().line_len_of(index)
    }

    /// Keep the cursor's column on screen for a line wider than the pane.
    /// With wrapping on there is nothing to scroll: every column is painted.
    fn follow_cursor(&mut self, cols: usize, wrap: bool) {
        if wrap || cols == 0 {
            self.doc_mut().hscroll = 0;
            return;
        }
        let col = self.display_col();
        if col < self.doc().hscroll {
            self.doc_mut().hscroll = col;
        } else if col >= self.doc().hscroll + cols {
            self.doc_mut().hscroll = col + 1 - cols;
        }
    }
}

// ========================================================================
// EditorPage: the pane it is read through
// ========================================================================

impl EditorPage {
    /// Keep what was painted where, so a click can be answered with the
    /// character that was under the pointer.
    fn remember_layout(&mut self, lines: &[usize], cols: usize, wrap: bool) {
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        let mut painted = vec![
            Painted {
                line: lines.first().copied().unwrap_or(0),
                offset: 0,
            };
            HEADER_ROWS
        ];
        for line in lines {
            let chars = expanded(&self.line_of(*line));
            for (start, _) in wrapped_lines(&chars, cols, wrap, gutter) {
                painted.push(Painted {
                    line: *line,
                    offset: match wrap {
                        true => start,
                        false => self.doc().hscroll,
                    },
                });
            }
        }
        self.painted = painted;
    }

    /// Put the cursor where the pointer is, and select from where it was
    /// pressed while it is dragged.
    fn point_at(&mut self, at: PagePoint) -> PageOutcome {
        let Some(spot) = self.painted.get(at.row).copied() else {
            return PageOutcome::Ignored;
        };
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        let display = spot.offset + at.col.saturating_sub(gutter);
        let col = self.char_at_display(spot.line, display);
        if at.drag {
            if self.anchor.is_none() {
                self.start_visual(false);
            }
        } else {
            self.anchor = None;
            if self.mode == EditMode::Visual {
                self.mode = EditMode::Normal;
            }
        }
        self.buffer_mut().move_to(spot.line, col);
        PageOutcome::Consumed
    }

    /// Read the file again where it has changed underneath an editor with
    /// nothing unsaved in it: what is on screen would otherwise be a copy of
    /// something that is no longer there.
    fn reread(&mut self) -> PageOutcome {
        if self.buffer().is_dirty() || !file::is_stale(&self.doc().path, &self.doc().stamp) {
            return PageOutcome::Consumed;
        }
        let Ok(loaded) = file::load(&self.doc().path) else {
            return PageOutcome::Consumed;
        };
        let (row, col) = (self.buffer().row(), self.buffer().col());
        self.doc_mut().buffer = TextBuffer::new(loaded.lines);
        self.buffer_mut().move_to(row, col);
        self.doc_mut().shape = loaded.shape;
        self.doc_mut().stamp = loaded.stamp;
        self.doc_mut().states = vec![LineState::default()];
        self.anchor = None;
        self.mode = EditMode::Normal;
        self.revision = self.buffer().revision();
        self.message = Some("changed on disk, re-read".to_string());
        PageOutcome::Consumed
    }
}

impl Page for EditorPage {
    fn title(&self) -> String {
        name_of(&self.doc().path)
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
        let text_cols = cols.saturating_sub(NUMBER_WIDTH + NUMBER_GAP.len());
        self.viewport = rows.saturating_sub(HEADER_ROWS);
        self.follow_cursor(text_cols, wrap);
        let total = self.buffer().lines().len();
        let window = wrap_window(
            self.doc().scroll,
            self.buffer().row(),
            total,
            self.viewport,
            |index| {
                row_height(
                    &expanded_text(&self.line_of(index)),
                    text_cols,
                    wrap,
                    NUMBER_WIDTH + NUMBER_GAP.len(),
                )
            },
        );
        self.doc_mut().scroll = window.start;
        self.learn_states(window.start + window.count);
        let mut page_rows = vec![self.header_row()];
        let lines: Vec<usize> = (window.start..(window.start + window.count).min(total)).collect();
        page_rows.extend(lines.iter().map(|index| self.text_row(*index, text_cols)));
        self.remember_layout(&lines, text_cols, wrap);
        // A wrapped line picks up under its own text rather than under the
        // line numbers, so the gutter stays a gutter.
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        let indents: Vec<usize> = (0..page_rows.len())
            .map(|row| if row < HEADER_ROWS { 0 } else { gutter })
            .collect();
        PageContent::new(page_rows)
            .with_cursor_line(HEADER_ROWS + window.cursor)
            .with_wrap_indents(indents)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        self.dispatch(key)
    }

    fn on_mouse(&mut self, at: PagePoint) -> PageOutcome {
        self.point_at(at)
    }

    fn on_scroll(&mut self, lines: isize) -> PageOutcome {
        // The window follows the cursor, so a page that scrolls carries the
        // cursor with it rather than leaving it behind off screen.
        match lines > 0 {
            true => self.buffer_mut().move_up(lines.unsigned_abs(), false),
            false => self.buffer_mut().move_down(lines.unsigned_abs(), false),
        }
        PageOutcome::Consumed
    }

    fn on_paste(&mut self, text: String) -> PageOutcome {
        self.paste_text(text)
    }

    fn on_resume(&mut self) -> PageOutcome {
        self.reread()
    }

    fn open_file(&mut self, target: OpenTarget) -> bool {
        // A file opened while an editor is up joins it as another buffer,
        // rather than covering it with an editor of its own.
        self.open(target.path, target.line);
        true
    }

    fn caret(&self) -> Option<PageCaret> {
        Some(PageCaret {
            col: self.caret_col(),
            insert: matches!(self.mode, EditMode::Insert | EditMode::Replace),
        })
    }

    fn hint(&self) -> Option<PageHint> {
        let pairs: Vec<(&str, String)> = match self.pending? {
            Pending::Bracket(_) => vec![("b", "the next open buffer".to_string())],
            Pending::Find(_) => vec![("<char>", "to the next one on this line".to_string())],
            Pending::Goto => vec![
                ("b", "list the open buffers".to_string()),
                ("g", "the first line".to_string()),
                ("e", "the end of the word before".to_string()),
                ("_", "the last character with ink on it".to_string()),
            ],
            Pending::Jump(_) => vec![
                ("<letter>", "to that mark".to_string()),
                ("' or `", "back to where the last jump started".to_string()),
            ],
            Pending::Leave => vec![
                ("Z", "write and close".to_string()),
                ("Q", "close, discarding edits".to_string()),
            ],
            Pending::Mark => vec![("<letter>", "name this place".to_string())],
            Pending::Motion => {
                let name = self.operator.map(name_of_kind).unwrap_or("act");
                vec![
                    ("<motion>", format!("{name} over it")),
                    ("i or a", format!("{name} a text object")),
                    ("same key", format!("{name} whole lines")),
                ]
            }
            Pending::Object(_) => vec![
                ("w or W", "a word".to_string()),
                ("p or s", "a paragraph, a sentence".to_string()),
                ("\" ' ` ( [ {", "what they hold".to_string()),
            ],
            Pending::Register => vec![
                ("<letter>", "use that register".to_string()),
                ("+", "use the system clipboard".to_string()),
            ],
            Pending::Replace => vec![("<char>", "replace with it".to_string())],
            Pending::Scroll => vec![
                ("z", "this line to the middle".to_string()),
                ("t", "this line to the top".to_string()),
                ("b", "this line to the bottom".to_string()),
            ],
        };
        Some(PageHint {
            items: pairs
                .into_iter()
                .map(|(key, what)| (key.to_string(), what))
                .collect(),
            title: "editor".to_string(),
        })
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        let said_yes = reply.answer.is_some();
        match reply.tag {
            ASK_BUFFER => {
                if let Some(chosen) = reply.answer {
                    let path = chosen.trim_end_matches(DIRTY_MARK);
                    if let Some(at) = self
                        .docs
                        .iter()
                        .position(|doc| doc.path.to_string_lossy() == path)
                    {
                        self.select_buffer(at);
                    }
                }
                PageOutcome::Consumed
            }
            ASK_DISCARD if said_yes => self.close_buffer(),
            ASK_OVERWRITE if said_yes => self.write(),
            ASK_SEARCH | ASK_SEARCH_BACK => match reply.answer {
                Some(pattern) => self.start_search(pattern, reply.tag == ASK_SEARCH),
                None => PageOutcome::Consumed,
            },
            _ => PageOutcome::Consumed,
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The buffer being edited, not the one the editor opened with, so
        // stepping through buffers takes the answer with it.
        self.doc().path.parent().map(PathBuf::from)
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// The file's own name, which is what identifies it in a tab or a header; the
/// directory it sits in is the pane's, and already on screen.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

/// A line with its tabs opened out to the columns they occupy on screen.
fn expanded(line: &str) -> Vec<char> {
    let mut out = Vec::new();
    for c in line.chars() {
        if c == '\t' {
            out.extend(std::iter::repeat_n(' ', TAB_WIDTH));
        } else {
            out.push(c);
        }
    }
    out
}

fn expanded_text(line: &str) -> String {
    expanded(line).into_iter().collect()
}

/// Move `buffer`'s cursor as `motion` says. The screen motions and the paging
/// ones measure against the pane, so they need what it is showing. `typed` is
/// the count that was typed before the motion, which a few of them read as
/// something other than how many times to do it.
pub(super) fn move_cursor(
    buffer: &mut TextBuffer,
    motion: CursorMove,
    typed: Option<usize>,
    page: usize,
    scroll: usize,
) {
    let count = typed.unwrap_or(1);
    match motion {
        CursorMove::Left => buffer.move_left(count),
        CursorMove::Right => buffer.move_right(count, false),
        CursorMove::Down => buffer.move_down(count, false),
        CursorMove::Up => buffer.move_up(count, false),
        CursorMove::LineStart => buffer.move_line_start(),
        CursorMove::LineEnd => buffer.move_line_end(false),
        CursorMove::FirstNonBlank => buffer.move_first_non_blank(),
        CursorMove::LastNonBlank => buffer.move_last_non_blank(),
        CursorMove::WordForward => buffer.move_word_forward(count, false),
        CursorMove::WordForwardBig => buffer.move_word_forward(count, true),
        CursorMove::WordBack => buffer.move_word_back(count, false),
        CursorMove::WordBackBig => buffer.move_word_back(count, true),
        CursorMove::WordEnd => buffer.move_word_end(count, false),
        CursorMove::WordEndBig => buffer.move_word_end(count, true),
        CursorMove::Top => buffer.move_top(),
        // `20G` is line twenty; `G` on its own is the last line.
        CursorMove::Bottom => match typed {
            Some(line) => buffer.move_to_line(line.saturating_sub(1)),
            None => buffer.move_bottom(),
        },
        CursorMove::PageDown => buffer.move_down(page, false),
        CursorMove::PageUp => buffer.move_up(page, false),
        CursorMove::HalfPageDown => buffer.move_down(page / HALF_PAGE, false),
        CursorMove::HalfPageUp => buffer.move_up(page / HALF_PAGE, false),
        CursorMove::ScreenTop => buffer.move_to_line(scroll),
        CursorMove::ScreenMiddle => buffer.move_to_line(scroll + page / HALF_PAGE),
        CursorMove::ScreenBottom => buffer.move_to_line(scroll + page.saturating_sub(1)),
        CursorMove::ParagraphBack => buffer.move_paragraph(count, false),
        CursorMove::ParagraphForward => buffer.move_paragraph(count, true),
        CursorMove::MatchingBracket => buffer.move_matching_bracket(),
        CursorMove::WordEndBack => buffer.move_word_end_back(count, false),
        CursorMove::WordEndBackBig => buffer.move_word_end_back(count, true),
        // Where the cursor's line sits in the pane is the page's to answer,
        // since the text knows nothing of the window it is read through.
        CursorMove::LineToBottom | CursorMove::LineToCenter | CursorMove::LineToTop => {}
    }
}

/// The painted spans for a row's cells: one span per run of cells sharing a
/// color, cut to what the pane has room for. One span per character would be
/// correct and would cost the renderer a shaping run for every letter.
///
/// A selected run takes the theme's selection colors instead of the ones its
/// syntax would have, the way selected text is marked everywhere else.
fn runs(cells: &[(char, Token)], cols: usize, marked: Option<(usize, usize)>) -> PageRow {
    let mut spans: Vec<PageSpan> = Vec::new();
    for (at, (c, token)) in cells.iter().take(cols).enumerate() {
        let selected = marked.is_some_and(|(start, end)| at >= start && at < end);
        let style = match selected {
            true => PageStyle::Marked,
            false => style_of(*token),
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.text.push(*c),
            _ => spans.push(PageSpan::new(style, c.to_string())),
        }
    }
    // A selection that reaches past the line's last character shows as one
    // marked cell there, so an empty line in a linewise selection is visible.
    if let Some((start, end)) = marked {
        if cells.len() < end && cells.len() >= start && cells.len() < cols {
            spans.push(PageSpan::new(PageStyle::Marked, " ".to_string()));
        }
    }
    spans
}

/// What the header calls the mode, for the modes worth naming: Normal is the
/// one the editor is in whenever it is not saying otherwise.
fn mode_name(mode: EditMode) -> Option<&'static str> {
    match mode {
        EditMode::Insert => Some("INSERT"),
        EditMode::Normal => None,
        EditMode::Replace => Some("REPLACE"),
        EditMode::Visual => Some("VISUAL"),
    }
}

/// How each kind of thing is painted. The page names what a run is and never
/// what color it takes, which is what lets the theme decide.
fn style_of(token: Token) -> PageStyle {
    match token {
        Token::Comment => PageStyle::SyntaxComment,
        Token::Keyword => PageStyle::SyntaxKeyword,
        Token::Normal => PageStyle::Normal,
        Token::Number => PageStyle::SyntaxNumber,
        Token::String => PageStyle::SyntaxString,
        Token::Type => PageStyle::SyntaxType,
    }
}

/// `n lines`, singular where it should be.
fn lines_written(buffer: &TextBuffer) -> String {
    match buffer.lines().len() {
        1 => "1 line".to_string(),
        n => format!("{n} lines"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use crate::model::input::KeyCode;

    /// A temporary directory that removes itself on drop, so a test that
    /// writes files never leaks state into the next run.
    struct TempDir(PathBuf);

    impl TempDir {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("winter-editor-page-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        /// A file in the directory, for opening as another buffer.
        fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, text).expect("temp file");
            path
        }

        /// A page over a file holding `text`, opened at its first line.
        fn page(&self, text: &str) -> EditorPage {
            let path = self.0.join("a.txt");
            std::fs::write(&path, text).expect("temp file");
            EditorPage::new(path, None).expect("opens")
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

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
            alt: false,
            code: KeyCode::Char(c),
            ctrl: true,
            shift: false,
        }
    }

    /// Type `keys` as bare characters, the way a user would.
    fn press(page: &mut EditorPage, keys: &str) {
        for c in keys.chars() {
            page.on_key(&key(KeyCode::Char(c)));
        }
    }

    /// The keys of a command, with `<esc>` for the one key a test cannot
    /// spell as a character.
    fn type_keys(page: &mut EditorPage, keys: &str) {
        let mut rest = keys;
        while !rest.is_empty() {
            if let Some(tail) = rest.strip_prefix("<esc>") {
                page.on_key(&key(KeyCode::Escape));
                rest = tail;
                continue;
            }
            let c = rest.chars().next().expect("a key");
            page.on_key(&key(KeyCode::Char(c)));
            rest = &rest[c.len_utf8()..];
        }
    }

    /// A page that has been painted once, so what it painted is known: a
    /// click, a paging key, and `zz` all measure against the pane.
    fn painted_page(page: &mut EditorPage, rows: usize) {
        page.content(rows, 40, false);
    }

    #[test]
    fn test_the_motions_that_used_to_do_nothing_now_move() {
        let tmp = TempDir::new("motions");
        let mut page = tmp.page("fn one() {\n    body()\n}\n\nfn two() {\n}\n");

        // `}` to the blank line past the first run of text, `{` back again.
        press(&mut page, "}");
        assert_eq!(page.buffer().row(), 3, "the blank line");
        press(&mut page, "{");
        assert_eq!(page.buffer().row(), 0, "and back to the top");

        // `%` between a bracket and its match, lines apart.
        press(&mut page, "$");
        assert_eq!((page.buffer().row(), page.buffer().col()), (0, 9));
        press(&mut page, "%");
        assert_eq!(
            (page.buffer().row(), page.buffer().col()),
            (2, 0),
            "the closing brace"
        );
        press(&mut page, "%");
        assert_eq!((page.buffer().row(), page.buffer().col()), (0, 9), "back");

        // `ge` to the end of the word before.
        press(&mut page, "j$");
        press(&mut page, "ge");
        assert_eq!((page.buffer().row(), page.buffer().col()), (1, 7), "`body`");
    }

    #[test]
    fn test_zz_puts_the_cursor_line_in_the_middle_of_the_pane() {
        let tmp = TempDir::new("scroll");
        let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let mut page = tmp.page(&text);
        painted_page(&mut page, 11);
        press(&mut page, "20G");
        painted_page(&mut page, 11);
        press(&mut page, "zz");
        assert_eq!(page.doc().scroll, 19 - 5, "the line sits five rows down");
        press(&mut page, "zt");
        assert_eq!(page.doc().scroll, 19, "and on the top row");
    }

    #[test]
    fn test_a_new_line_keeps_the_indent_of_the_one_it_was_split_from() {
        // A line that starts back at column zero is the indentation retyped
        // every time, in code that is nothing but indented.
        let tmp = TempDir::new("indent");
        let mut page = tmp.page("    let x = 1;\n");
        press(&mut page, "$a");
        page.on_key(&key(KeyCode::Enter));
        press(&mut page, "y");
        assert_eq!(page.buffer().lines(), ["    let x = 1;", "    y"]);
    }

    #[test]
    fn test_an_operator_takes_the_word_object_under_the_cursor() {
        let tmp = TempDir::new("objects");

        let mut inner = tmp.page("alpha beta gamma\n");
        press(&mut inner, "wdiw");
        assert_eq!(inner.buffer().lines(), ["alpha  gamma"], "`diw`");

        let mut around = tmp.page("alpha beta gamma\n");
        press(&mut around, "wdaw");
        assert_eq!(
            around.buffer().lines(),
            ["alpha gamma"],
            "`daw` the blank too"
        );
    }

    #[test]
    fn test_the_quote_and_bracket_objects_reach_inside_what_holds_them() {
        let tmp = TempDir::new("objects-held");

        let mut quoted = tmp.page("say \"hello there\" now\n");
        type_keys(&mut quoted, "ci\"gone<esc>");
        assert_eq!(quoted.buffer().lines(), ["say \"gone\" now"]);

        let mut nested = tmp.page("call(one, two)\n");
        press(&mut nested, "fodi(");
        assert_eq!(nested.buffer().lines(), ["call()"], "`di(` from inside");

        let mut around = tmp.page("call(one, two)\n");
        press(&mut around, "foda(");
        assert_eq!(around.buffer().lines(), ["call"], "`da(` takes the pair");
    }

    #[test]
    fn test_a_paragraph_object_takes_whole_lines() {
        let tmp = TempDir::new("paragraph");
        let mut page = tmp.page("one\ntwo\n\nthree\n");
        press(&mut page, "dap");
        assert_eq!(page.buffer().lines(), ["three"], "the run and its blank");
    }

    #[test]
    fn test_a_visual_selection_is_what_the_next_operator_acts_on() {
        let tmp = TempDir::new("visual");

        let mut chars = tmp.page("alpha beta\n");
        press(&mut chars, "vlld");
        assert_eq!(chars.buffer().lines(), ["ha beta"], "three cells taken");
        assert_eq!(chars.mode, EditMode::Normal, "and the selection is over");

        let mut lines = tmp.page("one\ntwo\nthree\n");
        press(&mut lines, "Vjd");
        assert_eq!(lines.buffer().lines(), ["three"], "two whole lines");

        let mut objects = tmp.page("alpha beta\n");
        press(&mut objects, "viwy");
        assert_eq!(
            objects.buffer().register().text,
            "alpha",
            "`viw` selects it"
        );
    }

    #[test]
    fn test_visual_o_swaps_which_end_of_the_selection_moves() {
        let tmp = TempDir::new("visual-o");
        let mut page = tmp.page("alphabet\n");
        press(&mut page, "llvll");
        assert_eq!(
            (page.buffer().col(), page.selection().unwrap().start.1),
            (4, 2)
        );
        press(&mut page, "o");
        assert_eq!(page.buffer().col(), 2, "the cursor is on the other end");
        press(&mut page, "hd");
        assert_eq!(page.buffer().lines(), ["abet"], "which is what moved");
    }

    #[test]
    fn test_the_indent_operators_shift_whole_lines() {
        let tmp = TempDir::new("shift");
        let mut page = tmp.page("one\ntwo\n");
        press(&mut page, ">>");
        assert_eq!(page.buffer().lines(), ["    one", "two"]);
        press(&mut page, "<<");
        assert_eq!(page.buffer().lines(), ["one", "two"]);
        press(&mut page, "Vj>");
        assert_eq!(
            page.buffer().lines(),
            ["    one", "    two"],
            "over a selection"
        );
    }

    #[test]
    fn test_the_line_tail_commands_change_and_delete_to_the_end() {
        let tmp = TempDir::new("tail");

        let mut deleted = tmp.page("alpha beta\n");
        press(&mut deleted, "wD");
        assert_eq!(deleted.buffer().lines(), ["alpha "]);

        let mut changed = tmp.page("alpha beta\n");
        type_keys(&mut changed, "wCtwo<esc>");
        assert_eq!(changed.buffer().lines(), ["alpha two"]);

        let mut swapped = tmp.page("alpha\n");
        type_keys(&mut swapped, "sb<esc>");
        assert_eq!(swapped.buffer().lines(), ["blpha"], "`s` types over one");

        let mut whole = tmp.page("alpha\nbeta\n");
        type_keys(&mut whole, "Sgone<esc>");
        assert_eq!(
            whole.buffer().lines(),
            ["gone", "beta"],
            "`S` the whole line"
        );
    }

    #[test]
    fn test_replace_mode_writes_over_what_is_already_there() {
        let tmp = TempDir::new("replace");
        let mut page = tmp.page("alpha\n");
        type_keys(&mut page, "Rbet<esc>");
        assert_eq!(page.buffer().lines(), ["betha"]);
        assert_eq!(page.mode, EditMode::Normal);
    }

    #[test]
    fn test_ctrl_a_and_ctrl_x_move_the_number_under_the_cursor() {
        let tmp = TempDir::new("numbers");
        let mut page = tmp.page("port 8080\n");
        page.on_key(&ctrl('a'));
        assert_eq!(page.buffer().lines(), ["port 8081"]);
        press(&mut page, "5");
        page.on_key(&ctrl('x'));
        assert_eq!(page.buffer().lines(), ["port 8076"], "with a count");
    }

    #[test]
    fn test_a_char_search_moves_to_the_character_and_repeats() {
        let tmp = TempDir::new("find");
        let mut page = tmp.page("one, two, three\n");
        press(&mut page, "f,");
        assert_eq!(page.buffer().col(), 3);
        press(&mut page, ";");
        assert_eq!(page.buffer().col(), 8, "the next one");
        press(&mut page, ",");
        assert_eq!(page.buffer().col(), 3, "and back to the one before");
        press(&mut page, "0dt,");
        assert_eq!(page.buffer().lines(), [", two, three"], "`dt` stops short");
    }

    #[test]
    fn test_a_named_register_keeps_what_was_yanked_into_it() {
        let tmp = TempDir::new("registers");
        let mut page = tmp.page("one\ntwo\n");
        press(&mut page, "\"ayy");
        press(&mut page, "jdd");
        assert_eq!(page.buffer().lines(), ["one"], "the unnamed take");
        press(&mut page, "\"ap");
        assert_eq!(page.buffer().lines(), ["one", "one"], "and the named one");
    }

    #[test]
    fn test_the_clipboard_register_leaves_the_page() {
        let tmp = TempDir::new("clipboard");
        let mut page = tmp.page("one\ntwo\n");
        page.on_key(&key(KeyCode::Char('"')));
        page.on_key(&key(KeyCode::Char('+')));
        press(&mut page, "y");
        let outcome = page.on_key(&key(KeyCode::Char('y')));
        assert_eq!(outcome, PageOutcome::Yank("one".to_string()));
    }

    #[test]
    fn test_the_dot_key_does_the_last_change_again() {
        let tmp = TempDir::new("dot");

        let mut page = tmp.page("aaaa\n");
        press(&mut page, "x..");
        assert_eq!(page.buffer().lines(), ["a"], "three characters gone");

        let mut typed = tmp.page("one\ntwo\n");
        type_keys(&mut typed, "Ix <esc>");
        type_keys(&mut typed, "j.");
        assert_eq!(typed.buffer().lines(), ["x one", "x two"], "the typing too");

        // An undo is not a change to be done again.
        let mut undone = tmp.page("aaaa\n");
        press(&mut undone, "xu.");
        assert_eq!(undone.buffer().lines(), ["aaa"], "the `x`, not the `u`");
    }

    #[test]
    fn test_a_search_goes_to_the_text_and_n_carries_on() {
        let tmp = TempDir::new("search");
        let mut page = tmp.page("one two\nthree\ntwo again\n");
        let outcome = page.on_key(&key(KeyCode::Char('/')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("a search asks for what to look for");
        };
        page.on_prompt(PromptReply {
            answer: Some("two".to_string()),
            tag: request.tag,
        });
        assert_eq!((page.buffer().row(), page.buffer().col()), (0, 4));
        press(&mut page, "n");
        assert_eq!((page.buffer().row(), page.buffer().col()), (2, 0));
        press(&mut page, "N");
        assert_eq!(
            (page.buffer().row(), page.buffer().col()),
            (0, 4),
            "back again"
        );
    }

    #[test]
    fn test_the_star_key_looks_for_the_word_under_the_cursor() {
        let tmp = TempDir::new("star");
        let mut page = tmp.page("widget\nother\nwidget\n");
        press(&mut page, "*");
        assert_eq!(page.buffer().row(), 2);
    }

    #[test]
    fn test_a_mark_is_a_place_to_come_back_to() {
        let tmp = TempDir::new("marks");
        let mut page = tmp.page("one\ntwo\nthree\nfour\n");
        press(&mut page, "jjma");
        press(&mut page, "gg");
        press(&mut page, "'a");
        assert_eq!(page.buffer().row(), 2, "the mark");
        press(&mut page, "''");
        assert_eq!(page.buffer().row(), 0, "and back where the jump started");
    }

    #[test]
    fn test_a_click_puts_the_cursor_under_the_pointer_and_a_drag_selects() {
        let tmp = TempDir::new("mouse");
        let mut page = tmp.page("one\ntwo\nthree\n");
        painted_page(&mut page, 6);
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        page.on_mouse(PagePoint {
            col: gutter + 2,
            drag: false,
            row: HEADER_ROWS + 1,
        });
        assert_eq!((page.buffer().row(), page.buffer().col()), (1, 2));
        page.on_mouse(PagePoint {
            col: gutter + 4,
            drag: true,
            row: HEADER_ROWS + 2,
        });
        assert_eq!(page.mode, EditMode::Visual, "a drag selects");
        let selection = page.selection().expect("a selection");
        assert_eq!((selection.start, selection.end), ((1, 2), (2, 4)));
    }

    #[test]
    fn test_the_clipboard_register_asks_the_host_for_what_to_put_back() {
        let tmp = TempDir::new("clipboard-put");
        let mut page = tmp.page("one\n");
        page.on_key(&key(KeyCode::Char('"')));
        page.on_key(&key(KeyCode::Char('+')));
        let outcome = page.on_key(&key(KeyCode::Char('p')));
        assert_eq!(
            outcome,
            PageOutcome::Paste,
            "the page cannot read it itself"
        );
        page.on_paste("two\n".to_string());
        assert_eq!(
            page.buffer().lines(),
            ["one", "two"],
            "whole lines, as copied"
        );
    }

    #[test]
    fn test_an_operator_over_a_paragraph_motion_stays_within_the_lines() {
        // `d}` is exclusive and charwise in Vim: it takes to the blank line
        // without taking the blank line itself.
        let tmp = TempDir::new("paragraph-operator");
        let mut page = tmp.page("one\ntwo\n\nthree\n");
        press(&mut page, "d}");
        assert_eq!(page.buffer().lines(), ["", "three"]);
    }

    #[test]
    fn test_the_wheel_carries_the_cursor_with_the_view() {
        let tmp = TempDir::new("wheel");
        let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let mut page = tmp.page(&text);
        painted_page(&mut page, 11);
        page.on_scroll(-3);
        assert_eq!(page.buffer().row(), 3, "down three");
        page.on_scroll(2);
        assert_eq!(page.buffer().row(), 1, "and back up two");
    }

    #[test]
    fn test_a_file_changed_underneath_a_clean_editor_is_read_again() {
        let tmp = TempDir::new("resume");
        let mut page = tmp.page("before\n");
        std::fs::write(tmp.0.join("a.txt"), "after\n").expect("rewrite");
        // The stamp is the file's mtime, which a second write may share.
        page.doc_mut().stamp = file::FileStamp::default();
        page.on_resume();
        assert_eq!(page.buffer().lines(), ["after"]);
    }

    #[test]
    fn test_a_second_file_opens_as_another_buffer_of_the_same_editor() {
        let tmp = TempDir::new("buffers");
        let mut page = tmp.page("one\n");
        let other = tmp.file("b.txt", "two\n");

        page.open(other.clone(), None);
        assert_eq!(page.buffer_count(), 2);
        assert_eq!(page.buffer().lines(), ["two"], "the new one is in front");

        press(&mut page, "[b");
        assert_eq!(page.buffer().lines(), ["one"], "`[b` steps back");
        press(&mut page, "]b");
        assert_eq!(page.buffer().lines(), ["two"], "and `]b` on again");

        // A file already open is one to go to, not one to open twice.
        press(&mut page, "[b");
        page.open(other, None);
        assert_eq!(page.buffer_count(), 2);
        assert_eq!(page.buffer().lines(), ["two"]);
    }

    #[test]
    fn test_the_recency_chord_goes_to_the_last_file_worked_on_not_the_next_one() {
        // The whole point of the chord over `]b`: with three files open and
        // the middle one visited last, it goes back to that rather than to
        // whichever file happens to sit beside this one in opening order.
        let tmp = TempDir::new("recent-buffers");
        let mut page = tmp.page("one\n");
        page.open(tmp.file("b.txt", "two\n"), None);
        page.open(tmp.file("c.txt", "three\n"), None);
        press(&mut page, "[b");
        assert_eq!(page.buffer().lines(), ["two"], "visited second");
        press(&mut page, "]b");
        assert_eq!(page.buffer().lines(), ["three"], "and back to the third");

        page.recent_buffer(false);
        assert_eq!(page.buffer().lines(), ["two"], "the one worked on before");
        page.recent_buffer(false);
        assert_eq!(page.buffer().lines(), ["one"], "and further back again");
        page.recent_buffer(true);
        assert_eq!(page.buffer().lines(), ["two"], "forward toward the recent");
    }

    #[test]
    fn test_a_walk_does_not_reshuffle_what_it_passes_over() {
        // Walking past a file must not make it the most recent, or a second
        // press would bounce between two files instead of going on back.
        let tmp = TempDir::new("recent-walk");
        let mut page = tmp.page("one\n");
        page.open(tmp.file("b.txt", "two\n"), None);
        page.open(tmp.file("c.txt", "three\n"), None);
        page.recent_buffer(false);
        page.recent_buffer(false);
        assert_eq!(
            page.buffer().lines(),
            ["one"],
            "two steps back, not a toggle"
        );
    }

    #[test]
    fn test_closing_a_buffer_renumbers_the_recency_order_behind_it() {
        // The indices above a closed buffer all shift down; without
        // renumbering, a later walk lands on the wrong file or on none.
        let tmp = TempDir::new("recent-close");
        let mut page = tmp.page("one\n");
        page.open(tmp.file("b.txt", "two\n"), None);
        page.open(tmp.file("c.txt", "three\n"), None);
        // Close the middle file, so index 2 ("three") becomes index 1.
        press(&mut page, "[b");
        assert_eq!(page.buffer().lines(), ["two"]);
        press(&mut page, "q");
        assert_eq!(page.buffer_count(), 2);
        for _ in 0..page.buffer_count() * 2 {
            page.recent_buffer(false);
            assert!(
                page.buffer().lines() != ["two"],
                "the closed file is never walked back onto"
            );
        }
    }

    #[test]
    fn test_coming_back_to_a_buffer_re_reads_a_file_written_to_meanwhile() {
        let tmp = TempDir::new("buffer-stale");
        let mut page = tmp.page("before\n");
        page.open(tmp.file("b.txt", "other\n"), None);

        std::fs::write(tmp.0.join("a.txt"), "after\n").expect("rewrite");
        // The stamp is the file's mtime, which a second write may share.
        page.docs[0].stamp = file::FileStamp::default();

        press(&mut page, "[b");
        assert_eq!(page.buffer().lines(), ["after"]);
    }

    #[test]
    fn test_the_buffer_list_goes_to_the_one_chosen() {
        let tmp = TempDir::new("buffer-list");
        let mut page = tmp.page("one\n");
        page.open(tmp.file("b.txt", "two\n"), None);

        page.on_key(&key(KeyCode::Char('g')));
        let PageOutcome::Pick(request) = page.on_key(&key(KeyCode::Char('b'))) else {
            panic!("`gb` asks for one of the open buffers");
        };
        assert_eq!(request.items.len(), 2);
        // Most recently used first: the file in front heads the list, and the
        // one before it follows, so the usual next choice is a row away.
        assert!(request.items[0].ends_with("b.txt"), "the file in front");
        assert!(request.items[1].ends_with("a.txt"), "the one before it");

        let previous = request.items[1].clone();
        page.on_prompt(PromptReply {
            answer: Some(previous),
            tag: request.tag,
        });
        assert_eq!(page.buffer().lines(), ["one"]);
    }

    #[test]
    fn test_closing_one_buffer_leaves_the_editor_on_the_rest() {
        // With one file open `q` closes the editor; with two it closes the
        // file, or a second file would be a trap door out of the first.
        let tmp = TempDir::new("close-buffer");
        let mut page = tmp.page("one\n");
        page.open(tmp.file("b.txt", "two\n"), None);

        assert_eq!(
            page.on_key(&key(KeyCode::Char('q'))),
            PageOutcome::Consumed,
            "the editor stays up"
        );
        assert_eq!(page.buffer_count(), 1);
        assert_eq!(page.buffer().lines(), ["one"]);
        assert_eq!(
            page.on_key(&key(KeyCode::Char('q'))),
            PageOutcome::Close,
            "and the last one takes the editor with it"
        );
    }

    #[test]
    fn test_an_unsaved_buffer_asks_before_it_is_the_one_closed() {
        let tmp = TempDir::new("close-unsaved");
        let mut page = tmp.page("one\n");
        page.open(tmp.file("b.txt", "two\n"), None);
        press(&mut page, "x");

        let PageOutcome::Prompt(request) = page.on_key(&key(KeyCode::Char('q'))) else {
            panic!("unsaved edits are worth a question");
        };
        page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: request.tag,
        });
        assert_eq!(page.buffer_count(), 1, "the buffer went, not the editor");
        assert_eq!(page.buffer().lines(), ["one"]);
    }

    #[test]
    fn test_what_buffers_share_and_what_belongs_to_each_file() {
        // A yank that did not reach the next buffer would make two buffers
        // two editors; a mark that did would name a line in the wrong file.
        let tmp = TempDir::new("shared");
        let mut page = tmp.page("one\n");
        press(&mut page, "mayy");
        page.open(tmp.file("b.txt", "two\n"), None);

        press(&mut page, "p");
        assert_eq!(page.buffer().lines(), ["two", "one"], "the register came");

        press(&mut page, "'a");
        assert!(
            page.message
                .as_deref()
                .unwrap_or_default()
                .contains("not set"),
            "the mark stayed with its own file"
        );
    }

    #[test]
    fn test_a_line_taken_with_dd_comes_back_with_p() {
        let tmp = TempDir::new("dd");
        let mut page = tmp.page("one\ntwo\nthree\n");
        press(&mut page, "dd");
        assert_eq!(page.buffer().lines(), ["two", "three"]);
        press(&mut page, "p");
        assert_eq!(page.buffer().lines(), ["two", "one", "three"]);
    }

    #[test]
    fn test_cw_changes_the_word_without_swallowing_the_space_after_it() {
        // `dw` takes the space and `cw` does not: the difference is the whole
        // reason Vim treats them separately, and getting it wrong runs the
        // changed word into the next one.
        let tmp = TempDir::new("cw");

        let mut change = tmp.page("alpha beta\n");
        press(&mut change, "cw");
        assert_eq!(change.mode, EditMode::Insert);
        assert_eq!(change.buffer().lines(), [" beta"]);

        let mut delete = tmp.page("alpha beta\n");
        press(&mut delete, "dw");
        assert_eq!(delete.buffer().lines(), ["beta"]);
    }

    #[test]
    fn test_the_inclusive_motions_take_the_character_they_land_on() {
        // `d$` leaving the last character behind, and `de` leaving a word's
        // final letter, are the classic off-by-one in an operator.
        let tmp = TempDir::new("inclusive");

        let mut dollar = tmp.page("keep this\n");
        press(&mut dollar, "ll");
        page_key(&mut dollar, KeyCode::Char('d'));
        page_key(&mut dollar, KeyCode::Char('$'));
        assert_eq!(dollar.buffer().lines(), ["ke"]);

        let mut end = tmp.page("alpha beta\n");
        press(&mut end, "de");
        assert_eq!(end.buffer().lines(), [" beta"]);
    }

    #[test]
    fn test_an_operator_over_a_motion_that_crosses_lines_takes_whole_lines() {
        // `dj` is linewise in Vim: taking only the characters between the two
        // columns would leave two half-lines spliced together.
        let tmp = TempDir::new("linewise");
        let mut page = tmp.page("one\ntwo\nthree\n");
        press(&mut page, "dj");
        assert_eq!(page.buffer().lines(), ["three"]);
    }

    #[test]
    fn test_a_count_applies_to_the_command_that_follows_it() {
        let tmp = TempDir::new("count");
        let mut page = tmp.page("abcdef\n");
        press(&mut page, "3x");
        assert_eq!(page.buffer().lines(), ["def"]);

        let mut lines = tmp.page("one\ntwo\nthree\nfour\n");
        press(&mut lines, "2dd");
        assert_eq!(lines.buffer().lines(), ["three", "four"]);
    }

    #[test]
    fn test_a_leading_zero_is_the_line_start_rather_than_a_count() {
        // Typed after a digit it is part of the count; typed first it is the
        // motion to column one, and reading it as a count of zero would make
        // the next command act ten times or not at all.
        let tmp = TempDir::new("zero");
        let mut page = tmp.page("abcdefghijkl\n");
        press(&mut page, "lll0x");
        assert_eq!(page.buffer().lines(), ["bcdefghijkl"]);

        let mut ten = tmp.page("abcdefghijkl\n");
        press(&mut ten, "10x");
        assert_eq!(ten.buffer().lines(), ["kl"]);
    }

    #[test]
    fn test_typing_lands_in_the_text_and_escape_steps_back_onto_it() {
        let tmp = TempDir::new("insert");
        let mut page = tmp.page("bc\n");
        press(&mut page, "i");
        assert_eq!(page.mode, EditMode::Insert);
        press(&mut page, "a");
        assert_eq!(page.buffer().lines(), ["abc"]);

        page.on_key(&key(KeyCode::Escape));
        assert_eq!(page.mode, EditMode::Normal);
        assert_eq!(page.buffer().col(), 0, "back onto the character just typed");
    }

    #[test]
    fn test_closing_with_unsaved_edits_asks_before_dropping_them() {
        // The one press that can lose work, so it is the one that has to ask.
        let tmp = TempDir::new("dirty");
        let mut page = tmp.page("one\n");
        assert_eq!(page.on_key(&key(KeyCode::Char('q'))), PageOutcome::Close);

        press(&mut page, "x");
        let outcome = page.on_key(&key(KeyCode::Char('q')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_DISCARD);
        assert_eq!(request.mode, PromptMode::Confirm);

        // Answering no keeps the page and the edits.
        assert_eq!(
            page.on_prompt(PromptReply {
                answer: None,
                tag: ASK_DISCARD,
            }),
            PageOutcome::Consumed
        );
    }

    #[test]
    fn test_saving_writes_the_text_and_clears_the_unsaved_mark() {
        let tmp = TempDir::new("save");
        let mut page = tmp.page("one\ntwo\n");
        press(&mut page, "x");
        assert!(page.buffer().is_dirty());

        page.on_key(&ctrl('s'));
        assert!(!page.buffer().is_dirty());
        let written = std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads");
        assert_eq!(written, "ne\ntwo\n");

        // And closing no longer has anything to ask about.
        assert_eq!(page.on_key(&key(KeyCode::Char('q'))), PageOutcome::Close);
    }

    #[test]
    fn test_saving_over_a_file_changed_underneath_asks_first() {
        // Overwriting silently is the one thing an editor does that cannot be
        // undone from inside it.
        let tmp = TempDir::new("stale");
        let mut page = tmp.page("one\n");
        press(&mut page, "x");
        std::fs::write(tmp.0.join("a.txt"), "written elsewhere\n").expect("outside write");

        let outcome = page.on_key(&ctrl('s'));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_OVERWRITE);
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
            "written elsewhere\n",
            "nothing was written while the question was open"
        );
    }

    #[test]
    fn test_zz_writes_and_closes_while_zq_closes_without_writing() {
        let tmp = TempDir::new("leave");

        let mut written = tmp.page("one\n");
        press(&mut written, "x");
        press(&mut written, "ZZ");
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
            "ne\n"
        );

        let mut discarded = tmp.page("one\n");
        press(&mut discarded, "x");
        assert_eq!(
            discarded.on_key(&key(KeyCode::Char('Z'))),
            PageOutcome::Consumed
        );
        assert_eq!(
            discarded.on_key(&key(KeyCode::Char('Q'))),
            PageOutcome::Close
        );
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
            "one\n",
            "the edits were dropped, not written"
        );
    }

    #[test]
    fn test_the_file_opens_on_the_line_the_tool_was_pointing_at() {
        // A grep hit opened at line one throws away what was searched for.
        let tmp = TempDir::new("line");
        let path = tmp.0.join("a.txt");
        std::fs::write(&path, "one\ntwo\nthree\n").expect("temp file");
        let page = EditorPage::new(path, Some(3)).expect("opens");
        assert_eq!(page.buffer().row(), 2);
    }

    #[test]
    fn test_handing_off_to_the_outside_editor_saves_first_and_carries_the_line() {
        // `$EDITOR` reads the file from disk, so unsaved edits would not be
        // there; landing it at line one would lose the place as well.
        let tmp = TempDir::new("escalate");
        let mut page = tmp.page("one\ntwo\nthree\n");
        press(&mut page, "jx");
        let outcome = page.on_key(&ctrl('o'));
        assert_eq!(
            outcome,
            PageOutcome::SpawnEditor(OpenTarget::at_line(tmp.0.join("a.txt"), 2))
        );
        assert_eq!(
            std::fs::read_to_string(tmp.0.join("a.txt")).expect("reads"),
            "one\nwo\nthree\n"
        );
    }

    #[test]
    fn test_the_window_chords_still_reach_the_window_from_either_mode() {
        // An Alt chord read as text types an `h` instead of moving focus, and
        // in Normal mode `Alt-p` would paste: a tool that swallows the window
        // chords traps the user inside it.
        let tmp = TempDir::new("alt");
        let alt = |c: char| Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
        };

        let mut page = tmp.page("one\ntwo\n");
        assert_eq!(page.on_key(&alt('h')), PageOutcome::Ignored);
        press(&mut page, "dd");
        assert_eq!(page.on_key(&alt('p')), PageOutcome::Ignored);
        assert_eq!(page.buffer().lines(), ["two"], "nothing was pasted");

        press(&mut page, "i");
        assert_eq!(page.on_key(&alt('h')), PageOutcome::Ignored);
        assert_eq!(page.buffer().lines(), ["two"], "and nothing was typed");

        // The two buffer ends the shared layer claims are still the tool's.
        let mut ends = tmp.page("one\ntwo\nthree\n");
        let shifted = Key {
            alt: true,
            code: KeyCode::Char('>'),
            ctrl: false,
            shift: false,
        };
        assert_eq!(ends.on_key(&shifted), PageOutcome::Consumed);
        assert_eq!(ends.buffer().row(), 2);
    }

    #[test]
    fn test_painting_survives_a_pane_with_no_room_and_a_line_with_no_text() {
        // Every one of these indexes the line the cursor is on to draw the
        // cursor into it, and an empty line, a pane one column wide, and a
        // pane with no rows at all are where that goes wrong.
        let tmp = TempDir::new("paint");
        let mut page = tmp.page("one\n\nthree\n");
        for (rows, cols) in [(0, 0), (1, 1), (2, 6), (40, 80)] {
            page.content(rows, cols, false);
            page.content(rows, cols, true);
        }

        // On the empty line, in both modes, and past the end of a short one.
        press(&mut page, "j");
        page.content(10, 20, false);
        press(&mut page, "i");
        page.content(10, 20, false);
    }

    /// The styles of one painted text row, run by run, with the line-number
    /// gutter dropped.
    fn painted(page: &mut EditorPage, row: usize) -> Vec<(PageStyle, String)> {
        let content = page.content(20, 60, false);
        content.rows[HEADER_ROWS + row]
            .iter()
            .skip(1)
            .map(|span| (span.style, span.text.clone()))
            .collect()
    }

    #[test]
    fn test_source_is_painted_by_what_each_run_of_it_is() {
        let tmp = TempDir::new("syntax");
        let path = tmp.0.join("a.rs");
        std::fs::write(
            &path,
            "let n = 42; // why
",
        )
        .expect("temp file");
        let mut page = EditorPage::new(path, None).expect("opens");

        assert_eq!(
            painted(&mut page, 0),
            vec![
                (PageStyle::SyntaxKeyword, "let".to_string()),
                (PageStyle::Normal, " n = ".to_string()),
                (PageStyle::SyntaxNumber, "42".to_string()),
                (PageStyle::Normal, "; ".to_string()),
                (PageStyle::SyntaxComment, "// why".to_string()),
            ]
        );
    }

    #[test]
    fn test_a_file_in_no_known_language_is_painted_as_the_text_it_is() {
        // Guessing at a language for a plain file colors words that mean
        // nothing, which is worse than coloring none of them.
        let tmp = TempDir::new("plain");
        let path = tmp.0.join("notes.txt");
        std::fs::write(
            &path,
            "let me note 42 things
",
        )
        .expect("temp file");
        let mut page = EditorPage::new(path, None).expect("opens");

        assert_eq!(
            painted(&mut page, 0),
            vec![(PageStyle::Normal, "let me note 42 things".to_string())]
        );
    }

    #[test]
    fn test_a_comment_opened_above_the_window_colors_what_it_covers() {
        // A line is colored by what precedes it, and what precedes it may be
        // off screen: scrolling into the middle of a block comment must not
        // show it as code.
        let tmp = TempDir::new("block");
        let path = tmp.0.join("a.rs");
        let mut text = String::from(
            "/* opened here
",
        );
        for index in 0..40 {
            text.push_str(&format!(
                "still inside {index}
"
            ));
        }
        text.push_str(
            "*/ let x = 1;
",
        );
        std::fs::write(&path, text).expect("temp file");
        let mut page = EditorPage::new(path, None).expect("opens");

        press(&mut page, "G");
        page.content(20, 60, false);
        let last = page.buffer().lines().len() - 1;
        let visible = last - page.doc().scroll;
        let styles: Vec<PageStyle> = painted(&mut page, visible - 1)
            .into_iter()
            .map(|(style, _)| style)
            .collect();
        assert!(
            styles
                .iter()
                .all(|style| *style == PageStyle::SyntaxComment),
            "the line above the closer is still inside the comment: {styles:?}"
        );
    }

    #[test]
    fn test_opening_a_comment_recolors_the_lines_under_it_as_it_is_typed() {
        // What a line means for the ones below it changes as it is edited, so
        // what was worked out about them cannot be kept.
        let tmp = TempDir::new("recolor");
        let path = tmp.0.join("a.rs");
        std::fs::write(
            &path,
            "let x = 1;
let y = 2;
",
        )
        .expect("temp file");
        let mut page = EditorPage::new(path, None).expect("opens");
        assert_eq!(painted(&mut page, 1)[0].0, PageStyle::SyntaxKeyword);

        // Open a comment on the first line; the second is now inside it.
        press(&mut page, "I");
        press(&mut page, "/*");
        assert!(
            painted(&mut page, 1)
                .iter()
                .all(|(style, _)| *style == PageStyle::SyntaxComment),
            "the line below was left as code"
        );
    }

    #[test]
    fn test_the_caret_lands_on_the_text_rather_than_in_the_line_numbers() {
        // The host puts a page's cursor on the row's first painted character
        // unless the page says otherwise, and here that is the line number.
        let tmp = TempDir::new("caret");
        let mut page = tmp.page("fn main() {\n\tlet x = 1;\n");
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();

        page.content(10, 40, false);
        let caret = page.caret().expect("a caret");
        assert_eq!(caret.col, gutter, "column one of the text, not of the row");
        assert!(!caret.insert);

        // A tab is one character in the file and several columns on screen,
        // so a caret that counted characters would drift left of the text.
        press(&mut page, "jll");
        page.content(10, 40, false);
        assert_eq!(
            page.caret().expect("a caret").col,
            gutter + TAB_WIDTH + 1,
            "past the tab's width, then one character"
        );

        // Typing says so, so the caret can take the shape typing has.
        press(&mut page, "i");
        assert!(page.caret().expect("a caret").insert);
    }

    #[test]
    fn test_a_line_wider_than_the_pane_scrolls_to_keep_the_cursor_in_view() {
        // Without this, editing past the pane's width types into a column
        // nobody can see.
        let tmp = TempDir::new("hscroll");
        let mut page = tmp.page(&format!("{}\n", "x".repeat(200)));
        page.content(10, 40, false);
        assert_eq!(page.doc().hscroll, 0);

        press(&mut page, "$");
        page.content(10, 40, false);
        assert!(
            page.doc().hscroll > 0,
            "the end of the line was scrolled to"
        );

        press(&mut page, "0");
        page.content(10, 40, false);
        assert_eq!(page.doc().hscroll, 0, "and back again");
    }

    fn page_key(page: &mut EditorPage, code: KeyCode) {
        page.on_key(&key(code));
    }
}

//! Editor: one file as editable text in a pane, opened from whichever tool
//! was pointing at it.
//!
//! - `comment`: commenting lines out and back in.
//! - `commit`: recalling earlier commit messages into one being written.
//! - `rebase`: the actions and checks of an interactive rebase's plan.
//! - [`file`]: reading a file in and writing it back.
//! - [`keys`]: what every key does to either.
//! - [`syntax`]: which run of characters is a comment, a string, a keyword.

mod comment;
mod commit;
pub mod file;
pub mod keys;
mod rebase;
pub mod syntax;

use std::io;
use std::path::{Path, PathBuf};

use crate::control::EditWaiter;
use crate::model::input::Key;
use crate::model::page::{
    row_height, wheel_scroll, window_anchor, wrap_window, wrapped_lines, OpenTarget, Page,
    PageCaret, PageContent, PageHint, PageOutcome, PagePoint, PageRow, PageSpan, PageStyle,
    PromptMode, PromptReply, PromptRequest,
};
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::modal::{EditMode, EditView, Marks, ModalEditor, Selection};

use commit::MessageRecall;
use file::{FileShape, FileStamp};
use syntax::{LineState, Syntax, Token};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question the editor asks.
const ASK_BUFFER: &str = "buffer";
const ASK_OVERWRITE: &str = "overwrite";
const ASK_SEARCH: &str = "search";
const ASK_SEARCH_BACK: &str = "search-back";

/// What marks a buffer holding edits that are not on disk, in the header
/// and in the list of open buffers.
pub(crate) const DIRTY_MARK: &str = " [+]";

/// What a commit message's header says its keys are.
const MESSAGE_HINT: &str = "  C-c C-c finish, C-c C-k cancel";

/// What a rebase plan's header says its keys are.
const TODO_HINT: &str = "  C-c p/r/e/s/f/d action, C-c j/k move, C-c C-c finish, C-c C-k abort";

/// Rows of header above the first line of text.
const HEADER_ROWS: usize = 1;

/// Width the line numbers are padded to, and the gap after them, so the text
/// starts in the same column whether the file has ten lines or ten thousand.
const NUMBER_WIDTH: usize = 4;
const NUMBER_GAP: &str = " ";

/// How many columns a tab occupies on screen. Display only: what is in the
/// file stays one tab character.
const TAB_WIDTH: usize = 4;

// ========================================================================
// Data Structures
// ========================================================================

/// The files open for editing, the keys that act on them, and everything the
/// two share: one yank reaches every buffer, as it does in Vim.
#[derive(Debug)]
pub struct EditorPage {
    /// Which of the open files the keys act on.
    at: usize,
    /// Every file open in this editor, in the order they were opened.
    docs: Vec<Document>,
    /// Where the cursor and scroll were before jump previewing started: (row, col, scroll).
    jump_origin: Option<(usize, usize, usize)>,
    /// The modal keys, and what outlives one buffer: the registers, the last
    /// search, the last change, and what the last command reported.
    modal: ModalEditor,
    /// Buffer indices, most recently shown first, so a recency walk and the
    /// buffer list both read in the order the files were last worked on.
    mru: Vec<usize>,
    /// Cursor into the recency order while a walk is in progress, so repeated
    /// steps go on through it instead of restarting from the current buffer.
    mru_walk: Option<usize>,
    /// What each screen row of the last painted frame holds, so a click lands
    /// on the character under the pointer.
    painted: Vec<Painted>,
    /// Whether `Ctrl-C` has just been pressed in a commit message, which the
    /// next `Ctrl-C` finishes and `Ctrl-K` cancels.
    message_chord: bool,
    /// Where `Alt-P`/`Alt-N` have got to through earlier commit messages, for
    /// a buffer that is one.
    recall: MessageRecall,
    /// How many rows of text the pane last had room for, so the paging keys
    /// can move by what is actually on screen.
    viewport: usize,
    /// Whether the wheel, not the cursor, last placed the view: the view then
    /// stays where it was put until a key brings the cursor back into it.
    wheel: bool,
    /// A program blocked until this page is done, such as `git commit` waiting
    /// on its message. Answered when the page is dropped.
    waiter: Option<EditWaiter>,
}

/// One open file: its text, where the cursor and the window sit in it, and
/// what it looked like on disk when it was read.
#[derive(Debug)]
struct Document {
    buffer: TextBuffer,
    /// The first column painted, for lines wider than the pane.
    hscroll: usize,
    /// The marks `m` has set in this file, and where `''` goes back to.
    marks: Marks,
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
        // A commit message is for typing into, so it opens ready to type.
        let mode = match commit::is_message_file(&path) {
            true => EditMode::Insert,
            false => EditMode::Normal,
        };
        Ok(Self {
            at: 0,
            docs: vec![Document::open(path, line)?],
            jump_origin: None,
            message_chord: false,
            modal: ModalEditor::new().with_mode(mode),
            mru: vec![0],
            mru_walk: None,
            painted: Vec::new(),
            recall: MessageRecall::default(),
            viewport: 0,
            wheel: false,
            waiter: None,
        })
    }

    /// Whether this page is editing the message of a commit about to be made,
    /// which is the one message worth showing the staged changes beside.
    pub(crate) fn edits_commit(&self) -> bool {
        commit::is_commit_message(&self.doc().path)
    }

    /// Have `waiter` answered when this page ends, which is what lets a
    /// program that launched the editor and blocked on it carry on.
    pub(crate) fn with_waiter(mut self, waiter: EditWaiter) -> Self {
        self.waiter = Some(waiter);
        self
    }

    /// Tell whoever is waiting on this page that its edit was thrown away,
    /// when closing the buffer closes the page.
    fn reject_waiter_if_closing(&mut self) {
        if self.docs.len() > 1 {
            return;
        }
        if let Some(waiter) = self.waiter.as_mut() {
            waiter.reject();
        }
    }

    /// Throw a commit message away the way git understands: empty the file and
    /// save it, so git stops on its own at an empty message. `None` where the
    /// buffer is not a message, or closing it would not end the page.
    pub(super) fn cancel_message(&mut self) -> Option<PageOutcome> {
        if self.docs.len() > 1 || !commit::is_message_file(&self.doc().path) {
            return None;
        }
        let rows = self.buffer().lines().len();
        self.buffer_mut().replace_rows(0, rows, vec![String::new()]);
        self.write();
        Some(PageOutcome::Close)
    }

    /// `Ctrl-Alt-P`/`Ctrl-Alt-N` in a commit message: swap the message above git's
    /// comment footer for an earlier one (`back`) or the one after it.
    pub(super) fn recall_message(&mut self, back: bool) {
        if !commit::is_message_file(&self.doc().path) {
            return;
        }
        let rows = commit::message_rows(self.buffer().lines());
        let typed = self.buffer().lines()[..rows].join("\n");
        let path = self.doc().path.clone();
        let recalled = match back {
            true => self.recall.previous(&path, typed),
            false => self.recall.next(),
        };
        if let Some(message) = recalled {
            let lines = message.split('\n').map(str::to_string).collect();
            self.buffer_mut().replace_rows(0, rows, lines);
        }
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
            Err(e) => self.modal.set_message(format!("{e}")),
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
        let at = self.at;
        self.modal.reset(&self.docs[at].buffer);
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

    /// What a Visual selection covers in the buffer being read.
    fn selection(&self) -> Option<Selection> {
        self.modal.selection(self.buffer())
    }

    /// Lend the modal keys the buffer being read, with its marks and the
    /// window it is read through.
    fn with_modal<R>(&mut self, f: impl FnOnce(&mut ModalEditor, EditView<'_>) -> R) -> R {
        let viewport = self.viewport;
        let at = self.at.min(self.docs.len() - 1);
        // The other buffers are lent read-only beside the one being edited,
        // for completion to draw words from.
        let (before, rest) = self.docs.split_at_mut(at);
        let Some((doc, after)) = rest.split_first_mut() else {
            unreachable!("`at` is clamped inside the open buffers");
        };
        let others: Vec<&[String]> = before
            .iter()
            .chain(after.iter())
            .map(|other| other.buffer.lines())
            .collect();
        let view = EditView {
            buffer: &mut doc.buffer,
            marks: &mut doc.marks,
            others: &others,
            scroll: &mut doc.scroll,
            viewport,
        };
        f(&mut self.modal, view)
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
                self.modal.set_message(format!("wrote {wrote}"));
            }
            Err(e) => self.modal.set_message(format!("could not write: {e}")),
        }
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
            marks: Marks::default(),
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
        if let Some(name) = mode_name(self.modal.mode()) {
            spans.push(PageSpan::new(PageStyle::Accent, format!("  {name}")));
        }
        if commit::is_message_file(&self.doc().path) {
            spans.push(PageSpan::new(PageStyle::Dim, MESSAGE_HINT));
        }
        if rebase::is_rebase_todo(&self.doc().path) {
            spans.push(PageSpan::new(PageStyle::Dim, TODO_HINT));
        }
        if let Some(register) = self.modal.register_prefix() {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  \"{register}")));
        }
        if let Some(message) = self.modal.message() {
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
    /// A file in no language this knows is one long run of plain text, except
    /// that git's own `#` lines in a message are comments.
    fn colors_of(&self, index: usize, line: &str) -> Vec<Token> {
        if commit::is_message_file(&self.doc().path) && commit::is_comment_line(line) {
            return vec![Token::Comment; line.chars().count()];
        }
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
            if !self.modal.is_selecting() {
                let at = self.at;
                self.modal.start_visual(&self.docs[at].buffer, false);
            }
        } else {
            self.modal.clear_selection();
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
        let at = self.at.min(self.docs.len() - 1);
        self.modal.reset(&self.docs[at].buffer);
        self.modal
            .set_message("changed on disk, re-read".to_string());
        PageOutcome::Consumed
    }
}

impl Page for EditorPage {
    fn title(&self) -> String {
        name_of(&self.doc().path)
    }

    /// Any buffer with unwritten edits, not just the one being read: closing
    /// the editor takes every buffer it holds with it.
    fn is_dirty(&self) -> bool {
        self.docs.iter().any(|doc| doc.buffer.is_dirty())
    }

    /// A program blocked on this edit, such as `git commit` waiting on its
    /// message, hears that it is over now rather than whenever the kept page
    /// is finally dropped.
    fn on_closed(&mut self) {
        self.waiter = None;
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
        let text_cols = cols.saturating_sub(NUMBER_WIDTH + NUMBER_GAP.len());
        self.viewport = rows.saturating_sub(HEADER_ROWS);
        self.follow_cursor(text_cols, wrap);
        let total = self.buffer().lines().len();
        let anchor = window_anchor(
            self.wheel,
            self.doc().scroll,
            self.buffer().row(),
            self.viewport,
        );
        let window = wrap_window(self.doc().scroll, anchor, total, self.viewport, |index| {
            row_height(
                &expanded_text(&self.line_of(index)),
                text_cols,
                wrap,
                NUMBER_WIDTH + NUMBER_GAP.len(),
            )
        });
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
        let content = PageContent::new(page_rows).with_wrap_indents(indents);
        match anchor == self.buffer().row() {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.modal.take_message();
        self.wheel = false;
        // The space bar is typed text here, in insert mode and after `f`, `r`
        // and the like, all of which match characters.
        self.dispatch(&key.typed())
    }

    fn on_mouse(&mut self, at: PagePoint) -> PageOutcome {
        self.wheel = false;
        self.point_at(at)
    }

    fn on_scroll(&mut self, lines: isize) -> PageOutcome {
        // The view moves and the cursor stays where it was, so the text can be
        // read without losing the place being edited; the next key brings the
        // view back to it.
        let total = self.buffer().lines().len();
        let scroll = wheel_scroll(self.doc().scroll, lines, total, self.viewport);
        self.doc_mut().scroll = scroll;
        self.wheel = true;
        PageOutcome::Consumed
    }

    fn on_paste(&mut self, text: String) -> PageOutcome {
        self.wheel = false;
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
            insert: self.modal.is_typing(),
        })
    }

    fn hint(&self) -> Option<PageHint> {
        Some(PageHint {
            items: self.modal.hint()?,
            title: "editor".to_string(),
        })
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        self.wheel = false;
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
            ASK_OVERWRITE if said_yes => self.write(),
            ASK_SEARCH | ASK_SEARCH_BACK => match reply.answer {
                Some(pattern) => {
                    self.jump_origin = None;
                    self.start_search(pattern, reply.tag == ASK_SEARCH)
                }
                None => {
                    if let Some((row, col, scroll)) = self.jump_origin.take() {
                        self.buffer_mut().move_to(row, col);
                        self.doc_mut().scroll = scroll;
                    }
                    PageOutcome::Consumed
                }
            },
            _ => PageOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> PageOutcome {
        if (tag == ASK_SEARCH || tag == ASK_SEARCH_BACK) && !query.is_empty() {
            if self.jump_origin.is_none() {
                self.jump_origin =
                    Some((self.buffer().row(), self.buffer().col(), self.doc().scroll));
            }
            self.start_search(query.to_string(), tag == ASK_SEARCH)
        } else {
            PageOutcome::Consumed
        }
    }

    /// The buffer being read, at its cursor's line. A commit message or a
    /// rebase plan is git's to reopen, not a restart's.
    fn snapshot(&self) -> Option<OpenTarget> {
        let path = &self.doc().path;
        if commit::is_message_file(path) || rebase::is_rebase_todo(path) {
            return None;
        }
        Some(OpenTarget::at_line(path.clone(), self.buffer().row() + 1))
    }

    fn is_commit_editor(&self) -> bool {
        self.edits_commit()
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The buffer being edited, not the one the editor opened with, so
        // stepping through buffers takes the answer with it.
        self.doc().path.parent().map(PathBuf::from)
    }

    fn address(&self) -> Option<String> {
        Some(crate::model::path::abbreviate_home(&self.doc().path))
    }

    fn file_reference(&self) -> Option<String> {
        let base = crate::model::path::abbreviate_home(&self.doc().path);
        if self.modal.mode() == EditMode::Visual {
            if let Some(selection) = self.selection() {
                let start = selection.start.0 + 1;
                let end = selection.end.0 + 1;
                let suffix = if start == end {
                    format!(":{start}")
                } else {
                    format!(":{start}-{end}")
                };
                return Some(format!("{base}{suffix}"));
            }
        }
        Some(base)
    }

    fn jump_targets(&self) -> Option<Vec<(usize, String)>> {
        let lines = self.buffer().lines();
        let mut targets: Vec<(usize, String)> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(idx, line)| (idx, format!("{:>5}  {}", idx + 1, line.trim())))
            .collect();
        if targets.is_empty() && !lines.is_empty() {
            targets = lines
                .iter()
                .enumerate()
                .map(|(idx, line)| (idx, format!("{:>5}  {}", idx + 1, line)))
                .collect();
        }
        Some(targets)
    }

    fn jump_cursor(&self) -> Option<usize> {
        Some(self.buffer().row())
    }

    fn jump_to(&mut self, target: usize) {
        if self.jump_origin.is_none() {
            self.jump_origin = Some((self.buffer().row(), self.buffer().col(), self.doc().scroll));
        }
        self.buffer_mut().move_to_line(target);
        self.doc_mut().scroll = target.saturating_sub(self.viewport / 2);
    }

    fn jump_cancel(&mut self) {
        if let Some((row, col, scroll)) = self.jump_origin.take() {
            self.buffer_mut().move_to(row, col);
            self.doc_mut().scroll = scroll;
        }
    }

    fn jump_confirm(&mut self, target: usize) {
        if let Some((row, col, _)) = self.jump_origin.take() {
            self.doc_mut().marks.jump = Some((row, col));
        }
        self.buffer_mut().move_to_line(target);
        self.doc_mut().scroll = target.saturating_sub(self.viewport / 2);
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
        assert_eq!(
            chars.modal.mode(),
            EditMode::Normal,
            "and the selection is over"
        );

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
        assert_eq!(page.modal.mode(), EditMode::Normal);
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
        assert_eq!(page.modal.mode(), EditMode::Visual, "a drag selects");
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
    fn test_the_wheel_moves_the_view_and_leaves_the_cursor() {
        // Reading further down must not drag the cursor along, or the place
        // being edited is lost the moment the wheel turns.
        let tmp = TempDir::new("wheel");
        let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let mut page = tmp.page(&text);
        painted_page(&mut page, 11);
        page.on_scroll(-3);
        painted_page(&mut page, 11);
        assert_eq!(page.doc().scroll, 3, "down three");
        assert_eq!(page.buffer().row(), 0, "the cursor stays on its line");
        page.on_scroll(2);
        painted_page(&mut page, 11);
        assert_eq!(page.doc().scroll, 1, "and back up two");
        assert_eq!(page.buffer().row(), 0);
    }

    fn press_alt(page: &mut EditorPage, code: KeyCode) -> PageOutcome {
        page.on_key(&Key {
            alt: true,
            ..key(code)
        })
    }

    fn press_ctrl(page: &mut EditorPage, c: char) -> PageOutcome {
        page.on_key(&Key {
            ctrl: true,
            ..key(KeyCode::Char(c))
        })
    }

    #[test]
    fn test_ctrl_slash_comments_the_selected_lines_and_the_cursor_line_back() {
        let tmp = TempDir::new("comment");
        let path = tmp.file("a.py", "x = 1\ny = 2\nz = 3\n");
        let mut page = EditorPage::new(path, None).expect("opens");
        press(&mut page, "Vj");
        press_ctrl(&mut page, '/');
        assert_eq!(page.buffer().lines(), ["# x = 1", "# y = 2", "z = 3"]);
        assert_eq!(
            page.modal.mode(),
            EditMode::Normal,
            "the selection is done with"
        );

        // Back on one line, in Insert mode, the caret keeps its character.
        press(&mut page, "A");
        press_ctrl(&mut page, '/');
        assert_eq!(page.buffer().lines()[1], "y = 2");
        assert_eq!(page.buffer().col(), 5, "still at the end of the line");
    }

    #[test]
    fn test_a_rebase_plan_takes_action_chords_and_refuses_to_open_with_a_fold() {
        let tmp = TempDir::new("rebase-todo");
        let path = tmp.file(
            "git-rebase-todo",
            "pick aaa first\npick bbb second\n# help\n",
        );
        let mut page = EditorPage::new(path.clone(), None).expect("opens");

        // `C-c s` on the first line would fold it into nothing.
        press_ctrl(&mut page, 'c');
        press(&mut page, "s");
        press_ctrl(&mut page, 'c');
        assert_eq!(
            press_ctrl(&mut page, 'c'),
            PageOutcome::Consumed,
            "refused, not closed"
        );
        assert!(page
            .modal
            .message()
            .unwrap_or_default()
            .contains("first commit"));

        // Fix it: setting the action stepped onto the second commit, which
        // moves above the first, and then is dropped.
        press_ctrl(&mut page, 'c');
        press(&mut page, "k");
        assert_eq!(page.buffer().lines()[0], "pick bbb second");
        press_ctrl(&mut page, 'c');
        press(&mut page, "d");
        press_ctrl(&mut page, 'c');
        assert_eq!(press_ctrl(&mut page, 'c'), PageOutcome::Close);
        assert_eq!(
            std::fs::read_to_string(&path).expect("written"),
            "drop bbb second\nsquash aaa first\n# help\n"
        );
    }

    #[test]
    fn test_insert_mode_walks_and_kills_by_word_with_the_alt_chords() {
        let tmp = TempDir::new("emacs-words");
        let mut page = tmp.page("one two three\n");
        press(&mut page, "A");
        press_alt(&mut page, KeyCode::Char('b'));
        assert_eq!(page.buffer().col(), 8, "back to the start of three");
        press_alt(&mut page, KeyCode::Char('b'));
        assert_eq!(page.buffer().col(), 4);
        press_alt(&mut page, KeyCode::Char('f'));
        assert_eq!(page.buffer().col(), 7, "forward to the end of two");
        press_alt(&mut page, KeyCode::Char('d'));
        assert_eq!(page.buffer().lines(), ["one two"]);
        press_alt(&mut page, KeyCode::Backspace);
        assert_eq!(page.buffer().lines(), ["one "]);
        // The two kills ran back to back, so they come back as one.
        press_ctrl(&mut page, 'y');
        assert_eq!(
            page.buffer().lines(),
            ["one two three"],
            "the whole run comes back"
        );
    }

    #[test]
    fn test_insert_mode_edits_around_the_cursor_with_the_ctrl_chords() {
        let tmp = TempDir::new("emacs-ctrl");
        let mut page = tmp.page("abcd\nxyz\n");
        press(&mut page, "i");
        press_ctrl(&mut page, 'f');
        press_ctrl(&mut page, 'f');
        press_ctrl(&mut page, 't');
        assert_eq!(page.buffer().lines(), ["acbd", "xyz"], "transposed");
        press_ctrl(&mut page, 'd');
        assert_eq!(page.buffer().lines(), ["acb", "xyz"], "deleted under it");
        press_ctrl(&mut page, 'h');
        assert_eq!(page.buffer().lines(), ["ac", "xyz"], "deleted behind it");
        press_ctrl(&mut page, 'n');
        assert_eq!(page.buffer().row(), 1, "down a line");
        press_ctrl(&mut page, 'p');
        assert_eq!(page.buffer().row(), 0, "and back up");
        press_ctrl(&mut page, 'o');
        assert_eq!(page.buffer().lines(), ["ac", "", "xyz"].map(String::from));
        assert_eq!(
            page.buffer().row(),
            0,
            "the cursor stays ahead of the break"
        );
    }

    #[test]
    fn test_insert_mode_kills_a_line_and_yanks_it_with_ctrl_k_and_ctrl_y() {
        let tmp = TempDir::new("emacs-kill");
        let mut page = tmp.page("abc def\n");
        press(&mut page, "i");
        press_alt(&mut page, KeyCode::Char('f'));
        press_ctrl(&mut page, 'k');
        assert_eq!(page.buffer().lines(), ["abc"]);
        press(&mut page, "X");
        press_ctrl(&mut page, 'y');
        assert_eq!(page.buffer().lines(), ["abcX def"]);
    }

    #[test]
    fn test_the_alt_chords_the_window_keeps_still_reach_it() {
        // `Alt-h` and `Alt-l` move focus, and `Alt-1` picks a pane; claiming
        // them for editing would strand the window chords inside a message.
        let tmp = TempDir::new("emacs-window");
        let mut page = tmp.page("abc\n");
        press(&mut page, "i");
        for c in ['h', 'j', 'k', 'l', '1'] {
            assert_eq!(
                press_alt(&mut page, KeyCode::Char(c)),
                PageOutcome::Ignored,
                "{c}"
            );
        }
        assert_eq!(page.buffer().lines(), ["abc"]);
    }

    #[test]
    fn test_the_space_bar_types_a_space_in_insert_and_after_r() {
        // Regression: the space bar is a key code of its own, which insert
        // mode did not match, so a commit message could not hold a space.
        let tmp = TempDir::new("space");
        let mut page = tmp.page("ab\n");
        press(&mut page, "i");
        page.on_key(&key(KeyCode::Space));
        press(&mut page, "c");
        assert_eq!(page.buffer().lines(), [" cab"]);

        page.on_key(&key(KeyCode::Escape));
        press(&mut page, "0lr");
        page.on_key(&key(KeyCode::Space));
        assert_eq!(
            page.buffer().lines(),
            ["  ab"],
            "r then space replaces the c"
        );
    }

    #[test]
    fn test_a_key_brings_the_view_back_to_the_cursor() {
        let tmp = TempDir::new("wheel-key");
        let text: String = (0..40).map(|n| format!("line {n}\n")).collect();
        let mut page = tmp.page(&text);
        painted_page(&mut page, 11);
        page.on_scroll(-20);
        painted_page(&mut page, 11);
        assert!(page.doc().scroll > 0);
        press(&mut page, "l");
        painted_page(&mut page, 11);
        assert_eq!(page.doc().scroll, 0, "the cursor is in view again");
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
            page.modal.message().unwrap_or_default().contains("not set"),
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
        assert_eq!(change.modal.mode(), EditMode::Insert);
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
        assert_eq!(page.modal.mode(), EditMode::Insert);
        press(&mut page, "a");
        assert_eq!(page.buffer().lines(), ["abc"]);

        page.on_key(&key(KeyCode::Escape));
        assert_eq!(page.modal.mode(), EditMode::Normal);
        assert_eq!(page.buffer().col(), 0, "back onto the character just typed");
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

    fn message_page(tmp: &TempDir) -> (EditorPage, PathBuf) {
        let path = tmp.0.join("COMMIT_EDITMSG");
        std::fs::write(&path, "\n# Please enter\n").expect("writes");
        (EditorPage::new(path.clone(), None).expect("opens"), path)
    }

    #[test]
    fn test_a_commit_message_opens_ready_to_type_and_c_c_c_c_saves_and_closes() {
        let tmp = TempDir::new("finish");
        let (mut page, path) = message_page(&tmp);
        assert_eq!(page.modal.mode(), EditMode::Insert);

        press(&mut page, "fix");
        assert_eq!(page.on_key(&ctrl('c')), PageOutcome::Consumed);
        assert_eq!(page.on_key(&ctrl('c')), PageOutcome::Close);
        assert_eq!(
            std::fs::read_to_string(path).expect("reads"),
            "fix\n# Please enter\n"
        );
    }

    #[test]
    fn test_ctrl_shift_c_saves_and_closes_a_commit_message() {
        let tmp = TempDir::new("ctrl-shift-c");
        let (mut page, path) = message_page(&tmp);
        press(&mut page, "fix");
        // The window reports the capital with Shift held, which is what a
        // real key press looks like.
        let chord = Key {
            code: KeyCode::Char('C'),
            shift: true,
            ..ctrl('c')
        };
        assert_eq!(page.on_key(&chord), PageOutcome::Close);
        assert_eq!(
            std::fs::read_to_string(&path).expect("reads"),
            "fix\n# Please enter\n"
        );

        let (mut page, _) = message_page(&tmp);
        let lower = Key {
            shift: true,
            ..ctrl('c')
        };
        assert_eq!(page.on_key(&lower), PageOutcome::Close);
    }

    #[test]
    fn test_c_c_c_k_and_zq_empty_a_commit_message_so_git_aborts() {
        let tmp = TempDir::new("cancel");
        let (mut page, path) = message_page(&tmp);
        press(&mut page, "fix");
        page.on_key(&ctrl('c'));
        assert_eq!(page.on_key(&ctrl('k')), PageOutcome::Close);
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), "\n");

        let (mut page, path) = message_page(&tmp);
        press(&mut page, "x");
        page.on_key(&key(KeyCode::Escape));
        press(&mut page, "ZQ");
        assert_eq!(std::fs::read_to_string(path).expect("reads"), "\n");
    }

    #[test]
    fn test_a_lone_ctrl_c_is_forgotten_by_the_next_key() {
        let tmp = TempDir::new("chord");
        let (mut page, path) = message_page(&tmp);
        page.on_key(&ctrl('c'));
        press(&mut page, "a");
        assert_eq!(page.on_key(&ctrl('c')), PageOutcome::Consumed);
        assert_eq!(
            std::fs::read_to_string(path).expect("reads"),
            "\n# Please enter\n",
            "nothing was saved"
        );
    }

    #[test]
    fn test_recalling_swaps_only_the_message_above_the_footer() {
        let tmp = TempDir::new("recall");
        let path = tmp.0.join("COMMIT_EDITMSG");
        std::fs::write(&path, "\n# Please enter\n# a message\n").expect("writes");
        let mut page = EditorPage::new(path, None).expect("opens");
        page.recall = MessageRecall::with_history(&["old", "new"]);

        page.recall_message(true);
        assert_eq!(
            page.buffer().lines(),
            ["new", "# Please enter", "# a message"]
        );
        page.recall_message(true);
        assert_eq!(page.buffer().lines()[0], "old");
        page.recall_message(false);
        page.recall_message(false);
        assert_eq!(
            page.buffer().lines(),
            ["", "# Please enter", "# a message"],
            "stepping past the newest restores what was typed"
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

    #[test]
    fn test_editor_jump_targets_preview_confirm_and_cancel() {
        let tmp = TempDir::new("editor_jump");
        let mut page = tmp.page("first line\n\nthird line\nfourth line\n");
        let targets = page.jump_targets().expect("jump targets");
        assert_eq!(targets.len(), 3);
        assert_eq!(targets[0].0, 0);
        assert_eq!(targets[1].0, 2);
        assert_eq!(targets[2].0, 3);
        assert_eq!(page.jump_cursor(), Some(0));

        // Live preview to line 2
        page.jump_to(2);
        assert_eq!(page.buffer().row(), 2);

        // Cancel restores cursor to line 0
        page.jump_cancel();
        assert_eq!(page.buffer().row(), 0);

        // With painted viewport (e.g. 10 rows), preview centers at viewport / 2
        page.content(10, 40, false);
        page.jump_to(3);
        assert_eq!(page.buffer().row(), 3);
        assert_eq!(page.doc().scroll, 3_usize.saturating_sub(10 / 2));

        // Preview to line 3 and confirm
        page.jump_confirm(3);
        assert_eq!(page.buffer().row(), 3);
        assert_eq!(page.doc().scroll, 3_usize.saturating_sub(10 / 2));
        assert_eq!(page.doc().marks.jump, Some((0, 0)));
    }

    fn page_key(page: &mut EditorPage, code: KeyCode) {
        page.on_key(&key(code));
    }
}

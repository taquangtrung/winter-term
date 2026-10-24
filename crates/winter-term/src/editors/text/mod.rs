//! Editor: one file as editable text in a tab, opened from whichever editor
//! was pointing at it.
//!
//! - `comment`: commenting lines out and back in.
//! - `commit`: recalling earlier commit messages into one being written.
//! - `rebase`: the actions and checks of an interactive rebase's plan.
//! - [`file`]: reading a file in and writing it back.
//! - `highlight`: which scopes of the file's grammar each run of characters
//!   sits in, and so what color it takes.
//! - [`keys`]: what every key does to either.
//! - `remote`: reading and writing a file as root, in a container, or over
//!   SSH, as its path says.

mod comment;
mod commit;
pub mod file;
mod highlight;
pub mod keys;
mod rebase;
mod remote;

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::control::EditWaiter;
use crate::editors::git::exec;
use crate::model::editor::{
    Editor, EditorHint, EditorMenuItem, EditorOutcome, EditorStatus, JobReply, JumpTarget, Notice,
    OpenTarget, PromptMode, PromptReply, PromptRequest, SearchReport,
};
use crate::model::input::table::KeyHelp;
use crate::model::input::{Key, KeyCode};
use crate::model::paint::{
    row_height, wheel_scroll, window_anchor, wrap_window, wrapped_lines, Caret, Frame, IconKind,
    Point, Row, Span, Style,
};
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::graphemes::char_width;
use crate::model::textedit::modal::{EditMode, EditView, Marks, ModalEditor, Selection};

use commit::MessageRecall;
use file::{FileShape, FileStamp, LoadedFile};
use highlight::{Grammar, LineState};
use remote::{ASK_AS_ROOT, ASK_PASSWORD_OPEN, ASK_PASSWORD_SAVE};

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

/// Width the line numbers are padded to, and the gap after them, so the text
/// starts in the same column whether the file has ten lines or ten thousand.
const NUMBER_WIDTH: usize = 4;
const NUMBER_GAP: &str = " ";

/// How many columns a tab occupies on screen. Display only: what is in the
/// file stays one tab character.
const TAB_WIDTH: usize = 4;

/// How long one paint may spend working out what the lines above the window
/// leave open, and one tick spend catching up on what a paint did not reach.
/// A file far longer than these allow is colored over a few frames, from the
/// top down, rather than holding the window still while it is read.
const LEARN_PAINT_BUDGET: Duration = Duration::from_millis(8);
const LEARN_TICK_BUDGET: Duration = Duration::from_millis(25);

/// How soon to come back to coloring that is still catching up.
const LEARN_TICK: Duration = Duration::from_millis(10);

/// The command that says what the grammar makes of the character under the
/// cursor, for the palette and a `keybindings.kdl` `editor` block.
pub(crate) const SHOW_SCOPES: &str = "show_scopes";

/// The grammar a commit message is read with.
const COMMIT_GRAMMAR: &str = "Git Commit";

/// The grammar a rebase plan is read with.
const REBASE_GRAMMAR: &str = "Git Rebase Todo";

// ========================================================================
// Data Structures
// ========================================================================

/// The files open for editing, the keys that act on them, and everything the
/// two share: one yank reaches every buffer, as it does in Vim.
#[derive(Debug)]
pub struct TextEditor {
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
    /// The first key of a commit message's or a rebase plan's sequence
    /// (`C-c`), while it waits for the key that completes it.
    chord_start: Option<Key>,
    /// Where `M-p`/`M-n` have got to through earlier commit messages, for
    /// a buffer that is one.
    recall: MessageRecall,
    /// How many rows of text the pane last had room for, so the paging keys
    /// can move by what is actually on screen.
    viewport: usize,
    /// Whether the wheel, not the cursor, last placed the view: the view then
    /// stays where it was put until a key brings the cursor back into it.
    wheel: bool,
    /// A program blocked until this editor is done, such as `git commit` waiting
    /// on its message. Answered when the editor is dropped.
    waiter: Option<EditWaiter>,
}

/// One open file: its text, where the cursor and the window sit in it, and
/// what it looked like on disk when it was read.
#[derive(Debug)]
struct Document {
    buffer: TextBuffer,
    /// The first column painted, for lines wider than the pane.
    hscroll: usize,
    /// The line to land on once a file not yet read is read.
    land_on: Option<usize>,
    /// The marks `m` has set in this file, and where `''` goes back to.
    marks: Marks,
    /// Where the file is: a local path, or a remote one's URI (`ssh://host/x`,
    /// see [`crate::remote`]) that says how it is reached.
    path: PathBuf,
    scroll: usize,
    shape: FileShape,
    stamp: FileStamp,
    /// The last line the window wants colored, which the states are worked
    /// out as far as, over a few ticks for a long file.
    color_upto: usize,
    /// The style of each character of each line painted so far, so a paint
    /// reads a line through its grammar once rather than every frame. Kept,
    /// as the states are, until an edit makes it untrue.
    colors: Vec<Option<Vec<Style>>>,
    /// The grammar the file is read with, or `None` for a file whose
    /// language this does not know.
    grammar: Option<Grammar>,
    /// What each line begins inside, one entry per line worked out so far.
    /// Kept because a comment opened on one line colors every line until it
    /// closes, so a line cannot be colored without knowing what precedes it.
    states: Vec<LineState>,
    /// Whether the file has yet to be read: one only root can read, waiting
    /// on the password, or a remote one restored with a session, read when
    /// first shown. The buffer stands empty and takes no edit meanwhile.
    unread: bool,
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
// TextEditor: state
// ========================================================================

impl TextEditor {
    /// Open `path`'s contents for editing, landing on `line` when whatever
    /// asked for the file knew which one mattered.
    pub fn new(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        // A commit message is for typing into, so it opens ready to type.
        let mode = match commit::is_message_file(&path) {
            true => EditMode::Insert,
            false => EditMode::Normal,
        };
        Ok(Self::with_doc(Document::open(path, line)?, mode))
    }

    /// An editor holding `doc` alone, its keys starting in `mode`.
    fn with_doc(doc: Document, mode: EditMode) -> Self {
        Self {
            at: 0,
            docs: vec![doc],
            jump_origin: None,
            chord_start: None,
            modal: ModalEditor::new().with_mode(mode),
            mru: vec![0],
            mru_walk: None,
            painted: Vec::new(),
            recall: MessageRecall::default(),
            viewport: 0,
            wheel: false,
            waiter: None,
        }
    }

    /// [`Self::new`] for a saved session coming back: a remote file is not
    /// read until it is first shown, so a restart does not wait on every
    /// host and container a session had open, or ask for a password before
    /// the window is up.
    pub fn restored(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        if crate::remote::remote_of(&path).is_none() {
            return Self::new(path, line);
        }
        Ok(Self::with_doc(
            Document::unread(path, line),
            EditMode::Normal,
        ))
    }

    /// Whether this editor is editing the message of a commit about to be made,
    /// which is the one message worth showing the staged changes beside.
    pub(crate) fn edits_commit(&self) -> bool {
        commit::is_commit_message(&self.doc().path)
    }

    /// Have `waiter` answered when this editor ends, which is what lets a
    /// program that launched the editor and blocked on it carry on.
    pub(crate) fn with_waiter(mut self, waiter: EditWaiter) -> Self {
        self.waiter = Some(waiter);
        self
    }

    /// Tell whoever is waiting on this editor that its edit was thrown away,
    /// when closing the buffer closes the editor.
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
    /// buffer is not a message, or closing it would not end the editor.
    pub(super) fn cancel_message(&mut self) -> Option<EditorOutcome> {
        if self.docs.len() > 1 || !commit::is_message_file(&self.doc().path) {
            return None;
        }
        let rows = self.buffer().lines().len();
        self.buffer_mut().replace_rows(0, rows, vec![String::new()]);
        self.write();
        Some(EditorOutcome::Close)
    }

    /// `C-M-p`/`C-M-n` in a commit message: swap the message above git's
    /// comment footer for an earlier one (`back`) or the one after it. The
    /// first step back asks git for the history and is taken when it comes
    /// back (see [`Editor::on_job`]).
    pub(super) fn recall_message(&mut self, back: bool) -> EditorOutcome {
        if !commit::is_message_file(&self.doc().path) {
            return EditorOutcome::Consumed;
        }
        if back {
            let path = self.doc().path.clone();
            if let Some(request) = self.recall.history_request(&path) {
                return EditorOutcome::Job(request);
            }
        }
        let rows = commit::message_rows(self.buffer().lines());
        let typed = self.buffer().lines()[..rows].join("\n");
        let recalled = match back {
            true => self.recall.previous(typed),
            false => self.recall.next(),
        };
        if let Some(message) = recalled {
            let lines = message.split('\n').map(str::to_string).collect();
            self.buffer_mut().replace_rows(0, rows, lines);
        }
        EditorOutcome::Consumed
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
            Err(e) => self.modal.report(Notice::error(e.to_string())),
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
    /// edits that are not on disk, beside the icon naming its file type.
    pub(super) fn buffer_list(&self) -> Vec<(String, IconKind)> {
        // Most recently used first, so the file being bounced to and from sits
        // at the top rather than wherever it happened to be opened. An index
        // the order has drifted out of step with is dropped rather than
        // panicking on it, and anything the order missed is appended.
        let named = |doc: &Document| {
            let label = match doc.buffer.is_dirty() {
                true => format!("{}{DIRTY_MARK}", doc.path.display()),
                false => doc.path.display().to_string(),
            };
            let name = doc
                .path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            (label, IconKind::File { name })
        };
        let mut listed: Vec<(String, IconKind)> = self
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
    fn save(&mut self) -> EditorOutcome {
        if remote::is_stale(&self.doc().path, &self.doc().stamp) {
            return EditorOutcome::Prompt(PromptRequest {
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
    fn write(&mut self) -> EditorOutcome {
        self.write_file()
    }

    /// Take the file as written, now reading as `stamp`, and go back to
    /// Normal mode: a save ends a stretch of typing.
    fn mark_written(&mut self, stamp: FileStamp) {
        self.doc_mut().stamp = stamp;
        self.buffer_mut().mark_saved();
        self.leave_to_normal();
        let wrote = lines_written(self.buffer());
        self.modal.report(Notice::info(format!("wrote {wrote}")));
    }

    /// Back to Normal mode from whichever has the keyboard, the way `Esc`
    /// leaves each: a typing cursor steps back onto the character it was
    /// past, and a selection is let go.
    fn leave_to_normal(&mut self) {
        if self.modal.mode() == EditMode::Normal {
            return;
        }
        let escape = Key {
            alt: false,
            code: KeyCode::Escape,
            ctrl: false,
            shift: false,
            sup: false,
        };
        self.with_modal(|modal, view| modal.on_key(view, &escape));
    }

    /// Hand the file to `$EDITOR` in a tab of its own, at the line under the
    /// cursor, for the work this editor deliberately cannot do. Unsaved edits
    /// would not be there, so they go to disk first.
    fn escalate(&mut self) -> EditorOutcome {
        if let Some(at) = crate::remote::remote_of(&self.doc().path) {
            self.modal.report(Notice::error(format!(
                "$EDITOR cannot reach a file on {}",
                at.label()
            )));
            return EditorOutcome::Consumed;
        }
        if self.buffer().is_dirty() {
            self.write();
        }
        EditorOutcome::SpawnEditor(OpenTarget::at_line(
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
    /// knew which one mattered. A file only root can read becomes a buffer
    /// of root's, and where sudo wants a password first it waits unread.
    fn open(path: PathBuf, line: Option<usize>) -> io::Result<Self> {
        let (path, loaded) = remote::read_for_open(path)?;
        let mut doc = Self::unread(path, line);
        if let Some(loaded) = loaded {
            doc.buffer = TextBuffer::new(loaded.lines);
            if let Some(line) = doc.land_on.take() {
                doc.buffer.move_to_line(line.saturating_sub(1));
            }
            doc.shape = loaded.shape;
            doc.stamp = loaded.stamp;
            doc.unread = false;
            doc.learn_grammar();
        }
        Ok(doc)
    }

    /// A file whose name says nothing of its language may say it on its
    /// first line: a script's shebang, a modeline.
    fn learn_grammar(&mut self) {
        if self.grammar.is_none() {
            self.grammar = self
                .buffer
                .lines()
                .first()
                .and_then(|line| highlight::of_first_line(line));
        }
    }

    /// A buffer for `path` that has not read it yet, to land on `line` once
    /// it does.
    fn unread(path: PathBuf, line: Option<usize>) -> Self {
        Self {
            buffer: TextBuffer::new(vec![String::new()]),
            hscroll: 0,
            land_on: line,
            marks: Marks::default(),
            scroll: 0,
            shape: FileShape::default(),
            stamp: FileStamp::default(),
            color_upto: 0,
            colors: Vec::new(),
            grammar: grammar_of(&path),
            states: Vec::new(),
            path,
            unread: true,
        }
    }

    /// Whether lines the window wants colored are still to be worked out.
    fn learning(&self) -> bool {
        let wanted = self
            .color_upto
            .min(self.buffer.lines().len().saturating_sub(1));
        self.grammar.is_some() && self.states.len() <= wanted
    }

    /// Work out what every line up to the one the window wants colored
    /// leaves open, as far as it is not already known and `budget` allows.
    /// A line's color depends on every line above it, so this is the price
    /// of the first look at a part of the file; what it learns is kept until
    /// an edit makes it untrue.
    fn learn_states(&mut self, budget: Duration) {
        if let Some(changed) = self.buffer.take_change() {
            self.states.truncate(changed + 1);
            self.colors.truncate(changed);
        }
        let Some(grammar) = self.grammar else {
            return;
        };
        if self.states.is_empty() {
            self.states.push(LineState::start(grammar));
        }
        let started = Instant::now();
        let lines = self.buffer.lines();
        while self.states.len() <= self.color_upto && self.states.len() < lines.len() {
            let index = self.states.len() - 1;
            let next = highlight::next_state(&lines[index], &self.states[index]);
            self.states.push(next);
            if started.elapsed() >= budget {
                break;
            }
        }
    }

    /// Work out the styles of lines `from..to`, as far as their states are
    /// known and their styles are not.
    ///
    /// A commit message's summary is marked past the width it reads well at,
    /// which is git's convention rather than anything its grammar says.
    fn color_lines(&mut self, from: usize, to: usize) {
        let known = self.buffer.lines().iter().zip(&self.states);
        for (index, (line, state)) in known.enumerate().take(to).skip(from) {
            if self.colors.get(index).is_some_and(Option::is_some) {
                continue;
            }
            let (mut styles, _) = highlight::highlight(line, state);
            if index == 0 && commit::is_message_file(&self.path) {
                for style in styles.iter_mut().skip(commit::SUMMARY_WIDTH) {
                    *style = Style::Removed;
                }
            }
            if self.colors.len() <= index {
                self.colors.resize(index + 1, None);
            }
            self.colors[index] = Some(styles);
        }
    }

    /// Start the file's coloring over, for text that has replaced the old.
    fn forget_colors(&mut self) {
        self.states = Vec::new();
        self.colors = Vec::new();
    }
}

// ========================================================================
// TextEditor: painting
// ========================================================================

impl TextEditor {
    /// One line of the file: its number, then as much of its text as the pane
    /// has room for. The caret is the host's to draw, on the cell this editor
    /// reports, so it takes the shape and the color every other caret in the
    /// window has.
    fn text_row(&self, index: usize, cols: usize) -> Row {
        let number = Span::new(
            Style::Dim,
            format!("{:>NUMBER_WIDTH$}{NUMBER_GAP}", index + 1),
        );
        let line = self.line_of(index);
        let colors = self.colors_of(index);
        // Tabs are opened out for the screen, and every column they open into
        // carries what the tab itself was, so a run of color survives one.
        let mut cells: Vec<(char, Style)> = Vec::new();
        for (offset, c) in line.chars().enumerate() {
            let style = colors.get(offset).copied().unwrap_or_default();
            match c == '\t' {
                true => cells.extend(std::iter::repeat_n((' ', style), TAB_WIDTH)),
                false => cells.push((c, style)),
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

    /// The style of each character of line `index`, as the last paint worked
    /// them out. A line not worked out yet, and every line of a file in no
    /// language this knows, is plain text.
    fn colors_of(&self, index: usize) -> &[Style] {
        self.doc()
            .colors
            .get(index)
            .and_then(Option::as_deref)
            .unwrap_or_default()
    }

    /// Say what the grammar makes of the character under the cursor: its
    /// scopes and the rows of the theme's scope table that color it, which
    /// is what the `syntax` colors are named by.
    fn show_scopes(&mut self) -> EditorOutcome {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let doc = self.doc_mut();
        doc.color_upto = doc.color_upto.max(row);
        doc.learn_states(Duration::MAX);
        let notice = match self.doc().states.get(row) {
            Some(state) => Notice::info(highlight::describe_at(&self.line_of(row), state, col)),
            None => Notice::info("no grammar colors this file"),
        };
        self.modal.report(notice);
        EditorOutcome::Consumed
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
    /// before it are opened out and each character before it takes the cells
    /// it is drawn in: two for a wide one, none for a combining mark.
    fn display_of(&self, index: usize, col: usize) -> usize {
        self.line_of(index).chars().take(col).map(cell_width).sum()
    }

    /// The character of line `index` painted at display column `display`,
    /// which is what a click on that cell is pointing at: the one whose
    /// cells reach past it, so a click on either half of a wide character
    /// lands on that character.
    fn char_at_display(&self, index: usize, display: usize) -> usize {
        let mut at = 0;
        for (col, c) in self.line_of(index).chars().enumerate() {
            let width = cell_width(c);
            if at + width > display {
                return col;
            }
            at += width;
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
// TextEditor: the pane it is read through
// ========================================================================

impl TextEditor {
    /// Keep what was painted where, so a click can be answered with the
    /// character that was under the pointer.
    fn remember_layout(&mut self, lines: &[usize], cols: usize, wrap: bool) {
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        let mut painted = Vec::new();
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
    fn point_at(&mut self, at: Point) -> EditorOutcome {
        let Some(spot) = self.painted.get(at.row).copied() else {
            return EditorOutcome::Ignored;
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
        EditorOutcome::Consumed
    }

    /// Read the file again where it has changed underneath an editor with
    /// nothing unsaved in it: what is on screen would otherwise be a copy of
    /// something that is no longer there.
    ///
    /// A file not yet read is read now, asking for the password where sudo
    /// wants one.
    fn reread(&mut self) -> EditorOutcome {
        if self.doc().unread {
            return self.read_in();
        }
        if self.buffer().is_dirty() || !remote::is_stale(&self.doc().path, &self.doc().stamp) {
            return EditorOutcome::Consumed;
        }
        let Ok(loaded) = remote::load(&self.doc().path) else {
            return EditorOutcome::Consumed;
        };
        self.replace_text(loaded);
        self.modal.report(Notice::info("changed on disk, re-read"));
        EditorOutcome::Consumed
    }

    /// Put `loaded` in place of the buffer's text, keeping the cursor where
    /// it was as far as the new text reaches.
    fn replace_text(&mut self, loaded: LoadedFile) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        self.doc_mut().buffer = TextBuffer::new(loaded.lines);
        self.buffer_mut().move_to(row, col);
        self.doc_mut().shape = loaded.shape;
        self.doc_mut().stamp = loaded.stamp;
        self.doc_mut().forget_colors();
        let at = self.at.min(self.docs.len() - 1);
        self.modal.reset(&self.docs[at].buffer);
    }
}

impl Editor for TextEditor {
    fn title(&self) -> String {
        name_of(&self.doc().path)
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::File {
            name: name_of(&self.doc().path),
        })
    }

    /// Any buffer with unwritten edits, not just the one being read: closing
    /// the editor takes every buffer it holds with it.
    fn is_dirty(&self) -> bool {
        self.docs.iter().any(|doc| doc.buffer.is_dirty())
    }

    /// A program blocked on this edit, such as `git commit` waiting on its
    /// message, hears that it is over now rather than whenever the kept editor
    /// is finally dropped.
    fn on_closed(&mut self) {
        self.waiter = None;
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let text_cols = cols.saturating_sub(NUMBER_WIDTH + NUMBER_GAP.len());
        self.viewport = rows;
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
        self.doc_mut().color_upto = window.start + window.count;
        self.doc_mut().learn_states(LEARN_PAINT_BUDGET);
        self.doc_mut()
            .color_lines(window.start, window.start + window.count);
        let lines: Vec<usize> = (window.start..(window.start + window.count).min(total)).collect();
        let frame_rows: Vec<Row> = lines
            .iter()
            .map(|index| self.text_row(*index, text_cols))
            .collect();
        self.remember_layout(&lines, text_cols, wrap);
        // A wrapped line picks up under its own text rather than under the
        // line numbers, so the gutter stays a gutter.
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        let indents = vec![gutter; frame_rows.len()];
        let content = Frame::new(frame_rows).with_wrap_indents(indents);
        match anchor == self.buffer().row() {
            true => content.with_cursor_line(window.cursor),
            false => content,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        // The one job an editor asks for: the commit messages a recall
        // steps back through.
        let JobReply::Command(output) = reply else {
            return EditorOutcome::Consumed;
        };
        if output.tag != exec::TAG_MESSAGES {
            return EditorOutcome::Consumed;
        }
        let log = exec::succeeded(&output).then_some(output.stdout.as_str());
        match self.recall.take_history(log) {
            true => self.recall_message(true),
            false => EditorOutcome::Consumed,
        }
    }

    /// Coloring that a paint did not reach in time, in a long file, catches
    /// up a tick at a time.
    fn next_tick(&self) -> Option<Instant> {
        self.doc().learning().then(|| Instant::now() + LEARN_TICK)
    }

    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        self.doc_mut().learn_states(LEARN_TICK_BUDGET);
        EditorOutcome::Consumed
    }

    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        match name {
            SHOW_SCOPES => Some(self.show_scopes()),
            _ => None,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.modal.take_notice()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        let doc = &self.docs[self.at.min(self.docs.len() - 1)];
        self.modal.take_search(&doc.buffer)
    }

    /// Where the cursor is and what is part-typed, led by what the tab does
    /// not say: the host a remote file is on, and which of the open files
    /// this is when there are several. Leading, they are the first the bar
    /// drops when short of room.
    fn status(&self) -> EditorStatus {
        let total = format!("{} lines", self.buffer().lines().len());
        let mut status = self.modal.editor_status(self.buffer(), total);
        let host = crate::remote::remote_of(&self.doc().path).map(|at| format!("[{}]", at.label()));
        let count =
            (self.docs.len() > 1).then(|| format!("{} of {}", self.at + 1, self.docs.len()));
        status.segments.splice(0..0, host.into_iter().chain(count));
        status
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.wheel = false;
        // The space bar is typed text here, in insert mode and after `f`, `r`
        // and the like, all of which match characters.
        self.dispatch(&key.typed())
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        self.menu_items()
    }

    /// A right-click while something is selected keeps it, for the menu's
    /// Copy and Cut to act on; with nothing selected it moves the cursor
    /// there, as a click does, for Paste to land where it was asked.
    fn on_context_mouse(&mut self, at: Point) -> EditorOutcome {
        if self.modal.is_selecting() {
            return EditorOutcome::Consumed;
        }
        self.on_mouse(at)
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        self.wheel = false;
        self.point_at(at)
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        // The view moves and the cursor stays where it was, so the text can be
        // read without losing the place being edited; the next key brings the
        // view back to it.
        let total = self.buffer().lines().len();
        let scroll = wheel_scroll(self.doc().scroll, lines, total, self.viewport);
        self.doc_mut().scroll = scroll;
        self.wheel = true;
        EditorOutcome::Consumed
    }

    fn on_paste(&mut self, text: String) -> EditorOutcome {
        self.wheel = false;
        if self.doc().unread {
            return self.read_in();
        }
        self.paste_text(text)
    }

    fn on_resume(&mut self) -> EditorOutcome {
        self.reread()
    }

    fn open_file(&mut self, target: OpenTarget) -> bool {
        // A file opened while an editor is up joins it as another buffer,
        // rather than covering it with an editor of its own.
        self.open(target.path, target.line);
        true
    }

    fn caret(&self) -> Option<Caret> {
        Some(Caret {
            col: self.caret_col(),
            insert: self.modal.is_typing(),
        })
    }

    fn hint(&self) -> Option<EditorHint> {
        Some(EditorHint {
            items: self.modal.hint()?,
            title: "editor".to_string(),
        })
    }

    fn keys(&self) -> Vec<KeyHelp> {
        self.key_help()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
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
                EditorOutcome::Consumed
            }
            ASK_OVERWRITE if said_yes => self.write(),
            ASK_AS_ROOT | ASK_PASSWORD_OPEN | ASK_PASSWORD_SAVE => self.answer_elsewhere(reply),
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
                    EditorOutcome::Consumed
                }
            },
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if (tag == ASK_SEARCH || tag == ASK_SEARCH_BACK) && !query.is_empty() {
            if self.jump_origin.is_none() {
                self.jump_origin =
                    Some((self.buffer().row(), self.buffer().col(), self.doc().scroll));
            }
            self.start_search(query.to_string(), tag == ASK_SEARCH)
        } else {
            EditorOutcome::Consumed
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

    fn holds_file(&self, path: &Path) -> bool {
        self.docs.iter().any(|doc| doc.path == path)
    }

    fn is_commit_editor(&self) -> bool {
        self.edits_commit()
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The buffer being edited, not the one the editor opened with, so
        // stepping through buffers takes the answer with it. A remote file's
        // directory is nowhere a local shell can start in.
        if crate::remote::remote_of(&self.doc().path).is_some() {
            return None;
        }
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

    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        let lines = self.buffer().lines();
        let mut targets: Vec<JumpTarget> = lines
            .iter()
            .enumerate()
            .filter(|(_, line)| !line.trim().is_empty())
            .map(|(idx, line)| JumpTarget::new(idx, format!("{:>5}  {}", idx + 1, line.trim())))
            .collect();
        if targets.is_empty() && !lines.is_empty() {
            targets = lines
                .iter()
                .enumerate()
                .map(|(idx, line)| JumpTarget::new(idx, format!("{:>5}  {}", idx + 1, line)))
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

/// How many cells `c` paints in: a tab opened out, or what the character
/// itself takes (see [`char_width`]).
fn cell_width(c: char) -> usize {
    match c {
        '\t' => TAB_WIDTH,
        _ => char_width(c),
    }
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
/// A selected run keeps its syntax color and takes the selection's
/// background, the way a terminal selection does.
fn runs(cells: &[(char, Style)], cols: usize, marked: Option<(usize, usize)>) -> Row {
    let mut spans: Vec<Span> = Vec::new();
    for (at, (c, style)) in cells.iter().take(cols).enumerate() {
        let selected = marked.is_some_and(|(start, end)| at >= start && at < end);
        match spans.last_mut() {
            Some(last) if last.style == *style && last.selected == selected => last.text.push(*c),
            _ => {
                let span = Span::new(*style, c.to_string());
                spans.push(if selected { span.selected() } else { span });
            }
        }
    }
    // A selection that reaches past the line's last character shows as one
    // marked cell there, so an empty line in a linewise selection is visible.
    if let Some((start, end)) = marked {
        if cells.len() < end && cells.len() >= start && cells.len() < cols {
            spans.push(Span::plain(" ").selected());
        }
    }
    spans
}

/// The grammar `path` is read with: git's own for the files git hands over,
/// else whatever the file's name or extension names.
fn grammar_of(path: &Path) -> Option<Grammar> {
    if commit::is_message_file(path) {
        return highlight::named(COMMIT_GRAMMAR);
    }
    if rebase::is_rebase_todo(path) {
        return highlight::named(REBASE_GRAMMAR);
    }
    highlight::of_path(path)
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
mod tests;

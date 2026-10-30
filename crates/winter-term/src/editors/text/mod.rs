//! Editor: one file as editable text in a tab, opened from whichever editor
//! was pointing at it.
//!
//! - `comment`: commenting lines out and back in.
//! - `commit`: recalling earlier commit messages into one being written.
//! - `compare`: comparing a buffer with the file on disk, the clipboard, or
//!   another buffer, in a diff beside it.
//! - `conflict`: the merge conflicts a file holds, and resolving them.
//! - `rebase`: the actions and checks of an interactive rebase's plan.
//! - [`text_file`](crate::editors::text_file), shared with the editors that write files too: reading a file in and writing it back.
//! - `outline`: the headings, sections, or definitions a file holds.
//! - `swatch`: the colors a file writes, drawn on the color each names.
//! - `context`: typing by context: comments and list items carried on
//!   to the next line, headings and bullets indented by `TAB`.
//! - `occur`: the word under the cursor, lit wherever it occurs and
//!   changed everywhere at once.
//! - `structure`: the definition the cursor is in, and the blocks the
//!   indentation makes, for moving by them.
//! - `lsp`: the file's language server, once one is running: its
//!   problems drawn under the text, and what the keys ask of it.
//! - `latex` and `markdown`: reading and changing prose in either, and
//!   `writing`: the editor's commands over them.
//! - `highlight`: which scopes of the file's grammar each run of characters
//!   sits in, and so what color it takes.
//! - [`keys`]: what every key does to either.
//! - `remote`: reading and writing a file as root, in a container, or over
//!   SSH, as its path says.

mod bibtex;
mod buffers;
mod comment;
mod commit;
mod compare;
mod complete;
mod completing;
mod conceal;
mod conflict;
mod context;
mod copilot;
mod diffing;
mod document;
mod fixes;
pub mod keys;
mod latex;
mod lookup;
mod lsp;
mod markdown;
mod occur;
mod outline;
mod painting;
mod problems;
mod query_replace;
mod rebase;
mod remote;
mod spelling;
mod spelling_scan;
mod structure;
mod swatch;
mod symbols;
mod view;
mod wrap;
mod writing;

pub(crate) use buffers::save_through;
use std::io;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::{Duration, Instant};

use crate::control::EditWaiter;
use crate::editors::git_exec;
use crate::editors::synctex;
use crate::editors::text_file;
use crate::model::editor::{
    self as editor, CommandOutput, Editor, EditorCommand, EditorHint, EditorMenuItem, EditorOutcome, EditorRequest, EditorStatus, JobReply,
    JobRequest, JumpStep, JumpTarget, Notice, OpenTarget, Popups, PromptReply, PromptRequest,
    SearchReport, DIFF_BUFFER, DIFF_CLIPBOARD, DIFF_DISK, LSP_COMMANDS, MERGE_NEXT, MERGE_PREV, PROBLEM_COMMANDS,
    MERGE_RESOLVE, COPY_RICH, SAVE_ALL, SYNCTEX_FORWARD, TOGGLE_SWATCHES,
};
use crate::model::input::table::KeyHelp;
use crate::model::input::{Key, KeyCode};
use crate::model::paint::{
    place_view, wheel_scroll_rows, Caret, CursorRows, DiagnosticLevel, Place, RowWindow, ViewAim,
    Frame, IconKind, Point, Row, Span, Style, TAB_WIDTH,
};
use crate::model::textedit::buffer::TextBuffer;
use crate::model::textedit::graphemes::char_width;
use crate::model::textedit::modal::{EditMode, EditView, Marks, ModalEditor, Selection};

use commit::MessageRecall;
use crate::editors::text_file::{FileStamp, LoadedFile};
use crate::editors::highlight::{self, Grammar, LineState};
use remote::{ASK_AS_ROOT, ASK_PASSWORD_OPEN, ASK_PASSWORD_SAVE};

// ========================================================================
// Constants
// ========================================================================

const ASK_COMPARE_BUFFER: &str = "compare-buffer";
const ASK_CONFLICT: &str = "conflict";
const ASK_ENV_NEW: &str = "environment-new";
const ASK_ENV_RENAME: &str = "environment-rename";
const ASK_OVERWRITE: &str = "overwrite";
const ASK_SEARCH: &str = "search";
const ASK_SEARCH_BACK: &str = "search-back";

/// The commands the `editor` block of `keybindings.kdl` can bind: moving
/// by the code and its occurrences, and the language server's.
pub const COMMANDS: &[EditorCommand] = &[
    EditorCommand {
        label: "Recenter Top Bottom",
        name: editor::EDITOR_RECENTER,
    },
    EditorCommand {
        label: "Flash Current Line",
        name: editor::EDITOR_FLASH_LINE,
    },
    EditorCommand {
        label: "Go to Offset",
        name: editor::EDITOR_GOTO_OFFSET,
    },
    EditorCommand {
        label: "Display Current Offset",
        name: editor::EDITOR_SHOW_OFFSET,
    },
    EditorCommand {
        label: "Run Cargo Fix",
        name: editor::RUST_CARGO_FIX,
    },
    EditorCommand {
        label: "Insert Math or Greek Symbol...",
        name: editor::WRITING_INSERT_SYMBOL,
    },
    EditorCommand {
        label: "Look Up Word",
        name: editor::DICTIONARY_LOOKUP,
    },
    EditorCommand {
        label: "Sort Entries by Key (A-Z)",
        name: editor::BIBTEX_SORT_KEY_ASC,
    },
    EditorCommand {
        label: "Sort Entries by Key (Z-A)",
        name: editor::BIBTEX_SORT_KEY_DESC,
    },
    EditorCommand {
        label: "Sort Entries by Year (Earliest First)",
        name: editor::BIBTEX_SORT_YEAR_ASC,
    },
    EditorCommand {
        label: "Sort Entries by Year (Latest First)",
        name: editor::BIBTEX_SORT_YEAR_DESC,
    },
    EditorCommand {
        label: "Compare Buffer with File",
        name: editor::DIFF_FILE,
    },
    EditorCommand {
        label: "Compare Buffer with Revision",
        name: editor::DIFF_REVISION,
    },
    EditorCommand {
        label: "Compare Buffer with Two Files",
        name: editor::DIFF_FILES,
    },
    EditorCommand {
        label: "Merge Another File into This One",
        name: editor::MERGE_FILE,
    },
    EditorCommand {
        label: "Resolve Merge Conflicts in 3-Way Merge",
        name: editor::MERGE_CONFLICTS_3WAY,
    },
    EditorCommand {
        label: "Change All Occurrences",
        name: editor::EDITOR_CHANGE_OCCURRENCES,
    },
    EditorCommand {
        label: "Toggle Occurrence Highlights",
        name: editor::EDITOR_TOGGLE_OCCURRENCES,
    },
    EditorCommand {
        label: "Next Occurrence",
        name: editor::EDITOR_NEXT_OCCURRENCE,
    },
    EditorCommand {
        label: "Previous Occurrence",
        name: editor::EDITOR_PREV_OCCURRENCE,
    },
    EditorCommand {
        label: "Go to Start of Function/Module",
        name: editor::EDITOR_DEFINITION_START,
    },
    EditorCommand {
        label: "Go to End of Function/Module",
        name: editor::EDITOR_DEFINITION_END,
    },
    EditorCommand {
        label: "Select Current Function/Module",
        name: editor::EDITOR_SELECT_DEFINITION,
    },
    EditorCommand {
        label: "Go to Next Fold",
        name: editor::EDITOR_NEXT_BLOCK,
    },
    EditorCommand {
        label: "Go to Previous Fold",
        name: editor::EDITOR_PREV_BLOCK,
    },
    EditorCommand {
        label: "Go to Definition",
        name: editor::LSP_DEFINITION,
    },
    EditorCommand {
        label: "Go to Declaration",
        name: editor::LSP_DECLARATION,
    },
    EditorCommand {
        label: "Go to Type Definition",
        name: editor::LSP_TYPE_DEFINITION,
    },
    EditorCommand {
        label: "Go to Implementation",
        name: editor::LSP_IMPLEMENTATION,
    },
    EditorCommand {
        label: "Find References",
        name: editor::LSP_REFERENCES,
    },
    EditorCommand {
        label: "Show Hover",
        name: editor::LSP_HOVER,
    },
    EditorCommand {
        label: "Show Signature",
        name: editor::LSP_SIGNATURE,
    },
    EditorCommand {
        label: "Rename Symbol",
        name: editor::LSP_RENAME,
    },
    EditorCommand {
        label: "Quick Fix and Refactor",
        name: editor::LSP_CODE_ACTION,
    },
    EditorCommand {
        label: "Format Document",
        name: editor::LSP_FORMAT,
    },
    EditorCommand {
        label: "Next Problem in File",
        name: editor::PROBLEM_NEXT_IN_FILE,
    },
    EditorCommand {
        label: "Previous Problem in File",
        name: editor::PROBLEM_PREV_IN_FILE,
    },
    EditorCommand {
        label: "First Problem in Project",
        name: editor::PROBLEM_FIRST,
    },
    EditorCommand {
        label: "Next Problem in Project",
        name: editor::PROBLEM_NEXT,
    },
    EditorCommand {
        label: "Previous Problem in Project",
        name: editor::PROBLEM_PREV,
    },
    EditorCommand {
        label: "First Error in Project",
        name: editor::PROBLEM_FIRST_ERROR,
    },
    EditorCommand {
        label: "Next Error in Project",
        name: editor::PROBLEM_NEXT_ERROR,
    },
    EditorCommand {
        label: "Previous Error in Project",
        name: editor::PROBLEM_PREV_ERROR,
    },
    EditorCommand {
        label: "First Writing Issue in Project",
        name: editor::PROBLEM_FIRST_WRITING,
    },
    EditorCommand {
        label: "Next Writing Issue in Project",
        name: editor::PROBLEM_NEXT_WRITING,
    },
    EditorCommand {
        label: "Previous Writing Issue in Project",
        name: editor::PROBLEM_PREV_WRITING,
    },
    EditorCommand {
        label: "Problems in Project",
        name: editor::PROBLEM_LIST,
    },
    EditorCommand {
        label: "Navigate Back",
        name: editor::NAVIGATE_BACK,
    },
    EditorCommand {
        label: "Navigate Forward",
        name: editor::NAVIGATE_FORWARD,
    },
];

/// What marks a buffer holding edits that are not on disk, in the header
/// and in the list of open buffers.
pub(crate) const DIRTY_MARK: &str = " [+]";

/// Width the line numbers are padded to, and the gap after them, so the text
/// starts in the same column whether the file has ten lines or ten thousand.
const NUMBER_WIDTH: usize = 4;
const NUMBER_GAP: &str = " ";

/// How long one paint may spend working out what the lines above the window
/// leave open, and one tick spend catching up on what a paint did not reach.
/// A file far longer than these allow is colored over a few frames, from the
/// top down, rather than holding the window still while it is read.
const LEARN_PAINT_BUDGET: Duration = Duration::from_millis(8);
const LEARN_TICK_BUDGET: Duration = Duration::from_millis(25);

/// How soon to come back to coloring that is still catching up.
const LEARN_TICK: Duration = Duration::from_millis(10);

/// How often the file being edited is looked at on disk, so a change made to
/// it elsewhere (another editor, a formatter, a `git checkout`) is read in.
const DISK_CHECK: Duration = Duration::from_secs(1);

/// The command that says what the grammar makes of the character under the
/// cursor, for the palette and a `keybindings.kdl` `editor` block.
pub(crate) const SHOW_SCOPES: &str = crate::model::editor::SHOW_SCOPES;

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
    /// Where the cursor and the view's top were before jump previewing
    /// started: (row, col, (scroll, scroll_skip)).
    jump_origin: Option<(usize, usize, (usize, usize))>,
    /// The modal keys, and what outlives one buffer: the registers, the last
    /// search, the last change, and what the last command reported.
    modal: ModalEditor,
    /// The lines the last paint laid out, and which of them show on the
    /// pane's top, middle, and bottom screen rows, for `H`, `M`, and `L`.
    laid_out: RowWindow,
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
    /// What this editor has asked of the window since it last asked: places
    /// and edits in other files.
    requests: Vec<EditorRequest>,
    viewport: usize,
    /// Whether the wheel, not the cursor, last placed the view: the view then
    /// stays where it was put until a key brings the cursor back into it.
    wheel: bool,
    /// A program blocked until this editor is done, such as `git commit` waiting
    /// on its message. Answered when the editor is dropped.
    waiter: Option<EditWaiter>,
    /// Whether the TextMate token & scope inspect popup is active under the cursor.
    inspect_scopes: bool,
    /// The PDF a SyncTeX forward search is asking about, while it runs.
    synctex_pdf: Option<(PathBuf, bool)>,
    /// Whether the clipboard the host is asked for is to be compared with
    /// the buffer rather than pasted into it.
    compare_clipboard: bool,
    /// Whether a color the file writes (`#51afef`) is drawn on that color.
    swatches: bool,
    /// When the file being edited is next looked at on disk.
    disk_check_at: Instant,
    /// The completion popup, while it is open.
    completion: Option<completing::Completion>,
    /// The popup waiting for typing to pause before it opens by itself.
    completion_due: Option<completing::CompletionDue>,
    /// The problems store's generation the documents' problems were last
    /// taken at (see [`winter_lsp::diagnostics::generation`]).
    diagnostics_seen: u64,
    /// The earliest deadline of such a popup called off since the host last
    /// asked, for it to stop waiting on.
    cancelled_tick: Option<Instant>,
    /// The settings completion follows.
    completion_config: crate::config::CompletionConfig,
    /// What the file read as on disk when it was last found changed under
    /// edits not yet saved, so that is said once rather than every second.
    stale_said: Option<FileStamp>,
    /// The language servers this editor's documents are open with.
    link: lsp::Link,
    /// The fixes last offered at the cursor, from every source.
    fixes: fixes::Fixes,
    /// Whether the word under the cursor is lit wherever it occurs.
    occurrences: bool,
    /// Every occurrence of a word, while they are changed together.
    occur: Option<occur::Occur>,
    /// The revision a diff session asked git for, while it reads it.
    diff_revision: Option<String>,
    /// When the project's prose files were last checked for spelling.
    prose_scan: spelling_scan::ProseScan,
    /// Where a run of `C-l` presses has got to.
    recenter: Option<view::Recenter>,
    /// Until when the cursor's line is lit, after a jump.
    flash_until: Option<Instant>,
    /// How many cells of text each screen row holds while lines are folded
    /// to the pane, as the last paint folded them; `None` with wrapping off.
    fold: Option<usize>,
    /// The column within its screen row a run of vertical moves keeps to.
    wrap_goal: Option<usize>,
    /// How far the rows a wrapped line continues on are indented.
    wrap_indent: crate::model::paint::WrapIndent,
    /// How many screen rows the view keeps above and below the cursor's.
    scroll_margin: usize,
    /// Whether the key before was a yank (`C-y`, `M-y`), after which `M-y`
    /// steps back through what was yanked.
    yanked: bool,
    /// Copilot's suggestions: asked for, and on screen.
    pilot: copilot::Pilot,
    /// When markup's markers hide.
    markup_markers: crate::model::paint::MarkupMarkers,
    /// The markup spans found in the document in front, kept while its
    /// text stands.
    conceal_cache: std::cell::RefCell<conceal::Cache>,
    /// In-progress interactive query-replace session.
    query_replace: Option<query_replace::QueryReplaceState>,
}

/// One open file: its text, where the cursor and the window sit in it, and
/// what it looked like on disk when it was read.
#[derive(Debug)]
struct Document {
    buffer: TextBuffer,
    /// The conflicts in the text and the revision they were found at: the
    /// paint and the status line both ask on every frame, and reading every
    /// line for markers each time lagged a long file.
    conflicts: std::cell::RefCell<Option<(usize, Vec<conflict::Conflict>)>>,
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
    /// How many screen rows of the first line in view are scrolled past, so
    /// the view moves by screen rows over wrapped lines, whether the wheel,
    /// the cursor, or a placement moves it. Nought with wrapping off.
    scroll_skip: usize,
    /// What the tabs showing the file hold in common beside its text: where
    /// it stood on disk, its shape, and what the servers have of it.
    file: Rc<buffers::BufferState>,
    /// Whether this tab counts as one open with the servers.
    attached: bool,
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
    /// What the language server serving the file last said is wrong in it.
    diagnostics: Vec<winter_lsp::protocol::Diagnostic>,
    /// The words spelled wrong in it, while they are checked.
    spelling: Option<spelling::Spelling>,
}

/// One painted cell of a line: its character, its color, the wavy line
/// under it where a language server found a problem, and whether it is lit
/// as an occurrence of the word under the cursor.
type Cell = (char, Style, Option<DiagnosticLevel>, bool);

/// What one tab's view of the file keeps while another tab on the same file
/// is the one acted on: where the cursor and the window are, and what was
/// painted there last, which clicks and paging measure against.
#[derive(Clone, Debug)]
struct TextView {
    /// Which of the editor's files the view is on.
    at: usize,
    col: usize,
    fold: Option<usize>,
    hscroll: usize,
    jump_origin: Option<(usize, usize, (usize, usize))>,
    laid_out: RowWindow,
    painted: Vec<Painted>,
    recenter: Option<view::Recenter>,
    row: usize,
    scroll: usize,
    scroll_skip: usize,
    viewport: usize,
    wheel: bool,
    wrap_goal: Option<usize>,
}

/// What one screen row of a painted frame holds: which line of the file, and
/// the display column its text begins at, which is the fold a wrapped line
/// picks up from or the scroll a wide one is read through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Painted {
    /// The cells before the row's text: the gutter, and the indent of a
    /// row a wrapped line continues on.
    lead: usize,
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

    /// Open `target`'s file for editing at its line and column.
    pub fn at(target: OpenTarget) -> io::Result<Self> {
        let mut editor = Self::new(target.path, target.line)?;
        if let Some(column) = target.column {
            editor.move_to_column(column);
        }
        Ok(editor)
    }

    /// An editor holding `doc` alone, its keys starting in `mode`.
    fn with_doc(doc: Document, mode: EditMode) -> Self {
        Self {
            at: 0,
            docs: vec![doc],
            jump_origin: None,
            chord_start: None,
            modal: ModalEditor::new().with_mode(mode).with_clipboard(),
            laid_out: RowWindow::default(),
            painted: Vec::new(),
            recall: MessageRecall::default(),
            requests: Vec::new(),
            viewport: 0,
            wheel: false,
            waiter: None,
            inspect_scopes: false,
            synctex_pdf: None,
            compare_clipboard: false,
            swatches: true,
            disk_check_at: Instant::now() + DISK_CHECK,
            completion: None,
            completion_due: None,
            diagnostics_seen: u64::MAX,
            cancelled_tick: None,
            completion_config: crate::config::CompletionConfig::default(),
            stale_said: None,
            link: lsp::Link::default(),
            fixes: fixes::Fixes::default(),
            occurrences: true,
            occur: None,
            diff_revision: None,
            prose_scan: spelling_scan::ProseScan::default(),
            yanked: false,
            pilot: copilot::Pilot::default(),
            markup_markers: crate::model::paint::MarkupMarkers::default(),
            conceal_cache: std::cell::RefCell::default(),
            recenter: None,
            flash_until: None,
            fold: None,
            wrap_goal: None,
            wrap_indent: crate::model::paint::WrapIndent::default(),
            scroll_margin: 0,
            query_replace: None,
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
        self.write_file();
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

    /// Go to `line` of `path` where it is this editor's file; another file
    /// is the window's to show, in that file's own buffer.
    pub fn open(&mut self, path: PathBuf, line: Option<usize>) {
        let real = path.canonicalize().ok();
        let ours = self.docs.iter().any(|doc| {
            doc.path == path
                || real.as_deref() == Some(&doc.path)
                || doc.path.canonicalize().ok().as_deref() == real.as_deref()
        });
        if ours {
            if let Some(line) = line {
                self.buffer_mut().move_to_line(line.saturating_sub(1));
            }
            return;
        }
        self.requests.push(EditorRequest::Open(OpenTarget {
            column: None,
            line,
            path,
        }));
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
        let (viewport, shown) = (self.viewport, self.shown());
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
            shown,
            viewport,
        };
        f(&mut self.modal, view)
    }

    /// Write the text back, unless something else has written to the file
    /// since it was opened, which is worth a question rather than a save.
    fn save(&mut self) -> EditorOutcome {
        if remote::is_stale(&self.doc().path, &self.doc().stamp()) {
            let question = format!("{} changed on disk. Overwrite?", name_of(&self.doc().path));
            return EditorOutcome::Prompt(PromptRequest::confirm(ASK_OVERWRITE, &question));
        }
        self.write_file()
    }

    /// Write every buffer with edits to its file, as a build asks before it
    /// starts. A buffer whose file changed on disk since it was read, or
    /// that is not on this machine, is left for its own save to ask about.
    fn save_all(&mut self) -> EditorOutcome {
        let showing = self.at;
        let mut written = 0;
        let mut left = 0;
        for at in 0..self.docs.len() {
            let doc = &self.docs[at];
            if !doc.buffer.is_dirty() || doc.unread {
                continue;
            }
            if !doc.writes_unasked() {
                left += 1;
                continue;
            }
            self.at = at;
            // A write that needs a question (root's, a password) is one the
            // build cannot wait on.
            match self.write_file() {
                EditorOutcome::Prompt(_) => left += 1,
                _ => written += 1,
            }
        }
        self.at = showing;
        let notice = match left {
            0 => Notice::info(format!("saved {written} before the build")),
            _ => Notice::error(format!(
                "saved {written} before the build; {left} left unsaved"
            )),
        };
        self.modal.report(notice);
        EditorOutcome::Consumed
    }

    /// Write every buffer with edits that can be written without a
    /// question: read in, on this machine, not changed on disk since it was
    /// read, and writable without root. The rest wait for their own save,
    /// which asks. The mode stays as it is and nothing is said, since the
    /// keyboard has gone elsewhere; only a write that failed is reported.
    /// Returns the files written.
    fn save_quietly(&mut self) -> Vec<PathBuf> {
        let showing = self.at;
        let mut written = Vec::new();
        for at in 0..self.docs.len() {
            let doc = &self.docs[at];
            if !doc.buffer.is_dirty() || doc.unread || !doc.writes_unasked() {
                continue;
            }
            match text_file::save(&doc.path, doc.buffer.lines(), doc.shape()) {
                Ok(stamp) => {
                    self.at = at;
                    self.record_written(stamp);
                    written.push(self.doc().path.clone());
                }
                Err(e) if remote::is_denied(&e) => {}
                Err(e) => {
                    let said = format!("could not save {}: {e}", name_of(&doc.path));
                    self.modal.report(Notice::error(said));
                }
            }
        }
        self.at = showing;
        written
    }

    /// Put the cursor on `column` of its line, counting from one.
    fn move_to_column(&mut self, column: usize) {
        let row = self.buffer().row();
        self.buffer_mut().move_to(row, column.saturating_sub(1));
    }

    /// Open `target` as another buffer, or go to it, landing on its line
    /// and column.
    pub fn open_target(&mut self, target: OpenTarget) {
        let column = target.column;
        self.open(target.path, target.line);
        if let Some(column) = column {
            self.move_to_column(column);
        }
    }

    /// Take the file as written, now reading as `stamp`, and go back to
    /// Normal mode: a save ends a stretch of typing.
    fn mark_written(&mut self, stamp: FileStamp) {
        self.record_written(stamp);
        self.leave_to_normal();
        let wrote = lines_written(self.buffer());
        self.modal.report(Notice::info(format!("wrote {wrote}")));
    }

    /// Take the buffer in front as written to its file, now reading as
    /// `stamp`: nothing left to save, and the servers told.
    fn record_written(&mut self, stamp: FileStamp) {
        self.doc().set_stamp(stamp);
        self.buffer_mut().mark_saved();
        self.tell_saved();
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
            self.write_file();
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
}

// ========================================================================
// TextEditor: painting
// ========================================================================

impl TextEditor {
    /// Show where the cursor's line was set in the PDF built from this
    /// document, as magic-vscode's F6 does: `synctex view` names the box,
    /// and the viewer shows it, in a tab of this pane or, `beside`, of the
    /// other. In a `.bib` file the line is a `\cite` of the entry under the
    /// cursor, and outside any entry the PDF is only opened. A PDF with no
    /// SyncTeX data, or a line SyncTeX does not find, opens the PDF as it is.
    fn synctex_forward(&mut self, beside: bool) -> EditorOutcome {
        let path = self.doc().path.clone();
        let local = crate::remote::parse(&path).is_none();
        if !local || !(synctex::is_tex(&path) || synctex::is_bib(&path)) {
            self.modal
                .report(Notice::error("SyncTeX: not a LaTeX or BibTeX file on this machine"));
            return EditorOutcome::Consumed;
        }
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let mut source = (path.clone(), row + 1, col + 1);
        let mut sync = true;
        if synctex::is_bib(&path) {
            match synctex::bib_key_at(self.buffer().lines(), row) {
                Some(key) => match synctex::cite_of(&path, &key) {
                    Some(cited) => source = cited,
                    None => {
                        self.modal.report(Notice::error(format!("SyncTeX: \"{key}\" is not cited in any .tex file")));
                        return EditorOutcome::Consumed;
                    }
                },
                None => sync = false,
            }
        }
        let Some(pdf) = synctex::pdf_for(&source.0) else {
            self.modal.report(Notice::error("SyncTeX: no PDF built from this document yet (C-S-b builds it)"));
            return EditorOutcome::Consumed;
        };
        // A PDF built without `-synctex=1` (by another tool, or a command
        // typed over the build) has nothing to search: it opens as it is.
        if sync && !synctex::has_data(&pdf) {
            let name = pdf.file_name().map_or_else(String::new, |name| name.to_string_lossy().to_string());
            self.modal.report(
                Notice::warning(format!("SyncTeX: {name} was built without SyncTeX data; C-S-b rebuilds it with it"))
                    .unlogged(),
            );
            sync = false;
        }
        if !sync {
            return EditorOutcome::ShowInPdf { beside, mark: None, path: pdf };
        }
        let (file, line, column) = source;
        let request = synctex::view_request(&file, line, column, &pdf);
        self.synctex_pdf = Some((pdf, beside));
        EditorOutcome::Job(JobRequest::Command(request))
    }

    /// `synctex view` answered: show the first box it named, or the PDF as
    /// it is where it named none.
    fn synctex_shown(&mut self, output: CommandOutput) -> EditorOutcome {
        let Some((path, beside)) = self.synctex_pdf.take() else {
            return EditorOutcome::Consumed;
        };
        if !output.succeeded() {
            self.modal
                .report(Notice::error(format!("SyncTeX: {}", output.failure())));
            return EditorOutcome::Consumed;
        }
        let mark = synctex::parse_view(&output.stdout).into_iter().next();
        if mark.is_none() {
            self.modal.report(
                Notice::info("SyncTeX: this line was not found in the PDF (rebuild it?); showing the PDF").unlogged(),
            );
        }
        EditorOutcome::ShowInPdf { beside, mark, path }
    }

}

// ========================================================================
// TextEditor: the pane it is read through
// ========================================================================

impl TextEditor {
    /// Keep what was painted where, so a click can be answered with the
    /// character that was under the pointer.
    fn remember_layout(&mut self, lines: &[usize]) {
        let mut painted = Vec::new();
        for line in lines {
            for (at, start) in self.segments(*line).into_iter().enumerate() {
                painted.push(Painted {
                    lead: NUMBER_WIDTH + NUMBER_GAP.len() + self.row_hang(*line, at),
                    line: *line,
                    offset: match self.fold {
                        Some(_) => self.display_of(*line, start),
                        None => self.doc().hscroll,
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
        let display = spot.offset + at.col.saturating_sub(spot.lead);
        let col = self.char_at_display(spot.line, display);
        self.close_card();
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

    /// The line and the character painted at pane cell `at`; `None` in the
    /// gutter, past a line's end, or under the last line.
    fn char_at_cell(&self, at: Point) -> Option<(usize, usize)> {
        let spot = self.painted.get(at.row).copied()?;
        let display = spot.offset + at.col.checked_sub(spot.lead)?;
        let col = self.char_at_display(spot.line, display);
        (col < self.buffer().line_len_of(spot.line)).then_some((spot.line, col))
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
        if self.buffer().is_dirty() || !remote::is_stale(&self.doc().path, &self.doc().stamp()) {
            return EditorOutcome::Consumed;
        }
        let Ok(loaded) = remote::load(&self.doc().path) else {
            return EditorOutcome::Consumed;
        };
        self.replace_text(loaded);
        self.modal.report(Notice::info("changed on disk, re-read"));
        EditorOutcome::Consumed
    }

    /// Whether the file being edited is looked at on disk for changes made
    /// elsewhere: a file on this machine that has been read. A remote one
    /// would cost a round trip a second, and is read again on `C-r`.
    fn watches_disk(&self) -> bool {
        let doc = self.doc();
        !doc.unread && crate::remote::parse(&doc.path).is_none()
    }

    /// Read the file in again if something else has changed it, keeping the
    /// cursor; with edits here not yet saved, keep them and say once that
    /// the file changed, since reading it would throw them away.
    fn check_disk(&mut self) -> EditorOutcome {
        let path = self.doc().path.clone();
        if !text_file::is_stale(&path, &self.doc().stamp()) {
            return EditorOutcome::Consumed;
        }
        if !self.buffer().is_dirty() {
            self.stale_said = None;
            return self.reread();
        }
        let Ok(now) = text_file::stamp_of(&path) else {
            return EditorOutcome::Consumed;
        };
        if self.stale_said.as_ref() != Some(&now) {
            self.stale_said = Some(now);
            self.modal.report(Notice::warning(
                "changed on disk, but edited here too: saving asks before writing over it",
            ));
        }
        EditorOutcome::Consumed
    }

    /// Put `loaded` in place of the buffer's text, keeping the cursor where
    /// it was as far as the new text reaches.
    fn replace_text(&mut self, loaded: LoadedFile) {
        let (row, col) = (self.buffer().row(), self.buffer().col());
        // In place, for every tab showing the file: read again, it reads
        // again for all of them.
        self.buffer_mut().replace_all(loaded.lines);
        self.buffer_mut().move_to(row, col);
        self.doc().set_shape(loaded.shape);
        self.doc().set_stamp(loaded.stamp);
        let doc = self.doc();
        buffers::keep_buffer(&doc.path, &doc.file);
        self.doc_mut().forget_colors();
        self.resend_text();
        let at = self.at.min(self.docs.len() - 1);
        self.modal.reset(&self.docs[at].buffer);
    }
}

// ========================================================================
// TextEditor: what only the window asks of it
// ========================================================================

impl TextEditor {
    /// Whether this editor is editing a commit message.
    /// Take the language servers running, which `generation` names: an
    /// editor of documents hands them to the servers that serve them, and
    /// asks those what it needs. Called with the same generation again
    /// when nothing has changed, which costs nothing.
    pub(crate) fn connect_lsp(&mut self, generation: u64, ports: &[winter_lsp::Port]) {
        self.connect(generation, ports);
    }

    /// Hear what a language server said: an answer to something this asked,
    /// new problems in a document, or edits to make. `Ignored` where it was
    /// nothing of this editor's.
    pub(crate) fn on_lsp(&mut self, incoming: &winter_lsp::Incoming) -> EditorOutcome {
        if let Some(outcome) = self.copilot_answer(incoming) {
            return outcome;
        }
        self.on_lsp_message(incoming)
    }

    /// The language and the file a language server would be started for,
    /// from the document in front.
    pub(crate) fn lsp_subject(&self) -> Option<(String, PathBuf)> {
        let doc = self.doc();
        let local = crate::remote::parse(&doc.path).is_none();
        (local && !doc.unread).then(|| (self.language(), doc.path.clone()))
    }

    /// Take the settings that bear on the editor (how wrapped lines indent,
    /// how completion behaves), before each paint.
    pub fn configure(&mut self, config: &crate::config::Config) {
        self.set_wrap_indent(config.editor_wrap_indent);
        self.set_markup_markers(config.editor_markup_markers);
        self.set_completion_config(&config.completion);
        self.set_copilot_config(&config.copilot);
    }

    /// Make `edits` to the file this editor holds, as another editor's
    /// server asked. Returns whether this editor took them.
    pub(crate) fn apply_text_edits(&mut self, edits: &[winter_lsp::protocol::TextEdit]) -> bool {
        let at = self.at.min(self.docs.len() - 1);
        self.apply_to_buffer(at, edits);
        self.sync_documents();
        true
    }

    /// Take the settings completion follows.
    pub(crate) fn set_completion_config(&mut self, config: &crate::config::CompletionConfig) {
        self.take_completion_config(config);
    }

    /// Take the settings Copilot's suggestions follow.
    pub(crate) fn set_copilot_config(&mut self, config: &crate::config::CopilotConfig) {
        self.take_copilot_config(config);
    }

    /// Take how far the rows a wrapped line continues on are indented.
    pub(crate) fn set_wrap_indent(&mut self, indent: crate::model::paint::WrapIndent) {
        self.wrap_indent = indent;
    }

    /// Take when markup's markers hide.
    pub(crate) fn set_markup_markers(&mut self, markers: crate::model::paint::MarkupMarkers) {
        self.take_markup_markers(markers);
    }

    /// The text of `path` as this editor holds it, the lines left out where
    /// they are still at revision `known`.
    #[cfg(feature = "preview")]
    pub(crate) fn shared_text(&self, path: &Path, known: Option<usize>) -> Option<editor::SharedText> {
        let doc = self.docs.iter().find(|doc| doc.path == path && !doc.unread)?;
        // Text read in afresh counts its revisions from nought again; the
        // line count keeps that from reading as unchanged.
        let revision = doc.buffer.revision().wrapping_mul(1 << 20) ^ doc.buffer.lines().len();
        Some(editor::SharedText {
            cursor: doc.buffer.row(),
            lines: (known != Some(revision)).then(|| doc.buffer.lines().to_vec()),
            revision,
        })
    }
}

impl Editor for TextEditor {
    fn is_waited_on(&self) -> bool {
        self.waiter.is_some()
    }

    /// A commit message is written for one commit: buried, it would come
    /// back as the message of a commit already made.
    fn is_kept_when_closed(&self) -> bool {
        !self.is_waited_on() && !self.edits_commit()
    }

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

    fn focus_lost(&mut self) -> Vec<PathBuf> {
        self.save_quietly()
    }

    fn document(&self) -> Option<(PathBuf, String)> {
        Some((self.doc().path.clone(), self.buffer().lines().join("\n")))
    }

    fn document_path(&self) -> Option<PathBuf> {
        Some(self.doc().path.clone())
    }

    fn text_revision(&self) -> Option<usize> {
        Some(self.buffer().revision())
    }

    /// A program blocked on this edit, such as `git commit` waiting on its
    /// message, hears that it is over, and the servers that the file is closed.
    fn on_closed(&mut self) {
        self.waiter = None;
        self.tell_closed();
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        self.catch_up_buffers();
        // Any back-end may have published since: a server, the spelling, an
        // extension. The problems are taken again only when one has.
        let seen = winter_lsp::diagnostics::generation();
        if self.diagnostics_seen != seen {
            self.diagnostics_seen = seen;
            self.refresh_diagnostics(None);
        }
        let text_cols = cols.saturating_sub(NUMBER_WIDTH + NUMBER_GAP.len());
        self.viewport = rows;
        self.follow_cursor(text_cols, wrap);
        let total = self.buffer().lines().len();
        // Folded, the editor cuts each line into its screen rows itself, so
        // the caret and a click fall where the text is painted.
        self.fold = wrap.then_some(text_cols.max(1));
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let (within, _, _) = self.segment_at(row, col);
        let height = |index: usize| self.segments(index).len();
        // The view's top is a screen row: a line and how many of its rows
        // are scrolled past. The wheel holds the view where it put it (its
        // turns already taken, in `on_scroll`); anything else brings the
        // cursor's screen row into it, a screen row at a time, the margin
        // round it.
        let aim = match self.wheel {
            true => ViewAim::Held,
            false => ViewAim::Follow,
        };
        let cursor = CursorRows {
            first: (row, within),
            last: (row, within),
        };
        let top = (self.doc().scroll, self.doc().scroll_skip);
        let window = place_view(top, 0, aim, cursor, total, self.viewport, self.scroll_margin, height);
        let (scroll, skip) = (window.start, window.skip);
        let end = scroll + window.count;
        self.laid_out = window;
        self.doc_mut().scroll = scroll;
        self.doc_mut().scroll_skip = skip;
        self.doc_mut().color_upto = end;
        self.doc_mut().learn_states(LEARN_PAINT_BUDGET);
        self.doc_mut().color_lines(scroll, end);
        self.check_spelling();
        let lines: Vec<usize> = (scroll..end).collect();
        let conflicts = self.doc().conflicts();
        let frame_rows: Vec<Row> = lines
            .iter()
            .flat_map(|index| {
                let marker = conflicts.iter().any(|found| found.is_marker(*index));
                self.text_rows(*index, text_cols, marker)
            })
            .skip(skip)
            .take(self.viewport)
            .collect();
        self.remember_layout(&lines);
        self.painted.drain(..skip.min(self.painted.len()));
        self.painted.truncate(self.viewport);
        // The cursor's screen row: the rows of the lines above it in the
        // window, and its own row within its line.
        let above: usize = lines.iter().take_while(|index| **index < row).map(|index| self.segments(*index).len()).sum();
        // Every row fits the pane now, and a gutter-wide indent keeps any
        // that would not under the text.
        let gutter = NUMBER_WIDTH + NUMBER_GAP.len();
        let indents = vec![gutter; frame_rows.len()];
        let content = Frame::new(frame_rows).with_wrap_indents(indents);
        // A cursor in the rows scrolled past, or below them, is not on screen.
        match (above + within).checked_sub(skip) {
            Some(line) if lines.contains(&row) && line < self.viewport => content.with_cursor_line(line),
            Some(_) | None => content,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        // The jobs an editor asks for: a SyncTeX forward search, and the
        // commit messages a recall steps back through.
        let JobReply::Command(output) = reply else {
            return EditorOutcome::Consumed;
        };
        if output.tag == synctex::TAG_VIEW {
            return self.synctex_shown(output);
        }
        if output.tag == diffing::TAG_DIFF_REVISION {
            return self.revision_read(&output);
        }
        if output.tag == writing::TAG_TEXCOUNT {
            return self.latex_words_counted(&output);
        }
        if output.tag != git_exec::TAG_MESSAGES {
            return EditorOutcome::Consumed;
        }
        let log = git_exec::succeeded(&output).then_some(output.stdout.as_str());
        match self.recall.take_history(log) {
            true => self.recall_message(true),
            false => EditorOutcome::Consumed,
        }
    }

    /// Coloring that a paint did not reach in time, in a long file, catches
    /// up a tick at a time.
    fn next_tick(&self) -> Option<Instant> {
        let learning = self.doc().learning().then(|| Instant::now() + LEARN_TICK);
        let disk = self.watches_disk().then_some(self.disk_check_at);
        let flash = self.flash_until.filter(|until| *until > Instant::now());
        let pilot = self.copilot_next_tick();
        let completion = self.completion_next_tick();
        let spelling = spelling::spelling_due(self.doc());
        let landed = self.spelling_landed_tick();
        let scan = self.prose_scan_next_tick();
        [learning, disk, self.lsp_next_tick(), flash, pilot, completion, spelling, landed, scan]
            .into_iter()
            .flatten()
            .min()
    }

    fn take_cancelled_tick(&mut self) -> Option<Instant> {
        self.take_cancelled_completion()
    }

    /// Edits a server has still to hear of, such as a rename's into a buried
    /// buffer, are handed over buried or not, so the server's view of the
    /// project stays whole.
    fn has_work_in_flight(&self) -> bool {
        self.has_unsynced_edits()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        self.catch_up_buffers();
        self.doc_mut().learn_states(LEARN_TICK_BUDGET);
        self.lsp_tick(now);
        self.copilot_tick(now);
        self.completion_tick(now);
        if spelling::spelling_due(self.doc()).is_some_and(|due| now >= due) {
            self.check_spelling();
        }
        self.take_landed_spelling();
        self.take_prose_scan();
        if self.watches_disk() && now >= self.disk_check_at {
            self.disk_check_at = now + DISK_CHECK;
            return self.check_disk();
        }
        EditorOutcome::Consumed
    }

    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        self.catch_up_buffers();
        match name {
            SHOW_SCOPES => Some(self.show_scopes()),
            editor::HIDE_SCOPES => {
                self.inspect_scopes = false;
                Some(EditorOutcome::Consumed)
            }
            SYNCTEX_FORWARD => Some(self.synctex_forward(false)),
            editor::SYNCTEX_FORWARD_BESIDE => Some(self.synctex_forward(true)),
            SAVE_ALL => Some(self.save_all()),
            MERGE_NEXT => Some(self.step_conflict(true)),
            MERGE_PREV => Some(self.step_conflict(false)),
            MERGE_RESOLVE => Some(self.conflict_menu()),
            COPY_RICH => Some(self.copy_rich()),
            editor::QUERY_REPLACE => Some(self.start_query_replace()),
            TOGGLE_SWATCHES => {
                self.swatches = !self.swatches;
                let state = if self.swatches { "on" } else { "off" };
                self.modal
                    .report(Notice::info(format!("color swatches {state}")).unlogged());
                Some(EditorOutcome::Consumed)
            }
            DIFF_DISK => Some(self.compare_with_disk()),
            DIFF_CLIPBOARD => Some(self.ask_to_compare_with_clipboard()),
            DIFF_BUFFER => Some(self.ask_buffer_to_compare()),
            name if LSP_COMMANDS.contains(&name) => Some(self.run_lsp_command(name)),
            name if PROBLEM_COMMANDS.contains(&name) => Some(self.run_problem_command(name)),
            editor::DIFF_FILE | editor::DIFF_REVISION | editor::DIFF_FILES | editor::MERGE_FILE
            | editor::MERGE_CONFLICTS_3WAY => self.diff_command(name),
            editor::WRITING_INSERT_SYMBOL => Some(self.ask_symbol()),
            editor::EDITOR_RECENTER => Some(self.recenter()),
            editor::EDITOR_FLASH_LINE => Some(self.flash_line()),
            editor::EDITOR_GOTO_OFFSET => Some(self.ask_offset()),
            editor::EDITOR_SHOW_OFFSET => Some(self.show_offset()),
            editor::RUST_CARGO_FIX => Some(match self.language().as_str() {
                "rust" => EditorOutcome::RunActionHere("cargo_fix".to_string()),
                _ => {
                    self.modal.report(Notice::info("cargo fix: not a Rust file"));
                    EditorOutcome::Consumed
                }
            }),
            editor::DICTIONARY_LOOKUP => Some(self.look_up_word()),
            editor::BIBTEX_SORT_KEY_ASC => Some(self.sort_bibtex(bibtex::SortBy::Key, true)),
            editor::BIBTEX_SORT_KEY_DESC => Some(self.sort_bibtex(bibtex::SortBy::Key, false)),
            editor::BIBTEX_SORT_YEAR_ASC => Some(self.sort_bibtex(bibtex::SortBy::Year, true)),
            editor::BIBTEX_SORT_YEAR_DESC => Some(self.sort_bibtex(bibtex::SortBy::Year, false)),
            editor::EDITOR_CHANGE_OCCURRENCES => Some(self.start_occur()),
            editor::EDITOR_TOGGLE_OCCURRENCES => Some(self.toggle_occurrences()),
            editor::EDITOR_NEXT_OCCURRENCE => Some(self.step_occurrence(true)),
            editor::EDITOR_PREV_OCCURRENCE => Some(self.step_occurrence(false)),
            editor::EDITOR_DEFINITION_START => Some(self.goto_definition_edge(false)),
            editor::EDITOR_DEFINITION_END => Some(self.goto_definition_edge(true)),
            editor::EDITOR_SELECT_DEFINITION => Some(self.select_definition()),
            editor::EDITOR_NEXT_BLOCK => Some(self.step_block(true)),
            editor::EDITOR_PREV_BLOCK => Some(self.step_block(false)),
            name => self.run_writing_command(name),
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
        let conflicts = self.doc().conflicts();
        let conflicted = (!conflicts.is_empty()).then(|| match conflicts.len() {
            1 => "1 conflict".to_string(),
            n => format!("{n} conflicts"),
        });
        let problems = self.problem_counts();
        status.segments.splice(
            0..0,
            host.into_iter()
                .chain(count)
                .chain(conflicted)
                .chain(problems)
                .chain(self.occur_status())
                .chain(self.copilot_status()),
        );
        status
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.catch_up_buffers();
        self.wheel = false;
        self.close_card();
        // Copilot's keys go first while it shows something, the completion
        // popup being in front of it when open.
        let revision = self.buffer().revision();
        if self.completion.is_none() && !self.doc().unread {
            if let Some(outcome) = self.copilot_key(key) {
                self.copilot_after_key(revision);
                return outcome;
            }
        }
        // The space bar is typed text here, in insert mode and after `f`, `r`
        // and the like, all of which match characters.
        let outcome = self.dispatch(&key.typed());
        self.copilot_after_key(revision);
        outcome
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
        self.catch_up_buffers();
        self.wheel = false;
        self.point_at(at)
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.scroll_margin = rows;
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.catch_up_buffers();
        // The view moves and the cursor stays where it was, so the text can be
        // read without losing the place being edited; the next key brings the
        // view back to it.
        let total = self.buffer().lines().len();
        let top = (self.doc().scroll, self.doc().scroll_skip);
        let (scroll, skip) = wheel_scroll_rows(top, lines, total, self.viewport, |index| self.segments(index).len());
        self.doc_mut().scroll = scroll;
        self.doc_mut().scroll_skip = skip;
        self.wheel = true;
        EditorOutcome::Consumed
    }

    fn on_paste(&mut self, text: String) -> EditorOutcome {
        self.catch_up_buffers();
        self.wheel = false;
        if self.doc().unread {
            return self.read_in();
        }
        if std::mem::take(&mut self.compare_clipboard) {
            return self.compare_with_clipboard(&text);
        }
        self.paste_text(text)
    }

    fn take_requests(&mut self) -> Vec<EditorRequest> {
        std::mem::take(&mut self.requests)
    }

    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        let doc = self.doc();
        Some(Box::new(TextView {
            at: self.at,
            col: doc.buffer.col(),
            fold: self.fold,
            hscroll: doc.hscroll,
            jump_origin: self.jump_origin,
            laid_out: self.laid_out,
            painted: self.painted.clone(),
            recenter: self.recenter,
            row: doc.buffer.row(),
            scroll: doc.scroll,
            scroll_skip: doc.scroll_skip,
            viewport: self.viewport,
            wheel: self.wheel,
            wrap_goal: self.wrap_goal,
        }))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        let Ok(view) = state.downcast::<TextView>() else {
            return;
        };
        self.fold = view.fold;
        self.jump_origin = view.jump_origin;
        self.laid_out = view.laid_out;
        self.painted = view.painted;
        self.recenter = view.recenter;
        self.viewport = view.viewport;
        self.wheel = view.wheel;
        self.wrap_goal = view.wrap_goal;
        self.at = view.at.min(self.docs.len().saturating_sub(1));
        let doc = self.doc_mut();
        doc.hscroll = view.hscroll;
        doc.scroll = view.scroll;
        doc.scroll_skip = view.scroll_skip;
        // The text may have changed under this view since it was kept.
        doc.buffer.move_to(view.row, view.col);
        doc.buffer.clamp(false);
    }

    fn on_resume(&mut self) -> EditorOutcome {
        self.catch_up_buffers();
        self.reread()
    }

    fn open_file(&mut self, target: OpenTarget) -> bool {
        // A file opened while an editor is up joins it as another buffer,
        // rather than covering it with an editor of its own.
        self.open_target(target);
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
            menu: false,
            title: "editor".to_string(),
        })
    }

    fn keys(&self) -> Vec<KeyHelp> {
        self.key_help()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.catch_up_buffers();
        self.wheel = false;
        if let Some(outcome) = self.on_lsp_prompt(&reply) {
            return outcome;
        }
        if let Some(outcome) = self.on_diff_prompt(reply.tag, reply.answer.clone()) {
            return outcome;
        }
        if reply.tag == view::ASK_OFFSET {
            return match reply.answer {
                Some(answer) => self.goto_offset(&answer),
                None => EditorOutcome::Consumed,
            };
        }
        if reply.tag == symbols::ASK_SYMBOL {
            return match reply.answer {
                Some(chosen) => self.symbol_chosen(&chosen),
                None => EditorOutcome::Consumed,
            };
        }
        let said_yes = reply.answer.is_some();
        match reply.tag {
            fixes::ASK_FIX => match reply.answer {
                Some(chosen) => self.fix_chosen(&chosen),
                None => EditorOutcome::Consumed,
            },
            ASK_COMPARE_BUFFER => match reply.answer {
                Some(chosen) => self.compare_with_buffer(&chosen),
                None => EditorOutcome::Consumed,
            },
            ASK_ENV_NEW => match reply.answer {
                Some(name) => self.new_environment(&name),
                None => EditorOutcome::Consumed,
            },
            ASK_ENV_RENAME => match reply.answer {
                Some(name) => self.rename_environment(&name),
                None => EditorOutcome::Consumed,
            },
            ASK_CONFLICT => match reply.answer {
                Some(choice) => self.conflict_chosen(&choice),
                None => EditorOutcome::Consumed,
            },
            ASK_OVERWRITE if said_yes => self.write_file(),
            ASK_AS_ROOT | ASK_PASSWORD_OPEN | ASK_PASSWORD_SAVE => self.answer_elsewhere(reply),
            ASK_SEARCH | ASK_SEARCH_BACK => match reply.answer {
                Some(pattern) => {
                    self.jump_origin = None;
                    self.start_search(pattern, reply.tag == ASK_SEARCH)
                }
                None => {
                    if let Some((row, col, top)) = self.jump_origin.take() {
                        self.buffer_mut().move_to(row, col);
                        (self.doc_mut().scroll, self.doc_mut().scroll_skip) = top;
                    }
                    EditorOutcome::Consumed
                }
            },
            query_replace::ASK_QUERY_REPLACE_SEARCH => self.on_query_replace_search(reply.answer),
            query_replace::ASK_QUERY_REPLACE_TARGET => self.on_query_replace_target(reply.answer),
            query_replace::ASK_QUERY_REPLACE_ACTION => self.on_query_replace_action(reply.answer),
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if (tag == ASK_SEARCH || tag == ASK_SEARCH_BACK) && !query.is_empty() {
            if self.jump_origin.is_none() {
                self.jump_origin = Some((self.buffer().row(), self.buffer().col(), self.view_top()));
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
        let real = path.canonicalize().ok();
        self.docs.iter().any(|doc| {
            doc.path == path
                || real.as_deref() == Some(&doc.path)
                || doc.path.canonicalize().ok().as_deref() == real.as_deref()
        })
    }

    fn returns_to_git(&self) -> Option<PathBuf> {
        if !self.edits_commit() {
            return None;
        }
        // The message is written in the repository's git directory; the view
        // is of the work tree around it.
        let dir = self.cwd()?;
        Some(match dir.file_name().is_some_and(|name| name == ".git") {
            true => dir.parent().map_or_else(|| dir.clone(), PathBuf::from),
            false => dir,
        })
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

    fn popups(&self) -> Popups {
        Popups {
            card: self.card().or_else(|| self.copilot_card()),
            completion: self.popup(),
            drop_list: None,
        }
    }

    fn on_hover(&mut self, at: Option<Point>) -> EditorOutcome {
        self.hover_at(at)
    }

    fn outline_targets(&mut self) -> Option<Vec<JumpTarget>> {
        Some(self.outline())
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

    fn on_jump(&mut self, step: JumpStep) {
        let target = match step {
            JumpStep::Cancel => {
                if let Some((row, col, top)) = self.jump_origin.take() {
                    self.buffer_mut().move_to(row, col);
                    (self.doc_mut().scroll, self.doc_mut().scroll_skip) = top;
                }
                return;
            }
            JumpStep::Confirm(target) => {
                if let Some((row, col, _)) = self.jump_origin.take() {
                    self.doc_mut().marks.jump = Some((row, col));
                }
                target
            }
            JumpStep::Preview(target) => {
                if self.jump_origin.is_none() {
                    self.jump_origin = Some((self.buffer().row(), self.buffer().col(), self.view_top()));
                }
                target
            }
        };
        self.buffer_mut().move_to_line(target);
        self.place_cursor_row(Place::Middle);
    }

    fn token_inspect(&mut self) -> Option<crate::model::editor::TokenInspectData> {
        if !self.inspect_scopes {
            return None;
        }
        let (row, col) = (self.buffer().row(), self.buffer().col());
        let line = self.line_of(row);
        let doc = self.doc_mut();
        doc.color_upto = doc.color_upto.max(row);
        doc.learn_states(Duration::MAX);
        let grammar_name = doc.grammar.map_or("Plain Text", |g| g.name());
        let state = doc.states.get(row)?;
        Some(highlight::inspect_token(&line, state, col, row, grammar_name))
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

/// The painted spans for a row's cells: one span per run of cells sharing a
/// color, cut to what the pane has room for. One span per character would be
/// correct and would cost the renderer a shaping run for every letter.
///
/// A selected run keeps its syntax color and takes the selection's
/// background, the way a terminal selection does.
fn runs(cells: &[Cell], cols: usize, marked: Option<(usize, usize)>) -> Row {
    let mut spans: Vec<Span> = Vec::new();
    for (at, (c, style, squiggle, lit)) in cells.iter().take(cols).enumerate() {
        let selected = marked.is_some_and(|(start, end)| at >= start && at < end);
        match spans.last_mut() {
            Some(last)
                if last.style == *style
                    && last.selected == selected
                    && last.squiggle == *squiggle
                    && last.lit == *lit =>
            {
                last.text.push(*c)
            }
            _ => {
                let span = Span::new(*style, c.to_string()).squiggled(*squiggle).lit(*lit);
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

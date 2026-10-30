//! Editors: the tabs Winter paints itself instead of running a terminal in
//! them. An editor takes its own keys and hands the host a [`Frame`] of styled
//! rows each paint; the rows themselves are [`super::paint`]'s.

use std::any::Any;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use super::notify::Notice;

use super::guard::Guarded;

use super::input::table::KeyHelp;
use super::input::{Key, MacroStep};
#[cfg(doc)]
use super::paint::Gutter;
use super::paint::{Caret, Frame, IconKind, Point, Row};
use super::picture::{Picture, Tile};
use super::process::ProcessSample;
use super::system::SystemSample;
use super::textedit::modal::EditMode;

// ========================================================================
// Constants
// ========================================================================

/// The editor command name that toggles token / scope inspection.
pub const SHOW_SCOPES: &str = "show_scopes";

/// The editor command name that puts token / scope inspection away, for
/// the window dismissing it: an Escape sent instead could also leave a mode
/// or drop a selection.
pub const HIDE_SCOPES: &str = "hide_scopes";

/// The editor command name that shows where the cursor's line of LaTeX was
/// set in the PDF built from it (SyncTeX forward).
pub const SYNCTEX_FORWARD: &str = "synctex_forward";

/// The same, the PDF shown in the other pane.
pub const SYNCTEX_FORWARD_BESIDE: &str = "synctex_forward_beside";

/// The PDF viewer command name that goes back to the LaTeX source of the spot
/// under the cursor or reading line (SyncTeX backward).
pub const SYNCTEX_BACKWARD: &str = "synctex_backward";

/// The editor command name that writes every file being edited to disk,
/// what a build asks for before it starts.
pub const SAVE_ALL: &str = "save_all";

/// The text editor's command that steps to the next merge conflict.
pub const MERGE_NEXT: &str = "merge_next_conflict";

/// The text editor's command that steps back to the previous merge conflict.
pub const MERGE_PREV: &str = "merge_prev_conflict";

/// The text editor's command that offers what of the merge conflict under
/// the cursor to keep.
pub const MERGE_RESOLVE: &str = "merge_resolve_conflict";

/// The text editor's command that compares the buffer with the file on disk
/// it was read from.
pub const DIFF_DISK: &str = "diff_buffer_disk";

/// The text editor's command that compares the buffer with the clipboard.
pub const DIFF_CLIPBOARD: &str = "diff_buffer_clipboard";

/// The text editor's command that compares the buffer with another one open.
pub const DIFF_BUFFER: &str = "diff_buffer_buffer";

/// The text editor's commands for writing Markdown and LaTeX, by the name
/// its `run_command` knows each by.
pub const WRITING_COMMANDS: &[&str] = &[
    "markdown_next_heading",
    "markdown_prev_heading",
    "markdown_promote",
    "markdown_demote",
    "markdown_promote_subtree",
    "markdown_demote_subtree",
    "markdown_bold",
    "markdown_italic",
    "markdown_code",
    "markdown_math",
    "markdown_task",
    "markdown_renumber",
    "markdown_word_count",
    "latex_new_environment",
    "latex_rename_environment",
    "latex_toggle_display",
    "latex_bold",
    "latex_italic",
    "latex_emph",
    "latex_typewriter",
    "latex_sans",
    "latex_small_caps",
    "latex_word_count",
];

/// The text editor's command that copies the selected lines, or the whole
/// file, as rich text in the theme's syntax colors.
pub const COPY_RICH: &str = "editor_copy_rich";

/// The text editor's command for interactive query replace (`M-%`).
pub const QUERY_REPLACE: &str = "query_replace";

/// The text editor's command that turns its color swatches off or on.
pub const TOGGLE_SWATCHES: &str = "editor_toggle_swatches";

// The text editor's language server commands, which the palette hands it
// by these names.

/// Go to where the name under the cursor is defined.
pub const LSP_DEFINITION: &str = "lsp_definition";

/// Go to where it is declared.
pub const LSP_DECLARATION: &str = "lsp_declaration";

/// Go to where its type is defined.
pub const LSP_TYPE_DEFINITION: &str = "lsp_type_definition";

/// Go to what implements it.
pub const LSP_IMPLEMENTATION: &str = "lsp_implementation";

/// List every place it is used.
pub const LSP_REFERENCES: &str = "lsp_references";

/// Say what it is, and the problems found there.
pub const LSP_HOVER: &str = "lsp_hover";

/// Show the signature of the call being typed.
pub const LSP_SIGNATURE: &str = "lsp_signature_help";

/// Rename it everywhere.
pub const LSP_RENAME: &str = "lsp_rename";

/// Offer the fixes and refactorings there.
pub const LSP_CODE_ACTION: &str = "lsp_code_action";

/// Format the file.
pub const LSP_FORMAT: &str = "lsp_format";

/// Back to where the last jump left from.
pub const NAVIGATE_BACK: &str = "navigate_back";

/// Forward again, after going back.
pub const NAVIGATE_FORWARD: &str = "navigate_forward";

/// Every language server command the text editor runs by name.
pub const LSP_COMMANDS: &[&str] = &[
    LSP_DEFINITION,
    LSP_DECLARATION,
    LSP_TYPE_DEFINITION,
    LSP_IMPLEMENTATION,
    LSP_REFERENCES,
    LSP_HOVER,
    LSP_SIGNATURE,
    LSP_RENAME,
    LSP_CODE_ACTION,
    LSP_FORMAT,
    NAVIGATE_BACK,
    NAVIGATE_FORWARD,
];

// The text editor's problem commands, over what every back-end found
// wrong (see `winter_lsp::diagnostics`), which the palette hands it by these
// names.

/// On to the next problem in the file.
pub const PROBLEM_NEXT_IN_FILE: &str = "problem_next_in_file";

/// Back to the problem before, in the file.
pub const PROBLEM_PREV_IN_FILE: &str = "problem_prev_in_file";

/// To the first problem in the project, an error first.
pub const PROBLEM_FIRST: &str = "problem_first";

/// On to the next problem in the project.
pub const PROBLEM_NEXT: &str = "problem_next";

/// Back to the problem before, in the project.
pub const PROBLEM_PREV: &str = "problem_prev";

/// To the first error in the project.
pub const PROBLEM_FIRST_ERROR: &str = "problem_first_error";

/// On to the next error in the project.
pub const PROBLEM_NEXT_ERROR: &str = "problem_next_error";

/// Back to the error before, in the project.
pub const PROBLEM_PREV_ERROR: &str = "problem_prev_error";

/// To the first writing issue in the project.
pub const PROBLEM_FIRST_WRITING: &str = "problem_first_writing";

/// On to the next writing issue in the project.
pub const PROBLEM_NEXT_WRITING: &str = "problem_next_writing";

/// Back to the writing issue before, in the project.
pub const PROBLEM_PREV_WRITING: &str = "problem_prev_writing";

/// List every problem in the project.
pub const PROBLEM_LIST: &str = "problem_list";

/// Every problem command the text editor runs by name.
pub const PROBLEM_COMMANDS: &[&str] = &[
    PROBLEM_NEXT_IN_FILE,
    PROBLEM_PREV_IN_FILE,
    PROBLEM_FIRST,
    PROBLEM_NEXT,
    PROBLEM_PREV,
    PROBLEM_FIRST_ERROR,
    PROBLEM_NEXT_ERROR,
    PROBLEM_PREV_ERROR,
    PROBLEM_FIRST_WRITING,
    PROBLEM_NEXT_WRITING,
    PROBLEM_PREV_WRITING,
    PROBLEM_LIST,
];

/// Change every occurrence of the word under the cursor at once.
pub const EDITOR_CHANGE_OCCURRENCES: &str = "editor_change_occurrences";

/// Light the occurrences of the word under the cursor, or stop.
pub const EDITOR_TOGGLE_OCCURRENCES: &str = "editor_toggle_occurrences";

/// On to the next occurrence of the word under the cursor.
pub const EDITOR_NEXT_OCCURRENCE: &str = "editor_next_occurrence";

/// Back to the occurrence before.
pub const EDITOR_PREV_OCCURRENCE: &str = "editor_prev_occurrence";

/// To the start of the function or module the cursor is in.
pub const EDITOR_DEFINITION_START: &str = "editor_definition_start";

/// To its end.
pub const EDITOR_DEFINITION_END: &str = "editor_definition_end";

/// Select the function or module the cursor is in.
pub const EDITOR_SELECT_DEFINITION: &str = "editor_select_definition";

/// On to the next block the indentation makes, as a fold.
pub const EDITOR_NEXT_BLOCK: &str = "editor_next_block";

/// Back to the block before.
pub const EDITOR_PREV_BLOCK: &str = "editor_prev_block";

/// Compare the buffer with another file, in the diff tool.
pub const DIFF_FILE: &str = "diff_buffer_file";

/// Compare the buffer with a revision git has of its file.
pub const DIFF_REVISION: &str = "diff_buffer_revision";

/// Compare the buffer with two other files at once.
pub const DIFF_FILES: &str = "diff_buffer_files";

/// Merge another file into the buffer, in the diff tool.
pub const MERGE_FILE: &str = "merge_file";

/// Resolve the buffer's merge conflicts in a 3-way merge.
pub const MERGE_CONFLICTS_3WAY: &str = "merge_conflicts_3way";

/// Insert a math or Greek symbol picked from a list.
pub const WRITING_INSERT_SYMBOL: &str = "editor_insert_symbol";

/// Look the word under the cursor up in the dictionary.
pub const DICTIONARY_LOOKUP: &str = "dictionary_lookup";

/// Sort a BibTeX file's entries by citation key, A to Z.
pub const BIBTEX_SORT_KEY_ASC: &str = "bibtex_sort_key_ascending";

/// Sort them by citation key, Z to A.
pub const BIBTEX_SORT_KEY_DESC: &str = "bibtex_sort_key_descending";

/// Sort them by year, the earliest first.
pub const BIBTEX_SORT_YEAR_ASC: &str = "bibtex_sort_year_ascending";

/// Sort them by year, the latest first.
pub const BIBTEX_SORT_YEAR_DESC: &str = "bibtex_sort_year_descending";

/// Put the cursor's line in the middle of the pane, then at its top, then its bottom.
pub const EDITOR_RECENTER: &str = "editor_recenter";

/// Light the cursor's line for a moment.
pub const EDITOR_FLASH_LINE: &str = "editor_flash_line";

/// Go to a character offset from the file's start.
pub const EDITOR_GOTO_OFFSET: &str = "editor_goto_offset";

/// Say the character offset the cursor is at.
pub const EDITOR_SHOW_OFFSET: &str = "editor_show_offset";

/// Run `cargo fix` on the Rust project the file is in.
pub const RUST_CARGO_FIX: &str = "rust_cargo_fix";

/// The commands the palette hands to the editor in front by the name its
/// `run_command` knows them by: what only the text being edited can do.
pub const FORWARDED_COMMANDS: &[&str] = &[
    EDITOR_RECENTER,
    QUERY_REPLACE,
    EDITOR_FLASH_LINE,
    EDITOR_GOTO_OFFSET,
    EDITOR_SHOW_OFFSET,
    WRITING_INSERT_SYMBOL,
    DICTIONARY_LOOKUP,
    BIBTEX_SORT_KEY_ASC,
    BIBTEX_SORT_KEY_DESC,
    BIBTEX_SORT_YEAR_ASC,
    BIBTEX_SORT_YEAR_DESC,
    DIFF_FILE,
    DIFF_REVISION,
    DIFF_FILES,
    MERGE_FILE,
    MERGE_CONFLICTS_3WAY,
    EDITOR_CHANGE_OCCURRENCES,
    EDITOR_TOGGLE_OCCURRENCES,
    EDITOR_NEXT_OCCURRENCE,
    EDITOR_PREV_OCCURRENCE,
    EDITOR_DEFINITION_START,
    EDITOR_DEFINITION_END,
    EDITOR_SELECT_DEFINITION,
    EDITOR_NEXT_BLOCK,
    EDITOR_PREV_BLOCK,

    COPY_RICH,
    TOGGLE_SWATCHES,
    MERGE_NEXT,
    MERGE_PREV,
    MERGE_RESOLVE,
    DIFF_DISK,
    DIFF_CLIPBOARD,
    DIFF_BUFFER,
];

// ========================================================================
// Data Structures
// ========================================================================

/// Slow work an editor is asking the host to do off the event-loop thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobRequest {
    /// Run a program and capture what it wrote.
    Command(CommandRequest),
    /// Total the bytes under this directory.
    DirSize(PathBuf),
    /// Take a snapshot of every process on the machine.
    Processes,
    /// Read these files, skipping the ones that are not there. For the small
    /// state files a program keeps beside its data, where "not there" is an
    /// answer rather than a failure.
    ReadFiles(Vec<PathBuf>),
    /// Find the lines under a directory that hold some text.
    Search(SearchRequest),
    /// Take a snapshot of the machine's CPU, memory, disks, and GPUs.
    System,
    /// Copy, move, delete, or trash files. A large tree takes long, and the
    /// window must not wait on it; the stop key can end it between entries.
    FileOps(FileOps),
}

/// The file work a [`JobRequest::FileOps`] does, its paths already resolved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum FileOps {
    /// Copy each `(from, to)`.
    Copy(Vec<(PathBuf, PathBuf)>),
    /// Delete each path, a directory with everything in it.
    Delete(Vec<PathBuf>),
    /// Move each `(from, to)`.
    Move(Vec<(PathBuf, PathBuf)>),
    /// Move each path to the trash.
    Trash(Vec<PathBuf>),
}

/// A program to run on an editor's behalf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// Variables set for the program on top of the app's own environment.
    pub env: Vec<(String, String)>,
    /// Which request of its tag this is, handed back in the reply: an editor
    /// keeping what it asked each request for finds it again by this. Nought
    /// for one that keeps nothing.
    pub id: u64,
    /// The program.
    pub program: String,
    /// What to write to the program's standard input, for a command that reads
    /// its payload rather than taking it as an argument.
    pub stdin: Option<String>,
    /// Which request this is, so the editor knows what finished.
    pub tag: &'static str,
}

/// A command an editor runs by name, which its block in `keybindings.kdl` can
/// bind to a chord or a sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorCommand {
    /// What the hint and the Keys editor call it.
    pub label: &'static str,
    /// The name a binding gives it, and [`Editor::run_command`] is handed.
    pub name: &'static str,
}

/// What an editor waiting on the second key of a command offers: what the
/// sequence is called, and the keys that complete it with what each does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorHint {
    /// The continuations, as `(key, what it does)`.
    pub items: Vec<(String, String)>,
    /// Whether this is a menu to choose from, as Git's popups are, shown at
    /// once, rather than the keys a prefix can go on with, shown only once
    /// typing has stalled on it as every other key hint is.
    pub menu: bool,
    /// What the sequence is called, shown as the hint's heading.
    pub title: String,
}

/// A choice an editor asks the host to collect from a list it already knows: the
/// branches to check out, the tags to delete, the remotes to prune.
///
/// What the host shows is a filter over `items`, not a text field: a name that
/// is not in the list is not an answer, which is the difference between this
/// and a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickRequest {
    /// The icon drawn before each choice, in the same order: a list of files
    /// shows their types. Empty for a list drawn without icons.
    pub icons: Vec<IconKind>,
    /// The choices, in the order they should read.
    pub items: Vec<String>,
    /// What the list is of, shown where a prompt shows its question.
    pub label: String,
    /// Where the list is shown.
    pub placement: PickPlacement,
    /// Which question this answers, so the editor knows what came back.
    pub tag: &'static str,
}

/// Where a list an editor asks from is shown.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PickPlacement {
    /// Centred on the window under a title bar naming what it is of: a
    /// question about the file or the project.
    #[default]
    Centered,
    /// Floating at the cursor with no title bar: what can be done right
    /// there, as the fixes and the spellings of a word.
    Cursor,
}

/// A question an editor answers from a list rather than from typing, held while
/// the list itself is gathered.
///
/// The gathering differs per editor: a git branch list is a command away, a
/// directory's own rows are already in hand — but what happens to the answer
/// does not: it arrives under [`PickQuestion::tag`] as the reply to the same
/// question a [`PromptRequest`] would have asked. A list that comes back
/// empty is no list at all, and every constructor here says so with `None`,
/// which is the caller's cue to fall back to asking for it typed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickQuestion {
    /// What the list is of, shown as the picker's heading.
    pub label: String,
    /// The question the choice answers.
    pub tag: &'static str,
}

impl PickQuestion {
    /// A question to be answered from a list.
    pub fn new(tag: &'static str, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tag,
        }
    }

    /// The list an editor already holds, in the order it should read.
    pub fn over(&self, items: Vec<String>) -> Option<PickRequest> {
        (!items.is_empty()).then(|| PickRequest {
            icons: Vec::new(),
            items,
            label: self.label.clone(),
            placement: PickPlacement::Centered,
            tag: self.tag,
        })
    }

    /// The list a program wrote, one candidate per line: blank lines are
    /// dropped, each line is trimmed, and `keep` has the last word on what
    /// belongs (a remote's own `HEAD`, say, which stands for a name already
    /// listed).
    pub fn over_lines(&self, text: &str, keep: impl Fn(&str) -> bool) -> Option<PickRequest> {
        self.over(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && keep(line))
                .map(str::to_string)
                .collect(),
        )
    }
}

/// One place the jump list can go: a line, a row, an entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct JumpTarget {
    /// What the place is, drawn before its label: a listing's file or folder.
    /// `None` for a line of text, which is not a thing with a type.
    pub icon: Option<IconKind>,
    /// How the list shows it.
    pub label: String,
    /// The editor's own index for it, as a [`JumpStep`] names it.
    pub target: usize,
}

/// One step through the jump list or an outline, offered to the editor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum JumpStep {
    /// The jump called off: back to where it started.
    Cancel,
    /// The move to the target made for good, where it started kept as the
    /// place to jump back to.
    Confirm(usize),
    /// The target shown while the list is still being gone through.
    Preview(usize),
}

/// A text search over a directory tree, run without a pattern engine: the
/// query is literal text, read the way [`SearchPattern`] says.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    /// How a text query is read, past finding it as it is: case, regular
    /// expressions, whole words, and which files are looked in.
    pub options: SearchOptions,
    /// How the query is matched against a line.
    pub pattern: SearchPattern,
    /// The text every reported line holds, or the words one of which it holds.
    pub query: String,
    /// Where the walk starts.
    pub root: PathBuf,
}

/// How a [`SearchPattern::Text`] query is read, as the Grep editor's toggles
/// set it: the default finds the text as it is, in any case, in any file.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchOptions {
    /// Whether case counts: off, `foo` finds `Foo`.
    pub case_sensitive: bool,
    /// Only the files whose name this glob matches (`*.rs`), or every file.
    pub glob: Option<String>,
    /// Whether the query is a regular expression rather than text.
    pub regex: bool,
    /// Whether what the query matches must stand as a whole word.
    pub whole_word: bool,
}

/// How a [`SearchRequest`]'s query is matched against a line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchPattern {
    /// The query as one run of text, found anywhere in the line, whatever the
    /// case: what a person typing a search means.
    Text,
    /// The query as words split on whitespace, any one of which the line holds
    /// as a whole word in exactly that case: `TODO` finds `// TODO: x` and
    /// `TODO(me)`, never `todos` or `TODOS`. What a list of comment keywords
    /// means.
    Words,
}

impl SearchRequest {
    /// Look under `root` for lines `query` matches, read as `pattern` says.
    pub fn new(pattern: SearchPattern, query: impl Into<String>, root: PathBuf) -> Self {
        Self {
            options: SearchOptions::default(),
            pattern,
            query: query.into(),
            root,
        }
    }

    /// The same search, reading its query as `options` say.
    pub fn with_options(self, options: SearchOptions) -> Self {
        Self { options, ..self }
    }

    /// The words a [`SearchPattern::Words`] query stands for, without blanks.
    pub fn split_words(&self) -> Vec<&str> {
        self.query.split_whitespace().collect()
    }
}

/// The answer to a [`JobRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobReply {
    /// What a program wrote, and how it exited.
    Command(CommandOutput),
    /// What a directory holds, with unreadable parts skipped rather than
    /// reported as an error: a total is worth more than a failure here.
    DirSize {
        /// Bytes under the directory.
        bytes: u64,
        /// The directory that was totalled.
        path: PathBuf,
    },
    /// What the files that could be read hold, keyed by the path asked for.
    /// A file that is missing or unreadable is simply absent.
    Files(Vec<(PathBuf, String)>),
    /// Every process, or why the system would not list them.
    Processes(Result<ProcessSample, String>),
    /// The lines a search found.
    Search(SearchResult),
    /// The machine, or why the system would not describe it.
    System(Result<SystemSample, String>),
    /// What a [`JobRequest::FileOps`] did (`"copied 3"`), or where it stopped
    /// and why, with how many it had done by then.
    FileOps(Result<String, String>),
}

/// What running a program produced. A program that could not be started at all
/// reports no code and its reason on `stderr`, so one path covers both.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandOutput {
    /// Exit status, absent when the program never ran.
    pub code: Option<i32>,
    /// The id of the request this answers (see [`CommandRequest::id`]).
    pub id: u64,
    /// What it wrote to standard error.
    pub stderr: String,
    /// What it wrote to standard output.
    pub stdout: String,
    /// The tag of the request this answers.
    pub tag: &'static str,
}

impl CommandOutput {
    /// Whether the program ran and reported success.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// The first line of the failure, for a one-line report.
    pub fn failure(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        text.lines().next().unwrap_or("failed").to_string()
    }
}

/// What a search found. A walk that hit its cap reports what it had rather
/// than failing, since a first page of matches is still worth showing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResult {
    /// Why nothing was looked for, for a query that cannot be read: a
    /// regular expression that does not parse.
    pub error: Option<String>,
    /// The matching lines, in the order the walk found them.
    pub hits: Vec<SearchHit>,
    /// The query this answers, so an editor that has moved on to another one can
    /// tell that this is not about the query it is showing.
    pub query: String,
    /// Whether the cap stopped the walk with matches left unreported.
    pub truncated: bool,
}

/// One matching line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    /// Which line of the file it is, counting from one.
    pub line: usize,
    /// The file holding it.
    pub path: PathBuf,
    /// The line itself, with trailing whitespace dropped.
    pub text: String,
}

/// The search an editor just ran or repeated, which the host highlights in the
/// tab and counts in the status bar the way it does a terminal's.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchReport {
    /// The cursor's place among the matches, counting from one, or 0 when it
    /// is on none.
    pub index: usize,
    /// The text looked for.
    pub query: String,
    /// Whether the search runs backwards, so the bar shows `?` rather than `/`.
    pub reverse: bool,
    /// How many matches there are, or 0 when the editor does not count them.
    pub total: usize,
}

/// A command an editor wants run in a real terminal rather than captured.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// The program.
    pub program: String,
}

/// What an editor puts in the window's status bar while it has the keyboard:
/// where the keys stand, rather than what the editor shows. What the editor
/// shows stays in its own rows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorStatus {
    /// The editor's own mode, drawn where a terminal's is. `None` for an editor
    /// with no mode of its own, which the bar marks as an editor.
    pub mode: Option<EditMode>,
    /// Short facts about where the cursor is and what is part-typed, drawn
    /// at the bar's right edge in order: `12:5`, `340 lines`, `"a`.
    pub segments: Vec<String>,
}

/// What an editor is asking the host to read from the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptRequest {
    /// Text the input starts out holding, for an answer that is usually an edit
    /// of what is already there.
    pub initial: String,
    /// What is being asked, shown before the input.
    pub label: String,
    /// Whether the answer is typed or a single yes-or-no key.
    pub mode: PromptMode,
    /// Which question this is, so the editor knows what the answer belongs to.
    pub tag: &'static str,
}

impl PromptRequest {
    /// A yes-or-no question for what cannot be undone: `question` with its
    /// first letter a capital, and the keys that answer it after.
    pub fn confirm(tag: &'static str, question: &str) -> Self {
        let mut chars = question.chars();
        let label = match chars.next() {
            Some(first) => format!("{}{} (y/n) ", first.to_uppercase(), chars.as_str()),
            None => "(y/n) ".to_string(),
        };
        Self {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag,
        }
    }

    /// The `/` prompt, searching forward through what the editor shows.
    pub fn search(tag: &'static str) -> Self {
        Self {
            initial: String::new(),
            label: "/".to_string(),
            mode: PromptMode::Search { reverse: false },
            tag,
        }
    }
}

/// The answer to a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptReply {
    /// What the user typed, or `None` when the prompt was cancelled.
    pub answer: Option<String>,
    /// The tag of the question being answered.
    pub tag: &'static str,
}

/// How a prompt collects its answer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PromptMode {
    /// One key: `y` confirms, anything else cancels. For a question whose wrong
    /// answer cannot be undone.
    Confirm,
    /// Single-key query-replace choice: `y` (yes), `n` (no), `!` (all), `.` (replace & exit), `q` (quit).
    QueryReplace,
    /// A line of text to search for, ending at `Enter`: the host highlights
    /// what it matches in the tab while it is typed.
    Search {
        /// Whether the search runs backwards (`?`).
        reverse: bool,
    },
    /// A line of text drawn as dots, ending at `Enter`: a password. The
    /// host neither recalls nor remembers what is typed into it.
    Secret,
    /// A line of text, ending at `Enter`.
    #[default]
    Text,
}

/// What an editor has open over its rows, for the window to draw there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Popups {
    /// The card over the word the pointer or the cursor is on.
    pub card: Option<InfoCard>,
    /// The completion popup at the cursor.
    pub completion: Option<CompletionPopup>,
    /// The drop-down list over the pane.
    pub drop_list: Option<DropList>,
}

/// The completion popup an editor has open at its cursor, for the window to
/// draw under it.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionPopup {
    /// What it offers, the best first.
    pub items: Vec<CompletionItem>,
    /// Which entry is chosen.
    pub selected: usize,
    /// How many characters before the cursor the entries would replace, so
    /// the popup lines up under the word being completed.
    pub typed: usize,
}

/// One entry of a completion popup.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    /// More about it, drawn dim beside it.
    pub detail: Option<String>,
    /// The letter its kind is marked with.
    pub kind: char,
    /// What it reads as.
    pub label: String,
    /// The back-end that offered it.
    pub source: &'static str,
}

/// A file's text as the editor holding it has it, for a view following it
/// (a Markdown preview): a number that moves whenever the text does, where
/// the cursor is, and the lines, left out where the asker already has them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedText {
    /// The cursor's line, from zero.
    pub cursor: usize,
    /// The text, or `None` where it has not changed since the revision the
    /// asker named.
    pub lines: Option<Vec<String>>,
    /// The text's revision.
    pub revision: usize,
}

/// A card of text an editor shows over its rows: what a name is, or the
/// problem a server found there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InfoCard {
    /// The cell it hangs from, as a row and a column of the pane, for one
    /// the pointer asked for; `None` hangs it from the cursor.
    pub cell: Option<(usize, usize)>,
    /// What kind of thing it is about, said apart at its foot: the problems'
    /// kinds and what found them. `None` for a card with no foot.
    pub footer: Option<String>,
    /// What it says, one line per line.
    pub text: String,
}

/// A bar an editor has drawn over its top rows in the window's own font, as
/// the status bar is, rather than painted into the rows themselves: the rows
/// it takes, which the editor leaves blank under it, and its items.
#[derive(Clone, Debug, PartialEq)]
pub struct Toolbar {
    /// The items from the left edge.
    pub left: Vec<winter_render::ToolbarItem>,
    /// The items against the right edge.
    pub right: Vec<winter_render::ToolbarItem>,
    /// How many of the pane's top rows it covers.
    pub rows: usize,
}

/// A drop-down list an editor has open over its pane, lying on its cells:
/// the PDF viewer's outline under its toolbar. A click on it comes to the
/// editor as one on the cell under it, as every click does.
#[derive(Clone, Debug, PartialEq)]
pub struct DropList {
    /// The pane cell its top-left corner lies on.
    pub cell: (usize, usize),
    /// How many cells wide it is.
    pub cols: usize,
    /// Its rows, one to a cell row.
    pub rows: Vec<winter_render::DropListRow>,
    /// Which row is chosen, if any shows.
    pub selected: Option<usize>,
}

/// Two files or directories to show the differences between, in a diff
/// beside the work.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CompareRequest {
    /// The files written only to be compared, removed once the view goes,
    /// each with the file it stands for, where it stands for one: a buffer's
    /// text written out is the file the buffer edits, so a hunk opens that.
    pub copies: Vec<(PathBuf, Option<PathBuf>)>,
    /// The side the changes go to.
    pub new: PathBuf,
    /// The side the changes come from.
    pub old: PathBuf,
    /// What the comparison is of, for the tab.
    pub title: String,
}

/// A git view to read in a tab of its own, as Magit reads a commit in a
/// buffer of its own: a commit, a stash, a log, a diff, or a blame, asked
/// for from the status or from another such view.
#[derive(Clone, Debug, PartialEq)]
pub struct GitViewRequest {
    /// Where the repository keeps its own files, where git said.
    pub git_dir: Option<PathBuf>,
    /// The read whose answer is the view.
    pub request: JobRequest,
    /// The repository's working tree.
    pub root: PathBuf,
}

/// Texts to compare or merge in the diff tool, as Emacs' ediff does:
/// two or three to compare, or two to merge (against their ancestor where
/// one is given) into a third.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffRequest {
    /// The ancestor two merged texts share.
    pub ancestor: Option<DiffText>,
    /// Where a merge is written, for a merge; `None` to compare.
    pub merge: Option<DiffMerge>,
    /// The texts, A and B, and C for a three-way comparison.
    pub texts: Vec<DiffText>,
}

/// What a file's bytes said about their own layout, kept so that saving an
/// unrelated one-line change does not rewrite every line of the file. Both of
/// these are invisible on screen and loud in a diff.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct FileShape {
    /// Whether the file began with a byte order mark.
    pub bom: bool,
    /// Whether lines ended `\r\n` rather than `\n`.
    pub crlf: bool,
    /// Whether the last line ended with a newline of its own.
    pub trailing_newline: bool,
}

/// One text of a diff session.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffText {
    /// What it is called: a file's path, a revision, a buffer.
    pub label: String,
    /// What it reads, which may be a buffer's unsaved text.
    pub lines: Vec<String>,
    /// The file it is written back to, or `None` for one that is not (a
    /// revision, the clipboard).
    pub path: Option<PathBuf>,
    /// How that file's lines are laid out, which a write keeps: carried
    /// from where the text was read, so the diff does not read the file
    /// again to learn it. `None` where no file stands behind the text.
    pub shape: Option<FileShape>,
}

/// Where a diff session's merge is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiffMerge {
    /// The file, or `None` for a merge only read.
    pub output: Option<PathBuf>,
}

/// What the host should do with a key the editor was offered.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorOutcome {
    /// Close the editor, and the pane holding it.
    Close,
    /// Show the differences between two files, in a view beside this one.
    Compare(CompareRequest),
    /// Put `rows` on the clipboard as rich text in the theme's colors, for a
    /// document or a slide, with `text` for what takes only plain text.
    CopyStyled {
        /// The lines, as they are painted.
        rows: Vec<super::paint::Row>,
        /// The same lines as plain text.
        text: String,
    },
    /// The editor acted on the key.
    Consumed,
    /// Read a git view in a tab of its own, or go to the tab already
    /// showing it.
    GitView(GitViewRequest),
    /// The editor has no binding for it, so resolve it as an ordinary key.
    Ignored,
    /// The editor asks the host to open this path with the system handler.
    OpenExternal(PathBuf),
    /// The editor asks the host to stop the work it started.
    CancelJobs,
    /// The editor asks the host to do slow work for it.
    Job(JobRequest),
    /// The editor holds a place for what is chosen to open there, and it
    /// is chosen: the host opens it in the editor's tab, in its directory.
    Launch(Launch),
    /// The editor asks the host to read an answer from the user.
    Prompt(PromptRequest),
    /// The editor asks the host to have the user choose from a list, rather
    /// than type. The answer comes back as a [`PromptReply`] under the same
    /// tag, so a question asked either way is answered in one place.
    Pick(PickRequest),
    /// The editor asks the host to run one of its own commands, named the way the
    /// command palette names it, and is done: it closes first, as the
    /// command list does once a command is picked.
    RunAction(String),
    /// The editor asks the host to run one of its own commands for it, and
    /// stays open: a file's editor opening its preview or building its
    /// document.
    RunActionHere(String),
    /// The editor asks the host to record or replay a keyboard macro.
    Macro(MacroStep),
    /// The editor asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The editor asks the host to copy this to the clipboard.
    Yank(String),
    /// The editor asks the host to put what a delete took on the clipboard,
    /// saying nothing.
    Cut(String),
    /// The editor asks the host to copy text to the clipboard and say
    /// something of its own, for text too long to echo back the way
    /// [`Self::Yank`] does.
    Copy {
        /// What to say once it is copied.
        notice: String,
        /// What to copy.
        text: String,
    },
    /// The editor asks the host to open this file for editing, in the editor
    /// the app carries.
    OpenPath(OpenTarget),
    /// The editor is starting a build: the host saves the files being
    /// edited first, and takes it as the newest build, the one rerun and
    /// next-error mean. The editor starts the build on its next tick.
    Build(BuildStart),
    /// The editor asks the host to open this file beside the editor rather
    /// than over it, so what it was opened from stays in sight.
    OpenBeside(OpenTarget),
    /// The editor wrote this file to disk, and acted on the key that asked
    /// it to: the host may follow a save up, rebuilding the document it
    /// belongs to.
    Wrote(PathBuf),
    /// The editor asks the host to open this file in `$EDITOR`, in a pane of
    /// its own, for the editing the app's own editor deliberately cannot do.
    SpawnEditor(OpenTarget),
    /// Compare or merge texts in the diff tool, in a tab of its own.
    Diff(DiffRequest),
    /// The editor asks the host for what is on the clipboard, which comes back
    /// through [`Editor::on_paste`].
    Paste,
    /// The editor asks the host to show the PDF at `path`, in the tab
    /// already showing it or a new one, with `mark` brought into view.
    ShowInPdf {
        /// Whether in the other pane rather than this one.
        beside: bool,
        /// Where on which page, or `None` to open the PDF as it is.
        mark: Option<PdfMark>,
        /// The PDF.
        path: PathBuf,
    },
}

/// What a pane's launcher opens in its place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Launch {
    /// A directory listing.
    Dir,
    /// The file picker, over what is under the directory.
    Files,
    /// The working tree's state.
    Git,
    /// A shell.
    Terminal,
}

/// A build an editor is about to start, as the host knows it by.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
pub struct BuildStart {
    /// The shell command line.
    pub command: String,
    /// Where it runs.
    pub dir: PathBuf,
    /// The project it belongs to.
    pub root: PathBuf,
}

/// A box on a page of a PDF, for the viewer to bring into view and mark:
/// where SyncTeX says a line of LaTeX was set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfMark {
    /// How tall, in points.
    pub height: f32,
    /// The left edge, in points from the page's.
    pub left: f32,
    /// The page, from zero.
    pub page: usize,
    /// The top edge, in points down from the page's.
    pub top: f32,
    /// How wide, in points; zero for a spot with no width of its own.
    pub width: f32,
}

/// One entry an editor offers in the menu opened over its own rows.
///
/// An entry names a key the editor already binds, rather than a command of its
/// own. A menu built that way cannot offer something the keyboard cannot do,
/// cannot drift from what the keys mean, and costs an editor one list rather
/// than a second dispatch path beside `on_key`.
pub struct EditorMenuItem {
    /// The keys choosing this entry stands for, typed in order; none for a
    /// separator.
    pub keys: Vec<Key>,
    /// What the host shows for it.
    pub label: String,
}

/// What an editor asks of the window that only the window can do, since it
/// reaches past the editor's own file.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorRequest {
    /// Make `edits` to the file at `path`, in the buffer holding it, opened
    /// buried where no buffer does.
    Edit {
        /// The edits, as the server gave them.
        edits: Vec<winter_lsp::protocol::TextEdit>,
        /// The file they are to.
        path: PathBuf,
    },
    /// Show the file `target` names, at its place: a jump into another file.
    Open(OpenTarget),
}

/// The window's look, for an editor that draws a page of its own in it: the
/// theme's colors, how many screen pixels a logical one is, and how rows of
/// highlighted code read as HTML in those colors.
pub struct Appearance<'a> {
    /// Screen pixels to a logical pixel.
    pub scale: f32,
    /// `rows` as a `<pre>` of spans in the theme's colors.
    pub styled_html: &'a dyn Fn(&[Row]) -> String,
    /// The theme in effect.
    pub theme: &'a winter_render::Theme,
}

/// A file an editor wants opened, and where in it to land.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenTarget {
    /// The column to put the cursor on, counting from one, when the line
    /// is known and so is the column.
    pub column: Option<usize>,
    /// The line to put the cursor on, when the editor knows one.
    pub line: Option<usize>,
    /// The file to open.
    pub path: PathBuf,
}

// ========================================================================
// Functions
// ========================================================================

/// The index of the next label holding `query`, looking one past `from` and
/// wrapping once so the row under the cursor is tried last. Matching is
/// [`match_spans`]'s, which keeps an editor's search free of a pattern engine.
pub fn find_match(
    labels: &[impl AsRef<str>],
    query: &str,
    from: usize,
    forward: bool,
) -> Option<usize> {
    let total = labels.len();
    if query.is_empty() || total == 0 {
        return None;
    }
    (1..=total)
        .map(|step| {
            if forward {
                (from + step) % total
            } else {
                (from + total - step) % total
            }
        })
        .find(|&index| contains_match(labels[index].as_ref(), query))
}

/// What a search over rows comes to with the cursor on row `at`: the rows
/// holding `query` are the matches, since a row is what `n` steps to.
pub fn row_search_report(
    labels: &[impl AsRef<str>],
    query: &str,
    at: usize,
    reverse: bool,
) -> SearchReport {
    let mut report = SearchReport {
        query: query.to_string(),
        reverse,
        ..SearchReport::default()
    };
    for (row, label) in labels.iter().enumerate() {
        if contains_match(label.as_ref(), query) {
            report.total += 1;
            if row == at {
                report.index = report.total;
            }
        }
    }
    report
}

/// Every place `query` occurs in `text`, as `(start, end)` character columns,
/// overlapping ones included. A query typed in lower case matches either
/// case, and one with a capital in it matches exactly: the rule every editor
/// searches by, so what a search lands on is what is highlighted.
pub fn match_spans(text: &str, query: &str) -> Vec<(usize, usize)> {
    let folded = !query.chars().any(char::is_uppercase);
    // Folded a character at a time, so a column of the folded text is a
    // column of the text even where lowering a letter would lengthen it.
    let fold = |s: &str| -> Vec<char> {
        s.chars()
            .map(|c| match folded {
                true => c.to_lowercase().next().unwrap_or(c),
                false => c,
            })
            .collect()
    };
    let needle = fold(query);
    let haystack = fold(text);
    if needle.is_empty() || haystack.len() < needle.len() {
        return Vec::new();
    }
    (0..=haystack.len() - needle.len())
        .filter(|&start| haystack[start..].starts_with(&needle))
        .map(|start| (start, start + needle.len()))
        .collect()
}

/// Whether `query` occurs anywhere in `text`, by [`match_spans`]'s rule.
pub fn contains_match(text: &str, query: &str) -> bool {
    !match_spans(text, query).is_empty()
}

/// Token inspection details under the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenInspectData {
    /// The character under the cursor and its codepoint.
    pub character: String,
    /// The scope selector rule that matched the token's color.
    pub color_rule: Option<String>,
    /// The hex color string if defined by the matching rule.
    pub foreground_hex: Option<String>,
    /// The syntax grammar name.
    pub language: String,
    /// Human-readable 1-based line and column in the file.
    pub position: String,
    /// The TextMate scope stack from root to leaf.
    pub scopes: Vec<String>,
    /// The font style (e.g. bold, italic).
    pub style: Option<String>,
}

impl TokenInspectData {
    /// Format a character with its codepoint (e.g. `'a' (U+0061)`).
    pub fn format_char(ch: char) -> String {
        format!("'{}' (U+{:04X})", ch.escape_debug(), ch as u32)
    }

    /// Format a character with optional combining tail codepoints.
    pub fn format_char_with_tail(ch: char, tail: Option<&str>) -> String {
        if ch == ' ' || ch == '\0' {
            format!("' ' (U+{:04X})", if ch == '\0' { 0 } else { 0x20 })
        } else if let Some(tail) = tail {
            format!(
                "'{}{}' (U+{:04X})",
                ch.escape_debug(),
                tail.escape_debug(),
                ch as u32
            )
        } else {
            Self::format_char(ch)
        }
    }

    /// Format a character, or newline if absent.
    pub fn format_char_or_newline(ch: Option<char>) -> String {
        match ch {
            Some(c) => Self::format_char(c),
            None => "\\n (U+000A)".to_string(),
        }
    }

    /// Human-readable 1-based line and column in the file or listing.
    pub fn format_position(row: usize, col: usize) -> String {
        format!("ln {}, col {}", row + 1, col + 1)
    }
}

// ========================================================================
// Traits
// ========================================================================

/// What an editor tab holds. An editor paints itself as rows of styled text and
/// gets first refusal on keys while its tab is focused.
///
/// The trait holds what any editor may do. What only one kind of editor does
/// (the text editor's language servers, Git's status view) is that editor's
/// own method, reached through `with_kind`.
pub trait Editor: AsAny {
    /// A short label for the pane title.
    fn title(&self) -> String;

    /// What the tab's icon depicts, when the editor shows something with a type
    /// of its own: the file an editor has open. `None` draws the icon of the
    /// editor's kind.
    fn icon(&self) -> Option<IconKind> {
        None
    }

    /// The editor's buffer is being killed: tell whatever is attached to the
    /// file (a language server, a program blocked on the edit) that it is
    /// over, ahead of the drop.
    fn on_closed(&mut self) {}

    /// A copy of the state of the view loaded now (cursor, scroll, selection,
    /// pane geometry), for the window to keep while another view of the same
    /// buffer is loaded. Small by design: what can be shared (the text, its
    /// caches) stays in the editor. `None` for a kind whose views share one.
    fn save_view(&self) -> Option<Box<dyn Any>> {
        None
    }

    /// Load a view's state that [`Editor::save_view`] gave earlier.
    fn restore_view(&mut self, _state: Box<dyn Any>) {}

    /// Whether any tab shows the editor. A buried one stops what it refreshes
    /// on a timer (a process list, a file watch), and refreshes once when a
    /// tab shows it again; work running (a build, a search) goes on.
    fn set_shown(&mut self, _shown: bool) {}

    /// Drop what can be built again, as the editor's buffer is buried: what
    /// a buried buffer keeps should be its state, not its pictures.
    fn shed_caches(&mut self) {}

    /// Whether a program is blocked on this edit, such as `git commit` waiting
    /// on its message. Closing such an editor's tab kills its buffer rather
    /// than burying it, since a buried one would keep the program waiting.
    fn is_waited_on(&self) -> bool {
        false
    }

    /// Whether closing this editor's last tab buries its buffer to come back
    /// to, rather than letting it go. An editor holding nothing worth coming
    /// back to (a launcher, a commit message whose commit has run) says no.
    fn is_kept_when_closed(&self) -> bool {
        !self.is_waited_on()
    }

    /// Whether the editor's cursor picks whole rows, as a listing's does: a
    /// view of it keeps its column at the start of the line, outside typing,
    /// so stepping between rows lands each at its name.
    fn picks_rows(&self) -> bool {
        false
    }

    /// Hear which directories the other panes on screen are looking at, just
    /// before a key arrives, for an editor that offers them as a destination
    /// (a listing copying into the listing beside it).
    fn set_neighbors(&mut self, _dirs: Vec<PathBuf>) {}

    /// The repository whose git view the window goes back to once this
    /// editor's tab closes: a commit message's, its work done there. `None`
    /// for every other editor.
    fn returns_to_git(&self) -> Option<PathBuf> {
        None
    }

    /// Where the title bar says this editor is: one fixed location, written
    /// with `~` for home, that no cursor movement, line, or editor changes.
    /// `None` for an editor tied to nowhere on disk, which the bar then names by
    /// its kind alone.
    fn address(&self) -> Option<String> {
        None
    }

    /// The rows to draw this frame, given how many the pane can show below any
    /// rows it pins, and how wide the pane is: an editor longer than its pane
    /// returns the window it wants visible, so scrolling stays where the
    /// cursor is, and when `wrap` is set a row wider than `cols` wraps onto
    /// the next screen row — folding at its last word boundary — rather than
    /// clipping at the pane's edge.
    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame;

    /// Offer a key to the editor.
    fn on_key(&mut self, key: &Key) -> EditorOutcome;

    /// Run one of the editor's own commands by the name its `keybindings.kdl`
    /// block binds it under, or `None` for a name it does not know.
    fn run_command(&mut self, _name: &str) -> Option<EditorOutcome> {
        None
    }

    /// Offer a press or a drag of the pointer to the editor. Editors that take no
    /// pointer never implement it, and the click falls through to the host.
    fn on_mouse(&mut self, _at: Point) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Offer a double-click to the editor, after the press that ends it was
    /// offered through [`Self::on_mouse`]. By default nothing more happens.
    fn on_double_click(&mut self, _at: Point) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Whether the editor glides its own scrolling, by the pixel: the host
    /// then hands it a wheel's whole turn at once, rather than feeding it the
    /// rows a frame at a time.
    fn scrolls_smoothly(&self) -> bool {
        false
    }

    /// Offer a touchpad's scroll to the editor in the pixels it measures,
    /// positive upwards, for an editor that follows it exactly. By default
    /// it is declined, and the host offers it as rows.
    fn on_scroll_pixels(&mut self, _pixels: f32) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Keep `rows` screen rows above and below the cursor's as the view
    /// follows it, where the rows allow: the window's `scroll-margin`, told
    /// before each paint. Editors with no cursor to follow never implement it.
    fn set_scroll_margin(&mut self, _rows: usize) {}

    /// Offer a turn of the wheel to the editor, in rows, positive upwards.
    /// Editors that do not scroll themselves never implement it.
    fn on_scroll(&mut self, _lines: isize) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Offer a press of the mouse's back (`back`) or forward side button,
    /// to step through the places the editor has been. Editors that keep no
    /// such places never implement it.
    fn on_side_button(&mut self, _back: bool) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Hear the pointer rest on the cell `at` with no button held, or leave
    /// the editor's pane (`None`). Editors that show nothing on hover never
    /// implement it.
    fn on_hover(&mut self, _at: Option<Point>) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Hand back what the clipboard holds, for an editor that asked for it.
    fn on_paste(&mut self, _text: String) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// Offer a file to the editor, for an editor that can show more than one.
    /// `false`, the default, means the host opens it however it would have.
    fn open_file(&mut self, _target: OpenTarget) -> bool {
        false
    }

    /// What the editor is waiting for, when it is part-way through a command
    /// that takes more than one key: the sequence's name and what the next
    /// key may be. The host draws it the way it draws its own key hints, so a
    /// multi-key command looks the same wherever it is being typed. Editors
    /// with no multi-key commands never implement it.
    fn hint(&self) -> Option<EditorHint> {
        None
    }

    /// Every key the editor answers in the state it is in, read from the same
    /// key tables its dispatch looks keys up in, for the host's help list.
    fn keys(&self) -> Vec<KeyHelp> {
        Vec::new()
    }

    /// Hand back the answer to a prompt the editor asked for. Editors that never
    /// ask never implement it.
    fn on_prompt(&mut self, _reply: PromptReply) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// Hand back intermediate, debounced changes to what has been typed into an
    /// open prompt, allowing editors to perform iterative search or live filtering.
    fn on_prompt_input(&mut self, _query: &str, _tag: &'static str) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Hand back the result of work the editor asked for.
    fn on_job(&mut self, _reply: JobReply) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// When the editor next wants [`Editor::on_tick`], for an editor that keeps
    /// itself current without being asked. The host wakes for the earliest
    /// deadline of any editor showing. Editors whose content changes only on a key
    /// never implement it.
    fn next_tick(&self) -> Option<Instant> {
        None
    }

    /// The earliest deadline [`Editor::next_tick`] gave that the editor has
    /// since called off, as a timer is cancelled: the host, which keeps the
    /// earliest deadline it was given, drops that one rather than waking
    /// for nothing. `None` where nothing was called off since last asked.
    fn take_cancelled_tick(&mut self) -> Option<Instant> {
        None
    }

    /// The deadline from [`Editor::next_tick`] has passed: refresh whatever the
    /// editor shows, usually by asking for a job.
    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// What an extension's view asked of Winter while taking keys, each
    /// with the extension, for the window to do on its own host. Asked as
    /// [`Editor::take_requests`] is.
    fn take_extension_calls(&mut self) -> Vec<(&'static str, winter_extension::Reply)> {
        Vec::new()
    }

    /// What the editor asks of the window besides its outcome since it was
    /// last asked: a place in another file to go to, edits to make to
    /// another file. Asked after every call, as [`Editor::take_notice`] is,
    /// so an editor answering a server can ask whatever it was doing.
    fn take_requests(&mut self) -> Vec<EditorRequest> {
        Vec::new()
    }

    /// What the editor has to report since it was last asked, for the status
    /// bar: the one way an editor reports. The host asks after every call it
    /// makes into the editor, so a report made alongside some other outcome,
    /// such as a job, still shows, and an editor never draws a report itself.
    fn take_notice(&mut self) -> Option<Notice> {
        None
    }

    /// The search the editor has run or repeated since it was last asked,
    /// taken like [`Editor::take_notice`]. The host highlights its matches
    /// over what the editor paints, so an editor never draws a match itself.
    fn take_search(&mut self) -> Option<SearchReport> {
        None
    }

    /// What the status bar shows for the editor while it is focused: its mode
    /// and where its cursor is. An editor with neither shows nothing there.
    fn status(&self) -> EditorStatus {
        EditorStatus::default()
    }

    /// Where the editor's own caret is on its cursor line, for an editor that puts
    /// one somewhere other than the first thing it painted. An editor whose
    /// cursor is a whole row reports none.
    fn caret(&self) -> Option<Caret> {
        None
    }

    /// Where an editor or shell opened from this editor starts: the directory it
    /// is looking at, following the cursor rather than its root. Editors tied
    /// to nowhere on disk never implement it.
    fn cwd(&self) -> Option<PathBuf> {
        None
    }

    /// Where the editor is looking, for opening it there again after a restart:
    /// the directory an editor is rooted in or the file an editor has open, and
    /// the line its cursor is on. `None` for an editor that cannot be reopened
    /// from a place alone.
    fn snapshot(&self) -> Option<OpenTarget> {
        None
    }

    /// Whether the editor has the file at `path` open, shown or not: an editor
    /// holds every buffer it has open, a viewer the one file it shows.
    fn holds_file(&self, path: &Path) -> bool {
        self.snapshot().is_some_and(|at| at.path == path)
    }

    /// A picture for the host to draw over the whole pane, for an editor whose
    /// content is an image rather than rows. `None`, the default, for a
    /// editor that paints rows.
    fn picture(&self) -> Option<Picture> {
        None
    }

    /// Hear the size of the picture the host decoded and of the pane it is
    /// drawn in, in pixels, each frame it is drawn: what an editor zooming and
    /// scrolling it reckons from.
    fn set_picture_geometry(&mut self, _natural: (f32, f32), _pane: (f32, f32)) {}

    /// Pixels the editor drew itself, each placed in the pane, for an editor
    /// whose content is drawn rather than painted as rows: the pages of a
    /// document. Empty, the default, for an editor that paints rows.
    fn tiles(&self) -> Vec<Tile> {
        Vec::new()
    }

    /// How many of the pane's top rows are the editor's painted rows and not
    /// its drawing: what [`Editor::tiles`] places there is cut away, so a page
    /// scrolled up under a toolbar never covers it. None, the default.
    fn header_rows(&self) -> usize {
        0
    }

    /// Take the window's look each frame it is drawn, ahead of
    /// [`Editor::set_pane_size`], for an editor drawing a page of its own in
    /// the theme's colors.
    fn set_appearance(&mut self, _appearance: &Appearance) {}

    /// Hear the pane's size in pixels each frame it is drawn, for an editor
    /// that lays out its own drawing.
    fn set_pane_size(&mut self, _pane: (f32, f32)) {}

    /// The base directory this editor is browsing or rooted in. Unlike `cwd()`,
    /// this does not track cursor movements across entries inside the editor.
    fn root_dir(&self) -> Option<PathBuf> {
        self.cwd()
    }

    /// A file or directory reference for the current view, formatted for copying.
    /// Editors with a more specific location (like an active selection or cursor line)
    /// override this.
    fn file_reference(&self) -> Option<String> {
        self.cwd().map(|p| crate::model::path::abbreviate_home(&p))
    }

    /// Whether the editor is running work of its own that the host's jobs
    /// do not see, a build, which its tab shows as a spinner.
    fn is_busy(&self) -> bool {
        false
    }

    /// Whether work already under way still wants ticks while the editor's
    /// buffer is buried: a build to see through, edits a language server has
    /// still to hear of. A buried buffer is ticked for nothing else; what it
    /// looks at on a timer waits until a pane shows it again.
    fn has_work_in_flight(&self) -> bool {
        self.is_busy()
    }

    /// A key pressed while a `v` or `V` selection is up over `rows`, the
    /// screen rows it covers: the editor's to act on all of them at once
    /// (Git stages them), or `None` to leave the key to the selection.
    fn on_selected_rows_key(
        &mut self,
        _key: &Key,
        _rows: RangeInclusive<usize>,
    ) -> Option<EditorOutcome> {
        None
    }

    /// Whether the editor is holding work that would be lost with it.
    ///
    /// The host asks before closing a pane or a tab over one, keeps it out of
    /// the reopen stash's way when that fills up, and marks it in the list of
    /// what was closed. Editors holding nothing a reader could lose never
    /// implement it.
    fn is_dirty(&self) -> bool {
        false
    }

    /// The file the editor is showing and its text as edited, for an
    /// extension to read. Editors holding no file's text never implement it.
    fn document(&self) -> Option<(PathBuf, String)> {
        None
    }

    /// A number that moves whenever the text of [`Editor::document`] does,
    /// so an extension can be told of an edit. Editors holding no file's
    /// text never implement it.
    fn text_revision(&self) -> Option<usize> {
        None
    }

    /// The file the editor is showing, without its text: what an
    /// extension is told was opened or closed.
    fn document_path(&self) -> Option<PathBuf> {
        self.document().map(|(path, _)| path)
    }

    /// The keyboard has left the editor, for another pane or another
    /// window: write what it holds, as far as that can be done without
    /// asking anything, and say which files were written. Editors holding
    /// nothing to write never implement it.
    fn focus_lost(&mut self) -> Vec<PathBuf> {
        Vec::new()
    }

    /// Data describing the character and TextMate syntax scope stack under the
    /// cursor, when token inspection is active.
    fn token_inspect(&mut self) -> Option<TokenInspectData> {
        None
    }

    /// The editor is showing again after another one was closed over the top of
    /// it. What it was showing may have changed while it was covered, so a
    /// editor that reads the world re-reads it here. Editors showing something
    /// that cannot go stale never implement it.
    fn on_resume(&mut self) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// What to offer in a menu opened over the editor's rows, for the row the
    /// cursor is on: the host puts the cursor under the pointer before
    /// asking, so an editor answers for what was clicked.
    ///
    /// An empty list, the default, means the editor offers no menu and the
    /// click does nothing. Editors whose rows all mean the same thing never
    /// implement it.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        Vec::new()
    }

    /// Hear a right-click at `at`, before the menu is built: by default the
    /// same as a click, so the menu acts on the row under the pointer. A
    /// editor whose click would lose what the menu should act on (a selection
    /// a "Copy" entry would copy) answers otherwise.
    fn on_context_mouse(&mut self, at: Point) -> EditorOutcome {
        self.on_mouse(at)
    }

    /// What the editor has open over its rows: a card, a completion popup,
    /// a drop-down list. None of them for most editors.
    fn popups(&self) -> Popups {
        Popups::default()
    }

    /// The bar the editor wants drawn over its top rows in the window's own
    /// font. `None` for an editor without one.
    fn toolbar(&self) -> Option<Toolbar> {
        None
    }

    /// A press on button `button` of the editor's toolbar.
    fn on_toolbar(&mut self, _button: u16) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Tooltip text for button `button` of the editor's toolbar.
    fn toolbar_tooltip(&self, _button: u16) -> Option<String> {
        None
    }

    /// What the editor is showing, as an outline: its headings, sections,
    /// and definitions, each naming an index a [`JumpStep`] takes, so the
    /// outline is gone through as the jump list is. `None` for an editor
    /// with no outline to give.
    fn outline_targets(&mut self) -> Option<Vec<JumpTarget>> {
        None
    }

    /// Jump targets in the current view, each naming an internal index (a
    /// line, a row) a [`JumpStep`] takes.
    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        None
    }

    /// Current cursor row or position index corresponding to target indices
    /// in [`jump_targets`](Editor::jump_targets), used to preselect the nearest target when opening jump.
    fn jump_cursor(&self) -> Option<usize> {
        None
    }

    /// Take one step through the jump list or the outline: show a target,
    /// settle on it, or go back to where the jump started.
    fn on_jump(&mut self, _step: JumpStep) {}
}

/// An editor seen as [`Any`], for [`dyn Editor::downcast_ref`]: given to
/// every editor by the blanket impl below, so none writes it.
pub trait AsAny {
    /// This, as [`Any`].
    fn as_any(&self) -> &dyn Any;
    /// The same, to change.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl dyn Editor {
    /// Run `run` on this editor as the kind `T`, if it is one: under the
    /// guard the editor runs in, so a panic in `run` stops the editor as one
    /// in any other call does. `None` for an editor of another kind, or one
    /// stopped.
    ///
    /// The only way into a kind's own methods: there is no reference to one
    /// to hold, so no call can slip past the guard.
    pub fn with_kind<T: Editor + 'static, R>(&self, run: impl FnOnce(&T) -> R) -> Option<R> {
        match (*self).as_any().downcast_ref::<Guarded>() {
            Some(guarded) => guarded.with_inner(|editor| (*editor).as_any().downcast_ref::<T>().map(run)),
            None => (*self).as_any().downcast_ref::<T>().map(run),
        }
    }

    /// The same, to change.
    pub fn with_kind_mut<T: Editor + 'static, R>(&mut self, run: impl FnOnce(&mut T) -> R) -> Option<R> {
        if (*self).as_any().is::<Guarded>() {
            return self
                .as_any_mut()
                .downcast_mut::<Guarded>()?
                .with_inner_mut(|editor| (*editor).as_any_mut().downcast_mut::<T>().map(run));
        }
        self.as_any_mut().downcast_mut::<T>().map(run)
    }

    /// This editor as the kind `T`, seen through the guard, for a test to
    /// look inside. Outside tests a kind is reached only through
    /// [`Self::with_kind`].
    #[cfg(test)]
    pub fn downcast_ref<T: Editor + 'static>(&self) -> Option<&T> {
        match (*self).as_any().downcast_ref::<Guarded>() {
            Some(guarded) => guarded.inner().downcast_ref(),
            None => (*self).as_any().downcast_ref(),
        }
    }

    /// The same, to change.
    #[cfg(test)]
    pub fn downcast_mut<T: Editor + 'static>(&mut self) -> Option<&mut T> {
        if (*self).as_any().is::<Guarded>() {
            return self.as_any_mut().downcast_mut::<Guarded>()?.inner_mut().downcast_mut();
        }
        self.as_any_mut().downcast_mut()
    }
}

// ========================================================================
// EditorOutcome
// ========================================================================

impl EditorOutcome {
    /// Copy `text`, saying how many characters went rather than echoing
    /// what may be pages of it.
    pub fn copied(text: String) -> Self {
        let count = text.chars().count();
        let noun = match count {
            1 => "character",
            _ => "characters",
        };
        Self::Copy {
            notice: format!("Copied {count} {noun}"),
            text,
        }
    }
}

// ========================================================================
// JumpTarget
// ========================================================================

impl JumpTarget {
    /// A place called `label`, drawn without an icon.
    pub fn new(target: usize, label: String) -> Self {
        Self {
            icon: None,
            label,
            target,
        }
    }

    /// The same place, drawn with `icon` before its label.
    pub fn with_icon(self, icon: IconKind) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }
}

// ========================================================================
// OpenTarget
// ========================================================================

impl OpenTarget {
    /// Open `path`, leaving it to the editor where to start.
    pub fn file(path: PathBuf) -> Self {
        Self {
            column: None,
            line: None,
            path,
        }
    }

    /// Open `path` with the cursor on `line`, counting from one.
    pub fn at_line(path: PathBuf, line: usize) -> Self {
        Self {
            column: None,
            line: Some(line),
            path,
        }
    }

    /// Open `path` with the cursor on `line` and, where known, `column`,
    /// both counting from one.
    pub fn at(path: PathBuf, line: usize, column: Option<usize>) -> Self {
        Self {
            column,
            line: Some(line),
            path,
        }
    }
}

// ========================================================================
// EditorMenuItem
// ========================================================================

impl EditorMenuItem {
    /// An entry called `label`, run by `key`.
    pub fn new(key: Key, label: impl Into<String>) -> Self {
        Self::typed(vec![key], label)
    }

    /// An entry called `label`, run by typing `keys` in order: `gg`, or the
    /// `"+y` that copies to the clipboard rather than a register.
    pub fn typed(keys: Vec<Key>, label: impl Into<String>) -> Self {
        Self {
            keys,
            label: label.into(),
        }
    }

    /// A line between two groups of entries.
    pub fn separator() -> Self {
        Self {
            keys: Vec::new(),
            label: String::new(),
        }
    }

    /// Whether this is a line between groups rather than an entry.
    pub fn is_separator(&self) -> bool {
        self.keys.is_empty()
    }
}

/// `groups` of entries, each set off from the next by a separator, with any
/// group left empty dropped rather than leaving two lines together.
pub fn menu_groups(groups: impl IntoIterator<Item = Vec<EditorMenuItem>>) -> Vec<EditorMenuItem> {
    let mut items = Vec::new();
    for group in groups.into_iter().filter(|group| !group.is_empty()) {
        if !items.is_empty() {
            items.push(EditorMenuItem::separator());
        }
        items.extend(group);
    }
    items
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_yes_or_no_question_starts_with_a_capital_and_ends_with_its_keys() {
        // Built by hand in seven editors, two of which capitalized by byte
        // slicing, which panics on a question starting with `é`.
        let asked = PromptRequest::confirm("t", "kill 42 (vim)?");
        assert_eq!(asked.label, "Kill 42 (vim)? (y/n) ");
        assert_eq!(asked.mode, PromptMode::Confirm);
        assert_eq!(PromptRequest::confirm("t", "éteindre?").label, "Éteindre? (y/n) ");
    }

    #[test]
    fn test_menu_groups_set_groups_apart_and_drop_the_empty_ones() {
        let entry = |label: &str| {
            EditorMenuItem::new(Key::plain(crate::model::input::KeyCode::Char('x')), label)
        };
        let items = menu_groups([vec![entry("a"), entry("b")], Vec::new(), vec![entry("c")]]);
        let shape: Vec<&str> = items
            .iter()
            .map(|item| match item.is_separator() {
                true => "-",
                false => item.label.as_str(),
            })
            .collect();
        assert_eq!(shape, ["a", "b", "-", "c"]);
        assert!(menu_groups([Vec::new(), Vec::new()]).is_empty());
    }

    #[test]
    fn test_a_search_looks_past_the_cursor_and_wraps() {
        let labels = ["main.rs", "lib.rs", "mod.rs"];
        assert_eq!(find_match(&labels, "mod", 0, true), Some(2));
        assert_eq!(find_match(&labels, "main", 2, true), Some(0));
        assert_eq!(find_match(&labels, "mod", 0, false), Some(2));
        assert_eq!(find_match(&labels, "lib", 0, false), Some(1));
    }

    #[test]
    fn test_a_search_ignores_case_until_the_query_has_a_capital() {
        assert_eq!(find_match(&["Cargo.toml"], "cargo", 0, true), Some(0));
        assert_eq!(find_match(&["cargo.toml"], "Cargo", 0, true), None);
        assert_eq!(find_match(&["x", "Cargo.toml"], "Cargo", 0, true), Some(1));
    }

    #[test]
    fn test_match_spans_lists_every_occurrence_in_columns_of_the_text() {
        assert_eq!(match_spans("aXa xa", "xa"), vec![(1, 3), (4, 6)]);
        assert_eq!(match_spans("aaa", "aa"), vec![(0, 2), (1, 3)]);
        // `İ` lowers to two characters; folding it whole would shift every
        // column after it one to the right of the text it highlights.
        assert_eq!(match_spans("İ mod", "mod"), vec![(2, 5)]);
    }

    #[test]
    fn test_a_row_search_report_counts_matching_rows_and_the_cursors_place() {
        let labels = ["mod.rs", "lib.rs", "mod_two.rs"];
        let report = row_search_report(&labels, "mod", 2, false);
        assert_eq!((report.index, report.total), (2, 2));
        assert_eq!(row_search_report(&labels, "mod", 1, false).index, 0);
    }

    #[test]
    fn test_the_row_under_the_cursor_matches_last() {
        // Otherwise a query the cursor already sits on never moves anywhere,
        // and repeating it looks like the search broke.
        let labels = ["mod.rs", "modal.rs"];
        assert_eq!(find_match(&labels, "mod", 0, true), Some(1));
        assert_eq!(find_match(&["mod.rs", "lib.rs"], "mod", 0, true), Some(0));
    }

    #[test]
    fn test_nothing_is_found_without_a_query_or_without_rows() {
        // An empty listing would divide by its own length.
        let empty: [&str; 0] = [];
        assert_eq!(find_match(&empty, "mod", 0, true), None);
        assert_eq!(find_match(&["mod.rs"], "", 0, true), None);
        assert_eq!(find_match(&["mod.rs"], "absent", 0, true), None);
    }
}

//! Editors: the tabs Winter paints itself instead of running a terminal in
//! them. An editor takes its own keys and hands the host a [`Frame`] of styled
//! rows each paint; the rows themselves are [`super::paint`]'s.

use std::path::{Path, PathBuf};
use std::time::Instant;

pub use super::notify::Notice;
use super::notify::Notification;

use super::input::table::KeyHelp;
use super::input::Key;
#[cfg(doc)]
use super::paint::Gutter;
use super::paint::{Caret, Frame, GutterSide, IconKind, Point};
use super::picture::{Picture, Tile};
use super::process::ProcessSample;
use super::system::SystemSample;
use super::textedit::modal::EditMode;

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
    /// Which question this answers, so the editor knows what came back.
    pub tag: &'static str,
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
    /// The editor's own index for it, as [`Editor::jump_to`] and
    /// [`Editor::jump_confirm`] take it.
    pub target: usize,
}

/// A text search over a directory tree, run without a pattern engine: the
/// query is literal text, matched whatever the case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    /// The text every reported line holds.
    pub query: String,
    /// Where the walk starts.
    pub root: PathBuf,
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

/// What the host should do with a key the editor was offered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum EditorOutcome {
    /// Close the editor, and the pane holding it.
    Close,
    /// The editor acted on the key.
    Consumed,
    /// The editor has no binding for it, so resolve it as an ordinary key.
    Ignored,
    /// The editor asks the host to open this path with the system handler.
    OpenExternal(PathBuf),
    /// The editor asks the host to stop the work it started.
    CancelJobs,
    /// The editor asks the host to do slow work for it.
    Job(JobRequest),
    /// The editor asks the host to read an answer from the user.
    Prompt(PromptRequest),
    /// The editor asks the host to have the user choose from a list, rather
    /// than type. The answer comes back as a [`PromptReply`] under the same
    /// tag, so a question asked either way is answered in one place.
    Pick(PickRequest),
    /// The editor asks the host to run one of its own commands, named the way the
    /// command palette names it.
    RunAction(String),
    /// The editor asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The editor asks the host to copy this to the clipboard.
    Yank(String),
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
    /// The editor asks the host to open this file in `$EDITOR`, in a pane of
    /// its own, for the editing the app's own editor deliberately cannot do.
    SpawnEditor(OpenTarget),
    /// The editor asks the host for what is on the clipboard, which comes back
    /// through [`Editor::on_paste`].
    Paste,
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

/// A file an editor wants opened, and where in it to land.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenTarget {
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

// ========================================================================
// Traits
// ========================================================================

/// What an editor tab holds. An editor paints itself as rows of styled text and
/// gets first refusal on keys while its tab is focused.
pub trait Editor {
    /// A short label for the pane title.
    fn title(&self) -> String;

    /// What the tab's icon depicts, when the editor shows something with a type
    /// of its own: the file an editor has open. `None` draws the icon of the
    /// editor's kind.
    fn icon(&self) -> Option<IconKind> {
        None
    }

    /// The editor is being closed and put by for reopening, not dropped. An editor
    /// that something else is blocked on lets go of it here, since an editor kept
    /// alive would otherwise leave that waiting for as long as the window
    /// lives. What it held is not there when the editor comes back.
    fn on_closed(&mut self) {}

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

    /// Offer a turn of the wheel to the editor, in rows, positive upwards.
    /// Editors that do not scroll themselves never implement it.
    fn on_scroll(&mut self, _lines: isize) -> EditorOutcome {
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

    /// The deadline from [`Editor::next_tick`] has passed: refresh whatever the
    /// editor shows, usually by asking for a job.
    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        EditorOutcome::Consumed
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

    /// Hear which directories the other panes on screen are looking at, just
    /// before a key arrives, for an editor that offers them as a destination
    /// (a listing copying into the listing beside it).
    fn set_neighbors(&mut self, _dirs: Vec<PathBuf>) {}

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

    /// Hear the pane's size in pixels each frame it is drawn, for an editor
    /// that lays out its own drawing.
    fn set_pane_size(&mut self, _pane: (f32, f32)) {}

    /// Hear which edge the user wants the gutter at, for an editor that leads
    /// its rows with one (see [`Gutter`]), just before it is painted.
    fn set_gutter_side(&mut self, _side: GutterSide) {}

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

    /// Whether the editor is holding work that would be lost with it.
    ///
    /// The host asks before closing a pane or a tab over one, keeps it out of
    /// the reopen stash's way when that fills up, and marks it in the list of
    /// what was closed. Editors holding nothing a reader could lose never
    /// implement it.
    fn is_dirty(&self) -> bool {
        false
    }

    /// Whether this editor is editing a commit message.
    fn is_commit_editor(&self) -> bool {
        false
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

    /// Hear the window's notification history, newest first, each time it
    /// changes, with the time it was handed over in seconds since the Unix
    /// epoch: for the editor that lists it. Every other editor ignores it.
    fn set_notifications(&mut self, _entries: &[Notification], _now: i64) {}

    /// Jump targets in the current view, each naming an internal index (a
    /// line, a row) understood by [`jump_to`] and [`jump_confirm`].
    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        None
    }

    /// Current cursor row or position index corresponding to target indices
    /// in [`jump_targets`], used to preselect the nearest target when opening jump.
    fn jump_cursor(&self) -> Option<usize> {
        None
    }

    /// Move the view/cursor to preview `target` while fuzzy jumping.
    fn jump_to(&mut self, _target: usize) {}

    /// Restore the pre-jump view/cursor position when jump is cancelled.
    fn jump_cancel(&mut self) {}

    /// Confirm jumping to `target`, committing the move and saving the jump origin.
    fn jump_confirm(&mut self, _target: usize) {}
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
        Self { line: None, path }
    }

    /// Open `path` with the cursor on `line`, counting from one.
    pub fn at_line(path: PathBuf, line: usize) -> Self {
        Self {
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

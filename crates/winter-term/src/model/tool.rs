//! Tools: the tabs Winter paints itself instead of running a terminal in
//! them. A tool takes its own keys and hands the host a [`Frame`] of styled
//! rows each paint; the rows themselves are [`super::paint`]'s.

use std::path::PathBuf;
use std::time::Instant;

pub use winter_render::NoticeKind;

use super::input::Key;
use super::paint::{Caret, Frame, Point};
use super::process::ProcessSample;
use super::system::SystemSample;
use super::textedit::modal::EditMode;

// ========================================================================
// Data Structures
// ========================================================================

/// Slow work a tool is asking the host to do off the event-loop thread.
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
}

/// A program to run on a tool's behalf.
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
    /// Which request this is, so the tool knows what finished.
    pub tag: &'static str,
}

/// What a tool waiting on the second key of a command offers: what the
/// sequence is called, and the keys that complete it with what each does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolHint {
    /// The continuations, as `(key, what it does)`.
    pub items: Vec<(String, String)>,
    /// What the sequence is called, shown as the hint's heading.
    pub title: String,
}

/// A choice a tool asks the host to collect from a list it already knows: the
/// branches to check out, the tags to delete, the remotes to prune.
///
/// What the host shows is a filter over `items`, not a text field: a name that
/// is not in the list is not an answer, which is the difference between this
/// and a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickRequest {
    /// The choices, in the order they should read.
    pub items: Vec<String>,
    /// What the list is of, shown where a prompt shows its question.
    pub label: String,
    /// Which question this answers, so the tool knows what came back.
    pub tag: &'static str,
}

/// A question a tool answers from a list rather than from typing, held while
/// the list itself is gathered.
///
/// The gathering differs per tool: a git branch list is a command away, a
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

    /// The list a tool already holds, in the order it should read.
    pub fn over(&self, items: Vec<String>) -> Option<PickRequest> {
        (!items.is_empty()).then(|| PickRequest {
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
    /// The query this answers, so a tool that has moved on to another one can
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

/// A command a tool wants run in a real terminal rather than captured.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// The program.
    pub program: String,
}

/// A transient message for the shared status bar (or floating notice fallback).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolNotice {
    /// Visual style of the notice (error or info confirmation).
    pub kind: NoticeKind,
    /// The message to display.
    pub text: String,
}

impl ToolNotice {
    /// Create an error notice (red in status bar).
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Error,
            text: text.into(),
        }
    }

    /// Create an informational confirmation notice (green/accent in status bar).
    pub fn info(text: impl Into<String>) -> Self {
        Self {
            kind: NoticeKind::Info,
            text: text.into(),
        }
    }
}

/// What a tool puts in the window's status bar while it has the keyboard:
/// where the keys stand, rather than what the tool shows. What the tool
/// shows stays in its own rows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ToolStatus {
    /// The tool's own mode, drawn where a terminal's is. `None` for a tool
    /// with no mode of its own, which the bar marks as a tool.
    pub mode: Option<EditMode>,
    /// Short facts about where the cursor is and what is part-typed, drawn
    /// at the bar's right edge in order: `12:5`, `340 lines`, `"a`.
    pub segments: Vec<String>,
}

/// What a tool is asking the host to read from the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptRequest {
    /// Text the input starts out holding, for an answer that is usually an edit
    /// of what is already there.
    pub initial: String,
    /// What is being asked, shown before the input.
    pub label: String,
    /// Whether the answer is typed or a single yes-or-no key.
    pub mode: PromptMode,
    /// Which question this is, so the tool knows what the answer belongs to.
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
    /// A line of text, ending at `Enter`.
    #[default]
    Text,
}

/// What the host should do with a key the tool was offered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ToolOutcome {
    /// Close the tool, and the pane holding it.
    Close,
    /// The tool acted on the key.
    Consumed,
    /// The tool has no binding for it, so resolve it as an ordinary key.
    Ignored,
    /// The tool asks the host to open this path with the system handler.
    OpenExternal(PathBuf),
    /// The tool asks the host to stop the work it started.
    CancelJobs,
    /// The tool asks the host to do slow work for it.
    Job(JobRequest),
    /// The tool asks the host to read an answer from the user.
    Prompt(PromptRequest),
    /// The tool asks the host to have the user choose from a list, rather
    /// than type. The answer comes back as a [`PromptReply`] under the same
    /// tag, so a question asked either way is answered in one place.
    Pick(PickRequest),
    /// The tool asks the host to run one of its own commands, named the way the
    /// command palette names it.
    RunAction(String),
    /// The tool asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The tool asks the host to copy this to the clipboard.
    Yank(String),
    /// The tool asks the host to open this file for editing, in the editor
    /// the app carries.
    OpenPath(OpenTarget),
    /// The tool asks the host to open this file in `$EDITOR`, in a pane of
    /// its own, for the editing the app's own editor deliberately cannot do.
    SpawnEditor(OpenTarget),
    /// The tool asks the host for what is on the clipboard, which comes back
    /// through [`Tool::on_paste`].
    Paste,
}

/// One entry a tool offers in the menu opened over its own rows.
///
/// An entry names a key the tool already binds, rather than a command of its
/// own. A menu built that way cannot offer something the keyboard cannot do,
/// cannot drift from what the keys mean, and costs a tool one list rather
/// than a second dispatch path beside `on_key`.
pub struct ToolMenuItem {
    /// The key choosing this entry stands for.
    pub key: Key,
    /// What the host shows for it.
    pub label: String,
}

/// A WebView a tool owns, covering the pane below whatever rows the tool
/// paints itself.
///
/// A tool returns one when its content is something Winter cannot draw as
/// styled text: a rendered PDF tool, for instance. The host gives it a real
/// web engine over the pane's pixels, and goes on painting the tool's own
/// rows above it, so the header and the key hints stay in the terminal's font
/// and theme while the document is drawn by the engine.
pub struct ToolSurface {
    /// Resolves a path under the surface's own asset root to the bytes to
    /// serve for it. A plain `fn` rather than a closure so the host can hand
    /// it to a protocol handler that outlives this call.
    pub assets: fn(&str) -> Option<SurfaceAsset>,
    /// A file the surface is allowed to read, served to it as the document.
    /// Nothing else on disk is reachable from inside the surface.
    pub document: PathBuf,
    /// The document to load, as a path under the surface's asset root.
    pub entry: String,
    /// The first pane row the surface covers. Rows above it are painted as
    /// ordinary tool rows, which is how a tool keeps a native header over a
    /// surface it does not draw.
    pub top_row: usize,
}

/// One asset a [`ToolSurface`] serves to its own WebView.
pub struct SurfaceAsset {
    /// The file's contents.
    pub bytes: Vec<u8>,
    /// The `Content-Type` to serve it under. A web engine refuses to run a
    /// module script sent as anything but a JavaScript type.
    pub mime: &'static str,
}

/// A file a tool wants opened, and where in it to land.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenTarget {
    /// The line to put the cursor on, when the tool knows one.
    pub line: Option<usize>,
    /// The file to open.
    pub path: PathBuf,
}

// ========================================================================
// Functions
// ========================================================================

/// The index of the next label holding `query`, ignoring case, looking one past
/// `from` and wrapping once so the row under the cursor is tried last. Matching
/// is literal text, which keeps a tool's search free of a pattern engine.
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
    let needle = query.to_lowercase();
    (1..=total)
        .map(|step| {
            if forward {
                (from + step) % total
            } else {
                (from + total - step) % total
            }
        })
        .find(|&index| labels[index].as_ref().to_lowercase().contains(&needle))
}

// ========================================================================
// Traits
// ========================================================================

/// What a tool tab holds. A tool paints itself as rows of styled text and
/// gets first refusal on keys while its tab is focused.
pub trait Tool {
    /// A short label for the pane title.
    fn title(&self) -> String;

    /// The tool is being closed and put by for reopening, not dropped. A tool
    /// that something else is blocked on lets go of it here, since a tool kept
    /// alive would otherwise leave that waiting for as long as the window
    /// lives. What it held is not there when the tool comes back.
    fn on_closed(&mut self) {}

    /// Where the title bar says this tool is: one fixed location, written
    /// with `~` for home, that no cursor movement, line, or tool changes.
    /// `None` for a tool tied to nowhere on disk, which the bar then names by
    /// its kind alone.
    fn address(&self) -> Option<String> {
        None
    }

    /// The rows to draw this frame, given how many the pane can show below any
    /// rows it pins, and how wide the pane is: a tool longer than its pane
    /// returns the window it wants visible, so scrolling stays where the
    /// cursor is, and when `wrap` is set a row wider than `cols` wraps onto
    /// the next screen row — folding at its last word boundary — rather than
    /// clipping at the pane's edge.
    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame;

    /// Offer a key to the tool.
    fn on_key(&mut self, key: &Key) -> ToolOutcome;

    /// Offer a press or a drag of the pointer to the tool. Tools that take no
    /// pointer never implement it, and the click falls through to the host.
    fn on_mouse(&mut self, _at: Point) -> ToolOutcome {
        ToolOutcome::Ignored
    }

    /// Offer a turn of the wheel to the tool, in rows, positive upwards.
    /// Tools that do not scroll themselves never implement it.
    fn on_scroll(&mut self, _lines: isize) -> ToolOutcome {
        ToolOutcome::Ignored
    }

    /// Hand back what the clipboard holds, for a tool that asked for it.
    fn on_paste(&mut self, _text: String) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    /// Offer a file to the tool, for a tool that can show more than one.
    /// `false`, the default, means the host opens it however it would have.
    fn open_file(&mut self, _target: OpenTarget) -> bool {
        false
    }

    /// What the tool is waiting for, when it is part-way through a command
    /// that takes more than one key: the sequence's name and what the next
    /// key may be. The host draws it the way it draws its own key hints, so a
    /// multi-key command looks the same wherever it is being typed. Tools
    /// with no multi-key commands never implement it.
    fn hint(&self) -> Option<ToolHint> {
        None
    }

    /// Hand back the answer to a prompt the tool asked for. Tools that never
    /// ask never implement it.
    fn on_prompt(&mut self, _reply: PromptReply) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    /// Hand back intermediate, debounced changes to what has been typed into an
    /// open prompt, allowing tools to perform iterative search or live filtering.
    fn on_prompt_input(&mut self, _query: &str, _tag: &'static str) -> ToolOutcome {
        ToolOutcome::Ignored
    }

    /// Hand back the result of work the tool asked for.
    fn on_job(&mut self, _reply: JobReply) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    /// When the tool next wants [`Tool::on_tick`], for a tool that keeps
    /// itself current without being asked. The host wakes for the earliest
    /// deadline of any tool showing. Tools whose content changes only on a key
    /// never implement it.
    fn next_tick(&self) -> Option<Instant> {
        None
    }

    /// The deadline from [`Tool::next_tick`] has passed: refresh whatever the
    /// tool shows, usually by asking for a job.
    fn on_tick(&mut self, _now: Instant) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    /// What the tool has to report since it was last asked, for the status
    /// bar: the one way a tool reports. The host asks after every call it
    /// makes into the tool, so a report made alongside some other outcome,
    /// such as a job, still shows, and a tool never draws a report itself.
    fn take_notice(&mut self) -> Option<ToolNotice> {
        None
    }

    /// What the status bar shows for the tool while it is focused: its mode
    /// and where its cursor is. A tool with neither shows nothing there.
    fn status(&self) -> ToolStatus {
        ToolStatus::default()
    }

    /// Where the tool's own caret is on its cursor line, for a tool that puts
    /// one somewhere other than the first thing it painted. A tool whose
    /// cursor is a whole row reports none.
    fn caret(&self) -> Option<Caret> {
        None
    }

    /// Where a tool or shell opened from this tool starts: the directory it
    /// is looking at, following the cursor rather than its root. Tools tied
    /// to nowhere on disk never implement it.
    fn cwd(&self) -> Option<PathBuf> {
        None
    }

    /// Where the tool is looking, for opening it there again after a restart:
    /// the directory a tool is rooted in or the file an editor has open, and
    /// the line its cursor is on. `None` for a tool that cannot be reopened
    /// from a place alone.
    fn snapshot(&self) -> Option<OpenTarget> {
        None
    }

    /// Hear which directories the other panes on screen are looking at, just
    /// before a key arrives, for a tool that offers them as a destination
    /// (a listing copying into the listing beside it).
    fn set_neighbors(&mut self, _dirs: Vec<PathBuf>) {}

    /// The base directory this tool is browsing or rooted in. Unlike `cwd()`,
    /// this does not track cursor movements across entries inside the tool.
    fn root_dir(&self) -> Option<PathBuf> {
        self.cwd()
    }

    /// A file or directory reference for the current view, formatted for copying.
    /// Tools with a more specific location (like an active selection or cursor line)
    /// override this.
    fn file_reference(&self) -> Option<String> {
        self.cwd().map(|p| crate::model::path::abbreviate_home(&p))
    }

    /// Whether the tool is holding work that would be lost with it.
    ///
    /// The host asks before closing a pane or a tab over one, keeps it out of
    /// the reopen stash's way when that fills up, and marks it in the list of
    /// what was closed. Tools holding nothing a reader could lose never
    /// implement it.
    fn is_dirty(&self) -> bool {
        false
    }

    /// Whether this tool is editing a commit message.
    fn is_commit_editor(&self) -> bool {
        false
    }

    /// The tool is showing again after another one was closed over the top of
    /// it. What it was showing may have changed while it was covered, so a
    /// tool that reads the world re-reads it here. Tools showing something
    /// that cannot go stale never implement it.
    fn on_resume(&mut self) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    /// The WebView this tool owns, if its content is something Winter cannot
    /// paint as text. Tools that draw themselves in rows never implement it.
    fn surface(&self) -> Option<ToolSurface> {
        None
    }

    /// Script the tool wants run inside its surface, taken and cleared: this
    /// is how a key the tool bound reaches the document the engine is
    /// drawing. Tools with no surface never implement it.
    fn take_surface_script(&mut self) -> Option<String> {
        None
    }

    /// Hand back what the tool's surface posted out of the engine, which is
    /// how a surface reports state (the tool it scrolled to, how many there
    /// are) that only it knows.
    fn on_surface_message(&mut self, _message: String) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    /// What to offer in a menu opened over the tool's rows, for the row the
    /// cursor is on: the host puts the cursor under the pointer before
    /// asking, so a tool answers for what was clicked.
    ///
    /// An empty list, the default, means the tool offers no menu and the
    /// click does nothing. Tools whose rows all mean the same thing never
    /// implement it, and neither does a tool drawn by a [`ToolSurface`]: the
    /// child WebView takes the click before the window sees it, and the menu
    /// is painted by the GPU underneath a native view that is always on top
    /// of it.
    fn context_items(&self) -> Vec<ToolMenuItem> {
        Vec::new()
    }

    /// Jump targets in the current view: pairs of `(target_id, display_label)`.
    /// `target_id` is an internal index (e.g. line or row) understood by
    /// [`jump_to`] and [`jump_confirm`].
    fn jump_targets(&self) -> Option<Vec<(usize, String)>> {
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
// ToolMenuItem
// ========================================================================

impl ToolMenuItem {
    /// An entry called `label`, run by `key`.
    pub fn new(key: Key, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_search_looks_past_the_cursor_and_wraps() {
        let labels = ["main.rs", "lib.rs", "mod.rs"];
        assert_eq!(find_match(&labels, "mod", 0, true), Some(2));
        assert_eq!(find_match(&labels, "main", 2, true), Some(0));
        assert_eq!(find_match(&labels, "mod", 0, false), Some(2));
        assert_eq!(find_match(&labels, "lib", 0, false), Some(1));
    }

    #[test]
    fn test_a_search_ignores_case_on_both_sides() {
        assert_eq!(find_match(&["Cargo.toml"], "cargo", 0, true), Some(0));
        assert_eq!(find_match(&["cargo.toml"], "CARGO", 0, true), Some(0));
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

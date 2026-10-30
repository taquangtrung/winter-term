//! Compile: a build run in a tab of its own, its output streamed in as it
//! comes, the places it points at made into links, the way Emacs'
//! compilation mode reads a build.
//!
//! The tab reads its build in four parts, each its own module:
//!
//! - [`runner`] runs the command under a hidden pty and hands its output
//!   over as it is written;
//! - [`transcript`] plays the terminal moves in that output out into the
//!   lines it means;
//! - [`locate`] finds the `path:line:col` each finished line points at;
//! - [`tex`] reads a LaTeX document's log once its build is done, for the
//!   warnings TeX writes there and not to the terminal.
//!
//! [`task`] says what can be built in a project and how. The tab runs one
//! task; the host decides which tab a build goes to, saves the files being
//! edited before one starts, and walks the newest build's places with `F8`.

pub mod locate;
pub mod runner;
pub mod task;
pub mod tex;
pub mod transcript;

use std::fs;
use std::mem;
use std::ops::Range;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::editors::background::{Background, POLL};
use crate::editors::list::{RowSearch, Viewport};
use crate::model::editor::{
    menu_groups, Editor, EditorMenuItem, EditorOutcome,
    EditorStatus, Notice, OpenTarget, PromptMode, PromptReply, PromptRequest, SearchReport,
};
use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::{CursorMove, Key};
use crate::model::paint::{
    row_height, row_text, DiagnosticLevel, Frame, Point, Row, Span, Style, TerminalStyle,
};
use crate::model::vim::nav::{VimKey, VimNav};

use self::locate::{Location, Locator};
use self::runner::{Event, ExitStatus, Runner};
use self::task::Task;
use self::tex::{TexMessage, TEX_ENVIRONMENT};
use self::transcript::Transcript;

// ========================================================================
// Constants
// ========================================================================

/// The command that runs the build again as it last ran.
pub const RERUN_COMMAND: &str = "compile_rerun";

/// The command that opens the next place the output points at: what `F8`
/// asks of the tab.
pub const NEXT_PLACE_COMMAND: &str = "next_error";

/// The command that opens the previous place, `S-F8`'s.
pub const PREVIOUS_PLACE_COMMAND: &str = "prev_error";

/// The editor's questions.
const ASK_COMMAND: &str = "compile-command";
const ASK_INPUT: &str = "compile-input";
const ASK_SEARCH: &str = "compile-search";

/// What the title says before the build's name, and how much of the name
/// it keeps.
const TITLE: &str = "Compile";
const TITLE_ROOM: usize = 40;

/// The rows above the output: the command and where it runs.
const HEADER_ROWS: usize = 1;

/// The rows after a build's output once it has ended: a blank, then how it
/// ended.
const FOOTER_ROWS: usize = 2;

/// The rows above the log's messages: a blank, then their heading.
const LOG_HEADING_ROWS: usize = 2;

/// How many of the output's last lines are read for the log TeX names.
const LOG_SEARCH_LINES: usize = 400;

/// What ends a line typed for the build: the key a terminal sends for
/// Enter.
const ENTER: &[u8] = b"\r";

/// The editor's own keys, ahead of the motions every list shares.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Open,
        keys: &["RET"],
        label: "Open This Place Beside",
    },
    Binding {
        action: Act::OpenExternal,
        keys: &["C-o"],
        label: "Open This Place in $EDITOR",
    },
    Binding {
        action: Act::Yank,
        keys: &["y"],
        label: "Copy path:line",
    },
    // `M-n` and `M-p` step through a list's groups everywhere, and through
    // the places here, as they do in Emacs' compilation mode; `n` and `N`
    // stay the search's.
    Binding {
        action: Act::NextPlace,
        keys: &["M-n"],
        label: "Next Error or Warning",
    },
    Binding {
        action: Act::PreviousPlace,
        keys: &["M-p"],
        label: "Previous Error or Warning",
    },
    Binding {
        action: Act::ErrorsOnly,
        keys: &["e"],
        label: "Show Only Errors and Warnings, or Everything",
    },
    // Rerunning is `C-r` in every editor.
    Binding {
        action: Act::Rerun,
        keys: &["C-r"],
        label: "Run the Build Again",
    },
    Binding {
        action: Act::AskCommand,
        keys: &["c"],
        label: "Run Another Command Here...",
    },
    Binding {
        action: Act::Input,
        keys: &["i"],
        label: "Type a Line to the Build...",
    },
    // `C-g` stops the running work of any editor tab.
    Binding {
        action: Act::Stop,
        keys: &["C-g"],
        label: "Stop the Build",
    },
    // AUCTeX's chord for viewing the document.
    Binding {
        action: Act::ShowPdf,
        keys: &["C-c C-v"],
        label: "Open the PDF",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Find",
    },
    Binding {
        action: Act::SearchStep(true),
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Act::SearchStep(false),
        keys: &["N"],
        label: "Previous Match",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    AskCommand,
    ErrorsOnly,
    Input,
    NextPlace,
    Open,
    OpenExternal,
    PreviousPlace,
    Rerun,
    Search,
    /// The search again, forward when set.
    SearchStep(bool),
    ShowPdf,
    Stop,
    Yank,
}

/// A build, and everything it has written so far.
pub struct CompileEditor {
    /// The view row the cursor is on.
    cursor: usize,
    /// Whether only the rows naming places are shown.
    errors_only: bool,
    /// Whether the cursor rides the end of the output as it grows.
    follow: bool,
    /// The places the output points at, in the order of the rows they are
    /// on.
    found: Vec<Found>,
    /// Whether a build that fails opens its first error, and whether this
    /// one has yet.
    jump: Jump,
    locator: Locator,
    /// The document's log being read, once its build has ended.
    log: Option<Background<Vec<TexMessage>>>,
    /// What the document's log reported.
    log_messages: Vec<TexMessage>,
    nav: VimNav,
    notice: Option<Notice>,
    /// The whole-output index of the next line to look for a place in.
    parsed: usize,
    /// The first key of a sequence, while it waits for the second.
    pending: Option<Key>,
    phase: Phase,
    /// The project the build belongs to.
    root: PathBuf,
    runner: Option<Runner>,
    /// The last text searched for, and the search the host is still to take.
    search: RowSearch,
    /// The rows shown while only places are, by row index, in order.
    shown: Vec<usize>,
    /// How many places, rows, and let-go lines there were when `shown` was
    /// worked out, so it is worked out again only once one changes.
    shown_for: [usize; 3],
    /// Whether the build is to start on the next tick, once the host has
    /// saved the files being edited.
    starting: bool,
    /// Whether the build was asked to stop, so its end reads as stopped.
    stopping: bool,
    task: Task,
    transcript: Transcript,
    view: Viewport,
    /// The place the last step through them opened.
    walked: Option<usize>,
}

/// What one tab's view of the build keeps while another tab on it is the
/// one acted on: the row the cursor is on, whether it rides the end, and
/// the window. The output, its places, and which rows are shown are the
/// build's, so every tab on it shares them.
#[derive(Clone, Debug)]
struct CompileView {
    cursor: usize,
    follow: bool,
    view: Viewport,
}

/// Opening a failed build's first error by itself.
#[derive(Clone, Copy, Debug)]
struct Jump {
    /// Whether the settings ask for it.
    enabled: bool,
    /// Whether this build's has been opened, by itself or by a step.
    spent: bool,
}

/// Where the build has got to.
#[derive(Debug)]
enum Phase {
    /// It has ended.
    Done(Done),
    /// Nothing has run yet.
    Idle,
    /// It is running.
    Running(Running),
}

/// A build that has ended, and how.
#[derive(Debug)]
struct Done {
    elapsed: Duration,
    end: End,
}

/// A build still running.
#[derive(Debug)]
struct Running {
    started: Instant,
}

/// How a build ended.
#[derive(Debug)]
enum End {
    /// It ran to its end.
    Exited(ExitStatus),
    /// It never started.
    Failed(String),
    /// It was stopped.
    Stopped,
}

/// One place the output points at, and the row it is on.
#[derive(Clone, Debug)]
struct Found {
    location: Location,
    origin: Origin,
}

/// Which row a place is on. Ordered as the rows are, the output's before
/// the log's, which is why the variants are not alphabetical.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum Origin {
    /// One of the build's own lines, by its whole-output index.
    Output(usize),
    /// One of the log's messages, listed after the output.
    Log(usize),
}

/// What a row of the tab shows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RowKind {
    /// The space between the output and what follows it.
    Blank,
    /// How the build ended.
    Footer,
    /// The command, and where it runs.
    Header,
    /// One of the log's messages.
    Log(usize),
    /// The heading over the log's messages.
    LogHeading,
    /// One of the build's lines, by its index in the transcript.
    Output(usize),
}

/// The place a painted line points at.
#[derive(Clone, Copy, Debug)]
struct Painted<'a> {
    /// Whether it is the place the last step opened.
    current: bool,
    level: DiagnosticLevel,
    span: &'a Range<usize>,
}

// ========================================================================
// CompileEditor
// ========================================================================

impl CompileEditor {
    // --------------------------------------------------------------------
    // Building
    // --------------------------------------------------------------------

    /// A tab for builds of `task` in the project at `root`. A failed build
    /// opens its first error by itself when `jump_to_error` is set. Nothing
    /// runs until asked.
    pub fn new(root: PathBuf, task: Task, jump_to_error: bool) -> Self {
        Self {
            cursor: 0,
            errors_only: false,
            follow: true,
            found: Vec::new(),
            jump: Jump {
                enabled: jump_to_error,
                spent: false,
            },
            locator: Locator::new(task.dir.clone()),
            log: None,
            log_messages: Vec::new(),
            nav: VimNav::new(),
            notice: None,
            parsed: 0,
            pending: None,
            phase: Phase::Idle,
            root,
            runner: None,
            search: RowSearch::default(),
            shown: Vec::new(),
            shown_for: [0; 3],
            starting: false,
            stopping: false,
            task,
            transcript: Transcript::new(),
            view: Viewport::new(),
            walked: None,
        }
    }

    /// Ask for the build to start afresh, stopping one still running. It
    /// starts on the next tick: the host first saves what is being edited.
    fn request_start(&mut self) -> EditorOutcome {
        if let Some(mut running) = self.runner.take() {
            running.stop();
        }
        self.starting = true;
        EditorOutcome::Build(self.task.build_start(&self.root))
    }

    /// Start the build now.
    fn start(&mut self) {
        self.starting = false;
        self.transcript = Transcript::new();
        self.locator = Locator::new(self.task.dir.clone());
        self.found.clear();
        self.log = None;
        self.log_messages.clear();
        self.parsed = 0;
        self.stopping = false;
        self.walked = None;
        self.jump.spent = false;
        self.follow = true;
        let started = Instant::now();
        let (program, args) = self.task.argv();
        let mut env = self.task.env.clone();
        if self.task.document.is_some() {
            env.extend(
                TEX_ENVIRONMENT
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string())),
            );
        }
        self.phase = match Runner::start(&program, &args, &self.task.dir, &env) {
            Ok(runner) => {
                self.runner = Some(runner);
                Phase::Running(Running { started })
            }
            Err(reason) => Phase::Done(Done {
                elapsed: Duration::ZERO,
                end: End::Failed(reason),
            }),
        };
        self.sync_rows();
    }

    /// Ask the build to stop.
    fn stop(&mut self) -> EditorOutcome {
        match self.runner.as_mut() {
            Some(runner) => {
                self.stopping = true;
                runner.stop();
                self.notice = Some(Notice::info("stopping the build"));
            }
            None => self.notice = Some(Notice::info("no build is running")),
        }
        EditorOutcome::Consumed
    }

    /// Take in what the build has sent, and the document's log once read.
    /// A failed build's first error, once known, is what comes back.
    fn take_progress(&mut self) -> EditorOutcome {
        let events = self
            .runner
            .as_ref()
            .map(Runner::take_events)
            .unwrap_or_default();
        for event in events {
            match event {
                Event::Output(bytes) => self.transcript.feed(&bytes),
                Event::Exited(status) => self.finish(status),
            }
        }
        self.read_places(self.transcript.dropped() + self.transcript.finished());
        if let Some(messages) = self.log.as_ref().and_then(Background::poll) {
            self.log = None;
            self.take_log(messages);
            self.report_end();
        }
        self.sync_rows();
        self.jump_to_first_error()
    }

    /// The build has exited: settle how it ended, read the line it left
    /// unfinished, and read the document's log.
    fn finish(&mut self, status: ExitStatus) {
        self.runner = None;
        let elapsed = match &self.phase {
            Phase::Running(running) => running.started.elapsed(),
            Phase::Done(_) | Phase::Idle => Duration::ZERO,
        };
        let end = match self.stopping {
            true => End::Stopped,
            false => End::Exited(status),
        };
        self.phase = Phase::Done(Done { elapsed, end });
        self.read_places(self.transcript.dropped() + self.transcript.len());
        let Some(document) = self.task.document.clone() else {
            self.report_end();
            return;
        };
        let last = self.transcript.len();
        let lines: Vec<String> = (last.saturating_sub(LOG_SEARCH_LINES)..last)
            .map(|index| self.transcript.text(index))
            .collect();
        let log = tex::log_path(lines.iter().map(String::as_str), &self.task.dir, &document);
        self.log = Some(Background::start("winter compile log", move |_| {
            fs::read_to_string(&log)
                .map(|text| tex::parse_log(&text))
                .unwrap_or_default()
        }));
    }

    /// Say how the build ended, wherever the reader is: a failure as an
    /// error, which the window raises as a toast.
    fn report_end(&mut self) {
        let Phase::Done(done) = &self.phase else {
            return;
        };
        let (errors, warnings) = self.counts();
        let counts = counted(errors, warnings);
        let name = &self.task.name;
        self.notice = Some(match &done.end {
            End::Exited(status) if status.code == Some(0) => {
                Notice::info(format!("{name}: finished ({counts})"))
            }
            End::Exited(_) | End::Failed(_) => {
                Notice::error(format!("{name}: {} ({counts})", ended(done)))
            }
            End::Stopped => Notice::info(format!("{name}: stopped")),
        });
    }

    /// Open a failed build's first error, once, where the settings ask for
    /// it: what Emacs' `compilation-auto-jump-to-first-error` does.
    fn jump_to_first_error(&mut self) -> EditorOutcome {
        let failed = match &self.phase {
            Phase::Done(done) => match &done.end {
                End::Exited(status) => status.code != Some(0),
                End::Failed(_) | End::Stopped => false,
            },
            Phase::Idle | Phase::Running(_) => false,
        };
        if !self.jump.enabled || self.jump.spent || !failed || self.log.is_some() {
            return EditorOutcome::Consumed;
        }
        self.jump.spent = true;
        let first = self
            .found
            .iter()
            .position(|found| found.location.level == DiagnosticLevel::Error);
        match first {
            Some(first) => self.open_place(first),
            None => EditorOutcome::Consumed,
        }
    }

    /// Read the places the output's lines up to whole-output index `upto`
    /// point at, each line once.
    fn read_places(&mut self, upto: usize) {
        let dropped = self.transcript.dropped();
        // Lines let go before they were read are not read.
        self.parsed = self.parsed.max(dropped);
        self.found.retain(|found| match found.origin {
            Origin::Output(at) => at >= dropped,
            Origin::Log(_) => true,
        });
        while self.parsed < upto {
            let text = self.transcript.text(self.parsed - dropped);
            if let Some(location) = self.locator.scan(self.parsed, &text) {
                self.found.push(Found {
                    location,
                    origin: Origin::Output(self.parsed),
                });
            }
            self.parsed += 1;
        }
    }

    /// Take in the log's messages, leaving out the errors the output has
    /// already placed.
    fn take_log(&mut self, messages: Vec<TexMessage>) {
        let placed: Vec<(PathBuf, usize)> = self
            .found
            .iter()
            .map(|found| (normalized(&found.location.file), found.location.line))
            .collect();
        self.log_messages = messages
            .into_iter()
            .filter(|message| {
                message.level != DiagnosticLevel::Error
                    || message
                        .line
                        .is_none_or(|line| !placed.contains(&(normalized(&message.file), line)))
            })
            .collect();
        for (index, message) in self.log_messages.iter().enumerate() {
            let Some(line) = message.line else {
                continue;
            };
            let reference = format!("{}:{line}", message.file.display());
            self.found.push(Found {
                location: Location {
                    column: None,
                    dir: self.task.dir.clone(),
                    file: message.file.clone(),
                    heading: None,
                    level: message.level,
                    line,
                    message: message.message.clone(),
                    span: 0..reference.len(),
                },
                origin: Origin::Log(index),
            });
        }
    }

    // --------------------------------------------------------------------
    // Rows
    // --------------------------------------------------------------------

    /// How many rows follow the output: none while the build runs, then
    /// how it ended, then the log's messages.
    fn trailer_rows(&self) -> usize {
        match self.phase {
            Phase::Done(_) => {
                let log = match self.log_messages.is_empty() {
                    true => 0,
                    false => LOG_HEADING_ROWS + self.log_messages.len(),
                };
                FOOTER_ROWS + log
            }
            Phase::Idle | Phase::Running(_) => 0,
        }
    }

    /// How many of the build's lines show: all of them but an empty last
    /// one, which is only where the next would start.
    fn output_rows(&self) -> usize {
        let len = self.transcript.len();
        let last_is_empty = len
            .checked_sub(1)
            .is_some_and(|last| self.transcript.text(last).is_empty());
        match last_is_empty {
            true => len - 1,
            false => len,
        }
    }

    /// How many rows there are, shown or not.
    fn total_rows(&self) -> usize {
        HEADER_ROWS + self.output_rows() + self.trailer_rows()
    }

    /// How many rows show.
    fn view_len(&self) -> usize {
        match self.errors_only {
            true => self.shown.len(),
            false => self.total_rows(),
        }
    }

    /// The row view row `at` shows.
    fn row_at(&self, at: usize) -> usize {
        match self.errors_only {
            true => self.shown.get(at).copied().unwrap_or(0),
            false => at,
        }
    }

    /// The view row showing row `row`, or the nearest one after it while
    /// only places are shown.
    fn view_of(&self, row: usize) -> usize {
        match self.errors_only {
            true => self.shown.partition_point(|shown| *shown < row),
            false => row,
        }
    }

    /// What row `row` shows.
    fn row_kind(&self, row: usize) -> RowKind {
        if row < HEADER_ROWS {
            return RowKind::Header;
        }
        let output = row - HEADER_ROWS;
        if output < self.output_rows() {
            return RowKind::Output(output);
        }
        let after = output - self.output_rows();
        let log = FOOTER_ROWS + LOG_HEADING_ROWS;
        if after >= log {
            return RowKind::Log(after - log);
        }
        if after == FOOTER_ROWS - 1 {
            return RowKind::Footer;
        }
        if after == log - 1 {
            return RowKind::LogHeading;
        }
        RowKind::Blank
    }

    /// The row a whole-output line is on, while it is still kept.
    fn row_of_line(&self, line: usize) -> Option<usize> {
        line.checked_sub(self.transcript.dropped())
            .map(|index| HEADER_ROWS + index)
    }

    /// The row a place is on, while it is still kept.
    fn row_of(&self, origin: Origin) -> Option<usize> {
        match origin {
            Origin::Output(at) => self.row_of_line(at),
            Origin::Log(index) => {
                Some(HEADER_ROWS + self.output_rows() + FOOTER_ROWS + LOG_HEADING_ROWS + index)
            }
        }
    }

    /// The place on row `row`, as an index into the places found.
    fn place_on(&self, row: usize) -> Option<usize> {
        let origin = match self.row_kind(row) {
            RowKind::Output(index) => Origin::Output(index + self.transcript.dropped()),
            RowKind::Log(index) => Origin::Log(index),
            RowKind::Blank | RowKind::Footer | RowKind::Header | RowKind::LogHeading => {
                return None
            }
        };
        let at = self.found.partition_point(|found| found.origin < origin);
        self.found
            .get(at)
            .filter(|found| found.origin == origin)
            .map(|_| at)
    }

    /// The rows shown while only places are: the command, each place with
    /// the message line over it, and how the build ended.
    fn place_rows(&self) -> Vec<usize> {
        let mut rows: Vec<usize> = (0..HEADER_ROWS).collect();
        for found in &self.found {
            let heading = found
                .location
                .heading
                .and_then(|line| self.row_of_line(line));
            rows.extend(heading);
            rows.extend(self.row_of(found.origin));
        }
        let trailer = HEADER_ROWS + self.output_rows();
        rows.extend(trailer..self.total_rows());
        rows.sort_unstable();
        rows.dedup();
        rows
    }

    /// Bring the rows shown up to date, keeping the cursor on the end when
    /// it rides it.
    fn sync_rows(&mut self) {
        let now = self.shown_key();
        if self.errors_only && now != self.shown_for {
            self.shown = self.place_rows();
            self.shown_for = now;
        }
        let last = self.view_len().saturating_sub(1);
        self.cursor = match self.follow {
            true => last,
            false => self.cursor.min(last),
        };
    }

    /// What `shown` is worked out from.
    fn shown_key(&self) -> [usize; 3] {
        [
            self.found.len(),
            self.total_rows(),
            self.transcript.dropped(),
        ]
    }

    /// Show only the rows naming places, or every row again, keeping the
    /// cursor on the row it was on, or the nearest shown.
    fn toggle_errors_only(&mut self) -> EditorOutcome {
        let row = self.row_at(self.cursor);
        self.errors_only = !self.errors_only;
        self.shown = match self.errors_only {
            true => self.place_rows(),
            false => Vec::new(),
        };
        self.shown_for = self.shown_key();
        self.cursor = self.view_of(row).min(self.view_len().saturating_sub(1));
        self.notice = Some(Notice::info(match self.errors_only {
            true => "showing only errors and warnings",
            false => "showing all the output",
        }));
        EditorOutcome::Consumed
    }

    // --------------------------------------------------------------------
    // Painting
    // --------------------------------------------------------------------

    fn row(&self, row: usize) -> Row {
        match self.row_kind(row) {
            RowKind::Blank => Row::new(),
            RowKind::Header => vec![
                Span::new(Style::Header, self.task.command.clone()),
                Span::new(Style::Dim, format!("  in {}", self.task.dir.display())),
            ],
            RowKind::Output(index) => {
                let runs = self.transcript.runs(index);
                let place = self.place_on(row).map(|at| Painted {
                    current: self.walked == Some(at),
                    level: self.found[at].location.level,
                    span: &self.found[at].location.span,
                });
                output_row(&runs, place)
            }
            RowKind::Footer => self.footer(),
            RowKind::LogHeading => vec![Span::new(Style::Header, self.log_heading())],
            RowKind::Log(index) => self.log_row(row, index),
        }
    }

    /// Row `row`'s text, for the search to read: the output's own text,
    /// which is what the eye reads, without painting it.
    fn row_label(&self, row: usize) -> String {
        match self.row_kind(row) {
            RowKind::Output(index) => self.transcript.text(index),
            RowKind::Blank
            | RowKind::Footer
            | RowKind::Header
            | RowKind::Log(_)
            | RowKind::LogHeading => row_text(&self.row(row)),
        }
    }

    /// How the build ended, in its hue.
    fn footer(&self) -> Row {
        let Phase::Done(done) = &self.phase else {
            return Row::new();
        };
        let style = match &done.end {
            End::Exited(status) if status.code == Some(0) => Style::Passed,
            End::Exited(_) | End::Failed(_) => Style::Failed,
            End::Stopped => Style::Dim,
        };
        let (errors, warnings) = self.counts();
        vec![
            Span::new(style, format!("Build {}", ended(done))),
            Span::new(Style::Dim, format!("  ({})", counted(errors, warnings))),
        ]
    }

    fn log_heading(&self) -> String {
        let count = |level| {
            self.log_messages
                .iter()
                .filter(|message| message.level == level)
                .count()
        };
        format!(
            "LaTeX log: {}, {}, {}",
            plural(count(DiagnosticLevel::Error), "error", "errors"),
            plural(count(DiagnosticLevel::Warning), "warning", "warnings"),
            plural(count(DiagnosticLevel::Info), "bad box", "bad boxes"),
        )
    }

    /// One of the log's messages, on row `row`: its place, how bad it is,
    /// and what it says.
    fn log_row(&self, row: usize, index: usize) -> Row {
        let Some(message) = self.log_messages.get(index) else {
            return Row::new();
        };
        let kind = match message.level {
            DiagnosticLevel::Error => "error",
            DiagnosticLevel::Info => "bad box",
            DiagnosticLevel::Warning => "warning",
        };
        let place = match message.line {
            Some(line) => {
                let span = Span::new(
                    Style::Diagnostic(message.level),
                    format!("{}:{line}", message.file.display()),
                );
                let current = self.walked.is_some() && self.place_on(row) == self.walked;
                match current {
                    true => span.selected(),
                    false => span,
                }
            }
            None => Span::new(Style::Dim, message.file.display().to_string()),
        };
        vec![place, Span::plain(format!(": {kind}: {}", message.message))]
    }

    /// How many errors and warnings the build has pointed at.
    fn counts(&self) -> (usize, usize) {
        let count = |level| {
            self.found
                .iter()
                .filter(|found| found.location.level == level)
                .count()
        };
        (
            count(DiagnosticLevel::Error),
            count(DiagnosticLevel::Warning),
        )
    }

    // --------------------------------------------------------------------
    // Cursor
    // --------------------------------------------------------------------

    /// Put the cursor on view row `at`, which stops it riding the end.
    fn set_cursor(&mut self, at: usize) {
        self.cursor = at.min(self.view_len().saturating_sub(1));
        self.follow = false;
    }

    /// Carry out one of the motions a list of rows takes. `false` for the
    /// ones that mean nothing over rows (words, columns).
    fn apply_motion(&mut self, motion: CursorMove) -> bool {
        let last = self.view_len().saturating_sub(1);
        let rows = self.view.rows();
        let half = (rows / 2).max(1);
        let page = rows.max(1);
        let scroll = self.view.scroll();
        let at = self.cursor;
        let target = match motion {
            CursorMove::Down => at + 1,
            CursorMove::Up => at.saturating_sub(1),
            CursorMove::Top => 0,
            CursorMove::Bottom => last,
            CursorMove::HalfPageDown => at + half,
            CursorMove::HalfPageUp => at.saturating_sub(half),
            CursorMove::PageDown => at + page,
            CursorMove::PageUp => at.saturating_sub(page),
            CursorMove::ScreenTop => scroll,
            CursorMove::ScreenMiddle => scroll + rows / 2,
            CursorMove::ScreenBottom => scroll + rows.saturating_sub(1),
            // Words, paragraphs, and columns mean nothing over these rows.
            _ => return false,
        };
        self.cursor = target.min(last);
        // Back at the end, the cursor rides it again.
        self.follow = self.cursor == last;
        true
    }

    // --------------------------------------------------------------------
    // Places
    // --------------------------------------------------------------------

    /// Open place `at` beside the tab, moving the cursor onto it.
    fn open_place(&mut self, at: usize) -> EditorOutcome {
        let Some(found) = self.found.get(at) else {
            return EditorOutcome::Consumed;
        };
        if let Some(row) = self.row_of(found.origin) {
            let view = self.view_of(row);
            self.set_cursor(view);
        }
        self.walked = Some(at);
        self.jump.spent = true;
        let location = &self.found[at].location;
        match location.resolve() {
            Some(path) => {
                EditorOutcome::OpenBeside(OpenTarget::at(path, location.line, location.column))
            }
            None => {
                self.notice = Some(Notice::error(format!(
                    "no such file: {}",
                    location.file.display()
                )));
                EditorOutcome::Consumed
            }
        }
    }

    /// The place under the cursor.
    fn place_at_cursor(&self) -> Option<usize> {
        self.place_on(self.row_at(self.cursor))
    }

    /// Move the cursor to the next or previous place.
    fn move_to_place(&mut self, forward: bool) -> EditorOutcome {
        let cursor = self.row_at(self.cursor);
        let mut rows = self
            .found
            .iter()
            .filter_map(|found| self.row_of(found.origin));
        let target = match forward {
            true => rows.find(|row| *row > cursor),
            false => rows.rev().find(|row| *row < cursor),
        };
        match target {
            Some(row) => {
                let view = self.view_of(row);
                self.set_cursor(view);
            }
            None => self.notice = Some(Notice::info("no more errors")),
        }
        EditorOutcome::Consumed
    }

    /// Open the next or previous place after the last one opened, the way
    /// `F8` walks them: from the first again after the last.
    fn step_place(&mut self, forward: bool) -> EditorOutcome {
        let count = self.found.len();
        if count == 0 {
            self.notice = Some(Notice::error(match self.phase {
                Phase::Running(_) => "the build has pointed at nothing yet",
                Phase::Done(_) | Phase::Idle => "the build pointed at nothing",
            }));
            return EditorOutcome::Consumed;
        }
        let at = match (self.walked, forward) {
            (Some(at), true) => (at + 1) % count,
            (Some(at), false) => (at + count - 1) % count,
            (None, true) => 0,
            (None, false) => count - 1,
        };
        let outcome = self.open_place(at);
        if self.notice.is_none() {
            let found = &self.found[at];
            self.notice = Some(Notice::info(format!(
                "{} of {count}: {}",
                at + 1,
                found.location.reference()
            )));
        }
        outcome
    }

    // --------------------------------------------------------------------
    // Questions
    // --------------------------------------------------------------------

    fn ask_command(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: self.task.command.clone(),
            label: "Compile: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_COMMAND,
        })
    }

    fn ask_input(&mut self) -> EditorOutcome {
        if self.runner.is_none() {
            self.notice = Some(Notice::info("no build is running"));
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "Type to the build: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_INPUT,
        })
    }

    /// Type `line` to the build, with the Enter that ends it.
    fn send_line(&mut self, line: &str) -> EditorOutcome {
        let sent = self.runner.as_mut().is_some_and(|runner| {
            let mut bytes = line.as_bytes().to_vec();
            bytes.extend_from_slice(ENTER);
            runner.send(&bytes)
        });
        if !sent {
            self.notice = Some(Notice::error("the build is not taking input"));
        }
        EditorOutcome::Consumed
    }

    /// The labels of the rows shown, for the search.
    fn search_labels(&self) -> Vec<String> {
        (0..self.view_len())
            .map(|at| self.row_label(self.row_at(at)))
            .collect()
    }

    /// Move to the next or previous row holding the last search.
    fn search_step(&mut self, forward: bool) -> EditorOutcome {
        let labels = self.search_labels();
        match self.search.step(&labels, self.cursor, forward) {
            Ok(at) => self.set_cursor(at),
            Err(notice) => self.notice = Some(notice),
        }
        EditorOutcome::Consumed
    }

    fn search_for(&mut self, query: &str) -> EditorOutcome {
        self.search.set(query.trim());
        self.search_step(true)
    }

    // --------------------------------------------------------------------
    // Keys
    // --------------------------------------------------------------------

    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::AskCommand => self.ask_command(),
            Act::ErrorsOnly => self.toggle_errors_only(),
            Act::Input => self.ask_input(),
            Act::NextPlace => self.move_to_place(true),
            Act::Open => match self.place_at_cursor() {
                Some(at) => self.open_place(at),
                None => {
                    self.notice = Some(Notice::info("no error on this line"));
                    EditorOutcome::Consumed
                }
            },
            Act::OpenExternal => {
                let target = self.place_at_cursor().and_then(|at| {
                    let location = &self.found[at].location;
                    location
                        .resolve()
                        .map(|path| OpenTarget::at(path, location.line, location.column))
                });
                match target {
                    Some(target) => EditorOutcome::SpawnEditor(target),
                    None => EditorOutcome::Consumed,
                }
            }
            Act::PreviousPlace => self.move_to_place(false),
            Act::Rerun => self.request_start(),
            Act::Search => EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH)),
            Act::SearchStep(forward) => self.search_step(forward),
            Act::ShowPdf => self.show_pdf(),
            Act::Stop => self.stop(),
            Act::Yank => match self.place_at_cursor() {
                Some(at) => EditorOutcome::Yank(self.found[at].location.reference()),
                None => EditorOutcome::Consumed,
            },
        }
    }

    /// Open the PDF the document's build made.
    fn show_pdf(&mut self) -> EditorOutcome {
        let pdf = self
            .task
            .document
            .as_ref()
            .map(|root| root.with_extension("pdf"));
        match pdf {
            Some(pdf) if pdf.is_file() => EditorOutcome::OpenBeside(OpenTarget::file(pdf)),
            Some(pdf) => {
                self.notice = Some(Notice::error(format!("no PDF yet: {}", pdf.display())));
                EditorOutcome::Consumed
            }
            None => {
                self.notice = Some(Notice::error("this build makes no PDF"));
                EditorOutcome::Consumed
            }
        }
    }

    fn on_motion(&mut self, key: &Key) -> EditorOutcome {
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => match self.apply_motion(motion) {
                true => EditorOutcome::Consumed,
                false => EditorOutcome::Ignored,
            },
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }
}

impl Editor for CompileEditor {
    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(CompileView {
            cursor: self.cursor,
            follow: self.follow,
            view: self.view.clone(),
        }))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        let Ok(saved) = state.downcast::<CompileView>() else {
            return;
        };
        self.cursor = saved.cursor;
        self.follow = saved.follow;
        self.view = saved.view;
        self.sync_rows();
    }

    fn title(&self) -> String {
        let name: String = self.task.name.chars().take(TITLE_ROOM).collect();
        match name.len() < self.task.name.len() {
            true => format!("{TITLE}: {name}\u{2026}"),
            false => format!("{TITLE}: {name}"),
        }
    }

    fn address(&self) -> Option<String> {
        Some(self.task.dir.display().to_string())
    }

    fn cwd(&self) -> Option<PathBuf> {
        Some(self.task.dir.clone())
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn is_busy(&self) -> bool {
        self.starting || self.runner.is_some()
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        self.sync_rows();
        let total = self.view_len();
        self.view.set_rows(rows);
        // The window is lent out while it is placed, so the rows it asks the
        // height of can be painted from the rest of the editor: only those
        // it asks about are, and only when they may fold.
        let mut view = mem::take(&mut self.view);
        let placed = view.place(self.cursor, total, rows, |at| match wrap {
            true => row_height(&row_text(&self.row(self.row_at(at))), cols, wrap, 0),
            false => 1,
        });
        self.view = view;
        let window = placed.window;
        let shown: Vec<Row> = (window.start..(window.start + window.count).min(total))
            .map(|at| self.row(self.row_at(at)))
            .collect();
        placed.apply(Frame::new(shown), 0)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.search.take_report()
    }

    fn status(&self) -> EditorStatus {
        let (errors, warnings) = self.counts();
        let mut segments = vec![counted(errors, warnings)];
        if self.errors_only {
            segments.push("errors only".to_string());
        }
        match &self.phase {
            Phase::Running(running) => {
                segments.push(format!("running {}", seconds(running.started.elapsed())))
            }
            Phase::Done(done) => segments.push(match &done.end {
                End::Exited(status) => match status.code {
                    Some(code) => format!("exit {code}"),
                    None => "killed".to_string(),
                },
                End::Failed(_) => "not started".to_string(),
                End::Stopped => "stopped".to_string(),
            }),
            Phase::Idle => {}
        }
        EditorStatus {
            mode: None,
            segments,
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.view.release();
        if let Some(first) = self.pending.take() {
            return match table::lookup(KEYS, &[first, *key]) {
                TableMatch::Exact(act) => self.run(act),
                TableMatch::Miss | TableMatch::Prefix => EditorOutcome::Consumed,
            };
        }
        if self.nav.in_sequence() {
            return self.on_motion(key);
        }
        match table::lookup(KEYS, std::slice::from_ref(key)) {
            TableMatch::Exact(act) => self.run(act),
            TableMatch::Prefix => {
                self.pending = Some(*key);
                EditorOutcome::Consumed
            }
            TableMatch::Miss => self.on_motion(key),
        }
    }

    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        match name {
            NEXT_PLACE_COMMAND => Some(self.step_place(true)),
            PREVIOUS_PLACE_COMMAND => Some(self.step_place(false)),
            RERUN_COMMAND => Some(self.request_start()),
            _ => None,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.view.set_margin(rows);
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.follow = false;
        self.view.wheel(lines);
        EditorOutcome::Consumed
    }

    /// A press puts the cursor on the row under the pointer.
    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        self.view.release();
        let row = self.view.scroll() + at.row;
        if row < self.view_len() {
            self.set_cursor(row);
        }
        EditorOutcome::Consumed
    }

    /// A double-click opens the place on the row.
    fn on_double_click(&mut self, at: Point) -> EditorOutcome {
        self.on_mouse(at);
        match self.place_at_cursor() {
            Some(place) => self.open_place(place),
            None => EditorOutcome::Consumed,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.view.release();
        match (reply.tag, reply.answer) {
            (ASK_COMMAND, Some(command)) if !command.trim().is_empty() => {
                self.task = self.task.with_command(command.trim().to_string());
                self.request_start()
            }
            (ASK_INPUT, Some(line)) => self.send_line(&line),
            (ASK_SEARCH, Some(query)) => self.search_for(&query),
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_SEARCH && !query.trim().is_empty() {
            return self.search_for(query);
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        if self.starting {
            return Some(Instant::now());
        }
        (self.runner.is_some() || self.log.is_some()).then(|| Instant::now() + POLL)
    }

    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        if self.starting {
            self.start();
        }
        self.take_progress()
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        let entry =
            |act| table::menu_entry(KEYS, act).map(|(key, label)| EditorMenuItem::new(key, label));
        let place: Vec<EditorMenuItem> = match self.place_at_cursor() {
            Some(_) => [Act::Open, Act::OpenExternal, Act::Yank]
                .into_iter()
                .filter_map(entry)
                .collect(),
            None => Vec::new(),
        };
        let running: Vec<EditorMenuItem> = match self.runner.is_some() {
            true => [Act::Input, Act::Stop]
                .into_iter()
                .filter_map(entry)
                .collect(),
            false => Vec::new(),
        };
        let pdf = match self.task.document.is_some() {
            true => table::keys("C-c C-v")
                .map(|keys| EditorMenuItem::typed(keys, "Open the PDF"))
                .into_iter()
                .collect(),
            false => Vec::new(),
        };
        menu_groups([
            place,
            [Act::Rerun, Act::AskCommand, Act::ErrorsOnly]
                .into_iter()
                .filter_map(entry)
                .chain(running)
                .collect(),
            pdf,
        ])
    }
}

// ========================================================================
// Functions
// ========================================================================

/// One line of the build's output as spans: each run in the colors the
/// build gave it, and the place the line points at, if any, in the hue of
/// how bad it is, on the selection's background when it is the current one.
fn output_row(runs: &[(TerminalStyle, String)], place: Option<Painted<'_>>) -> Row {
    let style_of = |style: TerminalStyle| match style == TerminalStyle::default() {
        true => Style::Normal,
        false => Style::Terminal(style),
    };
    let mut row = Row::new();
    let mut offset = 0;
    for (style, text) in runs {
        let (start, end) = (offset, offset + text.len());
        offset = end;
        let Some(place) = place.filter(|place| place.span.start < end && place.span.end > start)
        else {
            row.push(Span::new(style_of(*style), text.clone()));
            continue;
        };
        let cut = |at: usize| at.clamp(start, end) - start;
        let (from, to) = (cut(place.span.start), cut(place.span.end));
        if from > 0 {
            row.push(Span::new(style_of(*style), &text[..from]));
        }
        if to > from {
            let span = Span::new(Style::Diagnostic(place.level), &text[from..to]);
            row.push(match place.current {
                true => span.selected(),
                false => span,
            });
        }
        if to < text.len() {
            row.push(Span::new(style_of(*style), &text[to..]));
        }
    }
    row
}

/// How a build ended, as a phrase following "Build".
fn ended(done: &Done) -> String {
    let took = seconds(done.elapsed);
    match &done.end {
        End::Exited(status) => match (status.code, &status.signal) {
            (Some(0), _) => format!("finished in {took}"),
            (Some(code), _) => format!("failed with exit code {code} after {took}"),
            (None, Some(signal)) => format!("ended by {signal} after {took}"),
            (None, None) => format!("ended after {took}"),
        },
        End::Failed(reason) => format!("did not start: {reason}"),
        End::Stopped => format!("stopped after {took}"),
    }
}

/// `path` without its `./` steps, so the same file named two ways compares
/// equal.
fn normalized(path: &Path) -> PathBuf {
    path.components()
        .filter(|part| !matches!(part, Component::CurDir))
        .collect()
}

/// `count` things, called `one` when there is one and `many` otherwise.
fn plural(count: usize, one: &str, many: &str) -> String {
    match count {
        1 => format!("1 {one}"),
        _ => format!("{count} {many}"),
    }
}

/// The errors and warnings, counted.
fn counted(errors: usize, warnings: usize) -> String {
    format!(
        "{}, {}",
        plural(errors, "error", "errors"),
        plural(warnings, "warning", "warnings")
    )
}

/// A duration as seconds, to a tenth.
fn seconds(elapsed: Duration) -> String {
    format!("{:.1}s", elapsed.as_secs_f64())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

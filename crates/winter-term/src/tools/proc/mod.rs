//! Process monitor: every process on the machine as a tree or a sorted list,
//! kept current on a timer, with the actions of a task manager (terminate,
//! kill, suspend, resume, renice) a key away.
//!
//! - [`act`]: the commands that act on a process.
//! - [`collect`]: taking a snapshot, the one side-effecting module.
//! - [`cpu`]: CPU use worked out from two snapshots.
//! - [`format`]: wording sizes and percentages.
//! - [`parse`]: reading each system's process listing.
//! - [`view`]: the order the list is shown in.

pub mod act;
pub mod collect;
pub mod cpu;
pub mod format;
pub mod parse;
pub mod view;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::model::input::{CursorMove, Key, KeyCode};
use crate::model::page::{
    row_height, row_text, wrap_window, CommandOutput, JobReply, JobRequest, Page, PageContent,
    PageMenuItem, PageOutcome, PagePoint, PageRow, PageSpan, PageStyle, PromptMode, PromptReply,
    PromptRequest,
};
use crate::model::process::{ProcState, ProcessSample};
use crate::model::vim::nav::{VimKey, VimNav};
use crate::tools::refresh::RefreshTimer;

use self::act::{Signal, TAG_RENICE, TAG_SIGNAL};
use self::cpu::CpuTracker;
use self::format::{format_kb, format_percent};
use self::view::{
    descendants, layout, Process, SortDirection, SortKey, ViewMode, ViewOptions, ViewRow,
};

// ========================================================================
// Constants
// ========================================================================

/// The questions the page asks.
const ASK_CONFIRM: &str = "proc-confirm";
const ASK_FILTER: &str = "proc-filter";
const ASK_RENICE: &str = "proc-renice-value";

/// Rows above the first process: the summary and the column headings.
const HEADER_ROWS: usize = 2;

/// The row the column headings are painted on, where a click sorts.
const COLUMN_ROW: usize = 1;

/// Rows the detail strip under the list takes, and the pane height below
/// which it is left out so the list keeps what room there is.
const DETAIL_ROWS: usize = 3;
const MIN_ROWS_FOR_DETAIL: usize = 12;

/// How long after a snapshot arrives the next is asked for. Long enough for a
/// CPU rate to mean something, short enough to feel live.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long a snapshot may be outstanding before it is given up on and asked
/// for again. The runner drops a request when it is busy, so without this a
/// dropped one would leave the page waiting forever.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// A CPU or memory share at or past this is drawn in the accent color.
const HOT_PERCENT: f64 = 50.0;

/// What the monitor is called in the pane title.
const TITLE: &str = "Processes";

/// The one-line key reminder after the summary.
const HINTS: &str = "  s sort  v view  / filter  x kill  q close";

/// The label of the renice question on each family of systems.
const RENICE_LABEL_UNIX: &str = "nice value (-20 to 19): ";
const RENICE_LABEL_WINDOWS: &str =
    "priority (Idle, BelowNormal, Normal, AboveNormal, High, RealTime): ";

/// Column widths, in cells. A column is followed by one space.
const PID_WIDTH: usize = 7;
const USER_WIDTH: usize = 10;
const CPU_WIDTH: usize = 6;
const MEM_WIDTH: usize = 6;
const RSS_WIDTH: usize = 7;
const STATE_WIDTH: usize = 6;
const VRAM_WIDTH: usize = 7;

/// How wide the NAME column may be. It starts at the least and grows to fit
/// the longest name (and its tree indent) the list has shown, up to the most,
/// so a name is printed whole unless it is longer than anyone's.
const MIN_NAME_WIDTH: usize = 16;
const MAX_NAME_WIDTH: usize = 40;

/// Characters of a process name kept visible however deep it sits in the tree,
/// so the indent stops growing before it pushes the name out of its column.
const MIN_NAME_CHARS: usize = 8;

/// The mark on a name too long for its column.
const ELLIPSIS: char = '\u{2026}';

/// What the video memory column shows for a process no GPU tool lists.
const NO_VRAM: &str = "-";

/// The heading of the last column, which takes whatever width is left.
const COMMAND_LABEL: &str = "COMMAND";

/// Cells a level of the tree indents by.
const TREE_INDENT: &str = "  ";

/// The marker before a process with children, folded and not.
const MARKER_FOLDED: char = '\u{25b8}';
const MARKER_OPEN: char = '\u{25be}';

/// What the menu opened over a row calls each of the entries it offers.
const LABEL_COPY_COMMAND: &str = "Copy Command";
const LABEL_COPY_PID: &str = "Copy PID";
const LABEL_FOLD: &str = "Fold / Unfold";
const LABEL_KILL: &str = "Kill";
const LABEL_KILL_TREE: &str = "Kill Tree";
const LABEL_RENICE: &str = "Renice...";
const LABEL_RESUME: &str = "Resume";
const LABEL_SUSPEND: &str = "Suspend";
const LABEL_TERMINATE: &str = "Terminate";
const LABEL_TERMINATE_TREE: &str = "Terminate Tree";

// ========================================================================
// Data Structures
// ========================================================================

/// A process monitor.
#[derive(Debug)]
pub struct ProcPage {
    /// What each signal or priority change in flight is reported as.
    acting: Option<String>,
    /// Tree parents whose children are folded away.
    collapsed: HashSet<u32>,
    cpu: CpuTracker,
    cursor: usize,
    direction: SortDirection,
    /// Why the last snapshot failed, until one succeeds.
    error: Option<String>,
    filter: String,
    /// Whether any snapshot has listed video memory, which is what earns the
    /// column its place. It stays once seen, so one failed query does not make
    /// the columns jump.
    has_gpu: bool,
    /// Whether a snapshot has arrived, so an empty list can say why.
    loaded: bool,
    /// What the last action reported, shown until the next key.
    message: Option<String>,
    mode: ViewMode,
    /// Cells the NAME column takes now: grown to fit, never shrunk, so the
    /// columns after it do not jump about as the list changes.
    name_width: usize,
    /// The kill the user is being asked to confirm.
    pending_signal: Option<PendingSignal>,
    /// The process a priority is being asked for.
    pending_renice: Option<u32>,
    processes: Vec<Process>,
    rows: Vec<ViewRow>,
    scroll: usize,
    /// The process the cursor is on, so a refresh that reorders the list
    /// leaves the cursor on the same process rather than the same row.
    selected_pid: Option<u32>,
    sort: SortKey,
    nav: VimNav,
    /// The screen height of each list row last painted, and the list row the
    /// first of them is, so a click on the pane maps back to a process.
    painted: Vec<usize>,
    painted_start: usize,
    /// When snapshots are asked for.
    timer: RefreshTimer,
    /// Rows the list had room for when last painted.
    viewport: usize,
}

/// A signal waiting on the answer to "are you sure".
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct PendingSignal {
    pid: u32,
    signal: Signal,
    tree: bool,
}

/// One of the columns before the command: how it is headed, how wide it is,
/// and what clicking its heading sorts by.
#[derive(Clone, Copy, Debug)]
struct Column {
    label: &'static str,
    right: bool,
    sort: SortKey,
    width: usize,
}

// ========================================================================
// ProcPage
// ========================================================================

impl ProcPage {
    /// A monitor that has read nothing yet and asks for its first snapshot on
    /// the next tick.
    pub fn new() -> Self {
        let sort = SortKey::Cpu;
        Self {
            acting: None,
            collapsed: HashSet::new(),
            cpu: CpuTracker::new(),
            cursor: 0,
            direction: sort.default_direction(),
            error: None,
            filter: String::new(),
            has_gpu: false,
            loaded: false,
            message: None,
            mode: ViewMode::Tree,
            name_width: MIN_NAME_WIDTH,
            pending_signal: None,
            pending_renice: None,
            processes: Vec::new(),
            rows: Vec::new(),
            scroll: 0,
            selected_pid: None,
            sort,
            nav: VimNav::new(),
            painted: Vec::new(),
            painted_start: 0,
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
            viewport: 0,
        }
    }

    /// Take in a snapshot and settle the cursor on the process it was on.
    fn on_sample(&mut self, sample: &ProcessSample) {
        self.processes = self.cpu.rates(sample);
        self.loaded = true;
        self.has_gpu |= sample.has_gpu;
        self.error = None;
        self.rebuild();
    }

    /// Lay the list out again, keeping the cursor on the same process when it
    /// is still listed and on the same row when it is not.
    fn rebuild(&mut self) {
        let options = ViewOptions {
            collapsed: &self.collapsed,
            direction: self.direction,
            filter: &self.filter,
            mode: self.mode,
            sort: self.sort,
        };
        self.rows = layout(&self.processes, &options);
        self.grow_name_column();
        let kept = self.selected_pid.and_then(|pid| {
            self.rows
                .iter()
                .position(|row| self.processes[row.index].pid == pid)
        });
        match kept {
            Some(at) => self.cursor = at,
            None => self.move_to(self.cursor),
        }
    }

    /// Widen the NAME column to fit the longest name listed with the indent
    /// and fold marker the tree puts before it.
    fn grow_name_column(&mut self) {
        let indent = |row: &ViewRow| match self.mode {
            ViewMode::Flat => 0,
            ViewMode::Tree => row.depth * TREE_INDENT.len() + 2,
        };
        let needed = self
            .rows
            .iter()
            .map(|row| indent(row) + self.processes[row.index].name.chars().count())
            .max()
            .unwrap_or(0);
        self.name_width = self.name_width.max(needed.min(MAX_NAME_WIDTH));
    }

    /// The process the cursor is on.
    fn selected(&self) -> Option<&Process> {
        let row = self.rows.get(self.cursor)?;
        self.processes.get(row.index)
    }

    /// Put the cursor on row `at`, or the nearest row there is.
    fn move_to(&mut self, at: usize) {
        self.cursor = at.min(self.rows.len().saturating_sub(1));
        self.selected_pid = self.selected().map(|process| process.pid);
    }

    /// Move the cursor by `delta` rows, stopping at either end.
    fn move_by(&mut self, delta: isize) {
        let target = self.cursor as isize + delta;
        self.move_to(target.max(0) as usize);
    }

    /// Interpret a motion of the shared Vim layer over the rows.
    fn apply_motion(&mut self, motion: CursorMove) {
        let half = (self.viewport / 2).max(1) as isize;
        let page = self.viewport.max(1) as isize;
        match motion {
            CursorMove::Down => self.move_by(1),
            CursorMove::Up => self.move_by(-1),
            CursorMove::Top => self.move_to(0),
            CursorMove::Bottom => self.move_to(usize::MAX),
            CursorMove::HalfPageDown => self.move_by(half),
            CursorMove::HalfPageUp => self.move_by(-half),
            CursorMove::PageDown => self.move_by(page),
            CursorMove::PageUp => self.move_by(-page),
            CursorMove::ScreenTop => self.move_to(self.scroll),
            CursorMove::ScreenMiddle => self.move_to(self.scroll + self.viewport / 2),
            CursorMove::ScreenBottom => {
                self.move_to(self.scroll + self.viewport.saturating_sub(1));
            }
            // The column motions and the rest mean nothing over rows.
            _ => {}
        }
    }

    /// Sort by `sort`, in the direction that column starts in.
    fn sort_by(&mut self, sort: SortKey) {
        self.sort = sort;
        self.direction = sort.default_direction();
        self.rebuild();
    }

    /// Fold the selected process's children away, or bring them back.
    fn set_folded(&mut self, folded: Option<bool>) {
        if self.mode != ViewMode::Tree {
            return;
        }
        let Some(row) = self.rows.get(self.cursor).copied() else {
            return;
        };
        if !row.has_children {
            return;
        }
        let pid = self.processes[row.index].pid;
        let fold = folded.unwrap_or(!row.folded);
        if fold {
            self.collapsed.insert(pid);
        } else {
            self.collapsed.remove(&pid);
        }
        self.rebuild();
    }

    /// Ask for a snapshot now, whatever the timer says.
    fn refresh_now(&mut self) -> PageOutcome {
        self.timer.request_now(Instant::now());
        PageOutcome::Job(JobRequest::Processes)
    }

    /// Send a signal to the selected process: stop and continue at once, and
    /// the ones that end a process after the user has said yes.
    fn signal_selected(&mut self, signal: Signal, tree: bool) -> PageOutcome {
        let Some((pid, label)) = self.selected().map(|p| (p.pid, p.name.clone())) else {
            return PageOutcome::Consumed;
        };
        if matches!(signal, Signal::Continue | Signal::Stop) {
            return self.send_signal(PendingSignal { pid, signal, tree });
        }
        let what = if tree {
            format!("{label} (pid {pid}) and everything under it")
        } else {
            format!("{label} (pid {pid})")
        };
        self.pending_signal = Some(PendingSignal { pid, signal, tree });
        let mut verb = signal.verb().to_string();
        verb[..1].make_ascii_uppercase();
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!("{verb} {what}? (y/n) "),
            mode: PromptMode::Confirm,
            tag: ASK_CONFIRM,
        })
    }

    /// Run a signal that has been decided on.
    fn send_signal(&mut self, pending: PendingSignal) -> PageOutcome {
        let below = if pending.tree {
            descendants(&self.processes, pending.pid)
        } else {
            Vec::new()
        };
        let name = self
            .processes
            .iter()
            .find(|process| process.pid == pending.pid)
            .map_or_else(|| pending.pid.to_string(), |process| process.name.clone());
        match act::signal_request(
            pending.signal,
            pending.pid,
            &below,
            pending.tree,
            cfg!(windows),
        ) {
            Ok(request) => {
                self.acting = Some(format!(
                    "{} {name} ({})",
                    pending.signal.past_tense(),
                    pending.pid
                ));
                PageOutcome::Job(JobRequest::Command(request))
            }
            Err(reason) => {
                self.message = Some(reason);
                PageOutcome::Consumed
            }
        }
    }

    /// Ask what priority the selected process should have.
    fn ask_renice(&mut self) -> PageOutcome {
        let Some(pid) = self.selected().map(|process| process.pid) else {
            return PageOutcome::Consumed;
        };
        self.pending_renice = Some(pid);
        let label = if cfg!(windows) {
            RENICE_LABEL_WINDOWS
        } else {
            RENICE_LABEL_UNIX
        };
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag: ASK_RENICE,
        })
    }

    /// Ask what the list should be narrowed to.
    fn ask_filter(&self) -> PageOutcome {
        PageOutcome::Prompt(PromptRequest {
            initial: self.filter.clone(),
            label: "filter: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_FILTER,
        })
    }

    /// What a finished signal or priority change reports.
    fn on_command_done(&mut self, output: &CommandOutput) -> PageOutcome {
        let done = self.acting.take();
        self.message = Some(match (output.succeeded(), done) {
            (true, Some(what)) => what,
            (true, None) => "done".to_string(),
            (false, _) => format!("failed: {}", output.failure()),
        });
        // The list is about to be wrong, so the next snapshot is not left to
        // the timer.
        self.timer.refresh_soon(Instant::now());
        PageOutcome::Consumed
    }

    /// The columns before the command, left to right.
    fn columns(&self) -> Vec<Column> {
        let column = |label, width, right, sort| Column {
            label,
            right,
            sort,
            width,
        };
        let mut columns = vec![
            column("PID", PID_WIDTH, true, SortKey::Pid),
            column("NAME", self.name_width, false, SortKey::Name),
            column("CPU%", CPU_WIDTH, true, SortKey::Cpu),
            column("MEM%", MEM_WIDTH, true, SortKey::Memory),
            column("RSS", RSS_WIDTH, true, SortKey::Memory),
        ];
        if self.has_gpu {
            columns.push(column("VRAM", VRAM_WIDTH, true, SortKey::Gpu));
        }
        columns.push(column("STATE", STATE_WIDTH, false, SortKey::State));
        columns.push(column("USER", USER_WIDTH, false, SortKey::User));
        columns
    }

    /// Cells the columns before the command take, so a command that wraps
    /// lines up under itself rather than under the pid.
    fn fixed_width(&self) -> usize {
        self.columns().iter().map(|column| column.width + 1).sum()
    }

    /// What clicking the heading row at `col` sorts by: the column there, or
    /// the command for anything past the columns.
    fn sort_key_at(&self, col: usize) -> SortKey {
        let mut start = 0;
        for column in self.columns() {
            start += column.width + 1;
            if col < start {
                return column.sort;
            }
        }
        SortKey::Command
    }

    /// Sort by `key`: the first click on a column sorts it the way it starts,
    /// and a click on the sorted column turns it round.
    fn sort_on_click(&mut self, key: SortKey) {
        if key == self.sort {
            self.direction = self.direction.reversed();
            self.rebuild();
        } else {
            self.sort_by(key);
        }
    }

    // ====================================================================
    // Painting
    // ====================================================================

    /// The first row: how many processes, how they are listed, and what the
    /// last action said.
    fn summary_row(&self) -> PageRow {
        let mut text = match (self.loaded, self.processes.len()) {
            (false, _) => "reading processes...".to_string(),
            (true, count) => format!("{count} processes"),
        };
        if !self.filter.is_empty() {
            text.push_str(&format!(" ({} shown)", self.rows.len()));
        }
        let mode = match self.mode {
            ViewMode::Flat => "flat",
            ViewMode::Tree => "tree",
        };
        text.push_str(&format!("  {mode}  sort {}", self.sort.label()));
        if !self.filter.is_empty() {
            text.push_str(&format!("  filter: {}", self.filter));
        }
        if self.timer.is_paused() {
            text.push_str("  paused");
        }
        let mut row = vec![PageSpan::new(PageStyle::Header, text)];
        let report = self.message.as_ref().or(self.error.as_ref());
        match report {
            Some(report) => row.push(PageSpan::new(PageStyle::Accent, format!("  {report}"))),
            None => row.push(PageSpan::new(PageStyle::Dim, HINTS)),
        }
        row
    }

    /// The column headings, with the sorted one marked.
    fn column_row(&self) -> PageRow {
        let heading = |label: &str, sorted: bool| {
            if sorted {
                format!("{}{label}", self.direction.arrow())
            } else {
                label.to_string()
            }
        };
        let style = |sorted: bool| {
            if sorted {
                PageStyle::HeaderAccent
            } else {
                PageStyle::Header
            }
        };
        let mut row: PageRow = self
            .columns()
            .into_iter()
            .map(|column| {
                let sorted = column.sort == self.sort;
                let label = heading(column.label, sorted);
                let width = column.width;
                let cell = if column.right {
                    format!("{label:>width$} ")
                } else {
                    format!("{label:<width$} ")
                };
                PageSpan::new(style(sorted), cell)
            })
            .collect();
        let sorted = self.sort == SortKey::Command;
        row.push(PageSpan::new(style(sorted), heading(COMMAND_LABEL, sorted)));
        row
    }

    /// The list row painted at screen row `row`, if one is: a wrapped row
    /// takes several screen rows, which is why the heights are kept.
    fn list_row_at(&self, row: usize) -> Option<usize> {
        let mut top = HEADER_ROWS;
        for (offset, height) in self.painted.iter().enumerate() {
            if row < top + height {
                return Some(self.painted_start + offset);
            }
            top += height;
        }
        None
    }

    /// The strip under the list describing the process the cursor is on: its
    /// ids, shares, and paths, and the whole of its command line.
    fn detail_rows(&self, cols: usize) -> Vec<PageRow> {
        let Some(process) = self.selected() else {
            return Vec::new();
        };
        let field = |label: &str, value: String| {
            [
                PageSpan::new(PageStyle::Dim, format!("{label} ")),
                PageSpan::plain(format!("{value}  ")),
            ]
        };
        let mut facts: PageRow = Vec::new();
        facts.extend(field("PID", process.pid.to_string()));
        facts.extend(field(
            "PPID",
            process.ppid.map_or("-".to_string(), |p| p.to_string()),
        ));
        facts.extend(field(
            "USER",
            process.user.clone().unwrap_or("-".to_string()),
        ));
        facts.extend(field("STATE", state_label(process.state).to_string()));
        facts.extend(field(
            "CPU",
            format!("{}%", format_percent(process.cpu_percent)),
        ));
        facts.extend(field(
            "MEM",
            format!("{}%", format_percent(process.mem_percent)),
        ));
        facts.extend(field("RSS", format_kb(process.rss_kb)));
        if let Some(gpu_kb) = process.gpu_kb {
            facts.extend(field("VRAM", format_kb(gpu_kb)));
        }
        let mut places: PageRow = Vec::new();
        if let Some(path) = &process.exec_path {
            places.extend(field("PATH", path.clone()));
        }
        if let Some(cwd) = &process.cwd {
            places.extend(field("CWD", cwd.clone()));
        }
        let command = vec![
            PageSpan::new(PageStyle::Dim, "CMD "),
            PageSpan::plain(process.command.clone()),
        ];
        vec![
            clip_row(facts, cols),
            clip_row(places, cols),
            clip_row(command, cols),
        ]
    }

    /// The NAME cell: the tree's indent and fold marker, then the name, cut to
    /// fit and padded to the column.
    fn name_cell(&self, process: &Process, row: &ViewRow) -> [PageSpan; 2] {
        let prefix = match self.mode {
            ViewMode::Flat => String::new(),
            ViewMode::Tree => {
                let marker = match (row.has_children, row.folded) {
                    (false, _) => ' ',
                    (true, true) => MARKER_FOLDED,
                    (true, false) => MARKER_OPEN,
                };
                let room = self.name_width.saturating_sub(MIN_NAME_CHARS + 2);
                let indent: String = TREE_INDENT.repeat(row.depth).chars().take(room).collect();
                format!("{indent}{marker} ")
            }
        };
        let room = self.name_width.saturating_sub(prefix.chars().count());
        let name = fit(&process.name, room);
        let padding = room.saturating_sub(name.chars().count());
        [
            PageSpan::new(PageStyle::Dim, prefix),
            PageSpan::plain(format!("{name}{} ", " ".repeat(padding))),
        ]
    }

    /// One process as a line.
    fn process_row(&self, row: &ViewRow) -> PageRow {
        let process = &self.processes[row.index];
        let user = process.user.as_deref().unwrap_or("-");
        let hot = |percent: f64| {
            if percent >= HOT_PERCENT {
                PageStyle::Accent
            } else {
                PageStyle::Normal
            }
        };
        let vram = process.gpu_kb.map_or(NO_VRAM.to_string(), format_kb);
        let mut spans = vec![PageSpan::new(
            PageStyle::Dim,
            format!("{:>PID_WIDTH$} ", process.pid),
        )];
        spans.extend(self.name_cell(process, row));
        spans.push(PageSpan::new(
            hot(process.cpu_percent),
            format!("{:>CPU_WIDTH$} ", format_percent(process.cpu_percent)),
        ));
        spans.push(PageSpan::new(
            hot(process.mem_percent),
            format!("{:>MEM_WIDTH$} ", format_percent(process.mem_percent)),
        ));
        spans.push(PageSpan::plain(format!(
            "{:>RSS_WIDTH$} ",
            format_kb(process.rss_kb)
        )));
        if self.has_gpu {
            spans.push(PageSpan::plain(format!("{vram:>VRAM_WIDTH$} ")));
        }
        spans.push(PageSpan::new(
            state_style(process.state),
            format!("{:<STATE_WIDTH$} ", state_label(process.state)),
        ));
        spans.push(PageSpan::new(
            PageStyle::Dim,
            format!("{user:<USER_WIDTH$.USER_WIDTH$} "),
        ));
        spans.push(PageSpan::plain(process.command.clone()));
        spans
    }
}

impl Default for ProcPage {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Page
// ========================================================================

impl Page for ProcPage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
        let detail_rows = if rows >= MIN_ROWS_FOR_DETAIL {
            DETAIL_ROWS
        } else {
            0
        };
        self.viewport = rows.saturating_sub(HEADER_ROWS + detail_rows);
        // Neither heading row may wrap: a click is told which screen row it
        // landed on, and the headings are only on screen row one if the
        // summary above them stays on a single line.
        let mut page_rows = vec![
            clip_row(self.summary_row(), cols),
            clip_row(self.column_row(), cols),
        ];
        let fixed = self.fixed_width();
        self.painted.clear();
        if self.rows.is_empty() {
            let note = match self.loaded {
                true => "no matching processes",
                false => "",
            };
            page_rows.push(vec![PageSpan::new(PageStyle::Dim, note)]);
            return PageContent::new(page_rows);
        }
        let height_of = |index: usize| {
            row_height(
                &row_text(&self.process_row(&self.rows[index])),
                cols,
                wrap,
                fixed,
            )
        };
        let window = wrap_window(
            self.scroll,
            self.cursor,
            self.rows.len(),
            self.viewport,
            height_of,
        );
        let shown = window.start..window.start + window.count;
        let heights: Vec<usize> = shown.clone().map(height_of).collect();
        self.scroll = window.start;
        self.painted = heights;
        self.painted_start = window.start;
        page_rows.extend(self.rows[shown].iter().map(|row| self.process_row(row)));
        let mut indents = vec![0; HEADER_ROWS];
        indents.extend(std::iter::repeat_n(fixed, window.count));
        if detail_rows > 0 {
            // Blank rows keep the detail strip at the bottom of the pane
            // however few processes there are.
            let used: usize = self.painted.iter().sum();
            let padding = self.viewport.saturating_sub(used);
            page_rows.extend(std::iter::repeat_n(PageRow::new(), padding));
            indents.extend(std::iter::repeat_n(0, padding));
            let detail = self.detail_rows(cols);
            indents.extend(std::iter::repeat_n(0, detail.len()));
            page_rows.extend(detail);
        }
        PageContent::new(page_rows)
            .with_cursor_line(HEADER_ROWS + window.cursor)
            .with_wrap_indents(indents)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        if self.nav.in_sequence() || key.alt || key.ctrl {
            return self.on_motion_key(key);
        }
        match key.code {
            KeyCode::Char('s') => {
                let mut next = self.sort.next();
                // A column that is not shown is not one to land on.
                if next == SortKey::Gpu && !self.has_gpu {
                    next = next.next();
                }
                self.sort_by(next);
                PageOutcome::Consumed
            }
            KeyCode::Char('S') => {
                self.direction = self.direction.reversed();
                self.rebuild();
                PageOutcome::Consumed
            }
            KeyCode::Char('v') => {
                self.mode = match self.mode {
                    ViewMode::Flat => ViewMode::Tree,
                    ViewMode::Tree => ViewMode::Flat,
                };
                self.rebuild();
                PageOutcome::Consumed
            }
            KeyCode::Enter | KeyCode::Space | KeyCode::Tab => {
                self.set_folded(None);
                PageOutcome::Consumed
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.set_folded(Some(true));
                PageOutcome::Consumed
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.set_folded(Some(false));
                PageOutcome::Consumed
            }
            KeyCode::Char('/') => self.ask_filter(),
            KeyCode::Char('x') => self.signal_selected(Signal::Terminate, false),
            KeyCode::Char('X') => self.signal_selected(Signal::Kill, false),
            KeyCode::Char('t') => self.signal_selected(Signal::Terminate, true),
            KeyCode::Char('T') => self.signal_selected(Signal::Kill, true),
            KeyCode::Char('z') => self.signal_selected(Signal::Stop, false),
            KeyCode::Char('Z') => self.signal_selected(Signal::Continue, false),
            KeyCode::Char('R') => self.ask_renice(),
            KeyCode::Char('r') => self.refresh_now(),
            KeyCode::Char('p') => {
                self.timer.toggle_pause(Instant::now());
                PageOutcome::Consumed
            }
            KeyCode::Char('y') => match self.selected() {
                Some(process) => PageOutcome::Yank(process.pid.to_string()),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('Y') => match self.selected() {
                Some(process) => PageOutcome::Yank(process.command.clone()),
                None => PageOutcome::Consumed,
            },
            // Escape first lets go of a filter, and only then closes, so the
            // key that means stop never costs the page you were reading.
            KeyCode::Escape if !self.filter.is_empty() => {
                self.filter.clear();
                self.rebuild();
                PageOutcome::Consumed
            }
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            _ => self.on_motion_key(key),
        }
    }

    fn on_mouse(&mut self, at: PagePoint) -> PageOutcome {
        // Only a press on a heading or a process means anything here; the
        // rest of the pane is left to the terminal's own selection.
        if at.drag {
            return PageOutcome::Ignored;
        }
        if at.row == COLUMN_ROW {
            let key = self.sort_key_at(at.col);
            self.sort_on_click(key);
            return PageOutcome::Consumed;
        }
        match self.list_row_at(at.row) {
            Some(index) => {
                self.move_to(index);
                PageOutcome::Consumed
            }
            None => PageOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        let answer = reply.answer;
        match reply.tag {
            ASK_CONFIRM => {
                let pending = self.pending_signal.take();
                match (answer, pending) {
                    (Some(_), Some(pending)) => self.send_signal(pending),
                    _ => PageOutcome::Consumed,
                }
            }
            ASK_FILTER => {
                if let Some(text) = answer {
                    self.filter = text.trim().to_string();
                    self.rebuild();
                }
                PageOutcome::Consumed
            }
            ASK_RENICE => {
                let pid = self.pending_renice.take();
                let (Some(value), Some(pid)) = (answer, pid) else {
                    return PageOutcome::Consumed;
                };
                match act::renice_request(pid, &value, cfg!(windows)) {
                    Ok(request) => {
                        self.acting = Some(format!("set priority of {pid} to {}", value.trim()));
                        PageOutcome::Job(JobRequest::Command(request))
                    }
                    Err(reason) => {
                        self.message = Some(reason);
                        PageOutcome::Consumed
                    }
                }
            }
            _ => PageOutcome::Consumed,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> PageOutcome {
        match reply {
            JobReply::Processes(result) => {
                self.timer.on_answer(Instant::now());
                match result {
                    Ok(sample) => self.on_sample(&sample),
                    Err(reason) => self.error = Some(reason),
                }
            }
            JobReply::Command(output) if output.tag == TAG_SIGNAL || output.tag == TAG_RENICE => {
                return self.on_command_done(&output);
            }
            JobReply::Command(_)
            | JobReply::DirSize { .. }
            | JobReply::Files(_)
            | JobReply::Search(_)
            | JobReply::System(_) => {}
        }
        PageOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> PageOutcome {
        if !self.timer.on_tick(now) {
            return PageOutcome::Consumed;
        }
        PageOutcome::Job(JobRequest::Processes)
    }

    fn on_resume(&mut self) -> PageOutcome {
        // Work asked for before the page was covered was cancelled with it.
        self.timer.restart(Instant::now());
        PageOutcome::Consumed
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.selected()
            .and_then(|process| process.cwd.as_ref())
            .map(PathBuf::from)
    }

    fn context_items(&self) -> Vec<PageMenuItem> {
        if self.selected().is_none() {
            return Vec::new();
        }
        let entries = [
            (KeyCode::Char('x'), LABEL_TERMINATE),
            (KeyCode::Char('X'), LABEL_KILL),
            (KeyCode::Char('t'), LABEL_TERMINATE_TREE),
            (KeyCode::Char('T'), LABEL_KILL_TREE),
            (KeyCode::Char('z'), LABEL_SUSPEND),
            (KeyCode::Char('Z'), LABEL_RESUME),
            (KeyCode::Char('R'), LABEL_RENICE),
            (KeyCode::Enter, LABEL_FOLD),
            (KeyCode::Char('y'), LABEL_COPY_PID),
            (KeyCode::Char('Y'), LABEL_COPY_COMMAND),
        ];
        entries
            .into_iter()
            .map(|(code, label)| PageMenuItem::new(Key::plain(code), label))
            .collect()
    }
}

// ========================================================================
// Helpers
// ========================================================================

impl ProcPage {
    /// Offer a key to the shared Vim motion layer, and interpret what it
    /// resolves to over the rows.
    fn on_motion_key(&mut self, key: &Key) -> PageOutcome {
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                PageOutcome::Consumed
            }
            VimKey::Pending => PageOutcome::Consumed,
            VimKey::Unhandled => PageOutcome::Ignored,
        }
    }
}

/// `row` cut to `cols` cells, so it takes one screen row whatever the pane's
/// width and the host's wrapping.
fn clip_row(row: PageRow, cols: usize) -> PageRow {
    let mut room = cols;
    let mut clipped = Vec::with_capacity(row.len());
    for span in row {
        if room == 0 {
            break;
        }
        let text: String = span.text.chars().take(room).collect();
        room -= text.chars().count();
        clipped.push(PageSpan::new(span.style, text));
    }
    clipped
}

/// `text` cut to `width` cells, with a mark where it was cut.
fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    match width {
        0 => String::new(),
        _ => format!("{kept}{ELLIPSIS}"),
    }
}

/// The word under STATE.
fn state_label(state: ProcState) -> &'static str {
    match state {
        ProcState::Running => "run",
        ProcState::Sleeping => "sleep",
        ProcState::Stopped => "stop",
        ProcState::Unknown => "-",
        ProcState::Zombie => "zombie",
    }
}

/// How a state is colored: what is running and what is wrong stand out from
/// the sleeping majority.
fn state_style(state: ProcState) -> PageStyle {
    match state {
        ProcState::Running => PageStyle::ChangeAdded,
        ProcState::Sleeping | ProcState::Unknown => PageStyle::Dim,
        ProcState::Stopped => PageStyle::ChangeModified,
        ProcState::Zombie => PageStyle::ChangeDeleted,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::model::page::CommandRequest;
    use crate::model::process::{CpuReading, RawProcess};

    use super::*;

    fn press(code: KeyCode) -> Key {
        Key::plain(code)
    }

    fn raw(pid: u32, ppid: u32, name: &str, ms: u64) -> RawProcess {
        RawProcess {
            command: format!("{name} --serve"),
            cpu: CpuReading::Time(ms),
            cwd: Some(format!("/work/{name}")),
            exec_path: None,
            gpu_kb: None,
            name: name.to_string(),
            pid,
            ppid: Some(ppid),
            rss_kb: 1024,
            state: ProcState::Sleeping,
            user: Some("me".to_string()),
        }
    }

    fn sample_at(taken: Instant, ms: [u64; 3]) -> ProcessSample {
        ProcessSample {
            entries: vec![
                raw(1, 0, "init", ms[0]),
                raw(10, 1, "server", ms[1]),
                raw(11, 10, "worker", ms[2]),
            ],
            has_gpu: false,
            taken,
            total_mem_kb: 1_000_000,
        }
    }

    fn loaded_page() -> ProcPage {
        let mut page = ProcPage::new();
        page.on_job(JobReply::Processes(Ok(sample_at(
            Instant::now(),
            [0, 0, 0],
        ))));
        page
    }

    fn cursor_on(page: &mut ProcPage, name: &str) {
        let at = page
            .rows
            .iter()
            .position(|row| page.processes[row.index].name == name)
            .expect("process is listed");
        page.move_to(at);
    }

    fn command_of(outcome: PageOutcome) -> CommandRequest {
        let PageOutcome::Job(JobRequest::Command(request)) = outcome else {
            panic!("expected a command, got {outcome:?}");
        };
        request
    }

    #[test]
    fn test_the_first_tick_asks_for_a_snapshot_and_a_pending_one_is_not_asked_twice() {
        let mut page = ProcPage::new();
        let now = Instant::now();
        assert_eq!(page.on_tick(now), PageOutcome::Job(JobRequest::Processes));
        // With one outstanding, the next wake is the give-up deadline, not
        // another request.
        let deadline = page.next_tick().unwrap();
        assert!(deadline >= now + STALL_TIMEOUT - Duration::from_secs(1));
    }

    #[test]
    fn test_a_snapshot_that_never_came_back_is_asked_for_again() {
        let mut page = ProcPage::new();
        page.on_tick(Instant::now());
        // The runner drops a request when it is full. Retrying at the
        // give-up deadline is what stops the page waiting forever.
        let again = page.on_tick(Instant::now() + STALL_TIMEOUT);
        assert_eq!(again, PageOutcome::Job(JobRequest::Processes));
    }

    #[test]
    fn test_an_answer_schedules_the_next_snapshot_one_interval_out() {
        let mut page = ProcPage::new();
        page.on_tick(Instant::now());
        let before = Instant::now();
        page.on_job(JobReply::Processes(Ok(sample_at(before, [0, 0, 0]))));
        assert!(page.next_tick().unwrap() >= before + REFRESH_INTERVAL);
    }

    #[test]
    fn test_a_paused_page_asks_for_nothing_and_wakes_nobody() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('p')));
        assert_eq!(page.next_tick(), None);
        assert_eq!(page.on_tick(Instant::now()), PageOutcome::Consumed);
    }

    #[test]
    fn test_the_cursor_stays_on_its_process_when_a_refresh_reorders_the_list() {
        let mut page = ProcPage::new();
        let start = Instant::now();
        page.on_job(JobReply::Processes(Ok(sample_at(start, [0, 0, 0]))));
        page.on_key(&press(KeyCode::Char('v')));
        cursor_on(&mut page, "worker");
        // Now the worker is the busiest and sorts to the top of the flat list.
        let later = start + Duration::from_secs(2);
        page.on_job(JobReply::Processes(Ok(sample_at(later, [0, 100, 2000]))));
        assert_eq!(page.selected().unwrap().name, "worker");
        assert_eq!(page.cursor, 0);
    }

    #[test]
    fn test_a_process_that_exits_leaves_the_cursor_on_the_same_row() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('v')));
        page.move_to(2);
        let mut smaller = sample_at(Instant::now(), [0, 0, 0]);
        smaller.entries.pop();
        page.on_job(JobReply::Processes(Ok(smaller)));
        // Row two no longer exists, so the cursor lands on the last row there
        // is rather than on nothing.
        assert!(page.cursor < page.rows.len());
        assert!(page.selected().is_some());
    }

    #[test]
    fn test_terminating_asks_first_and_names_the_process() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('x'))) else {
            panic!("expected a confirmation");
        };
        assert_eq!(request.mode, PromptMode::Confirm);
        assert!(request.label.contains("server"), "got {:?}", request.label);
        assert!(request.label.contains("10"), "got {:?}", request.label);
    }

    #[cfg(unix)]
    #[test]
    fn test_confirming_sends_the_signal_to_the_selected_pid_only() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        page.on_key(&press(KeyCode::Char('x')));
        let request = command_of(page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_CONFIRM,
        }));
        assert_eq!(request.args, ["-TERM", "10"]);
    }

    #[cfg(unix)]
    #[test]
    fn test_a_tree_kill_reaches_the_children_first() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        page.on_key(&press(KeyCode::Char('T')));
        let request = command_of(page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_CONFIRM,
        }));
        assert_eq!(request.args, ["-KILL", "11", "10"]);
    }

    #[test]
    fn test_declining_the_confirmation_runs_nothing() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        page.on_key(&press(KeyCode::Char('X')));
        let outcome = page.on_prompt(PromptReply {
            answer: None,
            tag: ASK_CONFIRM,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
        // And the declined kill must not linger to be fired by a later,
        // unrelated answer.
        let stray = page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_CONFIRM,
        });
        assert_eq!(stray, PageOutcome::Consumed);
    }

    #[cfg(unix)]
    #[test]
    fn test_suspending_needs_no_confirmation() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        let request = command_of(page.on_key(&press(KeyCode::Char('z'))));
        assert_eq!(request.args, ["-STOP", "10"]);
    }

    #[test]
    fn test_a_finished_signal_reports_what_it_did_and_asks_for_a_fresh_list() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        page.on_key(&press(KeyCode::Char('z')));
        let before = Instant::now();
        page.on_job(JobReply::Command(CommandOutput {
            code: Some(0),
            stderr: String::new(),
            stdout: String::new(),
            tag: TAG_SIGNAL,
        }));
        assert_eq!(page.message.as_deref(), Some("suspended server (10)"));
        assert!(page.next_tick().unwrap() <= Instant::now().max(before));
    }

    #[test]
    fn test_a_failed_signal_says_why() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        page.on_key(&press(KeyCode::Char('z')));
        page.on_job(JobReply::Command(CommandOutput {
            code: Some(1),
            stderr: "kill: (10): Operation not permitted\n".to_string(),
            stdout: String::new(),
            tag: TAG_SIGNAL,
        }));
        let message = page.message.unwrap();
        assert!(
            message.contains("Operation not permitted"),
            "got {message:?}"
        );
    }

    #[test]
    fn test_a_filter_answer_narrows_the_list_and_escape_clears_it_before_closing() {
        let mut page = loaded_page();
        page.on_prompt(PromptReply {
            answer: Some(" work ".to_string()),
            tag: ASK_FILTER,
        });
        // Tree mode keeps the worker's ancestors above it.
        assert_eq!(page.rows.len(), 3);
        page.on_key(&press(KeyCode::Char('v')));
        assert_eq!(page.rows.len(), 1);
        assert_eq!(page.on_key(&press(KeyCode::Escape)), PageOutcome::Consumed);
        assert_eq!(page.rows.len(), 3);
        assert_eq!(page.on_key(&press(KeyCode::Escape)), PageOutcome::Close);
    }

    #[test]
    fn test_folding_hides_children_and_the_cursor_row_survives() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        page.on_key(&press(KeyCode::Enter));
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.selected().unwrap().name, "server");
        page.on_key(&press(KeyCode::Enter));
        assert_eq!(page.rows.len(), 3);
    }

    #[test]
    fn test_the_page_reports_the_selected_process_directory() {
        let mut page = loaded_page();
        cursor_on(&mut page, "worker");
        assert_eq!(page.cwd(), Some(PathBuf::from("/work/worker")));
    }

    #[test]
    fn test_a_failed_snapshot_is_shown_and_retried() {
        let mut page = ProcPage::new();
        page.on_tick(Instant::now());
        page.on_job(JobReply::Processes(Err("ps: not found".to_string())));
        let summary = row_text(&page.summary_row());
        assert!(summary.contains("ps: not found"), "got {summary:?}");
        assert!(page.next_tick().is_some());
    }

    fn click(page: &mut ProcPage, col: usize) -> PageOutcome {
        page.on_mouse(PagePoint {
            col,
            drag: false,
            row: COLUMN_ROW,
        })
    }

    fn gpu_page() -> ProcPage {
        let mut page = ProcPage::new();
        let mut sample = sample_at(Instant::now(), [0, 0, 0]);
        sample.has_gpu = true;
        sample.entries[1].gpu_kb = Some(2048);
        page.on_job(JobReply::Processes(Ok(sample)));
        page
    }

    #[test]
    fn test_clicking_a_heading_sorts_by_that_column_in_its_starting_direction() {
        let mut page = loaded_page();
        // The fixture's names are short, so NAME keeps its least width: PID is
        // cells 0 to 7, NAME 8 to 24, MEM% 32 to 38, STATE 47 to 53, USER 54
        // to 64, and the command is everything after.
        assert_eq!(click(&mut page, 0), PageOutcome::Consumed);
        assert_eq!(
            (page.sort, page.direction),
            (SortKey::Pid, SortDirection::Ascending)
        );
        click(&mut page, 10);
        assert_eq!(
            (page.sort, page.direction),
            (SortKey::Name, SortDirection::Ascending)
        );
        click(&mut page, 33);
        assert_eq!(
            (page.sort, page.direction),
            (SortKey::Memory, SortDirection::Descending)
        );
        click(&mut page, 48);
        assert_eq!(page.sort, SortKey::State);
        click(&mut page, 58);
        assert_eq!(page.sort, SortKey::User);
        click(&mut page, 100);
        assert_eq!(page.sort, SortKey::Command);
    }

    #[test]
    fn test_clicking_the_sorted_heading_again_reverses_it() {
        let mut page = loaded_page();
        click(&mut page, 0);
        click(&mut page, 3);
        assert_eq!(
            (page.sort, page.direction),
            (SortKey::Pid, SortDirection::Descending)
        );
        click(&mut page, 3);
        assert_eq!(page.direction, SortDirection::Ascending);
    }

    #[test]
    fn test_a_click_off_the_heading_row_or_a_drag_is_left_to_the_terminal() {
        let mut page = loaded_page();
        let below = page.on_mouse(PagePoint {
            col: 0,
            drag: false,
            row: HEADER_ROWS,
        });
        assert_eq!(below, PageOutcome::Ignored);
        let drag = page.on_mouse(PagePoint {
            col: 0,
            drag: true,
            row: COLUMN_ROW,
        });
        assert_eq!(drag, PageOutcome::Ignored);
        assert_eq!(page.sort, SortKey::Cpu);
    }

    #[test]
    fn test_the_video_memory_column_appears_only_when_a_gpu_tool_answered() {
        let plain = loaded_page().content(20, 120, false);
        assert!(!row_text(&plain.rows[COLUMN_ROW]).contains("VRAM"));
        let mut page = gpu_page();
        let content = page.content(20, 120, false);
        assert!(row_text(&content.rows[COLUMN_ROW]).contains("VRAM"));
        // The one with a reading shows it, and the others show a dash rather
        // than a zero they were never measured at.
        let lines: Vec<String> = content.rows[HEADER_ROWS..].iter().map(row_text).collect();
        assert!(
            lines.iter().any(|line| line.contains("2.0M")),
            "got {lines:?}"
        );
        assert!(
            lines.iter().any(|line| line.contains(" - ")),
            "got {lines:?}"
        );
    }

    #[test]
    fn test_the_heading_clicks_shift_right_when_the_video_memory_column_is_there() {
        let mut page = gpu_page();
        // Cell 48 is STATE without the column and VRAM with it.
        click(&mut page, 48);
        assert_eq!(page.sort, SortKey::Gpu);
        click(&mut page, 56);
        assert_eq!(page.sort, SortKey::State);
    }

    #[test]
    fn test_stepping_the_sort_skips_a_column_that_is_not_shown() {
        let mut page = loaded_page();
        page.sort_by(SortKey::Memory);
        page.on_key(&press(KeyCode::Char('s')));
        assert_eq!(page.sort, SortKey::State);
        let mut with_gpu = gpu_page();
        with_gpu.sort_by(SortKey::Memory);
        with_gpu.on_key(&press(KeyCode::Char('s')));
        assert_eq!(with_gpu.sort, SortKey::Gpu);
    }

    #[test]
    fn test_the_heading_rows_never_wrap_in_a_narrow_pane() {
        // A wrapped summary would push the headings to screen row two and
        // make every heading click land on the wrong row.
        let mut page = gpu_page();
        let content = page.content(20, 24, true);
        for row in &content.rows[..HEADER_ROWS] {
            assert!(
                row_text(row).chars().count() <= 24,
                "got {:?}",
                row_text(row)
            );
        }
    }

    fn press_at(page: &mut ProcPage, row: usize) -> PageOutcome {
        page.on_mouse(PagePoint {
            col: 0,
            drag: false,
            row,
        })
    }

    #[test]
    fn test_clicking_a_process_moves_the_cursor_onto_it() {
        let mut page = loaded_page();
        page.content(30, 120, false);
        // Rows are init, server, worker from screen row two down.
        assert_eq!(press_at(&mut page, HEADER_ROWS + 2), PageOutcome::Consumed);
        assert_eq!(page.selected().unwrap().name, "worker");
        press_at(&mut page, HEADER_ROWS);
        assert_eq!(page.selected().unwrap().name, "init");
    }

    #[test]
    fn test_a_click_below_the_last_process_selects_nothing() {
        let mut page = loaded_page();
        page.content(30, 120, false);
        assert_eq!(press_at(&mut page, HEADER_ROWS + 3), PageOutcome::Ignored);
        assert_eq!(page.selected().unwrap().name, "init");
    }

    #[test]
    fn test_a_click_lands_on_the_right_process_when_an_earlier_row_wraps() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('v')));
        // Flat: three rows. In a pane too narrow for the first row's command,
        // that row takes more than one screen row, so the second process is
        // not on the screen row after the first.
        page.content(30, 75, true);
        let first_height = page.painted[0];
        assert!(first_height > 1, "the fixture row should wrap");
        press_at(&mut page, HEADER_ROWS + first_height);
        assert_eq!(page.cursor, 1);
    }

    #[test]
    fn test_the_detail_strip_describes_the_selected_process_at_the_bottom() {
        let mut page = loaded_page();
        cursor_on(&mut page, "worker");
        let content = page.content(20, 160, false);
        assert_eq!(
            content.rows.len(),
            20,
            "padding keeps the strip at the bottom"
        );
        let facts = row_text(&content.rows[20 - DETAIL_ROWS]);
        assert!(facts.contains("PID 11"), "got {facts:?}");
        assert!(facts.contains("PPID 10"), "got {facts:?}");
        let cwd = row_text(&content.rows[20 - DETAIL_ROWS + 1]);
        assert!(cwd.contains("CWD /work/worker"), "got {cwd:?}");
        let command = row_text(&content.rows[19]);
        assert!(command.contains("worker --serve"), "got {command:?}");
    }

    #[test]
    fn test_a_short_pane_drops_the_detail_strip_to_keep_the_list() {
        let mut page = loaded_page();
        let content = page.content(MIN_ROWS_FOR_DETAIL - 1, 160, false);
        assert_eq!(content.rows.len(), HEADER_ROWS + 3);
    }

    fn name_cell_text(page: &ProcPage, name: &str) -> String {
        let at = page
            .rows
            .iter()
            .position(|row| page.processes[row.index].name == name)
            .expect("process is listed");
        let spans = page.name_cell(&page.processes[page.rows[at].index], &page.rows[at]);
        spans.iter().map(|span| span.text.as_str()).collect()
    }

    #[test]
    fn test_the_name_is_the_second_column_with_the_tree_in_it() {
        let mut page = loaded_page();
        let content = page.content(11, 160, false);
        let heading = row_text(&content.rows[COLUMN_ROW]);
        let pid = heading.find("PID").unwrap();
        let name = heading.find("NAME").unwrap();
        let cpu = heading.find("CPU%").unwrap();
        assert!(pid < name && name < cpu, "got {heading:?}");
        // The command column carries the command alone; the fold marker and
        // indent belong to the name.
        let server = row_text(&content.rows[HEADER_ROWS + 1]);
        assert!(server.contains("\u{25be} server"), "got {server:?}");
        assert!(server.ends_with("server --serve"), "got {server:?}");
        // The cell is always the column's width, so the columns after it line up.
        assert_eq!(
            name_cell_text(&page, "worker").chars().count(),
            page.name_width + 1
        );
        assert!(name_cell_text(&page, "worker").starts_with("    "));
    }

    fn long_named_page(name: &str) -> ProcPage {
        let mut page = ProcPage::new();
        let mut sample = sample_at(Instant::now(), [0, 0, 0]);
        sample.entries[2].name = name.to_string();
        page.on_job(JobReply::Processes(Ok(sample)));
        page
    }

    #[test]
    fn test_the_name_column_grows_to_print_a_long_name_whole() {
        let name = "kworker/u56:0-flush-259:5";
        let page = long_named_page(name);
        // The worker sits two levels down: indent and marker, then the name.
        assert!(page.name_width >= 2 * TREE_INDENT.len() + 2 + name.chars().count());
        assert!(name_cell_text(&page, name).contains(name));
        assert!(!name_cell_text(&page, name).contains(ELLIPSIS));
    }

    #[test]
    fn test_the_name_column_stops_growing_at_its_cap_and_never_shrinks() {
        let mut page = long_named_page(&"x".repeat(100));
        assert_eq!(page.name_width, MAX_NAME_WIDTH);
        let cell = name_cell_text(&page, &"x".repeat(100));
        assert!(cell.contains(ELLIPSIS), "got {cell:?}");
        // A later list of short names does not pull the columns back in.
        page.on_job(JobReply::Processes(Ok(sample_at(
            Instant::now(),
            [0, 0, 0],
        ))));
        assert_eq!(page.name_width, MAX_NAME_WIDTH);
    }

    #[test]
    fn test_a_flat_list_shows_the_bare_name() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('v')));
        assert!(name_cell_text(&page, "worker").starts_with("worker"));
    }

    #[test]
    fn test_a_name_too_long_for_its_column_is_cut_with_a_mark() {
        assert_eq!(fit("short", 10), "short");
        assert_eq!(fit("abcdefghij", 5), format!("abcd{ELLIPSIS}"));
        assert_eq!(fit("abc", 0), "");
    }

    #[test]
    fn test_a_deep_process_keeps_its_name_visible() {
        let page = loaded_page();
        let deep = ViewRow {
            depth: 40,
            folded: false,
            has_children: false,
            index: 0,
        };
        let cell: String = page
            .name_cell(&page.processes[0], &deep)
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        // Forty levels of indent must not push "init" out of the column.
        assert_eq!(cell.chars().count(), page.name_width + 1);
        assert!(cell.contains("init"), "got {cell:?}");
    }

    #[test]
    fn test_sorting_by_name_orders_alphabetically_and_by_command_orders_the_whole_line() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('v')));
        page.sort_by(SortKey::Name);
        let names: Vec<&str> = page
            .rows
            .iter()
            .map(|row| page.processes[row.index].name.as_str())
            .collect();
        assert_eq!(names, ["init", "server", "worker"]);
        page.sort_by(SortKey::Command);
        assert_eq!(page.sort, SortKey::Command);
    }

    #[test]
    fn test_the_content_marks_the_cursor_below_the_two_header_rows() {
        let mut page = loaded_page();
        cursor_on(&mut page, "server");
        let content = page.content(MIN_ROWS_FOR_DETAIL - 1, 120, false);
        assert_eq!(content.cursor_line, Some(HEADER_ROWS + 1));
        assert_eq!(content.rows.len(), HEADER_ROWS + 3);
        let line = row_text(&content.rows[HEADER_ROWS + 1]);
        assert!(line.contains("server --serve"), "got {line:?}");
    }
}

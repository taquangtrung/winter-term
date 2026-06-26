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

use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::{CursorMove, Key};
use crate::model::paint::{row_height, row_text, Frame, Point, Row, Span, Style};
use crate::model::process::{ProcState, ProcessSample};
use crate::model::tool::{
    CommandOutput, JobReply, JobRequest, Notice, PromptMode, PromptReply, PromptRequest,
    SearchReport, Tool, ToolMenuItem, ToolOutcome, menu_groups,
};
use crate::model::vim::nav::{VimKey, VimNav};
use crate::tools::list::ListView;
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

/// The questions the tool asks.
const ASK_CONFIRM: &str = "proc-confirm";
const ASK_FILTER: &str = "proc-filter";
const ASK_RENICE: &str = "proc-renice-value";
const ASK_SEARCH: &str = "proc-search";

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
/// dropped one would leave the tool waiting forever.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// A CPU or memory share at or past this is drawn in the accent color.
const HOT_PERCENT: f64 = 50.0;

/// What the monitor is called in the pane title.
const TITLE: &str = "Processes";

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

/// The monitor's own keys. Refreshing is `C-r` in every tool; `r` stays as
/// the short key.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Refresh,
        keys: &["r", "C-r"],
        label: "Refresh Now",
    },
    Binding {
        action: Act::Pause,
        keys: &["p"],
        label: "Pause / Resume",
    },
    Binding {
        action: Act::Sort,
        keys: &["s"],
        label: "Sort by the Next Column",
    },
    Binding {
        action: Act::Reverse,
        keys: &["S"],
        label: "Reverse the Sort",
    },
    // Not `v`, which starts a text selection in every tool.
    Binding {
        action: Act::View,
        keys: &["="],
        label: "Tree / Flat List",
    },
    Binding {
        action: Act::Fold,
        keys: &["RET", "SPC", "TAB"],
        label: "Fold / Unfold",
    },
    Binding {
        action: Act::Close,
        keys: &["h", "<left>"],
        label: "Fold",
    },
    Binding {
        action: Act::Open,
        keys: &["l", "<right>"],
        label: "Unfold",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Search",
    },
    Binding {
        action: Act::NextMatch,
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Act::PreviousMatch,
        keys: &["N"],
        label: "Previous Match",
    },
    Binding {
        action: Act::Filter,
        keys: &["f"],
        label: "Filter...",
    },
    // Escape first lets go of a filter, and only then closes, so the key
    // that means stop never costs the tool you were reading.
    Binding {
        action: Act::ClearFilter,
        keys: &["ESC"],
        label: "Clear the Filter",
    },
    Binding {
        action: Act::Terminate,
        keys: &["x"],
        label: "Terminate",
    },
    Binding {
        action: Act::Kill,
        keys: &["X"],
        label: "Kill",
    },
    Binding {
        action: Act::TerminateTree,
        keys: &["t"],
        label: "Terminate Tree",
    },
    Binding {
        action: Act::KillTree,
        keys: &["T"],
        label: "Kill Tree",
    },
    Binding {
        action: Act::Suspend,
        keys: &["z"],
        label: "Suspend",
    },
    Binding {
        action: Act::Resume,
        keys: &["Z"],
        label: "Resume",
    },
    Binding {
        action: Act::Renice,
        keys: &["R"],
        label: "Renice...",
    },
    Binding {
        action: Act::CopyPid,
        keys: &["y"],
        label: "Copy PID",
    },
    Binding {
        action: Act::CopyCommand,
        keys: &["Y"],
        label: "Copy Command",
    },
];

/// What the menu opened over a process offers, in its order, a group at a
/// time: ending it, pausing it, its place in the tree, copying it.
const MENU: [&[Act]; 5] = [
    &[Act::Terminate, Act::Kill, Act::TerminateTree, Act::KillTree],
    &[Act::Suspend, Act::Resume],
    &[Act::Renice],
    &[Act::Fold],
    &[Act::CopyPid, Act::CopyCommand],
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the monitor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    ClearFilter,
    Close,
    CopyCommand,
    CopyPid,
    Filter,
    Fold,
    Kill,
    KillTree,
    NextMatch,
    Open,
    Pause,
    PreviousMatch,
    Refresh,
    Renice,
    Resume,
    Reverse,
    Search,
    Sort,
    Suspend,
    Terminate,
    TerminateTree,
    View,
}

/// A process monitor.
#[derive(Debug)]
pub struct ProcTool {
    /// What each signal or priority change in flight is reported as.
    acting: Option<String>,
    /// Tree parents whose children are folded away.
    collapsed: HashSet<u32>,
    cpu: CpuTracker,
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
    /// What the last action reported, waiting for the status bar to take it.
    notice: Option<Notice>,
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
    /// The cursor over the rows, keyed by process id, so a refresh that
    /// reorders the list leaves the cursor on the same process rather than
    /// the same row.
    list: ListView<u32>,
    sort: SortKey,
    nav: VimNav,
    /// The screen height of each list row last painted, and the list row the
    /// first of them is, so a click on the pane maps back to a process.
    painted: Vec<usize>,
    painted_start: usize,
    /// When snapshots are asked for.
    timer: RefreshTimer,
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
// ProcTool
// ========================================================================

impl ProcTool {
    /// A monitor that has read nothing yet and asks for its first snapshot on
    /// the next tick.
    pub fn new() -> Self {
        let sort = SortKey::Cpu;
        Self {
            acting: None,
            collapsed: HashSet::new(),
            cpu: CpuTracker::new(),
            direction: sort.default_direction(),
            error: None,
            filter: String::new(),
            has_gpu: false,
            loaded: false,
            notice: None,
            mode: ViewMode::Tree,
            name_width: MIN_NAME_WIDTH,
            pending_signal: None,
            pending_renice: None,
            processes: Vec::new(),
            rows: Vec::new(),
            list: ListView::new(),
            sort,
            nav: VimNav::new(),
            painted: Vec::new(),
            painted_start: 0,
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
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
        let pids = self
            .rows
            .iter()
            .map(|row| self.processes[row.index].pid)
            .collect();
        self.list.set_keys(pids);
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
        let row = self.rows.get(self.list.cursor())?;
        self.processes.get(row.index)
    }

    /// Put the cursor on row `at`, or the nearest row there is.
    fn move_to(&mut self, at: usize) {
        self.list.set_cursor(at);
    }

    /// Interpret a motion of the shared Vim layer over the rows. The word,
    /// paragraph, and column motions mean nothing over a list of processes.
    fn apply_motion(&mut self, motion: CursorMove) {
        self.list.apply_motion(motion);
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
        let Some(row) = self.rows.get(self.list.cursor()).copied() else {
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
    fn refresh_now(&mut self) -> ToolOutcome {
        self.timer.request_now(Instant::now());
        ToolOutcome::Job(JobRequest::Processes)
    }

    /// Send a signal to the selected process: stop and continue at once, and
    /// the ones that end a process after the user has said yes.
    fn signal_selected(&mut self, signal: Signal, tree: bool) -> ToolOutcome {
        let Some((pid, label)) = self.selected().map(|p| (p.pid, p.name.clone())) else {
            return ToolOutcome::Consumed;
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
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: format!("{verb} {what}? (y/n) "),
            mode: PromptMode::Confirm,
            tag: ASK_CONFIRM,
        })
    }

    /// Run a signal that has been decided on.
    fn send_signal(&mut self, pending: PendingSignal) -> ToolOutcome {
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
                ToolOutcome::Job(JobRequest::Command(request))
            }
            Err(reason) => {
                self.notice = Some(Notice::error(reason));
                ToolOutcome::Consumed
            }
        }
    }

    /// Ask what priority the selected process should have.
    fn ask_renice(&mut self) -> ToolOutcome {
        let Some(pid) = self.selected().map(|process| process.pid) else {
            return ToolOutcome::Consumed;
        };
        self.pending_renice = Some(pid);
        let label = if cfg!(windows) {
            RENICE_LABEL_WINDOWS
        } else {
            RENICE_LABEL_UNIX
        };
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag: ASK_RENICE,
        })
    }

    /// Ask what to move the cursor to.
    fn ask_search(&self) -> ToolOutcome {
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "/".to_string(),
            mode: PromptMode::Search { reverse: false },
            tag: ASK_SEARCH,
        })
    }

    /// Remember `query` and move to the next process it matches.
    fn search_for(&mut self, query: &str) -> ToolOutcome {
        let labels = self.search_labels();
        if let Some(notice) = self.list.search_for(&labels, query.trim()) {
            self.notice = Some(notice);
        }
        ToolOutcome::Consumed
    }

    /// Move to the next listed process matching the last search, looking one
    /// past the cursor and wrapping so the cursor's own row is tried last, or
    /// report that there is nothing to move to.
    fn search_step(&mut self, forward: bool) -> ToolOutcome {
        let labels = self.search_labels();
        if let Some(notice) = self.list.search_step(&labels, forward) {
            self.notice = Some(notice);
        }
        ToolOutcome::Consumed
    }

    /// What a search reads over, one label per listed process.
    fn search_labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| self.processes[row.index].search_text())
            .collect()
    }

    /// Ask what the list should be narrowed to.
    fn ask_filter(&self) -> ToolOutcome {
        ToolOutcome::Prompt(PromptRequest {
            initial: self.filter.clone(),
            label: "filter: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_FILTER,
        })
    }

    /// What a finished signal or priority change reports.
    fn on_command_done(&mut self, output: &CommandOutput) -> ToolOutcome {
        let done = self.acting.take();
        self.notice = Some(match (output.succeeded(), done) {
            (true, Some(what)) => Notice::info(what),
            (true, None) => Notice::info("done"),
            (false, _) => Notice::error(format!("failed: {}", output.failure())),
        });
        // The list is about to be wrong, so the next snapshot is not left to
        // the timer.
        self.timer.refresh_soon(Instant::now());
        ToolOutcome::Consumed
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
    fn summary_row(&self) -> Row {
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
        let mut row = vec![Span::new(Style::Header, text)];
        if let Some(report) = &self.error {
            row.push(Span::new(Style::Accent, format!("  {report}")));
        }
        row
    }

    /// The column headings, with the sorted one marked.
    fn column_row(&self) -> Row {
        let heading = |label: &str, sorted: bool| {
            if sorted {
                format!("{}{label}", self.direction.arrow())
            } else {
                label.to_string()
            }
        };
        let style = |sorted: bool| {
            if sorted {
                Style::HeaderAccent
            } else {
                Style::Header
            }
        };
        let mut row: Row = self
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
                Span::new(style(sorted), cell)
            })
            .collect();
        let sorted = self.sort == SortKey::Command;
        row.push(Span::new(style(sorted), heading(COMMAND_LABEL, sorted)));
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
    fn detail_rows(&self, cols: usize) -> Vec<Row> {
        let Some(process) = self.selected() else {
            return Vec::new();
        };
        let field = |label: &str, value: String| {
            [
                Span::new(Style::Dim, format!("{label} ")),
                Span::plain(format!("{value}  ")),
            ]
        };
        let mut facts: Row = Vec::new();
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
        let mut places: Row = Vec::new();
        if let Some(path) = &process.exec_path {
            places.extend(field("PATH", path.clone()));
        }
        if let Some(cwd) = &process.cwd {
            places.extend(field("CWD", cwd.clone()));
        }
        let command = vec![
            Span::new(Style::Dim, "CMD "),
            Span::plain(process.command.clone()),
        ];
        vec![
            clip_row(facts, cols),
            clip_row(places, cols),
            clip_row(command, cols),
        ]
    }

    /// The NAME cell: the tree's indent and fold marker, then the name, cut to
    /// fit and padded to the column.
    fn name_cell(&self, process: &Process, row: &ViewRow) -> [Span; 2] {
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
            Span::new(Style::Dim, prefix),
            Span::plain(format!("{name}{} ", " ".repeat(padding))),
        ]
    }

    /// One process as a line.
    fn process_row(&self, row: &ViewRow) -> Row {
        let process = &self.processes[row.index];
        let user = process.user.as_deref().unwrap_or("-");
        let hot = |percent: f64| {
            if percent >= HOT_PERCENT {
                Style::Accent
            } else {
                Style::Normal
            }
        };
        let vram = process.gpu_kb.map_or(NO_VRAM.to_string(), format_kb);
        let mut spans = vec![Span::new(
            Style::Dim,
            format!("{:>PID_WIDTH$} ", process.pid),
        )];
        spans.extend(self.name_cell(process, row));
        spans.push(Span::new(
            hot(process.cpu_percent),
            format!("{:>CPU_WIDTH$} ", format_percent(process.cpu_percent)),
        ));
        spans.push(Span::new(
            hot(process.mem_percent),
            format!("{:>MEM_WIDTH$} ", format_percent(process.mem_percent)),
        ));
        spans.push(Span::plain(format!(
            "{:>RSS_WIDTH$} ",
            format_kb(process.rss_kb)
        )));
        if self.has_gpu {
            spans.push(Span::plain(format!("{vram:>VRAM_WIDTH$} ")));
        }
        spans.push(Span::new(
            state_style(process.state),
            format!("{:<STATE_WIDTH$} ", state_label(process.state)),
        ));
        spans.push(Span::new(
            Style::Dim,
            format!("{user:<USER_WIDTH$.USER_WIDTH$} "),
        ));
        spans.push(Span::plain(process.command.clone()));
        spans
    }
}

impl Default for ProcTool {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Tool
// ========================================================================

impl Tool for ProcTool {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        // The view moves and the cursor stays, as in an editor.
        let total = self.rows.len();
        self.list.view_mut().wheel(lines, total);
        ToolOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let detail_rows = if rows >= MIN_ROWS_FOR_DETAIL {
            DETAIL_ROWS
        } else {
            0
        };
        let viewport = rows.saturating_sub(HEADER_ROWS + detail_rows);
        self.list.view_mut().set_rows(viewport);
        // Neither heading row may wrap: a click is told which screen row it
        // landed on, and the headings are only on screen row one if the
        // summary above them stays on a single line.
        let mut frame_rows = vec![
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
            frame_rows.push(vec![Span::new(Style::Dim, note)]);
            return Frame::new(frame_rows);
        }
        // The cursor is taken out for the layout, which measures rows by
        // painting them from the rest of the tool.
        let mut list = std::mem::take(&mut self.list);
        let height_of = |index: usize| {
            row_height(
                &row_text(&self.process_row(&self.rows[index])),
                cols,
                wrap,
                fixed,
            )
        };
        let placed = list.place(viewport, height_of);
        let window = placed.window;
        let shown = window.start..window.start + window.count;
        let heights: Vec<usize> = shown.clone().map(height_of).collect();
        self.list = list;
        self.painted = heights;
        self.painted_start = window.start;
        frame_rows.extend(self.rows[shown].iter().map(|row| self.process_row(row)));
        let mut indents = vec![0; HEADER_ROWS];
        indents.extend(std::iter::repeat_n(fixed, window.count));
        if detail_rows > 0 {
            // Blank rows keep the detail strip at the bottom of the pane
            // however few processes there are.
            let used: usize = self.painted.iter().sum();
            let padding = viewport.saturating_sub(used);
            frame_rows.extend(std::iter::repeat_n(Row::new(), padding));
            indents.extend(std::iter::repeat_n(0, padding));
            let detail = self.detail_rows(cols);
            indents.extend(std::iter::repeat_n(0, detail.len()));
            frame_rows.extend(detail);
        }
        let content = Frame::new(frame_rows).with_wrap_indents(indents);
        match placed.shows_cursor {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        self.list.view_mut().release();
        if self.nav.in_sequence() || key.alt {
            return self.on_motion_key(key);
        }
        match table::find(KEYS, key).and_then(|command| self.run(command)) {
            Some(outcome) => outcome,
            None => self.on_motion_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_mouse(&mut self, at: Point) -> ToolOutcome {
        // Only a press on a heading or a process means anything here; the
        // rest of the pane is left to the terminal's own selection.
        if at.drag {
            return ToolOutcome::Ignored;
        }
        self.list.view_mut().release();
        if at.row == COLUMN_ROW {
            let key = self.sort_key_at(at.col);
            self.sort_on_click(key);
            return ToolOutcome::Consumed;
        }
        match self.list_row_at(at.row) {
            Some(index) => {
                self.move_to(index);
                ToolOutcome::Consumed
            }
            None => ToolOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        self.list.view_mut().release();
        let answer = reply.answer;
        match reply.tag {
            ASK_CONFIRM => {
                let pending = self.pending_signal.take();
                match (answer, pending) {
                    (Some(_), Some(pending)) => self.send_signal(pending),
                    _ => ToolOutcome::Consumed,
                }
            }
            ASK_FILTER => {
                if let Some(text) = answer {
                    self.filter = text.trim().to_string();
                    self.rebuild();
                }
                ToolOutcome::Consumed
            }
            ASK_SEARCH => match answer {
                Some(text) => self.search_for(&text),
                None => ToolOutcome::Consumed,
            },
            ASK_RENICE => {
                let pid = self.pending_renice.take();
                let (Some(value), Some(pid)) = (answer, pid) else {
                    return ToolOutcome::Consumed;
                };
                match act::renice_request(pid, &value, cfg!(windows)) {
                    Ok(request) => {
                        self.acting = Some(format!("set priority of {pid} to {}", value.trim()));
                        ToolOutcome::Job(JobRequest::Command(request))
                    }
                    Err(reason) => {
                        self.notice = Some(Notice::error(reason));
                        ToolOutcome::Consumed
                    }
                }
            }
            _ => ToolOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> ToolOutcome {
        if tag == ASK_FILTER {
            self.filter = query.trim().to_string();
            self.rebuild();
            ToolOutcome::Consumed
        } else if tag == ASK_SEARCH && !query.trim().is_empty() {
            self.search_for(query)
        } else {
            ToolOutcome::Ignored
        }
    }

    fn on_job(&mut self, reply: JobReply) -> ToolOutcome {
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
            | JobReply::FileOps(_)
            | JobReply::Search(_)
            | JobReply::System(_) => {}
        }
        ToolOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> ToolOutcome {
        if !self.timer.on_tick(now) {
            return ToolOutcome::Consumed;
        }
        ToolOutcome::Job(JobRequest::Processes)
    }

    fn on_resume(&mut self) -> ToolOutcome {
        // Work asked for before the tool was covered was cancelled with it.
        self.timer.restart(Instant::now());
        ToolOutcome::Consumed
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.selected()
            .and_then(|process| process.cwd.as_ref())
            .map(PathBuf::from)
    }

    fn context_items(&self) -> Vec<ToolMenuItem> {
        if self.selected().is_none() {
            return Vec::new();
        }
        menu_groups(MENU.into_iter().map(|group| {
            group
                .iter()
                .filter_map(|&command| table::menu_entry(KEYS, command))
                .map(|(key, label)| ToolMenuItem::new(key, label))
                .collect()
        }))
    }
}

// ========================================================================
// Helpers
// ========================================================================

impl ProcTool {
    /// Do what one of the monitor's own keys asks. `None` for a key that has
    /// nothing to do in the state the monitor is in, which then reaches the
    /// motion layer.
    fn run(&mut self, command: Act) -> Option<ToolOutcome> {
        let outcome = match command {
            Act::Refresh => self.refresh_now(),
            Act::Pause => {
                self.timer.toggle_pause(Instant::now());
                ToolOutcome::Consumed
            }
            Act::Sort => {
                let mut next = self.sort.next();
                // A column that is not shown is not one to land on.
                if next == SortKey::Gpu && !self.has_gpu {
                    next = next.next();
                }
                self.sort_by(next);
                ToolOutcome::Consumed
            }
            Act::Reverse => {
                self.direction = self.direction.reversed();
                self.rebuild();
                ToolOutcome::Consumed
            }
            Act::View => {
                self.mode = match self.mode {
                    ViewMode::Flat => ViewMode::Tree,
                    ViewMode::Tree => ViewMode::Flat,
                };
                self.rebuild();
                ToolOutcome::Consumed
            }
            Act::Fold => {
                self.set_folded(None);
                ToolOutcome::Consumed
            }
            Act::Close => {
                self.set_folded(Some(true));
                ToolOutcome::Consumed
            }
            Act::Open => {
                self.set_folded(Some(false));
                ToolOutcome::Consumed
            }
            Act::Search => self.ask_search(),
            Act::NextMatch => self.search_step(true),
            Act::PreviousMatch => self.search_step(false),
            Act::Filter => self.ask_filter(),
            Act::ClearFilter if self.filter.is_empty() => return None,
            Act::ClearFilter => {
                self.filter.clear();
                self.rebuild();
                ToolOutcome::Consumed
            }
            Act::Terminate => self.signal_selected(Signal::Terminate, false),
            Act::Kill => self.signal_selected(Signal::Kill, false),
            Act::TerminateTree => self.signal_selected(Signal::Terminate, true),
            Act::KillTree => self.signal_selected(Signal::Kill, true),
            Act::Suspend => self.signal_selected(Signal::Stop, false),
            Act::Resume => self.signal_selected(Signal::Continue, false),
            Act::Renice => self.ask_renice(),
            Act::CopyPid => match self.selected() {
                Some(process) => ToolOutcome::Yank(process.pid.to_string()),
                None => ToolOutcome::Consumed,
            },
            Act::CopyCommand => match self.selected() {
                Some(process) => ToolOutcome::Yank(process.command.clone()),
                None => ToolOutcome::Consumed,
            },
        };
        Some(outcome)
    }

    /// Offer a key to the shared Vim motion layer, and interpret what it
    /// resolves to over the rows.
    fn on_motion_key(&mut self, key: &Key) -> ToolOutcome {
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                ToolOutcome::Consumed
            }
            VimKey::Pending => ToolOutcome::Consumed,
            VimKey::Unhandled => ToolOutcome::Ignored,
        }
    }
}

/// `row` cut to `cols` cells, so it takes one screen row whatever the pane's
/// width and the host's wrapping.
fn clip_row(row: Row, cols: usize) -> Row {
    let mut room = cols;
    let mut clipped = Vec::with_capacity(row.len());
    for span in row {
        if room == 0 {
            break;
        }
        let text: String = span.text.chars().take(room).collect();
        room -= text.chars().count();
        clipped.push(Span::new(span.style, text));
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
fn state_style(state: ProcState) -> Style {
    match state {
        ProcState::Running => Style::ChangeAdded,
        ProcState::Sleeping | ProcState::Unknown => Style::Dim,
        ProcState::Stopped => Style::ChangeModified,
        ProcState::Zombie => Style::ChangeDeleted,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

//! Process monitor: every process on the machine as a tree or a sorted list,
//! kept current on a timer, with the actions of a task manager (terminate,
//! kill, suspend, resume, renice) a key away.
//!
//! - [`act`]: the commands that act on a process.
//! - [`collect`]: taking a snapshot, the one side-effecting module.
//! - [`cpu`]: CPU use worked out from two snapshots.
//! - [`details`]: reading one process's files, sockets, and memory.
//! - [`format`]: wording sizes and percentages.
//! - [`parse`]: reading each system's process listing.
//! - [`view`]: the order the list is shown in.
//!
//! Misty draws the monitor as a view: a summary over a table whose headings
//! sort on a click and whose rows put the cursor where they are clicked,
//! the rows that fit around the cursor built and no more, and a footer
//! describing the process the cursor is on. `Enter` looks into that
//! process ([`details`], drawn by `details_view`) over the list.

pub mod act;
pub mod collect;
pub mod cpu;
pub mod details;
mod details_view;
pub mod notice;
pub mod parse;
pub mod view;

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use misty::view::{
    LazySource, Space, Text, badge, column, divider, lazy_column, row, text, untitled_page,
};
use misty::{
    AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect, ScrollStep,
    Theme, ThemeToken,
};

use winter_model::editor::refresh::RefreshTimer;
use winter_model::editor::{
    CommandOutput, EditorMenuItem, EditorOutcome, JobReply, JobRequest, Notice, OwnView, Page,
    PageView, PromptMode, PromptReply, PromptRequest, SearchReport, ViewSlot, menu_of,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{CursorMove, Key, format_key};
use winter_model::list::{ListView, Viewport};
use winter_model::monitor::{MonitorAnswer, MonitorQuery};
use winter_model::paint::{Frame, Row};
use winter_model::process::{ProcState, ProcessSample};
use winter_model::vim::nav::{VimKey, VimNav};

use self::act::{Signal, TAG_RENICE, TAG_SIGNAL};
use self::cpu::CpuTracker;
use self::details_view::{BODY as DETAILS_BODY, Details, SECTIONS};
use self::view::{
    Process, SortDirection, SortKey, ViewMode, ViewOptions, ViewRow, descendants, layout,
};
use crate::table::{ROW_PADDING_COLS, cell, fit, heading as sort_heading, marked, table_row};
use crate::units::{format_kb, format_percent};
use notice::{Acting, ProcNotice, Renice, Signalled};

// ========================================================================
// Constants
// ========================================================================

/// The questions the editor asks.
const ASK_CONFIRM: &str = "proc-confirm";

const ASK_FILTER: &str = "proc-filter";
const ASK_RENICE: &str = "proc-renice-value";
const ASK_SEARCH: &str = "proc-search";

/// The pane height below which the footer describing the selected process
/// is left out, so the list keeps what room there is.
const MIN_ROWS_FOR_DETAIL: usize = 20;

/// What the list says with nothing to show once a snapshot is in.
const NO_MATCH: &str = "no matching processes";

/// How long after a snapshot arrives the next is asked for. Long enough for a
/// CPU rate to mean something, short enough to feel live.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);

/// How long a snapshot may be outstanding before it is given up on and asked
/// for again. The runner drops a request when it is busy, so without this a
/// dropped one would leave the editor waiting forever.
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// A CPU share at or past this is drawn in the warning color: half the
/// machine is a process worth a look.
const HOT_PERCENT: f64 = 50.0;

/// What the monitor is called in the pane title.
const TITLE: &str = "Process Monitor";

/// The label of the renice question on each family of systems.
const RENICE_LABEL_UNIX: &str = "nice value (-20 to 19): ";
const RENICE_LABEL_WINDOWS: &str =
    "priority (Idle, BelowNormal, Normal, AboveNormal, High, RealTime): ";

/// Column widths, in cells. A column is followed by one space.
const PID_WIDTH: usize = 7;
const USER_WIDTH: usize = 10;
const CPU_WIDTH: usize = 6;
const MEM_WIDTH: usize = 7;
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

/// What the video memory column shows for a process no GPU editor lists.
const NO_VRAM: &str = "-";

/// The heading of the last column, which takes whatever width is left.
const COMMAND_LABEL: &str = "COMMAND";

/// The other columns' headings, which also name them.
const PID_LABEL: &str = "PID";
const NAME_LABEL: &str = "NAME";
const CPU_LABEL: &str = "CPU%";
const MEM_LABEL: &str = "MEM";
const VRAM_LABEL: &str = "VRAM";
const STATE_LABEL: &str = "STATE";
const USER_LABEL: &str = "USER";

/// The columns a narrow pane leaves out, the first first, until the rest
/// fit; the id, the name, and the CPU share always stay.
const DROP_ORDER: [&str; 4] = [VRAM_LABEL, USER_LABEL, MEM_LABEL, STATE_LABEL];

/// The text columns the command keeps at the least, and those the page's
/// padding takes.
const MIN_COMMAND_COLS: usize = 16;
const PAGE_PADDING_COLS: usize = 6;

/// Cells a level of the tree indents by.
const TREE_INDENT: &str = "  ";

/// The marker before a process with children, folded and not.
const MARKER_FOLDED: char = '\u{25b8}';
const MARKER_OPEN: char = '\u{25be}';

/// Spacing in the selected process footer: between the name, the state
/// badge, and the command line, and between the footer's lines.
const DETAIL_TITLE_GAP_COLS: f32 = 1.0;
const DETAIL_GAP_ROWS: f32 = 0.2;

/// The footer's grid of facts, column by column: its ids, its owner and
/// state, its shares, and its places. Every cell is always there, `-` where
/// the system does not tell, so the footer is the same size whatever the
/// cursor is on.
const DETAIL_COLUMNS: usize = 5;
const DETAIL_ROWS: usize = 2;
const DETAIL_LABELS: [[&str; DETAIL_ROWS]; DETAIL_COLUMNS] = [
    ["PID", "PPID"],
    ["USER", "STATE"],
    ["CPU", "MEM"],
    ["VRAM", "CHILDREN"],
    ["PATH", "CWD"],
];

/// How wide a value in each of the grid's columns but the last is, in
/// cells: a parent's id and name, a user, a size and its share, a size or
/// a count. Kept tight, as a cell already leaves a column after its value,
/// so the last, the paths, takes as much as the room left. Then the gap
/// after a column.
const DETAIL_VALUE_COLS: [usize; DETAIL_COLUMNS - 1] = [16, 8, 12, 6];
const DETAIL_COLUMN_GAP_COLS: f32 = 1.0;

/// What a fact the system does not tell shows.
const NO_FACT: &str = "-";

/// What the footer's title says with no process under the cursor.
const NO_SELECTION: &str = "no process selected";

/// The monitor's own keys. Refreshing is `C-r` in every editor; `r` stays as
/// the short key.
const KEYS: KeyTable<Act> = KeyTable {
    block: "proc",
    bindings: &[
        Binding {
            action: Act::Refresh,
            id: "refresh",
            keys: &["r", "C-r", "M-S-r"],
            label: "Refresh Now",
        },
        Binding {
            action: Act::Pause,
            id: "pause",
            keys: &["p"],
            label: "Pause / Resume",
        },
        Binding {
            action: Act::Sort,
            id: "sort",
            keys: &["s"],
            label: "Sort by the Next Column",
        },
        Binding {
            action: Act::Reverse,
            id: "reverse",
            keys: &["S"],
            label: "Reverse the Sort",
        },
        // Not `v`, which starts a text selection in every editor.
        Binding {
            action: Act::View,
            id: "view",
            keys: &["="],
            label: "Tree / Flat List",
        },
        Binding {
            action: Act::Details,
            id: "details",
            keys: &["RET"],
            label: "Look Into the Process",
        },
        Binding {
            action: Act::Fold,
            id: "fold",
            keys: &["TAB"],
            label: "Fold / Unfold",
        },
        Binding {
            action: Act::Close,
            id: "close",
            keys: &["h", "<left>"],
            label: "Fold",
        },
        Binding {
            action: Act::Open,
            id: "open",
            keys: &["l", "<right>"],
            label: "Unfold",
        },
        Binding {
            action: Act::Search,
            id: "search",
            keys: &["/"],
            label: "Search",
        },
        Binding {
            action: Act::NextMatch,
            id: "next_match",
            keys: &["n"],
            label: "Next Match",
        },
        Binding {
            action: Act::PreviousMatch,
            id: "previous_match",
            keys: &["N"],
            label: "Previous Match",
        },
        Binding {
            action: Act::Filter,
            id: "filter",
            keys: &["f"],
            label: "Filter…",
        },
        // Escape first lets go of a filter, and only then closes, so the key
        // that means stop never costs the editor you were reading.
        Binding {
            action: Act::ClearFilter,
            id: "clear_filter",
            keys: &["ESC"],
            label: "Clear the Filter",
        },
        Binding {
            action: Act::Terminate,
            id: "terminate",
            keys: &["x"],
            label: "Terminate",
        },
        Binding {
            action: Act::Kill,
            id: "kill",
            keys: &["X"],
            label: "Kill",
        },
        Binding {
            action: Act::TerminateTree,
            id: "terminate_tree",
            keys: &["t"],
            label: "Terminate Tree",
        },
        Binding {
            action: Act::KillTree,
            id: "kill_tree",
            keys: &["T"],
            label: "Kill Tree",
        },
        Binding {
            action: Act::Suspend,
            id: "suspend",
            keys: &["z"],
            label: "Suspend",
        },
        Binding {
            action: Act::Resume,
            id: "resume",
            keys: &["Z"],
            label: "Resume",
        },
        Binding {
            action: Act::Renice,
            id: "renice",
            keys: &["R"],
            label: "Renice…",
        },
        Binding {
            action: Act::CopyPid,
            id: "copy_pid",
            keys: &["y"],
            label: "Copy PID",
        },
        Binding {
            action: Act::CopyCommand,
            id: "copy_command",
            keys: &["Y"],
            label: "Copy Command",
        },
    ],
    configurable: true,
    title: "Processes",
};

/// The keys while the monitor looks into one process. Refreshing is `C-r`
/// in every editor; `q` and Escape go back to the list, as a Git tab's `q`
/// does.
const DETAIL_KEYS: KeyTable<DetailAct> = KeyTable {
    block: "proc_details",
    bindings: &[
        Binding {
            action: DetailAct::Back,
            id: "back",
            keys: &["q", "ESC"],
            label: "Back to the List",
        },
        Binding {
            action: DetailAct::Reload,
            id: "reload",
            keys: &["r", "C-r", "M-S-r"],
            label: "Read Again",
        },
        Binding {
            action: DetailAct::NextSection,
            id: "next_section",
            keys: &["]"],
            label: "Next Section",
        },
        Binding {
            action: DetailAct::PreviousSection,
            id: "previous_section",
            keys: &["["],
            label: "Previous Section",
        },
        Binding {
            action: DetailAct::LineDown,
            id: "line_down",
            keys: &["C-e"],
            label: "Scroll a Line Down",
        },
        Binding {
            action: DetailAct::LineUp,
            id: "line_up",
            keys: &["C-y"],
            label: "Scroll a Line Up",
        },
        Binding {
            action: DetailAct::CopyPid,
            id: "copy_pid",
            keys: &["y"],
            label: "Copy PID",
        },
        Binding {
            action: DetailAct::CopyCommand,
            id: "copy_command",
            keys: &["Y"],
            label: "Copy Command",
        },
    ],
    configurable: true,
    title: "Process Details",
};

/// Every key table of this surface, for the Keys editor to list.
pub const KEY_TABLES: &[&dyn Listed] = &[&KEYS, &DETAIL_KEYS];

/// A half of a screen, for the half-page motions over the details.
const HALF_PAGE: f32 = 0.5;

/// How far past a section's top the details must be for `[` to go back to
/// that section rather than the one before it, in pixels.
const SECTION_SLACK: f32 = 1.0;

/// What the menu opened over a process offers, in its order, a group at a
/// time: ending it, pausing it, its place in the tree, copying it.
const MENU: [&[Act]; 6] = [
    &[Act::Details],
    &[Act::Terminate, Act::Kill, Act::TerminateTree, Act::KillTree],
    &[Act::Suspend, Act::Resume],
    &[Act::Renice],
    &[Act::Fold],
    &[Act::CopyPid, Act::CopyCommand],
];

/// What the menu opened over a process looked into offers.
const DETAIL_MENU: [&[DetailAct]; 3] = [
    &[DetailAct::Back, DetailAct::Reload],
    &[DetailAct::NextSection, DetailAct::PreviousSection],
    &[DetailAct::CopyPid, DetailAct::CopyCommand],
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
    Details,
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

/// What a key does while the monitor looks into one process.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DetailAct {
    Back,
    CopyCommand,
    CopyPid,
    LineDown,
    LineUp,
    NextSection,
    PreviousSection,
    Reload,
}

/// A process monitor.
#[derive(Debug)]
pub struct ProcEditor {
    /// What each signal or priority change in flight is reported as.
    acting: Option<Acting>,
    /// Tree parents whose children are folded away.
    collapsed: HashSet<u32>,
    cpu: CpuTracker,
    /// The process looked into, shown over the list until the user goes
    /// back to it.
    details: Option<PageView<Details>>,
    direction: SortDirection,
    /// Why the last snapshot failed, until one succeeds.
    error: Option<String>,
    filter: String,
    /// Whether any snapshot has listed video memory, which is what earns the
    /// column its place. It stays once seen, so one failed query does not make
    /// the columns jump.
    has_gpu: bool,
    /// How many text columns wide the pane is, which says which columns
    /// fit.
    cols: usize,
    /// Whether a snapshot has arrived, so an empty list can say why.
    loaded: bool,
    /// Whether the tab is shown, which with the details closed is what
    /// keeps the list refreshing.
    shown: bool,
    /// What the last action reported, waiting for the status bar to take it.
    notice: Option<ProcNotice>,
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
    /// Whether the pane is tall enough for the card describing the
    /// selected process.
    detail: bool,
    /// When snapshots are asked for.
    timer: RefreshTimer,
    /// The page's view, which the monitor is the state of.
    view: ViewSlot<ProcEditor>,
}

/// A signal waiting on the answer to "are you sure".
#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingSignal {
    /// The command line the process had when it was chosen, so a pid that
    /// ended and was handed to another program is not signalled in its place.
    command: String,
    pid: u32,
    signal: Signal,
    tree: bool,
}

/// One of the columns before the command: how it is headed, how wide it is,
/// and what clicking its heading sorts by.
#[derive(Clone, Copy, Debug)]
struct Column {
    /// What the column holds in words, for its heading's tooltip.
    about: &'static str,
    label: &'static str,
    right: bool,
    sort: SortKey,
    width: usize,
}

// ========================================================================
// ProcEditor
// ========================================================================

impl ProcEditor {
    /// A monitor that has read nothing yet and asks for its first snapshot on
    /// the next tick.
    pub fn new() -> Self {
        let sort = SortKey::Cpu;
        Self {
            acting: None,
            collapsed: HashSet::new(),
            cpu: CpuTracker::new(),
            details: None,
            direction: sort.default_direction(),
            error: None,
            filter: String::new(),
            has_gpu: false,
            cols: usize::MAX,
            loaded: false,
            shown: true,
            notice: None,
            mode: ViewMode::Flat,
            name_width: MIN_NAME_WIDTH,
            pending_signal: None,
            pending_renice: None,
            processes: Vec::new(),
            rows: Vec::new(),
            list: ListView::new(),
            sort,
            nav: VimNav::new(),
            detail: false,
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
            view: ViewSlot::new(),
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
    fn refresh_now(&mut self) -> EditorOutcome {
        self.timer.request_now(Instant::now());
        EditorOutcome::Job(JobRequest::Processes)
    }

    /// Look into the selected process, the list set aside and not read
    /// while it is.
    fn open_details(&mut self) -> EditorOutcome {
        let Some(process) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let details = Details {
            command: process.command.clone(),
            info: None,
            name: process.name.clone(),
            pid: process.pid,
        };
        let pid = details.pid;
        self.details = Some(PageView::new(details));
        self.sync_timer();
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Details(pid)))
    }

    /// Go back to the list, read again at once.
    fn close_details(&mut self) {
        self.details = None;
        self.sync_timer();
    }

    /// Keep the list's readings to when it is in sight: the tab shown and
    /// no process looked into over it.
    fn sync_timer(&mut self) {
        let hidden = !self.shown || self.details.is_some();
        self.timer.set_hidden(hidden, Instant::now());
    }

    /// Do what a key asks while a process is looked into.
    fn on_details_key(&mut self, key: &Key) -> EditorOutcome {
        if !self.nav.in_sequence()
            && let Some(act) = table::find(&DETAIL_KEYS, key)
        {
            return self.run_details(act);
        }
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                let step = match motion {
                    CursorMove::Down => ScrollStep::Rows(1.0),
                    CursorMove::Up => ScrollStep::Rows(-1.0),
                    CursorMove::Top => ScrollStep::Start,
                    CursorMove::Bottom => ScrollStep::End,
                    CursorMove::HalfPageDown => ScrollStep::Pages(HALF_PAGE),
                    CursorMove::HalfPageUp => ScrollStep::Pages(-HALF_PAGE),
                    CursorMove::PageDown => ScrollStep::Pages(1.0),
                    CursorMove::PageUp => ScrollStep::Pages(-1.0),
                    _ => return EditorOutcome::Consumed,
                };
                self.scroll_details(step);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    /// Do what one of the details' own keys asks.
    fn run_details(&mut self, act: DetailAct) -> EditorOutcome {
        let Some(view) = &mut self.details else {
            return EditorOutcome::Consumed;
        };
        let state = view.state();
        match act {
            DetailAct::Back => {
                self.close_details();
                return EditorOutcome::Consumed;
            }
            DetailAct::Reload => {
                let pid = state.pid;
                return EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Details(pid)));
            }
            DetailAct::CopyPid => return EditorOutcome::Yank(state.pid.to_string()),
            DetailAct::CopyCommand => return EditorOutcome::Yank(state.command.clone()),
            DetailAct::NextSection => self.jump_section(true),
            DetailAct::PreviousSection => self.jump_section(false),
            DetailAct::LineDown => self.scroll_details(ScrollStep::Rows(1.0)),
            DetailAct::LineUp => self.scroll_details(ScrollStep::Rows(-1.0)),
        }
        EditorOutcome::Consumed
    }

    /// Scroll the details by `step`.
    fn scroll_details(&mut self, step: ScrollStep) {
        if let Some(view) = &mut self.details {
            view.scroll(DETAILS_BODY, step);
        }
    }

    /// Bring the next section of the details to the top, or the one before
    /// the section at the top.
    fn jump_section(&mut self, forward: bool) {
        let Some(view) = &mut self.details else {
            return;
        };
        let Some(at) = view.offset(DETAILS_BODY) else {
            return;
        };
        let tops = SECTIONS
            .iter()
            .filter_map(|id| Some((*id, view.top_of(id)?)));
        let target = match forward {
            true => tops
                .filter(|(_, top)| *top > at + SECTION_SLACK)
                .min_by(|a, b| a.1.total_cmp(&b.1)),
            false => tops
                .filter(|(_, top)| *top < at - SECTION_SLACK)
                .max_by(|a, b| a.1.total_cmp(&b.1)),
        };
        if let Some((id, _)) = target {
            view.reveal_top(id);
        }
    }

    /// Send a signal to the selected process: stop and continue at once, and
    /// the ones that end a process after the user has said yes.
    fn signal_selected(&mut self, signal: Signal, tree: bool) -> EditorOutcome {
        let Some((pid, label, command)) = self
            .selected()
            .map(|p| (p.pid, p.name.clone(), p.command.clone()))
        else {
            return EditorOutcome::Consumed;
        };
        if matches!(signal, Signal::Continue | Signal::Stop) {
            return self.send_signal(PendingSignal {
                command,
                pid,
                signal,
                tree,
            });
        }
        let what = if tree {
            format!("{label} (pid {pid}) and everything under it")
        } else {
            format!("{label} (pid {pid})")
        };
        self.pending_signal = Some(PendingSignal {
            command,
            pid,
            signal,
            tree,
        });
        let question = format!("{} {what}?", signal.verb());
        let request =
            PromptRequest::confirm(ASK_CONFIRM, &question).titled(signal.title(), signal.button());
        EditorOutcome::Prompt(match signal.ends() {
            true => request.destructive(),
            false => request,
        })
    }

    /// Run a signal that has been decided on.
    fn send_signal(&mut self, pending: PendingSignal) -> EditorOutcome {
        // The list has been read again while the question was open: a process
        // gone from it, or running something else under the same pid, is not
        // the one the user said yes about.
        let still_there = self
            .processes
            .iter()
            .any(|process| process.pid == pending.pid && process.command == pending.command);
        if !still_there {
            self.notice = Some(ProcNotice::Gone(pending.pid));
            return EditorOutcome::Consumed;
        }
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
                self.acting = Some(Acting::Signalled(Signalled {
                    name,
                    pid: pending.pid,
                    signal: pending.signal,
                }));
                EditorOutcome::Job(JobRequest::Command(request))
            }
            Err(refusal) => {
                self.notice = Some(ProcNotice::Refused(refusal));
                EditorOutcome::Consumed
            }
        }
    }

    /// Ask what priority the selected process should have.
    fn ask_renice(&mut self) -> EditorOutcome {
        let Some(pid) = self.selected().map(|process| process.pid) else {
            return EditorOutcome::Consumed;
        };
        self.pending_renice = Some(pid);
        let label = if cfg!(windows) {
            RENICE_LABEL_WINDOWS
        } else {
            RENICE_LABEL_UNIX
        };
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag: ASK_RENICE,
            title: None,
        })
    }

    /// Ask what to move the cursor to.
    fn ask_search(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH))
    }

    /// Remember `query` and move to the next process it matches.
    fn search_for(&mut self, query: &str) -> EditorOutcome {
        let labels = self.search_labels();
        if let Some(miss) = self.list.search_for(&labels, query.trim()) {
            self.notice = Some(ProcNotice::Search(miss));
        }
        EditorOutcome::Consumed
    }

    /// Move to the next listed process matching the last search, looking one
    /// past the cursor and wrapping so the cursor's own row is tried last, or
    /// report that there is nothing to move to.
    fn search_step(&mut self, forward: bool) -> EditorOutcome {
        let labels = self.search_labels();
        if let Some(miss) = self.list.search_step(&labels, forward) {
            self.notice = Some(ProcNotice::Search(miss));
        }
        EditorOutcome::Consumed
    }

    /// What a search reads over, one label per listed process.
    fn search_labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|row| self.processes[row.index].search_text())
            .collect()
    }

    /// Ask what the list should be narrowed to.
    fn ask_filter(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: self.filter.clone(),
            label: "filter: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_FILTER,
            title: None,
        })
    }

    /// What a finished signal or priority change reports.
    fn on_command_done(&mut self, output: &CommandOutput) -> EditorOutcome {
        let done = self.acting.take();
        self.notice = Some(match (output.succeeded(), done) {
            (true, Some(acting)) => ProcNotice::Done(acting),
            (true, None) => ProcNotice::DoneQuietly,
            (false, _) => ProcNotice::Failed(output.failure_detail()),
        });
        // The list is about to be wrong, so the next snapshot is not left to
        // the timer.
        self.timer.refresh_soon(Instant::now());
        EditorOutcome::Consumed
    }

    /// The columns before the command, left to right, those that do not
    /// fit the pane left out, least needed first: video memory, the user,
    /// the memory share, the resident memory, then the state.
    fn columns(&self) -> Vec<Column> {
        let column = |label, about, width, right, sort| Column {
            about,
            label,
            right,
            sort,
            width,
        };
        let mut columns = vec![
            column(PID_LABEL, "Process ID", PID_WIDTH, true, SortKey::Pid),
            column(
                NAME_LABEL,
                "Program name",
                self.name_width,
                false,
                SortKey::Name,
            ),
            column(
                CPU_LABEL,
                "Share of the whole machine's CPU",
                CPU_WIDTH,
                true,
                SortKey::Cpu,
            ),
            column(
                MEM_LABEL,
                "Memory in use (resident set size)",
                MEM_WIDTH,
                true,
                SortKey::Memory,
            ),
        ];
        if self.has_gpu {
            columns.push(column(
                VRAM_LABEL,
                "Video memory in use",
                VRAM_WIDTH,
                true,
                SortKey::Gpu,
            ));
        }
        columns.push(column(
            STATE_LABEL,
            "Running, sleeping, stopped, or a zombie",
            STATE_WIDTH,
            false,
            SortKey::State,
        ));
        columns.push(column(
            USER_LABEL,
            "Who runs it",
            USER_WIDTH,
            false,
            SortKey::User,
        ));
        for dropped in DROP_ORDER {
            if fits(&columns, self.cols) {
                break;
            }
            columns.retain(|column| column.label != dropped);
        }
        columns
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
    // Drawing
    // ====================================================================

    /// The summary at the footer's right: how many processes, how they
    /// are listed, and whether the readings are paused, and why the last
    /// snapshot failed.
    fn summary(&self) -> Vec<Element<'_, Self>> {
        let count = match (self.loaded, self.processes.len()) {
            (false, _) => "reading processes…".to_string(),
            (true, count) if self.filter.is_empty() => format!("{count} processes"),
            (true, count) => format!("{count} processes ({} shown)", self.rows.len()),
        };
        let (mode, listed) = match self.mode {
            ViewMode::Flat => ("flat", "Listed flat; {} shows the tree"),
            ViewMode::Tree => ("tree", "Listed as a tree; {} lists them flat"),
        };
        let sorted = format!(
            "Sorted by {}; {} sorts by the next column, {} turns it round",
            self.sort.label(),
            key_of(Act::Sort),
            key_of(Act::Reverse)
        );
        let mut parts = vec![
            text(count).muted().proportional().into(),
            badge(mode, ThemeToken::Muted).tooltip(listed.replace("{}", &key_of(Act::View))),
            badge(format!("sort {}", self.sort.label()), ThemeToken::Muted).tooltip(sorted),
        ];
        if !self.filter.is_empty() {
            let shown = format!(
                "Only processes matching {}; {} shows them all",
                self.filter,
                key_of(Act::ClearFilter)
            );
            parts
                .push(badge(format!("filter: {}", self.filter), ThemeToken::Accent).tooltip(shown));
        }
        if self.timer.is_paused() {
            let paused = format!("Readings paused; {} goes on", key_of(Act::Pause));
            parts.push(badge("paused", ThemeToken::Warning).tooltip(paused));
        }
        if let Some(report) = &self.error {
            parts.push(
                text(report.as_str())
                    .color(ThemeToken::Danger)
                    .proportional()
                    .into(),
            );
        }
        parts
    }

    /// The column headings, the sorted one marked with its direction: each
    /// a button sorting by it.
    fn heading_view(&self) -> Element<'_, Self> {
        let heading = |label: &'static str,
                       about: &'static str,
                       key: SortKey,
                       width: Option<usize>,
                       right: bool| {
            let arrow = (key == self.sort).then(|| self.direction.arrow());
            sort_heading(
                label,
                about,
                arrow,
                width,
                right,
                move |editor: &mut Self| {
                    editor.list.view_mut().release();
                    editor.sort_on_click(key);
                },
            )
        };
        let mut cells: Vec<Element<'_, Self>> = self
            .columns()
            .into_iter()
            .map(|column| {
                let width = Some(column.width);
                heading(column.label, column.about, column.sort, width, column.right)
            })
            .collect();
        cells.push(heading(
            COMMAND_LABEL,
            "The command line",
            SortKey::Command,
            None,
            false,
        ));
        table_row(cells)
    }

    /// List row `at` as the table shows it: a cell for each column, the
    /// CPU share at or past [`HOT_PERCENT`] in the warning color and the state in its
    /// color; the cursor's row marked. A click puts the cursor on it.
    fn process_view(&self, at: usize) -> Element<'_, Self> {
        let row = &self.rows[at];
        let process = &self.processes[row.index];
        let hot = |percent: f64| {
            let share = text(format_percent(percent));
            match percent >= HOT_PERCENT {
                true => share.color(ThemeToken::Warning),
                false => share,
            }
        };
        let (prefix, name) = self.name_parts(process, row);
        let mut cells: Vec<Element<'_, Self>> = Vec::new();
        for column in self.columns() {
            let width = Some(column.width);
            cells.push(match column.label {
                PID_LABEL => cell(text(process.pid.to_string()).muted(), width, true),
                NAME_LABEL => cell(
                    text(prefix.clone()).muted().then(name.clone()),
                    width,
                    false,
                ),
                CPU_LABEL => cell(hot(process.cpu_percent), width, true),
                MEM_LABEL => cell(text(format_kb(process.rss_kb)), width, true).tooltip(format!(
                    "{}% of the machine's memory",
                    format_percent(process.mem_percent)
                )),
                VRAM_LABEL => {
                    let vram = process.gpu_kb.map_or(NO_VRAM.to_string(), format_kb);
                    cell(text(vram), width, true)
                }
                STATE_LABEL => cell(
                    text(state_label(process.state)).color(state_tone(process.state)),
                    width,
                    false,
                )
                .tooltip(state_name(process.state)),
                _ => cell(
                    text(process.user.as_deref().unwrap_or("-")).muted(),
                    width,
                    false,
                ),
            });
        }
        // A command that only repeats the name (a kernel thread's) says
        // nothing more, so its cell is left blank.
        let command = match process.command.trim() == process.name {
            true => "",
            false => process.command.as_str(),
        };
        cells.push(cell(text(command), None, false));
        let line = table_row(cells).on_pointer_press(move |editor: &mut Self, _| {
            editor.list.view_mut().release();
            editor.move_to(at);
        });
        marked(line, at == self.list.cursor())
    }

    /// `start` and then the summary at its right, as the footer's first
    /// line.
    fn summary_line<'a>(&'a self, start: Element<'a, Self>) -> Element<'a, Self> {
        let mut parts = vec![start];
        parts.extend(self.summary());
        row(parts)
            .gap(Space::Cells(DETAIL_TITLE_GAP_COLS))
            .grow(1.0)
    }

    /// The page's footer, describing the process the cursor is on: its
    /// name and command line on the first line, the summary at its right, then a grid of its ids, its
    /// state, its shares, and its paths. Always the same three lines, so the
    /// list above keeps its height as the cursor moves.
    fn detail_view(&self) -> Element<'_, Self> {
        let Some(process) = self.selected() else {
            let blank = || [text(NO_FACT), text(NO_FACT)];
            let title = self.summary_line(Element::from(text(NO_SELECTION).muted()).grow(1.0));
            return detail_grid(title, [blank(), blank(), blank(), blank(), blank()]);
        };
        let parent = process.ppid.map_or(NO_FACT.to_string(), |ppid| {
            match self.processes.iter().find(|other| other.pid == ppid) {
                Some(other) => format!("{ppid} ({})", other.name),
                None => ppid.to_string(),
            }
        });
        let children = self
            .processes
            .iter()
            .filter(|other| other.ppid == Some(process.pid))
            .count();
        let known = |value: &Option<String>| text(value.clone().unwrap_or(NO_FACT.to_string()));
        let title = row([
            Element::from(text(process.name.as_str()).emphasized()).shrink(0.0),
            self.summary_line(
                Element::from(text(process.command.as_str()).muted())
                    .grow(1.0)
                    .tooltip(process.command.as_str()),
            ),
        ])
        .gap(Space::Cells(DETAIL_TITLE_GAP_COLS));
        detail_grid(
            title,
            [
                [text(process.pid.to_string()), text(parent)],
                [
                    known(&process.user),
                    text(state_label(process.state)).color(state_tone(process.state)),
                ],
                [
                    text(format!("{}%", format_percent(process.cpu_percent))),
                    text(format!(
                        "{} ({}%)",
                        format_kb(process.rss_kb),
                        format_percent(process.mem_percent)
                    )),
                ],
                [
                    text(process.gpu_kb.map_or(NO_FACT.to_string(), format_kb)),
                    text(children.to_string()),
                ],
                [known(&process.exec_path), known(&process.cwd)],
            ],
        )
    }

    /// The tree's indent and fold marker before a NAME, and the name cut to
    /// fit the rest of the column.
    fn name_parts(&self, process: &Process, row: &ViewRow) -> (String, String) {
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
        (prefix, fit(&process.name, room))
    }
}

impl LazySource<ProcEditor> for ProcEditor {
    fn count(&self) -> usize {
        self.rows.len()
    }

    fn row(&self, index: usize) -> Element<'_, ProcEditor> {
        self.process_view(index)
    }
}

impl HostedView for ProcEditor {
    /// The column headings over the processes, misty building those that
    /// show, and the summary in the footer, with the selected process where
    /// the pane has room for it.
    fn view(&self) -> Element<'_, Self> {
        let mut table = vec![self.heading_view(), divider()];
        if self.rows.is_empty() && self.loaded {
            table.push(text(NO_MATCH).muted().proportional().into());
        }
        // The list's own keys go by the rows misty shows.
        let rows = lazy_column(self, self.list.cursor())
            .on_shown(|editor: &mut Self, shown| editor.list.view_mut().set_shown(shown));
        table.push(rows.into());
        let table = column(table).gap(Space::Cells(0.0));
        // The tab already names the monitor, so the table starts at the
        // page's top, and the summary goes at the footer's right.
        let footer = match self.detail {
            true => self.detail_view(),
            false => self.summary_line(Element::from(text("")).grow(1.0)),
        };
        // The headings sit right under the tab, and the footer's text lines
        // up with the table's.
        untitled_page()
            .content_padding(Space::Cells(0.0))
            .filled(table)
            .footer(footer.padding(ROW_PADDING_COLS, 0.0))
            .into()
    }
}

impl OwnView for ProcEditor {
    fn view_slot(&mut self) -> &mut ViewSlot<Self> {
        &mut self.view
    }
}

impl Default for ProcEditor {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Page for ProcEditor {
    /// Not shown, the list stops refreshing; shown again, it refreshes at
    /// once.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        self.sync_timer();
        EditorOutcome::Consumed
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn visit_viewports(&mut self, visit: &mut dyn FnMut(&mut Viewport)) {
        visit(self.list.view_mut());
    }

    /// Misty draws the page; the grid under it is empty, its height kept
    /// for whether the footer describing the selected process fits.
    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.cols = cols;
        self.detail = rows >= MIN_ROWS_FOR_DETAIL;
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        match &mut self.details {
            Some(details) => details.draw_view(frame, area, theme),
            None => self.draw_own_view(frame, area, theme),
        }
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = match &mut self.details {
            Some(details) => details.view_input(input, cx),
            None => self.own_view_input(input, cx),
        };
        used.then_some(EditorOutcome::Consumed)
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = match &mut self.details {
            Some(details) => details.access_action(action),
            None => self.own_access_action(action),
        };
        used.then_some(EditorOutcome::Consumed)
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        match &mut self.details {
            Some(details) => details.view_about_to_wait(cx),
            None => self.own_view_about_to_wait(cx),
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(ProcNotice::into_notice)
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.details.is_some() {
            return self.on_details_key(key);
        }
        self.list.view_mut().release();
        if self.nav.in_sequence() {
            return self.on_motion_key(key);
        }
        match table::find(&KEYS, key).and_then(|command| self.run(command)) {
            Some(outcome) => outcome,
            None => self.on_motion_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = match self.details {
            Some(_) => table::help(&DETAIL_KEYS),
            None => table::help(&KEYS),
        };
        rows.extend(VimNav::help());
        rows
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.list.view_mut().release();
        let answer = reply.answer;
        match reply.tag {
            ASK_CONFIRM => {
                let pending = self.pending_signal.take();
                match (answer, pending) {
                    (Some(_), Some(pending)) => self.send_signal(pending),
                    _ => EditorOutcome::Consumed,
                }
            }
            ASK_FILTER => {
                if let Some(text) = answer {
                    self.filter = text.trim().to_string();
                    self.rebuild();
                }
                EditorOutcome::Consumed
            }
            ASK_SEARCH => match answer {
                Some(text) => self.search_for(&text),
                None => EditorOutcome::Consumed,
            },
            ASK_RENICE => {
                let pid = self.pending_renice.take();
                let (Some(value), Some(pid)) = (answer, pid) else {
                    return EditorOutcome::Consumed;
                };
                match act::renice_request(pid, &value, cfg!(windows)) {
                    Ok(request) => {
                        self.acting = Some(Acting::Reniced(Renice {
                            pid,
                            value: value.trim().to_string(),
                        }));
                        EditorOutcome::Job(JobRequest::Command(request))
                    }
                    Err(refusal) => {
                        self.notice = Some(ProcNotice::Refused(refusal));
                        EditorOutcome::Consumed
                    }
                }
            }
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_FILTER {
            self.filter = query.trim().to_string();
            self.rebuild();
            EditorOutcome::Consumed
        } else if tag == ASK_SEARCH && !query.trim().is_empty() {
            self.search_for(query)
        } else {
            EditorOutcome::Ignored
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
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
            JobReply::Monitor(MonitorAnswer::Details(info)) => {
                if let Some(view) = &mut self.details
                    && view.state().pid == info.pid
                {
                    view.state_mut().info = Some(info);
                }
            }
            JobReply::Command(_)
            | JobReply::DirSize { .. }
            | JobReply::Monitor(_)
            | JobReply::Files(_)
            | JobReply::FileOps(_)
            | JobReply::Search(_)
            | JobReply::System(_) => {}
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if !self.timer.on_tick(now) {
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Job(JobRequest::Processes)
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // Work asked for before the editor was covered was cancelled with it.
        self.timer.restart(Instant::now());
        match &self.details {
            Some(view) if view.state().info.is_none() => {
                let pid = view.state().pid;
                EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Details(pid)))
            }
            _ => EditorOutcome::Consumed,
        }
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        if self.details.is_some() {
            return menu_of(&DETAIL_KEYS, &DETAIL_MENU);
        }
        if self.selected().is_none() {
            return Vec::new();
        }
        menu_of(&KEYS, &MENU)
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.selected()
            .and_then(|process| process.cwd.as_ref())
            .map(PathBuf::from)
    }
}

// ========================================================================
// Helpers
// ========================================================================

impl ProcEditor {
    /// Do what one of the monitor's own keys asks. `None` for a key that has
    /// nothing to do in the state the monitor is in, which then reaches the
    /// motion layer.
    fn run(&mut self, command: Act) -> Option<EditorOutcome> {
        let outcome = match command {
            Act::Refresh => self.refresh_now(),
            Act::Pause => {
                self.timer.toggle_pause(Instant::now());
                EditorOutcome::Consumed
            }
            Act::Sort => {
                let mut next = self.sort.next();
                // A column that is not shown is not one to land on.
                if next == SortKey::Gpu && !self.has_gpu {
                    next = next.next();
                }
                self.sort_by(next);
                EditorOutcome::Consumed
            }
            Act::Reverse => {
                self.direction = self.direction.reversed();
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::View => {
                self.mode = match self.mode {
                    ViewMode::Flat => ViewMode::Tree,
                    ViewMode::Tree => ViewMode::Flat,
                };
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::Details => self.open_details(),
            Act::Fold => {
                self.set_folded(None);
                EditorOutcome::Consumed
            }
            Act::Close => {
                self.set_folded(Some(true));
                EditorOutcome::Consumed
            }
            Act::Open => {
                self.set_folded(Some(false));
                EditorOutcome::Consumed
            }
            Act::Search => self.ask_search(),
            Act::NextMatch => self.search_step(true),
            Act::PreviousMatch => self.search_step(false),
            Act::Filter => self.ask_filter(),
            Act::ClearFilter if self.filter.is_empty() => return None,
            Act::ClearFilter => {
                self.filter.clear();
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::Terminate => self.signal_selected(Signal::Terminate, false),
            Act::Kill => self.signal_selected(Signal::Kill, false),
            Act::TerminateTree => self.signal_selected(Signal::Terminate, true),
            Act::KillTree => self.signal_selected(Signal::Kill, true),
            Act::Suspend => self.signal_selected(Signal::Stop, false),
            Act::Resume => self.signal_selected(Signal::Continue, false),
            Act::Renice => self.ask_renice(),
            Act::CopyPid => match self.selected() {
                Some(process) => EditorOutcome::Yank(process.pid.to_string()),
                None => EditorOutcome::Consumed,
            },
            Act::CopyCommand => match self.selected() {
                Some(process) => EditorOutcome::Yank(process.command.clone()),
                None => EditorOutcome::Consumed,
            },
        };
        Some(outcome)
    }

    /// Offer a key to the shared Vim motion layer, and interpret what it
    /// resolves to over the rows.
    fn on_motion_key(&mut self, key: &Key) -> EditorOutcome {
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                self.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
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

/// The footer: `title` over a grid of [`DETAIL_LABELS`] beside `values`,
/// column by column, the last column taking the room the others leave.
fn detail_grid<'a>(
    title: Element<'a, ProcEditor>,
    values: [[Text<'a>; DETAIL_ROWS]; DETAIL_COLUMNS],
) -> Element<'a, ProcEditor> {
    let last = DETAIL_COLUMNS - 1;
    let columns = DETAIL_LABELS
        .iter()
        .zip(values)
        .enumerate()
        .map(|(at, (labels, values))| {
            let label_cols = labels
                .iter()
                .map(|label| label.len())
                .max()
                .unwrap_or_default();
            let width = DETAIL_VALUE_COLS.get(at).copied();
            let lines = labels.iter().zip(values).map(|(label, value)| {
                row([
                    cell(text(*label).muted(), Some(label_cols), false),
                    cell(value, width, false),
                ])
                .gap(Space::Cells(0.0))
            });
            let shown = column(lines).gap(Space::Cells(DETAIL_GAP_ROWS));
            match at < last {
                true => shown.shrink(0.0),
                false => shown.grow(1.0),
            }
        });
    let grid = row(columns).gap(Space::Cells(DETAIL_COLUMN_GAP_COLS));
    column([title, grid]).gap(Space::Cells(DETAIL_GAP_ROWS))
}

/// Whether `columns` and the command at its least fit a pane `cols` text
/// columns wide, each column with the gap after it.
fn fits(columns: &[Column], cols: usize) -> bool {
    let taken: usize = columns.iter().map(|column| column.width + 1).sum();
    taken + MIN_COMMAND_COLS + PAGE_PADDING_COLS <= cols
}

/// What a state means, for its cell's tooltip: its short label says it in
/// a word, and its color alone says which it is.
fn state_name(state: ProcState) -> &'static str {
    match state {
        ProcState::Running => "Running",
        ProcState::Sleeping => "Sleeping: waiting for something to do",
        ProcState::Stopped => "Stopped, as by a suspend",
        ProcState::Unknown => "State not known",
        ProcState::Zombie => "A zombie: it ended, and its parent has not yet heard",
    }
}

/// The key the monitor's own `act` is typed by, the user's where they bound
/// it, for a tooltip to name.
fn key_of(act: Act) -> String {
    table::menu_entry(&KEYS, act).map_or_else(String::new, |(key, _)| format_key(&key))
}

/// How a state is colored: what is running and what is wrong stand out from
/// the sleeping majority.
fn state_tone(state: ProcState) -> ThemeToken {
    match state {
        ProcState::Running => ThemeToken::Accent,
        ProcState::Sleeping | ProcState::Unknown => ThemeToken::Muted,
        ProcState::Stopped => ThemeToken::Warning,
        ProcState::Zombie => ThemeToken::Danger,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

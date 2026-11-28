//! Ports monitor: every socket on the machine and the process holding it,
//! the listening ones by default, kept current on a timer while shown, with
//! the process holding one a key away from being ended. What answers "what
//! is on port 3000?".
//!
//! Misty draws it as the process monitor is drawn, under a summary the
//! window puts on the header: a table whose headings sort on a click,
//! the rows that fit built and no more, and a footer describing the socket
//! the cursor is on.

use std::time::{Duration, Instant};

use misty::ThemeToken;
use misty::view::{LazySource, Space, column, divider, lazy_column, row, text};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect, Theme};

use winter_model::editor::refresh::RefreshTimer;
use winter_model::editor::{
    EditorMenuItem, EditorOutcome, JobReply, JobRequest, Notice, OwnView, Page, PromptMode,
    PromptReply, PromptRequest, SearchReport, ViewSlot, menu_of,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::{Key, format_key};
use winter_model::list::{ListView, Viewport};
use winter_model::monitor::{MonitorAnswer, MonitorQuery, Protocol, Socket};
use winter_model::paint::{Frame, Row};
use winter_model::view::ToolbarTone;
use winter_model::vim::nav::{VimKey, VimNav};

use crate::header::{PORT_MONITOR, SummaryPart, monitor_page};
use crate::proc::act::{self, Signal, TAG_SIGNAL};
use crate::proc::notice::{Acting, ProcConfirmation, ProcNotice, Signalled};
use crate::proc::view::SortDirection;
use crate::table::{ROW_PADDING_COLS, cell, fit, heading, marked, table_row};

// ========================================================================
// Constants
// ========================================================================

/// What the monitor is called in the pane title.
const TITLE: &str = "Port Monitor";

/// The questions the monitor asks.
const ASK_FILTER: &str = "ports-filter";
const ASK_SEARCH: &str = "ports-search";

/// How long after a listing arrives the next is asked for, and how long
/// one may be outstanding before it is asked for again.
const REFRESH_INTERVAL: Duration = Duration::from_secs(2);
const STALL_TIMEOUT: Duration = Duration::from_secs(10);

/// The pane height below which the footer describing the socket is left
/// out, so the list keeps what room there is.
const MIN_ROWS_FOR_DETAIL: usize = 16;

/// Column widths, in cells. A column is followed by one space.
const PROTO_WIDTH: usize = 5;
const PORT_WIDTH: usize = 6;
const ADDRESS_WIDTH: usize = 24;
const STATE_WIDTH: usize = 12;
const PID_WIDTH: usize = 7;

/// What the program column shows for a socket whose holder the system will
/// not say: another user's.
const NO_PROGRAM: &str = "-";

/// The monitor's own keys. Refreshing is `C-r` in every editor; `r` stays as
/// the short key.
const KEYS: KeyTable<Act> = KeyTable {
    block: "ports",
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
            action: Act::All,
            id: "all",
            keys: &["a"],
            label: "Listening / All Sockets",
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
            label: "Terminate Its Process",
        },
        Binding {
            action: Act::Kill,
            id: "kill",
            keys: &["X"],
            label: "Kill Its Process",
        },
        Binding {
            action: Act::CopyAddress,
            id: "copy_address",
            keys: &["y"],
            label: "Copy Address",
        },
        Binding {
            action: Act::CopyLine,
            id: "copy_line",
            keys: &["Y"],
            label: "Copy the Whole Line",
        },
    ],
    configurable: true,
    title: "Port Monitor",
};

/// Every key table of this surface, for the Keys editor to list.
pub const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

/// What the menu opened over a socket offers, a group at a time.
const MENU: [&[Act]; 3] = [
    &[Act::Terminate, Act::Kill],
    &[Act::All],
    &[Act::CopyAddress, Act::CopyLine],
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the monitor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    All,
    ClearFilter,
    CopyAddress,
    CopyLine,
    Filter,
    Kill,
    NextMatch,
    Pause,
    PreviousMatch,
    Refresh,
    Reverse,
    Search,
    Sort,
    Terminate,
}

/// The column the list is sorted by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SortKey {
    Local,
    Pid,
    Port,
    Program,
    Protocol,
    Remote,
    State,
}

/// A ports monitor.
#[derive(Debug)]
pub struct PortsEditor {
    /// What the signal in flight is reported as.
    acting: Option<Acting>,
    /// Whether every socket is listed, or the listening ones alone.
    all: bool,
    /// Whether the pane is tall enough for the footer.
    detail: bool,
    direction: SortDirection,
    /// Why the last listing failed, until one succeeds.
    error: Option<String>,
    filter: String,
    /// The cursor over the rows, keyed by each socket's line, so a refresh
    /// that reorders the list leaves the cursor on the same socket.
    list: ListView<String>,
    /// Whether a listing has arrived, so an empty list can say why.
    loaded: bool,
    nav: VimNav,
    /// What the last action reported, waiting for the status bar.
    notice: Option<ProcNotice>,
    /// What a press in the header asked of the window, waiting for it.
    outcome: Option<EditorOutcome>,
    /// The process being asked about ending: its id and name.
    pending: Option<(u32, String, Signal)>,
    /// The sockets shown, as indexes into `sockets`, in order.
    rows: Vec<usize>,
    sockets: Vec<Socket>,
    sort: SortKey,
    timer: RefreshTimer,
    view: ViewSlot<PortsEditor>,
}

// ========================================================================
// SortKey
// ========================================================================

impl SortKey {
    /// Every column, in the order `s` steps through them.
    const ALL: [SortKey; 7] = [
        SortKey::Port,
        SortKey::Protocol,
        SortKey::Local,
        SortKey::Remote,
        SortKey::State,
        SortKey::Pid,
        SortKey::Program,
    ];

    /// The column after this one.
    fn next(self) -> Self {
        let at = Self::ALL.iter().position(|key| *key == self).unwrap_or(0);
        Self::ALL[(at + 1) % Self::ALL.len()]
    }

    /// How `a` and `b` compare by this column, the port breaking ties.
    fn compare(self, a: &Socket, b: &Socket) -> std::cmp::Ordering {
        let first = match self {
            SortKey::Local => a.local.cmp(&b.local),
            SortKey::Pid => a.pid.cmp(&b.pid),
            SortKey::Port => a.port.cmp(&b.port),
            SortKey::Program => a.program.cmp(&b.program),
            SortKey::Protocol => a.protocol.cmp(&b.protocol),
            SortKey::Remote => a.remote.cmp(&b.remote),
            SortKey::State => a.state.cmp(&b.state),
        };
        first.then(a.port.cmp(&b.port))
    }
}

// ========================================================================
// PortsEditor
// ========================================================================

impl PortsEditor {
    /// A monitor that has read nothing yet and asks for its first listing on
    /// the next tick.
    pub fn new() -> Self {
        Self {
            acting: None,
            all: false,
            detail: false,
            direction: SortDirection::Ascending,
            error: None,
            filter: String::new(),
            list: ListView::new(),
            loaded: false,
            nav: VimNav::new(),
            notice: None,
            outcome: None,
            pending: None,
            rows: Vec::new(),
            sockets: Vec::new(),
            sort: SortKey::Port,
            timer: RefreshTimer::new(REFRESH_INTERVAL, STALL_TIMEOUT),
            view: ViewSlot::new(),
        }
    }

    /// Take in a listing that came back, or why none did.
    fn take(&mut self, result: Result<Vec<Socket>, String>) {
        match result {
            Ok(sockets) => {
                self.sockets = sockets;
                self.loaded = true;
                self.error = None;
                self.rebuild();
            }
            Err(why) => self.error = Some(why),
        }
    }

    /// Order and filter the list again, keeping the cursor on the same socket.
    fn rebuild(&mut self) {
        let filter = self.filter.to_lowercase();
        let mut rows: Vec<usize> = (0..self.sockets.len())
            .filter(|at| {
                let socket = &self.sockets[*at];
                (self.all || socket.listening)
                    && (filter.is_empty() || line_of(socket).to_lowercase().contains(&filter))
            })
            .collect();
        rows.sort_by(|a, b| {
            let order = self.sort.compare(&self.sockets[*a], &self.sockets[*b]);
            match self.direction {
                SortDirection::Ascending => order,
                SortDirection::Descending => order.reverse(),
            }
        });
        self.rows = rows;
        let keys = self
            .rows
            .iter()
            .map(|at| line_of(&self.sockets[*at]))
            .collect();
        self.list.set_keys(keys);
    }

    /// The socket the cursor is on.
    fn selected(&self) -> Option<&Socket> {
        self.sockets.get(*self.rows.get(self.list.cursor())?)
    }

    /// Ask for a listing now, whatever the timer says.
    fn refresh_now(&mut self) -> EditorOutcome {
        self.timer.request_now(Instant::now());
        EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Ports))
    }

    /// Ask whether to end the process holding the selected socket.
    fn ask_signal(&mut self, signal: Signal) -> EditorOutcome {
        let Some(socket) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let Some(pid) = socket.pid else {
            self.notice = Some(ProcNotice::Failed(
                "the system does not say which process holds it".to_string(),
            ));
            return EditorOutcome::Consumed;
        };
        let name = socket.program.clone().unwrap_or_else(|| pid.to_string());
        let what = format!("{name} (pid {pid}), which holds {}", socket.local);
        self.pending = Some((pid, name, signal));
        EditorOutcome::Confirm(ProcConfirmation::Signal { signal, what }.to_confirmation())
    }

    /// Send the signal said yes to, if the process still holds a socket.
    fn send_signal(&mut self, (pid, name, signal): (u32, String, Signal)) -> EditorOutcome {
        if !self.sockets.iter().any(|socket| socket.pid == Some(pid)) {
            self.notice = Some(ProcNotice::Gone(pid));
            return EditorOutcome::Consumed;
        }
        match act::signal_request(signal, pid, &[], false, cfg!(windows)) {
            Ok(request) => {
                self.acting = Some(Acting::Signalled(Signalled { name, pid, signal }));
                EditorOutcome::Job(JobRequest::Command(request))
            }
            Err(refusal) => {
                self.notice = Some(ProcNotice::Refused(refusal));
                EditorOutcome::Consumed
            }
        }
    }

    /// What a search reads over, one label per row.
    fn search_labels(&self) -> Vec<String> {
        self.rows
            .iter()
            .map(|at| line_of(&self.sockets[*at]))
            .collect()
    }

    /// Sort by `key`, or turn the sort round when it already is.
    fn sort_on(&mut self, key: SortKey) {
        match key == self.sort {
            true => self.direction = self.direction.reversed(),
            false => {
                self.sort = key;
                self.direction = SortDirection::Ascending;
            }
        }
        self.rebuild();
    }

    /// Do what one of the monitor's own keys asks. `None` for a key with
    /// nothing to do now, which then reaches the motion layer.
    fn run(&mut self, act: Act) -> Option<EditorOutcome> {
        let outcome = match act {
            Act::Refresh => self.refresh_now(),
            Act::Pause => {
                self.timer.toggle_pause(Instant::now());
                EditorOutcome::Consumed
            }
            Act::All => {
                self.all = !self.all;
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::Sort => {
                self.sort = self.sort.next();
                self.direction = SortDirection::Ascending;
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::Reverse => {
                self.direction = self.direction.reversed();
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::Search => EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH)),
            Act::NextMatch | Act::PreviousMatch => {
                let labels = self.search_labels();
                if let Some(miss) = self.list.search_step(&labels, act == Act::NextMatch) {
                    self.notice = Some(ProcNotice::Search(miss));
                }
                EditorOutcome::Consumed
            }
            Act::Filter => EditorOutcome::Prompt(PromptRequest {
                initial: self.filter.clone(),
                label: "filter: ".to_string(),
                mode: PromptMode::Text,
                tag: ASK_FILTER,
            }),
            Act::ClearFilter if self.filter.is_empty() => return None,
            Act::ClearFilter => {
                self.filter.clear();
                self.rebuild();
                EditorOutcome::Consumed
            }
            Act::Terminate => self.ask_signal(Signal::Terminate),
            Act::Kill => self.ask_signal(Signal::Kill),
            Act::CopyAddress => match self.selected() {
                Some(socket) => EditorOutcome::Yank(socket.local.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::CopyLine => match self.selected() {
                Some(socket) => EditorOutcome::Yank(line_of(socket)),
                None => EditorOutcome::Consumed,
            },
        };
        Some(outcome)
    }

    // ====================================================================
    // Drawing
    // ====================================================================

    /// The summary on the header: how many sockets, which are listed
    /// and how, any filter, whether the readings are paused, and why the
    /// last reading failed. Each part but the count and the error runs on a
    /// press what its key does.
    fn summary(&self) -> Vec<SummaryPart<Act>> {
        let count = match (self.loaded, self.all) {
            (false, _) => "reading sockets…".to_string(),
            (true, true) => format!("{} sockets", self.rows.len()),
            (true, false) => format!("{} listening", self.rows.len()),
        };
        let listed = |chosen: bool, now: &str, switch: &str| match chosen {
            true => now.to_string(),
            false => format!("{switch} ({})", key_of(Act::All)),
        };
        let mut parts = vec![
            SummaryPart::label(count, ToolbarTone::Normal),
            SummaryPart::choice(
                "Listening",
                !self.all,
                Act::All,
                listed(
                    !self.all,
                    "Only sockets waiting for connections",
                    "List Listening",
                ),
            ),
            SummaryPart::choice(
                "All",
                self.all,
                Act::All,
                listed(self.all, "Every socket, connections too", "List All"),
            ),
        ];
        if !self.filter.is_empty() {
            let matching = format!(
                "Only sockets matching {}; {} shows them all",
                self.filter,
                key_of(Act::ClearFilter)
            );
            parts.push(SummaryPart::button(
                format!("filter: {}", self.filter),
                ToolbarTone::Accent,
                Act::ClearFilter,
                matching,
            ));
        }
        if self.timer.is_paused() {
            let paused = format!("Readings paused; {} goes on", key_of(Act::Pause));
            parts.push(SummaryPart::button(
                "paused",
                ToolbarTone::Warning,
                Act::Pause,
                paused,
            ));
        }
        if let Some(why) = &self.error {
            parts.push(SummaryPart::label(why.as_str(), ToolbarTone::Danger));
        }
        parts
    }

    /// A press on a part of the summary: what its key does.
    fn run_summary(&mut self, act: Act) {
        self.list.view_mut().release();
        self.run(act);
    }

    /// What a press in the header asks of the window, for it to take.
    fn go(&mut self, outcome: EditorOutcome) {
        self.outcome = Some(outcome);
    }

    /// The column headings, each sorting by its column on a click.
    fn heading_view(&self) -> Element<'_, Self> {
        let head = |label: &str, about: &str, key: SortKey, width: Option<usize>, right: bool| {
            let arrow = (key == self.sort).then(|| self.direction.arrow());
            heading(
                label,
                about,
                arrow,
                width,
                right,
                move |editor: &mut Self| {
                    editor.list.view_mut().release();
                    editor.sort_on(key);
                },
            )
        };
        table_row(vec![
            head(
                "PROTO",
                "TCP or UDP",
                SortKey::Protocol,
                Some(PROTO_WIDTH),
                false,
            ),
            head(
                "PORT",
                "The local port",
                SortKey::Port,
                Some(PORT_WIDTH),
                true,
            ),
            head(
                "LOCAL",
                "The address it is bound to",
                SortKey::Local,
                Some(ADDRESS_WIDTH),
                false,
            ),
            head(
                "REMOTE",
                "The address it is connected to",
                SortKey::Remote,
                Some(ADDRESS_WIDTH),
                false,
            ),
            head(
                "STATE",
                "Its TCP state",
                SortKey::State,
                Some(STATE_WIDTH),
                false,
            ),
            head(
                "PID",
                "The process holding it",
                SortKey::Pid,
                Some(PID_WIDTH),
                true,
            ),
            head(
                "PROGRAM",
                "That process's name",
                SortKey::Program,
                None,
                false,
            ),
        ])
    }

    /// Row `at` as the table shows it, the cursor's marked; a click puts the
    /// cursor on it.
    fn socket_view(&self, at: usize) -> Element<'_, Self> {
        let socket = &self.sockets[self.rows[at]];
        let state = match socket.listening {
            true => text("listening").color(ThemeToken::Accent),
            false => text(socket.state.to_lowercase()).muted(),
        };
        let line = table_row(vec![
            cell(
                text(protocol_name(socket.protocol)).muted(),
                Some(PROTO_WIDTH),
                false,
            ),
            cell(text(socket.port.to_string()), Some(PORT_WIDTH), true),
            cell(
                text(fit(&socket.local, ADDRESS_WIDTH)),
                Some(ADDRESS_WIDTH),
                false,
            )
            .tooltip(socket.local.clone()),
            cell(
                text(fit(socket.remote.as_deref().unwrap_or(""), ADDRESS_WIDTH)).muted(),
                Some(ADDRESS_WIDTH),
                false,
            ),
            cell(state, Some(STATE_WIDTH), false),
            cell(
                text(socket.pid.map_or_else(String::new, |pid| pid.to_string())).muted(),
                Some(PID_WIDTH),
                true,
            ),
            cell(
                text(socket.program.as_deref().unwrap_or(NO_PROGRAM)),
                None,
                false,
            ),
        ])
        .on_pointer_press(move |editor: &mut Self, _| {
            editor.list.view_mut().release();
            editor.list.set_cursor(at);
        });
        marked(line, at == self.list.cursor())
    }

    /// The footer, describing the socket the cursor is on.
    fn detail_view(&self) -> Option<Element<'_, Self>> {
        let socket = self.selected()?;
        let reach = match &socket.remote {
            Some(remote) => format!("{} \u{2192} {remote}", socket.local),
            None => socket.local.clone(),
        };
        let holder = match (socket.pid, &socket.program) {
            (Some(pid), Some(program)) => format!("held by {program} (pid {pid})"),
            (Some(pid), None) => format!("held by pid {pid}"),
            _ => "its holder belongs to another user".to_string(),
        };
        Some(
            column([
                row([
                    text(protocol_name(socket.protocol)).emphasized().into(),
                    text(reach).into(),
                ])
                .gap(Space::Cells(1.0)),
                text(holder).muted().into(),
            ])
            .gap(Space::Cells(0.2)),
        )
    }
}

impl LazySource<PortsEditor> for PortsEditor {
    fn count(&self) -> usize {
        self.rows.len()
    }

    fn row(&self, index: usize) -> Element<'_, PortsEditor> {
        self.socket_view(index)
    }
}

impl HostedView for PortsEditor {
    /// The headings over the sockets, misty building those that show, and
    /// the selected socket in the footer where there is room.
    fn view(&self) -> Element<'_, Self> {
        let mut table = vec![self.heading_view(), divider()];
        if self.rows.is_empty() && self.loaded {
            let none = match self.all {
                true => "no sockets",
                false => "nothing is listening",
            };
            table.push(text(none).muted().into());
        }
        table.push(
            lazy_column(self, self.list.cursor())
                .on_shown(|editor: &mut Self, shown| editor.list.view_mut().set_shown(shown))
                .into(),
        );
        // The tab names the monitor and the header sums it up, so the
        // headings sit right under them, and the footer's text lines up
        // with the table's.
        let shown = monitor_page(
            self,
            PORT_MONITOR,
            Self::summary,
            Self::run_summary,
            Self::go,
        )
        .filled(column(table).gap(Space::Cells(0.0)));
        match self.detail.then(|| self.detail_view()).flatten() {
            Some(detail) => shown.footer(detail.padding(ROW_PADDING_COLS, 0.0)).into(),
            None => shown.into(),
        }
    }
}

impl OwnView for PortsEditor {
    fn view_slot(&mut self) -> &mut ViewSlot<Self> {
        &mut self.view
    }
}

impl Default for PortsEditor {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// Editor
// ========================================================================

impl Page for PortsEditor {
    /// Not shown, the list stops refreshing; shown again, it refreshes at
    /// once.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.timer.set_hidden(!shown, Instant::now());
        EditorOutcome::Consumed
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn visit_viewports(&mut self, visit: &mut dyn FnMut(&mut Viewport)) {
        visit(self.list.view_mut());
    }

    /// Misty draws the page; the grid's height says whether the footer fits.
    fn content(&mut self, rows: usize, _cols: usize, _wrap: bool) -> Frame {
        self.detail = rows >= MIN_ROWS_FOR_DETAIL;
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.draw_own_view(frame, area, theme);
    }

    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = self.own_view_input(input, cx);
        self.outcome
            .take()
            .or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = self.own_access_action(action);
        self.outcome
            .take()
            .or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.own_view_about_to_wait(cx)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(ProcNotice::into_notice)
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence()
            && let Some(outcome) = table::find(&KEYS, key).and_then(|act| self.run(act))
        {
            return outcome;
        }
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                self.list.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(&KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_confirm(&mut self, yes: bool) -> EditorOutcome {
        self.list.view_mut().release();
        match (self.pending.take(), yes) {
            (Some(pending), true) => self.send_signal(pending),
            (_, _) => EditorOutcome::Consumed,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.list.view_mut().release();
        match (reply.tag, reply.answer) {
            (ASK_FILTER, Some(answer)) => {
                self.filter = answer.trim().to_string();
                self.rebuild();
                EditorOutcome::Consumed
            }
            (ASK_SEARCH, Some(answer)) => {
                let labels = self.search_labels();
                if let Some(miss) = self.list.search_for(&labels, answer.trim()) {
                    self.notice = Some(ProcNotice::Search(miss));
                }
                EditorOutcome::Consumed
            }
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag != ASK_FILTER {
            return EditorOutcome::Ignored;
        }
        self.filter = query.trim().to_string();
        self.rebuild();
        EditorOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match reply {
            JobReply::Monitor(MonitorAnswer::Ports(result)) => {
                self.timer.on_answer(Instant::now());
                self.take(result);
            }
            JobReply::Command(output) if output.tag == TAG_SIGNAL => {
                let done = self.acting.take();
                self.notice = Some(match (output.succeeded(), done) {
                    (true, Some(acting)) => ProcNotice::Done(acting),
                    (true, None) => ProcNotice::DoneQuietly,
                    (false, _) => ProcNotice::Failed(output.failure_detail()),
                });
                self.timer.refresh_soon(Instant::now());
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.timer.next_tick()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        match self.timer.on_tick(now) {
            true => EditorOutcome::Job(JobRequest::Monitor(MonitorQuery::Ports)),
            false => EditorOutcome::Consumed,
        }
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // Work asked for before the editor was covered was cancelled with it.
        self.timer.restart(Instant::now());
        EditorOutcome::Consumed
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        match self.selected() {
            Some(_) => menu_of(&KEYS, &MENU),
            None => Vec::new(),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A protocol as the table names it.
fn protocol_name(protocol: Protocol) -> &'static str {
    match protocol {
        Protocol::Tcp => "tcp",
        Protocol::Udp => "udp",
    }
}

/// A socket in one line: what a search and a filter read, `Y` copies, and
/// the cursor is kept on.
fn line_of(socket: &Socket) -> String {
    let mut parts = vec![
        protocol_name(socket.protocol).to_string(),
        socket.local.clone(),
    ];
    if let Some(remote) = &socket.remote {
        parts.push(format!("\u{2192} {remote}"));
    }
    if !socket.state.is_empty() {
        parts.push(socket.state.clone());
    }
    if let Some(pid) = socket.pid {
        parts.push(pid.to_string());
    }
    if let Some(program) = &socket.program {
        parts.push(program.clone());
    }
    parts.join(" ")
}

/// The key the monitor's own `act` is typed by, the user's where they bound
/// it, for a tooltip to name.
fn key_of(act: Act) -> String {
    table::menu_entry(&KEYS, act).map_or_else(String::new, |(key, _)| format_key(&key))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

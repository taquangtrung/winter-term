//! The Sessions sidebar view: managed agents of the project on top and open
//! terminals across workspaces below, each with live status icons and quick
//! actions.

use std::collections::HashSet;
use std::time::Instant;

use misty::view::{Press, divider, row, text};
use misty::{Element, HostedView, IconSet, ThemeToken};

use winter_model::editor::EditorOutcome;
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::layout::TabId;
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;

use super::cards;
use super::{ListAct, ListRow, Mark, PanelList, PanelView, RowIcon, mark_view};
use crate::icons;
use crate::window::agents::{Agent, AgentAction};
use crate::window::picker::AgentRequest;

// ========================================================================
// Constants
// ========================================================================

/// What the panel's switcher calls the view.
const TITLE: &str = "Sessions";

/// What it says before sessions are known, and when none exist.
const LOOKING: &str = "Looking for sessions…";
const NONE: &str = "No sessions: start an agent with Agent: New…";

/// What an agent started by hand says on hover, after its folder.
const BY_HAND_NOTE: &str = "Agent: Manage gives it a branch of its own";

/// Horizontal padding cells reserved around a flush-left subtitle path.
const SUBTITLE_PAD_CELLS: usize = 2;

/// Section headings for managed agents and open terminals.
const AGENTS_HEADING: &str = "Agents";
const TERMINALS_HEADING: &str = "Terminals";

/// Bundled symbol icons for sections, fallback avatars, details, states, and
/// inline card actions.
const ACTION_DISCARD_ICON: &str = "trash";
const ACTION_MERGE_ICON: &str = "git-merge";
const ACTION_REVIEW_ICON: &str = "diff";
const ACTION_STOP_ICON: &str = "debug-stop";
const AGENT_FALLBACK_ICON: &str = "hubot";
const BASE_ICON: &str = "git-merge";
const DONE_ICON: &str = "check";
const FAILED_ICON: &str = "error";
const RUNNING_ICON: &str = "circle-filled";
const STOPPED_ICON: &str = "circle-outline";
const TASK_ICON: &str = "comment";
const TERMINAL_ICON: &str = "terminal";
const WAITING_ICON: &str = "bell-dot";

/// Symbol icons always kept loaded for card action buttons and empty state.
const CARD_SYMBOL_ICONS: &[&str] = &[
    ACTION_DISCARD_ICON,
    ACTION_MERGE_ICON,
    ACTION_REVIEW_ICON,
    ACTION_STOP_ICON,
    AGENT_FALLBACK_ICON,
    TERMINAL_ICON,
];

/// Double-click threshold for opening a stopped managed agent card.
const DOUBLE_CLICK: u8 = 2;

/// Fallback labels when a status icon is not bundled.
const DONE: &str = "done";
const RUNNING: &str = "running";
const STOPPED: &str = "stopped";
const WAITING: &str = "waiting";
const WORKING: &str = "working";

/// Keys reserved per managed agent: its own row plus its detail rows.
const KEYS_PER_AGENT: usize = 4;

/// Hover notes for managed agent detail lines.
const BASE_NOTE: &str = "The branch it started from, where Agent: Merge lands it";
const TASK_NOTE: &str = "What it is doing, as its terminal's title says";

/// The Sessions view's own keys, block `panel_sessions`, ahead of the panel
/// keys.
pub(super) const KEYS: KeyTable<AgentAction> = KeyTable {
    block: "panel_sessions",
    bindings: &[
        Binding {
            action: AgentAction::Review,
            id: "review",
            keys: &["r"],
            label: "Review the Agent's Work",
        },
        Binding {
            action: AgentAction::Merge,
            id: "merge",
            keys: &["m"],
            label: "Merge the Agent",
        },
        Binding {
            action: AgentAction::Discard,
            id: "discard",
            keys: &["x"],
            label: "Discard the Agent",
        },
        Binding {
            action: AgentAction::Stop,
            id: "stop",
            keys: &["s"],
            label: "Stop the Agent",
        },
    ],
    configurable: true,
    title: "Sessions View",
};

// ========================================================================
// Data Structures
// ========================================================================

/// The managed and hand-started agents of the project and ordinary terminals
/// open in the window, along with any pending user request.
#[derive(Debug, Default)]
pub(in crate::window) struct SessionsView {
    /// Managed agents found, in row order; `None` before the first fill.
    agents: Option<Vec<Managed>>,
    /// Width of the panel in cells, used to shorten paths; 0 before first draw.
    cols: usize,
    /// Hand-started agents running in open terminals, in row order.
    detected: Vec<Detected>,
    /// Ordinary open terminals, in row order; `None` before the first fill.
    entries: Option<Vec<TerminalEntry>>,
    /// Agents currently folded shut.
    folded: HashSet<Folded>,
    list: PanelList,
    /// Pending request for the window to carry out once.
    request: Option<AgentRequest>,
    /// Whether the view is currently visible.
    shown: bool,
    /// Whether the view wants the window to refresh its data.
    stale: bool,
}

/// A managed agent, as the view shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Managed {
    pub(in crate::window) agent: Agent,
    pub(in crate::window) standing: Standing,
    /// What it is doing, as its terminal's title says, while it has one.
    pub(in crate::window) task: Option<String>,
}

/// An agent started by hand in one of the window's terminals.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct Detected {
    /// Which agent it is, as `settings.kdl` names it.
    pub(in crate::window) agent: String,
    /// Where it works.
    pub(in crate::window) folder: Option<String>,
    pub(in crate::window) standing: Standing,
    /// Its terminal's tab.
    pub(in crate::window) tab: TabId,
    /// What it is doing, as its terminal's title says.
    pub(in crate::window) task: Option<String>,
}

/// Which agent's fold state is tracked across refills.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Folded {
    /// A managed agent, by its branch.
    Branch(String),
    /// A hand-started agent, by its terminal tab.
    Tab(TabId),
}

/// How an agent stands, as the view shows it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Standing {
    /// Its session is live; its tab says no more.
    Running,
    /// Its session is gone.
    Stopped,
    /// It waits for you.
    Waiting,
    /// It works.
    Working,
}

/// An ordinary terminal open in the window, as the view shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct TerminalEntry {
    /// Where its shell is.
    pub(in crate::window) folder: Option<String>,
    /// Program currently running in the terminal, when known.
    pub(in crate::window) program: Option<String>,
    pub(in crate::window) ran: Ran,
    pub(in crate::window) tab: TabId,
    /// What it runs, or last ran: the command line, else the program, else
    /// its tab's name.
    pub(in crate::window) task: String,
    /// The parked workspace it belongs to; `None` for the active workspace.
    pub(in crate::window) workspace: Option<String>,
}

/// How an ordinary terminal's command went.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Ran {
    /// Its last command failed, with this exit code.
    Failed(i32),
    /// Its last command finished well.
    Finished,
    /// It has run nothing the shell reported, or did not say how it went.
    Idle,
    /// A command runs in it.
    Running,
}

// ========================================================================
// SessionsView Implementations
// ========================================================================

impl SessionsView {
    /// Show `agents`, the managed agents found.
    pub(in crate::window) fn set_agents(&mut self, agents: Vec<Managed>) {
        if self.agents.as_ref() != Some(&agents) {
            self.agents = Some(agents);
            self.build_rows();
        }
    }

    /// Show `detected`, the agents started by hand in open terminals.
    pub(in crate::window) fn set_detected(&mut self, detected: Vec<Detected>) {
        if detected != self.detected {
            self.detected = detected;
            self.build_rows();
        }
    }

    /// Show `entries`, the window's ordinary open terminals.
    pub(in crate::window) fn set_terminals(&mut self, entries: Vec<TerminalEntry>) {
        if self.entries.as_ref() != Some(&entries) {
            self.entries = Some(entries);
            self.build_rows();
        }
    }

    /// The managed agents currently held, or `None` before the first fill.
    pub(in crate::window) fn managed_agents(&self) -> Option<Vec<Agent>> {
        let agents = self.agents.as_ref()?;
        Some(agents.iter().map(|managed| managed.agent.clone()).collect())
    }

    /// Whether the view is currently shown.
    pub(in crate::window) fn is_shown(&self) -> bool {
        self.shown
    }

    /// Whether the window should refresh sessions, taken once when shown.
    pub(in crate::window) fn take_stale(&mut self) -> bool {
        std::mem::take(&mut self.stale) && self.shown
    }

    /// Ask the window to refresh on the next check while shown.
    pub(in crate::window) fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// Pending action requested by the user, taken once.
    pub(in crate::window) fn take_request(&mut self) -> Option<AgentRequest> {
        self.request.take()
    }

    // --------------------------------------------------------------------
    // Row construction and lookup
    // --------------------------------------------------------------------

    /// Rebuild backing rows for keyboard navigation, icon loading, and spinner
    /// tracking.
    fn build_rows(&mut self) {
        let width = self.path_width();
        let mut rows = Vec::new();
        let managed = self.managed();
        let agent_count = managed.len() + self.detected.len();
        if agent_count > 0 {
            rows.push(section_row(0, AGENTS_HEADING, agent_count));
        }
        for (at, entry) in managed.iter().enumerate() {
            let key = at * KEYS_PER_AGENT;
            let open = !self
                .folded
                .contains(&Folded::Branch(entry.agent.branch.clone()));
            rows.push(managed_row(key, entry, open, width));
            if open {
                let details = [
                    entry
                        .task
                        .as_deref()
                        .map(|task| (task, TASK_NOTE, TASK_ICON, "Task")),
                    entry
                        .agent
                        .base
                        .as_deref()
                        .map(|base| (base, BASE_NOTE, BASE_ICON, "Base branch")),
                    None,
                ];
                rows.extend(detail_rows(key, details));
            }
        }
        let first_detected = self.first_detected_key();
        for (at, detected) in self.detected.iter().enumerate() {
            let key = first_detected + at * KEYS_PER_AGENT;
            let open = !self.folded.contains(&Folded::Tab(detected.tab));
            rows.push(detected_row(key, detected, open, width));
            if open {
                let details = [
                    detected
                        .task
                        .as_deref()
                        .map(|task| (task, TASK_NOTE, TASK_ICON, "Task")),
                    None,
                    None,
                ];
                rows.extend(detail_rows(key, details));
            }
        }
        let first_terminal = self.first_terminal_key();
        let entries = self.entries();
        let mut at = 0;
        while at < entries.len() {
            let workspace = &entries[at].workspace;
            let end = entries[at..]
                .iter()
                .position(|entry| &entry.workspace != workspace)
                .map_or(entries.len(), |offset| at + offset);
            let count = end - at;
            let heading = match workspace {
                Some(name) => section_row(first_terminal + at, name, count),
                None => section_row(first_terminal + at, TERMINALS_HEADING, count),
            };
            rows.push(heading);
            for (offset, entry) in entries[at..end].iter().enumerate() {
                rows.push(terminal_row(first_terminal + at + offset, entry, width));
            }
            at = end;
        }
        self.list.set_rows(rows);
        self.list.ensure_symbol_icons(CARD_SYMBOL_ICONS);
    }

    /// Cells available for a flush-left subtitle path.
    fn path_width(&self) -> usize {
        self.cols.saturating_sub(SUBTITLE_PAD_CELLS)
    }

    /// Managed agents loaded so far.
    fn managed(&self) -> &[Managed] {
        self.agents.as_deref().unwrap_or_default()
    }

    /// Terminal entries loaded so far.
    fn entries(&self) -> &[TerminalEntry] {
        self.entries.as_deref().unwrap_or_default()
    }

    /// First key reserved for hand-started agent rows.
    fn first_detected_key(&self) -> usize {
        self.managed().len() * KEYS_PER_AGENT
    }

    /// First key reserved for ordinary terminal rows.
    fn first_terminal_key(&self) -> usize {
        (self.managed().len() + self.detected.len()) * KEYS_PER_AGENT
    }

    /// Managed agent corresponding to `key` (either its row or a detail row).
    fn managed_at(&self, key: usize) -> Option<&Managed> {
        if key >= self.first_detected_key() {
            return None;
        }
        self.managed().get(key / KEYS_PER_AGENT)
    }

    /// Hand-started agent corresponding to `key` (either its row or a detail
    /// row).
    fn detected_at(&self, key: usize) -> Option<&Detected> {
        if key >= self.first_terminal_key() {
            return None;
        }
        let offset = key.checked_sub(self.first_detected_key())?;
        self.detected.get(offset / KEYS_PER_AGENT)
    }

    /// Terminal entry corresponding to `key`.
    fn terminal_at(&self, key: usize) -> Option<&TerminalEntry> {
        let at = key.checked_sub(self.first_terminal_key())?;
        self.entries().get(at)
    }

    /// Fold identity for the agent row (or detail row) at `key`.
    fn folded_at(&self, key: usize) -> Option<Folded> {
        match self.managed_at(key) {
            Some(managed) => Some(Folded::Branch(managed.agent.branch.clone())),
            None => self
                .detected_at(key)
                .map(|detected| Folded::Tab(detected.tab)),
        }
    }

    /// Selected non-heading row key.
    fn selected_key(&self) -> Option<usize> {
        self.list
            .selected_row()
            .filter(|row| !row.heading)
            .map(|row| row.key)
    }

    /// Managed agent currently under the cursor, if any.
    fn selected_managed(&self) -> Option<&Agent> {
        self.managed_at(self.selected_key()?)
            .map(|managed| &managed.agent)
    }

    // --------------------------------------------------------------------
    // Interaction handlers
    // --------------------------------------------------------------------

    /// Fold or unfold the selected agent and return the cursor to its card row.
    fn fold(&mut self, shut: bool) {
        let Some(key) = self.selected_key() else {
            return;
        };
        let Some(folded) = self.folded_at(key) else {
            return;
        };
        match shut {
            true => self.folded.insert(folded),
            false => self.folded.remove(&folded),
        };
        self.build_rows();
        self.list.select_key(key - key % KEYS_PER_AGENT, false);
    }

    /// Toggle the folded state of `folded` from a card chevron click.
    fn toggle_fold(&mut self, folded: Folded, key: usize) {
        if !self.folded.remove(&folded) {
            self.folded.insert(folded);
        }
        self.build_rows();
        self.list.select_key(key, false);
    }

    /// Select an agent card at `key` and open it on single click (or double
    /// click when a managed agent is stopped).
    fn click_agent(&mut self, key: usize, press: Press) {
        self.list.select_key(key, false);
        if self.opens_on_click(key) || press.clicks >= DOUBLE_CLICK {
            self.request = self.open_request();
        }
    }

    /// Select and open the terminal card at `key`.
    fn click_terminal(&mut self, key: usize, tab: TabId) {
        self.list.select_key(key, false);
        self.request = Some(AgentRequest::ShowTab(tab));
    }

    /// Dispatch an inline card action for a managed agent.
    fn run_card_action(&mut self, key: usize, action: AgentAction, agent: Agent) {
        self.list.select_key(key, false);
        self.request = Some(AgentRequest::Act(action, agent));
    }

    /// Request to open whatever row is currently selected.
    fn open_request(&self) -> Option<AgentRequest> {
        let key = self.selected_key()?;
        if let Some(managed) = self.managed_at(key) {
            return Some(AgentRequest::Open(managed.agent.clone()));
        }
        if let Some(detected) = self.detected_at(key) {
            return Some(AgentRequest::ShowTab(detected.tab));
        }
        self.terminal_at(key)
            .map(|entry| AgentRequest::ShowTab(entry.tab))
    }
}

impl PanelView for SessionsView {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn list(&self) -> &PanelList {
        &self.list
    }

    fn list_mut(&mut self) -> &mut PanelList {
        &mut self.list
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match act {
            ListAct::Open => self.request = self.open_request(),
            ListAct::Collapse => self.fold(true),
            ListAct::Expand => self.fold(false),
            ListAct::Refresh => self.stale = true,
            ListAct::Down | ListAct::First | ListAct::Last | ListAct::OpenTool | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }

    fn own_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        let action = table::find(&KEYS, key)?;
        self.request = self
            .selected_managed()
            .cloned()
            .map(|agent| AgentRequest::Act(action, agent));
        Some(EditorOutcome::Consumed)
    }

    fn opens_on_click(&self, key: usize) -> bool {
        match self.managed_at(key) {
            Some(managed) => managed.standing != Standing::Stopped,
            None => self.detected_at(key).is_some() || self.terminal_at(key).is_some(),
        }
    }

    fn fit(&mut self, _rows: usize, cols: usize) {
        if cols != self.cols {
            self.cols = cols;
            self.build_rows();
        }
    }

    fn next_tick(&self) -> Option<Instant> {
        self.list.next_spinner_tick(self.shown)
    }

    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        self.stale |= shown;
        EditorOutcome::Consumed
    }
}

impl HostedView for SessionsView {
    fn view(&self) -> Element<'_, Self> {
        let icons = self.list.icons();
        let managed = self.managed();
        let entries = self.entries();
        let agent_count = managed.len() + self.detected.len();
        if agent_count == 0 && entries.is_empty() {
            let empty = match (self.agents.as_ref(), self.entries.as_ref()) {
                (None, None) => LOOKING,
                _ => NONE,
            };
            return cards::empty_state(icons, TERMINAL_ICON, empty);
        }
        let spinner_epoch = self.list.spinner_epoch();
        let first_detected = self.first_detected_key();
        let first_terminal = self.first_terminal_key();
        let selected_key = self.selected_key();
        let selected_agent = selected_key
            .filter(|key| *key < first_terminal)
            .map(|key| key - key % KEYS_PER_AGENT);
        let mut items: Vec<Element<'_, Self>> = Vec::new();
        if agent_count > 0 {
            items.push(cards::section_bar(AGENTS_HEADING, agent_count, false));
            for (at, entry) in managed.iter().enumerate() {
                let key = at * KEYS_PER_AGENT;
                let Some(row) = self.list.row_for(key) else {
                    continue;
                };
                let open = !self
                    .folded
                    .contains(&Folded::Branch(entry.agent.branch.clone()));
                items.push(managed_card(
                    icons,
                    spinner_epoch,
                    key,
                    entry,
                    row,
                    selected_agent == Some(key),
                    open,
                ));
            }
            for (at, entry) in self.detected.iter().enumerate() {
                let key = first_detected + at * KEYS_PER_AGENT;
                let Some(row) = self.list.row_for(key) else {
                    continue;
                };
                let open = !self.folded.contains(&Folded::Tab(entry.tab));
                items.push(detected_card(
                    icons,
                    spinner_epoch,
                    key,
                    entry,
                    row,
                    selected_agent == Some(key),
                    open,
                ));
            }
        }
        let mut at = 0;
        while at < entries.len() {
            let workspace = &entries[at].workspace;
            let end = entries[at..]
                .iter()
                .position(|entry| &entry.workspace != workspace)
                .map_or(entries.len(), |offset| at + offset);
            let count = end - at;
            let separated = at > 0 || agent_count > 0;
            let label = workspace.as_deref().unwrap_or(TERMINALS_HEADING);
            items.push(cards::section_bar(label, count, separated));
            for (offset, entry) in entries[at..end].iter().enumerate() {
                let key = first_terminal + at + offset;
                let Some(row) = self.list.row_for(key) else {
                    continue;
                };
                items.push(terminal_card(
                    icons,
                    spinner_epoch,
                    key,
                    entry,
                    row,
                    selected_key == Some(key),
                ));
            }
            at = end;
        }
        cards::card_list(items)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Card view for a managed agent: brand icon, branch, status icon, flush-left
/// worktree path, expandable details, and quick inline action buttons.
fn managed_card<'a>(
    icons: &'a IconSet,
    spinner_epoch: Option<Instant>,
    key: usize,
    managed: &'a Managed,
    list_row: &'a ListRow,
    selected: bool,
    open: bool,
) -> Element<'a, SessionsView> {
    let agent = &managed.agent;
    let has_details = managed.task.is_some() || agent.base.is_some();
    let mut header: Vec<Element<'a, SessionsView>> = Vec::new();
    if has_details {
        let branch = agent.branch.clone();
        header.push(cards::card_fold_button(
            open,
            move |view: &mut SessionsView, _| {
                view.toggle_fold(Folded::Branch(branch.clone()), key);
            },
        ));
    }
    if let Some(icon) = list_row.icon.as_ref()
        && let Some(drawn) = cards::leading_icon(icons, icon)
    {
        header.push(drawn);
    }
    header.push(Element::from(text(agent.branch.as_str()).proportional()).grow(1.0));
    if let Some(mark) = list_row.mark.as_ref() {
        header.push(mark_view(icons, spinner_epoch, mark));
    }

    let mut lines: Vec<Element<'a, SessionsView>> = vec![row(header).gap(cards::inline_gap())];
    if let Some(subtitle) = list_row.subtitle.as_deref() {
        lines.push(Element::from(text(subtitle).muted()));
    }
    if open {
        lines.push(divider());
        if let Some(task) = managed.task.as_deref() {
            lines.push(cards::detail_line(
                icons, TASK_ICON, "Task", task, TASK_NOTE,
            ));
        }
        if let Some(base) = agent.base.as_deref() {
            lines.push(cards::detail_line(
                icons,
                BASE_ICON,
                "Base branch",
                base,
                BASE_NOTE,
            ));
        }
        let mut actions: Vec<Element<'a, SessionsView>> = Vec::new();
        let review_agent = agent.clone();
        if let Some(btn) = cards::action_button(
            icons,
            ACTION_REVIEW_ICON,
            "Review (r)",
            ThemeToken::Muted,
            move |view: &mut SessionsView, _| {
                view.run_card_action(key, AgentAction::Review, review_agent.clone())
            },
        ) {
            actions.push(btn);
        }
        let merge_agent = agent.clone();
        if let Some(btn) = cards::action_button(
            icons,
            ACTION_MERGE_ICON,
            "Merge (m)",
            ThemeToken::Muted,
            move |view: &mut SessionsView, _| {
                view.run_card_action(key, AgentAction::Merge, merge_agent.clone())
            },
        ) {
            actions.push(btn);
        }
        if managed.standing != Standing::Stopped {
            let stop_agent = agent.clone();
            if let Some(btn) = cards::action_button(
                icons,
                ACTION_STOP_ICON,
                "Stop (s)",
                ThemeToken::Muted,
                move |view: &mut SessionsView, _| {
                    view.run_card_action(key, AgentAction::Stop, stop_agent.clone())
                },
            ) {
                actions.push(btn);
            }
        }
        let discard_agent = agent.clone();
        if let Some(btn) = cards::action_button(
            icons,
            ACTION_DISCARD_ICON,
            "Discard (x)",
            ThemeToken::Danger,
            move |view: &mut SessionsView, _| {
                view.run_card_action(key, AgentAction::Discard, discard_agent.clone())
            },
        ) {
            actions.push(btn);
        }
        if !actions.is_empty() {
            lines.push(row(actions).gap(cards::inline_gap()));
        }
    }
    cards::card_shell(
        format!("session-agent/{}", agent.branch),
        selected,
        list_row.tooltip.as_deref(),
        lines,
        move |view: &mut SessionsView, press| view.click_agent(key, press),
    )
}

/// Card view for a hand-started terminal agent.
fn detected_card<'a>(
    icons: &'a IconSet,
    spinner_epoch: Option<Instant>,
    key: usize,
    detected: &'a Detected,
    list_row: &'a ListRow,
    selected: bool,
    open: bool,
) -> Element<'a, SessionsView> {
    let has_details = detected.task.is_some();
    let mut header: Vec<Element<'a, SessionsView>> = Vec::new();
    if has_details {
        let tab = detected.tab;
        header.push(cards::card_fold_button(
            open,
            move |view: &mut SessionsView, _| {
                view.toggle_fold(Folded::Tab(tab), key);
            },
        ));
    }
    if let Some(icon) = list_row.icon.as_ref()
        && let Some(drawn) = cards::leading_icon(icons, icon)
    {
        header.push(drawn);
    }
    header.push(Element::from(text(detected.agent.as_str()).proportional()).grow(1.0));
    if let Some(mark) = list_row.mark.as_ref() {
        header.push(mark_view(icons, spinner_epoch, mark));
    }

    let mut lines: Vec<Element<'a, SessionsView>> = vec![row(header).gap(cards::inline_gap())];
    if let Some(subtitle) = list_row.subtitle.as_deref() {
        lines.push(Element::from(text(subtitle).muted()));
    }
    if open && let Some(task) = detected.task.as_deref() {
        lines.push(divider());
        lines.push(cards::detail_line(
            icons, TASK_ICON, "Task", task, TASK_NOTE,
        ));
    }
    cards::card_shell(
        format!("session-detected/{}", detected.tab.0),
        selected,
        list_row.tooltip.as_deref(),
        lines,
        move |view: &mut SessionsView, press| view.click_agent(key, press),
    )
}

/// Card view for an open terminal: app or terminal icon, command/task label,
/// optional status icon, and flush-left cwd path.
fn terminal_card<'a>(
    icons: &'a IconSet,
    spinner_epoch: Option<Instant>,
    key: usize,
    entry: &'a TerminalEntry,
    list_row: &'a ListRow,
    selected: bool,
) -> Element<'a, SessionsView> {
    let mut header: Vec<Element<'a, SessionsView>> = Vec::new();
    if let Some(icon) = list_row.icon.as_ref()
        && let Some(drawn) = cards::leading_icon(icons, icon)
    {
        header.push(drawn);
    }
    header.push(Element::from(text(entry.task.as_str()).proportional()).grow(1.0));
    if let Some(mark) = list_row.mark.as_ref() {
        header.push(mark_view(icons, spinner_epoch, mark));
    }

    let mut lines: Vec<Element<'a, SessionsView>> = vec![row(header).gap(cards::inline_gap())];
    if let Some(subtitle) = list_row.subtitle.as_deref() {
        lines.push(Element::from(text(subtitle).muted()));
    }
    let tab = entry.tab;
    cards::card_shell(
        format!("session-term/{}", tab.0),
        selected,
        list_row.tooltip.as_deref(),
        lines,
        move |view: &mut SessionsView, _| view.click_terminal(key, tab),
    )
}

/// A section header row with a title and item count badge.
fn section_row(key: usize, label: &str, count: usize) -> ListRow {
    ListRow {
        detail: Some(count.to_string()),
        heading: true,
        key,
        label: label.to_string(),
        ..ListRow::default()
    }
}

/// Leading icon for an agent: its bundled app icon when available, otherwise a
/// robot symbol muted when stopped.
fn agent_icon(name: &str, standing: Standing) -> RowIcon {
    if icons::has_program_icon(name) {
        return RowIcon::new(
            IconKind::Program {
                name: name.to_string(),
            },
            name,
            ThemeToken::Foreground,
        );
    }
    let color = match standing {
        Standing::Stopped => ThemeToken::Muted,
        Standing::Running | Standing::Waiting | Standing::Working => ThemeToken::Foreground,
    };
    RowIcon::new(
        IconKind::Symbol {
            name: AGENT_FALLBACK_ICON,
        },
        name,
        color,
    )
}

/// Leading icon for a terminal: its program's bundled app icon when available,
/// otherwise a terminal symbol lit while active.
fn terminal_icon(program: Option<&str>, ran: Ran) -> RowIcon {
    if let Some(name) = program.filter(|name| icons::has_program_icon(name)) {
        return RowIcon::new(
            IconKind::Program {
                name: name.to_string(),
            },
            name,
            ThemeToken::Foreground,
        );
    }
    let color = match ran {
        Ran::Running => ThemeToken::Foreground,
        Ran::Failed(_) | Ran::Finished | Ran::Idle => ThemeToken::Muted,
    };
    RowIcon::new(
        IconKind::Symbol {
            name: TERMINAL_ICON,
        },
        "Terminal",
        color,
    )
}

/// Backing row for a managed agent under `key`.
fn managed_row(key: usize, managed: &Managed, open: bool, width: usize) -> ListRow {
    let agent = &managed.agent;
    let folder = abbreviate_home(&agent.worktree);
    let has_details = managed.task.is_some() || agent.base.is_some();
    let tooltip = match &agent.base {
        Some(base) => format!("Started from {base}, in {folder}"),
        None => folder.clone(),
    };
    ListRow {
        fold: has_details.then_some(open),
        icon: Some(agent_icon(&agent.kind, managed.standing)),
        key,
        label: agent.branch.clone(),
        mark: Some(agent_mark(managed.standing)),
        subtitle: Some(fit_path(&folder, width)),
        tooltip: Some(tooltip),
        ..ListRow::default()
    }
}

/// Backing row for a hand-started agent under `key`.
fn detected_row(key: usize, detected: &Detected, open: bool, width: usize) -> ListRow {
    let has_details = detected.task.is_some();
    ListRow {
        fold: has_details.then_some(open),
        icon: Some(agent_icon(&detected.agent, detected.standing)),
        key,
        label: detected.agent.clone(),
        mark: Some(agent_mark(detected.standing)),
        subtitle: detected
            .folder
            .as_deref()
            .map(|folder| fit_path(folder, width)),
        tooltip: Some(match &detected.folder {
            Some(folder) => format!("Started by hand in {folder}: {BY_HAND_NOTE}"),
            None => format!("Started by hand: {BY_HAND_NOTE}"),
        }),
        ..ListRow::default()
    }
}

/// Backing row for a terminal entry under `key`.
fn terminal_row(key: usize, entry: &TerminalEntry, width: usize) -> ListRow {
    ListRow {
        icon: Some(terminal_icon(entry.program.as_deref(), entry.ran)),
        key,
        label: entry.task.clone(),
        mark: terminal_mark(entry.ran),
        subtitle: entry
            .folder
            .as_deref()
            .map(|folder| fit_path(folder, width)),
        tooltip: entry.folder.clone(),
        ..ListRow::default()
    }
}

/// Detail rows under a managed agent's `key`.
fn detail_rows<'a>(
    key: usize,
    details: [Option<(&'a str, &'a str, &'static str, &'static str)>; KEYS_PER_AGENT - 1],
) -> impl Iterator<Item = ListRow> + 'a {
    details
        .into_iter()
        .enumerate()
        .filter_map(move |(at, detail)| {
            let (words, note, icon, icon_name) = detail?;
            Some(ListRow {
                depth: 1,
                icon: Some(RowIcon::new(
                    IconKind::Symbol { name: icon },
                    icon_name,
                    ThemeToken::Muted,
                )),
                key: key + 1 + at,
                label: words.to_string(),
                muted: true,
                tooltip: Some(note.to_string()),
                ..ListRow::default()
            })
        })
}

/// Shorten `folder` from the left to fit within `width` cells while keeping
/// the last component intact.
fn fit_path(folder: &str, width: usize) -> String {
    let mut parts: Vec<String> = folder.split('/').map(str::to_string).collect();
    let length = |parts: &[String]| {
        parts.iter().map(|part| part.chars().count()).sum::<usize>() + parts.len() - 1
    };
    let last = parts.len().saturating_sub(1);
    for at in 0..last {
        if length(&parts) <= width {
            break;
        }
        if parts[at] == "~" {
            continue;
        }
        let keep = match parts[at].starts_with('.') {
            true => 2,
            false => 1,
        };
        parts[at] = parts[at].chars().take(keep).collect();
    }
    parts.join("/")
}

/// Status mark for a managed agent.
fn agent_mark(standing: Standing) -> Mark {
    let (icon, text, color, meaning) = match standing {
        Standing::Working => (
            IconKind::Spinner,
            WORKING,
            ThemeToken::Muted,
            "Working on a request",
        ),
        Standing::Waiting => (
            IconKind::Symbol { name: WAITING_ICON },
            WAITING,
            ThemeToken::Foreground,
            "Waiting for you",
        ),
        Standing::Running => (
            IconKind::Symbol { name: RUNNING_ICON },
            RUNNING,
            ThemeToken::Muted,
            "Running",
        ),
        Standing::Stopped => (
            IconKind::Symbol { name: STOPPED_ICON },
            STOPPED,
            ThemeToken::Muted,
            "Stopped: Enter opens it again",
        ),
    };
    Mark {
        color,
        icon: Some(icon),
        meaning: meaning.to_string(),
        text: text.to_string(),
    }
}

/// Status mark for a terminal entry; `None` when idle.
fn terminal_mark(ran: Ran) -> Option<Mark> {
    let (icon, text, color, meaning) = match ran {
        Ran::Failed(code) => (
            IconKind::Symbol { name: FAILED_ICON },
            format!("exit {code}"),
            ThemeToken::Danger,
            format!("Its last command failed with exit code {code}"),
        ),
        Ran::Finished => (
            IconKind::Symbol { name: DONE_ICON },
            DONE.to_string(),
            ThemeToken::Muted,
            "Its last command finished".to_string(),
        ),
        Ran::Idle => return None,
        Ran::Running => (
            IconKind::Spinner,
            RUNNING.to_string(),
            ThemeToken::Muted,
            "Running".to_string(),
        ),
    };
    Some(Mark {
        color,
        icon: Some(icon),
        meaning,
        text,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use winter_model::input::KeyCode;

    use super::*;

    fn agent(branch: &str) -> Agent {
        Agent {
            base: Some("main".to_string()),
            branch: branch.to_string(),
            kind: "claude".to_string(),
            running: true,
            worktree: PathBuf::from("/src/app.worktrees").join(branch),
        }
    }

    fn managed(branch: &str, standing: Standing) -> Managed {
        Managed {
            agent: agent(branch),
            standing,
            task: None,
        }
    }

    fn terminal(tab: u64, task: &str, ran: Ran, workspace: Option<&str>) -> TerminalEntry {
        TerminalEntry {
            folder: Some("/src/app".to_string()),
            program: None,
            ran,
            tab: TabId(tab),
            task: task.to_string(),
            workspace: workspace.map(str::to_string),
        }
    }

    fn by_hand(tab: TabId) -> Detected {
        Detected {
            agent: "claude".to_string(),
            folder: Some("/src/app".to_string()),
            standing: Standing::Waiting,
            tab,
            task: Some("Refactor the parser".to_string()),
        }
    }

    fn filled() -> SessionsView {
        let mut view = SessionsView::default();
        view.set_shown(true);
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Working),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        view.set_detected(vec![by_hand(TabId(3))]);
        view.set_terminals(vec![
            terminal(1, "cargo test", Ran::Running, None),
            terminal(2, "make lint", Ran::Failed(2), None),
            terminal(4, "zsh", Ran::Idle, Some("notes")),
        ]);
        view
    }

    fn labels(view: &SessionsView) -> Vec<(String, bool, usize)> {
        view.list
            .rows
            .iter()
            .map(|row| (row.label.clone(), row.heading, row.depth))
            .collect()
    }

    #[test]
    fn test_coming_into_sight_asks_for_sessions_once() {
        let mut view = SessionsView::default();
        assert!(!view.take_stale(), "out of sight, nothing");
        view.set_shown(true);
        assert!(view.take_stale());
        assert!(!view.take_stale(), "once");
    }

    #[test]
    fn test_enter_opens_managed_agent_or_terminal_and_agent_keys_act_only_on_managed() {
        let mut view = filled();
        view.list.select_key(KEYS_PER_AGENT, false);
        view.act(ListAct::Open);
        assert_eq!(
            view.take_request(),
            Some(AgentRequest::Open(agent("agent/codex-1")))
        );
        view.own_key(&Key::plain(KeyCode::Char('m')));
        assert_eq!(
            view.take_request(),
            Some(AgentRequest::Act(
                AgentAction::Merge,
                agent("agent/codex-1")
            ))
        );
        assert_eq!(view.take_request(), None, "taken once");

        let first_detected = 2 * KEYS_PER_AGENT;
        view.list.select_key(first_detected + 1, false);
        view.act(ListAct::Open);
        assert_eq!(view.take_request(), Some(AgentRequest::ShowTab(TabId(3))));
        view.own_key(&Key::plain(KeyCode::Char('m')));
        assert_eq!(
            view.take_request(),
            None,
            "hand-started agent has no branch"
        );

        let first_terminal = 3 * KEYS_PER_AGENT;
        view.list.select_key(first_terminal + 2, false);
        view.act(ListAct::Open);
        assert_eq!(view.take_request(), Some(AgentRequest::ShowTab(TabId(4))));
        view.own_key(&Key::plain(KeyCode::Char('m')));
        assert_eq!(view.take_request(), None, "plain terminal has no branch");
    }

    #[test]
    fn test_managed_agent_details_lead_back_to_it_and_fold_holds_across_refill() {
        let mut view = SessionsView::default();
        view.set_shown(true);
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Working),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        view.list.select_key(1, false);
        view.act(ListAct::Collapse);
        assert_eq!(
            view.list.selected_row().map(|row| row.key),
            Some(0),
            "cursor returns to agent row"
        );
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Waiting),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        let shown: Vec<String> = labels(&view).into_iter().map(|(label, ..)| label).collect();
        assert_eq!(
            shown,
            [AGENTS_HEADING, "agent/claude-1", "agent/codex-1", "main"]
        );
        view.act(ListAct::Expand);
        assert_eq!(view.list.rows.len(), 5);
    }

    #[test]
    fn test_one_click_shows_running_agent_or_terminal_and_never_starts_stopped_agent() {
        let mut view = SessionsView::default();
        view.set_shown(true);
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Running),
            managed("agent/claude-2", Standing::Stopped),
        ]);
        view.set_terminals(vec![terminal(7, "cargo test", Ran::Running, None)]);
        let at_once: Vec<bool> = [0, 1, KEYS_PER_AGENT, KEYS_PER_AGENT + 1, 2 * KEYS_PER_AGENT]
            .into_iter()
            .map(|key| view.opens_on_click(key))
            .collect();
        assert_eq!(at_once, [true, true, false, false, true]);
    }

    #[test]
    fn test_parked_workspaces_group_under_their_own_headings() {
        let view = filled();
        let headings: Vec<(&str, Option<&str>)> = view
            .list
            .rows
            .iter()
            .filter(|row| row.heading)
            .map(|row| (row.label.as_str(), row.detail.as_deref()))
            .collect();
        assert_eq!(
            headings,
            [
                (AGENTS_HEADING, Some("3")),
                (TERMINALS_HEADING, Some("2")),
                ("notes", Some("1")),
            ]
        );
    }

    #[test]
    fn test_a_path_is_shortened_from_the_left_only_as_far_as_its_width_needs() {
        let path = "~/Workspace/apps/winter-term";
        assert_eq!(fit_path(path, 40), path, "room for all of it");
        assert_eq!(fit_path(path, 20), "~/W/apps/winter-term");
        assert_eq!(fit_path(path, 17), "~/W/a/winter-term");
        assert_eq!(
            fit_path(path, 5),
            "~/W/a/winter-term",
            "the last kept whole"
        );
        assert_eq!(fit_path("~/.config/winter-term", 0), "~/.c/winter-term");
        assert_eq!(fit_path("/srv/app", 0), "/s/app");
        assert_eq!(fit_path("~", 0), "~");
        assert_eq!(fit_path("/", 0), "/");
    }

    #[test]
    fn test_a_wider_panel_shows_more_of_each_path() {
        let mut view = filled();
        let subtitle = |view: &SessionsView| view.list.rows[1].subtitle.clone();
        view.fit(10, 60);
        assert_eq!(
            subtitle(&view).as_deref(),
            Some("/src/app.worktrees/agent/claude-1")
        );
        view.fit(10, 20);
        assert_eq!(subtitle(&view).as_deref(), Some("/s/a/a/claude-1"));
    }

    #[test]
    fn test_rows_coming_and_going_keep_each_icon_its_own() {
        let mut page = super::super::PanelPage::new(SessionsView::default());
        let window = misty::testing::TestWindow::new(400, 600);
        page.state_mut().set_shown(true);
        crate::editors::view_tests::drawn(&mut page, &window);
        page.state_mut().set_agents(vec![
            managed("agent/claude-1", Standing::Working),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        page.state_mut()
            .set_terminals(vec![terminal(7, "cargo test", Ran::Running, None)]);
        crate::editors::view_tests::drawn(&mut page, &window);
        page.state_mut().set_terminals(Vec::new());
        crate::editors::view_tests::drawn(&mut page, &window);
    }

    #[test]
    fn test_every_state_is_drawn_as_a_bundled_icon() {
        let standings = [
            Standing::Running,
            Standing::Stopped,
            Standing::Waiting,
            Standing::Working,
        ];
        for mark in standings.into_iter().map(agent_mark) {
            if mark.icon == Some(IconKind::Spinner) {
                continue;
            }
            let name = mark.icon.as_ref().and_then(crate::icons::name_for);
            assert!(
                name.as_deref().is_some_and(crate::icons::contains),
                "{} has no bundled icon: {name:?}",
                mark.meaning
            );
        }
        let rans = [Ran::Failed(2), Ran::Finished, Ran::Running];
        for mark in rans.into_iter().filter_map(terminal_mark) {
            if mark.icon == Some(IconKind::Spinner) {
                continue;
            }
            let name = mark.icon.as_ref().and_then(crate::icons::name_for);
            assert!(
                name.as_deref().is_some_and(crate::icons::contains),
                "{} has no bundled icon: {name:?}",
                mark.meaning
            );
        }
        for icon in [
            ACTION_DISCARD_ICON,
            ACTION_MERGE_ICON,
            ACTION_REVIEW_ICON,
            ACTION_STOP_ICON,
            AGENT_FALLBACK_ICON,
            BASE_ICON,
            TASK_ICON,
            TERMINAL_ICON,
        ] {
            let name = crate::icons::name_for(&IconKind::Symbol { name: icon });
            assert!(
                name.as_deref().is_some_and(crate::icons::contains),
                "{icon} has no bundled icon: {name:?}"
            );
        }
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = SessionsView::default();
        view.set_shown(true);
        view.set_agents(vec![managed("agent/claude-1", Standing::Working)]);
        view.set_detected(vec![by_hand(TabId(3))]);
        view.set_terminals(vec![terminal(1, "cargo test", Ran::Running, None)]);
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }

    #[test]
    fn test_spinner_ticks_only_while_shown_and_active() {
        let mut view = SessionsView::default();
        view.set_agents(vec![managed("agent/claude-1", Standing::Working)]);
        assert_eq!(view.next_tick(), None, "hidden view never ticks");
        view.set_shown(true);
        assert!(view.next_tick().is_some(), "shown working agent spins");
        view.set_agents(vec![managed("agent/claude-1", Standing::Waiting)]);
        assert_eq!(view.next_tick(), None, "waiting agent stops spinning");
        view.set_terminals(vec![terminal(1, "cargo test", Ran::Running, None)]);
        assert!(view.next_tick().is_some(), "running terminal spins");
        view.set_terminals(vec![terminal(1, "cargo test", Ran::Finished, None)]);
        assert_eq!(view.next_tick(), None, "finished terminal stops spinning");
    }
}

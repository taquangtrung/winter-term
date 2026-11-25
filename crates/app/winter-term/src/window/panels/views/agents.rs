//! The Agents view (see `docs/design/agent-workspace.md`): the managed
//! agents of the project, each with its branch, and those started by hand in
//! the window's terminals, each folding open on what it is doing. `Enter`,
//! or a click on one that runs, shows its terminal; its own keys review,
//! merge, discard, or stop a managed one. The window fills it from the
//! agents' thread each time it comes into sight and after an agent is
//! started or ended, keeps it current as the agents change, and carries out
//! what it asks ([`AgentsView::take_request`]). Every terminal, agents' and
//! all, is the Terminals view's (see [`super::terminals`]).

use std::collections::HashSet;

use misty::{Element, HostedView, ThemeToken};

use winter_model::editor::EditorOutcome;
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::layout::TabId;
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;

use super::{ListAct, ListRow, Mark, PanelList, PanelView, list_view};
use crate::window::agents::{Agent, AgentAction};
use crate::window::picker::AgentRequest;

// ========================================================================
// Constants
// ========================================================================

/// What the panel's switcher calls the view.
const TITLE: &str = "Agents";

/// What it says before the agents are known, and with none.
const LOOKING: &str = "Looking for agents…";
const NONE: &str = "No agents: start one with Agent: New…";

/// What an agent started by hand says of itself on hover, after where.
const BY_HAND_NOTE: &str = "Agent: Manage gives it a branch of its own";

/// The cells of a row its path may not take: the space around the row and
/// the fold mark's slot, with a cell between the path and the mark.
pub(super) const ROW_SPARE_CELLS: usize = 4;

/// The cells the marks' column takes: one icon.
pub(super) const MARK_CELLS: usize = 2;

/// The icons saying how an agent stands, from the bundled codicons: at
/// work, and waiting for you, as the Terminals view says them too.
pub(super) const WORKING_ICON: &str = "sync";
pub(super) const WAITING_ICON: &str = "bell-dot";
const RUNNING_ICON: &str = "circle-filled";
const STOPPED_ICON: &str = "circle-outline";

/// What an agent's mark says it is doing.
const WORKING: &str = "working";
const WAITING: &str = "waiting";
const RUNNING: &str = "running";
const STOPPED: &str = "stopped";

/// The keys an agent's rows take: its own row's, then its details'. An
/// agent's rows are keyed from its place times this, so any of them leads
/// back to it.
const KEYS_PER_AGENT: usize = 4;

/// What its details' rows say of each part, on hover.
const TASK_NOTE: &str = "What it is doing, as its terminal's title says";
const BASE_NOTE: &str = "The branch it started from, where Agent: Merge lands it";

/// The Agents view's own keys, block `panel_agents`, ahead of the panel
/// keys.
pub(super) const KEYS: KeyTable<AgentAction> = KeyTable {
    block: "panel_agents",
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
            keys: &["D"],
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
    title: "Agents View",
};

// ========================================================================
// Data Structures
// ========================================================================

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

/// Which agent a fold is kept for, across the window filling it again.
#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum Folded {
    /// A managed agent, by its branch.
    Branch(String),
    /// One started by hand, by its terminal.
    Tab(TabId),
}

/// The managed agents of the project and those started by hand, and what
/// was asked of one.
#[derive(Debug, Default)]
pub(in crate::window) struct AgentsView {
    /// The agents found, in the rows' order; `None` before the window first
    /// fills it.
    agents: Option<Vec<Managed>>,
    /// The agents started by hand, in the rows' order, after the managed
    /// ones.
    detected: Vec<Detected>,
    /// How many cells across the panel is, which its paths are shortened
    /// to fit; 0 before it is first drawn.
    cols: usize,
    /// The agents folded shut; the rest show their details.
    folded: HashSet<Folded>,
    list: PanelList,
    /// What was asked of an agent, until the window takes it.
    request: Option<AgentRequest>,
    /// Whether it is in sight, the only time it is filled.
    shown: bool,
    /// Whether it wants the window to look for the agents again.
    stale: bool,
}

// ========================================================================
// AgentsView Implementations
// ========================================================================

impl AgentsView {
    /// Show `agents`, the managed agents found.
    pub(in crate::window) fn set_agents(&mut self, agents: Vec<Managed>) {
        if self.agents.as_ref() != Some(&agents) {
            self.agents = Some(agents);
            self.build_rows();
        }
    }

    /// Show `detected`, the agents started by hand.
    pub(in crate::window) fn set_detected(&mut self, detected: Vec<Detected>) {
        if detected != self.detected {
            self.detected = detected;
            self.build_rows();
        }
    }

    /// The managed agents it shows, for the window to bring up to date;
    /// `None` while it waits for the first look.
    pub(in crate::window) fn managed_agents(&self) -> Option<Vec<Agent>> {
        let agents = self.agents.as_ref()?;
        Some(agents.iter().map(|managed| managed.agent.clone()).collect())
    }

    /// Whether it is in sight, the only time it wants what changes.
    pub(in crate::window) fn is_shown(&self) -> bool {
        self.shown
    }

    /// The rows: each managed agent, then each started by hand, followed by
    /// its details unless folded shut.
    fn build_rows(&mut self) {
        let width = self.path_width();
        let mut rows = Vec::new();
        let managed = self.managed();
        for (at, managed) in managed.iter().enumerate() {
            let key = at * KEYS_PER_AGENT;
            let open = !self
                .folded
                .contains(&Folded::Branch(managed.agent.branch.clone()));
            rows.push(managed_row(key, managed, open, width));
            if open {
                let details = [
                    managed.task.as_deref().map(|task| (task, TASK_NOTE)),
                    managed.agent.base.as_deref().map(|base| (base, BASE_NOTE)),
                    None,
                ];
                rows.extend(detail_rows(key, details));
            }
        }
        let first = managed.len() * KEYS_PER_AGENT;
        for (at, detected) in self.detected.iter().enumerate() {
            let key = first + at * KEYS_PER_AGENT;
            let open = !self.folded.contains(&Folded::Tab(detected.tab));
            rows.push(detected_row(key, detected, open, width));
            if open {
                let details = [
                    detected.task.as_deref().map(|task| (task, TASK_NOTE)),
                    None,
                    None,
                ];
                rows.extend(detail_rows(key, details));
            }
        }
        self.list.set_rows(rows);
    }

    /// How many cells a path may take under its row's words: the panel's
    /// width, less the space around a row and the marks' column, which
    /// every agent's row has.
    fn path_width(&self) -> usize {
        self.cols.saturating_sub(ROW_SPARE_CELLS + MARK_CELLS)
    }

    /// The managed agents found so far.
    fn managed(&self) -> &[Managed] {
        self.agents.as_deref().unwrap_or_default()
    }

    /// The managed agent any row under `key` stands for.
    fn managed_at(&self, key: usize) -> Option<&Managed> {
        self.managed().get(key / KEYS_PER_AGENT)
    }

    /// The agent started by hand any row under `key` stands for.
    fn detected_at(&self, key: usize) -> Option<&Detected> {
        let at = (key / KEYS_PER_AGENT).checked_sub(self.managed().len())?;
        self.detected.get(at)
    }

    /// What keeps the fold of the agent any row under `key` stands for.
    fn folded_at(&self, key: usize) -> Option<Folded> {
        match self.managed_at(key) {
            Some(managed) => Some(Folded::Branch(managed.agent.branch.clone())),
            None => self
                .detected_at(key)
                .map(|detected| Folded::Tab(detected.tab)),
        }
    }

    /// The key of the row the cursor is on, never a heading's.
    fn selected_key(&self) -> Option<usize> {
        self.list
            .selected_row()
            .filter(|row| !row.heading)
            .map(|row| row.key)
    }

    /// Fold the agent the cursor is on shut, or open, putting the cursor on
    /// its own row.
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

    /// Whether the window should look for the agents again, taken so it
    /// looks once: it came into sight, or `r` asked.
    pub(in crate::window) fn take_stale(&mut self) -> bool {
        std::mem::take(&mut self.stale) && self.shown
    }

    /// Look again the next time the window asks, if in sight.
    pub(in crate::window) fn mark_stale(&mut self) {
        self.stale = true;
    }

    /// What was asked of an agent, taken so it is done once.
    pub(in crate::window) fn take_request(&mut self) -> Option<AgentRequest> {
        self.request.take()
    }

    /// The managed agent the cursor is on, on its row or its details'.
    fn selected(&self) -> Option<&Agent> {
        self.managed_at(self.selected_key()?)
            .map(|managed| &managed.agent)
    }

    /// What showing the agent the cursor is on asks: a managed one shown or
    /// opened again, or the terminal of one started by hand.
    fn open_request(&self) -> Option<AgentRequest> {
        let key = self.selected_key()?;
        if let Some(managed) = self.managed_at(key) {
            return Some(AgentRequest::Open(managed.agent.clone()));
        }
        self.detected_at(key)
            .map(|detected| AgentRequest::ShowTab(detected.tab))
    }
}

impl PanelView for AgentsView {
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
            .selected()
            .cloned()
            .map(|agent| AgentRequest::Act(action, agent));
        Some(EditorOutcome::Consumed)
    }

    /// A running agent shows at one click, on its row or its details'; a
    /// stopped one, which a click would start again, takes two, as
    /// elsewhere. Its fold mark folds it.
    fn opens_on_click(&self, key: usize) -> bool {
        match self.managed_at(key) {
            Some(managed) => managed.standing != Standing::Stopped,
            None => self.detected_at(key).is_some(),
        }
    }

    /// Its paths are shortened again when the panel's width changes.
    fn fit(&mut self, _rows: usize, cols: usize) {
        if cols != self.cols {
            self.cols = cols;
            self.build_rows();
        }
    }

    /// Looks again each time it comes into sight.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        self.stale |= shown;
        EditorOutcome::Consumed
    }
}

impl HostedView for AgentsView {
    fn view(&self) -> Element<'_, Self> {
        let empty = match self.agents {
            None => LOOKING,
            Some(_) => NONE,
        };
        list_view(self, empty)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `managed` under `key`: its branch over its folder, marked
/// with how it stands, open on its details or folded shut.
fn managed_row(key: usize, managed: &Managed, open: bool, width: usize) -> ListRow {
    let agent = &managed.agent;
    let folder = abbreviate_home(&agent.worktree);
    let tooltip = match &agent.base {
        Some(base) => format!("Started from {base}, in {folder}"),
        None => folder.clone(),
    };
    ListRow {
        fold: Some(open),
        key,
        label: agent.branch.clone(),
        mark: Some(mark(managed.standing)),
        subtitle: Some(fit_path(&folder, width)),
        tooltip: Some(tooltip),
        trailing_fold: true,
        ..ListRow::default()
    }
}

/// The row of `detected`, an agent started by hand, under `key`: which
/// agent it is over where it works, marked with how it stands, open on its
/// details or folded shut.
fn detected_row(key: usize, detected: &Detected, open: bool, width: usize) -> ListRow {
    ListRow {
        fold: Some(open),
        key,
        label: detected.agent.clone(),
        mark: Some(mark(detected.standing)),
        subtitle: detected
            .folder
            .as_deref()
            .map(|folder| fit_path(folder, width)),
        tooltip: Some(match &detected.folder {
            Some(folder) => format!("Started by hand in {folder}: {BY_HAND_NOTE}"),
            None => format!("Started by hand: {BY_HAND_NOTE}"),
        }),
        trailing_fold: true,
        ..ListRow::default()
    }
}

/// The rows of an agent's `details` under the agent's `key`, each its words
/// and what they are, set in under it; a detail it lacks has none.
fn detail_rows<'a>(
    key: usize,
    details: [Option<(&'a str, &'a str)>; KEYS_PER_AGENT - 1],
) -> impl Iterator<Item = ListRow> + 'a {
    details
        .into_iter()
        .enumerate()
        .filter_map(move |(at, detail)| {
            let (words, note) = detail?;
            Some(ListRow {
                depth: 1,
                key: key + 1 + at,
                label: words.to_string(),
                muted: true,
                tooltip: Some(note.to_string()),
                ..ListRow::default()
            })
        })
}

/// `folder` in no more than `width` cells where it can be: its folders
/// shortened from the left, each to its first letter (a hidden one's dot
/// kept), as fish's prompt does (`~/Workspace/apps/winter` as
/// `~/W/apps/winter`, then `~/W/a/winter`), only as far as it must be. The
/// last folder is never shortened, so a path that still does not fit is
/// cut; the row's tooltip says it in full.
pub(super) fn fit_path(folder: &str, width: usize) -> String {
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

/// The mark saying an agent stands as `standing`.
fn mark(standing: Standing) -> Mark {
    let (icon, text, color, meaning) = match standing {
        Standing::Working => (
            WORKING_ICON,
            WORKING,
            ThemeToken::Accent,
            "Working on a request",
        ),
        Standing::Waiting => (
            WAITING_ICON,
            WAITING,
            ThemeToken::Warning,
            "Waiting for you",
        ),
        Standing::Running => (RUNNING_ICON, RUNNING, ThemeToken::Muted, "Running"),
        Standing::Stopped => (
            STOPPED_ICON,
            STOPPED,
            ThemeToken::Muted,
            "Stopped: Enter opens it again",
        ),
    };
    Mark {
        color,
        icon: Some(IconKind::Symbol { name: icon }),
        meaning: meaning.to_string(),
        text: text.to_string(),
    }
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

    fn by_hand(tab: TabId) -> Detected {
        Detected {
            agent: "claude".to_string(),
            folder: Some("/src/app".to_string()),
            standing: Standing::Waiting,
            tab,
            task: Some("Refactor the parser".to_string()),
        }
    }

    fn filled() -> AgentsView {
        let mut view = AgentsView::default();
        view.set_shown(true);
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Working),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        view
    }

    /// The rows' words, and whether each heads a group or is set in.
    fn labels(view: &AgentsView) -> Vec<(String, bool, usize)> {
        view.list
            .rows
            .iter()
            .map(|row| (row.label.clone(), row.heading, row.depth))
            .collect()
    }

    /// The rows' subtitles.
    fn subtitles(view: &AgentsView) -> Vec<Option<&str>> {
        view.list
            .rows
            .iter()
            .map(|row| row.subtitle.as_deref())
            .collect()
    }

    #[test]
    fn test_coming_into_sight_asks_for_the_agents_once() {
        let mut view = AgentsView::default();
        assert!(!view.take_stale(), "out of sight, nothing");
        view.set_shown(true);
        assert!(view.take_stale());
        assert!(!view.take_stale(), "once");
    }

    #[test]
    fn test_enter_shows_the_agent_and_its_keys_act_on_it() {
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
    }

    #[test]
    fn test_each_agent_opens_on_its_details_and_they_lead_back_to_it() {
        let mut view = filled();
        let tab = TabId(7);
        view.set_detected(vec![by_hand(tab)]);
        let row = |label: &str, heading, depth| (label.to_string(), heading, depth);
        assert_eq!(
            labels(&view),
            [
                row("agent/claude-1", false, 0),
                row("main", false, 1),
                row("agent/codex-1", false, 0),
                row("main", false, 1),
                row("claude", false, 0),
                row("Refactor the parser", false, 1),
            ]
        );
        // Where each works, under its name.
        assert_eq!(
            subtitles(&view),
            [
                Some("/s/a/a/claude-1"),
                None,
                Some("/s/a/a/codex-1"),
                None,
                Some("/s/app"),
                None,
            ]
        );
        // The task line of the agent started by hand.
        let first = 2 * KEYS_PER_AGENT;
        view.list.select_key(first + 1, false);
        view.act(ListAct::Open);
        assert_eq!(view.take_request(), Some(AgentRequest::ShowTab(tab)));
        view.own_key(&Key::plain(KeyCode::Char('m')));
        assert_eq!(view.take_request(), None, "no branch to merge");
    }

    #[test]
    fn test_a_fold_shuts_an_agent_on_its_row_and_holds_as_it_is_filled_again() {
        let mut view = filled();
        view.list.select_key(1, false);
        view.act(ListAct::Collapse);
        assert_eq!(
            view.list.selected_row().map(|row| row.key),
            Some(0),
            "the cursor goes to the agent's own row"
        );
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Waiting),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        let shown: Vec<String> = labels(&view).into_iter().map(|(label, ..)| label).collect();
        assert_eq!(shown, ["agent/claude-1", "agent/codex-1", "main"]);
        view.act(ListAct::Expand);
        assert_eq!(view.list.rows.len(), 4);
    }

    #[test]
    fn test_one_click_shows_a_running_agent_and_never_starts_a_stopped_one() {
        let mut view = AgentsView::default();
        view.set_shown(true);
        view.set_agents(vec![
            managed("agent/claude-1", Standing::Running),
            managed("agent/claude-2", Standing::Stopped),
        ]);
        view.set_detected(vec![by_hand(TabId(7))]);
        let at_once: Vec<bool> = [0, 1, KEYS_PER_AGENT, KEYS_PER_AGENT + 1, 2 * KEYS_PER_AGENT]
            .into_iter()
            .map(|key| view.opens_on_click(key))
            .collect();
        assert_eq!(at_once, [true, true, false, false, true]);
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
        let subtitle = |view: &AgentsView| view.list.rows[0].subtitle.clone();
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
        // Regression: a status icon is drawn on a canvas, which misty, in a
        // debug build, refuses to know by its place alone once the rows
        // around it come and go: opening the view stopped it.
        let mut page = super::super::PanelPage::new(AgentsView::default());
        let window = misty::testing::TestWindow::new(400, 600);
        page.state_mut().set_shown(true);
        crate::editors::view_tests::drawn(&mut page, &window);
        page.state_mut().set_agents(vec![
            managed("agent/claude-1", Standing::Working),
            managed("agent/codex-1", Standing::Waiting),
        ]);
        page.state_mut().set_detected(vec![by_hand(TabId(7))]);
        crate::editors::view_tests::drawn(&mut page, &window);
        page.state_mut().set_detected(Vec::new());
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
        for mark in standings.into_iter().map(mark) {
            let name = mark.icon.as_ref().and_then(crate::icons::name_for);
            assert!(
                name.as_deref().is_some_and(crate::icons::contains),
                "{} has no bundled icon: {name:?}",
                mark.meaning
            );
        }
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = filled();
        view.set_detected(vec![by_hand(TabId(7))]);
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}

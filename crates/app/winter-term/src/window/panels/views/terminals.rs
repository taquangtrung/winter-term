//! The Terminals view (see `docs/design/agent-workspace.md`): every
//! terminal open in the window, the window's workspace's first, buried ones
//! among them, then each workspace put aside's under its name, each by what
//! it runs or last ran over where its shell is, and marked with how that
//! went, or how the agent in it stands. `Enter`, or a click, shows the
//! terminal, going to its workspace first. The window keeps it current while
//! it is shown and carries out what it asks
//! ([`TerminalsView::take_request`]).

use misty::{Element, HostedView, ThemeToken};

use winter_model::editor::EditorOutcome;
use winter_model::layout::TabId;
use winter_model::paint::IconKind;

use super::agents::{MARK_CELLS, ROW_SPARE_CELLS, WAITING_ICON, WORKING_ICON, fit_path};
use super::{ListAct, ListRow, Mark, PanelList, PanelView, list_view};

// ========================================================================
// Constants
// ========================================================================

/// What the panel's switcher calls the view.
const TITLE: &str = "Terminals";

/// What it says before it is first filled.
const LOOKING: &str = "Looking for terminals…";

/// What heads the terminals of a workspace put aside (`In notes`).
const WORKSPACE_HEADING: &str = "In";

/// What a terminal's mark says, where its icon is not bundled.
const DONE: &str = "done";
const RUNNING: &str = "running";
const WAITING: &str = "waiting";

/// The icons saying how a terminal's last command went, from the bundled
/// codicons.
const DONE_ICON: &str = "check";
const FAILED_ICON: &str = "error";

// ========================================================================
// Data Structures
// ========================================================================

/// A terminal, as the view shows it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) struct TerminalEntry {
    /// Where its shell is.
    pub(in crate::window) folder: Option<String>,
    pub(in crate::window) ran: Ran,
    pub(in crate::window) tab: TabId,
    /// What it runs, or last ran: the command line, else the program, else
    /// its tab's name.
    pub(in crate::window) task: String,
    /// The workspace put aside it is in; `None` for the window's own.
    pub(in crate::window) workspace: Option<String>,
}

/// How a terminal's command went, or how the agent in it stands.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Ran {
    /// Its last command failed, with this exit code.
    Failed(i32),
    /// Its last command finished well.
    Finished,
    /// It has run nothing the shell said, or did not say how it went.
    Idle,
    /// A command runs in it, or the agent in it works.
    Running,
    /// The agent in it waits for you.
    Waiting,
}

/// Every terminal open in the window, and the one asked to be shown.
#[derive(Debug, Default)]
pub(in crate::window) struct TerminalsView {
    /// How many cells across the panel is, which its paths are shortened
    /// to fit; 0 before it is first drawn.
    cols: usize,
    /// The terminals, in the rows' order; `None` before it is first filled.
    entries: Option<Vec<TerminalEntry>>,
    list: PanelList,
    /// The terminal asked to be shown, until the window takes it.
    request: Option<TabId>,
    /// Whether it is in sight, the only time it is filled.
    shown: bool,
    /// Whether it came into sight and wants filling.
    stale: bool,
}

// ========================================================================
// TerminalsView Implementations
// ========================================================================

impl TerminalsView {
    /// Show `entries`, the window's terminals.
    pub(in crate::window) fn set_terminals(&mut self, entries: Vec<TerminalEntry>) {
        if self.entries.as_ref() != Some(&entries) {
            self.entries = Some(entries);
            self.build_rows();
        }
    }

    /// Whether it is in sight, the only time it wants what changes.
    pub(in crate::window) fn is_shown(&self) -> bool {
        self.shown
    }

    /// Whether it came into sight and wants filling, taken so it is filled
    /// once.
    pub(in crate::window) fn take_stale(&mut self) -> bool {
        std::mem::take(&mut self.stale) && self.shown
    }

    /// The terminal asked to be shown, taken so it is shown once.
    pub(in crate::window) fn take_request(&mut self) -> Option<TabId> {
        self.request.take()
    }

    /// The terminals filled in so far.
    fn entries(&self) -> &[TerminalEntry] {
        self.entries.as_deref().unwrap_or_default()
    }

    /// The rows: each terminal, keyed by its place, under a heading where
    /// the workspace changes to one put aside.
    fn build_rows(&mut self) {
        let width = self.path_width();
        let mut rows = Vec::new();
        let mut group = None;
        for (key, entry) in self.entries().iter().enumerate() {
            // Keyed as the first of its terminals, so no two headings share
            // a key.
            if entry.workspace != group {
                if let Some(name) = &entry.workspace {
                    rows.push(ListRow {
                        heading: true,
                        key,
                        label: format!("{WORKSPACE_HEADING} {name}"),
                        ..ListRow::default()
                    });
                }
                group.clone_from(&entry.workspace);
            }
            rows.push(terminal_row(key, entry, width));
        }
        self.list.set_rows(rows);
    }

    /// How many cells a path may take under its row's words: the panel's
    /// width, less the space around a row and the marks' column, where any
    /// row has a mark.
    fn path_width(&self) -> usize {
        let marked = self
            .entries()
            .iter()
            .any(|entry| terminal_mark(entry.ran).is_some());
        let marks = match marked {
            true => MARK_CELLS,
            false => 0,
        };
        self.cols.saturating_sub(ROW_SPARE_CELLS + marks)
    }

    /// The terminal the cursor is on.
    fn selected(&self) -> Option<&TerminalEntry> {
        let row = self.list.selected_row().filter(|row| !row.heading)?;
        self.entries().get(row.key)
    }
}

impl PanelView for TerminalsView {
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
            ListAct::Open => self.request = self.selected().map(|entry| entry.tab),
            ListAct::Refresh => self.stale = true,
            ListAct::Collapse
            | ListAct::Down
            | ListAct::Expand
            | ListAct::First
            | ListAct::Last
            | ListAct::OpenTool
            | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }

    /// A terminal shows at one click: there is nothing to start.
    fn opens_on_click(&self, key: usize) -> bool {
        self.entries().get(key).is_some()
    }

    /// Its paths are shortened again when the panel's width changes.
    fn fit(&mut self, _rows: usize, cols: usize) {
        if cols != self.cols {
            self.cols = cols;
            self.build_rows();
        }
    }

    /// Filled again each time it comes into sight.
    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        self.stale |= shown;
        EditorOutcome::Consumed
    }
}

impl HostedView for TerminalsView {
    fn view(&self) -> Element<'_, Self> {
        list_view(self, LOOKING)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `entry` under `key`: what it runs or last ran over where its
/// shell is, marked with how that went.
fn terminal_row(key: usize, entry: &TerminalEntry, width: usize) -> ListRow {
    ListRow {
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

/// The mark saying how a terminal's command went, or how the agent in it
/// stands, as `ran` says: its icon, in its color, its words for where the
/// icon is not bundled, and what it means on hover; none for one that ran
/// nothing.
fn terminal_mark(ran: Ran) -> Option<Mark> {
    let (icon, text, color, meaning) = match ran {
        Ran::Failed(code) => (
            FAILED_ICON,
            format!("exit {code}"),
            ThemeToken::Danger,
            format!("Its last command failed with exit code {code}"),
        ),
        Ran::Finished => (
            DONE_ICON,
            DONE.to_string(),
            ThemeToken::Muted,
            "Its last command finished".to_string(),
        ),
        Ran::Idle => return None,
        Ran::Running => (
            WORKING_ICON,
            RUNNING.to_string(),
            ThemeToken::Accent,
            "Running".to_string(),
        ),
        Ran::Waiting => (
            WAITING_ICON,
            WAITING.to_string(),
            ThemeToken::Warning,
            "The agent in it waits for you".to_string(),
        ),
    };
    Some(Mark {
        color,
        icon: Some(IconKind::Symbol { name: icon }),
        meaning,
        text,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A terminal of `workspace` (the window's own when `None`), running
    /// `task` in tab `tab`, as `ran` says.
    fn terminal(tab: u64, task: &str, ran: Ran, workspace: Option<&str>) -> TerminalEntry {
        TerminalEntry {
            folder: Some("/src/app".to_string()),
            ran,
            tab: TabId(tab),
            task: task.to_string(),
            workspace: workspace.map(str::to_string),
        }
    }

    fn filled() -> TerminalsView {
        let mut view = TerminalsView::default();
        view.set_shown(true);
        view.set_terminals(vec![
            terminal(1, "cargo test", Ran::Running, None),
            terminal(2, "make lint", Ran::Failed(2), None),
            terminal(3, "claude", Ran::Waiting, None),
            terminal(4, "zsh", Ran::Idle, Some("notes")),
        ]);
        view
    }

    #[test]
    fn test_the_window_s_terminals_come_first_then_each_workspace_put_aside_s() {
        let view = filled();
        let rows: Vec<(&str, bool, Option<&str>)> = view
            .list
            .rows
            .iter()
            .map(|row| {
                let mark = row.mark.as_ref().map(|mark| mark.text.as_str());
                (row.label.as_str(), row.heading, mark)
            })
            .collect();
        assert_eq!(
            rows,
            [
                ("cargo test", false, Some(RUNNING)),
                ("make lint", false, Some("exit 2")),
                ("claude", false, Some(WAITING)),
                ("In notes", true, None),
                ("zsh", false, None),
            ]
        );
    }

    #[test]
    fn test_one_click_or_enter_shows_the_terminal_of_another_workspace_too() {
        let mut view = filled();
        assert!(view.opens_on_click(3));
        view.list.select_key(3, false);
        view.act(ListAct::Open);
        assert_eq!(view.take_request(), Some(TabId(4)));
        assert_eq!(view.take_request(), None, "taken once");
    }

    #[test]
    fn test_coming_into_sight_asks_to_be_filled_once() {
        let mut view = TerminalsView::default();
        assert!(!view.take_stale(), "out of sight, nothing");
        view.set_shown(true);
        assert!(view.take_stale());
        assert!(!view.take_stale(), "once");
    }

    #[test]
    fn test_every_state_is_drawn_as_a_bundled_icon() {
        let rans = [Ran::Failed(2), Ran::Finished, Ran::Running, Ran::Waiting];
        for mark in rans.into_iter().filter_map(terminal_mark) {
            let name = mark.icon.as_ref().and_then(crate::icons::name_for);
            assert!(
                name.as_deref().is_some_and(crate::icons::contains),
                "{} has no bundled icon: {name:?}",
                mark.meaning
            );
        }
    }

    #[test]
    fn test_rows_coming_and_going_keep_each_icon_its_own() {
        // As in the Agents view: a status icon is a canvas, which misty, in
        // a debug build, refuses to know by its place once rows come and go.
        let mut page = super::super::PanelPage::new(TerminalsView::default());
        let window = misty::testing::TestWindow::new(400, 600);
        page.state_mut().set_shown(true);
        crate::editors::view_tests::drawn(&mut page, &window);
        page.state_mut()
            .set_terminals(vec![terminal(1, "cargo test", Ran::Running, None)]);
        crate::editors::view_tests::drawn(&mut page, &window);
        page.state_mut().set_terminals(Vec::new());
        crate::editors::view_tests::drawn(&mut page, &window);
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let (unnamed, unlit) = super::super::tests::control_faults_in(filled());
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}

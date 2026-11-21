//! The Output view, the bottom panel's: the builds run, newest first, down
//! its right, and the end of the one the cursor is on beside them, as it
//! is written. The window hands it the builds and that build's last lines
//! as they come. `t`, or opening a build, shows the whole of it in the
//! main area.

use misty::view::Extent;
use misty::view::{Space, column, row, text};
use misty::{Element, HostedView, ThemeToken};

use winter_model::editor::EditorOutcome;
use winter_model::layout::TabId;

use super::{ListAct, ListRow, Mark, PanelList, PanelView, list_view};
use crate::editors::compile::RunState;

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Output";

/// What it says before any build has run, and over a build with no output.
const NO_BUILDS: &str = "No builds run yet";
const NO_OUTPUT: &str = "No output yet";

/// How wide the list of builds is, in logical pixels.
const RUNS_WIDTH_PX: f32 = 220.0;

/// The space around the output's lines, in logical pixels across and down.
const LINES_PADDING_PX: (f32, f32) = (10.0, 4.0);

// ========================================================================
// Data Structures
// ========================================================================

/// The builds run, and the end of the one the cursor is on.
#[derive(Debug, Default)]
pub(in crate::window) struct OutputView {
    /// How many lines of output fit beside the builds.
    fits: usize,
    /// The last lines of the build the cursor is on, oldest first.
    lines: Vec<String>,
    /// The builds, as the rows list them.
    list: PanelList,
    /// The builds, newest first.
    runs: Vec<Run>,
    /// The build asked to be shown in the main area, until the window takes
    /// it.
    shown: Option<TabId>,
}

/// One build the view lists.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::window) struct Run {
    pub(in crate::window) name: String,
    pub(in crate::window) state: RunState,
    pub(in crate::window) tab: TabId,
}

// ========================================================================
// OutputView Implementations
// ========================================================================

impl OutputView {
    /// Take `runs`, the builds newest first.
    pub(in crate::window) fn set_runs(&mut self, runs: Vec<Run>) {
        if runs == self.runs {
            return;
        }
        let rows = runs
            .iter()
            .enumerate()
            .map(|(key, run)| run_row(key, run))
            .collect();
        let was = self.selected_run();
        self.runs = runs;
        self.list.set_rows(rows);
        if let Some(at) = was.and_then(|was| self.runs.iter().position(|run| run.tab == was)) {
            self.list.select_key(at, false);
        }
    }

    /// Put the cursor on the build of tab `tab`: a build just started.
    pub(in crate::window) fn select_run(&mut self, tab: TabId) {
        if let Some(at) = self.runs.iter().position(|run| run.tab == tab) {
            self.list.select_key(at, false);
        }
    }

    /// The build the cursor is on.
    pub(in crate::window) fn selected_run(&self) -> Option<TabId> {
        let row = self.list.selected_row()?;
        self.runs.get(row.key).map(|run| run.tab)
    }

    /// How many of its last lines to hand over.
    pub(in crate::window) fn fits(&self) -> usize {
        self.fits
    }

    /// Take `lines`, the last of the build the cursor is on.
    pub(in crate::window) fn set_lines(&mut self, lines: Vec<String>) {
        self.lines = lines;
    }

    /// The build asked to be shown in the main area, taken once.
    pub(in crate::window) fn take_shown(&mut self) -> Option<TabId> {
        self.shown.take()
    }
}

impl PanelView for OutputView {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn list(&self) -> &PanelList {
        &self.list
    }

    fn list_mut(&mut self) -> &mut PanelList {
        &mut self.list
    }

    fn fit(&mut self, rows: usize) {
        self.fits = rows;
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match act {
            ListAct::Open | ListAct::OpenTool => self.shown = self.selected_run(),
            ListAct::Collapse
            | ListAct::Down
            | ListAct::Expand
            | ListAct::First
            | ListAct::Last
            | ListAct::Refresh
            | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }
}

impl HostedView for OutputView {
    fn view(&self) -> Element<'_, Self> {
        if self.runs.is_empty() {
            return list_view(self, NO_BUILDS);
        }
        let lines: Element<'_, Self> = match self.lines.is_empty() {
            true => text(NO_OUTPUT).muted().proportional().into(),
            false => column(
                self.lines
                    .iter()
                    .map(|line| text(line.as_str()).into())
                    .collect::<Vec<_>>(),
            ),
        };
        let lines = lines
            .padding_px(LINES_PADDING_PX.0, LINES_PADDING_PX.1)
            .grow(1.0);
        let runs = list_view(self, NO_BUILDS)
            .width(Extent::Px(RUNS_WIDTH_PX))
            .border(ThemeToken::Border);
        row([lines, runs]).gap(Space::Px(0.0))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `run`, the `key`th newest: its name and how it stands.
fn run_row(key: usize, run: &Run) -> ListRow {
    let (text, color, meaning) = match run.state {
        RunState::Running => ("running", ThemeToken::Accent, "Still running"),
        RunState::Succeeded => ("done", ThemeToken::Muted, "Ended without an error"),
        RunState::Failed => ("failed", ThemeToken::Danger, "Ended with an error"),
        RunState::Stopped => ("stopped", ThemeToken::Warning, "Stopped before its end"),
        RunState::Idle => ("waiting", ThemeToken::Muted, "Not started yet"),
    };
    ListRow {
        key,
        label: run.name.clone(),
        mark: Some(Mark {
            color,
            meaning: meaning.to_string(),
            text: text.to_string(),
        }),
        tooltip: Some(run.name.clone()),
        ..ListRow::default()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn run(tab: u64, state: RunState) -> Run {
        Run {
            name: format!("build {tab}"),
            state,
            tab: TabId(tab),
        }
    }

    #[test]
    fn test_the_cursor_stays_on_its_build_as_newer_ones_arrive_and_opening_shows_it() {
        let mut view = OutputView::default();
        view.set_runs(vec![run(1, RunState::Running)]);
        view.set_runs(vec![run(2, RunState::Running), run(1, RunState::Failed)]);
        assert_eq!(view.selected_run(), Some(TabId(1)), "still on build 1");
        view.select_run(TabId(2));
        view.act(ListAct::Open);
        assert_eq!(view.take_shown(), Some(TabId(2)));
        assert_eq!(view.take_shown(), None);
    }
}

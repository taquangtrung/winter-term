//! The Output view, the bottom panel's: the builds run, newest first, down
//! its right, and the end of the one the cursor is on beside them, as it
//! is written. `l` or `RET` on a build reads the whole of it there, its
//! lines the list, following the end while the cursor is on the last line;
//! `h` goes back to the builds, and `]` and `[` read the next and previous
//! build. `t` shows the build in the main area. A run stays until `d` lets
//! it go, whatever happens to its tab.
//!
//! The window hands it the builds, and the lines of the one shown as they
//! come: the last that fit beside the builds, or every new line of the one
//! being read.

use misty::view::Extent;
use misty::view::{Space, column, row, text};
use misty::{Element, HostedView, ThemeToken};

use winter_model::editor::EditorOutcome;
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::layout::TabId;

use super::{ListAct, ListRow, Mark, PanelList, PanelView, list_view};
use crate::editors::compile::RunState;

// ========================================================================
// Constants
// ========================================================================

/// The Output view's own keys, block `panel_output`, ahead of the panel
/// keys.
pub(super) const KEYS: KeyTable<OutputAct> = KeyTable {
    block: "panel_output",
    bindings: &[
        Binding {
            action: OutputAct::Remove,
            id: "remove_run",
            keys: &["d"],
            label: "Remove Run",
        },
        Binding {
            action: OutputAct::Next,
            id: "next_run",
            keys: &["]"],
            label: "Read the Next Run",
        },
        Binding {
            action: OutputAct::Previous,
            id: "previous_run",
            keys: &["["],
            label: "Read the Previous Run",
        },
    ],
    configurable: true,
    title: "Output View",
};

/// What the view is called.
const TITLE: &str = "Output";

/// What it says before any build has run, and over a build with no output.
const NO_BUILDS: &str = "No builds run yet";
const NO_OUTPUT: &str = "No output yet";

/// What the heading over a build read says to the pointer.
const READ_HINT: &str = "Back to the builds: h; the next and previous build: ] and [";

/// How wide the list of builds is, in logical pixels.
const RUNS_WIDTH_PX: f32 = 220.0;

/// The space around the output's lines, in logical pixels across and down.
const LINES_PADDING_PX: (f32, f32) = (10.0, 4.0);

// ========================================================================
// Data Structures
// ========================================================================

/// What an Output view key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum OutputAct {
    Next,
    Previous,
    Remove,
}

/// The builds run, and the end of the one the cursor is on.
#[derive(Debug, Default)]
pub(in crate::window) struct OutputView {
    /// How many lines of output fit beside the builds.
    fits: usize,
    /// The last lines of the build the cursor is on, oldest first.
    lines: Vec<String>,
    /// The builds, as the rows list them, or the lines of the one read.
    list: PanelList,
    /// The build whose lines are the list, while one is read.
    reading: Option<TabId>,
    /// The build asked to be let go, until the window takes it.
    removed: Option<TabId>,
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
    /// Take `runs`, the builds newest first. A build read that is gone
    /// leaves the list of builds shown.
    pub(in crate::window) fn set_runs(&mut self, runs: Vec<Run>) {
        if runs == self.runs {
            return;
        }
        let was = self.selected_run();
        self.runs = runs;
        let gone = self
            .reading
            .is_some_and(|read| self.runs.iter().all(|run| run.tab != read));
        if gone {
            self.reading = None;
        }
        if self.reading.is_none() {
            self.list_runs();
            if let Some(was) = was {
                self.select_run(was);
            }
        }
    }

    /// Put the cursor on the build of tab `tab`: a build just started.
    pub(in crate::window) fn select_run(&mut self, tab: TabId) {
        if self.reading.is_some() {
            return;
        }
        if let Some(at) = self.runs.iter().position(|run| run.tab == tab) {
            self.list.select_key(at, false);
        }
    }

    /// The build read, else the one the cursor is on.
    pub(in crate::window) fn selected_run(&self) -> Option<TabId> {
        if self.reading.is_some() {
            return self.reading;
        }
        let row = self.list.selected_row()?;
        self.runs.get(row.key).map(|run| run.tab)
    }

    /// The build read and how many of its lines the view holds, for the
    /// window to hand over the rest.
    pub(in crate::window) fn reading(&self) -> Option<(TabId, usize)> {
        Some((self.reading?, self.list.rows.len()))
    }

    /// Take `lines` of the build read, from its line `from` on: its next
    /// lines, or all of it again from nought when it started over.
    pub(in crate::window) fn add_lines(&mut self, from: usize, lines: Vec<String>) {
        let rows = lines
            .into_iter()
            .enumerate()
            .map(|(at, line)| line_row(from + at, line))
            .collect();
        match from {
            0 => {
                self.list.set_rows(Vec::new());
                self.list.extend_rows(rows);
            }
            _ => self.list.extend_rows(rows),
        }
    }

    /// How many of its last lines to hand over beside the builds.
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

    /// The build asked to be let go, taken once.
    pub(in crate::window) fn take_removed(&mut self) -> Option<TabId> {
        self.removed.take()
    }

    /// List the builds.
    fn list_runs(&mut self) {
        let rows = self
            .runs
            .iter()
            .enumerate()
            .map(|(key, run)| run_row(key, run))
            .collect();
        self.list.set_rows(rows);
    }

    /// Read the build of tab `tab`, its lines the list once the window
    /// hands them over.
    fn read(&mut self, tab: TabId) {
        self.reading = Some(tab);
        self.list.set_rows(Vec::new());
    }

    /// Go back to the builds, the cursor on the one read.
    fn stop_reading(&mut self) {
        let Some(read) = self.reading.take() else {
            return;
        };
        self.list_runs();
        self.select_run(read);
    }

    /// Read the build `step` along the list from the one read.
    fn read_along(&mut self, step: isize) {
        let at = self
            .reading
            .and_then(|read| self.runs.iter().position(|run| run.tab == read));
        let next = at
            .and_then(|at| at.checked_add_signed(step))
            .and_then(|at| self.runs.get(at));
        if let Some(next) = next.map(|run| run.tab) {
            self.read(next);
        }
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

    fn fit(&mut self, rows: usize, _cols: usize) {
        self.fits = rows;
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match (act, self.reading) {
            (ListAct::Open | ListAct::Expand, None) => {
                if let Some(run) = self.selected_run() {
                    self.read(run);
                }
            }
            (ListAct::Collapse, Some(_)) => self.stop_reading(),
            (ListAct::OpenTool, _) | (ListAct::Open, Some(_)) => {
                self.shown = self.selected_run();
            }
            (ListAct::Collapse | ListAct::Expand, _)
            | (
                ListAct::Down
                | ListAct::First
                | ListAct::Last
                | ListAct::Refresh
                | ListAct::Toggle
                | ListAct::Up,
                _,
            ) => {}
        }
        EditorOutcome::Consumed
    }

    fn own_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        match table::find(&KEYS, key)? {
            OutputAct::Next => self.read_along(1),
            OutputAct::Previous => self.read_along(-1),
            OutputAct::Remove => self.removed = self.selected_run(),
        }
        Some(EditorOutcome::Consumed)
    }
}

impl HostedView for OutputView {
    fn view(&self) -> Element<'_, Self> {
        if self.runs.is_empty() {
            return list_view(self, NO_BUILDS);
        }
        if let Some(read) = self.reading {
            let name = self
                .runs
                .iter()
                .find(|run| run.tab == read)
                .map_or("", |run| run.name.as_str());
            let head = Element::from(text(name).muted().emphasized())
                .padding_px(LINES_PADDING_PX.0, LINES_PADDING_PX.1)
                .tooltip(READ_HINT);
            return column([head, list_view(self, NO_OUTPUT)]).gap(Space::Px(0.0));
        }
        let lines: Element<'_, Self> = match self.lines.is_empty() {
            true => text(NO_OUTPUT).muted().into(),
            false => column(
                self.lines
                    .iter()
                    .map(|line| text(line.as_str()).monospace().into())
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
            icon: None,
            meaning: meaning.to_string(),
            text: text.to_string(),
        }),
        tooltip: Some(run.name.clone()),
        ..ListRow::default()
    }
}

/// The row of a build's line `line`, its `key`th.
fn line_row(key: usize, line: String) -> ListRow {
    ListRow {
        cells: true,
        key,
        label: line,
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
    fn test_the_cursor_stays_on_its_build_as_newer_ones_arrive_and_t_shows_it() {
        let mut view = OutputView::default();
        view.set_runs(vec![run(1, RunState::Running)]);
        view.set_runs(vec![run(2, RunState::Running), run(1, RunState::Failed)]);
        assert_eq!(view.selected_run(), Some(TabId(1)), "still on build 1");
        view.select_run(TabId(2));
        view.act(ListAct::OpenTool);
        assert_eq!(view.take_shown(), Some(TabId(2)));
        assert_eq!(view.take_shown(), None);
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = OutputView::default();
        view.set_runs(vec![run(2, RunState::Running), run(1, RunState::Failed)]);
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }

    #[test]
    fn test_a_build_read_follows_its_end_until_the_cursor_leaves_it() {
        let mut view = OutputView::default();
        view.set_runs(vec![run(2, RunState::Running), run(1, RunState::Failed)]);
        view.act(ListAct::Open);
        assert_eq!(view.reading(), Some((TabId(2), 0)), "build 2 read");
        let on = |view: &OutputView| view.list.selected_row().map(|row| row.key);
        let lines = |words: &[&str]| words.iter().map(|word| word.to_string()).collect();
        view.add_lines(0, lines(&["a", "b"]));
        assert_eq!(on(&view), Some(1), "on the last line");
        view.add_lines(2, lines(&["c"]));
        assert_eq!(on(&view), Some(2), "following");
        view.list.move_cursor(ListAct::Up);
        view.add_lines(3, lines(&["d"]));
        assert_eq!(on(&view), Some(1), "moved away, it stays");
        view.list.move_cursor(ListAct::Last);
        view.add_lines(4, lines(&["e"]));
        assert_eq!(on(&view), Some(4), "following again");
        view.add_lines(0, lines(&["again"]));
        assert_eq!(view.reading(), Some((TabId(2), 1)), "started over");

        view.own_key(&Key {
            alt: false,
            code: winter_model::input::KeyCode::Char(']'),
            ctrl: false,
            shift: false,
            sup: false,
        });
        assert_eq!(view.reading(), Some((TabId(1), 0)), "the next build read");
        view.act(ListAct::Collapse);
        assert_eq!(view.reading(), None, "back to the builds");
        assert_eq!(view.selected_run(), Some(TabId(1)), "on the one read");
    }
}

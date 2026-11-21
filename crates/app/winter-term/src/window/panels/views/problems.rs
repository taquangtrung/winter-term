//! The Problems view, the bottom panel's: the errors and warnings the
//! builds found, one row each, under the file each is in. The window hands
//! it every build's places as they come; opening one shows its line in the
//! main area, and `t` shows the newest build there.

use std::path::{Path, PathBuf};

use misty::{Element, HostedView, ThemeToken};

use winter_model::editor::{EditorOutcome, OpenTarget};
use winter_model::layout::TabId;
use winter_model::paint::DiagnosticLevel;

use super::{ListAct, ListRow, Mark, PanelList, PanelView, list_view};
use crate::editors::compile::Problem;

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Problems";

/// What it says with nothing found.
const NONE_FOUND: &str = "No problems in the builds run";

// ========================================================================
// Data Structures
// ========================================================================

/// The places the builds pointed at.
#[derive(Debug, Default)]
pub(in crate::window) struct ProblemsView {
    /// The builds' tabs, newest last, with what each found.
    builds: Vec<(TabId, Vec<Problem>)>,
    list: PanelList,
    /// Every problem listed, in the rows' order: by file, then line.
    listed: Vec<Problem>,
    /// The build asked to be shown in the main area, until the window takes
    /// it.
    shown: Option<TabId>,
}

// ========================================================================
// ProblemsView Implementations
// ========================================================================

impl ProblemsView {
    /// Take `problems`, what the build of tab `build` has found so far, in
    /// place of what it found before.
    pub(in crate::window) fn set_build(&mut self, build: TabId, problems: Vec<Problem>) {
        match self.builds.iter_mut().find(|(tab, _)| *tab == build) {
            Some((_, held)) if *held == problems => return,
            Some((_, held)) => *held = problems,
            None => self.builds.push((build, problems)),
        }
        self.relist();
    }

    /// Forget the build of tab `build`, which has closed.
    pub(in crate::window) fn forget(&mut self, build: TabId) {
        let before = self.builds.len();
        self.builds.retain(|(tab, _)| *tab != build);
        if self.builds.len() != before {
            self.relist();
        }
    }

    /// The build asked to be shown in the main area, taken once.
    pub(in crate::window) fn take_shown(&mut self) -> Option<TabId> {
        self.shown.take()
    }

    /// How many errors and warnings are listed.
    pub(in crate::window) fn counts(&self) -> (usize, usize) {
        let count = |level| {
            self.listed
                .iter()
                .filter(|problem| problem.level == level)
                .count()
        };
        (
            count(DiagnosticLevel::Error),
            count(DiagnosticLevel::Warning),
        )
    }

    /// List every build's problems, by file then line, each file's name
    /// over its own.
    fn relist(&mut self) {
        let mut listed: Vec<Problem> = self
            .builds
            .iter()
            .flat_map(|(_, problems)| problems.iter().cloned())
            .collect();
        listed.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
        listed.dedup();
        let mut rows = Vec::new();
        let mut file: Option<&Path> = None;
        for (key, problem) in listed.iter().enumerate() {
            if file != Some(problem.path.as_path()) {
                file = Some(problem.path.as_path());
                let held = listed
                    .iter()
                    .filter(|other| other.path == problem.path)
                    .count();
                rows.push(ListRow {
                    detail: Some(held.to_string()),
                    heading: true,
                    key,
                    label: file_name(&problem.path),
                    tooltip: Some(problem.path.display().to_string()),
                    ..ListRow::default()
                });
            }
            rows.push(problem_row(key, problem));
        }
        self.listed = listed;
        self.list.set_rows(rows);
    }
}

impl PanelView for ProblemsView {
    fn title(&self) -> String {
        match self.counts() {
            (0, 0) => TITLE.to_string(),
            (errors, warnings) => format!("{TITLE} {}", errors + warnings),
        }
    }

    fn list(&self) -> &PanelList {
        &self.list
    }

    fn list_mut(&mut self) -> &mut PanelList {
        &mut self.list
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match act {
            ListAct::Open => {
                let row = self.list.selected_row().filter(|row| !row.heading);
                if let Some(problem) = row.and_then(|row| self.listed.get(row.key)) {
                    return EditorOutcome::OpenPath(OpenTarget::at(
                        problem.path.clone(),
                        problem.line,
                        problem.column,
                    ));
                }
            }
            ListAct::OpenTool => self.shown = self.builds.last().map(|(tab, _)| *tab),
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

impl HostedView for ProblemsView {
    fn view(&self) -> Element<'_, Self> {
        list_view(self, NONE_FOUND)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The row of `problem`, the `key`th listed: what the tool said, its level
/// as a colored mark, and its line and column.
fn problem_row(key: usize, problem: &Problem) -> ListRow {
    let place = match problem.column {
        Some(column) => format!("{}:{column}", problem.line),
        None => problem.line.to_string(),
    };
    let (text, color, meaning) = match problem.level {
        DiagnosticLevel::Error => ("error", ThemeToken::Danger, "An error"),
        DiagnosticLevel::Warning => ("warning", ThemeToken::Warning, "A warning"),
        DiagnosticLevel::Info => ("note", ThemeToken::Muted, "A note"),
    };
    ListRow {
        depth: 1,
        detail: Some(place.clone()),
        key,
        label: problem.message.clone(),
        mark: Some(Mark {
            color,
            meaning: meaning.to_string(),
            text: text.to_string(),
        }),
        tooltip: Some(format!(
            "{}:{place}: {}",
            problem.path.display(),
            problem.message
        )),
        ..ListRow::default()
    }
}

/// The last part of `path`.
fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| PathBuf::from(path).display().to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn problem(path: &str, line: usize, level: DiagnosticLevel) -> Problem {
        Problem {
            column: None,
            level,
            line,
            message: format!("at {line}"),
            path: PathBuf::from(path),
        }
    }

    #[test]
    fn test_a_build_s_new_places_replace_its_old_ones_and_files_head_their_problems() {
        let mut view = ProblemsView::default();
        view.set_build(TabId(1), vec![problem("/b.rs", 4, DiagnosticLevel::Error)]);
        view.set_build(
            TabId(2),
            vec![
                problem("/a.rs", 9, DiagnosticLevel::Warning),
                problem("/a.rs", 2, DiagnosticLevel::Error),
            ],
        );
        let labels: Vec<String> = view.list.rows.iter().map(|row| row.label.clone()).collect();
        assert_eq!(labels, ["a.rs", "at 2", "at 9", "b.rs", "at 4"]);
        assert_eq!(view.counts(), (2, 1));
        view.set_build(TabId(1), Vec::new());
        assert_eq!(view.counts(), (1, 1), "build 1's error fixed");
        view.forget(TabId(2));
        assert_eq!(view.title(), TITLE);
    }
}

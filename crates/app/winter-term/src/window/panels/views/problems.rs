//! The Problems view, the bottom panel's: the errors and warnings the
//! builds found, one row each, under the file each is in. The window hands
//! it every source's places as they come; opening one shows its line in the
//! main area, and `t` shows the newest build there.
//!
//! A scope row heads it: `All`, then each source with problems and how
//! many, the one in use marked. `]` and `[` step through it, and a click
//! picks one.

use std::path::{Path, PathBuf};

use misty::view::{Corners, Extent, Press, Space, column, row, text};
use misty::{Element, HostedView, ThemeToken};

use winter_lsp::diagnostics::Backend;
use winter_model::editor::{EditorOutcome, OpenTarget};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::layout::TabId;
use winter_model::paint::{DiagnosticLevel, IconKind};

use super::{ListAct, ListRow, PanelList, PanelView, RowIcon, list_view};
use crate::editors::compile::Problem;

// ========================================================================
// Constants
// ========================================================================

/// The Problems view's own keys, block `panel_problems`, ahead of the
/// panel keys.
pub(super) const KEYS: KeyTable<ProblemsAct> = KeyTable {
    block: "panel_problems",
    bindings: &[
        Binding {
            action: ProblemsAct::Next,
            id: "next_source",
            keys: &["]"],
            label: "Next Source",
        },
        Binding {
            action: ProblemsAct::Previous,
            id: "previous_source",
            keys: &["["],
            label: "Previous Source",
        },
    ],
    configurable: true,
    title: "Problems View",
};

/// What the view is called.
const TITLE: &str = "Problems";

/// What the scope row calls every source at once.
const ALL: &str = "All";

/// The space around a scope's words, and between scopes, in logical pixels.
const SCOPE_PADDING_PX: (f32, f32) = (8.0, 2.0);
const SCOPE_GAP_PX: f32 = 4.0;

/// What it says with nothing found.
const NONE_FOUND: &str = "No problems in the builds run";

// ========================================================================
// Data Structures
// ========================================================================

/// What a Problems view key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum ProblemsAct {
    Next,
    Previous,
}

/// What found a problem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum Source {
    /// The build of a Compile tab.
    Build(TabId),
    /// A checker the window runs: a language server, an extension's.
    Checker(Backend),
}

/// The places the sources pointed at.
#[derive(Debug, Default)]
pub(in crate::window) struct ProblemsView {
    list: PanelList,
    /// Every problem listed, in the rows' order: by file, then line.
    listed: Vec<Problem>,
    /// The checkers' store as last read, by the count of its changes.
    checked: Option<u64>,
    /// The source whose problems are listed, or `None` for all of them.
    scope: Option<Source>,
    /// The build asked to be shown in the main area, until the window takes
    /// it.
    shown: Option<TabId>,
    /// The sources, oldest first, with what each found.
    sources: Vec<Found>,
}

/// What one source found, and what it is called.
#[derive(Debug)]
struct Found {
    name: String,
    problems: Vec<Problem>,
    source: Source,
}

// ========================================================================
// ProblemsView Implementations
// ========================================================================

impl ProblemsView {
    /// Take `problems`, what `source`, called `name`, has found so far, in
    /// place of what it found before.
    pub(in crate::window) fn set_source(
        &mut self,
        source: Source,
        name: String,
        problems: Vec<Problem>,
    ) {
        match self.sources.iter_mut().find(|found| found.source == source) {
            Some(found) if found.problems == problems && found.name == name => return,
            Some(found) => {
                found.name = name;
                found.problems = problems;
            }
            None => self.sources.push(Found {
                name,
                problems,
                source,
            }),
        }
        self.relist();
    }

    /// Whether the checkers' store has changed since the view last took it,
    /// as of its count of changes `generation`.
    pub(in crate::window) fn is_checked(&self, generation: u64) -> bool {
        self.checked == Some(generation)
    }

    /// Take `found`, what each checker has found as of `generation`, in
    /// place of what the checkers had found before.
    pub(in crate::window) fn set_checkers(
        &mut self,
        generation: u64,
        found: Vec<(Backend, String, Vec<Problem>)>,
    ) {
        self.checked = Some(generation);
        self.sources
            .retain(|held| matches!(held.source, Source::Build(_)));
        self.sources
            .extend(found.into_iter().map(|(backend, name, problems)| Found {
                name,
                problems,
                source: Source::Checker(backend),
            }));
        if let Some(Source::Checker(backend)) = self.scope {
            let kept = self
                .sources
                .iter()
                .any(|held| held.source == Source::Checker(backend));
            if !kept {
                self.scope = None;
            }
        }
        self.relist();
    }

    /// Forget the build of tab `build`, which is gone.
    pub(in crate::window) fn forget(&mut self, build: TabId) {
        let before = self.sources.len();
        self.sources
            .retain(|found| found.source != Source::Build(build));
        if self.sources.len() != before {
            if self.scope == Some(Source::Build(build)) {
                self.scope = None;
            }
            self.relist();
        }
    }

    /// The build asked to be shown in the main area, taken once.
    pub(in crate::window) fn take_shown(&mut self) -> Option<TabId> {
        self.shown.take()
    }

    /// How many errors and warnings every source found.
    pub(in crate::window) fn counts(&self) -> (usize, usize) {
        let count = |level| {
            self.sources
                .iter()
                .flat_map(|found| &found.problems)
                .filter(|problem| problem.level == level)
                .count()
        };
        (
            count(DiagnosticLevel::Error),
            count(DiagnosticLevel::Warning),
        )
    }

    /// Each source's name and how many problems it found, for a test.
    #[cfg(test)]
    pub(in crate::window) fn source_names(&self) -> Vec<(String, usize)> {
        let named = self.sources.iter();
        named
            .map(|found| (found.name.clone(), found.problems.len()))
            .collect()
    }

    /// The sources the scope row offers: those with problems, and the one
    /// in use.
    fn scopes(&self) -> impl Iterator<Item = &Found> {
        self.sources
            .iter()
            .filter(|found| !found.problems.is_empty() || Some(found.source) == self.scope)
    }

    /// Use the scope `step` along the scope row from the one in use, `All`
    /// first.
    fn step_scope(&mut self, step: isize) {
        let mut scopes: Vec<Option<Source>> = vec![None];
        scopes.extend(self.scopes().map(|found| Some(found.source)));
        let at = scopes
            .iter()
            .position(|scope| *scope == self.scope)
            .unwrap_or(0);
        if let Some(next) = at.checked_add_signed(step).and_then(|at| scopes.get(at)) {
            self.scope = *next;
            self.relist();
        }
    }

    /// List the problems of the scope in use, by file then line, each
    /// file's name over its own.
    fn relist(&mut self) {
        let mut listed: Vec<Problem> = self
            .sources
            .iter()
            .filter(|found| self.scope.is_none_or(|scope| scope == found.source))
            .flat_map(|found| found.problems.iter().cloned())
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
                    icon: Some(RowIcon::of_path(&file_name(&problem.path), None)),
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
            ListAct::OpenTool => {
                let newest = self.sources.iter().rev().map(|found| found.source);
                let build = self
                    .scope
                    .into_iter()
                    .chain(newest)
                    .find_map(|source| match source {
                        Source::Build(tab) => Some(tab),
                        Source::Checker(_) => None,
                    });
                self.shown = build;
            }
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

    fn own_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        match table::find(&KEYS, key)? {
            ProblemsAct::Next => self.step_scope(1),
            ProblemsAct::Previous => self.step_scope(-1),
        }
        Some(EditorOutcome::Consumed)
    }
}

impl HostedView for ProblemsView {
    fn view(&self) -> Element<'_, Self> {
        if self.sources.iter().all(|found| found.problems.is_empty()) {
            return list_view(self, NONE_FOUND);
        }
        let all = self.sources.iter().map(|found| found.problems.len()).sum();
        let mut scopes = vec![scope_view(ALL.to_string(), all, None, self.scope.is_none())];
        scopes.extend(self.scopes().map(|found| {
            let used = self.scope == Some(found.source);
            scope_view(
                found.name.clone(),
                found.problems.len(),
                Some(found.source),
                used,
            )
        }));
        let scopes = row(scopes)
            .gap(Space::Px(SCOPE_GAP_PX))
            .padding_px(SCOPE_PADDING_PX.0, SCOPE_PADDING_PX.1 * 2.0);
        column([scopes, list_view(self, NONE_FOUND)]).gap(Space::Px(0.0))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The scope `name` on the scope row, `count` problems in it, picking
/// `scope` when clicked; marked when `used`.
fn scope_view<'a>(
    name: String,
    count: usize,
    scope: Option<Source>,
    used: bool,
) -> Element<'a, ProblemsView> {
    let words = format!("{name} {count}");
    let chip = row([text(words).into()])
        .width(Extent::Auto)
        .corners(Corners::Control)
        .padding_px(SCOPE_PADDING_PX.0, SCOPE_PADDING_PX.1)
        .on_pointer_press(move |view: &mut ProblemsView, _: Press| {
            view.scope = scope;
            view.relist();
        });
    match used {
        true => chip
            .background(ThemeToken::SelectedRow)
            .border(ThemeToken::Accent),
        false => chip.hover_background(ThemeToken::Hover),
    }
}

/// The row of `problem`, the `key`th listed: what the tool said, its level
/// as a colored mark, and its line and column.
fn problem_row(key: usize, problem: &Problem) -> ListRow {
    let place = match problem.column {
        Some(column) => format!("{}:{column}", problem.line),
        None => problem.line.to_string(),
    };
    let (symbol, color, meaning) = match problem.level {
        DiagnosticLevel::Error => ("error", ThemeToken::Danger, "Error"),
        DiagnosticLevel::Warning => ("warning", ThemeToken::Warning, "Warning"),
        DiagnosticLevel::Info => ("info", ThemeToken::Muted, "Note"),
    };
    ListRow {
        depth: 1,
        detail: Some(place.clone()),
        key,
        icon: Some(RowIcon::new(
            IconKind::Symbol { name: symbol },
            meaning,
            color,
        )),
        label: problem.message.clone(),
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
        view.set_source(
            Source::Build(TabId(1)),
            "build 1".to_string(),
            vec![problem("/b.rs", 4, DiagnosticLevel::Error)],
        );
        view.set_source(
            Source::Build(TabId(2)),
            "build 2".to_string(),
            vec![
                problem("/a.rs", 9, DiagnosticLevel::Warning),
                problem("/a.rs", 2, DiagnosticLevel::Error),
            ],
        );
        let labels: Vec<String> = view.list.rows.iter().map(|row| row.label.clone()).collect();
        assert_eq!(labels, ["a.rs", "at 2", "at 9", "b.rs", "at 4"]);
        assert_eq!(view.counts(), (2, 1));
        view.set_source(Source::Build(TabId(1)), "build 1".to_string(), Vec::new());
        assert_eq!(view.counts(), (1, 1), "build 1's error fixed");
        view.forget(TabId(2));
        assert_eq!(view.title(), TITLE);
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut view = ProblemsView::default();
        view.set_source(
            Source::Build(TabId(1)),
            "build 1".to_string(),
            vec![
                problem("/a.rs", 2, DiagnosticLevel::Error),
                problem("/a.rs", 9, DiagnosticLevel::Warning),
            ],
        );
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }

    #[test]
    fn test_a_problem_shows_its_level_as_an_icon_under_its_file_s() {
        let mut view = ProblemsView::default();
        view.set_source(
            Source::Build(TabId(1)),
            "build 1".to_string(),
            vec![
                problem("/a.rs", 2, DiagnosticLevel::Error),
                problem("/a.rs", 9, DiagnosticLevel::Warning),
            ],
        );
        let frame = super::super::tests::drawn_view(view);
        let file = super::super::tests::lefts_of(&frame, "File");
        assert_eq!(file.len(), 1, "a.rs heads its problems");
        assert_eq!(super::super::tests::lefts_of(&frame, "Error").len(), 1);
        assert_eq!(super::super::tests::lefts_of(&frame, "Warning").len(), 1);
    }

    #[test]
    fn test_the_scope_row_narrows_to_one_source_and_drops_one_with_nothing() {
        let mut view = ProblemsView::default();
        let build = |n| Source::Build(TabId(n));
        view.set_source(
            build(1),
            "check".to_string(),
            vec![problem("/a.rs", 2, DiagnosticLevel::Error)],
        );
        view.set_source(
            build(2),
            "test".to_string(),
            vec![problem("/b.rs", 4, DiagnosticLevel::Warning)],
        );
        let names = |view: &ProblemsView| -> Vec<String> {
            view.scopes().map(|found| found.name.clone()).collect()
        };
        assert_eq!(names(&view), ["check", "test"]);
        view.step_scope(1);
        assert_eq!(view.scope, Some(build(1)));
        assert_eq!(view.listed.len(), 1, "only check's");
        view.step_scope(1);
        view.set_source(build(2), "test".to_string(), Vec::new());
        assert_eq!(names(&view), ["check", "test"], "kept while in use");
        view.step_scope(-1);
        assert_eq!(names(&view), ["check"], "nothing found, dropped");
        view.step_scope(-1);
        assert_eq!(view.scope, None, "All");
        assert_eq!(view.listed.len(), 1);
    }
}

//! The window's panels as the app runs them: showing and hiding each, the
//! views they show, and what the window feeds those views (the outline of
//! the tab worked in, the builds, the searches kept). The layout of the panels themselves
//! is [`winter_model::layout::panels`]; the views are [`views`].
//!
//! A view is a page the window holds in no pane, named by the id its panel
//! keeps. Every tab lives in the main area: nothing moves into a panel.
//! Every view is there once a panel is first used, each in the panel it
//! starts in: the switchers list them all, none added or closed by hand.

pub(in crate::window) mod views;

use std::path::Path;
use std::time::Instant;

use winter_base::project;
use winter_lsp::diagnostics::{self, Backend};
use winter_lsp::protocol::{Diagnostic, Severity};
use winter_model::editor::Page;
use winter_model::layout::{
    BottomPanelKind, DockSnapshot, Edge, Panel, PanelState, Panels, Region, SidebarKind, TabId,
};
use winter_model::paint::DiagnosticLevel;

use crate::config::PanelSizes;
use crate::editors::compile::{CompileEditor, Problem};
use crate::editors::list::ListEditor;
use crate::session::{DockSession, DocksSession, MAX_SEARCH_LINES, SearchSession, ViewsSession};
use crate::window::App;
use crate::window::agents::Agent;
use crate::window::editor::EditorKind;
use crate::window::notice::WindowNotice;
use views::files::FilesView;
use views::git::GitView;
use views::outline::OutlineView;
use views::output::{OutputView, Run};
use views::problems::{ProblemsView, Source};
use views::search::{ResultSet, SearchView};
use views::sessions::{Managed, SessionsView};
use views::todo::TodoView;
use views::{PanelPage, PanelView};

// ========================================================================
// Constants
// ========================================================================

/// What Problems calls a language server it no longer knows the name of.
const LANGUAGE_SERVER: &str = "Language server";

/// Every sidebar view, in the order each sidebar lists those it starts with.
const SIDEBAR_KINDS: [SidebarKind; 5] = [
    SidebarKind::Files,
    SidebarKind::Git,
    SidebarKind::Outline,
    SidebarKind::Todo,
    SidebarKind::Sessions,
];

/// Every bottom panel view, in the order the bottom panel lists them.
const BOTTOM_KINDS: [BottomPanelKind; 3] = [
    BottomPanelKind::Output,
    BottomPanelKind::Problems,
    BottomPanelKind::Search,
];

// ========================================================================
// Data Structures
// ========================================================================

/// Which view a panel shows: a sidebar's or the bottom panel's.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub(in crate::window) enum ViewKind {
    Bottom(BottomPanelKind),
    Sidebar(SidebarKind),
}

// ========================================================================
// ViewKind Implementations
// ========================================================================

impl ViewKind {
    /// What the session and the icon tables call it.
    pub(in crate::window) fn name(self) -> &'static str {
        match self {
            ViewKind::Bottom(BottomPanelKind::Output) => "output",
            ViewKind::Bottom(BottomPanelKind::Problems) => "problems",
            ViewKind::Bottom(BottomPanelKind::Search) => "search",
            ViewKind::Sidebar(SidebarKind::Files) => "files",
            ViewKind::Sidebar(SidebarKind::Git) => "git",
            ViewKind::Sidebar(SidebarKind::Outline) => "outline",
            ViewKind::Sidebar(SidebarKind::Todo) => "todo",
            ViewKind::Sidebar(SidebarKind::Sessions) => "sessions",
        }
    }

    /// The command that shows it, whose keys its tooltip names.
    pub(in crate::window) fn command(self) -> &'static str {
        match self {
            ViewKind::Bottom(BottomPanelKind::Output) => "bottom_panel_output",
            ViewKind::Bottom(BottomPanelKind::Problems) => "bottom_panel_problems",
            ViewKind::Bottom(BottomPanelKind::Search) => "bottom_panel_search",
            ViewKind::Sidebar(SidebarKind::Files) => "sidebar_files",
            ViewKind::Sidebar(SidebarKind::Git) => "sidebar_git",
            ViewKind::Sidebar(SidebarKind::Outline) => "sidebar_outline",
            ViewKind::Sidebar(SidebarKind::Todo) => "sidebar_todo",
            ViewKind::Sidebar(SidebarKind::Sessions) => "sidebar_sessions",
        }
    }

    /// How the switcher and the title bar name it.
    pub(in crate::window) fn title(self) -> &'static str {
        match self {
            ViewKind::Bottom(BottomPanelKind::Output) => "Output",
            ViewKind::Bottom(BottomPanelKind::Problems) => "Problems",
            ViewKind::Bottom(BottomPanelKind::Search) => "Search",
            ViewKind::Sidebar(SidebarKind::Files) => "Files",
            ViewKind::Sidebar(SidebarKind::Git) => "Git",
            ViewKind::Sidebar(SidebarKind::Outline) => "Outline",
            ViewKind::Sidebar(SidebarKind::Todo) => "Todo",
            ViewKind::Sidebar(SidebarKind::Sessions) => "Sessions",
        }
    }
}

// ========================================================================
// App: panels
// ========================================================================

impl App {
    // --------------------------------------------------------------------
    // Showing
    // --------------------------------------------------------------------

    /// Show `panel` and give it the keyboard, or hide it when shown,
    /// whether it has the keyboard or not. A sidebar left with no view, the
    /// other holding them all, says so.
    pub(in crate::window) fn toggle_panel(&mut self, panel: Panel) {
        self.ready_views();
        match self.layout().panel_state(panel) {
            PanelState::Empty => self.notify(WindowNotice::NoViewLeft),
            PanelState::Shown => self.layout_mut().hide_panel(panel),
            PanelState::Hidden => {
                self.layout_mut().show_panel(panel);
            }
        }
        self.after_panel_change();
    }

    /// Show the sidebar view `kind`, wherever it is, and give it the
    /// keyboard.
    pub(in crate::window) fn open_sidebar_view(&mut self, kind: SidebarKind) {
        self.ready_views();
        let tab = self.held_sidebar_view(kind);
        self.layout_mut().focus(tab);
        self.after_panel_change();
    }

    /// Show the bottom panel view `kind`, giving it the keyboard when
    /// `focus`, else leaving the keyboard where it is. Its id.
    pub(in crate::window) fn open_bottom_view(
        &mut self,
        kind: BottomPanelKind,
        focus: bool,
    ) -> TabId {
        self.ready_views();
        let tab = self.held_bottom_view(kind);
        match focus {
            true => {
                self.layout_mut().focus(tab);
            }
            false => {
                self.layout_mut().bring_to_front(tab);
                self.layout_mut().set_panel_shown(Panel::Bottom, true);
            }
        }
        self.after_panel_change();
        tab
    }

    /// Move the focused sidebar view to the other sidebar.
    pub(in crate::window) fn move_view_to_other_sidebar(&mut self) {
        let focused = self.layout().focused();
        let Some(Panel::Sidebar(edge)) = self.layout().panel_of(focused) else {
            self.notify(WindowNotice::NotInASidebar);
            return;
        };
        let Some(kind) = self.layout().sidebar(edge).kind_of(focused) else {
            return;
        };
        self.layout_mut().move_sidebar_view(kind, other_edge(edge));
        self.after_panel_change();
    }

    /// Give the keyboard to the main area, where a panel has it: to the main
    /// tab that last had it.
    pub(in crate::window) fn focus_main_area(&mut self) {
        let focused = self.layout().focused();
        if self.layout().region_of(focused) != Some(Region::Main) {
            let back = self.layout().main_return();
            self.layout_mut().focus(back);
        }
    }

    // --------------------------------------------------------------------
    // What the window feeds the views
    // --------------------------------------------------------------------

    /// Feed the views shown what they show of the window, and carry out what
    /// a view asked of it: called once a frame, after [`Self::sync_shown`]. A
    /// view not shown is fed nothing.
    pub(in crate::window) fn sync_panel_views(&mut self) {
        self.sync_outline();
        self.sync_builds();
        self.take_view_requests();
    }

    /// Keep the build of tab `tab`, just started, in the Output view, and
    /// show it there without taking the keyboard.
    pub(in crate::window) fn show_build_output(&mut self, tab: TabId) {
        self.open_bottom_view(BottomPanelKind::Output, false);
        self.sync_builds();
        self.with_view(BottomPanelKind::Output, |view: &mut OutputView| {
            view.select_run(tab)
        });
    }

    /// Keep `set`, the lines a search found or a picker's rows, in the
    /// Search view, and show it there with the keyboard.
    pub(in crate::window) fn keep_search(&mut self, set: ResultSet) {
        self.open_bottom_view(BottomPanelKind::Search, true);
        self.with_view(BottomPanelKind::Search, |view: &mut SearchView| {
            view.push(set)
        });
    }

    /// Forget the build of tab `tab`, which has closed, in Problems.
    pub(in crate::window) fn forget_build_problems(&mut self, tab: TabId) {
        self.with_view(BottomPanelKind::Problems, |view: &mut ProblemsView| {
            view.forget(tab)
        });
    }

    /// Feed the Outline view, while shown, the outline of the main tab last
    /// worked in: read again when that tab changes, or when its outline
    /// does and the changes have paused (see [`OutlineView::is_due`]); the
    /// entry its cursor is in followed each frame while the view does not
    /// have the keyboard.
    fn sync_outline(&mut self) {
        let Some(outline) = self
            .sidebar_view(SidebarKind::Outline)
            .filter(|tab| self.is_shown(*tab))
        else {
            return;
        };
        let source = self.layout().main_return();
        let chosen = self
            .view_mut(outline, |view: &mut OutlineView| view.take_chosen())
            .flatten();
        if let Some(target) = chosen {
            self.layout_mut().focus(source);
            self.confirm_jump(target, source);
            self.dirty = true;
        }
        let revision = self
            .open_tabs
            .page(&source)
            .and_then(|slot| slot.page.outline_revision());
        let now = Instant::now();
        let due = self
            .view_mut(outline, |view: &mut OutlineView| {
                view.is_due(source, revision, now)
            })
            .unwrap_or(false);
        if due {
            let (name, entries) = match self.open_tabs.page_mut(&source) {
                Some(slot) => (
                    Some(slot.page.title()),
                    slot.page.outline_targets().unwrap_or_default(),
                ),
                None => (None, Vec::new()),
            };
            self.view_mut(outline, |view: &mut OutlineView| {
                view.set_outline(name, entries, None);
                view.mark_read(source, revision);
            });
        }
        let in_use = self.layout().focused() == outline;
        let cursor = self
            .open_tabs
            .page(&source)
            .and_then(|slot| slot.page.jump_cursor())
            .filter(|_| !in_use);
        if let Some(cursor) = cursor {
            self.view_mut(outline, |view: &mut OutlineView| view.follow_cursor(cursor));
        }
    }

    /// Hand Output the builds and the end of the one its cursor is on, and
    /// Problems what each build has found.
    fn sync_builds(&mut self) {
        let in_sight = |kind| {
            let tab = self.layout().bottom_panel().tab_of(kind)?;
            self.is_shown(tab).then_some(tab)
        };
        let output = in_sight(BottomPanelKind::Output);
        let problems = in_sight(BottomPanelKind::Problems);
        if output.is_none() && problems.is_none() {
            return;
        }
        let builds = self.builds_newest_first();
        let runs: Vec<Run> = builds
            .iter()
            .filter_map(|tab| {
                self.compile_editor(*tab, |editor| Run {
                    name: editor.task_name().to_string(),
                    state: editor.run_state(),
                    tab: *tab,
                })
            })
            .collect();
        if let Some(output) = output {
            self.view_mut(output, |view: &mut OutputView| view.set_runs(runs));
            let reading = self.view_mut(output, |view: &mut OutputView| view.reading());
            if let Some((run, held)) = reading.flatten() {
                // Only what is new, or all of it again once it started over.
                let added = self.compile_editor(run, |editor| {
                    let total = editor.output_len();
                    let from = if total < held { 0 } else { held };
                    let lines: Vec<String> = (from..total)
                        .map(|index| editor.output_line(index))
                        .collect();
                    (from, lines)
                });
                if let Some((from, lines)) =
                    added.filter(|(from, lines)| *from == 0 || !lines.is_empty())
                {
                    self.view_mut(output, |view: &mut OutputView| view.add_lines(from, lines));
                }
            }
            let wanted = self
                .view_mut(output, |view: &mut OutputView| {
                    (view.selected_run(), view.fits())
                })
                .and_then(|(run, fits)| Some((run?, fits)));
            if let Some((run, fits)) = wanted {
                let lines = self
                    .compile_editor(run, |editor| {
                        let total = editor.output_len();
                        (total.saturating_sub(fits)..total)
                            .map(|index| editor.output_line(index))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.view_mut(output, |view: &mut OutputView| view.set_lines(lines));
            }
        }
        if let Some(problems) = problems {
            let generation = diagnostics::generation();
            let checked = self.view_mut(problems, |view: &mut ProblemsView| {
                view.is_checked(generation)
            });
            if checked == Some(false) {
                let found = self.checkers_found();
                self.view_mut(problems, |view: &mut ProblemsView| {
                    view.set_checkers(generation, found)
                });
            }
            for tab in builds {
                let found = self.compile_editor(tab, |editor| {
                    (editor.task_name().to_string(), editor.problems())
                });
                if let Some((name, found)) = found {
                    self.view_mut(problems, |view: &mut ProblemsView| {
                        view.set_source(Source::Build(tab), name, found)
                    });
                }
            }
        }
    }

    /// Carry out what the views asked that only the window can: a build or
    /// a search set shown in the main area.
    fn take_view_requests(&mut self) {
        let shown = [
            self.with_view(BottomPanelKind::Output, OutputView::take_shown),
            self.with_view(BottomPanelKind::Problems, ProblemsView::take_shown),
        ];
        for build in shown.into_iter().flatten().flatten() {
            self.show_build_in_main_area(build);
        }
        let removed = self
            .with_view(BottomPanelKind::Output, OutputView::take_removed)
            .flatten();
        if let Some(build) = removed {
            self.remove_run(build);
        }
        let opened = self
            .with_view(BottomPanelKind::Search, SearchView::take_opened)
            .flatten();
        if let Some(set) = opened {
            let editor = ListEditor::new(&set.name, &set.query, set.lines);
            self.focus_main_area();
            self.show_editor(EditorKind::List, Box::new(editor));
        }
        self.take_sessions_view_requests();
    }

    /// What the Sessions view asked: an agent or terminal shown or acted on,
    /// or the view refreshed as it comes into sight.
    fn take_sessions_view_requests(&mut self) {
        let Some(tab) = self.sidebar_view(SidebarKind::Sessions) else {
            return;
        };
        let asked = self.view_mut(tab, SessionsView::take_request).flatten();
        if let Some(request) = asked {
            let focused = self.layout().focused();
            self.focus_main_area();
            self.carry_out_agent(request, focused);
        }
        if self.view_mut(tab, SessionsView::take_stale) == Some(true) {
            self.sync_sessions_view();
            self.look_for_agents_view();
        }
    }

    /// Fill the Sessions view with `agents`, the managed agents found, and
    /// open terminals.
    pub(in crate::window) fn fill_agents_view(&mut self, agents: Vec<Agent>) {
        let Some(tab) = self.sidebar_view(SidebarKind::Sessions) else {
            return;
        };
        let managed = self.managed_in_view(agents);
        self.take_in_running_agents();
        let detected = self.detected_agents();
        let entries = self.terminal_entries();
        self.view_mut(tab, |view: &mut SessionsView| {
            view.set_agents(managed);
            view.set_detected(detected);
            view.set_terminals(entries);
        });
        self.dirty = true;
    }

    /// Bring the views of what runs in the terminals up to date, when in sight:
    /// an agent started, stopped, or changed how it stands, a terminal
    /// retitled, ran something, opened, or closed.
    pub(in crate::window) fn sync_live_views(&mut self) {
        self.sync_sessions_view();
    }

    /// Bring the Sessions view up to date, when it is in sight.
    fn sync_sessions_view(&mut self) {
        let Some(tab) = self.sidebar_view(SidebarKind::Sessions) else {
            return;
        };
        let shown = self.view_mut(tab, |view: &mut SessionsView| view.is_shown());
        if shown != Some(true) {
            return;
        }
        self.take_in_running_agents();
        let agents = self
            .view_mut(tab, |view: &mut SessionsView| view.managed_agents())
            .flatten();
        let managed = agents.map(|agents| self.managed_in_view(agents));
        let detected = self.detected_agents();
        let entries = self.terminal_entries();
        self.view_mut(tab, |view: &mut SessionsView| {
            if let Some(managed) = managed {
                view.set_agents(managed);
            }
            view.set_detected(detected);
            view.set_terminals(entries);
        });
        self.dirty = true;
    }

    /// `agents` as the Sessions view shows them: each standing as its tab
    /// last said, or as its session is, and doing what its tab's title says.
    fn managed_in_view(&self, agents: Vec<Agent>) -> Vec<Managed> {
        agents
            .into_iter()
            .map(|agent| Managed {
                standing: self.agent_standing(&agent),
                task: self.agent_tab(&agent).and_then(|tab| self.agent_task(tab)),
                agent,
            })
            .collect()
    }

    /// Have the Sessions view look again, when it is in sight: an agent was
    /// started, or ended.
    pub(in crate::window) fn refresh_agents_view(&mut self) {
        if let Some(tab) = self.sidebar_view(SidebarKind::Sessions) {
            self.view_mut(tab, SessionsView::mark_stale);
        }
    }

    /// Show the build of tab `tab` in the main area, where its tab is or,
    /// buried, beside the main tab that last had the keyboard.
    pub(in crate::window) fn show_build_in_main_area(&mut self, tab: TabId) {
        self.focus_main_area();
        if self.open_tabs.unbury(tab) {
            self.layout_mut().add_tab(tab);
        } else {
            self.layout_mut().focus(tab);
        }
        self.after_tab_change();
    }

    // --------------------------------------------------------------------
    // Sessions and workspaces
    // --------------------------------------------------------------------

    /// The panels as the session keeps them.
    pub(in crate::window) fn docks_session(&self) -> DocksSession {
        let sidebar = |edge| {
            dock_session(&self.layout().sidebar(edge).snapshot(), |kind| {
                ViewKind::Sidebar(kind).name()
            })
        };
        DocksSession {
            bottom: dock_session(&self.layout().bottom_panel().snapshot(), |kind| {
                ViewKind::Bottom(kind).name()
            }),
            left: sidebar(Edge::Left),
            right: sidebar(Edge::Right),
        }
    }

    /// Put back the panels `docks`, as a session kept them, in place of
    /// empty ones: each view in the sidebar it was in, and those the session
    /// left out in the panel they start in.
    pub(in crate::window) fn restore_docks(&mut self, docks: &DocksSession) {
        for (edge, dock) in [(Edge::Left, &docks.left), (Edge::Right, &docks.right)] {
            for name in &dock.views {
                if let Some(kind) = sidebar_kind(name)
                    && self.layout().sidebar_of(kind).is_none()
                {
                    self.new_sidebar_view(edge, kind);
                }
            }
        }
        for name in &docks.bottom.views {
            if let Some(kind) = bottom_kind(name) {
                self.held_bottom_view(kind);
            }
        }
        self.ready_views();
        for (edge, dock) in [(Edge::Left, &docks.left), (Edge::Right, &docks.right)] {
            let active = dock.active.as_deref().and_then(sidebar_kind);
            let active = active.and_then(|kind| self.layout().sidebar(edge).tab_of(kind));
            self.restore_frame(Panel::Sidebar(edge), dock, active);
        }
        let active = docks.bottom.active.as_deref().and_then(bottom_kind);
        let active = active.and_then(|kind| self.layout().bottom_panel().tab_of(kind));
        self.restore_frame(Panel::Bottom, &docks.bottom, active);
    }

    /// Put `panels`, the window's, back in its layout in place of what is
    /// there, letting go of the views of any a session put there meanwhile.
    pub(in crate::window) fn put_window_panels(&mut self, panels: Panels) {
        let replaced = self.layout_mut().take_panels();
        for view in replaced.views() {
            self.open_tabs.remove(&view);
        }
        self.layout_mut().put_panels(panels);
    }

    /// The views' light state, as the session keeps it with the workspace.
    pub(in crate::window) fn views_session(&self) -> ViewsSession {
        let folds = self
            .sidebar_view(SidebarKind::Files)
            .and_then(|tab| self.view_ref(tab, FilesView::open_folders))
            .unwrap_or_default();
        let search = self
            .layout()
            .bottom_panel()
            .tab_of(BottomPanelKind::Search)
            .and_then(|tab| self.view_ref(tab, |view: &SearchView| view.latest().cloned()))
            .flatten()
            .map(|set| SearchSession {
                lines: set.lines.into_iter().take(MAX_SEARCH_LINES).collect(),
                name: set.name,
                query: set.query,
            });
        ViewsSession { folds, search }
    }

    /// Put the views' light state `views` back, as the session kept it.
    pub(in crate::window) fn restore_views(&mut self, views: ViewsSession) {
        if let Some(tab) = self.sidebar_view(SidebarKind::Files) {
            self.view_mut(tab, |view: &mut FilesView| {
                view.set_open_folders(&views.folds)
            });
        }
        self.with_view(
            BottomPanelKind::Search,
            |view: &mut SearchView| match views.search {
                Some(search) => view.push(ResultSet {
                    lines: search.lines,
                    name: search.name,
                    query: search.query,
                }),
                None => view.clear(),
            },
        );
    }

    /// Point the views that show a project at the one the window works in
    /// now: after a workspace switch.
    pub(in crate::window) fn retarget_views(&mut self) {
        let root = project::root(&self.focused_start_dir());
        let files = self.sidebar_view(SidebarKind::Files);
        let git = self.sidebar_view(SidebarKind::Git);
        let todo = self.sidebar_view(SidebarKind::Todo);
        let files = files.and_then(|tab| {
            let outcome = self.view_mut(tab, |view: &mut FilesView| view.set_root(root.clone()))?;
            Some((tab, outcome))
        });
        let git = git.and_then(|tab| {
            let outcome = self.view_mut(tab, |view: &mut GitView| view.set_start(root.clone()))?;
            Some((tab, outcome))
        });
        let todo = todo.and_then(|tab| {
            let outcome = self.view_mut(tab, |view: &mut TodoView| view.set_root(root.clone()))?;
            Some((tab, outcome))
        });
        for (tab, outcome) in [files, git, todo].into_iter().flatten() {
            self.act_on_editor_outcome(tab, outcome);
        }
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// The id of the sidebar view `kind`, wherever it is.
    fn sidebar_view(&self, kind: SidebarKind) -> Option<TabId> {
        let edge = self.layout().sidebar_of(kind)?;
        self.layout().sidebar(edge).tab_of(kind)
    }

    /// Hold every view no panel holds yet in the panel it starts in, out of
    /// sight, so each switcher lists all its views. Making a view is cheap:
    /// it reads nothing until first shown.
    fn ready_views(&mut self) {
        for kind in SIDEBAR_KINDS {
            self.held_sidebar_view(kind);
        }
        for kind in BOTTOM_KINDS {
            self.held_bottom_view(kind);
        }
    }

    /// The id of the sidebar view `kind`, made and held in the sidebar it
    /// starts in where neither holds it.
    fn held_sidebar_view(&mut self, kind: SidebarKind) -> TabId {
        match self.sidebar_view(kind) {
            Some(tab) => tab,
            None => self.new_sidebar_view(start_edge(kind), kind),
        }
    }

    /// The id of the bottom panel view `kind`, made and held there where
    /// the panel does not hold it.
    fn held_bottom_view(&mut self, kind: BottomPanelKind) -> TabId {
        if let Some(tab) = self.layout().bottom_panel().tab_of(kind) {
            return tab;
        }
        let tab = self.alloc_tab_id();
        let page = self.build_view(ViewKind::Bottom(kind));
        self.open_tabs
            .insert_page(tab, EditorKind::View(ViewKind::Bottom(kind)), page);
        self.layout_mut().hold_bottom_view(kind, tab);
        tab
    }

    /// Make the sidebar view `kind` and hold it in the sidebar on `edge`,
    /// without moving the keyboard. Its id.
    fn new_sidebar_view(&mut self, edge: Edge, kind: SidebarKind) -> TabId {
        let tab = self.alloc_tab_id();
        let page = self.build_view(ViewKind::Sidebar(kind));
        self.open_tabs
            .insert_page(tab, EditorKind::View(ViewKind::Sidebar(kind)), page);
        self.layout_mut().hold_sidebar_view(edge, kind, tab);
        tab
    }

    /// A new view of `kind`, on the project the focused tab works in for
    /// those that show one.
    fn build_view(&self, kind: ViewKind) -> Box<dyn Page> {
        let root = project::root(&self.focused_start_dir());
        match kind {
            ViewKind::Bottom(BottomPanelKind::Output) => {
                Box::new(PanelPage::new(OutputView::default()))
            }
            ViewKind::Bottom(BottomPanelKind::Problems) => {
                Box::new(PanelPage::new(ProblemsView::default()))
            }
            ViewKind::Bottom(BottomPanelKind::Search) => {
                Box::new(PanelPage::new(SearchView::default()))
            }
            ViewKind::Sidebar(SidebarKind::Files) => Box::new(PanelPage::new(FilesView::new(root))),
            ViewKind::Sidebar(SidebarKind::Git) => Box::new(PanelPage::new(GitView::new(root))),
            ViewKind::Sidebar(SidebarKind::Outline) => {
                Box::new(PanelPage::new(OutlineView::default()))
            }
            ViewKind::Sidebar(SidebarKind::Todo) => Box::new(PanelPage::new(TodoView::new(
                root,
                &self.config.todo_keywords,
            ))),
            ViewKind::Sidebar(SidebarKind::Sessions) => {
                Box::new(PanelPage::new(SessionsView::default()))
            }
        }
    }

    /// Run `run` on the view `tab`, when it is one of type `S`.
    fn view_mut<S: PanelView, R>(
        &mut self,
        tab: TabId,
        run: impl FnOnce(&mut S) -> R,
    ) -> Option<R> {
        let slot = self.open_tabs.page_mut(&tab)?;
        slot.page
            .with_kind_mut(|page: &mut PanelPage<S>| run(page.state_mut()))
    }

    /// What each checker found in the focused project, the writing
    /// checkers left out, named for the user: a language server by its
    /// program, an extension's by the extension.
    fn checkers_found(&self) -> Vec<(Backend, String, Vec<Problem>)> {
        let root = project::root(&self.focused_start_dir());
        let mut found: Vec<(Backend, String, Vec<Problem>)> = Vec::new();
        for each in diagnostics::under(&root) {
            if each.backend.is_writing() {
                continue;
            }
            let problem = problem_of(&each.path, &each.diagnostic);
            match found
                .iter_mut()
                .find(|(backend, _, _)| *backend == each.backend)
            {
                Some((_, _, problems)) => problems.push(problem),
                None => {
                    let name = match each.backend {
                        Backend::Extension(id) => id.to_string(),
                        Backend::Lsp(server) => self
                            .server_name(server)
                            .unwrap_or_else(|| LANGUAGE_SERVER.to_string()),
                        Backend::Spelling => continue,
                    };
                    found.push((each.backend, name, vec![problem]));
                }
            }
        }
        found
    }

    /// What `run` makes of the view of tab `tab`, where it is an `S`.
    fn view_ref<S: PanelView, R>(&self, tab: TabId, run: impl FnOnce(&S) -> R) -> Option<R> {
        let slot = self.open_tabs.page(&tab)?;
        slot.page.with_kind(|page: &PanelPage<S>| run(page.state()))
    }

    /// Run `run` on the bottom panel view `kind`, while the panel holds it.
    fn with_view<S: PanelView, R>(
        &mut self,
        kind: BottomPanelKind,
        run: impl FnOnce(&mut S) -> R,
    ) -> Option<R> {
        let tab = self.layout().bottom_panel().tab_of(kind)?;
        self.view_mut(tab, run)
    }

    /// Run `run` on the Compile editor of tab `tab`.
    fn compile_editor<R>(
        &mut self,
        tab: TabId,
        run: impl FnOnce(&CompileEditor) -> R,
    ) -> Option<R> {
        let slot = self.open_tabs.page(&tab)?;
        slot.page.with_kind(|editor: &CompileEditor| run(editor))
    }

    /// Apply any user-resized panel dimensions saved in local state, returning
    /// whether any panel size was set.
    pub(in crate::window) fn apply_panel_sizes(&mut self, sizes: &PanelSizes) -> bool {
        let mut applied = false;
        for (panel, size) in [
            (Panel::Sidebar(Edge::Left), sizes.left),
            (Panel::Sidebar(Edge::Right), sizes.right),
            (Panel::Bottom, sizes.bottom),
        ] {
            if let Some(size) = size {
                self.layout_mut().set_panel_size(panel, size);
                applied = true;
            }
        }
        applied
    }

    /// Reset panel sizes to their defaults, equalize main-area splits, and
    /// clear any saved panel-size overrides in local state.
    pub(in crate::window) fn reset_layout(&mut self) {
        self.layout_mut().reset();
        self.update_app_state(|state| {
            state.panel_sizes = PanelSizes::default();
        });
        self.after_panel_change();
    }

    /// Put back `panel`'s shown view and whether it is shown, as `dock` kept
    /// them. Panel sizes live in local state rather than the session.
    fn restore_frame(&mut self, panel: Panel, dock: &DockSession, active: Option<TabId>) {
        if let Some(active) = active {
            self.layout_mut().bring_to_front(active);
        }
        self.layout_mut().set_panel_shown(panel, dock.shown);
    }

    /// What follows a panel shown, hidden, or filled: the focus reports, the
    /// panes' sizes, and a frame.
    fn after_panel_change(&mut self) {
        self.sync_focus_reports();
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The sidebar a view starts in: Outline on the right, the rest on the
/// left.
fn start_edge(kind: SidebarKind) -> Edge {
    match kind {
        SidebarKind::Outline => Edge::Right,
        SidebarKind::Files
        | SidebarKind::Git
        | SidebarKind::Sessions
        | SidebarKind::Todo => Edge::Left,
    }
}

/// The sidebar across from `edge`.
fn other_edge(edge: Edge) -> Edge {
    match edge {
        Edge::Left => Edge::Right,
        Edge::Right => Edge::Left,
    }
}

/// The sidebar view the session's `name` stands for.
fn sidebar_kind(name: &str) -> Option<SidebarKind> {
    SIDEBAR_KINDS
        .into_iter()
        .find(|kind| ViewKind::Sidebar(*kind).name() == name)
}

/// The bottom panel view the session's `name` stands for.
fn bottom_kind(name: &str) -> Option<BottomPanelKind> {
    BOTTOM_KINDS
        .into_iter()
        .find(|kind| ViewKind::Bottom(*kind).name() == name)
}

/// `snapshot` as the session keeps it, each kind by `name`.
fn dock_session<K: Copy>(
    snapshot: &DockSnapshot<K>,
    name: impl Fn(K) -> &'static str,
) -> DockSession {
    DockSession {
        active: snapshot.active.map(|kind| name(kind).to_string()),
        shown: snapshot.shown,
        size: snapshot.size,
        views: snapshot
            .kinds
            .iter()
            .map(|kind| name(*kind).to_string())
            .collect(),
    }
}

/// `diagnostic`, which a checker found in `path`, as Problems lists it.
fn problem_of(path: &Path, diagnostic: &Diagnostic) -> Problem {
    let level = match diagnostic.severity {
        Severity::Error => DiagnosticLevel::Error,
        Severity::Warning => DiagnosticLevel::Warning,
        Severity::Hint | Severity::Information => DiagnosticLevel::Info,
    };
    let start = diagnostic.range.start;
    Problem {
        column: Some(start.character as usize + 1),
        level,
        line: start.line as usize + 1,
        message: diagnostic.message.clone(),
        path: path.to_path_buf(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::test_support::app_with_tabs;

    #[test]
    fn test_an_empty_sidebar_opens_with_files_on_the_left_and_outline_on_the_right() {
        let mut app = app_with_tabs(1);
        app.toggle_panel(Panel::Sidebar(Edge::Left));
        assert_eq!(
            app.layout().sidebar(Edge::Left).active_kind(),
            Some(SidebarKind::Files)
        );
        app.toggle_panel(Panel::Sidebar(Edge::Right));
        assert_eq!(
            app.layout().sidebar(Edge::Right).active_kind(),
            Some(SidebarKind::Outline)
        );
        let focused = app.layout().focused();
        assert_eq!(
            app.layout().panel_of(focused),
            Some(Panel::Sidebar(Edge::Right)),
            "the keyboard in the sidebar shown"
        );
    }

    #[test]
    fn test_the_bottom_panel_opens_with_output_hides_and_comes_back() {
        let mut app = app_with_tabs(1);
        let main = app.layout().focused();
        app.toggle_panel(Panel::Bottom);
        let view = app.layout().focused();
        assert_eq!(
            app.layout().bottom_panel().kind_of(view),
            Some(BottomPanelKind::Output)
        );
        app.toggle_panel(Panel::Bottom);
        assert_eq!(app.layout().panel_state(Panel::Bottom), PanelState::Hidden);
        assert_eq!(app.layout().focused(), main, "back to the main area");
        app.toggle_panel(Panel::Bottom);
        assert_eq!(app.layout().focused(), view, "the same view");
    }

    #[test]
    fn test_a_shown_sidebar_hides_though_the_keyboard_is_elsewhere() {
        let mut app = app_with_tabs(1);
        let main = app.layout().focused();
        app.toggle_panel(Panel::Sidebar(Edge::Left));
        app.layout_mut().focus(main);
        app.toggle_panel(Panel::Sidebar(Edge::Left));
        assert_eq!(
            app.layout().panel_state(Panel::Sidebar(Edge::Left)),
            PanelState::Hidden
        );
        assert_eq!(app.layout().focused(), main, "the keyboard stays");
    }

    #[test]
    fn test_a_view_moves_to_the_other_sidebar_and_an_empty_sidebar_takes_a_free_view() {
        let mut app = app_with_tabs(1);
        app.open_sidebar_view(SidebarKind::Files);
        app.move_view_to_other_sidebar();
        assert_eq!(
            app.layout().sidebar_of(SidebarKind::Files),
            Some(Edge::Right)
        );
        app.toggle_panel(Panel::Sidebar(Edge::Left));
        assert_eq!(
            app.layout().sidebar(Edge::Left).active_kind(),
            Some(SidebarKind::Git),
            "Files is on the right"
        );
    }

    #[test]
    fn test_a_view_is_shown_only_while_its_panel_shows_it() {
        let mut app = app_with_tabs(1);
        app.open_sidebar_view(SidebarKind::Files);
        let files = app.layout().focused();
        app.open_sidebar_view(SidebarKind::Git);
        let git = app.layout().focused();
        app.sync_shown();
        assert!(app.is_shown(git), "the view shown");
        assert!(!app.is_shown(files), "the one behind it is not");
        app.layout_mut().hide_panel(Panel::Sidebar(Edge::Left));
        app.sync_shown();
        assert!(!app.is_shown(git), "a hidden panel's view is not shown");
    }

    #[test]
    fn test_a_restart_brings_back_the_open_folders_and_the_search() {
        let mut app = app_with_tabs(1);
        app.open_sidebar_view(SidebarKind::Files);
        app.open_bottom_view(BottomPanelKind::Search, false);
        let root = project::root(&app.focused_start_dir());
        let lines: Vec<String> = (0..MAX_SEARCH_LINES + 5)
            .map(|n| format!("line {n}"))
            .collect();
        app.restore_views(ViewsSession {
            folds: vec![root.join("src")],
            search: Some(SearchSession {
                lines,
                name: "Search".to_string(),
                query: String::new(),
            }),
        });
        let json = serde_json::to_string(&app.capture_session()).expect("written");
        let mut session: crate::session::Session = serde_json::from_str(&json).expect("read");
        let mut restored = app_with_tabs(1);
        restored.restore_docks(&session.docks.take().expect("the panels"));
        restored.restore_views(session.views.take().expect("the views"));
        let views = restored.views_session();
        assert_eq!(views.folds, [root.join("src")]);
        let search = views.search.expect("the search");
        assert_eq!(search.lines.len(), MAX_SEARCH_LINES, "its first lines only");
    }

    #[test]
    fn test_problems_lists_a_checker_s_finds_by_its_name_and_never_spelling() {
        use winter_lsp::protocol::{Position, Range};
        let mut app = app_with_tabs(1);
        let root = project::root(&app.focused_start_dir());
        let file = root.join(format!("winter-problems-{}.rs", std::process::id()));
        let at = |line| Diagnostic {
            code: None,
            message: format!("at {line}"),
            range: Range {
                end: Position { character: 0, line },
                start: Position { character: 0, line },
            },
            severity: Severity::Warning,
            source: None,
        };
        diagnostics::publish(Backend::Extension("lint-test"), &file, vec![at(4)]);
        diagnostics::publish(Backend::Spelling, &file, vec![at(7)]);
        let problems = app.open_bottom_view(BottomPanelKind::Problems, false);
        app.sync_shown();
        app.sync_builds();
        let sources = app
            .view_ref(problems, |view: &ProblemsView| view.source_names())
            .expect("the Problems view");
        diagnostics::publish(Backend::Extension("lint-test"), &file, Vec::new());
        diagnostics::publish(Backend::Spelling, &file, Vec::new());
        assert_eq!(sources, [("lint-test".to_string(), 1)], "spelling left out");
        let tick = app.view_ref(problems, |view: &ProblemsView| view.next_tick());
        assert_eq!(tick, Some(None), "no timer");
    }

    #[test]
    fn test_the_panels_come_back_from_their_session() {
        let mut app = app_with_tabs(1);
        app.open_sidebar_view(SidebarKind::Todo);
        app.open_bottom_view(BottomPanelKind::Problems, false);
        app.layout_mut().hide_panel(Panel::Bottom);
        let docks = app.docks_session();
        let mut restored = app_with_tabs(1);
        restored.restore_docks(&docks);
        assert_eq!(
            restored.layout().sidebar(Edge::Left).active_kind(),
            Some(SidebarKind::Todo)
        );
        assert_eq!(
            restored.layout().panel_state(Panel::Bottom),
            PanelState::Hidden
        );
        assert_eq!(
            restored.layout().bottom_panel().active_kind(),
            Some(BottomPanelKind::Problems)
        );
    }

    #[test]
    fn test_a_panel_first_shown_lists_every_view_and_shows_only_the_first() {
        let mut app = app_with_tabs(1);
        app.toggle_panel(Panel::Sidebar(Edge::Left));
        assert_eq!(
            app.layout().sidebar(Edge::Left).kinds(),
            [
                SidebarKind::Files,
                SidebarKind::Git,
                SidebarKind::Todo,
                SidebarKind::Sessions
            ]
        );
        assert_eq!(
            app.layout().sidebar(Edge::Right).kinds(),
            [SidebarKind::Outline]
        );
        assert_eq!(
            app.layout().bottom_panel().kinds(),
            BOTTOM_KINDS,
            "every panel made ready at once"
        );
        assert_eq!(
            app.layout().panel_state(Panel::Bottom),
            PanelState::Hidden,
            "the others stay out of sight"
        );
    }

    #[test]
    fn test_a_session_missing_a_view_gets_it_back_in_the_panel_it_starts_in() {
        let mut app = app_with_tabs(1);
        let mut docks = app.docks_session();
        docks.right.views = vec!["files".to_string()];
        docks.right.active = Some("files".to_string());
        app.restore_docks(&docks);
        assert_eq!(
            app.layout().sidebar(Edge::Right).kinds(),
            [SidebarKind::Files, SidebarKind::Outline],
            "Files where the session kept it"
        );
        assert_eq!(
            app.layout().sidebar(Edge::Left).kinds(),
            [
                SidebarKind::Git,
                SidebarKind::Todo,
                SidebarKind::Sessions
            ]
        );
        assert_eq!(
            app.layout().sidebar(Edge::Right).active_kind(),
            Some(SidebarKind::Files)
        );
    }

    #[test]
    fn test_reset_layout_restores_default_panel_sizes() {
        let mut app = app_with_tabs(1);
        app.apply_panel_sizes(&PanelSizes {
            bottom: Some(360.0),
            left: Some(420.0),
            right: Some(210.0),
        });
        assert_eq!(app.layout().panel_size(Panel::Sidebar(Edge::Left)), 420.0);

        app.reset_layout();

        assert_eq!(
            app.layout().panel_size(Panel::Sidebar(Edge::Left)),
            Panel::Sidebar(Edge::Left).default_size()
        );
        assert_eq!(
            app.layout().panel_size(Panel::Sidebar(Edge::Right)),
            Panel::Sidebar(Edge::Right).default_size()
        );
        assert_eq!(
            app.layout().panel_size(Panel::Bottom),
            Panel::Bottom.default_size()
        );
    }
}

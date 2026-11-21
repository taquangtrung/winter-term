//! The window's panels as the app runs them: showing and hiding each, the
//! views they show, and what the window feeds those views (the outline of
//! the tab worked in, the builds, the searches kept). The layout of the panels themselves
//! is [`winter_model::layout::panels`]; the views are [`views`].
//!
//! A view is a page the window holds in no pane, named by the id its panel
//! keeps. Every tab lives in the main area: nothing moves into a panel.

pub(in crate::window) mod views;

use std::time::{Duration, Instant};

use misty::picker::{Row, RowKey};
use winter_base::project;
use winter_model::editor::Page;
use winter_model::layout::{
    BottomPanelKind, DockSnapshot, Edge, Panel, PanelState, Panels, Region, SidebarKind, TabId,
};

use crate::editors::compile::CompileEditor;
use crate::editors::list::ListEditor;
use crate::session::{DockSession, DocksSession};
use crate::window::App;
use crate::window::editor::EditorKind;
use crate::window::picker::WindowOutcome;
use crate::window::picker::sources::listing::{Entry, Listing};
use views::files::FilesView;
use views::git::GitView;
use views::outline::OutlineView;
use views::output::{OutputView, Run};
use views::problems::ProblemsView;
use views::search::{ResultSet, SearchView};
use views::todo::TodoView;
use views::{PanelPage, PanelView};

// ========================================================================
// Constants
// ========================================================================

/// How often, at most, the Outline view reads the outline of the tab being
/// worked on again while that tab stays the same.
const OUTLINE_EVERY: Duration = Duration::from_millis(500);

/// What moving a view says when the keyboard is in no sidebar.
const NOT_IN_A_SIDEBAR: &str = "Only a sidebar's view moves to the other sidebar";

/// What filling a sidebar says when the other one holds every view.
const NO_VIEW_LEFT: &str = "The other sidebar holds every view";

/// Every sidebar view, in the order the view picker lists them.
const SIDEBAR_KINDS: [SidebarKind; 4] = [
    SidebarKind::Files,
    SidebarKind::Git,
    SidebarKind::Outline,
    SidebarKind::Todo,
];

/// Every bottom panel view, in the order the view picker lists them.
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
        }
    }

    /// The command that shows it.
    fn command(self) -> &'static str {
        match self {
            ViewKind::Bottom(BottomPanelKind::Output) => "bottom_panel_output",
            ViewKind::Bottom(BottomPanelKind::Problems) => "bottom_panel_problems",
            ViewKind::Bottom(BottomPanelKind::Search) => "bottom_panel_search",
            ViewKind::Sidebar(SidebarKind::Files) => "sidebar_files",
            ViewKind::Sidebar(SidebarKind::Git) => "sidebar_git",
            ViewKind::Sidebar(SidebarKind::Outline) => "sidebar_outline",
            ViewKind::Sidebar(SidebarKind::Todo) => "sidebar_todo",
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

    /// Show `panel` and give it the keyboard, or hide it when it has the
    /// keyboard already. An empty one is filled first: a sidebar with the
    /// view it starts with, the bottom panel with Output.
    pub(in crate::window) fn toggle_panel(&mut self, panel: Panel) {
        let focused = self.layout().focused();
        match self.layout().panel_state(panel) {
            PanelState::Empty => self.fill_panel(panel),
            PanelState::Shown if self.layout().panel_of(focused) == Some(panel) => {
                self.layout_mut().hide_panel(panel);
            }
            PanelState::Shown | PanelState::Hidden => {
                self.layout_mut().show_panel(panel);
            }
        }
        self.after_panel_change();
    }

    /// Show the sidebar view `kind` and give it the keyboard: where it is,
    /// else added to the sidebar it starts in.
    pub(in crate::window) fn open_sidebar_view(&mut self, kind: SidebarKind) {
        let tab = match self.sidebar_view(kind) {
            Some(tab) => tab,
            None => self.add_sidebar_view(start_edge(kind), kind),
        };
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
        let tab = match self.layout().bottom_panel().tab_of(kind) {
            Some(tab) => tab,
            None => self.add_bottom_view(kind),
        };
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

    /// The views `panel` can show, as a picker: what its switcher's button
    /// offers.
    pub(in crate::window) fn open_view_picker(&mut self, panel: Panel) {
        let views: Vec<ViewKind> = match panel {
            Panel::Bottom => BOTTOM_KINDS.into_iter().map(ViewKind::Bottom).collect(),
            Panel::Sidebar(_) => SIDEBAR_KINDS.into_iter().map(ViewKind::Sidebar).collect(),
        };
        let mut listing = Listing::new("panel_views", "Panel View").placeholder("View");
        for (index, view) in views.into_iter().enumerate() {
            let row = Row::new(RowKey(index as u64), view.title());
            listing.push(Entry::new(
                row,
                WindowOutcome::RunCommand(view.command().to_string()),
            ));
        }
        self.open_picker(Box::new(listing));
    }

    /// Move the focused sidebar view to the other sidebar.
    pub(in crate::window) fn move_view_to_other_sidebar(&mut self) {
        let focused = self.layout().focused();
        let Some(Panel::Sidebar(edge)) = self.layout().panel_of(focused) else {
            self.set_notice(NOT_IN_A_SIDEBAR);
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

    /// Feed every view what it shows of the window, and carry out what a
    /// view asked of it: called once a frame.
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

    /// Hand the Outline view the outline of the tab being worked on, the
    /// main-area tab that last had the keyboard, and take that tab to an
    /// entry the view was told to go to. Read again when the tab changes,
    /// else at most every [`OUTLINE_EVERY`].
    fn sync_outline(&mut self) {
        let Some(outline) = self.sidebar_view(SidebarKind::Outline) else {
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
        let now = Instant::now();
        let due = match self.tabs.outline_read {
            Some((read_from, at)) => read_from != source || now >= at + OUTLINE_EVERY,
            None => true,
        };
        if !due && chosen.is_none() {
            return;
        }
        self.tabs.outline_read = Some((source, now));
        let (name, entries, cursor) = match self.open_tabs.page_mut(&source) {
            Some(slot) => (
                Some(slot.page.title()),
                slot.page.outline_targets().unwrap_or_default(),
                slot.page.jump_cursor(),
            ),
            None => (None, Vec::new(), None),
        };
        // The entry the cursor's line falls under, followed while the view
        // does not have the keyboard.
        let in_use = self.layout().focused() == outline;
        let at = cursor
            .filter(|_| !in_use)
            .and_then(|cursor| entries.iter().rposition(|entry| entry.target <= cursor));
        self.view_mut(outline, |view: &mut OutlineView| {
            view.set_outline(name, entries, at)
        });
    }

    /// Hand Output the builds and the end of the one its cursor is on, and
    /// Problems what each build has found.
    fn sync_builds(&mut self) {
        let output = self.layout().bottom_panel().tab_of(BottomPanelKind::Output);
        let problems = self
            .layout()
            .bottom_panel()
            .tab_of(BottomPanelKind::Problems);
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
            for tab in builds {
                let found = self.compile_editor(tab, CompileEditor::problems);
                if let Some(found) = found {
                    self.view_mut(problems, |view: &mut ProblemsView| {
                        view.set_build(tab, found)
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
        let opened = self
            .with_view(BottomPanelKind::Search, SearchView::take_opened)
            .flatten();
        if let Some(set) = opened {
            let editor = ListEditor::new(&set.name, &set.query, set.lines);
            self.focus_main_area();
            self.show_editor(EditorKind::List, Box::new(editor));
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
    /// empty ones.
    pub(in crate::window) fn restore_docks(&mut self, docks: &DocksSession) {
        for (edge, dock) in [(Edge::Left, &docks.left), (Edge::Right, &docks.right)] {
            for name in &dock.views {
                if let Some(kind) = sidebar_kind(name)
                    && self.layout().sidebar_of(kind).is_none()
                {
                    self.add_sidebar_view(edge, kind);
                }
            }
            let active = dock.active.as_deref().and_then(sidebar_kind);
            let active = active.and_then(|kind| self.layout().sidebar(edge).tab_of(kind));
            self.restore_frame(Panel::Sidebar(edge), dock, active);
        }
        for name in &docks.bottom.views {
            if let Some(kind) = bottom_kind(name) {
                self.add_bottom_view(kind);
            }
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

    /// Point the views that show a project at the one the window works in
    /// now: after a workspace switch.
    pub(in crate::window) fn retarget_views(&mut self) {
        let root = project::root(&self.focused_start_dir());
        let files = self.sidebar_view(SidebarKind::Files);
        let git = self.sidebar_view(SidebarKind::Git);
        let todo = self.sidebar_view(SidebarKind::Todo);
        if let Some(tab) = files {
            self.view_mut(tab, |view: &mut FilesView| view.set_root(root.clone()));
        }
        if let Some(tab) = git {
            self.view_mut(tab, |view: &mut GitView| view.set_start(root.clone()));
        }
        if let Some(tab) = todo {
            self.view_mut(tab, |view: &mut TodoView| view.set_root(root.clone()));
        }
        self.tabs.outline_read = None;
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// The id of the sidebar view `kind`, wherever it is.
    fn sidebar_view(&self, kind: SidebarKind) -> Option<TabId> {
        let edge = self.layout().sidebar_of(kind)?;
        self.layout().sidebar(edge).tab_of(kind)
    }

    /// Fill the empty `panel` and show it with the keyboard: a sidebar with
    /// the first view the other sidebar does not hold, the bottom panel with
    /// Output.
    fn fill_panel(&mut self, panel: Panel) {
        match panel {
            Panel::Bottom => {
                self.open_bottom_view(BottomPanelKind::Output, true);
            }
            Panel::Sidebar(edge) => {
                let free = first_views(edge)
                    .into_iter()
                    .find(|kind| self.layout().sidebar_of(*kind).is_none());
                match free {
                    Some(kind) => {
                        let tab = self.add_sidebar_view(edge, kind);
                        self.layout_mut().focus(tab);
                    }
                    None => self.set_notice(NO_VIEW_LEFT),
                }
            }
        }
    }

    /// Make the sidebar view `kind` and add it to the sidebar on `edge`,
    /// without moving the keyboard. Its id.
    fn add_sidebar_view(&mut self, edge: Edge, kind: SidebarKind) -> TabId {
        let tab = self.alloc_tab_id();
        let page = self.build_view(ViewKind::Sidebar(kind));
        self.open_tabs
            .insert_page(tab, EditorKind::View(ViewKind::Sidebar(kind)), page);
        self.layout_mut().add_sidebar_view(edge, kind, tab, false);
        tab
    }

    /// Make the bottom panel view `kind` and add it to the bottom panel,
    /// without moving the keyboard. Its id.
    fn add_bottom_view(&mut self, kind: BottomPanelKind) -> TabId {
        let tab = self.alloc_tab_id();
        let page = self.build_view(ViewKind::Bottom(kind));
        self.open_tabs
            .insert_page(tab, EditorKind::View(ViewKind::Bottom(kind)), page);
        self.layout_mut().add_bottom_view(kind, tab, false);
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

    /// Put back `panel`'s shown view, whether it is shown, and its size, as
    /// `dock` kept them.
    fn restore_frame(&mut self, panel: Panel, dock: &DockSession, active: Option<TabId>) {
        if let Some(active) = active {
            self.layout_mut().bring_to_front(active);
        }
        self.layout_mut().set_panel_size(panel, dock.size);
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
        SidebarKind::Files | SidebarKind::Git | SidebarKind::Todo => Edge::Left,
    }
}

/// The views an empty sidebar on `edge` fills with, the first free one
/// taken: what it starts with first.
fn first_views(edge: Edge) -> [SidebarKind; 4] {
    match edge {
        Edge::Left => [
            SidebarKind::Files,
            SidebarKind::Git,
            SidebarKind::Todo,
            SidebarKind::Outline,
        ],
        Edge::Right => [
            SidebarKind::Outline,
            SidebarKind::Files,
            SidebarKind::Git,
            SidebarKind::Todo,
        ],
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
            restored.layout().bottom_panel().kinds(),
            [BottomPanelKind::Problems]
        );
    }
}

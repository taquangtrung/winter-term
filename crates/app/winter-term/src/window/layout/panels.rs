//! The window's panels as the app runs them: showing and hiding each, tabs
//! moved between them and the main area, where each kind of tab opens, and
//! the launcher that keeps the main area from emptying. The layout of the
//! panels themselves is [`winter_model::layout::panels`].
//!
//! The sidebars hold a fixed set of tools ([`SidebarTool`]), one shown at a
//! time; the bottom panel holds any tab, a shell or a build most often.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use misty::picker::{Row, RowKey};
use winter_base::project;
use winter_dir::DirEditor;
use winter_git::GitEditor;
use winter_model::layout::{Direction, PanelState, Region, Side, TabId};
use winter_terminal::session::git_editor_command;

use crate::editors::launcher::LauncherEditor;
use crate::editors::outline::OutlinePage;
use crate::window::App;
use crate::window::editor::EditorKind;
use crate::window::picker::WindowOutcome;
use crate::window::picker::sources::listing::{Entry, Listing};

// ========================================================================
// Constants
// ========================================================================

/// How often, at most, the Outline page reads the outline of the file being
/// worked on again while that file stays the same.
const OUTLINE_EVERY: Duration = Duration::from_millis(500);

/// What a sidebar says when a tab that is no sidebar tool is moved to it.
const NOT_A_TOOL: &str = "Only the sidebar tools go in a sidebar: Files, Git, Outline, and Todo";

// ========================================================================
// Data Structures
// ========================================================================

/// A tool a sidebar holds: one of a fixed set, each opened by its command
/// (`Sidebar: Files` and the rest) and remembered for the side it was last
/// shown on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum SidebarTool {
    /// The project's directory, as a tree.
    Files,
    /// The project's git status.
    Git,
    /// The headings and symbols of the file being worked on.
    Outline,
    /// The project's TODO comments.
    Todo,
}

// ========================================================================
// SidebarTool Implementations
// ========================================================================

impl SidebarTool {
    /// Every tool, in the order the chooser lists them.
    pub(in crate::window) const ALL: [SidebarTool; 4] = [
        SidebarTool::Files,
        SidebarTool::Git,
        SidebarTool::Outline,
        SidebarTool::Todo,
    ];

    /// What the session file and the chooser call the tool.
    pub(in crate::window) fn name(self) -> &'static str {
        match self {
            SidebarTool::Files => "files",
            SidebarTool::Git => "git",
            SidebarTool::Outline => "outline",
            SidebarTool::Todo => "todo",
        }
    }

    /// The tool a stored name stands for.
    pub(in crate::window) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.name() == name)
    }

    /// The command that opens it.
    fn command(self) -> &'static str {
        match self {
            SidebarTool::Files => "sidebar_files",
            SidebarTool::Git => "sidebar_git",
            SidebarTool::Outline => "sidebar_outline",
            SidebarTool::Todo => "sidebar_todo",
        }
    }

    /// How the chooser names it.
    fn title(self) -> &'static str {
        match self {
            SidebarTool::Files => "Files",
            SidebarTool::Git => "Git",
            SidebarTool::Outline => "Outline",
            SidebarTool::Todo => "Todo",
        }
    }

    /// The kind of tab it is.
    fn kind(self) -> EditorKind {
        match self {
            SidebarTool::Files => EditorKind::Dir,
            SidebarTool::Git => EditorKind::Git,
            SidebarTool::Outline => EditorKind::Outline,
            SidebarTool::Todo => EditorKind::Todo,
        }
    }

    /// The tool a tab of `kind` is, if any.
    fn of_kind(kind: EditorKind) -> Option<Self> {
        Self::ALL.into_iter().find(|tool| tool.kind() == kind)
    }

    /// The sidebar it opens in until moved: the outline on the right, the
    /// rest on the left.
    fn default_side(self) -> Side {
        match self {
            SidebarTool::Outline => Side::Right,
            SidebarTool::Files | SidebarTool::Git | SidebarTool::Todo => Side::Left,
        }
    }

    /// The tool an empty sidebar on `side` opens with, before one was chosen
    /// there.
    fn first_on(side: Side) -> Self {
        match side {
            Side::Right => SidebarTool::Outline,
            Side::Bottom | Side::Left => SidebarTool::Files,
        }
    }
}

// ========================================================================
// App: panels
// ========================================================================

impl App {
    /// Show the panel on `side` and give it the keyboard, or hide it when it
    /// has the keyboard already. An empty one is filled first: the bottom
    /// panel with a shell where the work is, a sidebar with the tool last
    /// shown there. Hidden, a panel's tabs keep running.
    pub(in crate::window) fn toggle_panel(&mut self, side: Side) {
        let focused = self.layout().focused();
        match self.layout().panel_state(side) {
            PanelState::Empty => self.fill_panel(side),
            PanelState::Shown if self.layout().region_of(focused) == Some(Region::Panel(side)) => {
                self.layout_mut().hide_panel(side);
            }
            PanelState::Shown | PanelState::Hidden => {
                self.layout_mut().show_panel(side);
            }
        }
        self.after_panel_change();
    }

    /// Show `tool` in its sidebar and give it the keyboard: where it is
    /// already, else opened in the sidebar it opens in (see
    /// [`SidebarTool::default_side`]), taken from the main area where a tab
    /// of it is there.
    pub(in crate::window) fn open_sidebar_tool(&mut self, tool: SidebarTool) {
        let side = self.sidebar_of(tool).unwrap_or_else(|| tool.default_side());
        self.open_sidebar_tool_on(tool, side);
        self.after_panel_change();
    }

    /// The tools a sidebar can hold, as a picker: what a sidebar's new tab
    /// button and new tab key offer, a sidebar holding no other tab.
    pub(in crate::window) fn open_sidebar_tool_picker(&mut self) {
        let mut listing = Listing::new("sidebar_tools", "Sidebar Tool").placeholder("Tool");
        for (index, tool) in SidebarTool::ALL.into_iter().enumerate() {
            let row = Row::new(RowKey(index as u64), tool.title());
            listing.push(Entry::new(
                row,
                WindowOutcome::RunCommand(tool.command().to_string()),
            ));
        }
        self.open_picker(Box::new(listing));
    }

    /// Move the focused tab into `region`, remembering it there for its kind
    /// (see [`Self::region_for`]). The main area's last tab leaves a launcher
    /// in its place. A sidebar takes only its tools.
    pub(in crate::window) fn move_focused_tab_to(&mut self, region: Region) {
        let tab = self.layout().focused();
        if self.layout().region_of(tab) == Some(region) || self.refuses(tab, region) {
            return;
        }
        self.keep_main_area(tab);
        if !self.layout_mut().move_to_region(tab, region) {
            return;
        }
        self.remember_region(tab);
        self.after_tab_change();
        self.after_panel_change();
    }

    /// Whether `region` refuses `tab`, said so: a sidebar takes only its
    /// tools.
    pub(in crate::window) fn refuses(&mut self, tab: TabId, region: Region) -> bool {
        let is_tool = self
            .open_tabs
            .page(&tab)
            .is_some_and(|slot| SidebarTool::of_kind(slot.kind()).is_some());
        if is_sidebar(region) && !is_tool {
            self.set_notice(NOT_A_TOOL);
            return true;
        }
        false
    }

    /// Remember the region `tab` is in now: for a sidebar tool, the tool its
    /// sidebar shows; for another kind, where the next tab of its kind opens
    /// (the bottom panel or the main area).
    pub(in crate::window) fn remember_region(&mut self, tab: TabId) {
        let Some(kind) = self.open_tabs.page(&tab).map(|slot| slot.kind()) else {
            return;
        };
        match self.layout().region_of(tab) {
            Some(Region::Panel(side)) if is_sidebar(Region::Panel(side)) => {
                if let Some(tool) = SidebarTool::of_kind(kind) {
                    self.tabs.sidebar_tools.insert(side, tool);
                }
            }
            Some(region) if is_placed(kind) => {
                self.tabs.placement.insert(kind.name().to_string(), region);
            }
            _ => {}
        }
    }

    /// Where a new tab of `kind` opens: the region a tab of its kind was
    /// last moved to, else where it starts (a build in the bottom panel, the
    /// rest in the main area). Never a sidebar, which holds only the tools
    /// its commands open. The text editor and the launcher always open in
    /// the main area.
    pub(in crate::window) fn region_for(&self, kind: EditorKind) -> Region {
        if !is_placed(kind) {
            return Region::Main;
        }
        match self.tabs.placement.get(kind.name()) {
            Some(region) if !is_sidebar(*region) => *region,
            Some(_) => Region::Main,
            None if kind == EditorKind::Compile => Region::Panel(Side::Bottom),
            None => Region::Main,
        }
    }

    /// Put tab `tab`, a new or unburied tab of `kind`, where its kind opens
    /// (see [`Self::region_for`]), showing and focusing it: in its panel,
    /// or in the main area beside the main tab that last had focus.
    pub(in crate::window) fn place_tab(&mut self, tab: TabId, kind: EditorKind) {
        match self.region_for(kind) {
            Region::Panel(side) => self.layout_mut().open_in_panel(side, tab, true),
            Region::Main => {
                self.focus_main_area();
                self.layout_mut().add_tab(tab);
            }
        }
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

    /// Before a tab joins the focused pane: where a sidebar has the
    /// keyboard, give it to the main area, a sidebar holding only its tools.
    pub(in crate::window) fn leave_sidebar(&mut self) {
        let focused = self.layout().focused();
        if self.layout().region_of(focused).is_some_and(is_sidebar) {
            self.focus_main_area();
        }
    }

    /// Whether a sidebar has the keyboard.
    pub(in crate::window) fn sidebar_focused(&self) -> bool {
        let focused = self.layout().focused();
        self.layout().region_of(focused).is_some_and(is_sidebar)
    }

    /// Before `tab` leaves the main area, by closing or by moving to a
    /// panel: when it is the main area's last tab, a launcher joins its pane
    /// to take its place, so the main area is never left without one.
    pub(in crate::window) fn keep_main_area(&mut self, tab: TabId) {
        self.keep_main_area_of(&[tab], tab);
    }

    /// The same for `leaving`, tabs about to go at once: when they are all
    /// the main area holds, a launcher joins the pane holding `beside`.
    pub(in crate::window) fn keep_main_area_of(&mut self, leaving: &[TabId], beside: TabId) {
        let main = self.layout().main_members();
        if main.is_empty() || !main.iter().all(|tab| leaving.contains(tab)) {
            return;
        }
        let dir = self.focused_start_dir();
        let launcher = self.alloc_tab_id();
        self.open_tabs.insert_editor(
            launcher,
            EditorKind::Launcher,
            Box::new(LauncherEditor::new(dir)),
        );
        self.layout_mut().add_tab_beside(beside, launcher, false);
    }

    /// Hand every Outline page the outline of the file being worked on, the
    /// main-area tab that last had the keyboard, and take that file to an
    /// entry one was told to go to. Read again when the file changes, else
    /// at most every [`OUTLINE_EVERY`].
    pub(in crate::window) fn sync_outline(&mut self) {
        let Some(outline) = self.page_tab(EditorKind::Outline) else {
            return;
        };
        let source = self.layout().main_return();
        let chosen = self
            .open_tabs
            .page_mut(&outline)
            .and_then(|slot| {
                slot.page
                    .with_kind_mut(|page: &mut OutlinePage| page.take_chosen())
            })
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
        // The entry the cursor's line falls under, followed while the page
        // is not in use.
        let in_use = self.layout().focused() == outline;
        let at = cursor
            .filter(|_| !in_use)
            .and_then(|cursor| entries.iter().rposition(|entry| entry.target <= cursor));
        if let Some(slot) = self.open_tabs.page_mut(&outline) {
            slot.page
                .with_kind_mut(|page: &mut OutlinePage| page.set_outline(name, entries, at));
        }
    }

    /// What each sidebar last showed, by side name, as the session keeps it.
    pub(in crate::window) fn sidebar_tool_names(&self) -> BTreeMap<String, String> {
        self.tabs
            .sidebar_tools
            .iter()
            .filter_map(|(side, tool)| {
                Some((side_name(*side)?.to_string(), tool.name().to_string()))
            })
            .collect()
    }

    /// Take back what each sidebar last showed, from `names` as the session
    /// kept them.
    pub(in crate::window) fn restore_sidebar_tools(&mut self, names: &BTreeMap<String, String>) {
        self.tabs.sidebar_tools = names
            .iter()
            .filter_map(|(side, tool)| Some((side_from_name(side)?, SidebarTool::from_name(tool)?)))
            .collect();
    }

    /// The sidebar holding a tab of `tool`, if one does.
    fn sidebar_of(&self, tool: SidebarTool) -> Option<Side> {
        [Side::Left, Side::Right].into_iter().find(|&side| {
            self.layout().panel_tab(side).is_some_and(|front| {
                self.layout().tabs_in_pane(front).iter().any(|tab| {
                    self.open_tabs
                        .page(tab)
                        .is_some_and(|slot| slot.kind() == tool.kind())
                })
            })
        })
    }

    /// Show `tool` in the sidebar on `side`, opening it there, or moving a
    /// tab of it there from the main area, where it has none; and remember
    /// it as what that sidebar shows.
    fn open_sidebar_tool_on(&mut self, tool: SidebarTool, side: Side) {
        let held = self.layout().panel_tab(side).and_then(|front| {
            self.layout().tabs_in_pane(front).into_iter().find(|tab| {
                self.open_tabs
                    .page(tab)
                    .is_some_and(|slot| slot.kind() == tool.kind())
            })
        });
        let page = self.page_tab(tool.kind()).filter(|_| held.is_none());
        match (held, page) {
            (Some(tab), _) => {
                self.layout_mut().focus(tab);
            }
            (None, Some(tab)) => {
                self.keep_main_area(tab);
                self.layout_mut().move_to_region(tab, Region::Panel(side));
            }
            (None, None) => self.open_new_tool(tool, side),
        }
        self.tabs.sidebar_tools.insert(side, tool);
        self.after_tab_change();
    }

    /// Open a new tab of `tool` in the sidebar on `side`: the project the
    /// focused tab is in, for the tools that show one.
    fn open_new_tool(&mut self, tool: SidebarTool, side: Side) {
        let root = project::root(&self.focused_start_dir());
        let tab = self.alloc_tab_id();
        match tool {
            SidebarTool::Files => {
                self.open_tabs
                    .insert_editor(tab, EditorKind::Dir, Box::new(DirEditor::new(root)));
            }
            SidebarTool::Git => {
                let editor = GitEditor::new(root, git_editor_command());
                // The view knows nothing until git answers, as when opened
                // any other way.
                let first = editor.initial_request();
                self.open_tabs
                    .insert_editor(tab, EditorKind::Git, Box::new(editor));
                self.host.jobs.spawn(tab, first);
            }
            SidebarTool::Outline => {
                self.open_tabs
                    .insert_page(tab, EditorKind::Outline, Box::new(OutlinePage::new()));
                self.tabs.outline_read = None;
            }
            SidebarTool::Todo => {
                let Some(page) = self.built_page(EditorKind::Todo) else {
                    return;
                };
                self.open_tabs.insert_page(tab, EditorKind::Todo, page);
            }
        }
        self.layout_mut().open_in_panel(side, tab, true);
    }

    /// Fill the empty panel on `side` and show it: the bottom panel with a
    /// shell started where the focused tab is working, a sidebar with the
    /// tool last shown there.
    fn fill_panel(&mut self, side: Side) {
        match side {
            Side::Bottom => self.open_shell_in_panel(side),
            Side::Left | Side::Right => {
                let tool = self
                    .tabs
                    .sidebar_tools
                    .get(&side)
                    .copied()
                    .unwrap_or_else(|| SidebarTool::first_on(side));
                self.open_sidebar_tool_on(tool, side);
            }
        }
    }

    /// Open a shell in the panel on `side`, started where the focused tab is
    /// working; nothing, and said so, where no shell could be started.
    fn open_shell_in_panel(&mut self, side: Side) {
        let cwd = self.focused_start_cwd();
        let new_id = self.alloc_tab_id();
        self.layout_mut().open_in_panel(side, new_id, true);
        let (pane_cols, pane_rows) = self.spawn_grid_size(new_id, Direction::Horizontal);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_emulator::MAX_SCROLLBACK);
        let Some(terminal) = self.spawn_terminal_or_notify(
            pane_cols.max(1),
            pane_rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            self.layout_mut().close(new_id);
            return;
        };
        self.open_tabs.insert_terminal(new_id, terminal);
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

/// Whether tabs of `kind` remember where they were moved: every page and
/// editor but the text editor and the launcher, which open in the main area
/// or, by the new tab key, in the focused panel.
fn is_placed(kind: EditorKind) -> bool {
    !matches!(kind, EditorKind::Launcher | EditorKind::Text)
}

/// What the session calls the sidebar on `side`; `None` for the bottom
/// panel, which shows no tool.
fn side_name(side: Side) -> Option<&'static str> {
    match side {
        Side::Left => Some("left"),
        Side::Right => Some("right"),
        Side::Bottom => None,
    }
}

/// The sidebar the session's `name` stands for.
fn side_from_name(name: &str) -> Option<Side> {
    match name {
        "left" => Some(Side::Left),
        "right" => Some(Side::Right),
        _ => None,
    }
}

/// Whether `region` is a sidebar.
fn is_sidebar(region: Region) -> bool {
    matches!(region, Region::Panel(Side::Left | Side::Right))
}

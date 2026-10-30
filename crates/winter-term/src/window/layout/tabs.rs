//! Tabs within a pane: opening, showing, cycling, reordering, and closing them,
//! and the recency walk across the focused pane's strip.

use std::collections::HashMap;

use crate::model::layout::{FocusDir, Layout, Rect, TabId};
use crate::terminal::session::Terminal;

use crate::window::editor::Closing;
use crate::window::strip::StripHit;
use crate::window::{App, DEFAULT_COLS, DEFAULT_ROWS};
use winter_render::{Region, TabbarHit};

// ========================================================================
// Data Structures
// ========================================================================

/// The window's layout of panes and the state of the strips drawn over
/// them: hover, an in-progress drag, the rename prompt, and a recency walk.
pub(in crate::window) struct TabsState {
    /// The tab being dragged along its strip and the pointer (x, y) position when
    /// the drag began. `None` when no drag is in progress. Cleared on mouse
    /// release.
    pub(in crate::window) drag_start: Option<(TabId, f32, f32)>,
    /// Title-bar element currently under the cursor; drives hover highlights.
    pub(in crate::window) hover: TabbarHit,
    /// Where the strip tab under the pointer is, for its tooltip to hang
    /// under.
    pub(in crate::window) hover_anchor: Option<Region>,
    /// The split tree of panes.
    pub(in crate::window) layout: Layout,
    /// What each tab is called, by the user and by its program.
    pub(in crate::window) labels: HashMap<TabId, TabLabel>,
    /// The focused pane's recency order, frozen while the recency commands
    /// step through it, so repeated steps walk usage history instead of
    /// reshuffling it. `None` once a deliberate switch ends the walk.
    pub(in crate::window) recent_walk: Option<RecentWalk>,
    /// Pane-strip element currently under the cursor; drives hover highlights.
    pub(in crate::window) strip_hover: StripHit,
}

/// What a tab is called: the name the user gave it, which wins, and the
/// title its program set (OSC 0/2).
#[derive(Clone, Debug, Default)]
pub(in crate::window) struct TabLabel {
    pub(in crate::window) name: Option<String>,
    pub(in crate::window) title: Option<String>,
}

/// A recency walk in progress: the order it walks and where it has got to.
pub(in crate::window) struct RecentWalk {
    pub(in crate::window) at: usize,
    pub(in crate::window) order: Vec<TabId>,
}

impl Default for TabsState {
    /// A fresh window: one pane holding one tab.
    fn default() -> Self {
        Self {
            drag_start: None,
            hover: TabbarHit::None,
            hover_anchor: None,
            layout: Layout::new(),
            labels: HashMap::new(),
            recent_walk: None,
            strip_hover: StripHit::None,
        }
    }
}

// ========================================================================
// TabsState Implementations
// ========================================================================

impl TabsState {
    /// The name the user gave tab `tab`, if any.
    pub(in crate::window) fn name(&self, tab: TabId) -> Option<&str> {
        self.labels.get(&tab)?.name.as_deref()
    }

    /// Name tab `tab`, or drop its name with `None` or an empty one.
    pub(in crate::window) fn set_name(&mut self, tab: TabId, name: Option<String>) {
        self.labels.entry(tab).or_default().name = name.filter(|name| !name.is_empty());
    }

    /// The title tab `tab`'s program set, if it set one.
    pub(in crate::window) fn program_title(&self, tab: TabId) -> Option<&str> {
        self.labels.get(&tab)?.title.as_deref()
    }

    /// Record the title tab `tab`'s program set.
    pub(in crate::window) fn set_program_title(&mut self, tab: TabId, title: String) {
        self.labels.entry(tab).or_default().title = Some(title);
    }

    /// Forget the title tab `tab`'s program set, once it has finished.
    pub(in crate::window) fn clear_program_title(&mut self, tab: TabId) {
        if let Some(label) = self.labels.get_mut(&tab) {
            label.title = None;
        }
    }
}

// ========================================================================
// App: tabs
// ========================================================================

impl App {
    // --------------------------------------------------------------------
    // Layout access
    // --------------------------------------------------------------------

    /// The window's layout.
    pub(in crate::window) fn layout(&self) -> &Layout {
        &self.tabs.layout
    }

    /// The window's layout, mutably.
    pub(in crate::window) fn layout_mut(&mut self) -> &mut Layout {
        &mut self.tabs.layout
    }

    // --------------------------------------------------------------------
    // Opening
    // --------------------------------------------------------------------

    /// Open a new terminal tab beside the focused one and show it.
    pub(in crate::window) fn new_tab(&mut self) {
        let id = self.alloc_tab_id();
        // Open the new tab where the focused tab was being used rather than
        // at the process default (usually `$HOME`): beside an editor that is the
        // directory the editor is looking at, not a shell's own.
        let cwd = self.focused_start_cwd();
        let (cols, rows) = self.focused_grid_size();
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_core::MAX_SCROLLBACK);
        let Some(terminal) = self.spawn_terminal_or_notify(
            cols.max(1),
            rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            return;
        };
        self.push_new_tab(id, terminal);
    }

    /// Open a new window running the binary in a new process.
    pub(in crate::window) fn new_window(&mut self) {
        let Some(exe) = crate::terminal::session::runnable_current_exe() else {
            self.set_error("new_window: could not determine binary path".to_string());
            return;
        };
        let mut cmd = std::process::Command::new(exe);
        if let Some(cwd) = self.focused_start_cwd() {
            cmd.env("WINTER_CWD", &cwd);
            cmd.current_dir(&cwd);
        }
        // The new window keeps no terminal of this one's: it reads no input
        // from it and writes nowhere it is watching.
        cmd.stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        match cmd.spawn() {
            // Waited for on a thread of its own, so a window closed before
            // this one is reaped then, not left a zombie until this one exits.
            Ok(mut child) => {
                let reaper = std::thread::Builder::new()
                    .name("winter window reap".into())
                    .spawn(move || {
                        let _ = child.wait();
                    });
                if let Err(e) = reaper {
                    eprintln!("winter: could not watch the new window: {e}");
                }
            }
            Err(e) => self.set_error(format!("failed to spawn new window: {e}")),
        }
    }

    /// Open a new tab whose terminal is attached to a running mux session;
    /// the session's buffered output replays into it.
    pub(in crate::window) fn new_mux_tab(&mut self, session: &str) {
        self.new_mux_tab_at(&crate::mux::server::default_socket_path(), session);
    }

    /// Open a new tab attached to a session on the mux server at `path`.
    pub(in crate::window) fn new_mux_tab_at(&mut self, path: &str, session: &str) {
        let id = self.alloc_tab_id();
        let (cols, rows) = self.focused_grid_size();
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_core::MAX_SCROLLBACK);
        match Terminal::new_mux_at(path, cols.max(1), rows.max(1), session, scrollback) {
            Ok(terminal) => {
                self.set_notice(format!("attached to mux session '{session}'"));
                self.push_new_tab(id, terminal);
            }
            Err(e) => self.set_error(format!(
                "could not attach to '{session}' ({e}); start the server with 'winter mux serve'"
            )),
        }
    }

    /// Open a new tab attached to a session on a mux server reached over ssh
    /// at `host`.
    pub(in crate::window) fn new_mux_tab_remote_at(&mut self, host: &str, session: &str) {
        let id = self.alloc_tab_id();
        let (cols, rows) = self.focused_grid_size();
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_core::MAX_SCROLLBACK);
        match Terminal::new_mux_remote(host, cols.max(1), rows.max(1), session, scrollback) {
            Ok(terminal) => {
                self.set_notice(format!("attached to '{host}:{session}'"));
                self.push_new_tab(id, terminal);
            }
            Err(e) => self.set_error(format!("could not reach '{host}' over ssh ({e})")),
        }
    }

    /// Install an already-spawned terminal as a new tab beside the focused
    /// one and show it. Shared by every way a terminal tab is opened so all of
    /// them get the same bookkeeping: mode default, tile repositioning,
    /// resize, and title update.
    pub(in crate::window) fn push_new_tab(&mut self, id: TabId, terminal: Terminal) {
        self.open_tabs.insert_terminal(id, terminal);
        self.layout_mut().add_tab(id);
        self.after_tab_change();
    }

    /// The grid a new tab in the focused pane starts at, before
    /// [`Self::resize_all_panes`] fixes the exact size once it is placed.
    pub(in crate::window) fn focused_grid_size(&self) -> (usize, usize) {
        let focused = self.layout().focused();
        if let Some((_, rect)) = self.pane_rects().into_iter().find(|(id, _)| *id == focused) {
            if let Some(renderer) = &self.renderer {
                return renderer.grid_size_for(Self::layout_rect_to_pane(rect));
            }
        }
        self.renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize))
    }

    // --------------------------------------------------------------------
    // Switching
    // --------------------------------------------------------------------

    /// Show `pane` as a deliberate selection, wherever it is, ending any
    /// recency walk.
    pub(in crate::window) fn show_tab(&mut self, tab: TabId) {
        if self.open_tabs.buried().contains(&tab) {
            self.show_buffer(tab);
            return;
        }
        if !self.set_focus(tab) {
            return;
        }
        self.tabs.recent_walk = None;
        self.after_tab_change();
    }

    /// Show the most recently shown terminal of the focused pane, for the way
    /// back from an editor tab to the shell it was opened from. A terminal that
    /// is already showing stays showing; a pane with none gets a new one,
    /// started in the directory its editor is looking at.
    pub(in crate::window) fn reveal_recent_terminal(&mut self) {
        let focused = self.layout().focused();
        let terminal = self
            .layout()
            .recent_in_pane(focused)
            .into_iter()
            .find(|tab| self.open_tabs.is_terminal(tab) && !self.open_tabs.is_editor(tab));
        match terminal {
            Some(tab) => self.show_tab(tab),
            None => self.new_tab(),
        }
    }

    /// Show the tab at strip position `index` (0-based) of the focused pane.
    pub(in crate::window) fn switch_tab(&mut self, index: usize) {
        let members = self.layout().tabs_in_pane(self.layout().focused());
        if let Some(&pane) = members.get(index) {
            self.show_tab(pane);
        }
    }

    /// Show the next (`forward`) or previous tab of the focused pane by
    /// strip position, without wrapping around.
    pub(in crate::window) fn cycle_tab(&mut self, forward: bool) {
        if self.layout_mut().cycle_tab(forward) {
            self.tabs.recent_walk = None;
            self.after_tab_change();
        }
    }

    /// Step through the focused pane's tabs in most-recently-shown order:
    /// `forward` steps toward more recently shown, otherwise toward less,
    /// wrapping around. The order is held still across consecutive calls (a
    /// "walk") so the user can step back and forth through usage history; the
    /// next deliberate switch ends the walk.
    pub(in crate::window) fn recent_tab(&mut self, forward: bool) {
        let focused = self.layout().focused();
        let walk = match self.tabs.recent_walk.take() {
            Some(walk) if walk.order.contains(&focused) => walk,
            _ => RecentWalk {
                at: 0,
                order: self.layout().recent_in_pane(focused),
            },
        };
        let count = walk.order.len();
        if count <= 1 {
            return;
        }
        let at = match forward {
            true => (walk.at + count - 1) % count,
            false => (walk.at + 1) % count,
        };
        let tab = walk.order[at];
        self.set_focus(tab);
        self.tabs.recent_walk = Some(RecentWalk {
            at,
            order: walk.order,
        });
        self.after_tab_change();
    }

    /// Move the focused tab one place along its strip.
    pub(in crate::window) fn move_tab(&mut self, forward: bool) {
        if self.layout_mut().move_tab(forward) {
            self.dirty = true;
        }
    }

    /// Move `pane` to where `target` sits on its strip or across panes.
    pub(in crate::window) fn move_tab_to(&mut self, tab: TabId, target: TabId) {
        if self.layout_mut().move_tab_to(tab, target) {
            self.merge_views_in_pane(tab);
            self.after_tab_change();
        }
    }

    /// Move `tab` into the pane holding `target_pane_tab`, at the end of that pane.
    pub(in crate::window) fn move_tab_to_pane(&mut self, tab: TabId, target_pane_tab: TabId) {
        if self.layout_mut().move_tab_to_pane(tab, target_pane_tab) {
            self.merge_views_in_pane(tab);
            self.after_tab_change();
        }
    }

    /// A pane never has two tabs on one buffer: where `tab` has just come into
    /// a pane already showing its buffer, `tab` closes and the tab that was
    /// there takes the focus.
    fn merge_views_in_pane(&mut self, tab: TabId) {
        let pane = self.layout().tabs_in_pane(tab);
        let there = self
            .open_tabs
            .views_of(&tab)
            .into_iter()
            .find(|view| *view != tab && pane.contains(view));
        if let Some(there) = there {
            self.close_tab_now(tab, Closing::Keep);
            self.show_tab(there);
        }
    }

    /// Split `target_pane_tab`'s pane to make `tab` a new pane in `direction`.
    pub(in crate::window) fn split_tab_to_new_pane(
        &mut self,
        tab: TabId,
        target_pane_tab: TabId,
        direction: FocusDir,
    ) {
        if self
            .layout_mut()
            .split_tab_to_new_pane(tab, target_pane_tab, direction)
        {
            self.after_tab_change();
        }
    }

    /// Move the focused tab into the neighbouring pane in the given direction.
    /// If there is no pane in that direction, make that tab become the new pane.
    pub(in crate::window) fn move_tab_in_direction(&mut self, direction: FocusDir) {
        if self.layout().is_zoomed() {
            return;
        }
        let viewport = self.viewport_rect();
        let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
        let focused = self.layout().focused();
        if let Some(target) = self.layout().pane_in_direction(direction, layout_vp) {
            self.move_tab_to_pane(focused, target);
        } else if self.layout_mut().move_tab_to_new_pane(focused, direction) {
            self.after_tab_change();
        }
    }

    // --------------------------------------------------------------------
    // Closing
    // --------------------------------------------------------------------

    /// Close one tab. Closing the last tab of the window asks for the app to
    /// exit instead.
    ///
    /// Unwritten edits in an editor tab are asked about first, and the tab
    /// closes on the answer rather than here.
    pub(in crate::window) fn close_tab(&mut self, tab: TabId) {
        if self.ask_before_closing_tab(tab) {
            return;
        }
        self.close_tab_now(tab, Closing::Keep);
    }

    /// Close every other tab in the pane holding `keep`, leaving that tab.
    pub(in crate::window) fn close_other_tabs(&mut self, keep: TabId) {
        if self.ask_before_closing_other_tabs(keep) {
            return;
        }
        self.close_other_tabs_now(keep);
    }

    /// The same, once the question has been answered or there was none.
    pub(in crate::window) fn close_other_tabs_now(&mut self, keep: TabId) {
        let others: Vec<TabId> = self
            .layout()
            .tabs_in_pane(keep)
            .into_iter()
            .filter(|tab| *tab != keep)
            .collect();
        for tab in others {
            self.close_tab_now(tab, Closing::Keep);
        }
    }

    /// Close one tab without asking: an editor's buffer is buried, or killed
    /// with `Closing::Forget`, and a terminal's shell ends. A pane left with no
    /// tabs merges back into its sibling.
    pub(in crate::window) fn close_tab_now(&mut self, tab: TabId, closing: Closing) {
        if !self.layout().contains(tab) {
            return;
        }
        if self.layout().members().len() <= 1 {
            self.runtime.exit.requested = true;
            return;
        }
        let pane_gone = self.layout().tabs_in_pane(tab).len() <= 1;
        let commit_repo = self.open_tabs.editor(&tab).and_then(|slot| slot.editor.returns_to_git());
        match self.buries(tab, closing) {
            true => self.bury_tab(tab),
            false => self.drop_tab_state(tab),
        }
        self.layout_mut().close(tab);
        if pane_gone {
            // Rebalance the remaining panes so they stay evenly spaced,
            // without reshaping the tree.
            self.layout_mut().balance();
        }
        self.after_tab_change();
        if let Some(companion) = self.host.commit_companions.remove(&tab) {
            self.close_tab_now(companion, Closing::Forget);
        }
        if let Some(repo) = commit_repo {
            self.focus_or_open_git_editor(repo);
        }
        // Extensions hear of the file closed at once.
        self.watch_for_extensions();
    }

    /// Forget everything kept about one tab: its terminal or its editor's
    /// buffer, and every per-tab record the rest of the app holds under its id.
    pub(in crate::window) fn drop_tab_state(&mut self, tab: TabId) {
        // A view of a buffer another tab still shows hands its work on to that
        // tab, and the buffer lives on. Otherwise the work would come back to
        // a tab that is gone, and the editor hears it is over.
        let other_view = self
            .open_tabs
            .views_of(&tab)
            .into_iter()
            .find(|view| *view != tab);
        match other_view {
            Some(other) => self.host.jobs.hand_over(tab, other),
            None => {
                self.host.jobs.cancel_for(tab);
                if let Some(slot) = self.open_tabs.editor_mut(&tab) {
                    slot.editor.on_closed();
                }
            }
        }
        self.host.commit_companions
            .retain(|_, companion| *companion != tab);
        self.hide_tab_state(tab);
        self.open_tabs.remove(&tab);
        self.tabs.labels.remove(&tab);
        self.retain_image_blocks(|img| img.tab_id != tab);
        self.host.compile.forget(tab);
        if self.host.token_inspect == Some(tab) {
            self.host.token_inspect = None;
        }
        // Rebuilt on the next tick in any case, but a tab given the id in
        // between would wear the spinner.
        self.busy.since.remove(&tab);
        self.busy.spinning.remove(&tab);
    }

    /// Every tab id a record of the window holds, with the record's name:
    /// what closing a tab has to clear. Each record kept per tab, wherever
    /// its module keeps it, is listed here, so a test can find one a close
    /// path left behind.
    #[cfg(test)]
    pub(in crate::window) fn held_tab_ids(&self) -> Vec<(&'static str, TabId)> {
        let mut held: Vec<(&'static str, TabId)> = Vec::new();
        let mut add = |record: &'static str, tabs: Vec<TabId>| held.extend(tabs.into_iter().map(|tab| (record, tab)));
        add("terminal", self.open_tabs.terminals().map(|(tab, _)| *tab).collect());
        add("editor", self.open_tabs.editors().map(|(tab, _)| *tab).collect());
        add("label", self.tabs.labels.keys().copied().collect());
        add("vim", self.vim.tabs.keys().copied().collect());
        add("commit message", self.host.commit_companions.keys().copied().collect());
        add("commit companion", self.host.commit_companions.values().copied().collect());
        add("build", self.host.compile.tabs());
        add("job", self.host.jobs.tabs());
        add("busy", self.busy.since.keys().chain(&self.busy.spinning).copied().collect());
        add("image block", self.term.image_blocks.iter().map(|block| block.tab_id).collect());
        add("swoop", self.term.swoop_initial_cursor.iter().map(|(tab, _)| *tab).collect());
        add("token inspector", self.host.token_inspect.into_iter().collect());
        add("editor cursor", self.editor_cursor.iter().map(|cursor| cursor.tab).collect());
        add("selection", self.selection.span.iter().map(|span| span.tab).collect());
        add("search", self.search.debounce.iter().map(|(_, tab)| *tab).collect());
        add("prompt", self.host.prompts.debounce.iter().map(|(_, tab)| *tab).collect());
        held
    }

    /// The tab ids [`Self::held_tab_ids`] holds for tabs that are gone: in
    /// no pane, not buried, and in no workspace put aside.
    #[cfg(test)]
    pub(in crate::window) fn stale_tab_ids(&self) -> Vec<(&'static str, TabId)> {
        let parked = self.parked_tabs();
        self.held_tab_ids()
            .into_iter()
            .filter(|(_, tab)| !self.layout().contains(*tab) && !self.buried_tabs().contains(tab) && !parked.contains(tab))
            .collect()
    }

    /// Bury the buffer of editor tab `tab`, which is leaving its pane: what
    /// a pane kept about the tab goes, its buffer, jobs, and name stay.
    pub(in crate::window) fn bury_tab(&mut self, tab: TabId) {
        self.hide_tab_state(tab);
        self.open_tabs.bury(tab);
    }

    /// Forget what is kept about a tab while a pane shows it, leaving its
    /// buffer, its jobs, and its name.
    fn hide_tab_state(&mut self, tab: TabId) {
        self.drop_overlay_of(tab);
        if self
            .search
            .debounce
            .as_ref()
            .is_some_and(|(_, t)| *t == tab)
        {
            self.search.debounce = None;
        }
        if self.host.prompts.debounce.is_some_and(|(_, t)| t == tab) {
            self.host.prompts.debounce = None;
        }
        if self.term.swoop_initial_cursor.is_some_and(|(t, _)| t == tab) {
            self.term.swoop_initial_cursor = None;
        }
        if self.editor_cursor.as_ref().is_some_and(|c| c.tab == tab) {
            self.stop_editor_cursor();
        }
        self.vim.tabs.remove(&tab);
        if self.selection.span.as_ref().is_some_and(|s| s.tab == tab) {
            self.selection.span = None;
        }
    }

    // --------------------------------------------------------------------
    // Mux
    // --------------------------------------------------------------------

    /// Spawn a named session on the mux server at `path`, running
    /// `command` (or the default shell) in `cwd`, then open a tab attached
    /// to it. The spawn is confirmed with a bounded wait (the server answers
    /// on its poll cycle) before attaching, so failures surface as notices
    /// instead of a tab pointing at a session that never existed.
    pub(in crate::window) fn spawn_mux_tab_at(
        &mut self,
        path: &str,
        name: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        focused: TabId,
    ) {
        let mut client = match crate::mux::client::MuxClient::connect(path) {
            Ok(client) => client,
            Err(_) => {
                self.set_error("could not connect to mux daemon");
                return;
            }
        };
        // Size the session to the focused pane so the first frame fits.
        let (cols, rows) = self
            .open_tabs
            .terminal(&focused)
            .map(|terminal| {
                (
                    terminal.grid().cols().max(1) as u16,
                    terminal.grid().rows().max(1) as u16,
                )
            })
            .unwrap_or((80, 24));
        match client.spawn_confirmed(
            name,
            cols,
            rows,
            cwd,
            command,
            std::time::Duration::from_millis(200),
        ) {
            Ok((cols, rows)) => {
                self.new_mux_tab_at(path, name);
                let note = match command {
                    Some(command) => format!("started '{name}' ({cols}x{rows}): {command}"),
                    None => format!("started '{name}' ({cols}x{rows})"),
                };
                self.set_notice(note);
            }
            Err(message) => self.set_error(format!("mux: {message}")),
        }
    }

    // --------------------------------------------------------------------
    // Relayout
    // --------------------------------------------------------------------

    /// Re-lay-out panes after a change to the reserved top-tabbar or status-bar
    /// rows (menu style, status-bar visibility), and request a redraw.
    pub(in crate::window) fn relayout_tabbar(&mut self) {
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Bring everything that depends on which tabs are shown up to date after
    /// one was opened, shown, or closed.
    pub(in crate::window) fn after_tab_change(&mut self) {
        self.retire_launchers();
        self.selection.span = self
            .selection
            .span
            .take()
            .filter(|s| self.layout().shown_tabs().contains(&s.tab));
        self.close_menu();
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::test_support::*;
    use std::time::{Duration, Instant};

    #[test]
    fn test_a_closed_tab_leaves_nothing_kept_about_it_behind() {
        // What the window keeps per tab sits in maps of its own, each owned
        // by the module that uses it; one left out of the prune on close
        // grows with every tab opened and closed, and a later tab given the
        // same id would inherit it.
        let mut app = App::new();
        let dir = std::env::temp_dir();
        let tab = app.show_editor(
            crate::window::editor::EditorKind::Dir,
            Box::new(crate::editors::dir::DirEditor::new(dir.clone())),
        );
        let message = app.show_editor(
            crate::window::editor::EditorKind::Dir,
            Box::new(crate::editors::dir::DirEditor::new(dir)),
        );
        app.tabs.set_name(tab, Some("named".to_string()));
        app.vim.tab_mut(tab);
        app.host.compile.newest = Some(tab);
        app.host.token_inspect = Some(tab);
        app.host.commit_companions.insert(message, tab);

        app.drop_tab_state(tab);

        assert!(!app.open_tabs.is_editor(&tab), "the tab");
        assert_eq!(app.tabs.name(tab), None, "its name");
        assert!(!app.vim.tabs.contains_key(&tab), "its Vim state");
        assert_eq!(app.host.compile.newest, None, "its build");
        assert_eq!(app.host.token_inspect, None, "the inspector on it");
        assert!(app.host.commit_companions.is_empty(), "the commit it went with");
    }

    /// Give `tab` something in every per-tab record a test can fill without
    /// a GPU: what each close path must clear.
    fn mark_everything(app: &mut App, tab: TabId) {
        app.tabs.set_name(tab, Some("named".to_string()));
        app.vim.tab_mut(tab);
        app.host.compile.newest = Some(tab);
        app.host.token_inspect = Some(tab);
        app.busy.since.insert(tab, Instant::now());
        app.busy.spinning.insert(tab);
        app.term.swoop_initial_cursor = Some((tab, (0, 0)));
        app.search.debounce = Some((Instant::now(), tab));
        app.host.prompts.debounce = Some((Instant::now(), tab));
        app.term.image_blocks.push(crate::window::ImageBlock {
            abs_row: 0,
            block_index: 0,
            closed: false,
            content: crate::model::picture::Content::Raster(std::sync::Arc::from(&[][..])),
            drawn: None,
            id: tab.0,
            max_rows: 1,
            natural: None,
            segment_index: 0,
            tab_id: tab,
        });
        app.host.jobs.spawn(tab, crate::model::editor::JobRequest::Command(crate::model::editor::CommandRequest {
            args: vec!["5".to_string()],
            cwd: std::env::temp_dir(),
            env: Vec::new(),
            id: 0,
            program: "sleep".to_string(),
            stdin: None,
            tag: "test",
        }));
    }

    #[test]
    fn test_no_close_path_leaves_a_record_naming_a_tab_that_is_gone() {
        // Each record kept per tab sits in a map of its own module's, and the
        // close paths had drifted: one forgot the commit companions, another
        // the jobs. Every path is run here over tabs that hold something in
        // every record, and nothing may still name a tab that is gone.
        let mut app = App::new();
        let first = app.layout().focused();
        app.open_tabs.insert_terminal(first, idle_terminal());
        let mut opened = Vec::new();
        for _ in 0..5 {
            app.split_pane_with_terminal(crate::model::layout::Direction::Vertical);
            opened.push(app.layout().focused());
        }
        for tab in std::iter::once(first).chain(opened.iter().copied()) {
            mark_everything(&mut app, tab);
        }
        app.host.commit_companions.insert(opened[0], opened[1]);

        app.close_tab_now(opened[0], Closing::Forget);
        assert_eq!(app.stale_tab_ids(), [], "closing a tab");
        app.close_pane_now(opened[2]);
        assert_eq!(app.stale_tab_ids(), [], "closing a pane");
        app.close_tab_now(opened[3], Closing::Keep);
        assert_eq!(app.stale_tab_ids(), [], "closing a tab to keep");
        app.close_other_panes_now(first);
        assert_eq!(app.stale_tab_ids(), [], "closing the other panes");
        app.host.jobs.cancel_for(first);
    }

    #[test]
    fn test_revealing_a_terminal_in_a_pane_without_one_opens_one() {
        // A pane holding only editors used to answer with a notice, which left
        // the key doing nothing where a shell was most wanted.
        let mut app = App::new();
        let editor = app.show_editor(
            crate::window::editor::EditorKind::Dir,
            Box::new(crate::editors::dir::DirEditor::new(std::env::temp_dir())),
        );
        assert!(!app.open_tabs.is_terminal(&app.layout().focused()));

        app.reveal_recent_terminal();
        let shown = app.layout().focused();
        assert_ne!(shown, editor);
        assert!(
            app.open_tabs.is_terminal(&shown),
            "a new terminal is showing"
        );
        assert!(
            app.layout().tabs_in_pane(shown).contains(&editor),
            "in the same pane as the editor"
        );
    }

    #[test]
    fn test_closing_a_tab_forgets_everything_kept_under_its_id() {
        // A restore gives the id back out, and whatever was left under it
        // would belong to the tab that comes back.
        let mut app = App::new();
        let tab = app.layout().focused();
        app.vim.tab_mut(tab).marks.insert('a', (1, 0));
        app.vim.tab_mut(tab).jump_list.push((2, 0));
        app.tabs.set_name(tab, Some("work".to_string()));
        app.tabs.set_program_title(tab, "vim".to_string());

        app.drop_tab_state(tab);

        assert!(app.vim.tab(tab).is_none());
        assert_eq!(app.tabs.name(tab), None);
        assert_eq!(app.tabs.program_title(tab), None);
    }

    #[test]
    fn test_an_empty_name_puts_the_tab_back_to_its_own_title() {
        let mut state = TabsState::default();
        let tab = TabId(3);
        state.set_name(tab, Some("work".to_string()));
        state.set_name(tab, Some(String::new()));
        assert_eq!(state.name(tab), None);
    }

    #[test]
    fn test_new_mux_tab_attaches_and_replays() {
        use crate::mux::protocol::{self, ServerMessage};
        use std::io::{Read, Write};
        #[cfg(unix)]
        use std::os::unix::net::UnixListener;
        #[cfg(windows)]
        use uds_windows::UnixListener;

        fn read_frame(conn: &mut impl Read) -> Vec<u8> {
            let mut len_bytes = [0u8; 4];
            conn.read_exact(&mut len_bytes).unwrap();
            let mut framed = len_bytes.to_vec();
            let mut body = vec![0u8; u32::from_be_bytes(len_bytes) as usize];
            conn.read_exact(&mut body).unwrap();
            framed.extend(body);
            framed
        }

        let path =
            std::env::temp_dir().join(format!("winter-app-mux-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let socket_path = path.to_string_lossy().to_string();

        let server = std::thread::spawn(move || {
            let (mut conn, _) = listener.accept().unwrap();
            let _attach = read_frame(&mut conn); // pane's Attach
            for frame in [
                protocol::encode(&ServerMessage::Attached {
                    session: "work".into(),
                    cols: 80,
                    rows: 24,
                }),
                protocol::encode(&ServerMessage::Output {
                    session: "work".into(),
                    bytes: b"mux tab live\n".to_vec(),
                }),
            ] {
                conn.write_all(&frame).unwrap();
            }
            // Consume the attach-time Resize, then close.
            let _ = read_frame(&mut conn);
        });

        let mut app = App::new();
        app.new_mux_tab_at(&socket_path, "work");
        // The bootstrap tab plus the new mux tab, side by side in one pane.
        assert_eq!(app.layout().members().len(), 2);
        let focused = app.layout().focused();
        assert_eq!(
            app.open_tabs.terminal(&focused).unwrap().mux_session(),
            Some("work")
        );

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut text = String::new();
        while Instant::now() < deadline {
            if let Some(terminal) = app.open_tabs.terminal_mut(&focused) {
                terminal.drain_output();
            }
            text = app.open_tabs.terminal(&focused).unwrap().grid().to_text();
            if text.contains("mux tab live") {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            text.contains("mux tab live"),
            "attached pane must render session output, got: {text}"
        );
        assert_eq!(app.tab_title(focused), "mux: work");

        // Detaching closes the pane's tab view; the session itself lives
        // on the server, out of the app's hands.
        app.run_command("mux_detach_session", focused);
        assert_eq!(
            app.layout().members().len(),
            1,
            "the mux tab must be closed"
        );
        assert!(!app.open_tabs.is_terminal(&focused));
        assert!(app.chrome.notice_text().is_some());

        let _ = server.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn test_closing_the_shown_tab_shows_the_one_shown_before_it() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        app.close_tab(TabId(2));
        assert_eq!(app.layout().members(), vec![TabId(0), TabId(1)]);
        assert_eq!(app.layout().focused(), TabId(0));
    }

    #[test]
    fn test_switch_tab_shows_the_tab_at_that_strip_position() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        assert_eq!(app.layout().focused(), TabId(2));
        assert_eq!(
            app.layout().recent_in_pane(TabId(2)),
            vec![TabId(2), TabId(0), TabId(1)]
        );
        assert!(app.tabs.recent_walk.is_none());
    }

    #[test]
    fn test_recent_tab_walks_backward_and_forward_without_reshuffling() {
        let mut app = app_with_tabs(3);
        app.switch_tab(2);
        app.switch_tab(1); // Recency now [1, 2, 0], showing 1.

        // Backward steps toward less recently shown, holding the order still.
        app.recent_tab(false);
        assert_eq!(app.layout().focused(), TabId(2));
        app.recent_tab(false);
        assert_eq!(app.layout().focused(), TabId(0));

        // Forward steps back toward more recently shown.
        app.recent_tab(true);
        assert_eq!(app.layout().focused(), TabId(2));
        let walk = app.tabs.recent_walk.as_ref().unwrap();
        assert_eq!(walk.order, vec![TabId(1), TabId(2), TabId(0)]);
    }

    #[test]
    fn test_deliberate_switch_ends_the_recency_walk() {
        let mut app = app_with_tabs(3);
        app.switch_tab(1);
        app.recent_tab(false);
        assert!(app.tabs.recent_walk.is_some());
        app.switch_tab(2);
        assert!(app.tabs.recent_walk.is_none());
        assert_eq!(app.layout().recent_in_pane(TabId(2))[0], TabId(2));
    }

    #[test]
    fn test_recent_tab_is_noop_with_one_tab() {
        let mut app = App::new();
        let only = app.layout().focused();
        app.recent_tab(false);
        app.recent_tab(true);
        assert_eq!(app.layout().focused(), only);
        assert!(app.tabs.recent_walk.is_none());
    }

    #[test]
    fn test_switch_to_pane_shows_a_hidden_tab() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.new_tab();
        let second = app.layout().focused();
        assert_ne!(first, second);
        assert_eq!(
            app.layout().shown_tabs(),
            vec![second],
            "one pane, showing the new tab"
        );

        app.switch_to_tab(first);
        assert_eq!(app.layout().focused(), first);
        assert_eq!(app.layout().shown_tabs(), vec![first]);
    }

    #[test]
    fn test_cycle_tab_does_not_wrap_around() {
        let mut app = app_with_tabs(3);
        // app_with_tabs creates tabs [0, 1, 2] in one pane, with focused = TabId(0).
        assert_eq!(app.layout().focused(), TabId(0));

        // Stepping backward at the first tab does not wrap around.
        app.cycle_tab(false);
        assert_eq!(app.layout().focused(), TabId(0));

        // Stepping forward advances through tabs.
        app.cycle_tab(true);
        assert_eq!(app.layout().focused(), TabId(1));
        app.cycle_tab(true);
        assert_eq!(app.layout().focused(), TabId(2));

        // Stepping forward at the last tab does not wrap around.
        app.cycle_tab(true);
        assert_eq!(app.layout().focused(), TabId(2));

        // Stepping backward works again.
        app.cycle_tab(false);
        assert_eq!(app.layout().focused(), TabId(1));
    }
}

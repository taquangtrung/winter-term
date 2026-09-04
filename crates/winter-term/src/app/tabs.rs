//! Tab creation, activation order, and the most-recently-used ring.

use crate::model::layout::{PaneId, Tab};
use crate::model::mode::Mode;
use crate::terminal::pane::Pane;

use super::App;
use super::{DEFAULT_COLS, DEFAULT_ROWS};

// ========================================================================
// App: tabs
// ========================================================================

impl App {
    /// The currently visible tab.
    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs[self.active_tab]
    }
    /// The currently visible tab, mutably.
    pub(crate) fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active_tab]
    }
    /// Open a new tab with a fresh shell pane and switch to it.
    pub(crate) fn new_tab(&mut self) {
        let id = self.alloc_pane_id();
        // Open the new tab in the focused pane's working directory rather than
        // the process default (usually `$HOME`).
        let cwd = self.focused_cwd();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        // Sized roughly now; resize_all_panes fixes the exact grid once placed.
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let Some(pane) = self.spawn_pane_or_notify(
            cols.max(1),
            rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            return;
        };
        self.push_new_tab(id, pane);
    }
    /// Open a new foreground tab whose pane is attached to a running mux
    /// session; the session's buffered output replays into the pane.
    pub(crate) fn new_mux_tab(&mut self, session: &str) {
        self.new_mux_tab_at(&crate::mux::server::default_socket_path(), session);
    }
    /// Open a new foreground tab attached to a session on the mux server
    /// at `path`.
    pub(crate) fn new_mux_tab_at(&mut self, path: &str, session: &str) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Pane::new_mux_at(path, cols.max(1), rows.max(1), session, scrollback) {
            Ok(pane) => {
                self.set_notice(format!("attached to mux session '{session}'"));
                self.push_new_tab(id, pane);
            }
            Err(e) => self.set_error(format!(
                "could not attach to '{session}' ({e}); start the server with 'winter mux serve'"
            )),
        }
    }
    /// Open a new foreground tab attached to a session on a mux server
    /// reached over ssh at `host`.
    pub(crate) fn new_mux_tab_remote_at(&mut self, host: &str, session: &str) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self
            .renderer
            .as_ref()
            .map(|r| r.grid_size())
            .unwrap_or((DEFAULT_COLS as usize, DEFAULT_ROWS as usize));
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        match Pane::new_mux_remote(host, cols.max(1), rows.max(1), session, scrollback) {
            Ok(pane) => {
                self.set_notice(format!("attached to '{host}:{session}'"));
                self.push_new_tab(id, pane);
            }
            Err(e) => self.set_error(format!("could not reach '{host}' over ssh ({e})")),
        }
    }
    /// Install an already-spawned pane as a fresh foreground tab and switch to
    /// it. Shared by `new_tab`'s shell tab and `gx`'s editor tab so both get
    /// the same bookkeeping — mode default, MRU touch, tile repositioning,
    /// resize, and title update.
    pub(crate) fn push_new_tab(&mut self, id: PaneId, pane: Pane) {
        self.panes.insert(id, pane);
        self.modes.insert(id, Mode::default());
        self.tabs.push(Tab::with_root(id));
        self.active_tab = self.tabs.len() - 1;
        self.touch_mru(self.active_tab);
        self.close_menu();
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }
    /// Switch the visible tab to `index` as a deliberate selection: record it as
    /// most-recently-used (ending any recency walk) and show it.
    pub(crate) fn switch_tab(&mut self, index: usize) {
        if index >= self.tabs.len() || index == self.active_tab {
            return;
        }
        self.touch_mru(index);
        self.activate_tab(index);
    }
    /// Make `index` the visible tab without touching the MRU order. Shared by
    /// deliberate switches ([`Self::switch_tab`]) and recency walks
    /// ([`Self::recent_tab`]).
    pub(crate) fn activate_tab(&mut self, index: usize) {
        self.active_tab = index;
        self.selection = None;
        // Force a tile reposition so background-tab WebViews are hidden and the
        // new tab's are shown (the layout key alone may not have changed).
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }
    /// Move `index` to the front of the most-recently-used order (inserting it if
    /// new) and end any in-progress recency walk.
    pub(crate) fn touch_mru(&mut self, index: usize) {
        self.tab_mru.retain(|&i| i != index);
        self.tab_mru.insert(0, index);
        self.mru_walk = None;
    }
    /// Cycle to the next (`forward`) or previous tab by position, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        let next = if forward {
            (self.active_tab + 1) % count
        } else {
            (self.active_tab + count - 1) % count
        };
        self.switch_tab(next);
    }
    /// Switch tabs in most-recently-used order: `forward` steps toward more
    /// recently used, otherwise toward less recently used, wrapping around. The
    /// MRU order is held still across consecutive calls (a "walk") so the user can
    /// step back and forth through usage history; the next deliberate switch ends
    /// the walk and re-seeds the order from the chosen tab.
    pub(crate) fn recent_tab(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count <= 1 {
            return;
        }
        // Guard against any drift from tab open/close bookkeeping: a malformed
        // order is rebuilt with the current tab most-recent.
        if self.tab_mru.len() != count {
            self.tab_mru = (0..count).collect();
            self.touch_mru(self.active_tab);
        }
        let cursor = self.mru_walk.unwrap_or(0);
        let next = if forward {
            (cursor + count - 1) % count
        } else {
            (cursor + 1) % count
        };
        self.mru_walk = Some(next);
        self.activate_tab(self.tab_mru[next]);
    }
    /// Close tab `index`, dropping all its panes. The last tab is never closed.
    pub(crate) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        if self.tabs.len() <= 1 {
            self.exit_requested = true;
            return;
        }
        for id in self.tabs[index].panes() {
            self.panes.remove(&id);
            self.modes.remove(&id);
            self.nav_cursors.remove(&id);
            self.pane_titles.remove(&id);
            self.webview_mgr.remove_tiles_for_pane(id);
            self.image_blocks.retain(|img| img.pane_id != id);
        }
        self.tabs.remove(index);
        if index < self.active_tab {
            self.active_tab -= 1;
        }
        self.active_tab = self.active_tab.min(self.tabs.len() - 1);
        // Drop the closed tab from the MRU order and shift the indices above it
        // down, then re-seed the current tab as most-recent.
        self.tab_mru.retain(|&i| i != index);
        for i in self.tab_mru.iter_mut() {
            if *i > index {
                *i -= 1;
            }
        }
        // Shift custom tab names: remove the closed tab's name, shift those above it.
        self.tab_names = self
            .tab_names
            .iter()
            .filter_map(|(&i, name)| match i.cmp(&index) {
                std::cmp::Ordering::Less => Some((i, name.clone())),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some((i - 1, name.clone())),
            })
            .collect();
        self.touch_mru(self.active_tab);
        self.close_menu();
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }
    /// Swap tabs at positions `a` and `b`, keeping the active tab index pointing
    /// to the same content, and updating the MRU order and custom names.
    pub(crate) fn swap_tabs(&mut self, a: usize, b: usize) {
        if a == b || a >= self.tabs.len() || b >= self.tabs.len() {
            return;
        }
        self.tabs.swap(a, b);
        if self.active_tab == a {
            self.active_tab = b;
        } else if self.active_tab == b {
            self.active_tab = a;
        }
        for idx in &mut self.tab_mru {
            if *idx == a {
                *idx = b;
            } else if *idx == b {
                *idx = a;
            }
        }
        let a_name = self.tab_names.remove(&a);
        let b_name = self.tab_names.remove(&b);
        if let Some(n) = a_name {
            self.tab_names.insert(b, n);
        }
        if let Some(n) = b_name {
            self.tab_names.insert(a, n);
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }
    /// Spawn a named session on the mux server at `path` — running
    /// `command` (or the default shell) in `cwd` — then open a tab attached
    /// to it. The spawn is confirmed with a bounded wait (the server answers
    /// on its poll cycle) before attaching, so failures surface as notices
    /// instead of a tab pointing at a session that never existed.
    pub(crate) fn spawn_mux_tab_at(
        &mut self,
        path: &str,
        name: &str,
        command: Option<&str>,
        cwd: Option<&str>,
        focused: PaneId,
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
            .panes
            .get(&focused)
            .map(|pane| {
                (
                    pane.grid().cols().max(1) as u16,
                    pane.grid().rows().max(1) as u16,
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
    /// Re-lay-out panes after a change to the reserved top-tabbar or status-bar
    /// rows (menu style, status-bar visibility), and request a redraw.
    pub(crate) fn relayout_tabbar(&mut self) {
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }
}

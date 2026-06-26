//! Tab creation, activation order, and the most-recently-used ring.

use std::collections::HashMap;

use crate::model::layout::{PaneId, Tab};
use crate::model::mode::Mode;
use crate::terminal::pane::Pane;

use super::App;
use super::{DEFAULT_COLS, DEFAULT_ROWS};
use winter_render::TabbarHit;

// ========================================================================
// Data Structures
// ========================================================================

/// Every open tab, which one is active, and the state of the tab bar drawn
/// across the top of them: hover, in-progress drag, and the rename prompt.
///
/// The most-recently-used ring is what `Ctrl-Tab` walks, so it is ordered by
/// visit rather than by position.
pub(crate) struct TabsState {
    /// Which tab is shown; index into [`Self::tabs`].
    pub(crate) active: usize,
    /// All open tabs, each its own split-tree of panes.
    pub(crate) all: Vec<Tab>,
    /// Source tab index and the pointer x position when a tab drag began. `None`
    /// when no drag is in progress. Cleared on mouse release.
    pub(crate) drag_start: Option<(usize, f32)>,
    /// Tabbar element currently under the cursor; drives hover highlights.
    pub(crate) hover: TabbarHit,
    pub(crate) hover_pos: Option<(f32, f32)>,
    /// Tab indices in most-recently-used order (front = the current tab after a
    /// deliberate switch). Drives the recency tab commands.
    pub(crate) mru: Vec<usize>,
    /// Cursor into [`Self::tab_mru`] while a recency walk is in progress, so
    /// repeated recency commands step through usage order without reshuffling it.
    /// `None` once a deliberate switch ends the walk.
    pub(crate) mru_walk: Option<usize>,
    /// User-set custom names for tabs, keyed by tab index. Take priority over
    /// OSC-set titles. Indices are shifted down when a tab before them is closed.
    pub(crate) names: HashMap<usize, String>,
    /// In-progress tab rename input, set while the user is typing a new name.
    pub(crate) rename_input: Option<String>,
}

impl Default for TabsState {
    /// A fresh window: one empty tab, active, and alone in the MRU ring.
    fn default() -> Self {
        Self {
            active: 0,
            all: vec![Tab::new()],
            drag_start: None,
            hover: TabbarHit::None,
            hover_pos: None,
            mru: vec![0],
            mru_walk: None,
            names: HashMap::new(),
            rename_input: None,
        }
    }
}

// ========================================================================
// App: tabs
// ========================================================================

impl App {
    /// The currently visible tab.
    pub(crate) fn tab(&self) -> &Tab {
        &self.tabs.all[self.tabs.active]
    }
    /// The currently visible tab, mutably.
    pub(crate) fn tab_mut(&mut self) -> &mut Tab {
        &mut self.tabs.all[self.tabs.active]
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
    /// the same bookkeeping: mode default, MRU touch, tile repositioning,
    /// resize, and title update.
    pub(crate) fn push_new_tab(&mut self, id: PaneId, pane: Pane) {
        self.panes.insert(id, pane);
        self.modes.insert(id, Mode::default());
        self.tabs.all.push(Tab::with_root(id));
        self.tabs.active = self.tabs.all.len() - 1;
        self.touch_mru(self.tabs.active);
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
        if index >= self.tabs.all.len() || index == self.tabs.active {
            return;
        }
        self.touch_mru(index);
        self.activate_tab(index);
    }
    /// Make `index` the visible tab without touching the MRU order. Shared by
    /// deliberate switches ([`Self::switch_tab`]) and recency walks
    /// ([`Self::recent_tab`]).
    pub(crate) fn activate_tab(&mut self, index: usize) {
        self.tabs.active = index;
        self.selection.span = None;
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
        self.tabs.mru.retain(|&i| i != index);
        self.tabs.mru.insert(0, index);
        self.tabs.mru_walk = None;
    }
    /// Cycle to the next (`forward`) or previous tab by position, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        let count = self.tabs.all.len();
        if count <= 1 {
            return;
        }
        let next = if forward {
            (self.tabs.active + 1) % count
        } else {
            (self.tabs.active + count - 1) % count
        };
        self.switch_tab(next);
    }
    /// Switch tabs in most-recently-used order: `forward` steps toward more
    /// recently used, otherwise toward less recently used, wrapping around. The
    /// MRU order is held still across consecutive calls (a "walk") so the user can
    /// step back and forth through usage history; the next deliberate switch ends
    /// the walk and re-seeds the order from the chosen tab.
    pub(crate) fn recent_tab(&mut self, forward: bool) {
        let count = self.tabs.all.len();
        if count <= 1 {
            return;
        }
        // Guard against any drift from tab open/close bookkeeping: a malformed
        // order is rebuilt with the current tab most-recent.
        if self.tabs.mru.len() != count {
            self.tabs.mru = (0..count).collect();
            self.touch_mru(self.tabs.active);
        }
        let cursor = self.tabs.mru_walk.unwrap_or(0);
        let next = if forward {
            (cursor + count - 1) % count
        } else {
            (cursor + 1) % count
        };
        self.tabs.mru_walk = Some(next);
        self.activate_tab(self.tabs.mru[next]);
    }
    /// Close tab `index`, dropping all its panes. The last tab is never closed.
    pub(crate) fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.all.len() {
            return;
        }
        if self.tabs.all.len() <= 1 {
            self.exit_requested = true;
            return;
        }
        for id in self.tabs.all[index].panes() {
            self.panes.remove(&id);
            self.modes.remove(&id);
            self.nav_cursors.remove(&id);
            self.pane_titles.remove(&id);
            self.webview_mgr.remove_tiles_for_pane(id);
            self.image_blocks.retain(|img| img.pane_id != id);
        }
        self.tabs.all.remove(index);
        if index < self.tabs.active {
            self.tabs.active -= 1;
        }
        self.tabs.active = self.tabs.active.min(self.tabs.all.len() - 1);
        // Drop the closed tab from the MRU order and shift the indices above it
        // down, then re-seed the current tab as most-recent.
        self.tabs.mru.retain(|&i| i != index);
        for i in self.tabs.mru.iter_mut() {
            if *i > index {
                *i -= 1;
            }
        }
        // Shift custom tab names: remove the closed tab's name, shift those above it.
        self.tabs.names = self
            .tabs
            .names
            .iter()
            .filter_map(|(&i, name)| match i.cmp(&index) {
                std::cmp::Ordering::Less => Some((i, name.clone())),
                std::cmp::Ordering::Equal => None,
                std::cmp::Ordering::Greater => Some((i - 1, name.clone())),
            })
            .collect();
        self.touch_mru(self.tabs.active);
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
        if a == b || a >= self.tabs.all.len() || b >= self.tabs.all.len() {
            return;
        }
        self.tabs.all.swap(a, b);
        if self.tabs.active == a {
            self.tabs.active = b;
        } else if self.tabs.active == b {
            self.tabs.active = a;
        }
        for idx in &mut self.tabs.mru {
            if *idx == a {
                *idx = b;
            } else if *idx == b {
                *idx = a;
            }
        }
        let a_name = self.tabs.names.remove(&a);
        let b_name = self.tabs.names.remove(&b);
        if let Some(n) = a_name {
            self.tabs.names.insert(b, n);
        }
        if let Some(n) = b_name {
            self.tabs.names.insert(a, n);
        }
        self.last_tile_layout = None;
        self.dirty = true;
    }
    /// Spawn a named session on the mux server at `path`, running
    /// `command` (or the default shell) in `cwd`, then open a tab attached
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

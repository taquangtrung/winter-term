//! Tabs within a pane: opening, showing, cycling, reordering, and closing them,
//! and the recency walk across the focused group's strip.

use std::collections::HashMap;

use crate::model::layout::{Layout, PaneId};
use crate::model::mode::Mode;
use crate::terminal::pane::Pane;

use super::page::Closing;
use super::strip::StripHit;
use super::App;
use super::{DEFAULT_COLS, DEFAULT_ROWS};
use winter_render::TabbarHit;

// ========================================================================
// Data Structures
// ========================================================================

/// The window's layout of tab groups and the state of the strips drawn over
/// them: hover, an in-progress drag, the rename prompt, and a recency walk.
pub(crate) struct TabsState {
    /// The tab being dragged along its strip and the pointer x position when
    /// the drag began. `None` when no drag is in progress. Cleared on mouse
    /// release.
    pub(crate) drag_start: Option<(PaneId, f32)>,
    /// Title-bar element currently under the cursor; drives hover highlights.
    pub(crate) hover: TabbarHit,
    pub(crate) hover_pos: Option<(f32, f32)>,
    /// The split tree of tab groups.
    pub(crate) layout: Layout,
    /// User-set custom names for tabs. Take priority over OSC-set titles.
    pub(crate) names: HashMap<PaneId, String>,
    /// The focused group's recency order, frozen while the recency commands
    /// step through it, so repeated steps walk usage history instead of
    /// reshuffling it. `None` once a deliberate switch ends the walk.
    pub(crate) recent_walk: Option<RecentWalk>,
    /// In-progress tab rename input, set while the user is typing a new name
    /// for the focused tab.
    pub(crate) rename_input: Option<String>,
    /// Pane-strip element currently under the cursor; drives hover highlights.
    pub(crate) strip_hover: StripHit,
}

/// A recency walk in progress: the order it walks and where it has got to.
pub(crate) struct RecentWalk {
    pub(crate) at: usize,
    pub(crate) order: Vec<PaneId>,
}

impl Default for TabsState {
    /// A fresh window: one group holding one tab.
    fn default() -> Self {
        Self {
            drag_start: None,
            hover: TabbarHit::None,
            hover_pos: None,
            layout: Layout::new(),
            names: HashMap::new(),
            recent_walk: None,
            rename_input: None,
            strip_hover: StripHit::None,
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
    pub(crate) fn layout(&self) -> &Layout {
        &self.tabs.layout
    }

    /// The window's layout, mutably.
    pub(crate) fn layout_mut(&mut self) -> &mut Layout {
        &mut self.tabs.layout
    }

    // --------------------------------------------------------------------
    // Opening
    // --------------------------------------------------------------------

    /// Open a new terminal tab beside the focused one and show it.
    pub(crate) fn new_tab(&mut self) {
        let id = self.alloc_pane_id();
        // Open the new tab where the focused tab was being used rather than
        // at the process default (usually `$HOME`): beside a tool that is the
        // directory the tool is looking at, not a shell's own.
        let cwd = self.focused_start_cwd();
        let (cols, rows) = self.focused_grid_size();
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

    /// Open a new tab whose terminal is attached to a running mux session;
    /// the session's buffered output replays into it.
    pub(crate) fn new_mux_tab(&mut self, session: &str) {
        self.new_mux_tab_at(&crate::mux::server::default_socket_path(), session);
    }

    /// Open a new tab attached to a session on the mux server at `path`.
    pub(crate) fn new_mux_tab_at(&mut self, path: &str, session: &str) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self.focused_grid_size();
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

    /// Open a new tab attached to a session on a mux server reached over ssh
    /// at `host`.
    pub(crate) fn new_mux_tab_remote_at(&mut self, host: &str, session: &str) {
        let id = self.alloc_pane_id();
        let (cols, rows) = self.focused_grid_size();
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

    /// Install an already-spawned terminal as a new tab beside the focused
    /// one and show it. Shared by every way a terminal tab is opened so all of
    /// them get the same bookkeeping: mode default, tile repositioning,
    /// resize, and title update.
    pub(crate) fn push_new_tab(&mut self, id: PaneId, pane: Pane) {
        self.panes.insert(id, pane);
        self.modes.insert(id, Mode::default());
        self.layout_mut().add_tab(id);
        self.after_tab_change();
    }

    /// The grid a new tab in the focused group starts at, before
    /// [`Self::resize_all_panes`] fixes the exact size once it is placed.
    fn focused_grid_size(&self) -> (usize, usize) {
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
    pub(crate) fn show_tab(&mut self, pane: PaneId) {
        if !self.layout_mut().focus(pane) {
            return;
        }
        self.tabs.recent_walk = None;
        self.after_tab_change();
    }

    /// Show the tab at strip position `index` (0-based) of the focused group.
    pub(crate) fn switch_tab(&mut self, index: usize) {
        let members = self.layout().group_members(self.layout().focused());
        if let Some(&pane) = members.get(index) {
            self.show_tab(pane);
        }
    }

    /// Show the next (`forward`) or previous tab of the focused group by
    /// strip position, wrapping around.
    pub(crate) fn cycle_tab(&mut self, forward: bool) {
        if self.layout_mut().cycle_tab(forward) {
            self.tabs.recent_walk = None;
            self.after_tab_change();
        }
    }

    /// Step through the focused group's tabs in most-recently-shown order:
    /// `forward` steps toward more recently shown, otherwise toward less,
    /// wrapping around. The order is held still across consecutive calls (a
    /// "walk") so the user can step back and forth through usage history; the
    /// next deliberate switch ends the walk.
    pub(crate) fn recent_tab(&mut self, forward: bool) {
        let focused = self.layout().focused();
        let walk = match self.tabs.recent_walk.take() {
            Some(walk) if walk.order.contains(&focused) => walk,
            _ => RecentWalk {
                at: 0,
                order: self.layout().recent_in_group(focused),
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
        let pane = walk.order[at];
        self.layout_mut().focus(pane);
        self.tabs.recent_walk = Some(RecentWalk {
            at,
            order: walk.order,
        });
        self.after_tab_change();
    }

    /// Move the focused tab one place along its strip.
    pub(crate) fn move_tab(&mut self, forward: bool) {
        if self.layout_mut().move_tab(forward) {
            self.dirty = true;
        }
    }

    /// Move `pane` to where `target` sits on their shared strip.
    pub(crate) fn move_tab_to(&mut self, pane: PaneId, target: PaneId) {
        if self.layout_mut().move_tab_to(pane, target) {
            self.dirty = true;
        }
    }

    // --------------------------------------------------------------------
    // Closing
    // --------------------------------------------------------------------

    /// Close one tab. Closing the last tab of the window asks for the app to
    /// exit instead.
    ///
    /// Unwritten edits in a tool tab are asked about first, and the tab
    /// closes on the answer rather than here.
    pub(crate) fn close_tab(&mut self, pane: PaneId) {
        if self.ask_before_closing_tab(pane) {
            return;
        }
        self.close_tab_now(pane, Closing::Keep);
    }

    /// Close every other tab in the pane holding `keep`, leaving that tab.
    pub(crate) fn close_other_tabs(&mut self, keep: PaneId) {
        if self.ask_before_closing_other_tabs(keep) {
            return;
        }
        self.close_other_tabs_now(keep);
    }

    /// The same, once the question has been answered or there was none.
    pub(crate) fn close_other_tabs_now(&mut self, keep: PaneId) {
        let others: Vec<PaneId> = self
            .layout()
            .group_members(keep)
            .into_iter()
            .filter(|tab| *tab != keep)
            .collect();
        for tab in others {
            self.close_tab_now(tab, Closing::Keep);
        }
    }

    /// Close one tab without asking, keeping what it held for reopening or
    /// not. A group left with no tabs merges back into its sibling.
    pub(crate) fn close_tab_now(&mut self, pane: PaneId, closing: Closing) {
        if !self.layout().contains(pane) {
            return;
        }
        if self.layout().members().len() <= 1 {
            self.exit_requested = true;
            return;
        }
        let snapshot = self.layout().export_tree();
        let group_gone = self.layout().group_members(pane).len() <= 1;
        if let Some(slot) = self.pages.remove(&pane) {
            if closing == Closing::Keep {
                self.stash_closed_page(pane, slot);
            }
        } else if closing == Closing::Keep {
            self.stash_closed_pane(&[pane], snapshot, Vec::new(), group_gone);
        }
        self.drop_tab_state(pane);
        self.layout_mut().close(pane);
        if group_gone {
            // Rebalance the remaining groups so they stay evenly spaced,
            // without reshaping the tree.
            self.layout_mut().balance();
        }
        self.after_tab_change();
    }

    /// Forget everything kept about one tab: its terminal or its page, and
    /// every per-tab record the rest of the app holds under its id.
    pub(crate) fn drop_tab_state(&mut self, pane: PaneId) {
        // Work the tab asked for would come back to a tab that is gone, and
        // with its id given back out on a restore, to the wrong one.
        self.jobs.cancel_for(pane);
        self.pages.remove(&pane);
        if self.page_prompt.as_ref().is_some_and(|p| p.pane == pane) {
            self.page_prompt = None;
        }
        if self.page_cursor.as_ref().is_some_and(|c| c.pane == pane) {
            self.stop_page_cursor();
        }
        self.webview_mgr.remove_surface(pane);
        self.panes.remove(&pane);
        self.modes.remove(&pane);
        self.nav_cursors.remove(&pane);
        self.vim.jump_lists.remove(&pane);
        self.vim.change_lists.remove(&pane);
        self.vim.last_changes.remove(&pane);
        self.vim.insert_sessions.remove(&pane);
        self.vim.marks.retain(|(p, _), _| *p != pane);
        self.pane_titles.remove(&pane);
        self.tabs.names.remove(&pane);
        self.webview_mgr.remove_tiles_for_pane(pane);
        self.retain_image_blocks(|img| img.pane_id != pane);
        if self.selection.span.as_ref().is_some_and(|s| s.pane == pane) {
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

    // --------------------------------------------------------------------
    // Relayout
    // --------------------------------------------------------------------

    /// Re-lay-out panes after a change to the reserved top-tabbar or status-bar
    /// rows (menu style, status-bar visibility), and request a redraw.
    pub(crate) fn relayout_tabbar(&mut self) {
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Bring everything that depends on which tabs are shown up to date after
    /// one was opened, shown, or closed.
    pub(crate) fn after_tab_change(&mut self) {
        self.selection.span = self
            .selection
            .span
            .take()
            .filter(|s| self.layout().panes().contains(&s.pane));
        self.close_menu();
        // Force a tile reposition so hidden tabs' WebViews are hidden and the
        // shown tabs' are placed (the layout key alone may not have changed).
        self.last_tile_layout = None;
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        self.update_window_title();
    }
}

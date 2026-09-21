//! Pane creation, splitting, closing, and per-frame pane upkeep.

use std::collections::HashSet;
use std::path::PathBuf;

use crate::model::layout::{Direction, PaneId, Tab};
use crate::model::mode::Mode;
use crate::terminal::pane::Pane;

use super::page::{ClosedPane, Closing};
use super::App;
use super::{DEFAULT_COLS, DEFAULT_ROWS, SPLIT_RATIO};

// ========================================================================
// App: pane lifecycle
// ========================================================================

impl App {
    /// Allocate the next globally-unique pane id.
    pub(crate) fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        id
    }
    /// Spawns a pane, retrying with the OS default shell (ignoring `shell`)
    /// if the first attempt fails, so a bad `shell`/`shell-*` setting or a
    /// session-restore command that no longer exists surfaces as a
    /// status-bar error instead of crashing the app. `None` only when the
    /// fallback also fails (e.g. the OS itself is out of resources), in
    /// which case the caller must not proceed as if a pane was created.
    pub(crate) fn spawn_pane_or_notify(
        &mut self,
        cols: usize,
        rows: usize,
        shell: Option<&str>,
        scrollback: usize,
        cwd: Option<&str>,
    ) -> Option<Pane> {
        match Pane::new_with_cwd(cols, rows, shell, scrollback, cwd) {
            Ok(pane) => Some(pane),
            Err(e) => {
                self.set_error(format!(
                    "shell failed to start ({e}), trying the default shell"
                ));
                match Pane::new_with_cwd(cols, rows, None, scrollback, cwd) {
                    Ok(pane) => Some(pane),
                    Err(e2) => {
                        self.set_error(format!("could not start a shell: {e2}"));
                        None
                    }
                }
            }
        }
    }
    /// The working directory of the currently focused pane of the active tab, if
    /// available. Used to spawn a new pane/tab in the same directory.
    pub(crate) fn focused_cwd(&self) -> Option<String> {
        let focused = self.tab().focused();
        self.panes.get(&focused).and_then(|pane| pane.cwd())
    }
    /// Where an action launched from the focused pane starts: the page
    /// covering it knows what is being looked at and answers first, and only
    /// with no page there does the shell's own directory decide.
    pub(crate) fn focused_start_dir(&self) -> PathBuf {
        let focused = self.tab().focused();
        let dir = self.pages
            .get(&focused)
            .and_then(|slot| slot.page.cwd())
            .or_else(|| self.focused_cwd().map(PathBuf::from))
            .or_else(|| {
                let cur = std::env::current_dir().ok()?;
                #[cfg(windows)]
                {
                    let s = cur.to_string_lossy();
                    if s.ends_with("System32") || s.ends_with("system32") {
                        return crate::model::path::home_dir();
                    }
                }
                Some(cur)
            })
            .or_else(crate::model::path::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        crate::model::path::normalize_path(dir)
    }
    /// The same directory as a shell's working directory, dropped when the
    /// path cannot be spelled as one.
    pub(crate) fn focused_start_cwd(&self) -> Option<String> {
        self.focused_start_dir().to_str().map(str::to_string)
    }
    pub(crate) fn split_pane(&mut self, direction: Direction) {
        // Captured before the layout split so the new pane opens where the
        // focused one was being used, which for a pane under a tool is the
        // directory that tool is looking at rather than the shell's own.
        let cwd = self.focused_start_cwd();
        self.split_pane_at(direction, cwd);
    }

    /// Split the focused pane and start the new one's shell in `cwd`.
    ///
    /// Returns the new pane, or nothing when no shell could be started, in
    /// which case the split is undone rather than left pointing at a pane
    /// that does not exist.
    pub(crate) fn split_pane_at(
        &mut self,
        direction: Direction,
        cwd: Option<String>,
    ) -> Option<PaneId> {
        let new_id = self.alloc_pane_id();
        self.tab_mut().split(direction, SPLIT_RATIO, new_id);
        // Rebalance every split's ratio so all panes share the viewport equally,
        // without altering the tree shape the user built (mixed split
        // directions stay mixed; only the sizes change). See `Tab::balance`.
        self.tab_mut().balance();

        let (pane_cols, pane_rows) = self.spawn_grid_size(new_id, direction);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let Some(pane) = self.spawn_pane_or_notify(
            pane_cols.max(1),
            pane_rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            // Undo the split: no pane exists for `new_id`, so the tree can't
            // be left referencing it.
            self.tab_mut().close(new_id);
            self.tab_mut().balance();
            self.dirty = true;
            return None;
        };
        self.panes.insert(new_id, pane);
        self.modes.insert(new_id, Mode::default());

        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
        Some(new_id)
    }
    /// Grid size to spawn `pane` at: the exact size of its post-split rect
    /// when a renderer is available, else the legacy half-window guess.
    ///
    /// Spawning at the real size matters on Windows: the old estimate used
    /// `renderer.grid_size()` (the full window grid, including the
    /// tabbar/status-bar rows and ignoring existing splits), so the child was
    /// routinely started too large: by the chrome rows for a first split, and
    /// roughly 2× too tall when splitting an already-half-height pane.
    /// [`Self::resize_all_panes`] would then shrink the PTY + grid to fit. Unix
    /// PTYs reflow that shrink cleanly, but Windows ConPTY reflows a shrink
    /// asynchronously and lossily, so a shell that draws at startup (e.g.
    /// nushell's banner/prompt) briefly paints for the oversized grid and lands
    /// offset within the pane until it redraws, looking like the split missed
    /// the middle. Sizing the child correctly up front means its first render is
    /// already at the final size, so [`Self::resize_all_panes`] has nothing to
    /// shrink (its `Pane::resize` early-returns on the unchanged size).
    pub(crate) fn spawn_grid_size(&self, pane: PaneId, direction: Direction) -> (usize, usize) {
        let Some(renderer) = self.renderer.as_ref() else {
            let (cols, rows) = (DEFAULT_COLS as usize, DEFAULT_ROWS as usize);
            return match direction {
                Direction::Vertical => (cols / 2, rows),
                Direction::Horizontal => (cols, rows / 2),
            };
        };
        let viewport = self.content_viewport();
        if let Some((_, rect)) = self
            .tab()
            .rects(viewport)
            .into_iter()
            .find(|(id, _)| *id == pane)
        {
            return renderer.grid_size_for(Self::layout_rect_to_pane(rect));
        }
        // `pane` was just inserted by `split`, so the lookup above always
        // succeeds; this only guards a hypothetical caller that splits before
        // the layout is consistent.
        let (cols, rows) = renderer.grid_size();
        match direction {
            Direction::Vertical => (cols / 2, rows),
            Direction::Horizontal => (cols, rows / 2),
        }
    }
    pub(crate) fn close_pane(&mut self, pane_id: PaneId) {
        // Edits nowhere but in a page over this pane are the one thing
        // closing it cannot give back on its own, so they are asked about
        // first. The pane closes on the answer, not here.
        if self.ask_before_closing_pane(pane_id) {
            return;
        }
        self.close_pane_now(pane_id);
    }

    /// Close `pane_id` without asking, which is what an answered question
    /// and a pane with nothing to lose both come down to.
    pub(crate) fn close_pane_now(&mut self, pane_id: PaneId) {
        self.close_pane_in_any_tab(pane_id, Closing::Keep);
    }
    /// Close `pane_id` in whichever tab holds it, collapsing its split into the
    /// sibling. When it is the last pane in its tab, the whole tab is closed.
    /// Drops all per-pane state and re-lays-out if the affected tab is the active one.
    pub(crate) fn close_pane_in_any_tab(&mut self, pane_id: PaneId, closing: Closing) {
        let Some(tab_idx) = self
            .tabs
            .all
            .iter()
            .position(|t| t.panes().contains(&pane_id))
        else {
            return;
        };
        if self.tabs.all[tab_idx].panes().len() <= 1 {
            // Closing the last pane of a tab closes that tab, but never the last
            // pane of the only tab, so the close-pane command always leaves at
            // least one pane open.
            if self.tabs.all.len() <= 1 {
                return;
            }
            // Whatever was worth asking about was asked about before this
            // pane started closing.
            self.close_tab_now(tab_idx);
            return;
        }
        // The pane itself is kept, not just the tools it was holding: where
        // it sat and what it was running in are what a split merged back by
        // accident costs, and one key puts all of it back.
        let pages = self.stash_pane_pages(pane_id, closing);
        if closing == Closing::Keep {
            let layout = self.tabs.all[tab_idx].export_tree();
            self.stash_closed_pane(pane_id, tab_idx, layout, pages);
        }
        // Work the pane asked for would come back to a pane that is gone, and
        // with its id given back out on a restore, to the wrong one.
        self.jobs.cancel_for(pane_id);
        self.panes.remove(&pane_id);
        self.modes.remove(&pane_id);
        self.nav_cursors.remove(&pane_id);
        self.vim.jump_lists.remove(&pane_id);
        self.vim.change_lists.remove(&pane_id);
        self.vim.last_changes.remove(&pane_id);
        self.vim.insert_sessions.remove(&pane_id);
        self.vim.marks.retain(|(p, _), _| *p != pane_id);
        self.pane_titles.remove(&pane_id);
        self.webview_mgr.remove_tiles_for_pane(pane_id);
        self.retain_image_blocks(|img| img.pane_id != pane_id);
        self.last_tile_layout = None;
        self.tabs.all[tab_idx].close(pane_id);
        // Rebalance the remaining panes' ratios so they stay evenly spaced,
        // without reshaping the tree (closing one pane would otherwise leave
        // its sibling oversized).
        self.tabs.all[tab_idx].balance();
        if tab_idx == self.tabs.active && self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }
    /// Put a closed pane back: its split, a shell where its own was, and the
    /// tools it was holding.
    ///
    /// The shell itself cannot come back, since closing the pane dropped the
    /// pseudo-terminal and killed the child; what comes back is a new one in
    /// the same directory, the way session restore reopens a pane.
    ///
    /// The tab's whole tree is restored when the rest of it has not moved on
    /// since, which is what puts the pane back where it was rather than
    /// wherever the reader now is. A tab that has been split or closed in the
    /// meantime gets a plain split instead: the snapshot describes panes that
    /// no longer agree with it.
    pub(crate) fn restore_closed_pane(&mut self, closed: ClosedPane) {
        let restored = match self.reopen_pane_layout(&closed) {
            Some(pane_id) => Some(pane_id),
            None => self.split_pane_at(Direction::Vertical, closed.cwd()),
        };
        // No shell could be started, so there is nowhere to put the pages:
        // they stay in the stash rather than going down with the failure,
        // which `spawn_pane_or_notify` has already reported.
        let Some(pane_id) = restored else {
            return;
        };
        let pages = self.take_closed_pages(&closed.page_ids());
        self.tab_mut().focus(pane_id);
        self.restore_pages_into(pane_id, pages);
        self.dirty = true;
    }

    /// Put the tab's split tree back as it was and start a shell in the pane
    /// that was missing from it, or `None` when the snapshot no longer
    /// describes the tab.
    fn reopen_pane_layout(&mut self, closed: &ClosedPane) -> Option<PaneId> {
        let tab_idx = closed.tab();
        let tab = self.tabs.all.get(tab_idx)?;
        let held: HashSet<PaneId> = tab.panes().into_iter().collect();
        let snapshot: HashSet<PaneId> = closed.layout_panes().into_iter().collect();
        // Every pane the snapshot names is either still there or is the one
        // that was closed; anything else means the tab has moved on.
        if snapshot.len() != held.len() + 1 || !held.is_subset(&snapshot) {
            return None;
        }
        let pane_id = closed.pane();
        if !snapshot.contains(&pane_id) {
            return None;
        }

        self.switch_tab(tab_idx);
        self.tabs.all[tab_idx] = Tab::from_tree(closed.layout(), pane_id);
        let (cols, rows) = self.spawn_grid_size(pane_id, Direction::Vertical);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let pane = self.spawn_pane_or_notify(
            cols.max(1),
            rows.max(1),
            shell.as_deref(),
            scrollback,
            closed.cwd().as_deref(),
        )?;
        self.panes.insert(pane_id, pane);
        self.modes.insert(pane_id, Mode::default());
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        Some(pane_id)
    }

    /// Close every pane in the tab except `focused` (Vim `Ctrl-w o`).
    pub(crate) fn close_other_panes(&mut self, focused: PaneId) {
        // One question for the lot of them: asking pane by pane would put a
        // dialog up behind the dialog already waiting to be answered.
        if self.ask_before_closing_others(focused) {
            return;
        }
        self.close_other_panes_now(focused);
    }

    /// The same, once the question has been answered or there was none.
    pub(crate) fn close_other_panes_now(&mut self, focused: PaneId) {
        let others: Vec<PaneId> = self
            .tab()
            .panes()
            .into_iter()
            .filter(|&id| id != focused)
            .collect();
        for id in others {
            self.close_pane_now(id);
        }
    }
    pub(crate) fn switch_to_pane(&mut self, pane_id: PaneId) {
        if let Some(tab_index) = self
            .tabs
            .all
            .iter()
            .position(|tab| tab.panes().contains(&pane_id))
        {
            self.switch_tab(tab_index);
            self.tabs.all[tab_index].focus(pane_id);
            self.update_window_title();
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
    pub(crate) fn reap_dead_panes(&mut self) {
        let dead: Vec<PaneId> = self
            .panes
            .iter_mut()
            .filter_map(|(id, pane)| if pane.is_alive() { None } else { Some(*id) })
            .collect();
        for id in dead {
            let Some(tab_idx) = self.tabs.all.iter().position(|t| t.panes().contains(&id)) else {
                continue;
            };
            if self.tabs.all[tab_idx].panes().len() > 1 {
                // A shell that exited was asked to: the pane is not worth
                // keeping, though any tool it was holding still is.
                self.close_pane_in_any_tab(id, Closing::Forget);
            } else {
                // The last pane of a tab closes the whole tab (or, if it is
                // also the last tab, requests app exit). Never asked about: a
                // question raised by a shell exiting on its own would put a
                // dialog up over a pane nobody was closing, and refusing it
                // would leave the dead pane to ask again on the next frame.
                self.close_tab_now(tab_idx);
            }
        }
    }
    pub(crate) fn drain_all_panes(&mut self) -> bool {
        let mut any = false;
        let mut new_entries: Vec<(PaneId, crate::terminal::block_queue::BlockEntry)> = Vec::new();
        let mut patched_tiles: Vec<(PaneId, usize)> = Vec::new();
        let mut new_titles: Vec<(PaneId, String)> = Vec::new();
        // OSC 52 writes from the PTY, applied after the loop so the shared
        // clipboard handle can be borrowed without conflicting with `iter_mut`.
        let mut clipboard_write: Option<String> = None;
        // Panes whose PTY raised an OSC 52 read query, answered after the loop.
        let mut clipboard_reads: Vec<PaneId> = Vec::new();
        // Absolute row spans a `clear` (or any screen erase) blanked this pump,
        // per pane: the blocks anchored inside them are dropped below.
        let mut erased: Vec<(PaneId, (usize, usize))> = Vec::new();
        // Blocks the retention budget elided this pump, as
        // `(pane, block_index, segment_index)`: their rendered texture and
        // WebView tile can never show content again.
        let mut elided: Vec<(PaneId, usize, usize)> = Vec::new();
        // Row remaps from resizes applied during this pump (mux session
        // geometry), drained after the loop so `self` is free to be borrowed
        // mutably for the app's own anchors.
        let mut remapped: Vec<(PaneId, winter_render::RowRemap)> = Vec::new();
        // Re-assert the block trust ceiling every pump rather than at pane
        // construction: panes are created from a dozen places (new tab, split,
        // session restore, mux attach), and a construction site that forgot to
        // apply the policy would silently over-grant. Setting it here costs a
        // field write per pane and cannot be missed.
        let max_trust = self.config.security.block_max_trust;
        for (_, pane) in self.panes.iter_mut() {
            pane.block_queue_mut().set_max_trust(max_trust);
        }
        for (id, pane) in self.panes.iter_mut() {
            for remap in pane.take_row_remaps() {
                remapped.push((*id, remap));
            }
            let prev_count = pane.block_queue().entries().len();
            if pane.drain_output() {
                pane.grid_mut().detect_urls();
                any = true;
            }
            if let Some(text) = pane.take_clipboard_write() {
                clipboard_write = Some(text);
            }
            if pane.take_clipboard_read() {
                clipboard_reads.push(*id);
            }
            // Drain the terminal bell flag; the tab notification indicator was
            // removed, so the bell no longer drives any UI.
            pane.take_bell();
            if let Some(title) = pane.take_title() {
                new_titles.push((*id, title));
            }
            let curr_entries = pane.block_queue().entries();
            if curr_entries.len() > prev_count {
                for entry in &curr_entries[prev_count..] {
                    new_entries.push((*id, entry.clone()));
                }
            }
            let patched = pane.drain_live_patches();
            for idx in patched {
                patched_tiles.push((*id, idx));
            }
            for (block_index, segment_index) in pane.drain_elided_blocks() {
                elided.push((*id, block_index, segment_index));
            }
            for span in pane.take_erased_spans() {
                erased.push((*id, span));
            }
        }
        if !new_titles.is_empty() {
            for (id, title) in new_titles {
                self.pane_titles.insert(id, title);
            }
            self.update_window_title();
        }
        for (id, remap) in &remapped {
            self.remap_pane_anchors(*id, remap);
        }
        // Before new blocks are built, so a block emitted in the same pump as
        // the `clear` that preceded it survives.
        for (id, span) in erased {
            self.drop_blocks_in(id, span);
        }
        self.drop_elided_blocks(&elided);
        if !new_entries.is_empty() {
            self.create_block_tiles(&new_entries);
        }
        if !patched_tiles.is_empty() {
            self.update_live_tiles(&patched_tiles);
        }
        if let Some(text) = clipboard_write {
            if let Some(cb) = self.clipboard() {
                let _ = cb.set_text(&text);
            }
        }
        // OSC 52 reads answer only when `clipboard-read` opted in: the query
        // is silent on the tool's side, so the default must stay a refusal.
        if !clipboard_reads.is_empty() && self.config.clipboard_read {
            let text = self
                .clipboard()
                .and_then(|cb| cb.get_text().ok())
                .unwrap_or_default();
            let response = crate::terminal::pane::osc52_read_response(&text);
            for id in clipboard_reads {
                if let Some(pane) = self.panes.get_mut(&id) {
                    pane.write(&response);
                }
            }
        }
        any
    }
    pub(crate) fn resize_all_panes(&mut self) {
        let (cw, ch) = if let Some(renderer) = &self.renderer {
            renderer.cell_size()
        } else {
            (9.0, 20.0)
        };

        let layout_vp = self.content_viewport();
        let rects = self.tab().rects(layout_vp);

        // Size each grid to the renderer's content area (it insets every pane by
        // PANE_H_PAD horizontally). Computing cols from the raw rect width would
        // make the grid a column wider than what is drawn, pushing the scrollbar
        // past the pane's right edge (where the rightmost pane's bar gets clipped
        // by the surface, looking thinner than the others). Sizes are collected
        // first so the renderer borrow does not overlap the `panes` mutation.
        let sizes: Vec<(PaneId, usize, usize)> = rects
            .iter()
            .map(|(id, rect)| {
                let (cols, rows) = match &self.renderer {
                    Some(renderer) => renderer.grid_size_for(Self::layout_rect_to_pane(*rect)),
                    None => (
                        (rect.width / cw).floor().max(1.0) as usize,
                        (rect.height / ch).floor().max(1.0) as usize,
                    ),
                };
                (*id, cols, rows)
            })
            .collect();

        for (id, cols, rows) in sizes {
            if let Some(pane) = self.panes.get_mut(&id) {
                // A resize reflows the grid, which snaps the view back to the live
                // bottom; put the pane back where the user was reading. Starting or
                // ending a `/` search toggles the forced status bar, resizing every
                // pane by a row, losing the scroll position there would yank the
                // viewport away from the match being browsed.
                let offset = pane.grid().scroll_offset();
                pane.resize(cols.max(1), rows.max(1));
                if offset > 0 {
                    pane.grid_mut().set_scroll_offset(offset);
                }
                // The reflow moved the content the app's anchors name; push
                // them through the remap before the redraw lands.
                for remap in pane.take_row_remaps() {
                    self.remap_pane_anchors(id, &remap);
                }
            }
        }
    }
}

//! Pane creation, splitting, closing, and per-frame pane upkeep.

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::model::layout::{Direction, Layout, TabId};
use crate::terminal::session::{Reap, Terminal};

use super::appearance::terminal_presentation;
use super::tool::{ClosedPane, ClosedTerminal, Closing};
use super::App;
use super::{DEFAULT_COLS, DEFAULT_ROWS, SPLIT_RATIO};

// ========================================================================
// App: pane lifecycle
// ========================================================================

impl App {
    /// Allocate the next globally-unique tab id.
    pub(crate) fn alloc_tab_id(&mut self) -> TabId {
        let id = TabId(self.next_tab_id);
        self.next_tab_id += 1;
        id
    }
    /// Spawns a terminal, retrying with the OS default shell (ignoring `shell`)
    /// if the first attempt fails, so a bad `shell`/`shell-*` setting or a
    /// session-restore command that no longer exists surfaces as a
    /// status-bar error instead of crashing the app. `None` only when the
    /// fallback also fails (e.g. the OS itself is out of resources), in
    /// which case the caller must not proceed as if a terminal was created.
    pub(crate) fn spawn_terminal_or_notify(
        &mut self,
        cols: usize,
        rows: usize,
        shell: Option<&str>,
        scrollback: usize,
        cwd: Option<&str>,
    ) -> Option<Terminal> {
        match Terminal::new_with_cwd(cols, rows, shell, scrollback, cwd) {
            Ok(terminal) => Some(terminal),
            Err(e) => {
                self.set_error(format!(
                    "shell failed to start ({e}), trying the default shell"
                ));
                match Terminal::new_with_cwd(cols, rows, None, scrollback, cwd) {
                    Ok(terminal) => Some(terminal),
                    Err(e2) => {
                        self.set_error(format!("could not start a shell: {e2}"));
                        None
                    }
                }
            }
        }
    }
    /// The working directory of the focused tab's shell, else of the shell
    /// shown most recently in the same pane, if available. Used to spawn a new
    /// pane or tab in the same directory.
    pub(crate) fn focused_cwd(&self) -> Option<String> {
        let focused = self.layout().focused();
        self.layout().recent_in_pane(focused).iter().find_map(|id| {
            self.open_tabs
                .terminal(id)
                .and_then(|terminal| terminal.cwd())
        })
    }
    /// Where an action launched from the focused tab starts: a tool tab knows
    /// what is being looked at and answers first, and only without one does
    /// a shell's own directory decide.
    pub(crate) fn focused_start_dir(&self) -> PathBuf {
        let focused = self.layout().focused();
        let dir = self
            .open_tabs
            .tool(&focused)
            .and_then(|slot| slot.tool.cwd())
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
        // focused one was being used, which for a tool tab is the directory
        // that tool is looking at rather than a shell's own.
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
    ) -> Option<TabId> {
        let new_id = self.alloc_tab_id();
        self.layout_mut().split(direction, SPLIT_RATIO, new_id);
        // Rebalance every split's ratio so all panes share the viewport equally,
        // without altering the tree shape the user built (mixed split
        // directions stay mixed; only the sizes change). See `Tab::balance`.
        self.layout_mut().balance();

        let (pane_cols, pane_rows) = self.spawn_grid_size(new_id, direction);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let Some(terminal) = self.spawn_terminal_or_notify(
            pane_cols.max(1),
            pane_rows.max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) else {
            // Undo the split: no pane exists for `new_id`, so the tree can't
            // be left referencing it.
            self.layout_mut().close(new_id);
            self.layout_mut().balance();
            self.dirty = true;
            return None;
        };
        self.open_tabs.insert_terminal(new_id, terminal);

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
    pub(crate) fn spawn_grid_size(&self, tab: TabId, direction: Direction) -> (usize, usize) {
        let Some(renderer) = self.renderer.as_ref() else {
            let (cols, rows) = (DEFAULT_COLS as usize, DEFAULT_ROWS as usize);
            return match direction {
                Direction::Vertical => (cols / 2, rows),
                Direction::Horizontal => (cols, rows / 2),
            };
        };
        if let Some((_, rect)) = self.pane_rects().into_iter().find(|(id, _)| *id == tab) {
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
    /// Close the pane holding `tab_id`, every tab in it. The last pane of
    /// the window is never closed this way.
    pub(crate) fn close_pane(&mut self, tab_id: TabId) {
        // Edits nowhere but in a tool tab of this pane are the one thing
        // closing it cannot give back on its own, so they are asked about
        // first. The pane closes on the answer, not here.
        if self.ask_before_closing_pane(tab_id) {
            return;
        }
        self.close_pane_now(tab_id);
    }

    /// Close the pane holding `tab_id` without asking, which is what an
    /// answered question and a pane with nothing to lose both come down to.
    ///
    /// The pane is kept, not just its tools: where it sat and what its
    /// shells were running in are what a split merged back by accident
    /// costs, and one key puts all of it back.
    pub(crate) fn close_pane_now(&mut self, tab_id: TabId) {
        let tabs = self.layout().tabs_in_pane(tab_id);
        if tabs.is_empty() || self.layout().shown_tabs().len() <= 1 {
            return;
        }
        let snapshot = self.layout().export_tree();
        let mut tools = Vec::new();
        let mut terminals = Vec::new();
        for &tab in &tabs {
            match self.open_tabs.remove_tool(&tab) {
                Some(slot) => tools.push(self.stash_closed_tool(tab, slot)),
                None => terminals.push(tab),
            }
        }
        self.stash_closed_pane(&terminals, snapshot, tools, true);
        for &tab in &tabs {
            self.drop_tab_state(tab);
            self.layout_mut().close(tab);
        }
        // Rebalance the remaining panes' ratios so they stay evenly spaced,
        // without reshaping the tree (closing one pane would otherwise leave
        // its sibling oversized).
        self.layout_mut().balance();
        self.after_tab_change();
    }

    /// Put closed terminals back: shells where their own were, the tool tabs
    /// that went with them, and the split they sat in.
    ///
    /// The shells themselves cannot come back, since closing them dropped
    /// the pseudo-terminals and killed the children; what comes back is a new
    /// one in each directory, the way session restore reopens a pane.
    ///
    /// The layout snapshot is restored when the rest of the window has not
    /// moved on since, which puts everything back where it was rather than
    /// wherever the reader now is. Otherwise a closed pane comes back as a new
    /// split, and closed tabs join the focused pane.
    pub(crate) fn restore_closed_pane(&mut self, closed: ClosedPane) {
        let before: HashSet<TabId> = self.layout().members().into_iter().collect();
        let mut restored: Vec<TabId> = Vec::new();
        for terminal in closed.terminals() {
            if let Some(tab) = self.spawn_closed_terminal(&terminal) {
                restored.push(tab);
            }
        }
        // No shell could be started, so there is nowhere to put the tools:
        // they stay in the stash rather than going down with the failure,
        // which `spawn_terminal_or_notify` has already reported.
        if restored.is_empty() {
            return;
        }
        let tools = self.take_closed_tools(&closed.tool_ids());
        let first = restored[0];
        if closed.is_whole_pane() {
            self.split_layout_for(first);
        } else {
            self.layout_mut().add_tab(first);
        }
        for &tab in &restored[1..] {
            self.layout_mut().add_tab(tab);
        }
        for tool in tools {
            self.restore_closed_tool(tool);
        }
        self.reapply_snapshot(&closed, &before);
        self.set_focus(first);
        self.after_tab_change();
    }

    /// Start a shell in a closed terminal's directory, under its old tab id
    /// when that is free. Returns the tab it went into.
    fn spawn_closed_terminal(&mut self, terminal: &ClosedTerminal) -> Option<TabId> {
        let tab_id = match self.layout().contains(terminal.tab) {
            true => self.alloc_tab_id(),
            false => terminal.tab,
        };
        let (cols, rows) = self.spawn_grid_size(self.layout().focused(), Direction::Vertical);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let spawned = self.spawn_terminal_or_notify(
            cols.max(1),
            rows.max(1),
            shell.as_deref(),
            scrollback,
            terminal.cwd.as_deref(),
        )?;
        self.open_tabs.insert_terminal(tab_id, spawned);
        Some(tab_id)
    }

    /// Give `tab` a pane of its own beside the focused one.
    fn split_layout_for(&mut self, tab: TabId) {
        self.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, tab);
        self.layout_mut().balance();
    }

    /// Put the window back into the shape `closed` was taken in, when every
    /// tab open before the restore is one the snapshot names and every tab
    /// the snapshot names is open now: nothing was opened or closed since.
    fn reapply_snapshot(&mut self, closed: &ClosedPane, before: &HashSet<TabId>) {
        let snapshot = closed.layout();
        let named: HashSet<TabId> = snapshot.members().into_iter().collect();
        let now: HashSet<TabId> = self.layout().members().into_iter().collect();
        if !before.is_subset(&named) || named != now {
            return;
        }
        let focused = self.layout().focused();
        if let Some(layout) = Layout::from_tree(snapshot, focused) {
            *self.layout_mut() = layout;
        }
    }

    /// Close every pane except the one holding `focused` (Vim `C-w o`).
    pub(crate) fn close_other_panes(&mut self, focused: TabId) {
        // One question for the lot of them: asking pane by pane would put a
        // dialog up behind the dialog already waiting to be answered.
        if self.ask_before_closing_others(focused) {
            return;
        }
        self.close_other_panes_now(focused);
    }

    /// The same, once the question has been answered or there was none.
    pub(crate) fn close_other_panes_now(&mut self, focused: TabId) {
        let kept = self.layout().tabs_in_pane(focused);
        let others: Vec<TabId> = self
            .layout()
            .shown_tabs()
            .into_iter()
            .filter(|id| !kept.contains(id))
            .collect();
        for id in others {
            self.close_pane_now(id);
        }
    }

    /// Show and focus `tab_id`, wherever it is.
    pub(crate) fn switch_to_tab(&mut self, tab_id: TabId) {
        self.show_tab(tab_id);
        self.dirty = true;
    }

    /// Close every terminal tab whose shell has exited. The last tab of the
    /// window closing asks for the app to exit.
    ///
    /// Never asked about: a question raised by a shell exiting on its own
    /// would put a dialog up over a tab nobody was closing, and refusing it
    /// would leave the dead tab to ask again on the next frame.
    pub(crate) fn reap_dead_panes(&mut self) {
        let mut dead: Vec<TabId> = Vec::new();
        for (id, pane) in self.open_tabs.terminals_mut() {
            match pane.reap() {
                Reap::Close => dead.push(*id),
                Reap::Keep => {}
                Reap::ShowedExit => self.dirty = true,
            }
        }
        for id in dead {
            // A shell that exited was asked to: the tab is not worth keeping.
            self.close_tab_now(id, Closing::Forget);
        }
    }
    pub(crate) fn drain_all_panes(&mut self) -> bool {
        let mut any = false;
        let mut new_entries: Vec<(TabId, crate::terminal::block_queue::BlockEntry)> = Vec::new();
        let mut patched_tiles: Vec<(TabId, usize)> = Vec::new();
        let mut new_titles: Vec<(TabId, String)> = Vec::new();
        // OSC 52 writes from the PTY, applied after the loop so the shared
        // clipboard handle can be borrowed without conflicting with `iter_mut`.
        let mut clipboard_write: Option<String> = None;
        // Panes whose PTY raised an OSC 52 read query, answered after the loop.
        let mut clipboard_reads: Vec<TabId> = Vec::new();
        // Absolute row spans a `clear` (or any screen erase) blanked this pump,
        // per pane: the blocks anchored inside them are dropped below.
        let mut erased: Vec<(TabId, (usize, usize))> = Vec::new();
        // Blocks the retention budget elided this pump, as
        // `(pane, block_index, segment_index)`: their rendered texture can
        // never show content again.
        let mut elided: Vec<(TabId, usize, usize)> = Vec::new();
        // Row remaps from resizes applied during this pump (mux session
        // geometry), drained after the loop so `self` is free to be borrowed
        // mutably for the app's own anchors.
        let mut remapped: Vec<(TabId, winter_render::RowRemap)> = Vec::new();
        // How the window shows each terminal (its cell size and theme
        // colors, which programs may ask about) is set every pump: a font or
        // theme change reaches every pane on the next one, however it was
        // made.
        let presentation = self.renderer.as_ref().map(terminal_presentation);
        // And how its history folds once a width change settles: set here,
        // a settings-page toggle or a config reload reaches every pane too.
        let (word_wrap, wrap_indent) = (self.config.wrap_words, self.config.wrap_indent);
        for (_, terminal) in self.open_tabs.terminals_mut() {
            if let Some(presentation) = presentation {
                terminal.set_presentation(presentation);
            }
            let grid = terminal.grid_mut();
            grid.set_word_wrap(word_wrap);
            grid.set_wrap_indent(wrap_indent);
        }
        let now = Instant::now();
        for (id, terminal) in self.open_tabs.terminals_mut() {
            any |= terminal.settle_history(now);
            for remap in terminal.take_row_remaps() {
                remapped.push((*id, remap));
            }
            let prev_count = terminal.block_queue().entries().len();
            if terminal.drain_output() {
                terminal.grid_mut().detect_urls();
                any = true;
            }
            if let Some(text) = terminal.take_clipboard_write() {
                clipboard_write = Some(text);
            }
            if terminal.take_clipboard_read() {
                clipboard_reads.push(*id);
            }
            // Drain the terminal bell flag; the tab notification indicator was
            // removed, so the bell no longer drives any UI.
            terminal.take_bell();
            if let Some(title) = terminal.take_title() {
                new_titles.push((*id, title));
            }
            let curr_entries = terminal.block_queue().entries();
            if curr_entries.len() > prev_count {
                for entry in &curr_entries[prev_count..] {
                    new_entries.push((*id, entry.clone()));
                }
            }
            let patched = terminal.drain_live_patches();
            for idx in patched {
                patched_tiles.push((*id, idx));
            }
            for (block_index, segment_index) in terminal.drain_elided_blocks() {
                elided.push((*id, block_index, segment_index));
            }
            for span in terminal.take_erased_spans() {
                erased.push((*id, span));
            }
        }
        if !new_titles.is_empty() {
            for (id, title) in new_titles {
                self.tabs.set_program_title(id, title);
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
            let response = crate::terminal::session::osc52_read_response(&text);
            for id in clipboard_reads {
                if let Some(terminal) = self.open_tabs.terminal_mut(&id) {
                    terminal.write(&response);
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

        let rects = self.pane_rects();

        // Size each grid to the renderer's content area (it insets every pane by
        // PANE_H_PAD horizontally). Computing cols from the raw rect width would
        // make the grid a column wider than what is drawn, pushing the scrollbar
        // past the pane's right edge (where the rightmost pane's bar gets clipped
        // by the surface, looking thinner than the others). Sizes are collected
        // first so the renderer borrow does not overlap the `panes` mutation.
        let sizes: Vec<(TabId, usize, usize)> = rects
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
            if let Some(terminal) = self.open_tabs.terminal_mut(&id) {
                // A resize reflows the grid, which snaps the view back to the live
                // bottom; put the pane back where the user was reading. Starting or
                // ending a `/` search toggles the forced status bar, resizing every
                // pane by a row, losing the scroll position there would yank the
                // viewport away from the match being browsed.
                let offset = terminal.grid().scroll_offset();
                terminal.resize(cols.max(1), rows.max(1));
                if offset > 0 {
                    terminal.grid_mut().set_scroll_offset(offset);
                }
                // The reflow moved the content the app's anchors name; push
                // them through the remap before the redraw lands.
                for remap in terminal.take_row_remaps() {
                    self.remap_pane_anchors(id, &remap);
                }
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_support::*;

    #[test]
    fn test_a_new_pane_takes_the_wrap_settings_from_the_config() {
        // Regression: the wrap settings reached a terminal only when toggled
        // on the settings page, so every pane opened with them off whatever
        // the config said.
        let mut app = App::new();
        app.config.wrap_words = true;
        app.config.wrap_indent = true;
        let id = app.layout().focused();
        app.open_tabs.insert_terminal(id, idle_terminal());
        app.drain_all_panes();
        let grid = app.open_tabs.terminal(&id).expect("pane").grid();
        assert!(grid.word_wrap() && grid.wrap_indent());

        app.config.wrap_words = false;
        app.drain_all_panes();
        let grid = app.open_tabs.terminal(&id).expect("pane").grid();
        assert!(!grid.word_wrap(), "a reload or toggle reaches it too");
    }

    #[test]
    fn test_resize_all_panes_keeps_a_scrolled_back_pane_in_place() {
        // Regression: `Grid::resize` returns the view to the live bottom, so the
        // one-row resize that comes with showing/hiding the search status bar used
        // to yank a scrolled-back pane down to the prompt mid-browse.
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        let focused = app.layout().focused();
        let mut terminal = Terminal::with_command(
            40,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        for _ in 0..20 {
            terminal.grid_mut().line_feed();
        }
        terminal.grid_mut().scroll_up_history(6);
        app.open_tabs.insert_terminal(focused, terminal);

        app.resize_all_panes();

        assert_ne!(
            app.open_tabs.terminal(&focused).unwrap().grid().rows(),
            8,
            "fixture must actually be resized for this to prove anything"
        );
        assert_eq!(
            app.open_tabs
                .terminal(&focused)
                .unwrap()
                .grid()
                .scroll_offset(),
            6,
            "the pane should stay where it was scrolled to"
        );
    }

    #[test]
    fn test_close_pane_keeps_last_pane_of_the_window() {
        let mut app = app_with_tabs(1);
        let pane = app.layout().shown_tabs()[0];
        app.close_pane(pane);
        // The sole pane is preserved, not closed.
        assert!(app.layout().shown_tabs().contains(&pane));
    }

    #[test]
    fn test_close_pane_takes_every_tab_it_holds() {
        let mut app = app_with_tabs(2);
        app.layout_mut().split(Direction::Vertical, 0.5, TabId(9));
        app.close_pane(TabId(1));
        assert_eq!(app.layout().members(), vec![TabId(9)]);
    }

    #[test]
    fn test_a_failed_command_keeps_its_tab_with_its_last_output_until_a_key() {
        let mut app = App::new();
        let id = app.layout().focused();
        let mut terminal = Terminal::with_command(
            60,
            6,
            {
                let mut command = portable_pty::CommandBuilder::new("sh");
                command.args(["-c", "echo CONFLICT in notes.md; exit 3"]);
                command
            },
            1000,
        )
        .expect("test pane spawn");
        terminal.keep_on_failure();
        let deadline = Instant::now() + PTY_WAIT;
        while Instant::now() < deadline && terminal.is_alive() {
            std::thread::sleep(POLL_STEP);
        }
        app.open_tabs.insert_terminal(id, terminal);

        app.reap_dead_panes();
        let terminal = app.open_tabs.terminal(&id).expect("the tab stays");
        let text = terminal.grid().to_text();
        assert!(text.contains("CONFLICT in notes.md"), "{text}");
        assert!(text.contains("exited with code 3"), "{text}");
        app.reap_dead_panes();
        assert!(app.open_tabs.terminal(&id).is_some(), "and stays, waiting");
    }

    #[test]
    fn test_reap_dead_panes_closes_the_tab_whose_shell_died() {
        let mut app = app_with_tabs(2);
        insert_dead_pane(&mut app, TabId(0));
        app.reap_dead_panes();
        assert_eq!(app.layout().members(), vec![TabId(1)]);
        assert!(!app.exit_requested);
        assert!(app.closed.is_empty(), "a shell that exited is not kept");
    }

    #[test]
    fn test_reap_dead_panes_requests_exit_when_the_last_tab_dies() {
        let mut app = app_with_tabs(1);
        let dead_id = app.layout().shown_tabs()[0];
        insert_dead_pane(&mut app, dead_id);
        app.reap_dead_panes();
        // Nothing else to fall back to, so this requests the app quit rather
        // than leaving a pane whose shell has already exited on screen.
        assert!(app.exit_requested);
    }

    #[test]
    fn test_alloc_tab_id_is_monotonic() {
        let mut app = App::new();
        assert_eq!(app.alloc_tab_id(), TabId(1));
        assert_eq!(app.alloc_tab_id(), TabId(2));
    }
}

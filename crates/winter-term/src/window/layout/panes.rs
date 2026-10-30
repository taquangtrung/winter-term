//! Pane creation, splitting, closing, and per-frame pane upkeep.

use std::path::PathBuf;
use std::time::Instant;

use crate::editors::dir::DirEditor;
use crate::editors::git::GitEditor;
use crate::editors::launcher::LauncherEditor;
use crate::model::editor::{Editor, Launch};
use crate::model::layout::{Direction, DockState, TabId};
use crate::terminal::session::{Reap, Terminal};

use crate::window::appearance::terminal_presentation;
use crate::window::editor::{Closing, EditorKind};
use crate::window::{App, DEFAULT_COLS, DEFAULT_ROWS, DOCK_SHARE, SPLIT_RATIO};
use crate::window::RuntimeState;

// ========================================================================
// App: pane lifecycle
// ========================================================================

impl App {
    /// Allocate the next globally-unique tab id.
    pub(in crate::window) fn alloc_tab_id(&mut self) -> TabId {
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
    pub(in crate::window) fn spawn_terminal_or_notify(
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
    pub(in crate::window) fn focused_cwd(&self) -> Option<String> {
        let focused = self.layout().focused();
        self.layout().recent_in_pane(focused).iter().find_map(|id| {
            self.open_tabs
                .terminal(id)
                .and_then(|terminal| terminal.cwd())
        })
    }
    /// Where an action launched from the focused tab starts: an editor tab knows
    /// what is being looked at and answers first, and only without one does
    /// a shell's own directory decide.
    pub(in crate::window) fn focused_start_dir(&self) -> PathBuf {
        let focused = self.layout().focused();
        let dir = self
            .open_tabs
            .editor(&focused)
            .and_then(|slot| slot.editor.cwd())
            .or_else(|| self.focused_cwd().map(PathBuf::from))
            .or_else(|| {
                let cur = std::env::current_dir().ok()?;
                #[cfg(windows)]
                {
                    let s = cur.to_string_lossy();
                    if s.ends_with("System32") || s.ends_with("system32") {
                        return crate::paths::home_dir();
                    }
                }
                Some(cur)
            })
            .or_else(crate::paths::home_dir)
            .unwrap_or_else(|| PathBuf::from("/"));
        crate::model::path::normalize_path(dir)
    }
    /// The same directory as a shell's working directory, dropped when the
    /// path cannot be spelled as one.
    pub(in crate::window) fn focused_start_cwd(&self) -> Option<String> {
        self.focused_start_dir().to_str().map(str::to_string)
    }
    /// The dock along the bottom of the window, as VS Code's panel is
    /// toggled: opened with a shell where there is none, put away when it
    /// has the keyboard, else given the keyboard, and brought back when it
    /// is put away. Put away, its tabs keep running.
    pub(in crate::window) fn toggle_dock(&mut self) {
        let focused = self.layout().focused();
        match self.layout().dock_state() {
            DockState::Absent => self.open_dock(),
            DockState::Shown if self.layout().in_dock(focused) => self.layout_mut().hide_dock(),
            DockState::Shown | DockState::Hidden => self.layout_mut().show_dock(),
        }
        self.sync_focus_reports();
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Open the dock with a shell in it, started where the focused tab is
    /// working; nothing, and said so, where no shell could be started.
    fn open_dock(&mut self) {
        if self.layout().dock_state() != DockState::Absent {
            return;
        }
        let cwd = self.focused_start_cwd();
        let new_id = self.alloc_tab_id();
        self.layout_mut().open_dock(new_id, DOCK_SHARE);
        let (pane_cols, pane_rows) = self.spawn_grid_size(new_id, Direction::Horizontal);
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_core::MAX_SCROLLBACK);
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

    /// Split the focused pane, holding the new one with a launcher for
    /// whatever is chosen to go there, in the directory the focused tab is
    /// working in.
    pub(in crate::window) fn split_pane(&mut self, direction: Direction) {
        // Captured before the layout split, while the focus is still on the
        // pane split from.
        let dir = self.focused_start_dir();
        let new_id = self.alloc_tab_id();
        self.layout_mut().split(direction, SPLIT_RATIO, new_id);
        self.layout_mut().balance();
        self.open_tabs
            .insert_editor(new_id, EditorKind::Launcher, Box::new(LauncherEditor::new(dir)));
        self.after_tab_change();
    }

    /// Split the focused pane with a shell in the new one, for when a
    /// terminal is what goes there and the launcher would only be in the way.
    pub(in crate::window) fn split_pane_with_terminal(&mut self, direction: Direction) {
        // Captured before the layout split so the new pane opens where the
        // focused one was being used, which for an editor tab is the directory
        // that editor is looking at rather than a shell's own.
        let cwd = self.focused_start_cwd();
        self.split_pane_at(direction, cwd);
    }

    /// Open what launcher tab `tab` was asked for in its place, in its
    /// directory. The file picker opens over the pane instead, and the file
    /// picked takes the launcher's place as any tab opened in its pane does.
    pub(in crate::window) fn launch_from(&mut self, tab: TabId, launch: Launch) {
        let Some(dir) = self.open_tabs.editor(&tab).and_then(|slot| slot.editor.cwd()) else {
            return;
        };
        match launch {
            Launch::Dir => {
                self.record_recent_dir(&dir);
                self.replace_with_editor(tab, EditorKind::Dir, Box::new(DirEditor::new(dir)));
            }
            Launch::Files => self.open_find_file(),
            Launch::Git => {
                let editor = GitEditor::new(dir);
                // The view knows nothing until git answers, as when opened
                // any other way.
                let first = editor.initial_request();
                self.replace_with_editor(tab, EditorKind::Git, Box::new(editor));
                self.host.jobs.spawn(tab, first);
            }
            Launch::Terminal => {
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
                    dir.to_str(),
                ) else {
                    return;
                };
                self.drop_tab_state(tab);
                self.open_tabs.insert_terminal(tab, terminal);
                self.after_tab_change();
            }
        }
    }

    /// Put `editor` in tab `tab` in place of what the tab held, which is
    /// closed as any tab is (its jobs, its overlay, what a pane kept of it),
    /// keeping only its place in the layout: removed outright, a reply to a
    /// job it had asked for would reach the new editor under the same id.
    fn replace_with_editor(&mut self, tab: TabId, kind: EditorKind, editor: Box<dyn Editor>) {
        self.drop_tab_state(tab);
        self.open_tabs.insert_editor(tab, kind, editor);
        self.after_tab_change();
    }

    /// Close every launcher another tab has been shown over in its pane: one
    /// only holds an empty pane's place, so whatever is opened in the pane
    /// takes it. A tab merely added behind it (a build run quietly) leaves it
    /// in place: retired then, it vanished later, at an unrelated moment.
    pub(in crate::window) fn retire_launchers(&mut self) {
        let crowded: Vec<TabId> = self
            .open_tabs
            .editors()
            .filter(|(_, slot)| slot.kind == EditorKind::Launcher)
            .map(|(tab, _)| *tab)
            .filter(|tab| !self.layout().is_pane_front(*tab) && self.layout().tabs_in_pane(*tab).len() > 1)
            .collect();
        for tab in crowded {
            self.close_tab_now(tab, Closing::Forget);
        }
    }

    /// Split the focused pane and start the new one's shell in `cwd`.
    ///
    /// Returns the new pane, or nothing when no shell could be started, in
    /// which case the split is undone rather than left pointing at a pane
    /// that does not exist.
    pub(in crate::window) fn split_pane_at(
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
            .unwrap_or(winter_core::MAX_SCROLLBACK);
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
    pub(in crate::window) fn spawn_grid_size(
        &self,
        tab: TabId,
        direction: Direction,
    ) -> (usize, usize) {
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
    pub(in crate::window) fn close_pane(&mut self, tab_id: TabId) {
        // Edits nowhere but in an editor tab of this pane are the one thing
        // closing it cannot give back on its own, so they are asked about
        // first. The pane closes on the answer, not here.
        if self.ask_before_closing_pane(tab_id) {
            return;
        }
        self.close_pane_now(tab_id);
    }

    /// Close the pane holding `tab_id` without asking, which is what an
    /// answered question and a pane with nothing to lose both come down to.
    /// Its editors' buffers are buried, and its shells end.
    pub(in crate::window) fn close_pane_now(&mut self, tab_id: TabId) {
        // The last pane is the one holding every tab, as closing its last
        // tab would end the window: a hidden dock is somewhere to go back to,
        // as closing the tab alone already found.
        let tabs = self.layout().tabs_in_pane(tab_id);
        if tabs.is_empty() || tabs.len() >= self.layout().members().len() {
            return;
        }
        let mut companions = Vec::new();
        for &tab in &tabs {
            match self.buries(tab, Closing::Keep) {
                true => self.bury_tab(tab),
                false => self.drop_tab_state(tab),
            }
            self.layout_mut().close(tab);
            companions.extend(self.host.commit_companions.remove(&tab));
        }
        // Rebalance the remaining panes' ratios so they stay evenly spaced,
        // without reshaping the tree (closing one pane would otherwise leave
        // its sibling oversized).
        self.layout_mut().balance();
        self.after_tab_change();
        // A commit message's staged diff goes with it, wherever it was moved,
        // as closing the message's tab alone does.
        for companion in companions {
            self.close_tab_now(companion, Closing::Forget);
        }
        // Extensions hear of the files closed at once.
        self.watch_for_extensions();
    }

    /// Close every pane except the one holding `focused` (Vim `C-w o`).
    pub(in crate::window) fn close_other_panes(&mut self, focused: TabId) {
        // One question for the lot of them: asking pane by pane would put a
        // dialog up behind the dialog already waiting to be answered.
        if self.ask_before_closing_others(focused) {
            return;
        }
        self.close_other_panes_now(focused);
    }

    /// The same, once the question has been answered or there was none.
    pub(in crate::window) fn close_other_panes_now(&mut self, focused: TabId) {
        let others: Vec<TabId> = self
            .layout()
            .shown_tabs()
            .into_iter()
            .filter(|id| !self.layout().tabs_in_pane(focused).contains(id))
            .collect();
        for id in others {
            self.close_pane_now(id);
        }
    }

    /// Every tab in the panes shown beside `keep`'s, which closing the other
    /// panes closes: what is asked about and what is closed are this one
    /// list. A hidden dock is not shown, so neither asked about nor closed.
    pub(in crate::window) fn other_panes_tabs(&self, keep: TabId) -> Vec<TabId> {
        let kept = self.layout().tabs_in_pane(keep);
        let mut tabs = Vec::new();
        for shown in self.layout().shown_tabs() {
            if !kept.contains(&shown) {
                tabs.extend(self.layout().tabs_in_pane(shown));
            }
        }
        tabs
    }

    /// Show and focus `tab_id`, wherever it is.
    pub(in crate::window) fn switch_to_tab(&mut self, tab_id: TabId) {
        self.show_tab(tab_id);
        self.dirty = true;
    }

    /// Take note of the children a reload's previous image left that no tab
    /// took over (see [`RuntimeState::strays`]).
    pub(in crate::window) fn note_strays(&mut self) {
        let adopted: Vec<u32> = self
            .open_tabs
            .terminals_mut()
            .filter_map(|(_, terminal)| terminal.child_pid())
            .collect();
        self.runtime.strays = crate::process::child_pids()
            .into_iter()
            .filter(|pid| !adopted.contains(pid))
            .collect();
    }


    /// Close every terminal tab whose shell has exited. The last tab of the
    /// window closing asks for the app to exit.
    ///
    /// Never asked about: a question raised by a shell exiting on its own
    /// would put a dialog up over a tab nobody was closing, and refusing it
    /// would leave the dead tab to ask again on the next frame.
    pub(in crate::window) fn reap_dead_panes(&mut self) {
        self.runtime.reap_strays();
        let mut dead: Vec<TabId> = Vec::new();
        for (id, pane) in self.open_tabs.terminals_mut() {
            match pane.reap() {
                Reap::Close => dead.push(*id),
                Reap::Keep => {}
                Reap::ShowedExit => self.dirty = true,
            }
        }
        for id in dead {
            // One in a workspace put aside goes from that workspace: closing
            // it here would find no such tab, and leave it to be reaped again
            // on every pump.
            if !self.layout().contains(id) {
                self.close_parked_tab(id);
                continue;
            }
            // The window's last tab: with nothing to lose the window goes
            // with it. With edits or programs that would go too, quitting
            // would have to ask, and a no would leave the dead tab to ask
            // again on the next frame; so a fresh shell takes its place and
            // the window stays.
            if self.layout().members().len() <= 1 && self.would_quit_lose_anything() {
                self.new_tab();
                if self.layout().members().len() > 1 {
                    self.set_notice("the last shell exited: the window stays open for what is unsaved or running");
                }
            }
            // A shell that exited was asked to: the tab is not worth keeping.
            self.close_tab_now(id, Closing::Forget);
        }
    }
    pub(in crate::window) fn drain_all_panes(&mut self) -> bool {
        let mut any = false;
        let mut new_entries: Vec<(TabId, crate::terminal::block_queue::BlockEntry)> = Vec::new();
        let mut patched_tiles: Vec<(TabId, usize)> = Vec::new();
        let mut new_titles: Vec<(TabId, String)> = Vec::new();
        let mut exited: Vec<TabId> = Vec::new();
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
        let mut remapped: Vec<(TabId, winter_core::RowRemap)> = Vec::new();
        // What went wrong reading a terminal's output: its parse was reset.
        let mut failures: Vec<String> = Vec::new();
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
                // A program that finished leaves the title it set behind; it
                // goes, before any the shell sets in its place below.
                if terminal.take_program_exit() {
                    exited.push(*id);
                }
            }
            if let Some(why) = terminal.take_failure() {
                failures.push(why);
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
        for why in failures {
            self.set_error(format!("a terminal's output hit an internal error and its reading was reset: {why}"));
            any = true;
        }
        for id in &exited {
            self.tabs.clear_program_title(*id);
        }
        if !new_titles.is_empty() || !exited.is_empty() {
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
            if let Some(cb) = self.input.clipboard() {
                let _ = cb.set_text(&text);
            }
        }
        // OSC 52 reads answer only when `clipboard-read` opted in: the query
        // is silent on the editor's side, so the default must stay a refusal.
        if !clipboard_reads.is_empty() && self.config.clipboard_read {
            let text = self
                .input.clipboard()
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
    pub(in crate::window) fn resize_all_panes(&mut self) {
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
// RuntimeState
// ========================================================================

impl RuntimeState {
    /// Reap the strays that have ended.
    fn reap_strays(&mut self) {
        #[cfg(unix)]
        self.strays.retain(|pid| !crate::process::reap_if_ended(*pid));
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::layout::Layout;
    use crate::window::test_support::*;

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
            winter_core::MAX_SCROLLBACK,
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
    fn test_the_dock_opens_with_a_shell_hides_with_it_running_and_comes_back() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.toggle_dock();
        assert_eq!(app.layout().dock_state(), DockState::Shown);
        let dock = app.layout().focused();
        assert!(app.layout().in_dock(dock) && dock != main);
        assert!(app.open_tabs.terminal(&dock).is_some(), "a shell in it");
        app.toggle_dock();
        assert_eq!(app.layout().dock_state(), DockState::Hidden);
        assert_eq!(app.layout().focused(), main, "the keyboard goes back");
        assert!(app.open_tabs.terminal(&dock).is_some(), "still running");
        app.toggle_dock();
        assert_eq!(app.layout().focused(), dock, "back, with the keyboard");
        app.set_focus(main);
        app.toggle_dock();
        assert_eq!(
            (app.layout().dock_state(), app.layout().focused()),
            (DockState::Shown, dock),
            "shown but without the keyboard: it is given the keyboard"
        );
    }

    #[test]
    fn test_the_dock_remains_unique_after_session_restore_and_toggle() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.toggle_dock();
        assert_eq!(app.layout().dock_state(), DockState::Shown);
        let dock_id = app.layout().focused();
        assert_ne!(dock_id, main);

        // Simulate session capture and restore as happens during reload.
        let tree = app.layout().export_tree();
        let restored_layout = Layout::from_tree(tree, dock_id).expect("rebuilt layout");
        *app.layout_mut() = restored_layout;
        assert_eq!(app.layout().dock_state(), DockState::Shown);

        // Toggling dock while in dock puts it away, does NOT create another dock.
        app.toggle_dock();
        assert_eq!(app.layout().dock_state(), DockState::Hidden);
        assert_eq!(app.layout().shown_tabs().len(), 1, "only main pane shown");

        // Toggling dock again brings it back without duplicate panes.
        app.toggle_dock();
        assert_eq!(app.layout().dock_state(), DockState::Shown);
        assert_eq!(app.layout().shown_tabs().len(), 2, "only main and dock shown");
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
        assert!(!app.runtime.exit.requested);
        assert!(app.open_tabs.terminal(&TabId(0)).is_none(), "and is gone");
    }

    #[test]
    fn test_reap_dead_panes_requests_exit_when_the_last_tab_dies() {
        let mut app = app_with_tabs(1);
        let dead_id = app.layout().shown_tabs()[0];
        insert_dead_pane(&mut app, dead_id);
        app.reap_dead_panes();
        // Nothing else to fall back to, so this requests the app quit rather
        // than leaving a pane whose shell has already exited on screen.
        assert!(app.runtime.exit.requested);
    }

    #[test]
    fn test_alloc_tab_id_is_monotonic() {
        let mut app = App::new();
        assert_eq!(app.alloc_tab_id(), TabId(1));
        assert_eq!(app.alloc_tab_id(), TabId(2));
    }

    /// The launcher in tab `tab`'s directory, or `None` where the tab is no
    /// launcher.
    fn launcher_dir(app: &App, tab: TabId) -> Option<PathBuf> {
        app.open_tabs
            .editor(&tab)
            .filter(|slot| slot.kind == EditorKind::Launcher)
            .and_then(|slot| slot.editor.cwd())
    }

    #[test]
    fn test_a_split_holds_its_pane_with_a_launcher_where_the_tab_split_from_works() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.show_editor(EditorKind::Dir, Box::new(DirEditor::new(std::env::temp_dir())));
        let dir = app.focused_start_dir();
        app.split_pane(Direction::Vertical);
        let launcher = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(launcher_dir(&app, launcher), Some(dir));
        assert!(app.open_tabs.terminal(&launcher).is_none(), "no shell started");
    }

    #[test]
    fn test_choosing_a_terminal_opens_it_in_the_launchers_own_tab() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.split_pane(Direction::Horizontal);
        let launcher = app.layout().focused();
        app.launch_from(launcher, Launch::Terminal);
        assert!(app.open_tabs.terminal(&launcher).is_some());
        assert_eq!(app.layout().tabs_in_pane(launcher), vec![launcher], "in place");
    }

    #[test]
    fn test_a_tab_opened_in_a_launchers_pane_takes_its_place_for_good() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.split_pane(Direction::Vertical);
        let launcher = app.layout().focused();
        let dir = std::env::temp_dir();
        let listing = app.show_editor(EditorKind::Dir, Box::new(DirEditor::new(dir)));
        assert_eq!(app.layout().tabs_in_pane(listing), vec![listing]);
        assert!(!app.layout().contains(launcher));
        assert!(app.buried_tabs().is_empty(), "nothing to come back to");
        assert!(launcher_dir(&app, launcher).is_none());
    }

    #[test]
    fn test_a_tab_added_behind_a_launcher_leaves_it_in_sight() {
        // A quiet build's tab went into the launcher's pane behind it, and
        // the next tab change anywhere closed the launcher on screen.
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.split_pane(Direction::Vertical);
        let launcher = app.layout().focused();
        let behind = app.add_background_editor(EditorKind::Dir, Box::new(DirEditor::new(std::env::temp_dir())));
        app.after_tab_change();
        assert!(app.layout().contains(launcher), "still holding its pane");
        assert!(app.layout().is_pane_front(launcher));

        app.show_tab(behind);
        assert!(!app.layout().contains(launcher), "shown over, it goes");
    }

    #[test]
    fn test_closing_a_commit_message_s_pane_closes_its_diff_elsewhere() {
        // Closing the pane skipped what closing the tab does: the staged diff
        // moved to another pane stayed open, its companion entry leaking.
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.split_pane_with_terminal(Direction::Vertical);
        let message = app.layout().focused();
        app.split_pane_with_terminal(Direction::Vertical);
        let diff = app.layout().focused();
        app.host.commit_companions.insert(message, diff);
        app.close_pane_now(message);
        assert!(!app.layout().contains(diff), "the diff went with it");
        assert!(app.host.commit_companions.is_empty());
    }

    /// An app showing one terminal pane beside a dock it has hidden: the
    /// pane's tab and the dock's.
    fn pane_beside_a_hidden_dock() -> (App, TabId, TabId) {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        let dock = app.alloc_tab_id();
        app.open_tabs.insert_terminal(dock, idle_terminal());
        app.layout_mut().open_dock(dock, 0.3);
        app.layout_mut().focus(main);
        app.split_pane_with_terminal(Direction::Vertical);
        app.layout_mut().hide_dock();
        (app, main, dock)
    }

    #[test]
    fn test_a_pane_beside_a_hidden_dock_closes_as_its_last_tab_would() {
        // The pane close took the one pane shown as the last, and refused,
        // where closing its tab alone went back to the dock.
        let (mut app, main, dock) = pane_beside_a_hidden_dock();
        let beside = app.layout().focused();
        app.close_pane_now(beside);
        assert!(!app.layout().contains(beside));
        app.close_pane_now(main);
        assert!(!app.layout().contains(main), "the dock is somewhere to go back to");
        assert!(app.layout().contains(dock));
        app.close_pane_now(dock);
        assert!(app.layout().contains(dock), "the pane holding every tab stays");
    }

    #[test]
    fn test_closing_other_panes_asks_about_and_closes_the_same_tabs() {
        // The question gathered every tab, a hidden dock's too, which the
        // close then left alone.
        let (app, main, dock) = pane_beside_a_hidden_dock();
        let beside = app.layout().focused();
        assert_eq!(app.other_panes_tabs(beside), [main]);
        assert!(!app.other_panes_tabs(main).contains(&dock));
    }

    #[test]
    fn test_splitting_a_zoomed_pane_leaves_the_pane_split_from_in_sight() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.open_tabs.insert_terminal(main, idle_terminal());
        app.split_pane_with_terminal(Direction::Vertical);
        app.layout_mut().toggle_zoom();
        app.split_pane(Direction::Horizontal);
        assert_eq!(app.layout().shown_tabs().len(), 3);
    }
}

//! Restore a saved session: rebuild the split layout and respawn PTYs.

use portable_pty::CommandBuilder;

use crate::model::layout::{Layout, TabId};
use crate::session::Session;
use crate::terminal::session::{Terminal, MUX_COMMAND_PREFIX, MUX_REMOTE_COMMAND_PREFIX};

use crate::window::appearance::terminal_presentation;
use crate::window::{content_rows, App, DEFAULT_COLS, DEFAULT_ROWS};

// ========================================================================
// App: session restore
// ========================================================================

impl App {
    /// Replace the current layout with the saved `session`'s split tree,
    /// reopening each terminal tab at its saved cwd, and enter the workspace
    /// it was in. Returns `true` when the session was applied.
    pub(in crate::window) fn restore_saved_session(&mut self, session: Session) -> bool {
        // The workspace first, as a switch to it does: its settings (its
        // shell, its scrollback) are the ones the restored terminals start
        // with.
        self.enter_workspace(session.workspace.clone());
        self.restore_session(session)
    }

    /// Replace the current layout with `session`'s split tree and reopen each
    /// of its tabs. Returns `true` when anything of it could be opened; when
    /// nothing could, the window is left as it was.
    pub(in crate::window) fn restore_session(&mut self, mut session: Session) -> bool {
        // Programs the previous process image left running and handed on are
        // taken over as they are, ahead of anything that would start a new one.
        let mut handovers = session.take_handovers();
        let editors = session.take_editors();
        let dock_hidden = session.dock_hidden;
        let (tree, focused, terminal_map) = session.into_parts();

        let (cols, rows) = if let Some(r) = &self.renderer {
            r.grid_size()
        } else {
            (DEFAULT_COLS as usize, DEFAULT_ROWS as usize)
        };
        let want_rows = content_rows(rows);

        // Spawn a PTY for every pane in the session. A pane whose saved
        // command and the configured-shell fallback both fail to spawn (e.g.
        // the saved command no longer exists on this machine) is dropped
        // from the restored layout instead of crashing the whole restore.
        let max_scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_core::MAX_SCROLLBACK);
        let shell = self.config.active_shell().map(String::from);
        let mut failed_ids: Vec<TabId> = Vec::new();
        for (id, (cmd, cwd)) in &terminal_map {
            let adopted = handovers
                .remove(id)
                .and_then(|handover| self.adopt_handed_over(handover, cmd.clone(), max_scrollback));
            let terminal = if adopted.is_some() {
                adopted
            } else if let Some(host_session) = cmd
                .as_deref()
                .and_then(|c| c.strip_prefix(MUX_REMOTE_COMMAND_PREFIX))
            {
                // A saved remote-mux terminal re-attaches over ssh instead of
                // spawning a local shell.
                match host_session.split_once('|') {
                    Some((host, session)) => match Terminal::new_mux_remote(
                        host,
                        cols.max(1),
                        want_rows.max(1),
                        session,
                        max_scrollback,
                    ) {
                        Ok(terminal) => Some(terminal),
                        Err(e) => {
                            self.set_error(format!(
                                "re-attaching to '{host}:{session}' failed ({e})"
                            ));
                            None
                        }
                    },
                    // Said, as every other way a pane fails to come back is:
                    // dropped quietly, it just went missing.
                    None => {
                        self.set_error(format!(
                            "re-attaching to a remote mux session failed: no host in '{host_session}'"
                        ));
                        None
                    }
                }
            } else if let Some(session) = cmd
                .as_deref()
                .and_then(|c| c.strip_prefix(MUX_COMMAND_PREFIX))
            {
                // A saved mux pane re-attaches to its session instead of
                // spawning a local shell.
                match Terminal::new_mux(cols.max(1), want_rows.max(1), session, max_scrollback) {
                    Ok(terminal) => Some(terminal),
                    Err(e) => {
                        self.set_error(format!(
                            "re-attaching to mux session '{session}' failed ({e})"
                        ));
                        None
                    }
                }
            } else {
                // The configured shell must win over a pane's saved command, otherwise a `shell`/`shell-windows`/etc. change in settings.kdl is silently ignored on every restore since the session always has a saved value.
                let executable = shell.as_deref().or(cmd.as_deref()).unwrap_or("/bin/sh");
                let mut builder = CommandBuilder::new(executable);
                if let Some(dir) = cwd {
                    builder.cwd(dir);
                }
                match Terminal::with_command(cols.max(1), want_rows.max(1), builder, max_scrollback)
                {
                    Ok(terminal) => Some(terminal),
                    Err(e) => {
                        self.set_error(format!(
                            "restoring a pane failed ({e}), trying the default shell"
                        ));
                        self.spawn_terminal_or_notify(
                            cols.max(1),
                            want_rows.max(1),
                            shell.as_deref(),
                            max_scrollback,
                            cwd.as_deref(),
                        )
                    }
                }
            };
            match terminal {
                Some(mut terminal) => {
                    if let Some(renderer) = &self.renderer {
                        terminal.set_presentation(terminal_presentation(renderer));
                    }
                    self.open_tabs.insert_terminal(*id, terminal);
                }
                None => failed_ids.push(*id),
            }
        }
        // The editor tabs come back where they were looking; one that cannot
        // (its file gone, say) is dropped like a terminal that would not spawn.
        let restored: Vec<TabId> = editors
            .iter()
            .filter_map(|saved| {
                let id = TabId(saved.id as u64);
                self.restore_editor(id, saved).then_some(id)
            })
            .collect();
        // A terminal that could not be respawned is dropped from the layout,
        // and a pane left with none collapses into its sibling.
        let spawned = |id: TabId| {
            (terminal_map.contains_key(&id) && !failed_ids.contains(&id)) || restored.contains(&id)
        };
        let layout = Layout::from_tree(tree.retain(&spawned), focused);
        // A terminal the layout does not place would run on unseen.
        for id in terminal_map.keys() {
            if !layout.as_ref().is_some_and(|l| l.contains(*id)) && !failed_ids.contains(id) {
                self.open_tabs.remove(id);
            }
        }
        // A buried buffer comes back buried; any other editor the layout does
        // not place goes.
        let buried: Vec<TabId> = editors
            .iter()
            .filter(|saved| saved.buried)
            .map(|saved| TabId(saved.id as u64))
            .collect();
        // Nothing to lay out leaves the window as it was: buried editors
        // kept from the session would hold ids the window goes on to hand
        // out again, and a new tab under one would overwrite it.
        if layout.is_none() {
            for id in &restored {
                self.open_tabs.remove_editor(id);
            }
            return false;
        }
        for id in &restored {
            if layout.as_ref().is_some_and(|l| l.contains(*id)) {
                continue;
            }
            match buried.contains(id) {
                true => self.open_tabs.bury(*id),
                false => {
                    self.open_tabs.remove_editor(id);
                }
            }
        }
        let Some(mut layout) = layout else {
            return false;
        };
        if dock_hidden {
            layout.hide_dock();
        }

        // Replace the bootstrap pane that init_window laid out, with no shell
        // in it when a session was there to restore, and move the
        // pane-id counter past the restored ids so future alloc_tab_id()
        // calls don't collide with them.
        // Every tab of the layout replaced goes, not only the one focused: a
        // caller leaving more than its bootstrap there would otherwise leave
        // tabs running that no pane shows.
        for replaced in self.layout().members() {
            if !layout.contains(replaced) {
                self.drop_tab_state(replaced);
            }
        }
        self.tabs.layout = layout;
        let max_id = terminal_map
            .keys()
            .chain(restored.iter())
            .map(|id| id.0)
            .max()
            .unwrap_or(0);
        if max_id >= self.next_tab_id {
            self.next_tab_id = max_id + 1;
        }

        self.resize_all_panes();
        // Every program taken over is asked to draw itself again at the size it
        // now has, whatever it was drawing when the screen was copied.
        for (_, terminal) in self.open_tabs.terminals_mut() {
            if terminal.take_adopted() {
                terminal.nudge_redraw();
            }
        }
        self.dirty = true;
        true
    }

    /// The pane for a program the previous process image handed over: its pty
    /// and child adopted, and its screen built again from the dump taken then.
    /// `None` when the program cannot be adopted, which leaves the pane to be
    /// started fresh like any other.
    #[cfg(unix)]
    fn adopt_handed_over(
        &mut self,
        handover: crate::session::TerminalHandover,
        command: Option<String>,
        max_scrollback: usize,
    ) -> Option<Terminal> {
        let command = command.unwrap_or_default();
        match Terminal::adopt(
            handover.cols.max(1),
            handover.rows.max(1),
            handover.fd,
            handover.pid,
            max_scrollback,
            command,
        ) {
            Ok(mut terminal) => {
                terminal.restore_screen(handover.screen.as_bytes());
                terminal.mark_adopted();
                Some(terminal)
            }
            Err(e) => {
                // Not taken over, the program is hung up rather than left
                // running with no one reading its terminal. Its descriptor is
                // not closed here: a failed adopt may have taken and closed
                // it already, and the number may name another file by now.
                crate::process::hang_up(handover.pid);
                self.set_error(format!("keeping a running program failed ({e})"));
                None
            }
        }
    }

    #[cfg(not(unix))]
    fn adopt_handed_over(
        &mut self,
        _handover: crate::session::TerminalHandover,
        _command: Option<String>,
        _max_scrollback: usize,
    ) -> Option<Terminal> {
        None
    }
}

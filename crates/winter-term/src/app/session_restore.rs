//! Restore a saved session: rebuild the split layout and respawn PTYs.

use portable_pty::CommandBuilder;

use crate::model::layout::{Layout, PaneId};
use crate::model::mode::Mode;
use crate::session::Session;
use crate::terminal::pane::{Pane, MUX_COMMAND_PREFIX, MUX_REMOTE_COMMAND_PREFIX};

use super::{content_rows, App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH, DEFAULT_COLS, DEFAULT_ROWS};

// ========================================================================
// App: session restore
// ========================================================================

impl App {
    /// If a saved session exists, replace the current layout with the persisted
    /// split tree and reopen each terminal tab at its saved cwd. Returns `true`
    /// when a session was applied.
    pub(crate) fn restore_session_if_present(&mut self) -> bool {
        let Some(session) = Session::load() else {
            return false;
        };
        let (tree, focused, pane_map) = session.into_parts();

        let (cols, rows) = if let Some(r) = &self.renderer {
            r.grid_size()
        } else {
            (DEFAULT_COLS as usize, DEFAULT_ROWS as usize)
        };
        let (cw, ch) = if let Some(r) = &self.renderer {
            r.cell_size()
        } else {
            (APPROX_CELL_WIDTH as f32, APPROX_CELL_HEIGHT as f32)
        };
        let want_rows = content_rows(rows);

        // Spawn a PTY for every pane in the session. A pane whose saved
        // command and the configured-shell fallback both fail to spawn (e.g.
        // the saved command no longer exists on this machine) is dropped
        // from the restored layout instead of crashing the whole restore.
        let max_scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_render::MAX_SCROLLBACK);
        let shell = self.config.active_shell().map(String::from);
        let mut failed_ids: Vec<PaneId> = Vec::new();
        for (id, (cmd, cwd)) in &pane_map {
            let pane = if let Some(host_session) = cmd
                .as_deref()
                .and_then(|c| c.strip_prefix(MUX_REMOTE_COMMAND_PREFIX))
            {
                // A saved remote-mux pane re-attaches over ssh instead of
                // spawning a local shell.
                match host_session.split_once('|') {
                    Some((host, session)) => match Pane::new_mux_remote(
                        host,
                        cols.max(1),
                        want_rows.max(1),
                        session,
                        max_scrollback,
                    ) {
                        Ok(pane) => Some(pane),
                        Err(e) => {
                            self.set_error(format!(
                                "re-attaching to '{host}:{session}' failed ({e})"
                            ));
                            None
                        }
                    },
                    None => None,
                }
            } else if let Some(session) = cmd
                .as_deref()
                .and_then(|c| c.strip_prefix(MUX_COMMAND_PREFIX))
            {
                // A saved mux pane re-attaches to its session instead of
                // spawning a local shell.
                match Pane::new_mux(cols.max(1), want_rows.max(1), session, max_scrollback) {
                    Ok(pane) => Some(pane),
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
                match Pane::with_command(cols.max(1), want_rows.max(1), builder, max_scrollback) {
                    Ok(pane) => Some(pane),
                    Err(e) => {
                        self.set_error(format!(
                            "restoring a pane failed ({e}), trying the default shell"
                        ));
                        self.spawn_pane_or_notify(
                            cols.max(1),
                            want_rows.max(1),
                            shell.as_deref(),
                            max_scrollback,
                            cwd.as_deref(),
                        )
                    }
                }
            };
            match pane {
                Some(mut pane) => {
                    pane.set_cell_size(cw, ch);
                    self.panes.insert(*id, pane);
                    self.modes.insert(*id, Mode::default());
                }
                None => failed_ids.push(*id),
            }
        }
        // A terminal that could not be respawned is dropped from the layout,
        // and a group left with none collapses into its sibling.
        let spawned = |id: PaneId| pane_map.contains_key(&id) && !failed_ids.contains(&id);
        let layout = Layout::from_tree(tree.retain(&spawned), focused);
        // A terminal the layout does not place would run on unseen.
        for id in pane_map.keys() {
            if !layout.as_ref().is_some_and(|l| l.contains(*id)) && !failed_ids.contains(id) {
                self.panes.remove(id);
                self.modes.remove(id);
            }
        }
        let Some(layout) = layout else {
            return false;
        };

        // Replace the bootstrap pane that init_window created, and move the
        // pane-id counter past the restored ids so future alloc_pane_id()
        // calls don't collide with them.
        let bootstrap_id = self.layout().focused();
        if !layout.contains(bootstrap_id) {
            self.panes.remove(&bootstrap_id);
            self.modes.remove(&bootstrap_id);
        }
        self.tabs.layout = layout;
        let max_id = pane_map.keys().map(|id| id.0).max().unwrap_or(0);
        if max_id >= self.next_pane_id {
            self.next_pane_id = max_id + 1;
        }

        self.resize_all_panes();
        self.dirty = true;
        true
    }
}

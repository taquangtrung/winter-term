//! Command-palette input, and the pickers layered on top of it.

use winit::keyboard::{Key, NamedKey, PhysicalKey};

use crate::model::input::{self, EditBinding};
use crate::model::layout::PaneId;
use crate::model::mode::Mode;
use crate::model::palette::{Palette, PaletteMode};

use super::navigation;
use super::App;
use super::{parse_mux_attach_query, parse_mux_spawn_query, winit_key_to_code};

// ========================================================================
// App: command palette
// ========================================================================

impl App {
    pub(crate) fn handle_palette_input(
        &mut self,
        palette: &mut Palette,
        key: &Key,
        physical: &PhysicalKey,
        focused: PaneId,
    ) {
        // Prompt undo/redo chords (default `Ctrl-/` / `Ctrl-\`) also drive the
        // palette query history, resolved through the configurable keymap.
        let mods = self.modifiers.state();
        let model_key = input::Key {
            alt: mods.alt_key(),
            code: winit_key_to_code(key, physical),
            ctrl: mods.control_key(),
            shift: mods.shift_key(),
        };
        match self.window_keymap.edit_binding(&model_key) {
            Some(EditBinding::Undo) => {
                palette.undo();
                return;
            }
            Some(EditBinding::Redo) => {
                palette.redo();
                return;
            }
            _ => {}
        }
        if mods.control_key() {
            if let Key::Character(c) = key.as_ref() {
                let lower = c.to_lowercase();
                if lower == "n" {
                    palette.move_down();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                } else if lower == "p" {
                    palette.move_up();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                }
            }
        }
        if mods.alt_key() {
            if let Key::Character(c) = key.as_ref() {
                let lower = c.to_lowercase();
                if lower == "p" {
                    palette.history_prev();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                } else if lower == "n" {
                    palette.history_next();
                    if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                }
            }
        }
        match key {
            Key::Named(NamedKey::Escape) => {
                if palette.mode == PaletteMode::Swoop {
                    if let Some((pid, (r, c))) = self.swoop_initial_cursor.take() {
                        if pid == focused {
                            self.nav_cursors.insert(focused, (r, c));
                            self.reveal_position(focused, (r, c));
                        }
                    }
                }
                palette.close();
                self.palette = None;
                self.dirty = true;
                return;
            }
            Key::Named(NamedKey::Enter) => {
                self.confirm_palette_selection(palette, focused);
                palette.close();
                self.palette = None;
                self.dirty = true;
                return;
            }
            Key::Named(NamedKey::Backspace) => {
                palette.pop_char();
            }
            Key::Named(NamedKey::ArrowUp) => {
                palette.move_up();
            }
            Key::Named(NamedKey::ArrowDown) => {
                palette.move_down();
            }
            Key::Character(c) => {
                // In the pane switcher, the digit shown next to an entry
                // (e.g. "2") jumps straight to it instead of filtering.
                let shortcut_hit = (palette.mode == PaletteMode::Panes)
                    .then(|| palette.position_by_shortcut(c))
                    .flatten();
                if let Some(pos) = shortcut_hit {
                    palette.selected = pos;
                    self.confirm_palette_selection(palette, focused);
                    palette.close();
                    self.palette = None;
                    self.dirty = true;
                    return;
                } else {
                    for ch in c.chars() {
                        palette.push_char(ch);
                    }
                }
            }
            _ => {}
        }
        if palette.mode == PaletteMode::Swoop {
            self.update_swoop_preview(palette, focused);
        }
    }
    /// Act on the palette's currently selected entry (`Enter`, or a pane
    /// switcher digit-jump): run a command, replay shell history, `cd` into a
    /// recent directory, or switch to a pane, depending on `palette.mode`.
    pub(crate) fn confirm_palette_selection(&mut self, palette: &Palette, focused: PaneId) {
        self.record_palette_query(&palette.query);
        let action = palette.selected_action().map(str::to_string);
        match palette.mode {
            PaletteMode::Commands => {
                if let Some(action) = action {
                    self.run_command(&action, focused);
                }
            }
            PaletteMode::History => {
                if let Some(cmd) = action {
                    if let Some(pane) = self.panes.get_mut(&focused) {
                        pane.write(cmd.as_bytes());
                    }
                }
            }
            PaletteMode::RecentDirs => {
                if let Some(dir) = action {
                    // Reject paths containing control characters — a
                    // malicious OSC 7 sequence could embed a newline to
                    // inject a second shell command.
                    let safe = !dir.chars().any(|c| c.is_control());
                    if safe {
                        if let Some(pane) = self.panes.get_mut(&focused) {
                            // Single-quote the path so shell metacharacters
                            // in the directory name are inert. The only
                            // character that cannot appear inside single
                            // quotes is `'` itself, escaped as `'\''`.
                            let escaped = dir.replace('\'', "'\\''");
                            let cmd = format!("cd '{}'\n", escaped);
                            pane.write(cmd.as_bytes());
                        }
                    }
                }
            }
            PaletteMode::Panes => {
                if let Some(pane_id_str) = action {
                    if let Ok(pane_id_val) = pane_id_str.parse::<u64>() {
                        self.switch_to_pane(PaneId(pane_id_val));
                    }
                }
            }
            PaletteMode::Swoop => {
                if let Some(action) = action {
                    if let Ok(abs_row) = action.parse::<usize>() {
                        if let Some((_pid, origin)) = self.swoop_initial_cursor.take() {
                            self.vim.jump_lists.entry(focused).or_default().push(origin);
                        }
                        self.nav_cursors.insert(focused, (abs_row, 0));
                        self.reveal_position(focused, (abs_row, 0));
                        self.modes.insert(focused, Mode::Normal);
                    }
                }
            }
            PaletteMode::MuxSessions => {
                if let Some(session_entry) = action {
                    let session_name = session_entry
                        .split_whitespace()
                        .next()
                        .unwrap_or(&session_entry);
                    if !session_name.starts_with('(') {
                        self.new_mux_tab(session_name);
                    }
                }
            }
            PaletteMode::MuxKill => {
                if let Some(session_entry) = action {
                    let session_name = session_entry
                        .split_whitespace()
                        .next()
                        .unwrap_or(&session_entry);
                    if !session_name.starts_with('(') {
                        let sock_path = crate::mux::server::default_socket_path();
                        if let Ok(mut client) = crate::mux::client::MuxClient::connect(&sock_path) {
                            let _ = client.kill(session_name);
                            self.set_notice(format!("killed mux session '{session_name}'"));
                        } else {
                            self.set_error("could not connect to mux daemon");
                        }
                    }
                }
            }
            PaletteMode::MuxNew => {
                // The query is the input: "name [command...]".
                match parse_mux_spawn_query(&palette.query) {
                    Some((name, command)) => {
                        // Start where the user is: the focused pane's OSC 7
                        // cwd, when the shell has reported one.
                        let cwd = self.panes.get(&focused).and_then(|pane| pane.cwd());
                        self.spawn_mux_tab_at(
                            &crate::mux::server::default_socket_path(),
                            &name,
                            command.as_deref(),
                            cwd.as_deref(),
                            focused,
                        );
                    }
                    None => self.set_error("mux new: usage: name [command]"),
                }
            }
            PaletteMode::MuxAttachRemote => {
                // The query is the input: "host [session]".
                match parse_mux_attach_query(&palette.query) {
                    Some((host, session)) => {
                        self.new_mux_tab_remote_at(&host, session.as_deref().unwrap_or("default"));
                    }
                    None => self.set_error("mux attach: usage: host [session]"),
                }
            }
        }
    }
    pub(crate) fn record_palette_query(&mut self, query: &str) {
        let trimmed = query.trim();
        if trimmed.is_empty() {
            return;
        }
        self.palette_history.retain(|q| q != trimmed);
        self.palette_history.insert(0, trimmed.to_string());
        if self.palette_history.len() > 100 {
            self.palette_history.truncate(100);
        }
        self.save_app_state();
    }
    /// Query the mux server's session list for a palette, formatted as
    /// `name (colsxrows, up UPTIME, N attached) - command` entries whose
    /// first word is the session name; placeholder entries start with `(`
    /// so the palette's confirm handler skips them. `empty_fallback` is
    /// offered when the server holds no sessions, since attaching to a
    /// missing session creates it.
    pub(crate) fn mux_session_entries(&self, empty_fallback: &str) -> Vec<String> {
        let sock_path = crate::mux::server::default_socket_path();
        let mut client = match crate::mux::client::MuxClient::connect(&sock_path) {
            Ok(client) => client,
            Err(_) => {
                return vec!["(no daemon running — start with 'winter mux serve')".to_string()]
            }
        };
        // The server answers on its own poll cycle, so the query polls with
        // a deadline rather than racing it with one nonblocking read (which
        // always saw nothing and fell back to placeholder data). The wait
        // is bounded: this runs on the UI thread while the palette opens.
        match client.query_sessions(std::time::Duration::from_millis(200)) {
            Ok(sessions) if sessions.is_empty() => vec![empty_fallback.to_string()],
            Ok(sessions) => {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                sessions
                    .iter()
                    .map(|s| {
                        let uptime = crate::mux::protocol::format_uptime(s.created, now);
                        format!(
                            "{} ({}x{}, up {uptime}, {} attached) - {}",
                            s.name, s.cols, s.rows, s.attach_count, s.command
                        )
                    })
                    .collect()
            }
            Err(_) => vec!["(could not query sessions)".to_string()],
        }
    }
    /// Open the command palette to list or attach running background mux sessions.
    pub(crate) fn open_mux_palette(&mut self) {
        let sessions = self.mux_session_entries("default (80x24)");
        self.palette = Some(
            Palette::open_mux_sessions(sessions).with_query_history(self.palette_history.clone()),
        );
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Open the command palette to select a background mux session to kill.
    pub(crate) fn open_mux_kill_palette(&mut self) {
        let sessions = self.mux_session_entries("(no running sessions)");
        self.palette =
            Some(Palette::open_mux_kill(sessions).with_query_history(self.palette_history.clone()));
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Buffer swoop: open fuzzy line search over the focused pane's grid and scrollback.
    pub(crate) fn open_swoop(&mut self, focused: PaneId) {
        let Some(pane) = self.panes.get(&focused) else {
            return;
        };
        let initial = self.nav_cursor(focused).or_else(|| {
            Some((
                pane.grid().to_absolute_row(pane.grid().cursor().0),
                pane.grid().cursor().1,
            ))
        });
        if let Some(pos) = initial {
            self.swoop_initial_cursor = Some((focused, pos));
        }
        self.modes.insert(focused, Mode::Normal);
        let lines = navigation::swoop::extract_swoop_lines(pane.grid());
        let palette = Palette::open_swoop(lines).with_query_history(self.palette_history.clone());
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.nav_cursors.insert(focused, (abs_row, 0));
                self.reveal_position(focused, (abs_row, 0));
            }
        }
        self.palette = Some(palette);
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Update the live preview cursor during Buffer Swoop.
    pub(crate) fn update_swoop_preview(&mut self, palette: &Palette, focused: PaneId) {
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.nav_cursors.insert(focused, (abs_row, 0));
                self.reveal_position(focused, (abs_row, 0));
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
    }
}

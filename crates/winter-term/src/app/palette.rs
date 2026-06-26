//! Command-palette input, and the pickers layered on top of it.

use std::path::PathBuf;

use winit::keyboard::{Key, NamedKey, PhysicalKey};

use crate::model::input::{self, EditBinding};
use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::model::palette::{Palette, PaletteMode};
use crate::model::textedit::modal::EditOutcome;
use crate::model::tool;

use super::navigation;
use super::overlay::PaletteOverlay;
use super::tool::ActivePick;
use super::window_event::winit_key_to_code;
use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How many palette queries the history keeps.
const MAX_PALETTE_HISTORY: usize = 100;

/// The longest query kept in the history, in bytes.
const MAX_PALETTE_QUERY_BYTES: usize = 1024;

// ========================================================================
// App: command palette
// ========================================================================

impl App {
    /// Route one key into the open palette, if one is open: it is taken out
    /// while the key is worked on, and put back unless the key closed it or
    /// opened something else in its place.
    pub(crate) fn press_palette_key(&mut self, key: &Key, physical: &PhysicalKey, focused: TabId) {
        let Some(mut overlay) = self.take_palette_overlay() else {
            return;
        };
        let query = overlay.palette.query.clone();
        self.handle_palette_input(&mut overlay, key, physical, focused);
        let live = overlay.palette.active && overlay.palette.mode == PaletteMode::LiveSearch;
        if live && overlay.palette.query != query {
            self.on_live_query(&mut overlay.palette);
        }
        self.restore_palette_overlay(overlay);
    }

    fn handle_palette_input(
        &mut self,
        overlay: &mut PaletteOverlay,
        key: &Key,
        physical: &PhysicalKey,
        focused: TabId,
    ) {
        let palette = &mut overlay.palette;
        // Prompt undo/redo chords (default `C-/` / `C-\`) also drive the
        // palette query history, resolved through the configurable keymap.
        let mods = self.modifiers.state();
        let model_key = input::Key {
            alt: mods.alt_key(),
            code: winit_key_to_code(key, physical),
            ctrl: mods.control_key(),
            shift: mods.shift_key(),
            sup: mods.super_key(),
        };
        // `C-_` is Emacs' own undo beside the bound one, and walks back the
        // same query history: the field's undo knows only its own edits.
        let emacs_undo = model_key.ctrl && model_key.glyph() == input::KeyCode::Char('_');
        match self.keymap.edit_binding(&model_key) {
            _ if emacs_undo => {
                palette.undo();
                return;
            }
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
                // In the browser `C-h` goes up to the parent directory;
                // `Backspace` stays a plain delete of the typed character.
                if lower == "h" {
                    if let Some(up) = self.step_out(palette) {
                        *palette = up;
                        self.dirty = true;
                        return;
                    }
                }
                if lower == "n" {
                    palette.move_down();
                    if palette.mode == PaletteMode::Jump {
                        self.update_jump_preview(palette, focused);
                    } else if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                } else if lower == "p" {
                    palette.move_up();
                    if palette.mode == PaletteMode::Jump {
                        self.update_jump_preview(palette, focused);
                    } else if palette.mode == PaletteMode::Swoop {
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
                    if palette.mode == PaletteMode::Jump {
                        self.update_jump_preview(palette, focused);
                    } else if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                } else if lower == "n" {
                    palette.history_next();
                    if palette.mode == PaletteMode::Jump {
                        self.update_jump_preview(palette, focused);
                    } else if palette.mode == PaletteMode::Swoop {
                        self.update_swoop_preview(palette, focused);
                    }
                    return;
                }
            }
        }
        // Typing at the query's end appends, as it always has, so `/` still
        // enters a directory in the browser; any other key edits the query at
        // the caret with a field's Emacs keys.
        let plain = !mods.control_key() && !mods.alt_key() && !mods.super_key();
        let at_end = palette.is_caret_at_end();
        // In the browser `Tab` takes the selected row as `Enter` does: into a
        // directory, or open a file.
        let accept = matches!(key, Key::Named(NamedKey::Enter))
            || (palette.mode == PaletteMode::Files
                && plain
                && matches!(key, Key::Named(NamedKey::Tab)));
        match key {
            Key::Named(NamedKey::Escape) => {
                self.dismiss_palette(palette, focused);
                return;
            }
            _ if accept => {
                // In the browser a directory is somewhere to go rather than
                // something to open: the palette stays up, rooted there.
                if let Some(next) = self.step_into_selection(palette) {
                    *palette = next;
                    self.dirty = true;
                    return;
                }
                self.confirm_palette_selection(&overlay.palette, overlay.pick.take(), focused);
                overlay.palette.close();
                self.dirty = true;
                return;
            }
            Key::Named(NamedKey::Backspace) if plain && at_end => {
                palette.pop_char();
            }
            Key::Named(NamedKey::ArrowUp) => {
                palette.move_up();
            }
            Key::Named(NamedKey::ArrowDown) => {
                palette.move_down();
            }
            // In the browser Left and Right go up a directory and into one;
            // `C-b` and `C-f` move the caret there.
            Key::Named(NamedKey::ArrowLeft) if palette.mode == PaletteMode::Files => {
                if let Some(up) = self.step_out(palette) {
                    *palette = up;
                    self.dirty = true;
                    return;
                }
            }
            Key::Named(NamedKey::ArrowRight) if palette.mode == PaletteMode::Files => {
                if let Some(next) = self.step_into_selection(palette) {
                    *palette = next;
                    self.dirty = true;
                    return;
                }
            }
            Key::Character(c) if plain && at_end => {
                // In the pane switcher, the digit shown next to an entry
                // (e.g. "2") jumps straight to it instead of filtering.
                let shortcut_hit = (palette.mode == PaletteMode::Panes)
                    .then(|| palette.position_by_shortcut(c))
                    .flatten();
                if let Some(pos) = shortcut_hit {
                    palette.selected = pos;
                    self.confirm_palette_selection(&overlay.palette, overlay.pick.take(), focused);
                    overlay.palette.close();
                    self.dirty = true;
                    return;
                } else {
                    for ch in c.chars() {
                        palette.push_char(ch);
                    }
                }
            }
            _ => {
                if palette.edit_query(&model_key.typed()) == EditOutcome::Cancel {
                    self.dismiss_palette(palette, focused);
                    return;
                }
            }
        }
        if palette.mode == PaletteMode::Jump {
            self.update_jump_preview(palette, focused);
        } else if palette.mode == PaletteMode::Swoop {
            self.update_swoop_preview(palette, focused);
        }
    }
    /// Put the palette away unanswered (`Esc`, `C-g`): a jump or a swoop
    /// puts the cursor back where it was, and a tool waiting on a choice is
    /// told there was none.
    fn dismiss_palette(&mut self, palette: &mut Palette, focused: TabId) {
        if palette.mode == PaletteMode::Jump {
            self.cancel_jump(focused);
        } else if palette.mode == PaletteMode::Swoop {
            if let Some((pid, (r, c))) = self.swoop_initial_cursor.take() {
                if pid == focused {
                    self.set_nav_cursor(focused, (r, c));
                    self.reveal_position(focused, (r, c));
                }
            }
        }
        // A tool waiting on a choice is told there was none, the way
        // an escaped prompt answers with nothing, when the palette is
        // put away.
        palette.close();
        self.dirty = true;
    }

    /// Act on the palette's currently selected entry (`Enter`, or a pane
    /// switcher digit-jump): run a command, replay shell history, `cd` into a
    /// recent directory, or switch to a pane, depending on `palette.mode`.
    /// `pick` is the tool question a [`PaletteMode::ToolPick`] list answers.
    pub(crate) fn confirm_palette_selection(
        &mut self,
        palette: &Palette,
        pick: Option<ActivePick>,
        focused: TabId,
    ) {
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
                    if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                        terminal.write(cmd.as_bytes());
                    }
                }
            }
            PaletteMode::RecentDirs => {
                if let Some(dir) = action {
                    // Reject paths containing control characters: a
                    // malicious OSC 7 sequence could embed a newline to
                    // inject a second shell command.
                    let safe = !dir.chars().any(|c| c.is_control());
                    if safe {
                        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                            // Single-quote the path so shell metacharacters
                            // in the directory name are inert. The only
                            // character that cannot appear inside single
                            // quotes is `'` itself, escaped as `'\''`.
                            let escaped = dir.replace('\'', "'\\''");
                            let cmd = format!("cd '{}'\n", escaped);
                            terminal.write(cmd.as_bytes());
                        }
                    }
                }
            }
            PaletteMode::TabList => {
                if let Some(action) = action {
                    self.choose_tab_row(&action);
                }
            }
            PaletteMode::Recent => {
                if let Some(action) = action {
                    self.choose_recent_row(&action);
                }
            }
            PaletteMode::LiveSearch => {
                if let Some(action) = action {
                    self.open_live_search_row(&action);
                }
            }
            PaletteMode::ProjectFiles => {
                if let Some(path) = action {
                    self.open_path_tool(tool::OpenTarget::file(PathBuf::from(path)));
                }
            }
            PaletteMode::OpenWith => {
                if let Some(kind) = action {
                    self.open_focused_with(&kind, focused);
                }
            }
            // A key list is read, not chosen from.
            PaletteMode::TabKeys => {}
            PaletteMode::Panes => {
                if let Some(pane_id_str) = action {
                    if let Ok(pane_id_val) = pane_id_str.parse::<u64>() {
                        self.switch_to_tab(TabId(pane_id_val));
                    }
                }
            }
            PaletteMode::Files => {
                if let Some(path) = action {
                    // Over the focused pane, as every other way into a file
                    // does it, so closing it puts the shell back.
                    self.open_path_tool(tool::OpenTarget::file(PathBuf::from(path)));
                }
            }
            PaletteMode::ToolPick => {
                if let Some(pick) = pick {
                    self.answer_pick(pick, action);
                }
            }
            PaletteMode::Swoop => {
                if let Some(action) = action {
                    if let Ok(abs_row) = action.parse::<usize>() {
                        if let Some((_pid, origin)) = self.swoop_initial_cursor.take() {
                            self.vim.tab_mut(focused).jump_list.push(origin);
                        }
                        self.set_nav_cursor(focused, (abs_row, 0));
                        self.reveal_position(focused, (abs_row, 0));
                        self.open_tabs.set_mode(&focused, Mode::Normal);
                    }
                }
            }
            PaletteMode::Jump => {
                if let Some(action) = action {
                    if let Ok(target) = action.parse::<usize>() {
                        if let Some(slot) = self.open_tabs.tool_mut(&focused) {
                            slot.tool.jump_confirm(target);
                        } else {
                            if let Some((_pid, origin)) = self.swoop_initial_cursor.take() {
                                self.vim.tab_mut(focused).jump_list.push(origin);
                            }
                            self.set_nav_cursor(focused, (target, 0));
                            self.reveal_position_centered(focused, (target, 0));
                            self.open_tabs.set_mode(&focused, Mode::Normal);
                        }
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
                        let cwd = self
                            .open_tabs
                            .terminal(&focused)
                            .and_then(|terminal| terminal.cwd());
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
    /// The browser rooted at the selected row, when the browser is up and
    /// that row is a directory. `None` for every other row and every other
    /// mode, which is the caller's signal to go on treating the key as a
    /// choice rather than a step.
    fn step_into_selection(&mut self, palette: &Palette) -> Option<Palette> {
        if palette.mode != PaletteMode::Files {
            return None;
        }
        if !palette.selected_is_dir() {
            return None;
        }
        Some(Palette::open_files(PathBuf::from(
            palette.selected_action()?,
        )))
    }

    /// The browser rooted one directory up, where there is one to reach.
    fn step_out(&mut self, palette: &Palette) -> Option<Palette> {
        if palette.mode != PaletteMode::Files {
            return None;
        }
        let dir = palette.dir.as_ref()?;
        Some(Palette::open_files(palette.dirs.parent(dir)?))
    }

    pub(crate) fn record_palette_query(&mut self, query: &str) {
        let trimmed = query.trim();
        // A query past this is a paste, not something to run again.
        if trimmed.is_empty() || trimmed.len() > MAX_PALETTE_QUERY_BYTES {
            return;
        }
        // Another window may have recorded queries since this one read the
        // history: start from what is on disk, so they are kept.
        let mut kept = Vec::new();
        self.update_app_state(|state| {
            let history = &mut state.palette_history;
            history.retain(|q| q != trimmed);
            history.insert(0, trimmed.to_string());
            history.truncate(MAX_PALETTE_HISTORY);
            kept = history.clone();
        });
        self.palette_history = kept;
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
            Err(_) => return vec!["(no daemon running; start with 'winter mux serve')".to_string()],
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
        self.open_palette(
            Palette::open_mux_sessions(sessions).with_query_history(self.palette_history.clone()),
        );
    }
    /// Open the command palette to select a background mux session to kill.
    pub(crate) fn open_mux_kill_palette(&mut self) {
        let sessions = self.mux_session_entries("(no running sessions)");
        self.open_palette(
            Palette::open_mux_kill(sessions).with_query_history(self.palette_history.clone()),
        );
    }
    /// Buffer swoop: open fuzzy line search over the focused pane's grid and scrollback.
    pub(crate) fn open_swoop(&mut self, focused: TabId) {
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let initial = self.nav_cursor(focused).or_else(|| {
            Some((
                terminal.grid().to_absolute_row(terminal.grid().cursor().0),
                terminal.grid().cursor().1,
            ))
        });
        if let Some(pos) = initial {
            self.swoop_initial_cursor = Some((focused, pos));
        }
        let lines = navigation::swoop::extract_swoop_lines(terminal.grid());
        self.open_tabs.set_mode(&focused, Mode::Normal);
        let palette = Palette::open_swoop(lines).with_query_history(self.palette_history.clone());
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.set_nav_cursor(focused, (abs_row, 0));
                self.reveal_position(focused, (abs_row, 0));
            }
        }
        self.open_palette(palette);
    }
    /// Update the live preview cursor during Buffer Swoop.
    pub(crate) fn update_swoop_preview(&mut self, palette: &Palette, focused: TabId) {
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.set_nav_cursor(focused, (abs_row, 0));
                self.reveal_position(focused, (abs_row, 0));
                self.dirty = true;
            }
        }
    }

    /// Buffer jump: open fuzzy target completion dialog for the active buffer view.
    pub(crate) fn open_jump(&mut self, focused: TabId) {
        if let Some(slot) = self.open_tabs.tool_mut(&focused) {
            if let Some(targets) = slot.tool.jump_targets() {
                let title = format!("Jump: {}", slot.tool.title());
                let initial_cursor = slot.tool.jump_cursor();
                let mut palette = Palette::open_jump(targets, Some(title))
                    .with_query_history(self.palette_history.clone());
                if let Some(cur) = initial_cursor {
                    palette.preselect_nearest(cur);
                }
                if let Some(action) = palette.selected_action() {
                    if let Ok(target) = action.parse::<usize>() {
                        slot.tool.jump_to(target);
                    }
                }
                self.open_palette(palette);
                return;
            }
        }

        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let initial = self.nav_cursor(focused).or_else(|| {
            Some((
                terminal.grid().to_absolute_row(terminal.grid().cursor().0),
                terminal.grid().cursor().1,
            ))
        });
        if let Some(pos) = initial {
            self.swoop_initial_cursor = Some((focused, pos));
        }
        let lines = navigation::swoop::extract_swoop_lines(terminal.grid());
        self.open_tabs.set_mode(&focused, Mode::Normal);
        let targets = lines
            .into_iter()
            .map(|(abs_row, line)| {
                tool::JumpTarget::new(abs_row, format!("{:>5}  {}", abs_row + 1, line))
            })
            .collect();
        let mut palette = Palette::open_jump(targets, Some("Jump: Terminal".to_string()))
            .with_query_history(self.palette_history.clone());
        if let Some((abs_row, _)) = initial {
            palette.preselect_nearest(abs_row);
        }
        if let Some(action) = palette.selected_action() {
            if let Ok(abs_row) = action.parse::<usize>() {
                self.set_nav_cursor(focused, (abs_row, 0));
                self.reveal_position_centered(focused, (abs_row, 0));
            }
        }
        self.open_palette(palette);
    }

    /// Update the live preview cursor during Buffer Jump.
    pub(crate) fn update_jump_preview(&mut self, palette: &Palette, focused: TabId) {
        if let Some(action) = palette.selected_action() {
            if let Ok(target) = action.parse::<usize>() {
                if let Some(slot) = self.open_tabs.tool_mut(&focused) {
                    slot.tool.jump_to(target);
                } else {
                    self.set_nav_cursor(focused, (target, 0));
                    self.reveal_position_centered(focused, (target, 0));
                }
                self.dirty = true;
            }
        }
    }

    /// Restore the pre-jump view/cursor position when jump is cancelled.
    pub(crate) fn cancel_jump(&mut self, focused: TabId) {
        if let Some(slot) = self.open_tabs.tool_mut(&focused) {
            slot.tool.jump_cancel();
        } else if let Some((pid, (r, c))) = self.swoop_initial_cursor.take() {
            if pid == focused {
                self.set_nav_cursor(focused, (r, c));
                self.reveal_position(focused, (r, c));
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Split a MuxNew palette query into `(session name, command line)`: the
/// first whitespace-delimited word names the session, the rest is the
/// command it runs (the default shell when absent). `None` when there is
/// no usable name, missing, placeholder-prefixed (those entries start
/// with `(` by convention), or carrying control characters.
fn parse_mux_spawn_query(query: &str) -> Option<(String, Option<String>)> {
    let mut words = query.split_whitespace();
    let name = words.next()?.to_string();
    if name.starts_with('(') || name.chars().any(char::is_control) {
        return None;
    }
    let command = words.collect::<Vec<_>>().join(" ");
    Some((name, (!command.is_empty()).then_some(command)))
}

/// Split a MuxAttachRemote palette query into `(host, session name)`: the
/// first word names the ssh host, the rest (if any) names the session.
/// `None` when there is no usable host.
fn parse_mux_attach_query(query: &str) -> Option<(String, Option<String>)> {
    let mut words = query.split_whitespace();
    let host = words.next()?.to_string();
    if host.starts_with('(') || host.chars().any(char::is_control) {
        return None;
    }
    let session = words.collect::<Vec<_>>().join(" ");
    Some((host, (!session.is_empty()).then_some(session)))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mux_spawn_query_splits_name_from_command() {
        // The GUI hands the whole prompt line to the parser; whitespace runs
        // between the name and the command must collapse to single spaces,
        // and a bare name must mean "default shell" (None), not an
        // empty-string command the server would try to run.
        assert_eq!(
            parse_mux_spawn_query("dev  cargo   watch -x test"),
            Some(("dev".into(), Some("cargo watch -x test".into())))
        );
        assert_eq!(parse_mux_spawn_query("dev"), Some(("dev".into(), None)));
    }

    #[test]
    fn test_parse_mux_spawn_query_rejects_unusable_names() {
        // Enter on an untouched prompt yields the empty query; a
        // placeholder-style label and a name carrying control characters
        // must all be rejected as spawn names instead of reaching the
        // Spawn message.
        assert_eq!(parse_mux_spawn_query(""), None);
        assert_eq!(parse_mux_spawn_query("   "), None);
        assert_eq!(parse_mux_spawn_query("(default shell)"), None);
        assert_eq!(parse_mux_spawn_query("de\u{7}v cargo"), None);
    }

    #[test]
    fn test_parse_mux_attach_query_splits_host_from_session() {
        // A bare host means "default" session (None), matching the CLI's
        // local-attach default so the two entry points behave the same way.
        assert_eq!(
            parse_mux_attach_query("box.example.com work"),
            Some(("box.example.com".into(), Some("work".into())))
        );
        assert_eq!(
            parse_mux_attach_query("box.example.com"),
            Some(("box.example.com".into(), None))
        );
    }

    #[test]
    fn test_parse_mux_attach_query_rejects_unusable_hosts() {
        assert_eq!(parse_mux_attach_query(""), None);
        assert_eq!(parse_mux_attach_query("   "), None);
        assert_eq!(parse_mux_attach_query("(host [session])"), None);
        assert_eq!(parse_mux_attach_query("ho\u{7}st work"), None);
    }
}

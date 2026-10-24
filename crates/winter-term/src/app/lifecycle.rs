//! Process lifecycle: construction, state persistence, quit and reload.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use winit::event_loop::{ActiveEventLoop, EventLoop};

use crate::config::Config;
use crate::control::{self, ControlMessage, ControlRequest, EditWaiter};
use crate::model::input::{self, PendingPrefix};
use crate::model::layout::{Direction, FocusDir, Rect, TabId};
use crate::model::mode::Mode;
use crate::model::notify::Notifier;
use crate::model::palette::{Palette, PaletteEntry};
use crate::model::textedit::field::TextField;
use crate::model::textedit::recall::Answers;
use crate::session::Session;

use super::appearance::SessionOverrides;
use super::blink::{BlinkTimer, TEXT_BLINK_PERIOD};
use super::ime::ImeState;
use super::jobs::Jobs;
use super::navigation::{SearchState, VimState};
use super::open_tabs::OpenTabs;
use super::overlay::{Overlay, RenameInput};
use super::pointer::{PointerState, SelectionState};
use super::sequence::keymap_from_blocks;
use super::tabbar::MenuState;
use super::tabs::TabsState;
use super::wake::Waker;
use super::App;
use super::{CURSOR_BLINK_PERIOD, GEOMETRY_SAVE_SETTLE};

// ========================================================================
// App: lifecycle
// ========================================================================

impl App {
    /// An app holding its first tab and pane, before any window or GPU exists.
    pub fn new() -> Self {
        let (config, config_error) = Config::load_checked();
        let (keymap, keymap_error) = keymap_from_blocks(&config.keybindings);
        let settings_rejected = config_error.is_some();
        let pending_config_error = config_error
            .or_else(|| config.keybindings_error.clone())
            .or(keymap_error);
        // Made before the event loop is, and connected to it once it runs
        // (see `App::connect_event_loop`): the threads below wake the loop as
        // their messages arrive rather than leaving them for its next poll.
        let waker = Waker::default();
        let control = control::spawn_listener();
        let (config_watcher, config_watch_rx) = match crate::config::watch::spawn_watcher() {
            Some((watcher, rx)) => (Some(watcher), Some(waker.relay(rx))),
            None => (None, None),
        };
        let state = crate::config::load_state();

        Self {
            config,
            config_watch_rx,
            config_reload_due: None,
            geometry_save_due: None,
            _config_watcher: config_watcher,
            dirty: true,
            last_frame_at: None,
            notifier: Notifier::default(),
            job_progress: None,
            busy_since: HashMap::new(),
            spinning: HashSet::new(),
            spinner_frame: 0,
            started: Instant::now(),
            yank_flash: None,
            settings_rejected,
            pending_config_error,
            clipboard: None,
            suppress_synthesized_keys: false,
            window_focused: true,
            focus_reported: None,
            focus_flash: None,
            status_hover: None,
            last_find: None,
            image_blocks: Vec::new(),
            modifiers: winit::event::Modifiers::default(),
            exit_requested: false,
            quit_confirmed: false,
            reload_confirmed: false,
            pending_reload: false,
            control_rx: control.requests.map(|rx| waker.relay(rx)),
            _control_socket: control.socket,
            nav_resync_pending: false,
            next_image_id: 0,
            icon_textures: HashMap::new(),
            icon_texture_box: (0, 0),
            editor_textures: HashMap::new(),
            palette_icons: HashMap::new(),
            jobs: Jobs::new(waker.clone()),
            quota: None,
            live_search: None,
            prompt_debounce: None,
            search_debounce: None,
            prompt_answers: Answers::default(),
            error_walk: None,
            editor_cursor: None,
            editor_wrap: false,
            commit_companions: HashMap::new(),
            open_tabs: OpenTabs::default(),
            closed: Vec::new(),
            next_closed_id: 0,
            reopen_hint_shown: false,
            overlay: None,
            pending: PendingPrefix::None,
            pending_since: None,
            sequence: None,
            forwarded_presses: HashMap::new(),
            quick_select: None,
            find_labels: None,
            renderer: None,
            swoop_initial_cursor: None,
            palette_history: state.palette_history,
            recent_dirs: state.recent_dirs,
            recent_files: state.recent_files,
            next_tab_id: 1,
            window: None,
            keymap,
            window_title: String::new(),
            overrides: SessionOverrides::default(),
            blink_phase: true,
            blink_next_flip: Instant::now() + CURSOR_BLINK_PERIOD,
            text_blink: BlinkTimer::new(TEXT_BLINK_PERIOD, Instant::now()),
            ime: ImeState::default(),
            last_activity: Instant::now(),
            menus: MenuState::default(),
            tabs: TabsState::default(),
            pointer: PointerState::default(),
            search: SearchState::default(),
            selection: SelectionState::default(),
            vim: VimState::default(),
            waker,
        }
    }
    /// Let the threads feeding this app wake `event_loop`'s loop when they
    /// have something for it, rather than leaving it for the next poll. Call
    /// once, before the loop runs.
    pub fn connect_event_loop(&mut self, event_loop: &EventLoop<()>) {
        self.waker.connect(event_loop.create_proxy());
        self.waker.share();
    }

    /// Act on every request the control socket has delivered since the last
    /// tick, not one per tick: two `winter --edit-wait` calls arriving
    /// together (a rebase opening its files) are both answered now.
    pub(crate) fn serve_control_requests(&mut self) {
        let requests: Vec<ControlRequest> = match &self.control_rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for request in requests {
            match request.message {
                ControlMessage::Reload => self.pending_reload = true,
                ControlMessage::Edit { path } => {
                    self.open_waited_editor(path, EditWaiter::new(request.stream));
                    self.dirty = true;
                }
            }
        }
    }

    /// Note that the window moved or was resized at `now`: its geometry is
    /// written once it has held still for [`GEOMETRY_SAVE_SETTLE`].
    pub(crate) fn note_geometry_change(&mut self, now: Instant) {
        self.geometry_save_due = Some(now + GEOMETRY_SAVE_SETTLE);
    }

    /// Whether the geometry is due to be written by `now`, spending the
    /// deadline if it is.
    pub(crate) fn take_geometry_save_due(&mut self, now: Instant) -> bool {
        self.geometry_save_due.take_if(|due| *due <= now).is_some()
    }

    /// Save the window's geometry.
    pub(crate) fn save_app_state(&mut self) {
        self.update_app_state(|_| {});
    }

    /// Save the window's geometry over the state on disk, after `update` has
    /// changed it. The histories written are the ones on disk, which every
    /// window adds to: this window's own copies may be older, and writing
    /// them would erase what another window recorded since.
    pub(crate) fn update_app_state(&mut self, update: impl FnOnce(&mut crate::config::AppState)) {
        // Whatever geometry was waiting to be written is written now.
        self.geometry_save_due = None;
        let mut state = crate::config::load_state();
        update(&mut state);
        let window_size = self
            .window
            .as_ref()
            .map(|w| w.inner_size())
            .map(|s| (s.width, s.height));
        let window_position = self
            .window
            .as_ref()
            .and_then(|w| w.outer_position().ok())
            .map(|p| (p.x, p.y));
        state.window_size = window_size;
        state.window_position = window_position;
        crate::config::save_state(&state);
    }
    /// Run a named command, shared by the command palette and the menus.
    pub(crate) fn run_command(&mut self, action: &str, focused: TabId) {
        match action {
            "new_tab" => {
                self.new_tab();
            }
            "new_window" => {
                self.new_window();
            }
            "close_tab" => {
                self.close_tab(focused);
            }
            "close_other_tabs" => {
                self.close_other_tabs(focused);
            }
            "rename_tab" => {
                let text = self.tabs.name(focused).unwrap_or_default().to_string();
                self.open_overlay(Overlay::Rename(RenameInput::new(focused, &text)));
            }
            "next_tab" => {
                self.cycle_tab(true);
            }
            "prev_tab" => {
                self.cycle_tab(false);
            }
            "recent_tab_back" => {
                self.recent_tab(false);
            }
            "recent_tab_forward" => {
                self.recent_tab(true);
            }
            "reload" => {
                self.pending_reload = true;
            }
            "reveal_terminal" => {
                self.reveal_recent_terminal();
            }
            "toggle_mode" => {
                // An editor tab has no mode to toggle, and is left as it is.
                if let Some(mode) = self.open_tabs.mode(&focused) {
                    let new_mode = match mode {
                        Mode::Insert => Mode::Normal,
                        Mode::Normal | Mode::Visual => Mode::Insert,
                    };
                    self.open_tabs.set_mode(&focused, new_mode);
                }
            }
            "split_horizontal" => {
                self.split_pane(Direction::Horizontal);
            }
            "split_vertical" => {
                self.split_pane(Direction::Vertical);
            }
            "close_pane" => {
                self.close_pane(focused);
            }
            "dir_page" => {
                self.open_dir_editor();
            }
            "editor_show_scopes" => {
                self.run_editor_command(focused, crate::editors::text::SHOW_SCOPES);
            }
            "live_search" => {
                self.open_live_search();
            }
            "project_files" => {
                self.open_project_files();
            }
            "find_file" => {
                self.open_find_file();
            }
            "quota_refresh" => {
                self.refresh_quotas();
            }
            "quota_zai_key" => {
                self.open_overlay(Overlay::ZaiKey(TextField::line("")));
            }
            "file_browser" => {
                self.open_file_browser();
            }
            "file_browser_root" => {
                self.open_root_browser();
            }
            "file_browser_docker" => {
                self.open_target_browser(crate::remote::Kind::Docker);
            }
            "file_browser_ssh" => {
                self.open_target_browser(crate::remote::Kind::Ssh);
            }
            "git_page" => {
                self.open_git_editor();
            }
            "grep_page" => {
                self.open_grep_editor();
            }
            "proc_page" => {
                self.open_proc_editor();
            }
            "sys_page" => {
                self.open_sys_editor();
            }
            "keys_page" | "keys_view" => {
                self.open_keys_editor();
            }
            "reopen_page" => {
                self.reopen_last_closed();
            }
            "stop_tasks" => {
                self.stop_tasks(focused);
            }
            "recent_list" => {
                self.open_recent_list();
            }
            "tool_list" | "extension_list" => {
                self.open_tab_list();
            }
            "notifications" => {
                self.open_notifications();
            }
            "tab_keys" => {
                self.open_tab_keys(focused);
            }
            "clear_notifications" => {
                self.clear_notifications();
            }
            "copy_cwd" => {
                self.copy_pane_cwd(focused);
            }
            "open_under_cursor" => {
                self.open_under_cursor(focused);
            }
            "open_with" => {
                self.open_with_list(focused);
            }
            "focus_down" | "focus_up" | "focus_left" | "focus_right" => {
                let dir = match action {
                    "focus_down" => FocusDir::Down,
                    "focus_up" => FocusDir::Up,
                    "focus_left" => FocusDir::Left,
                    _ => FocusDir::Right,
                };
                let viewport = self.viewport_rect();
                let layout_vp = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
                self.layout_mut().focus_in_direction(dir, layout_vp);
            }
            "move_tab_down"
            | "move_tab_up"
            | "move_tab_left"
            | "move_tab_right"
            | "move_tab_left_pane"
            | "move_tab_right_pane"
            | "move_tab_up_pane"
            | "move_tab_down_pane"
            | "move_tab_to_left_pane"
            | "move_tab_to_right_pane"
            | "move_tab_to_up_pane"
            | "move_tab_to_down_pane" => {
                let dir = match action {
                    "move_tab_down" | "move_tab_down_pane" | "move_tab_to_down_pane" => {
                        FocusDir::Down
                    }
                    "move_tab_up" | "move_tab_up_pane" | "move_tab_to_up_pane" => FocusDir::Up,
                    "move_tab_left" | "move_tab_left_pane" | "move_tab_to_left_pane" => {
                        FocusDir::Left
                    }
                    _ => FocusDir::Right,
                };
                self.move_tab_in_direction(dir);
            }
            "search" => {
                self.search.query = Some(String::new());
            }
            "next_block" => {
                self.focus_block(input::BlockNav::Next, focused);
            }
            "prev_block" => {
                self.focus_block(input::BlockNav::Previous, focused);
            }
            "quick_select" => {
                self.enter_quick_select(focused);
            }
            "yank_block" => {
                self.yank_block_source(focused);
            }
            "toggle_fold" => {
                if let Some(tab) = self.open_tabs.terminal_tab_mut(&focused) {
                    let folded = &mut tab.folded_blocks;
                    if folded.is_empty() {
                        folded.insert(0);
                    } else {
                        folded.clear();
                    }
                }
                self.dirty = true;
            }
            "theme_dark" => {
                self.config.theme = crate::config::ThemeSetting::Dark;
                self.rebuild_theme();
            }
            "theme_light" => {
                self.config.theme = crate::config::ThemeSetting::Light;
                self.rebuild_theme();
            }
            "theme_auto" => {
                self.config.theme = crate::config::ThemeSetting::Auto;
                self.rebuild_theme();
            }
            "theme_new" => {
                self.open_overlay(Overlay::ThemeName(TextField::line("")));
            }
            "open_settings" => {
                self.open_settings();
            }
            "toggle_pane_zoom" => {
                self.layout_mut().toggle_zoom();
                if self.renderer.is_some() {
                    self.resize_all_panes();
                }
                self.dirty = true;
            }
            "cd_recent" => {
                let dirs = self
                    .open_tabs
                    .terminal(&focused)
                    .map(|p| {
                        let mut seen = std::collections::HashSet::new();
                        p.scrollback()
                            .blocks()
                            .iter()
                            .rev()
                            .filter_map(|b| b.cwd.as_deref())
                            .filter(|&d| seen.insert(d.to_string()))
                            .map(str::to_string)
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                self.open_palette(
                    Palette::open_recent_dirs(dirs)
                        .with_query_history(self.palette_history.clone()),
                );
            }
            "select_pane" => {
                let mut panes_list = Vec::new();
                for (pane_index, &shown) in self.layout().shown_tabs().iter().enumerate() {
                    // Name each tab after the pane holding it ("<pane
                    // number>: <title>"); its place on that pane's strip goes
                    // on the right.
                    for (tab_index, tab_id) in
                        self.layout().tabs_in_pane(shown).into_iter().enumerate()
                    {
                        let label = format!("{}: {}", pane_index + 1, self.tab_title(tab_id));
                        panes_list.push(
                            PaletteEntry::new(
                                tab_id.0.to_string(),
                                label,
                                (tab_index + 1).to_string(),
                            )
                            .with_icon(self.tab_icon_kind(tab_id)),
                        );
                    }
                }
                self.open_palette(
                    Palette::open_panes(panes_list)
                        .with_query_history(self.palette_history.clone()),
                );
            }
            "swoop" => {
                self.open_swoop(focused);
            }
            "jump" => {
                self.open_jump(focused);
            }
            "copy_scrollback" => {
                self.copy_scrollback(focused);
            }
            "export_scrollback_ansi" => {
                self.export_scrollback_ansi(focused);
            }
            "export_scrollback_editor" => {
                self.export_scrollback_editor(focused);
            }
            "export_scrollback_html" => {
                self.export_scrollback_html(focused);
            }
            "next_error" => self.step_error(focused, true),
            "prev_error" => self.step_error(focused, false),
            "export_block_text" => {
                self.export_focused_block_text(focused);
            }
            "export_block_svg" => {
                self.export_focused_block_svg(focused);
            }
            "toggle_rainbow_parens" => {
                self.toggle_rainbow_parens();
            }
            "toggle_sentence_highlight" => {
                self.toggle_sentence_highlight();
            }
            "mux_list_sessions" => {
                self.open_mux_palette();
            }
            "mux_new_session" => {
                self.open_palette(
                    Palette::open_mux_new().with_query_history(self.palette_history.clone()),
                );
            }
            "mux_attach_remote" => {
                self.open_palette(
                    Palette::open_mux_attach_remote()
                        .with_query_history(self.palette_history.clone()),
                );
            }
            "mux_kill_session" => {
                self.open_mux_kill_palette();
            }
            "mux_detach_session" => {
                let session = self
                    .open_tabs
                    .terminal(&focused)
                    .and_then(|terminal| terminal.mux_session().map(str::to_string));
                match session {
                    Some(name) => {
                        if self.layout().members().len() <= 1 {
                            self.set_notice(
                                "cannot detach the only tab: close the window or open another tab",
                            );
                        } else {
                            self.set_notice(format!("detached from mux session '{name}'"));
                            // Closing the tab drops the mux client, which
                            // detaches server-side; the session keeps running.
                            self.close_tab(focused);
                        }
                    }
                    None => self.set_notice("this pane is not attached to a mux session"),
                }
            }
            // Palette entries that mirror a keybindable window command
            // (`copy_selection`, `font_increase`, scrolling, …) dispatch
            // through the same mapping their chords use, so selecting one
            // from the palette and pressing its keybinding are equivalent.
            other => {
                if let Some(action) = crate::model::input::window_action_by_name(other) {
                    self.handle_action(action, focused);
                }
            }
        }
    }
    /// Persist the session and exit the event loop. Shared by the native close
    /// request and the custom window-close control.
    pub(crate) fn quit(&mut self, event_loop: &ActiveEventLoop) {
        // Edits on no disk, or a program that would be ended with the window,
        // are asked about first. The answer comes back as a quit that has
        // already been agreed to.
        if !self.quit_confirmed && self.ask_before_quitting() {
            return;
        }
        if self.config.restore_session {
            let saved = Session::save(
                &self.tabs.layout,
                &self.open_tabs.terminal_map(),
                self.editor_sessions(),
            );
            // The window is going: there is no status bar left to say it in.
            if let Err(e) = saved {
                eprintln!("winter: could not save the session: {e}");
            }
        }
        self.save_app_state();
        self.open_tabs.clear_terminals();
        event_loop.exit();
    }
    /// Replace this window with the binary now on disk, keeping the session.
    ///
    /// On Unix the process replaces its own image (`exec`), so the programs in
    /// its panes stay its children with their ptys open, and the new image
    /// takes them over with what each screen showed; see [`Session::save_handover`].
    /// If the new binary cannot be started the window stays as it is. Where
    /// that is not possible the session is saved, a fresh instance is started,
    /// and this one exits, so the programs end and are started again.
    #[cfg_attr(unix, allow(unused_variables))]
    pub(crate) fn reload(&mut self, event_loop: &ActiveEventLoop) {
        // What a reload would lose is asked about first, as a quit asks; the
        // answer comes back as a reload that has been agreed to.
        if !std::mem::take(&mut self.reload_confirmed) && self.ask_before_reloading() {
            return;
        }
        self.save_app_state();
        #[cfg(unix)]
        {
            self.exec_into_new_binary();
        }
        #[cfg(not(unix))]
        {
            Session::save(
                &self.tabs.layout,
                &self.open_tabs.terminal_map(),
                self.editor_sessions(),
            );
            if let Ok(exe) = std::env::current_exe() {
                let _ = std::process::Command::new(exe).spawn();
            }
            self.open_tabs.clear_terminals();
            event_loop.exit();
        }
    }

    /// The Unix half of [`App::reload`]. Returns only when the new binary could
    /// not be started.
    #[cfg(unix)]
    fn exec_into_new_binary(&mut self) {
        use std::os::unix::process::CommandExt;

        let Some(exe) = crate::terminal::session::runnable_current_exe() else {
            self.set_error("reload: the path of this binary is not known".to_string());
            return;
        };
        // Once this image is replaced, a new one that cannot start takes every
        // program with it. A binary that does not even answer `--version` (half
        // written by a build still running, a missing library) is caught here,
        // before anything is handed over.
        if let Err(problem) = probe_binary(&exe) {
            self.set_error(format!("reload: {} does not run: {problem}", exe.display()));
            return;
        }
        // What the programs have already written belongs on the screen that is
        // copied, not in a channel that goes with this image.
        for (_, terminal) in self.open_tabs.terminals_mut() {
            terminal.drain_output();
        }
        let saved = Session::save_handover(
            &self.tabs.layout,
            &self.open_tabs.terminal_map(),
            self.editor_sessions(),
        );
        if let Err(e) = saved {
            self.take_back_handover();
            self.set_error(format!(
                "reload: could not save the session to hand over: {e}"
            ));
            return;
        }
        crate::session::announce_handover();
        let error = std::process::Command::new(&exe)
            .args(std::env::args_os().skip(1))
            .exec();
        crate::session::withdraw_handover();
        self.take_back_handover();
        self.set_error(format!(
            "reload: could not start {}: {error}",
            exe.display()
        ));
    }

    /// Undo a handover whose `exec` will not happen: each program's PTY is
    /// closed on exec again, so it does not leak into every program this
    /// process starts from now on and outlive its tab.
    #[cfg(unix)]
    fn take_back_handover(&mut self) {
        for (_, terminal) in self.open_tabs.terminals_mut() {
            terminal.take_back();
        }
    }
}

/// How long the new binary has to answer `--version` before a reload gives up
/// on it.
#[cfg(unix)]
const RELOAD_PROBE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Run `exe --version` and say what is wrong if it does not answer in time
/// and cleanly: the cheapest check that the binary a reload is about to
/// become can start at all.
#[cfg(unix)]
fn probe_binary(exe: &std::path::Path) -> Result<(), String> {
    use std::process::{Command, Stdio};
    let mut child = Command::new(exe)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| e.to_string())?;
    let deadline = std::time::Instant::now() + RELOAD_PROBE_TIMEOUT;
    loop {
        match child.try_wait().map_err(|e| e.to_string())? {
            Some(status) if status.success() => return Ok(()),
            Some(status) => return Err(format!("`--version` failed ({status})")),
            None if std::time::Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("`--version` did not answer".to_string());
            }
            None => std::thread::sleep(std::time::Duration::from_millis(10)),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::palette::PaletteMode;

    #[test]
    fn test_window_geometry_is_written_once_it_settles_not_on_every_move() {
        // Regression: every move and resize event wrote and flushed the
        // state file on the event-loop thread, dozens of times a second.
        let mut app = App::new();
        let start = Instant::now();
        app.note_geometry_change(start);
        app.note_geometry_change(start + GEOMETRY_SAVE_SETTLE / 2);
        assert!(
            !app.take_geometry_save_due(start + GEOMETRY_SAVE_SETTLE),
            "a move since pushed the write back"
        );
        let settled = start + GEOMETRY_SAVE_SETTLE * 2;
        assert!(app.take_geometry_save_due(settled));
        assert!(!app.take_geometry_save_due(settled), "written once");
    }

    #[cfg(unix)]
    #[test]
    fn test_a_reload_target_must_answer_version_before_anything_is_handed_over() {
        assert_eq!(probe_binary(std::path::Path::new("/bin/true")), Ok(()));
        assert!(probe_binary(std::path::Path::new("/bin/false")).is_err());
        assert!(probe_binary(std::path::Path::new("/nonexistent/winter")).is_err());
    }

    #[test]
    fn test_app_default_has_one_pane() {
        let app = App::new();
        assert_eq!(app.layout().shown_tabs(), vec![TabId(0)]);
        assert!(app.open_tabs.terminals().next().is_none());
    }

    #[test]
    fn test_app_new_has_no_quick_select() {
        let app = App::new();
        assert!(app.quick_select.is_none());
    }

    #[test]
    fn test_run_command_action_dispatches_the_same_effect_as_the_palette() {
        // A chord bound to a palette-only command reaches the app as
        // `Action::RunCommand`; `handle_action` must forward it to
        // `run_command` so the effect matches selecting it from the palette.
        let mut app = App::new();
        let focused = app.layout().focused();
        app.handle_action(
            input::Action::RunCommand("mux_new_session".to_string()),
            focused,
        );
        assert_eq!(
            app.palette().map(|p| p.mode.clone()),
            Some(PaletteMode::MuxNew)
        );
    }
}

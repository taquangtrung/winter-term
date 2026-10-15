//! Process lifecycle: construction, state persistence, quit and reload.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use winit::event_loop::ActiveEventLoop;

use crate::config::Config;
use crate::control::{self};
use crate::model::input::{self, PendingPrefix, WindowKeymap};
use crate::model::layout::{Direction, FocusDir, Rect, TabId};
use crate::model::mode::Mode;
use crate::model::palette::Palette;
use crate::model::textedit::field::TextField;
use crate::model::textedit::recall::Answers;
use crate::session::Session;
use crate::terminal::webview::WebViewManager;

use super::navigation::{SearchState, VimState};
use super::open_tabs::OpenTabs;
use super::pointer::{PointerState, SelectionState};
use super::tabbar::MenuState;
use super::tabs::TabsState;
use super::App;
use super::CURSOR_BLINK_PERIOD;

// ========================================================================
// App: lifecycle
// ========================================================================

impl App {
    /// An app holding its first tab and pane, before any window or GPU exists.
    pub fn new() -> Self {
        let (config, config_error) = Config::load_checked();
        let window_keymap = WindowKeymap::from_config(
            config.keybindings.get("window"),
            config.keybindings.get("editing"),
        );
        let (config_watcher, config_watch_rx) = match crate::config::watch::spawn_watcher() {
            Some((watcher, rx)) => (Some(watcher), Some(rx)),
            None => (None, None),
        };
        let base_font_size = config.font_size;

        Self {
            config,
            config_watch_rx,
            _config_watcher: config_watcher,
            dirty: true,
            notice: None,
            job_progress: None,
            busy_since: HashMap::new(),
            spinning: HashSet::new(),
            spinner_frame: 0,
            started: Instant::now(),
            yank_flash: None,
            pending_config_error: config_error,
            clipboard: None,
            suppress_synthesized_keys: false,
            window_focused: true,
            last_find: None,
            image_blocks: Vec::new(),
            last_tile_layout: None,
            modifiers: winit::event::Modifiers::default(),
            exit_requested: false,
            quit_confirmed: false,
            pending_reload: false,
            control_rx: control::spawn_listener(),
            nav_resync_pending: false,
            next_image_id: 0,
            icon_textures: HashMap::new(),
            icon_texture_box: (0, 0),
            jobs: crate::app::jobs::Jobs::new(),
            page_prompt: None,
            prompt_debounce: None,
            search_debounce: None,
            prompt_answers: Answers::default(),
            error_walk: None,
            page_pick: None,
            page_cursor: None,
            page_wrap: false,
            commit_companions: HashMap::new(),
            open_tabs: OpenTabs::default(),
            closed: Vec::new(),
            next_closed_id: 0,
            reopen_hint_shown: false,
            pending_close: None,
            palette: None,
            tab_titles: HashMap::new(),
            theme_name_input: None,
            pending: PendingPrefix::None,
            pending_since: None,
            quick_select: None,
            find_labels: None,
            renderer: None,
            settings_page: None,
            swoop_initial_cursor: None,
            palette_history: crate::config::load_state().palette_history,
            next_tab_id: 1,
            webview_mgr: WebViewManager::new(),
            window: None,
            window_keymap,
            window_title: String::new(),
            base_font_size,
            blink_phase: true,
            blink_next_flip: Instant::now() + CURSOR_BLINK_PERIOD,
            last_activity: Instant::now(),
            menus: MenuState::default(),
            tabs: TabsState::default(),
            pointer: PointerState::default(),
            search: SearchState::default(),
            selection: SelectionState::default(),
            vim: VimState::default(),
        }
    }
    pub(crate) fn save_app_state(&self) {
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
        crate::config::save_state(&crate::config::AppState {
            palette_history: self.palette_history.clone(),
            window_size,
            window_position,
        });
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
                self.tabs.rename_input =
                    Some(self.tabs.names.get(&focused).cloned().unwrap_or_default());
                self.dirty = true;
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
                // A page tab has no mode to toggle, and is left as it is.
                if let Some(mode) = self.open_tabs.mode(&focused) {
                    let new_mode = match mode {
                        Mode::Insert => Mode::Normal,
                        Mode::Normal | Mode::Visual | Mode::BlockFocus => Mode::Insert,
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
                self.open_dir_page();
            }
            "file_browser" => {
                self.open_file_browser();
            }
            "git_page" => {
                self.open_git_page();
            }
            "grep_page" => {
                self.open_grep_page();
            }
            "proc_page" => {
                self.open_proc_page();
            }
            "sys_page" => {
                self.open_sys_page();
            }
            "keys_page" | "keys_view" => {
                self.open_keys_page();
            }
            "reopen_page" => {
                self.reopen_last_closed();
            }
            "tool_list" | "extension_list" => {
                self.open_tab_list();
            }
            "copy_cwd" => {
                self.copy_pane_cwd(focused);
            }
            "open_under_cursor" => {
                self.open_under_cursor(focused);
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
                self.theme_name_input = Some(TextField::line(""));
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
                self.palette = Some(
                    Palette::open_recent_dirs(dirs)
                        .with_query_history(self.palette_history.clone()),
                );
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
                        panes_list.push((tab_id, label, (tab_index + 1).to_string()));
                    }
                }
                self.palette = Some(
                    Palette::open_panes(panes_list)
                        .with_query_history(self.palette_history.clone()),
                );
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
                self.palette =
                    Some(Palette::open_mux_new().with_query_history(self.palette_history.clone()));
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            "mux_attach_remote" => {
                self.palette = Some(
                    Palette::open_mux_attach_remote()
                        .with_query_history(self.palette_history.clone()),
                );
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
            Session::save(
                &self.tabs.layout,
                &self.open_tabs.terminal_map(),
                self.page_sessions(),
            );
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
        self.save_app_state();
        #[cfg(unix)]
        {
            self.exec_into_new_binary();
            return;
        }
        #[cfg(not(unix))]
        {
            Session::save(
                &self.tabs.layout,
                &self.open_tabs.terminal_map(),
                self.page_sessions(),
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
        // What the programs have already written belongs on the screen that is
        // copied, not in a channel that goes with this image.
        for (_, terminal) in self.open_tabs.terminals_mut() {
            terminal.drain_output();
        }
        Session::save_handover(
            &self.tabs.layout,
            &self.open_tabs.terminal_map(),
            self.page_sessions(),
        );
        crate::session::announce_handover();
        let error = std::process::Command::new(&exe)
            .args(std::env::args_os().skip(1))
            .exec();
        crate::session::withdraw_handover();
        self.set_error(format!(
            "reload: could not start {}: {error}",
            exe.display()
        ));
    }
}

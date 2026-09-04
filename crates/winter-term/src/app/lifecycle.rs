//! Process lifecycle: construction, state persistence, quit and reload.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use winit::event_loop::ActiveEventLoop;

use crate::config::Config;
use crate::control::{self};
use crate::model::input::{self, PendingPrefix, VisualKind, WindowKeymap};
use crate::model::layout::{Direction, FocusDir, PaneId, Rect, Tab};
use crate::model::mode::Mode;
use crate::model::palette::Palette;
use crate::session::Session;
use crate::terminal::webview::WebViewManager;
use winter_render::TabbarHit;

use super::App;
use super::CURSOR_BLINK_PERIOD;

// ========================================================================
// App: lifecycle
// ========================================================================

impl App {
    pub fn new() -> Self {
        let mut modes = HashMap::new();
        modes.insert(PaneId(0), Mode::default());

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
            tabbar_hover: TabbarHit::None,
            tab_hover_pos: None,
            config,
            config_watch_rx,
            _config_watcher: config_watcher,
            cursor_pos: (0.0, 0.0),
            dirty: true,
            auto_scroll_next: Instant::now(),
            divider_drag: None,
            scrollbar_drag: None,
            notice: None,
            pending_config_error: config_error,
            clipboard: None,
            folded_blocks: HashMap::new(),
            last_click: None,
            suppress_synthesized_keys: false,
            window_focused: true,
            last_find: None,
            image_blocks: Vec::new(),
            last_tile_layout: None,
            modifiers: winit::event::Modifiers::default(),
            mouse_down: false,
            exit_requested: false,
            pending_reload: false,
            control_rx: control::spawn_listener(),
            nav_cursors: HashMap::new(),
            nav_resync_pending: false,
            next_image_id: 0,
            panes: HashMap::new(),
            pending_tab_completion: HashSet::new(),
            last_alt_screen_escape: None,
            prompt_shadows: HashMap::new(),
            palette: None,
            pane_titles: HashMap::new(),
            tab_names: HashMap::new(),
            tab_rename_input: None,
            theme_name_input: None,
            pending: PendingPrefix::None,
            pending_since: None,
            quick_select: None,
            find_labels: None,
            renderer: None,
            search_query: None,
            search_match_index: 0,
            search_match_total: 0,
            search_current: None,
            search_last: None,
            search_origin: None,
            search_reverse: false,
            selection: None,
            settings_page: None,
            swoop_initial_cursor: None,
            active_tab: 0,
            tab_mru: vec![0],
            mru_walk: None,
            jump_lists: HashMap::new(),
            change_lists: HashMap::new(),
            last_changes: HashMap::new(),
            insert_sessions: HashMap::new(),
            marks: HashMap::new(),
            registers: HashMap::new(),
            palette_history: crate::config::load_state().palette_history,
            last_visual: None,
            open_menu: None,
            open_submenu: None,
            selected_item: None,
            selected_subitem: None,
            next_pane_id: 1,
            tabs: vec![Tab::new()],
            modes,
            visual_anchor: None,
            visual_kind: VisualKind::Char,
            webview_mgr: WebViewManager::new(),
            window: None,
            window_keymap,
            window_title: String::new(),
            hovered_url: None,
            context_menu_pos: None,
            context_menu_url: None,
            context_menu_actions: Vec::new(),
            context_menu_selected: None,
            base_font_size,
            blink_phase: true,
            blink_next_flip: Instant::now() + CURSOR_BLINK_PERIOD,
            last_activity: Instant::now(),
            tab_drag_start: None,
        }
    }
    pub(crate) fn save_app_state(&self) {
        let window_size = self.window.as_ref().map(|w| {
            let s = w.inner_size();
            (s.width, s.height)
        });
        crate::config::save_state(&crate::config::AppState {
            palette_history: self.palette_history.clone(),
            window_size,
        });
    }
    /// Run a named command, shared by the command palette and the menus.
    pub(crate) fn run_command(&mut self, action: &str, focused: PaneId) {
        match action {
            "new_tab" => {
                self.new_tab();
            }
            "close_tab" => {
                self.close_tab(self.active_tab);
            }
            "rename_tab" => {
                self.tab_rename_input = Some(
                    self.tab_names
                        .get(&self.active_tab)
                        .cloned()
                        .unwrap_or_default(),
                );
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
            "toggle_mode" => {
                let mode = self.modes.get(&focused).copied().unwrap_or_default();
                let new_mode = match mode {
                    Mode::Insert => Mode::Normal,
                    Mode::Normal | Mode::Visual | Mode::BlockFocus => Mode::Insert,
                };
                self.modes.insert(focused, new_mode);
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
            "copy_cwd" => {
                self.copy_pane_cwd(focused);
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
                self.tab_mut().focus_in_direction(dir, layout_vp);
            }
            "search" => {
                self.search_query = Some(String::new());
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
                let folded = self.folded_blocks.entry(focused).or_default();
                if folded.is_empty() {
                    folded.insert(0);
                } else {
                    folded.clear();
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
                self.theme_name_input = Some(String::new());
            }
            "open_settings" => {
                self.open_settings();
            }
            "toggle_pane_zoom" => {
                self.tab_mut().toggle_zoom();
                if self.renderer.is_some() {
                    self.resize_all_panes();
                }
                self.dirty = true;
            }
            "cd_recent" => {
                let dirs = self
                    .panes
                    .get(&focused)
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
                for (tab_index, tab) in self.tabs.iter().enumerate() {
                    // Mirror the tab-bar format on the left ("<tab number>:
                    // <title>"); the pane number goes on the right.
                    let label = format!("{}: {}", tab_index + 1, self.tab_title(tab_index));
                    for (pane_index, &pane_id) in tab.panes().iter().enumerate() {
                        let shortcut = (pane_index + 1).to_string();
                        panes_list.push((pane_id, label.clone(), shortcut));
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
                    .panes
                    .get(&focused)
                    .and_then(|pane| pane.mux_session().map(str::to_string));
                match session {
                    Some(name) => {
                        if self.tabs.len() <= 1 && self.tabs[0].panes().len() <= 1 {
                            self.set_notice(
                                "cannot detach the only pane — close the window or open another tab",
                            );
                        } else {
                            self.set_notice(format!("detached from mux session '{name}'"));
                            // Closing the pane drops the mux client, which
                            // detaches server-side; the session keeps running.
                            self.close_pane(focused);
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
        if self.config.restore_session {
            Session::save(&self.tabs, self.active_tab, &self.panes);
        }
        self.panes.clear();
        event_loop.exit();
    }
    /// Persist the session, spawn a fresh instance of the current binary, and
    /// exit the event loop, so the binary can be replaced on disk while
    /// keeping the same tabs and panes across the restart. Unlike
    /// [`App::quit`], the session is always saved here regardless of
    /// `restore_session`, since reloading is an explicit request to carry
    /// state across the restart.
    pub(crate) fn reload(&mut self, event_loop: &ActiveEventLoop) {
        Session::save(&self.tabs, self.active_tab, &self.panes);
        if let Ok(exe) = std::env::current_exe() {
            let _ = std::process::Command::new(exe).spawn();
        }
        self.panes.clear();
        event_loop.exit();
    }
}

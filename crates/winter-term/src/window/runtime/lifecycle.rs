//! Process lifecycle: construction, state persistence, quit and reload.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Instant;

use winit::event_loop::{ActiveEventLoop, EventLoop};

use crate::config::Config;
use crate::control::{self, ControlMessage, ControlRequest, EditWaiter, SocketFile};
use crate::model::layout::{Layout, TabId};
use crate::session::Session;
use crate::terminal::session::Terminal;
use crate::wake::Waker;

use super::jobs::Jobs;
use crate::window::appearance::SessionOverrides;
use crate::window::frame::blink::Blinking;
use crate::window::frame::canvas::Textures;
use crate::window::input::focus::WindowFocus;
use crate::window::input::ime::ImeState;
use crate::window::input::sequence::keymap_from_blocks;
use crate::window::input::sequence::PendingKeys;
use crate::window::layout::open_tabs::OpenTabs;
use crate::window::workspace::WorkspaceState;
use crate::window::layout::tabs::TabsState;
use crate::window::picker::recent::RecentLists;
use crate::window::strip::busy::Busy;
use crate::window::terminal::navigation::{SearchState, VimState};
use crate::window::terminal::pointer::{PointerState, SelectionState};
use crate::window::{App, Chrome, EditorHost, InputState, RuntimeState, TerminalState, GEOMETRY_SAVE_SETTLE};

// ========================================================================
// Data Structures
// ========================================================================

/// The control socket other `winter` invocations talk to this window
/// through, and the requests it has relayed.
pub(in crate::window) struct ControlChannel {
    /// Background threads' control-channel messages (see [`crate::control`]),
    /// polled in `about_to_wait`. `None` when no control socket could be
    /// bound.
    pub(in crate::window) rx: Option<mpsc::Receiver<ControlRequest>>,
    /// This window's own control socket file, removed when the app is
    /// dropped.
    pub(in crate::window) _socket: Option<SocketFile>,
}

/// Hot reload of the config file: the watcher and its channel, when the
/// next reload is due, and what the last load got wrong.
pub(in crate::window) struct ConfigWatch {
    /// Filesystem-event receiver for the config directory: drained each
    /// `about_to_wait` tick to hot-reload as soon as a config/theme file is
    /// saved, rather than on a poll interval. `None` if the platform's watch
    /// backend failed to start, in which case hot-reload is simply disabled.
    pub(in crate::window) rx: Option<mpsc::Receiver<()>>,
    /// When the config directory, changed and since quiet, is due to be read
    /// again; `None` with nothing pending. See `reload_config_if_changed`.
    pub(in crate::window) reload_due: Option<Instant>,
    /// Owns the filesystem watch backing `rx`; never read again
    /// after construction, but must stay alive for as long as the app runs,
    /// dropping it stops event delivery.
    pub(in crate::window) _watcher: Option<notify::RecommendedWatcher>,
    /// A config diagnostic from startup, held until the window exists.
    /// A notice raised in `App::new` would start its expiry clock before GPU
    /// init and could lapse before the first frame ever paints.
    pub(in crate::window) pending_error: Option<String>,
    /// Whether settings.kdl failed to parse when last read, so what is in
    /// force is not what the file says and the settings page must not save
    /// over it.
    pub(in crate::window) settings_rejected: bool,
}

/// The window's way out: a quit or reload asked for, and confirmed.
#[derive(Default)]
pub(in crate::window) struct ExitState {
    /// Set when the custom window-close control is clicked, drained by the mouse
    /// handler into the same quit path as a native close request.
    pub(in crate::window) requested: bool,
    /// Whether quitting has been agreed to, so asking about what it would
    /// lose is not repeated when the quit is carried out.
    pub(in crate::window) quit_confirmed: bool,
    /// Whether a reload has been agreed to over what it would lose, so the
    /// reload that follows the answer does not ask again.
    pub(in crate::window) reload_confirmed: bool,
    /// Set by the "reload" command or an incoming [`crate::control::ControlMessage::Reload`],
    /// drained in `about_to_wait` into [`App::reload`] instead of [`App::quit`].
    pub(in crate::window) reload_pending: bool,
}

// ========================================================================
// ExitState Implementations
// ========================================================================

impl ExitState {
    /// Whether a quit was asked for since this was last asked, which it no
    /// longer is.
    pub(in crate::window) fn take_requested(&mut self) -> bool {
        std::mem::take(&mut self.requested)
    }
}

// ========================================================================
// ControlChannel Implementations
// ========================================================================

impl ControlChannel {
    /// The channel for a listener just spawned.
    pub(in crate::window) fn new(
        rx: Option<mpsc::Receiver<ControlRequest>>,
        socket: Option<SocketFile>,
    ) -> Self {
        Self {
            rx,
            _socket: socket,
        }
    }
}

// ========================================================================
// ConfigWatch Implementations
// ========================================================================

impl ConfigWatch {
    /// A watch over the config as it was just loaded.
    pub(in crate::window) fn new(
        watcher: Option<notify::RecommendedWatcher>,
        rx: Option<mpsc::Receiver<()>>,
        pending_error: Option<String>,
        settings_rejected: bool,
    ) -> Self {
        Self {
            rx,
            reload_due: None,
            _watcher: watcher,
            pending_error,
            settings_rejected,
        }
    }
}

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
            .or(keymap_error)
            .or_else(|| config.settings_warning.clone());
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
            chrome: Chrome::default(),
            host: EditorHost {
                commit_companions: HashMap::new(),
                compile: Default::default(),
                extensions: Default::default(),
                jobs: Jobs::new(waker.clone()),
                lsp: crate::window::editor::lsp::LspState::new(&waker),
                prompts: Default::default(),
                token_inspect: None,
            },
            runtime: RuntimeState {
                config_watch: ConfigWatch::new(
                    config_watcher,
                    config_watch_rx,
                    pending_config_error,
                    settings_rejected,
                ),
                control: ControlChannel::new(
                    control.requests.map(|rx| waker.relay(rx)),
                    control.socket,
                ),
                exit: ExitState::default(),
                geometry_save_due: None,
                last_activity: Instant::now(),
                last_frame_at: None,
                started: Instant::now(),
                strays: Vec::new(),
                waker,
            },
            input: InputState {
                clipboards: Default::default(),
                focus: WindowFocus::default(),
                ime: ImeState::default(),
                keys: PendingKeys::default(),
                modifiers: winit::event::Modifiers::default(),
                suppress_synthesized_keys: false,
                wheel: Default::default(),
            },
            term: TerminalState {
                error_walk: None,
                image_blocks: Vec::new(),
                live_search: None,
                nav_resync_pending: false,
                swoop_initial_cursor: None,
                yank_flash: None,
            },
            dirty: true,
            job_progress: None,
            textures: Textures::default(),
            quota: None,
            editor_cursor: None,
            open_tabs: OpenTabs::default(),
            renderer: None,
            recent: RecentLists::new(state.palette_history, state.recent_dirs, state.recent_files),
            workspace: WorkspaceState::default(),
            next_tab_id: 1,
            window: None,
            keymap,
            window_title: String::new(),
            overrides: SessionOverrides::default(),
            blink: Blinking::new(Instant::now()),
            tabs: TabsState::default(),
            pointer: PointerState::default(),
            search: SearchState::default(),
            selection: SelectionState::default(),
            vim: VimState::default(),
            macro_state: Default::default(),
            busy: Busy::default(),
        }
    }
    /// Let the threads feeding this app wake `event_loop`'s loop when they
    /// have something for it, rather than leaving it for the next poll. Call
    /// once, before the loop runs.
    pub fn connect_event_loop(&mut self, event_loop: &EventLoop<()>) {
        self.runtime.waker.connect(event_loop.create_proxy());
        self.runtime.waker.share();
    }

    /// Act on every request the control socket has delivered since the last
    /// tick, not one per tick: two `winter --edit-wait` calls arriving
    /// together (a rebase opening its files) are both answered now.
    pub(in crate::window) fn serve_control_requests(&mut self) {
        let requests: Vec<ControlRequest> = match &self.runtime.control.rx {
            Some(rx) => rx.try_iter().collect(),
            None => return,
        };
        for request in requests {
            match request.message {
                ControlMessage::Reload => self.runtime.exit.reload_pending = true,
                ControlMessage::Edit { path } => {
                    self.open_waited_editor(path, EditWaiter::new(request.stream));
                    self.dirty = true;
                }
                ControlMessage::Workspace(command) => {
                    let answer = self.answer_workspace_command(command);
                    crate::control::answer_workspace(request.stream, &answer);
                }
            }
        }
    }



    /// Save the window's geometry.
    pub(in crate::window) fn save_app_state(&mut self) {
        self.update_app_state(|_| {});
    }

    /// Save the window's geometry over the state on disk, after `update` has
    /// changed it. The histories written are the ones on disk, which every
    /// window adds to: this window's own copies may be older, and writing
    /// them would erase what another window recorded since.
    pub(in crate::window) fn update_app_state(
        &mut self,
        update: impl FnOnce(&mut crate::config::AppState),
    ) {
        // Whatever geometry was waiting to be written is written now.
        self.runtime.geometry_save_due = None;
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

    /// Toggle token and scope inspection on `focused`.
    pub(in crate::window) fn toggle_token_inspect(&mut self, focused: TabId) {
        if self.host.token_inspect == Some(focused) {
            self.dismiss_token_inspect();
        } else {
            self.dismiss_token_inspect();
            self.host.token_inspect = Some(focused);
            if let Some(slot) = self.open_tabs.editor_mut(&focused) {
                let _ = slot.editor.run_command(crate::model::editor::SHOW_SCOPES);
            }
            self.dirty = true;
        }
    }

    /// Dismiss token inspection if currently showing on any tab.
    pub(in crate::window) fn dismiss_token_inspect(&mut self) {
        if let Some(tab) = self.host.token_inspect.take() {
            if let Some(slot) = self.open_tabs.editor_mut(&tab) {
                let _ = slot.editor.run_command(crate::model::editor::HIDE_SCOPES);
            }
            self.dirty = true;
        }
    }
    /// A snapshot of the window as it is now, in the workspace it is in.
    pub(in crate::window) fn capture_session(&self) -> Session {
        self.capture_session_of(&self.tabs.layout, self.workspace.active.clone())
    }

    /// A snapshot of `layout` and the tabs in it, in `workspace`: the
    /// window's own, or a workspace's put aside.
    pub(in crate::window) fn capture_session_of(
        &self,
        layout: &Layout,
        workspace: Option<String>,
    ) -> Session {
        let mut editors = self.editor_sessions_in(layout);
        // The buried buffers are the window's, saved with its own layout.
        if std::ptr::eq(layout, &self.tabs.layout) {
            editors.extend(self.buried_sessions());
        }
        Session::capture(layout, &self.terminal_map_in(layout), editors, workspace)
    }

    /// The terminals of the tabs in `layout`, by tab.
    fn terminal_map_in(&self, layout: &Layout) -> HashMap<TabId, &Terminal> {
        let mut terminals = self.open_tabs.terminal_map();
        terminals.retain(|tab, _| layout.contains(*tab));
        terminals
    }

    /// Quit, where something done since quitting was last looked for asked
    /// to (closing the last tab, the close control). Says whether it did.
    pub(in crate::window) fn quit_if_requested(&mut self, event_loop: &ActiveEventLoop) -> bool {
        let requested = self.runtime.exit.take_requested();
        if requested {
            self.quit(event_loop);
        }
        requested
    }

    /// Persist the session and exit the event loop. Shared by the native close
    /// request and the custom window-close control.
    pub(in crate::window) fn quit(&mut self, event_loop: &ActiveEventLoop) {
        // Edits on no disk, or a program that would be ended with the window,
        // are asked about first. The answer comes back as a quit that has
        // already been agreed to.
        if !self.runtime.exit.quit_confirmed && self.ask_before_quitting() {
            return;
        }
        // A workspace keeps its layout whether or not the next start restores
        // one: going back to it later finds it as it was left.
        self.save_open_workspaces();
        if self.config.restore_session {
            let saved = self.capture_session().save();
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
    pub(in crate::window) fn reload(&mut self, event_loop: &ActiveEventLoop) {
        // What a reload would lose is asked about first, as a quit asks; the
        // answer comes back as a reload that has been agreed to.
        if !std::mem::take(&mut self.runtime.exit.reload_confirmed) && self.ask_before_reloading() {
            return;
        }
        self.save_open_workspaces();
        self.save_app_state();
        #[cfg(unix)]
        {
            self.exec_into_new_binary();
        }
        #[cfg(not(unix))]
        {
            let _ = self.capture_session().save();
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
        // Only the window's own layout is handed over: the programs of a
        // workspace put aside end with this image, and that workspace opens
        // as just saved when next switched to.
        let saved = Session::save_handover(
            &self.tabs.layout,
            &self.terminal_map_in(&self.tabs.layout),
            self.editor_sessions(),
            self.workspace.active.clone(),
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
// RuntimeState
// ========================================================================

impl RuntimeState {
    /// Note that the window moved or was resized at `now`: its geometry is
    /// written once it has held still for [`GEOMETRY_SAVE_SETTLE`].
    pub(in crate::window) fn note_geometry_change(&mut self, now: Instant) {
        self.geometry_save_due = Some(now + GEOMETRY_SAVE_SETTLE);
    }

    /// Whether the geometry is due to be written by `now`, spending the
    /// deadline if it is.
    pub(in crate::window) fn take_geometry_save_due(&mut self, now: Instant) -> bool {
        self.geometry_save_due.take_if(|due| *due <= now).is_some()
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::Action;
    use crate::model::picker::PickerMode;

    #[test]
    fn test_window_geometry_is_written_once_it_settles_not_on_every_move() {
        // Regression: every move and resize event wrote and flushed the
        // state file on the event-loop thread, dozens of times a second.
        let mut app = App::new();
        let start = Instant::now();
        app.runtime.note_geometry_change(start);
        app.runtime.note_geometry_change(start + GEOMETRY_SAVE_SETTLE / 2);
        assert!(
            !app.runtime.take_geometry_save_due(start + GEOMETRY_SAVE_SETTLE),
            "a move since pushed the write back"
        );
        let settled = start + GEOMETRY_SAVE_SETTLE * 2;
        assert!(app.runtime.take_geometry_save_due(settled));
        assert!(!app.runtime.take_geometry_save_due(settled), "written once");
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
    fn test_run_command_action_dispatches_the_same_effect_as_the_palette() {
        // A chord bound to a palette-only command reaches the app as
        // `Action::RunCommand`; `handle_action` must forward it to
        // `run_command` so the effect matches selecting it from the palette.
        let mut app = App::new();
        let focused = app.layout().focused();
        app.handle_action(
            Action::RunCommand("mux_new_session".to_string()),
            focused,
        );
        assert_eq!(
            app.chrome.picker().map(|p| p.mode.clone()),
            Some(PickerMode::MuxNew)
        );
    }
}

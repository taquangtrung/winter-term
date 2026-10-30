//! Where configuration and state live, and the state file itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ========================================================================
// Locations and persisted state
// ========================================================================

/// The state file's name inside the state directory (see
/// [`crate::paths::state_file`]).
const STATE_FILE_NAME: &str = "state.json";

/// Persistent runtime state stored in `<state_dir>/state.json`.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AppState {
    /// The command line each project was last built with, by its root.
    #[serde(default)]
    pub compile_commands: BTreeMap<PathBuf, String>,
    /// Previously executed palette queries, ordered most-recent first.
    #[serde(default)]
    pub palette_history: Vec<String>,
    /// Directories opened in a listing, most recent first.
    #[serde(default)]
    pub recent_dirs: Vec<PathBuf>,
    /// Files opened in an editor or handed to `$EDITOR`, most recent first.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    /// Each project's `.winter/tasks.kdl` as it read when the user trusted
    /// it to run commands on its own, by the project's root. A file that has
    /// changed since is not trusted until it is again.
    #[serde(default)]
    pub trusted_tasks: BTreeMap<PathBuf, String>,
    /// Last known window dimensions in physical pixels.
    pub window_size: Option<(u32, u32)>,
    /// Last known window top-left position in screen coordinates.
    #[serde(default)]
    pub window_position: Option<(i32, i32)>,
}
/// Where the state file is. `None` under test, so that no test reads the
/// developer's own history or writes the files it opened into it.
fn state_path() -> Option<PathBuf> {
    match cfg!(test) {
        true => None,
        false => crate::paths::state_file(STATE_FILE_NAME),
    }
}

/// Load state from `<state_dir>/state.json`, returning a default on any error.
pub fn load_state() -> AppState {
    state_path()
        .map(|path| crate::persist::read_json_or_set_aside(&path))
        .unwrap_or_default()
}
/// Persist state to `<state_dir>/state.json` in one step (see
/// [`crate::persist::write_atomic`]). A failure is logged: the state is
/// conveniences (history, window geometry), not worth interrupting for.
pub fn save_state(state: &AppState) {
    let written = state_path()
        .ok_or_else(|| std::io::Error::other("no state directory"))
        .and_then(|path| {
            let json = serde_json::to_string(state).map_err(std::io::Error::other)?;
            crate::persist::write_atomic(&path, json)
        });
    if let Err(e) = written {
        eprintln!("winter: could not save state.json: {e}");
    }
}
/// Whether the project at `root` was trusted to run `tasks`, the text of
/// its tasks file, on its own.
pub fn are_tasks_trusted(root: &Path, tasks: &str) -> bool {
    if state_path().is_none() {
        return TEST_TRUST.with(|trust| trust.borrow().get(root).map(String::as_str) == Some(tasks));
    }
    load_state().trusted_tasks.get(root).map(String::as_str) == Some(tasks)
}

/// Trust the project at `root` to run `tasks`, the text of its tasks file,
/// on its own, until that text changes.
pub fn trust_tasks(root: &Path, tasks: &str) {
    if state_path().is_none() {
        TEST_TRUST.with(|trust| trust.borrow_mut().insert(root.to_path_buf(), tasks.to_string()));
        return;
    }
    let mut state = load_state();
    state.trusted_tasks.insert(root.to_path_buf(), tasks.to_string());
    save_state(&state);
}

thread_local! {
    /// Where trust is kept with no state file to keep it in (a test), so a
    /// test can trust a project without touching the developer's own.
    static TEST_TRUST: std::cell::RefCell<BTreeMap<PathBuf, String>> =
        const { std::cell::RefCell::new(BTreeMap::new()) };
}

/// The command line the project at `root` was last built with.
pub fn remembered_compile_command(root: &Path) -> Option<String> {
    load_state().compile_commands.get(root).cloned()
}

/// Remember `command` as the one the project at `root` was last built
/// with, unless it already is.
pub fn remember_compile_command(root: &Path, command: &str) {
    // With nowhere to keep it (a test), there is nothing to remember into.
    if state_path().is_none() {
        return;
    }
    let mut state = load_state();
    if state.compile_commands.get(root).map(String::as_str) == Some(command) {
        return;
    }
    state
        .compile_commands
        .insert(root.to_path_buf(), command.to_string());
    save_state(&state);
}

/// The configuration directory: `%APPDATA%\winter-term` on Windows;
/// `$XDG_CONFIG_HOME/winter-term` or `~/.config/winter-term` elsewhere.
///
/// Started with none of those variables set (from cron, or a bare systemd
/// unit), the home directory comes from the system's user database, and
/// failing that a directory of Winter's own under the temporary one: never
/// the directory Winter happened to start in, whose `settings.kdl` it would
/// otherwise read, and into which a save would write one.
pub(crate) fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("APPDATA") {
            Ok(appdata) => PathBuf::from(appdata).join("winter-term"),
            Err(_) => no_config_home(),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg).join("winter-term")
        } else if let Some(home) = crate::paths::home_dir() {
            home.join(".config/winter-term")
        } else {
            no_config_home()
        }
    }
}

/// Where configuration lives when the system names no home for it.
fn no_config_home() -> PathBuf {
    std::env::temp_dir().join("winter-term-config")
}

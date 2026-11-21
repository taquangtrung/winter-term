//! Where configuration and state live, and the state file itself.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

// ========================================================================
// Locations and persisted state
// ========================================================================

/// The state file's name inside the state directory (see
/// [`winter_base::paths::state_file`]).
const STATE_FILE_NAME: &str = "state.json";

/// Persistent runtime state stored in `<state_dir>/state.json`.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AppState {
    /// The command line each project was last built with, by its root.
    #[serde(default)]
    pub compile_commands: BTreeMap<PathBuf, String>,
    /// Previously executed palette queries, ordered most-recent first: read
    /// once into the command palette's history, which `picker_history` keeps.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub palette_history: Vec<String>,
    /// Each picker's earlier queries, newest first.
    #[serde(default)]
    pub picker_history: misty::picker::History,
    /// How often and how lately each picker's rows were chosen.
    #[serde(default)]
    pub picker_recency: misty::picker::Recency,
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
        false => winter_base::paths::state_file(STATE_FILE_NAME),
    }
}

/// Load state from `<state_dir>/state.json`, returning a default on any error.
pub fn load_state() -> AppState {
    state_path()
        .map(|path| winter_base::persist::read_json_or_set_aside(&path))
        .unwrap_or_default()
}
/// Persist state to `<state_dir>/state.json` in one step (see
/// [`winter_base::persist::write_atomic`]). A failure is logged: the state is
/// conveniences (history, window geometry), not worth interrupting for.
pub fn save_state(state: &AppState) {
    let written = state_path()
        .ok_or_else(|| std::io::Error::other("no state directory"))
        .and_then(|path| {
            let json = serde_json::to_string(state).map_err(std::io::Error::other)?;
            winter_base::persist::write_atomic(&path, json)
        });
    if let Err(e) = written {
        eprintln!("winter: could not save state.json: {e}");
    }
}
/// Whether the project at `root` was trusted to run `tasks`, the text of
/// its tasks file, on its own.
pub fn are_tasks_trusted(root: &Path, tasks: &str) -> bool {
    if state_path().is_none() {
        return TEST_TRUST
            .with(|trust| trust.borrow().get(root).map(String::as_str) == Some(tasks));
    }
    load_state().trusted_tasks.get(root).map(String::as_str) == Some(tasks)
}

/// Trust the project at `root` to run `tasks`, the text of its tasks file,
/// on its own, until that text changes.
pub fn trust_tasks(root: &Path, tasks: &str) {
    if state_path().is_none() {
        TEST_TRUST.with(|trust| {
            trust
                .borrow_mut()
                .insert(root.to_path_buf(), tasks.to_string())
        });
        return;
    }
    let mut state = load_state();
    state
        .trusted_tasks
        .insert(root.to_path_buf(), tasks.to_string());
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

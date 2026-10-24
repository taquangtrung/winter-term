//! Where configuration and state live, and the state file itself.

use std::path::PathBuf;

// ========================================================================
// Locations and persisted state
// ========================================================================

/// The state file's name inside the state directory (see
/// [`crate::paths::state_file`]).
const STATE_FILE_NAME: &str = "state.json";

/// Persistent runtime state stored in `<state_dir>/state.json`.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AppState {
    /// Previously executed palette queries, ordered most-recent first.
    #[serde(default)]
    pub palette_history: Vec<String>,
    /// Directories opened in a listing, most recent first.
    #[serde(default)]
    pub recent_dirs: Vec<PathBuf>,
    /// Files opened in an editor or handed to `$EDITOR`, most recent first.
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
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
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|s| serde_json::from_str(&s).ok())
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
/// The configuration directory: `%APPDATA%\winter-term` on Windows;
/// `$XDG_CONFIG_HOME/winter-term` or `~/.config/winter-term` elsewhere.
pub(crate) fn config_dir() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("APPDATA") {
            Ok(appdata) => PathBuf::from(appdata).join("winter-term"),
            Err(_) => PathBuf::from("."),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
            PathBuf::from(xdg).join("winter-term")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".config/winter-term")
        } else {
            PathBuf::from(".")
        }
    }
}

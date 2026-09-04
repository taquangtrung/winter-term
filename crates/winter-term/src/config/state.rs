//! Where configuration and state live, and the state file itself.

use std::path::PathBuf;

// ========================================================================
// Locations and persisted state
// ========================================================================

/// Persistent runtime state stored in `<state_dir>/state.json`.
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct AppState {
    /// Previously executed palette queries, ordered most-recent first.
    #[serde(default)]
    pub palette_history: Vec<String>,
    /// Last known window dimensions in physical pixels.
    pub window_size: Option<(u32, u32)>,
}
/// Load state from `<state_dir>/state.json`, returning a default on any error.
pub fn load_state() -> AppState {
    let path = state_dir().join("state.json");
    std::fs::read_to_string(path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}
/// Persist state to `<state_dir>/state.json`, silently ignoring write errors.
pub fn save_state(state: &AppState) {
    let dir = state_dir();
    if std::fs::create_dir_all(&dir).is_ok() {
        if let Ok(json) = serde_json::to_string(state) {
            let _ = std::fs::write(dir.join("state.json"), json);
        }
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
/// The state directory: `%LOCALAPPDATA%\winter-term` on Windows (machine-local,
/// non-roaming — matching `state.json`'s regenerable, per-machine nature);
/// `$XDG_STATE_HOME/winter-term` or `~/.local/state/winter-term` elsewhere.
pub(crate) fn state_dir() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("LOCALAPPDATA") {
            Ok(local_appdata) => PathBuf::from(local_appdata).join("winter-term"),
            Err(_) => PathBuf::from("."),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            PathBuf::from(xdg).join("winter-term")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".local/state/winter-term")
        } else {
            PathBuf::from(".")
        }
    }
}

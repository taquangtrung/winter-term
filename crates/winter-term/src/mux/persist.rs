//! Session-definition persistence: `winter mux serve` writes each live
//! session's respawn recipe to `<state_dir>/mux-sessions.json` so a
//! restart can recreate them. Scrollback is not persisted: a respawned
//! session is a brand-new PTY with no history to replay.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

// ========================================================================
// Data Structures
// ========================================================================

/// A session's respawn recipe: name, command, and working directory.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct SessionDef {
    /// The command the session runs; `None` means the default shell.
    pub command: Option<String>,
    /// Directory the session starts in.
    pub cwd: Option<String>,
    /// The session's name, unique on this server.
    pub name: String,
}

/// The full set of session definitions persisted to disk.
#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SessionDefs {
    /// Every session the server should restore on start.
    #[serde(default)]
    pub sessions: Vec<SessionDef>,
}

// ========================================================================
// Persistence
// ========================================================================

/// Load session definitions from `<state_dir>/mux-sessions.json`,
/// returning an empty set on any error (missing file, corrupt JSON, or an
/// unwritable state directory) so a fresh or first-run server always
/// starts cleanly.
pub fn load_defs() -> SessionDefs {
    std::fs::read_to_string(defs_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

/// Persist session definitions to `<state_dir>/mux-sessions.json`,
/// silently ignoring write errors.
pub fn save_defs(defs: &SessionDefs) {
    let path = defs_path();
    let Some(parent) = path.parent() else {
        return;
    };
    if std::fs::create_dir_all(parent).is_ok() {
        if let Ok(json) = serde_json::to_string(defs) {
            let _ = std::fs::write(path, json);
        }
    }
}

/// `%LOCALAPPDATA%\winter-term\mux-sessions.json` on Windows (machine-local,
/// non-roaming); `$XDG_STATE_HOME/winter-term/mux-sessions.json` or
/// `~/.local/state/winter-term/mux-sessions.json` elsewhere.
fn defs_path() -> PathBuf {
    #[cfg(windows)]
    {
        match std::env::var("LOCALAPPDATA") {
            Ok(local_appdata) => PathBuf::from(local_appdata).join("winter-term/mux-sessions.json"),
            Err(_) => PathBuf::from("mux-sessions.json"),
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
            PathBuf::from(xdg).join("winter-term/mux-sessions.json")
        } else if let Ok(home) = std::env::var("HOME") {
            PathBuf::from(home).join(".local/state/winter-term/mux-sessions.json")
        } else {
            PathBuf::from("mux-sessions.json")
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_defs_path_names_the_shared_session_file() {
        // The directory varies with the environment, but every instance has to
        // agree on the file name or saved sessions are silently orphaned.
        assert_eq!(
            defs_path().file_name().and_then(|n| n.to_str()),
            Some("mux-sessions.json")
        );
    }

    // One test, not three: all three assert on `defs_path()`'s single
    // real file, and cargo runs tests in the same binary concurrently, so
    // splitting these into separate tests would race on that shared file.
    #[test]
    fn test_load_and_save_defs_round_trip_and_recover_from_corruption() {
        let path = defs_path();
        if path.exists() {
            std::fs::remove_file(&path).ok();
        }
        assert!(load_defs().sessions.is_empty());

        let defs = SessionDefs {
            sessions: vec![SessionDef {
                name: "work".to_string(),
                command: Some("bash".to_string()),
                cwd: Some("/tmp".to_string()),
            }],
        };
        save_defs(&defs);
        let loaded = load_defs();
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions[0].name, "work");
        assert_eq!(loaded.sessions[0].command.as_deref(), Some("bash"));
        assert_eq!(loaded.sessions[0].cwd.as_deref(), Some("/tmp"));

        // A partially written or hand-edited defs file must degrade to
        // "no sessions" on the next load, not a startup panic.
        std::fs::write(&path, "not json").unwrap();
        assert!(load_defs().sessions.is_empty());
    }
}

//! Session-definition persistence: `winter mux serve` writes each live
//! session's respawn recipe to `<state_dir>/mux-sessions.json` so a
//! restart can recreate them. Scrollback is not persisted: a respawned
//! session is a brand-new PTY with no history to replay.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

// ========================================================================
// Constants
// ========================================================================

/// The definitions' file name in the state directory.
const DEFS_FILE_NAME: &str = "mux-sessions.json";

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
/// returning an empty set on any error (missing file, corrupt JSON, or no
/// state directory) so a fresh or first-run server always starts cleanly.
pub fn load_defs() -> SessionDefs {
    defs_path()
        .map(|path| load_defs_from(&path))
        .unwrap_or_default()
}

/// Persist session definitions to `<state_dir>/mux-sessions.json` in one
/// step (see [`crate::persist::write_atomic`]), so a write cut short keeps
/// the previous definitions rather than leaving a file that loads as none.
pub fn save_defs(defs: &SessionDefs) {
    let written = match defs_path() {
        Some(path) => save_defs_to(&path, defs),
        None => Err(std::io::Error::other("no state directory")),
    };
    if let Err(e) = written {
        eprintln!("winter: could not save {DEFS_FILE_NAME}: {e}");
    }
}

fn load_defs_from(path: &Path) -> SessionDefs {
    crate::persist::read_json_or_set_aside(path)
}

fn save_defs_to(path: &Path, defs: &SessionDefs) -> std::io::Result<()> {
    let json = serde_json::to_string(defs).map_err(std::io::Error::other)?;
    crate::persist::write_atomic(path, json)
}

/// Where the definitions live (see [`crate::paths::state_file`]).
fn defs_path() -> Option<PathBuf> {
    crate::paths::state_file(DEFS_FILE_NAME)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_and_save_defs_round_trip_and_recover_from_corruption() {
        // Regression: this used the real defs file, so running the suite
        // deleted the developer's persisted sessions.
        let dir = std::env::temp_dir().join(format!("winter-mux-defs-{}", std::process::id()));
        let path = dir.join(DEFS_FILE_NAME);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(load_defs_from(&path).sessions.is_empty());

        let defs = SessionDefs {
            sessions: vec![SessionDef {
                name: "work".to_string(),
                command: Some("bash".to_string()),
                cwd: Some("/tmp".to_string()),
            }],
        };
        save_defs_to(&path, &defs).unwrap();
        let loaded = load_defs_from(&path);
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions[0].name, "work");
        assert_eq!(loaded.sessions[0].command.as_deref(), Some("bash"));
        assert_eq!(loaded.sessions[0].cwd.as_deref(), Some("/tmp"));

        // A partially written or hand-edited defs file must degrade to
        // "no sessions" on the next load, not a startup panic.
        std::fs::write(&path, "not json").unwrap();
        assert!(load_defs_from(&path).sessions.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

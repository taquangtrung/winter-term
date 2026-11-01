//! Locate: the whole disk searched for a name as the typing pauses, with
//! `plocate` or `locate` (or `mdfind` on macOS, `es` on Windows).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use winter_model::editor::OpenTarget;
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;
use winter_picker::{
    Action, ActionId, Feed, Highlights, MatchOn, Matching, Resume, Row, RowKey, Source,
};

use super::stream::{place_key, stream_lines};
use crate::window::picker::WindowOutcome;

// ========================================================================
// Constants
// ========================================================================

/// The shortest name searched for.
const MIN_LOCATE_CHARS: usize = 2;

/// How many paths a search keeps, and how many more each resume adds.
const MAX_RESULTS: usize = 500;

/// Said when no locate program is installed.
const NO_LOCATE: &str = "locate command not found (install plocate or locate)";

/// The one action of a row.
const OPEN: ActionId = ActionId("open");

// ========================================================================
// Data Structures
// ========================================================================

/// The disk, searched by name.
pub(in crate::window) struct LocateSource {
    cap: usize,
    paths: Arc<Mutex<HashMap<RowKey, PathBuf>>>,
}

// ========================================================================
// LocateSource
// ========================================================================

impl LocateSource {
    pub(in crate::window) fn new() -> Self {
        Self {
            cap: MAX_RESULTS,
            paths: Arc::default(),
        }
    }
}

impl Source<WindowOutcome> for LocateSource {
    fn id(&self) -> &'static str {
        "locate"
    }

    fn title(&self) -> String {
        "Locate Everywhere".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::BySource(MatchOn::LABEL)
    }

    fn resume(&self) -> Resume {
        Resume::Continue
    }

    fn feed(&mut self, query: &str) -> Feed {
        let pattern = query.trim();
        if pattern.chars().count() < MIN_LOCATE_CHARS {
            return Feed::Fixed(Vec::new());
        }
        let Some(program) = locate_program() else {
            return Feed::Failed(NO_LOCATE.to_string());
        };
        let mut command = Command::new(program);
        let cap = self.cap.to_string();
        match program {
            "mdfind" => command.args(["-name", pattern]),
            "es" => command.args(["-n", &cap, pattern]),
            _ => command.args(["-i", "-l", &cap, pattern]),
        };
        let paths = Arc::clone(&self.paths);
        stream_lines(command, self.cap, NO_LOCATE, move |line| {
            let path = PathBuf::from(String::from_utf8_lossy(line).trim_end().to_string());
            if path.as_os_str().is_empty() {
                return None;
            }
            let key = place_key(&path.to_string_lossy(), 0, 0);
            let row = path_row(key, &path);
            paths
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .insert(key, path);
            Some(row)
        })
    }

    fn highlights(&self, row: &Row, query: &str) -> Highlights {
        let part = query.trim().to_lowercase();
        let lower = row.label.to_lowercase();
        let label = match lower.rfind(&part) {
            Some(at) if !part.is_empty() => {
                let start = lower[..at].chars().count();
                (start..start + part.chars().count()).collect()
            }
            Some(_) | None => Vec::new(),
        };
        Highlights {
            label,
            ..Highlights::default()
        }
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        vec![Action {
            button: None,
            id: OPEN,
            title: "Open".to_string(),
        }]
    }

    fn run(&mut self, _action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        let path = self
            .paths
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&row)
            .cloned()?;
        Some(WindowOutcome::OpenFile(OpenTarget::file(path)))
    }

    fn refresh(&mut self) {
        self.cap += MAX_RESULTS;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A path's row: the path, home shortened, with a folder's or a file's icon.
fn path_row(key: RowKey, path: &Path) -> Row {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let icon = match path.is_dir() {
        true => IconKind::Dir {
            expanded: false,
            name,
        },
        false => IconKind::File { name },
    };
    Row::new(key, abbreviate_home(path)).with_icon(icon)
}

/// The locate program this machine has, looked for once.
fn locate_program() -> Option<&'static str> {
    static PROGRAM: OnceLock<Option<&'static str>> = OnceLock::new();
    *PROGRAM.get_or_init(|| {
        if cfg!(target_os = "macos") {
            return Some("mdfind");
        }
        if cfg!(target_os = "windows") {
            return Some("es");
        }
        ["plocate", "locate"].into_iter().find(|candidate| {
            Command::new(candidate)
                .arg("--version")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|status| status.success())
        })
    })
}

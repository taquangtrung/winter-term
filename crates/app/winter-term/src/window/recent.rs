//! What was used lately, kept across restarts: the files and directories
//! opened, and the command last run from the palette.
//!
//! Files and directories are remembered as they are opened, in this window
//! and in the state file every window shares, so the recent list carries
//! over from one session to the next.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The most files remembered.
const MAX_RECENT_FILES: usize = 50;

/// The most directories remembered.
const MAX_RECENT_DIRS: usize = 50;

// ========================================================================
// Data Structures
// ========================================================================

/// What was used lately: files and directories opened, and the command last
/// run from the palette.
#[derive(Default)]
pub(in crate::window) struct RecentLists {
    /// Directories opened in a listing, most recent first: this window's own,
    /// on top of what was on disk when it started.
    pub(in crate::window) dirs: Vec<PathBuf>,
    /// Files opened in an editor or handed to `$EDITOR`, most recent first.
    pub(in crate::window) files: Vec<PathBuf>,
    /// The last command run from the palette, for running it again.
    pub(in crate::window) last_palette_command: Option<String>,
}

// ========================================================================
// RecentLists
// ========================================================================

impl RecentLists {
    /// The lists as the saved state left them.
    pub(in crate::window) fn new(dirs: Vec<PathBuf>, files: Vec<PathBuf>) -> Self {
        Self {
            dirs,
            files,
            last_palette_command: None,
        }
    }
}

// ========================================================================
// App: remembering
// ========================================================================

impl App {
    /// Remember `path` as the file opened most recently.
    pub(in crate::window) fn record_recent_file(&mut self, path: &Path) {
        // A remote file's path is a URI, which is no local absolute path.
        if !path.is_absolute() && winter_base::remote::parse(path).is_none() {
            return;
        }
        remember(&mut self.recent.files, path, MAX_RECENT_FILES);
        self.update_app_state(|state| remember(&mut state.recent_files, path, MAX_RECENT_FILES));
    }

    /// Remember `path` as the directory opened most recently.
    pub(in crate::window) fn record_recent_dir(&mut self, path: &Path) {
        if !path.is_absolute() {
            return;
        }
        remember(&mut self.recent.dirs, path, MAX_RECENT_DIRS);
        self.update_app_state(|state| remember(&mut state.recent_dirs, path, MAX_RECENT_DIRS));
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Put `path` at the front of `list`, dropping an older copy of it and
/// whatever falls past `max`.
fn remember(list: &mut Vec<PathBuf>, path: &Path, max: usize) {
    list.retain(|kept| kept != path);
    list.insert(0, path.to_path_buf());
    list.truncate(max);
}

/// `lists` one after another with each path kept once, at its first place:
/// this window's own history ahead of what other windows saved.
pub(in crate::window) fn merged<const N: usize>(lists: [Vec<PathBuf>; N]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    lists
        .into_iter()
        .flatten()
        .filter(|path| seen.insert(path.clone()))
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_remembering_a_path_again_moves_it_to_the_front_once() {
        let mut list = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        remember(&mut list, Path::new("/b"), 3);
        assert_eq!(list, [PathBuf::from("/b"), PathBuf::from("/a")]);
        remember(&mut list, Path::new("/c"), 2);
        assert_eq!(list, [PathBuf::from("/c"), PathBuf::from("/b")], "capped");
    }

    #[test]
    fn test_merged_lists_keep_each_path_once_at_its_first_place() {
        let mine = vec![PathBuf::from("/b"), PathBuf::from("/a")];
        let saved = vec![PathBuf::from("/a"), PathBuf::from("/c")];
        let all = merged([mine, saved]);
        assert_eq!(all, ["/b", "/a", "/c"].map(PathBuf::from));
    }
}

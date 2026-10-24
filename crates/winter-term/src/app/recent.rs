//! The recent list (`C-S-z`): every tab open now, then the files and the
//! directories opened lately, as one list to go back to any of them from.
//!
//! Files and directories are remembered as they are opened, in this window
//! and in the state file every window shares, so the list carries over from
//! one session to the next.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::editor::OpenTarget;
use crate::model::layout::TabId;
use crate::model::paint::IconKind;
use crate::model::palette::{Palette, PaletteEntry};
use crate::model::path::abbreviate_home;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// The most files remembered.
const MAX_RECENT_FILES: usize = 50;

/// The most directories remembered.
const MAX_RECENT_DIRS: usize = 50;

/// What a row's action starts with, by what the row stands for.
const TAB_ROW: &str = "tab:";
const FILE_ROW: &str = "file:";
const DIR_ROW: &str = "dir:";

/// What each kind of row says on its right, after the pane a tab is in.
const FILE_HINT: &str = "file";
const DIR_HINT: &str = "directory";

// ========================================================================
// App: the recent list
// ========================================================================

impl App {
    /// Remember `path` as the file opened most recently.
    pub(crate) fn record_recent_file(&mut self, path: &Path) {
        // A remote file's path is a URI, which is no local absolute path.
        if !path.is_absolute() && crate::remote::parse(path).is_none() {
            return;
        }
        remember(&mut self.recent_files, path, MAX_RECENT_FILES);
        self.update_app_state(|state| remember(&mut state.recent_files, path, MAX_RECENT_FILES));
    }

    /// Remember `path` as the directory opened most recently.
    pub(crate) fn record_recent_dir(&mut self, path: &Path) {
        if !path.is_absolute() {
            return;
        }
        remember(&mut self.recent_dirs, path, MAX_RECENT_DIRS);
        self.update_app_state(|state| remember(&mut state.recent_dirs, path, MAX_RECENT_DIRS));
    }

    /// Open the recent list: the tabs open in every pane, then the files
    /// opened lately, then the directories, newest first within each.
    ///
    /// A local file or directory that is gone is left out rather than
    /// offered to fail. The directories take in where the shells have been as well as
    /// what was listed, since `cd` is how most of them are reached.
    pub(crate) fn open_recent_list(&mut self) {
        let mut rows = self.open_tab_rows();
        let saved = crate::config::load_state();
        for path in merged([self.recent_files.clone(), saved.recent_files]) {
            // A remote file is offered without a round trip to check on it.
            if crate::remote::parse(&path).is_none() && !path.is_file() {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            rows.push(
                PaletteEntry::new(
                    format!("{FILE_ROW}{}", path.display()),
                    abbreviate_home(&path),
                    FILE_HINT,
                )
                .with_icon(IconKind::File { name }),
            );
        }
        for path in merged([
            self.recent_dirs.clone(),
            saved.recent_dirs,
            self.shell_dirs(),
        ]) {
            if !path.is_dir() {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            rows.push(
                PaletteEntry::new(
                    format!("{DIR_ROW}{}", path.display()),
                    abbreviate_home(&path),
                    DIR_HINT,
                )
                .with_icon(IconKind::Dir {
                    expanded: false,
                    name,
                }),
            );
        }
        self.open_palette(Palette::open_recent(rows));
        self.dirty = true;
    }

    /// Act on a row of the recent list: go to the tab, open the file in the
    /// editor that shows it, or list the directory.
    pub(crate) fn choose_recent_row(&mut self, action: &str) {
        if let Some(tab) = action
            .strip_prefix(TAB_ROW)
            .and_then(|id| id.parse::<u64>().ok())
        {
            self.switch_to_tab(TabId(tab));
            return;
        }
        if let Some(path) = action.strip_prefix(FILE_ROW) {
            self.open_path_editor(OpenTarget::file(PathBuf::from(path)));
            return;
        }
        if let Some(path) = action.strip_prefix(DIR_ROW) {
            self.open_dir_editor_at(PathBuf::from(path));
        }
    }

    /// One row per tab open in any pane, in pane order, each saying which
    /// pane it is in.
    fn open_tab_rows(&self) -> Vec<PaletteEntry> {
        let mut rows = Vec::new();
        for (pane_index, &shown) in self.layout().shown_tabs().iter().enumerate() {
            for tab in self.layout().tabs_in_pane(shown) {
                rows.push(
                    PaletteEntry::new(
                        format!("{TAB_ROW}{}", tab.0),
                        self.tab_title(tab),
                        format!("pane {}", pane_index + 1),
                    )
                    .with_icon(self.tab_icon_kind(tab)),
                );
            }
        }
        rows
    }

    /// Where the shells have been, newest first, from each terminal's record
    /// of the directory every command ran in.
    fn shell_dirs(&self) -> Vec<PathBuf> {
        self.open_tabs
            .terminals()
            .flat_map(|(_, terminal)| {
                terminal
                    .scrollback()
                    .blocks()
                    .iter()
                    .rev()
                    .filter_map(|block| block.cwd.as_deref())
                    .map(PathBuf::from)
                    .collect::<Vec<_>>()
            })
            .collect()
    }
}

// ========================================================================
// Free functions
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
fn merged<const N: usize>(lists: [Vec<PathBuf>; N]) -> Vec<PathBuf> {
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
    use crate::model::palette::PaletteMode;

    #[test]
    fn test_remembering_a_path_again_moves_it_to_the_front_once() {
        let mut list = vec![PathBuf::from("/a"), PathBuf::from("/b")];
        remember(&mut list, Path::new("/b"), 3);
        assert_eq!(list, [PathBuf::from("/b"), PathBuf::from("/a")]);
        remember(&mut list, Path::new("/c"), 2);
        assert_eq!(list, [PathBuf::from("/c"), PathBuf::from("/b")], "capped");
    }

    #[test]
    fn test_the_recent_list_shows_tabs_then_files_then_directories() {
        let dir = std::env::temp_dir().join(format!("winter-recent-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("notes.md");
        std::fs::write(&file, "x").expect("temp file");

        let mut app = App::new();
        app.record_recent_file(&file);
        app.record_recent_file(&dir.join("gone.md"));
        app.record_recent_dir(&dir);
        app.open_recent_list();

        let palette = app.palette().expect("the list opens");
        assert_eq!(palette.mode, PaletteMode::Recent);
        let actions: Vec<&str> = palette
            .entries
            .iter()
            .map(|entry| entry.action.as_str())
            .collect();
        let tab = actions
            .iter()
            .position(|action| action.starts_with(TAB_ROW))
            .expect("the open tab is listed");
        let file_row = format!("{FILE_ROW}{}", file.display());
        let dir_row = format!("{DIR_ROW}{}", dir.display());
        let at = |row: &str| actions.iter().position(|action| *action == row);
        assert!(tab < at(&file_row).expect("the file is listed"));
        assert!(at(&file_row) < at(&dir_row), "files before directories");
        assert!(
            !actions.iter().any(|action| action.ends_with("gone.md")),
            "a file that is gone is left out"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

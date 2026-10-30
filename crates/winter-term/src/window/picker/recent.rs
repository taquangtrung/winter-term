//! The recent list (`C-S-z`): every tab open now, then the buried buffers,
//! then the files and the directories opened lately, as one list to go back
//! to any of them from.
//!
//! Files and directories are remembered as they are opened, in this window
//! and in the state file every window shares, so the list carries over from
//! one session to the next.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::model::editor::OpenTarget;
use crate::model::layout::TabId;
use crate::model::paint::IconKind;
use crate::model::picker::{Picker, PickerEntry};
use crate::model::path::abbreviate_home;

use crate::window::picker::buffers::BUFFER_ROW;
use crate::window::App;

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


// ========================================================================
// Data Structures
// ========================================================================

/// What was used lately, kept across restarts: commands run from the
/// palette, and files and directories opened.
pub(in crate::window) struct RecentLists {
    /// Previously executed palette queries, persisted across sessions.
    pub(in crate::window) palette: Vec<String>,
    /// Directories opened in a listing, most recent first: this window's own,
    /// on top of what was on disk when it started (see `recent`).
    pub(in crate::window) dirs: Vec<PathBuf>,
    /// Files opened in an editor or handed to `$EDITOR`, most recent first.
    pub(in crate::window) files: Vec<PathBuf>,
}

// ========================================================================
// RecentLists Implementations
// ========================================================================

impl RecentLists {
    /// The lists as the saved state left them.
    pub(in crate::window) fn new(
        palette: Vec<String>,
        dirs: Vec<PathBuf>,
        files: Vec<PathBuf>,
    ) -> Self {
        Self {
            palette,
            dirs,
            files,
        }
    }
}

// ========================================================================
// App: the recent list
// ========================================================================

impl App {
    /// Remember `path` as the file opened most recently.
    pub(in crate::window) fn record_recent_file(&mut self, path: &Path) {
        // A remote file's path is a URI, which is no local absolute path.
        if !path.is_absolute() && crate::remote::parse(path).is_none() {
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

    /// Open the recent list: the tabs open in every pane, then the buried
    /// buffers, then the files opened lately, then the directories, newest
    /// first within each.
    ///
    /// A local file or directory that is gone is left out rather than
    /// offered to fail, and a file a buried buffer holds is offered as that
    /// buffer alone. The directories take in where the shells have been as well as
    /// what was listed, since `cd` is how most of them are reached.
    pub(in crate::window) fn open_recent_list(&mut self) {
        let mut rows = self.open_tab_rows();
        rows.extend(self.buried_rows());
        let saved = crate::config::load_state();
        for path in merged([self.recent.files.clone(), saved.recent_files]) {
            // A remote file is offered without a round trip to check on it.
            if crate::remote::parse(&path).is_none() && !path.is_file() {
                continue;
            }
            if self.buried_holding(&path) {
                continue;
            }
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().to_string())
                .unwrap_or_default();
            rows.push(
                PickerEntry::new(
                    format!("{FILE_ROW}{}", path.display()),
                    abbreviate_home(&path),
                    "",
                )
                .with_icon(IconKind::File { name }),
            );
        }
        for path in merged([
            self.recent.dirs.clone(),
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
                PickerEntry::new(
                    format!("{DIR_ROW}{}", path.display()),
                    abbreviate_home(&path),
                    "",
                )
                .with_icon(IconKind::Dir {
                    expanded: false,
                    name,
                }),
            );
        }
        self.open_picker(Picker::open_recent(rows));
        self.dirty = true;
    }

    /// Act on a row of the recent list: go to the tab, bring a buried buffer
    /// back, open the file in the editor that shows it, or list the directory.
    pub(in crate::window) fn choose_recent_row(&mut self, action: &str) {
        if action.starts_with(BUFFER_ROW) {
            self.choose_buffer_row(action);
            return;
        }
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
    /// pane it is in. The dock's tabs are listed while it is put away too,
    /// since choosing one brings it back.
    fn open_tab_rows(&self) -> Vec<PickerEntry> {
        let mut rows = Vec::new();
        for (pane_index, tabs) in self.layout().pane_members().into_iter().enumerate() {
            for tab in tabs {
                rows.push(
                    PickerEntry::new(
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

    /// Whether a buried buffer holds the file at `path`.
    fn buried_holding(&self, path: &Path) -> bool {
        self.open_tabs.buried().iter().any(|tab| {
            self.open_tabs
                .editor(tab)
                .is_some_and(|slot| slot.editor.holds_file(path))
        })
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
    use crate::model::picker::PickerMode;

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

        let palette = app.chrome.picker().expect("the list opens");
        assert_eq!(palette.mode, PickerMode::Recent);
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
        let tab_entry = palette
            .entries
            .iter()
            .find(|entry| entry.action.starts_with(TAB_ROW))
            .expect("the open tab entry");
        assert!(
            tab_entry.shortcut.starts_with("pane "),
            "open tab keeps its pane hint"
        );
        let file_entry = palette
            .entries
            .iter()
            .find(|entry| entry.action == file_row)
            .expect("the file entry");
        assert!(
            file_entry.shortcut.is_empty(),
            "file entries do not show right-most hint"
        );
        let dir_entry = palette
            .entries
            .iter()
            .find(|entry| entry.action == dir_row)
            .expect("the dir entry");
        assert!(
            dir_entry.shortcut.is_empty(),
            "directory entries do not show right-most hint"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_file_a_buried_buffer_holds_is_listed_once_as_that_buffer() {
        use crate::editors::hex::HexEditor;
        use crate::window::editor::EditorKind;

        let dir = std::env::temp_dir().join(format!("winter-recent-buried-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("data.bin");
        std::fs::write(&file, [0u8, 1, 2]).expect("temp file");

        let mut app = App::new();
        let tab = app.show_editor(EditorKind::Hex, Box::new(HexEditor::new(file.clone())));
        app.record_recent_file(&file);
        app.close_editor(tab);
        app.open_recent_list();

        let actions: Vec<&str> = app
            .chrome.picker()
            .expect("the list opens")
            .entries
            .iter()
            .map(|entry| entry.action.as_str())
            .collect();
        let buffer_row = format!("{BUFFER_ROW}{}", tab.0);
        assert!(actions.contains(&buffer_row.as_str()), "{actions:?}");
        assert!(
            !actions.contains(&format!("{FILE_ROW}{}", file.display()).as_str()),
            "and not again as a file: {actions:?}"
        );
        app.choose_recent_row(&buffer_row);
        assert_eq!(app.layout().focused(), tab, "picking it brings it back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

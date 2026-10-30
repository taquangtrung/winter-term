//! The file pickers: every file under a directory, to fuzzy-find one by its
//! path and open it. `project_files` lists the focused tab's project (see
//! [`crate::project::root`]); `find_file`
//! lists only what is under the focused tab's own working directory.
//!
//! The files are the ones git knows of or would, its ignored ones left out,
//! else what `rg --files` lists, which honours the same ignore files.

use std::path::{Path, PathBuf};

use crate::model::paint::IconKind;
use crate::model::picker::{Picker, PickerEntry, PickerMode};
use crate::model::path::abbreviate_home;
use crate::project;
use crate::process;

use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The most files listed: past this the list is too long to read and slow
/// to filter, and the query is what finds a file anyway.
const MAX_FILES: usize = 50_000;

/// Said when nothing could list the project.
const NO_FILES: &str = "no files listed: not a git repository, and ripgrep (rg) is not installed";

// ========================================================================
// App: project files
// ========================================================================

impl App {
    /// Open the file picker over the focused tab's project, or put it away
    /// when a file picker is the one showing.
    pub(in crate::window) fn open_project_files(&mut self) {
        let root = project::root(&self.focused_start_dir());
        self.open_file_picker(root);
    }

    /// Open the file picker over what is under the focused tab's working
    /// directory, or put it away when a file picker is the one showing.
    pub(in crate::window) fn open_find_file(&mut self) {
        let root = self.focused_start_dir();
        self.open_file_picker(root);
    }

    /// Open the file picker over every file under `root`, or put it away
    /// when one is showing: the chord that opened it closes it.
    fn open_file_picker(&mut self, root: PathBuf) {
        let showing = self
            .chrome.picker()
            .is_some_and(|palette| palette.mode == PickerMode::ProjectFiles);
        if showing {
            self.dismiss_overlay();
            return;
        }
        let Some(files) = list_files(&root).filter(|files| !files.is_empty()) else {
            self.set_error(NO_FILES);
            return;
        };
        let entries = files
            .into_iter()
            .map(|relative| {
                let name = relative.rsplit('/').next().unwrap_or(&relative).to_string();
                let path = root.join(&relative);
                PickerEntry::new(path.to_string_lossy().into_owned(), relative, String::new())
                    .with_icon(IconKind::File { name })
            })
            .collect();
        let title = format!("Files in {}", abbreviate_home(&root));
        self.open_picker(Picker::open_project_files(entries, title));
        self.dirty = true;
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The files under `root`, as `/`-separated paths from it, sorted: git's
/// list where `root` is a repository, else ripgrep's. `None` when neither
/// could list it.
fn list_files(root: &Path) -> Option<Vec<String>> {
    let git_args = [
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ];
    let rg_args = ["--files", "--null"];
    let listing = output_in(root, "git", &git_args).or_else(|| output_in(root, "rg", &rg_args))?;
    Some(parse_listing(&listing))
}

/// The paths in a NUL-separated listing, once each, sorted and capped.
fn parse_listing(listing: &str) -> Vec<String> {
    let mut files: Vec<String> = listing
        .split('\0')
        .filter(|path| !path.is_empty())
        .map(|path| path.replace('\\', "/"))
        .collect();
    files.sort();
    // A file both tracked and changed is listed twice by `git ls-files`.
    files.dedup();
    files.truncate(MAX_FILES);
    files
}

/// What `program` prints when run in `dir`, when it succeeds.
fn output_in(dir: &Path, program: &str, args: &[&str]) -> Option<String> {
    let output = process::command(program)
        .args(args)
        .current_dir(dir)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_listing_is_read_once_each_in_path_order() {
        let listing = "src/main.rs\0README.md\0src/main.rs\0docs\\guide.md\0";
        assert_eq!(
            parse_listing(listing),
            ["README.md", "docs/guide.md", "src/main.rs"]
        );
    }

    #[test]
    fn test_the_picker_lists_this_repository_and_its_chord_toggles_it() {
        let mut app = App::new();
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let root = project::root(&here);
        let files = list_files(&root).expect("this checkout lists");
        assert!(
            files.iter().any(|file| file == "Cargo.toml"),
            "the workspace's own manifest is in it"
        );

        app.open_project_files();
        let palette = app.chrome.picker().expect("the picker");
        assert_eq!(palette.mode, PickerMode::ProjectFiles);
        app.open_project_files();
        assert!(app.chrome.picker().is_none(), "the chord again puts it away");
    }

    #[test]
    fn test_find_file_lists_only_what_is_under_the_working_directory() {
        // This crate's own directory is inside the workspace's repository: the
        // project picker reaches the workspace's top, find-file stays here.
        let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let files = list_files(&here).expect("this checkout lists");
        assert!(
            files.iter().any(|file| file == "Cargo.toml"),
            "its own manifest"
        );
        assert!(
            files.iter().all(|file| !file.starts_with("crates/")),
            "nothing from beside it in the workspace"
        );
    }
}

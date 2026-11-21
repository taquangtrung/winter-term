//! The Files view, a sidebar's: the project's folders as a tree, folders
//! first, each opened in place. Opening a file shows it in the main area;
//! `t` opens the project in Dir there.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use misty::{Element, HostedView};

use winter_model::editor::{EditorOutcome, OpenTarget};

use super::{ListAct, ListRow, PanelList, PanelView, list_view};

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Files";

/// What it says with nothing to list.
const EMPTY: &str = "This folder is empty";

/// How often the open folders are read again, for what changed on disk.
const READ_EVERY: Duration = Duration::from_secs(2);

/// The entries never listed: git's own folder.
const HIDDEN: &[&str] = &[".git"];

// ========================================================================
// Data Structures
// ========================================================================

/// The project's folders as a tree.
#[derive(Debug)]
pub(in crate::window) struct FilesView {
    /// Every entry listed, in the order the rows show them.
    entries: Vec<Entry>,
    list: PanelList,
    /// The folders open, by path.
    open: BTreeSet<PathBuf>,
    /// When the open folders were last read.
    read_at: Instant,
    /// The project's folder, the tree's root.
    root: PathBuf,
}

/// One entry listed.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Entry {
    /// How deep under the root it is, the root's own entries at nought.
    depth: usize,
    is_dir: bool,
    path: PathBuf,
}

// ========================================================================
// FilesView Implementations
// ========================================================================

impl FilesView {
    /// The tree of the project at `root`, its folders closed.
    pub(in crate::window) fn new(root: PathBuf) -> Self {
        let mut view = Self {
            entries: Vec::new(),
            list: PanelList::default(),
            open: BTreeSet::new(),
            read_at: Instant::now(),
            root,
        };
        view.read();
        view
    }

    /// Show the project at `root` instead, its folders closed.
    pub(in crate::window) fn set_root(&mut self, root: PathBuf) {
        if root != self.root {
            self.root = root;
            self.open.clear();
            self.read();
        }
    }

    /// Read the root and every open folder again, and list them.
    fn read(&mut self) {
        self.entries.clear();
        let root = self.root.clone();
        self.read_dir(&root, 0);
        let rows = self
            .entries
            .iter()
            .enumerate()
            .map(|(key, entry)| self.entry_row(key, entry))
            .collect();
        self.list.set_rows(rows);
        self.read_at = Instant::now();
    }

    /// List the entries of `dir`, `depth` deep, and those of each open
    /// folder among them under it.
    fn read_dir(&mut self, dir: &Path, depth: usize) {
        for entry in listed(dir) {
            let open = entry.is_dir && self.open.contains(&entry.path);
            let path = entry.path.clone();
            self.entries.push(Entry { depth, ..entry });
            if open {
                self.read_dir(&path, depth + 1);
            }
        }
    }

    /// The row of `entry`, the `key`th.
    fn entry_row(&self, key: usize, entry: &Entry) -> ListRow {
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default();
        let relative = entry.path.strip_prefix(&self.root).unwrap_or(&entry.path);
        ListRow {
            depth: entry.depth,
            fold: entry.is_dir.then(|| self.open.contains(&entry.path)),
            key,
            label: name,
            tooltip: Some(relative.to_string_lossy().to_string()),
            ..ListRow::default()
        }
    }

    /// The entry the cursor is on.
    fn selected(&self) -> Option<&Entry> {
        self.list
            .selected_row()
            .and_then(|row| self.entries.get(row.key))
    }

    /// Open or close the folder the cursor is on; the cursor on a file or an
    /// open folder's entry closing goes to its folder.
    fn fold(&mut self, open: bool) {
        let Some(entry) = self.selected().cloned() else {
            return;
        };
        match (entry.is_dir, open) {
            (true, true) => {
                self.open.insert(entry.path);
            }
            (true, false) if self.open.contains(&entry.path) => {
                self.open.remove(&entry.path);
            }
            (_, false) => {
                let Some(parent) = entry.path.parent().filter(|parent| *parent != self.root) else {
                    return;
                };
                let parent = parent.to_path_buf();
                self.open.remove(&parent);
                self.read();
                if let Some(at) = self.entries.iter().position(|entry| entry.path == parent) {
                    self.list.select_key(at, false);
                }
                return;
            }
            (false, true) => return,
        }
        self.read();
    }
}

impl PanelView for FilesView {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn list(&self) -> &PanelList {
        &self.list
    }

    fn list_mut(&mut self) -> &mut PanelList {
        &mut self.list
    }

    fn act(&mut self, act: ListAct) -> EditorOutcome {
        match act {
            ListAct::Open => match self.selected() {
                Some(entry) if entry.is_dir => {
                    let open = !self.open.contains(&entry.path);
                    self.fold(open);
                }
                Some(entry) => {
                    return EditorOutcome::OpenPath(OpenTarget::file(entry.path.clone()));
                }
                None => {}
            },
            ListAct::Expand => self.fold(true),
            ListAct::Collapse => self.fold(false),
            ListAct::Refresh => self.read(),
            ListAct::OpenTool => {
                return EditorOutcome::OpenPath(OpenTarget::file(self.root.clone()));
            }
            ListAct::Down | ListAct::First | ListAct::Last | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        Some(self.read_at + READ_EVERY)
    }

    fn on_tick(&mut self) -> EditorOutcome {
        self.read();
        EditorOutcome::Consumed
    }
}

impl HostedView for FilesView {
    fn view(&self) -> Element<'_, Self> {
        list_view(self, EMPTY)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The entries of `dir` a tree lists, folders first, each group by name
/// without regard to case; nothing where it cannot be read.
fn listed(dir: &Path) -> Vec<Entry> {
    let Ok(read) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut entries: Vec<Entry> = read
        .flatten()
        .filter(|entry| !HIDDEN.contains(&entry.file_name().to_string_lossy().as_ref()))
        .map(|entry| Entry {
            depth: 0,
            is_dir: entry.path().is_dir(),
            path: entry.path(),
        })
        .collect();
    entries.sort_by_key(|entry| {
        let name = entry
            .path
            .file_name()
            .map(|name| name.to_string_lossy().to_lowercase());
        (!entry.is_dir, name)
    });
    entries
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A folder that removes itself on drop.
    struct Project(PathBuf);

    impl Drop for Project {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    /// A project of a folder `src` holding `main.rs`, a file `README.md`, and
    /// git's folder.
    fn project() -> Project {
        let root = std::env::temp_dir().join(format!("winter-files-view-{}", std::process::id()));
        fs::create_dir_all(root.join("src")).expect("src");
        fs::create_dir_all(root.join(".git")).expect(".git");
        fs::write(root.join("src/main.rs"), "").expect("main.rs");
        fs::write(root.join("README.md"), "").expect("README.md");
        Project(root)
    }

    fn labels(view: &FilesView) -> Vec<(usize, String)> {
        view.list
            .rows
            .iter()
            .map(|row| (row.depth, row.label.clone()))
            .collect()
    }

    #[test]
    fn test_folders_come_first_git_s_is_left_out_and_a_folder_opens_in_place() {
        let root = project();
        let mut view = FilesView::new(root.0.clone());
        assert_eq!(
            labels(&view),
            [(0, "src".to_string()), (0, "README.md".to_string())]
        );
        view.act(ListAct::Expand);
        assert_eq!(
            labels(&view),
            [
                (0, "src".to_string()),
                (1, "main.rs".to_string()),
                (0, "README.md".to_string())
            ]
        );
        view.list_mut().move_cursor(ListAct::Down);
        let opened = view.act(ListAct::Open);
        assert!(
            matches!(&opened, EditorOutcome::OpenPath(target) if target.path.ends_with("src/main.rs")),
            "a file opens: {opened:?}"
        );
        view.act(ListAct::Collapse);
        assert_eq!(
            labels(&view).len(),
            2,
            "closing from a file closes its folder"
        );
        assert_eq!(
            view.list.selected_row().map(|row| row.label.as_str()),
            Some("src")
        );
    }
}

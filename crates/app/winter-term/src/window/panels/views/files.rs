//! The Files view, a sidebar's: the project's folders as a tree, folders
//! first, each opened in place. Opening a file shows it in the main area;
//! `t` opens the project in Dir there. In sight, the root and each open
//! folder are watched as Dir watches them, and read again only when one
//! changes; out of sight, nothing is read or watched.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::{Element, HostedView};

use winter_dir::DirWatch;
use winter_dir::tree::Folds;
use winter_model::editor::{EditorOutcome, OpenTarget};

use super::{ListAct, ListRow, PanelList, PanelView, list_view};

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Files";

/// What it says with nothing to list.
const EMPTY: &str = "This folder is empty";

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
    /// The folders open.
    folds: Folds,
    /// Whether the view is in sight, the only time it reads or watches.
    shown: bool,
    list: PanelList,
    /// The project's folder, the tree's root.
    root: PathBuf,
    /// The notifications on the root and the open folders, `None` where the
    /// platform cannot watch them: read again only when asked, then.
    watch: Option<DirWatch>,
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
    /// The tree of the project at `root`, its folders closed, read once it
    /// comes into sight.
    pub(in crate::window) fn new(root: PathBuf) -> Self {
        Self {
            entries: Vec::new(),
            folds: Folds::new(),
            shown: false,
            list: PanelList::default(),
            root,
            watch: None,
        }
    }

    /// Show the project at `root` instead, its folders closed.
    pub(in crate::window) fn set_root(&mut self, root: PathBuf) {
        if root != self.root {
            self.folds = Folds::new();
            self.root = root;
            match self.shown {
                true => self.come_into_sight(),
                false => {
                    self.entries.clear();
                    self.list.set_rows(Vec::new());
                }
            }
        }
    }

    /// Watch the root and the open folders, and read them.
    fn come_into_sight(&mut self) {
        self.watch = DirWatch::start(&self.root, &self.folds);
        self.read();
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
    }

    /// List the entries of `dir`, `depth` deep, and those of each open
    /// folder among them under it.
    fn read_dir(&mut self, dir: &Path, depth: usize) {
        for entry in listed(dir) {
            let open = entry.is_dir && self.folds.is_expanded(&entry.path);
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
            fold: entry.is_dir.then(|| self.folds.is_expanded(&entry.path)),
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
        let target = match (entry.is_dir, open) {
            (true, true) => entry.path,
            (true, false) if self.folds.is_expanded(&entry.path) => entry.path,
            (_, false) => {
                let Some(parent) = entry.path.parent().filter(|parent| *parent != self.root) else {
                    return;
                };
                parent.to_path_buf()
            }
            (false, true) => return,
        };
        if self.folds.is_expanded(&target) != open {
            self.folds.toggle(&target);
        }
        if let Some(watch) = self.watch.as_mut() {
            watch.sync_folds(&self.folds);
        }
        self.read();
        if let Some(at) = self.entries.iter().position(|entry| entry.path == target) {
            self.list.select_key(at, false);
        }
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
                    let open = !self.folds.is_expanded(&entry.path);
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

    /// Only when the watch has news, which wakes the window itself.
    fn next_tick(&self) -> Option<Instant> {
        self.watch.as_ref()?.next_look(Instant::now())
    }

    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        match shown {
            true => self.come_into_sight(),
            false => self.watch = None,
        }
        EditorOutcome::Consumed
    }

    fn on_tick(&mut self) -> EditorOutcome {
        if self
            .watch
            .as_mut()
            .is_some_and(|watch| watch.changed(Instant::now()))
        {
            self.read();
        }
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
    fn project(tag: &str) -> Project {
        let root =
            std::env::temp_dir().join(format!("winter-files-view-{tag}-{}", std::process::id()));
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
    fn test_out_of_sight_the_tree_is_neither_read_nor_watched() {
        let root = project("sight");
        let mut view = FilesView::new(root.0.clone());
        assert!(view.list.rows.is_empty(), "nothing read before it is seen");
        assert!(view.watch.is_none());
        view.set_shown(true);
        assert_eq!(labels(&view).len(), 2, "read once in sight");
        view.set_shown(false);
        assert!(view.watch.is_none(), "the watch let go");
        assert_eq!(view.next_tick(), None);
    }

    #[test]
    fn test_a_quiet_tree_asks_for_no_tick_and_a_new_file_is_read_in_when_watched() {
        let root = project("watch");
        let mut view = FilesView::new(root.0.clone());
        view.set_shown(true);
        if view.watch.is_none() {
            // A platform that cannot watch asks for nothing at all.
            assert_eq!(view.next_tick(), None);
            return;
        }
        assert_eq!(view.next_tick(), None, "nothing changed, no tick");
        fs::write(root.0.join("new.txt"), "").expect("new file");
        let end = Instant::now() + std::time::Duration::from_secs(5);
        while Instant::now() < end && !labels(&view).iter().any(|(_, name)| name == "new.txt") {
            if view.next_tick().is_some_and(|at| at <= Instant::now()) {
                view.on_tick();
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            labels(&view).iter().any(|(_, name)| name == "new.txt"),
            "the change was read in"
        );
    }

    #[test]
    fn test_folders_come_first_git_s_is_left_out_and_a_folder_opens_in_place() {
        let root = project("tree");
        let mut view = FilesView::new(root.0.clone());
        view.set_shown(true);
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

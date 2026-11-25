//! The Files view, a sidebar's: the project's folders as a tree, folders
//! first, each opened in place. Opening a file shows it in the main area;
//! `t` opens the project in Dir there.
//!
//! In a repository, git's state marks each changed file and the folders
//! holding it, and what git ignores is left out until `.` shows it, as Dir
//! reads them ([`winter_dir::vcs`]). The repository's watch, shared with
//! the Git view, says when to read the tree and the status again; outside
//! one, the root and each open folder are watched as Dir watches them. Out
//! of sight, nothing is read or watched.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::{Element, HostedView, ThemeToken};

use winter_dir::DirWatch;
use winter_dir::tree::Folds;
use winter_dir::vcs::{self, GitMarks, GitStatus, TAG_ROOT, TAG_STATUS};
use winter_model::background::Background;
use winter_model::editor::{EditorOutcome, JobReply, JobRequest, OpenTarget};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyTable};
use winter_model::paint::IconKind;
use winter_vcs::watch::TreeWatch;

use super::{ListAct, ListRow, Mark, PanelList, PanelView, RowIcon, list_view};

// ========================================================================
// Constants
// ========================================================================

/// What the view is called.
const TITLE: &str = "Files";

/// What it says with nothing to list.
const EMPTY: &str = "This folder is empty";

/// The entries never listed: git's own folder.
const HIDDEN: &[&str] = &[".git"];

/// The Files view's own keys, block `panel_files`, ahead of the panel keys.
pub(super) const KEYS: KeyTable<FilesAct> = KeyTable {
    block: "panel_files",
    bindings: &[Binding {
        action: FilesAct::Ignored,
        id: "ignored",
        keys: &["."],
        label: "Show or Hide Ignored Files",
    }],
    configurable: true,
    title: "Files View",
};

// ========================================================================
// Data Structures
// ========================================================================

/// What a Files view key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum FilesAct {
    Ignored,
}

/// The project's folders as a tree.
#[derive(Debug)]
pub(in crate::window) struct FilesView {
    /// Every entry read, in the order the rows show them, those git
    /// ignores among them.
    entries: Vec<Entry>,
    /// The folders open.
    folds: Folds,
    list: PanelList,
    /// Git's state of each path, from the last status read.
    marks: GitMarks,
    /// Whether a read of the status is out, so a change meanwhile waits for
    /// its answer.
    reading: bool,
    /// What git has said of the folder.
    repo: Repo,
    /// The project's folder, the tree's root.
    root: PathBuf,
    /// Whether what git ignores is listed.
    show_ignored: bool,
    /// Whether the view is in sight, the only time it reads or watches.
    shown: bool,
    /// The repository's notifications, in one: `None` until set up, or
    /// where the platform cannot watch it.
    tree_watch: Option<TreeWatch>,
    /// The repository's watch being set up on a thread of its own.
    tree_watch_starting: Option<Background<Option<TreeWatch>>>,
    /// Outside a repository, the notifications on the root and the open
    /// folders: `None` where the platform cannot watch them.
    watch: Option<DirWatch>,
}

/// What git has said of the folder Files shows.
#[derive(Debug)]
enum Repo {
    /// Nothing yet.
    Asking,
    /// The repository holding it.
    Found(FoundRepo),
    /// No repository holds it.
    Missing,
}

/// The repository holding the folder Files shows.
#[derive(Debug)]
struct FoundRepo {
    /// Where git keeps its own files.
    git_dir: PathBuf,
    /// Its root, which git reports paths against.
    top: PathBuf,
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
            list: PanelList::default(),
            marks: GitMarks::default(),
            reading: false,
            repo: Repo::Asking,
            root,
            show_ignored: false,
            shown: false,
            tree_watch: None,
            tree_watch_starting: None,
            watch: None,
        }
    }

    /// Show the project at `root` instead, its folders closed: what that
    /// asks of the window while in sight.
    pub(in crate::window) fn set_root(&mut self, root: PathBuf) -> EditorOutcome {
        if root == self.root {
            return EditorOutcome::Consumed;
        }
        let shown = self.shown;
        *self = Self::new(root);
        self.set_shown(shown)
    }

    /// The folders open.
    pub(in crate::window) fn open_folders(&self) -> Vec<PathBuf> {
        let mut open: Vec<PathBuf> = self.folds.expanded().iter().cloned().collect();
        open.sort();
        open
    }

    /// Open the folders `open` in place of those open, as a session kept
    /// them, reading them at once while in sight.
    pub(in crate::window) fn set_open_folders(&mut self, open: &[PathBuf]) {
        self.folds = Folds::new();
        for folder in open.iter().filter(|folder| folder.starts_with(&self.root)) {
            self.folds.expand(folder);
        }
        if let Some(watch) = self.watch.as_mut() {
            watch.sync_folds(&self.folds);
        }
        if self.shown {
            self.read();
        }
    }

    /// Read the tree and set its watch up, and ask git what it is next
    /// owed: where the repository is, then its status.
    fn come_into_sight(&mut self) -> EditorOutcome {
        self.read();
        match &self.repo {
            Repo::Asking => EditorOutcome::Job(JobRequest::Command(vcs::root_request(&self.root))),
            Repo::Found(_) => {
                self.start_tree_watch();
                self.ask_status()
            }
            Repo::Missing => {
                self.watch = DirWatch::start(&self.root, &self.folds);
                EditorOutcome::Consumed
            }
        }
    }

    /// Ask git for the repository's status, unless a read is out.
    fn ask_status(&mut self) -> EditorOutcome {
        let Repo::Found(found) = &self.repo else {
            return EditorOutcome::Consumed;
        };
        if self.reading {
            return EditorOutcome::Consumed;
        }
        self.reading = true;
        EditorOutcome::Job(JobRequest::Command(vcs::status_request(&found.top)))
    }

    /// Set the repository's watch up on a thread of its own, while in sight
    /// and none is running.
    fn start_tree_watch(&mut self) {
        let unwatched = self.tree_watch.is_none() && self.tree_watch_starting.is_none();
        if let (true, true, Repo::Found(found)) = (self.shown, unwatched, &self.repo) {
            self.tree_watch_starting = Some(TreeWatch::start_in_background(
                &found.top,
                Some(found.git_dir.as_path()),
            ));
        }
    }

    /// Take the repository's watch over once its thread has set it up.
    fn adopt_started_watch(&mut self) {
        let done = self
            .tree_watch_starting
            .take_if(|starting| starting.is_done());
        if let Some(starting) = done {
            self.tree_watch = starting.poll().flatten();
        }
    }

    /// Read the root and every open folder again, and list them.
    fn read(&mut self) {
        self.entries.clear();
        let root = self.root.clone();
        self.read_dir(&root, 0);
        self.list_entries();
    }

    /// List the entries read, leaving out what git ignores unless asked.
    fn list_entries(&mut self) {
        let rows = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.show_ignored || !self.is_ignored(entry))
            .map(|(key, entry)| self.entry_row(key, entry))
            .collect();
        self.list.set_rows(rows);
    }

    /// Whether git ignores `entry`.
    fn is_ignored(&self, entry: &Entry) -> bool {
        self.marks.get(&entry.path) == Some(GitStatus::Ignored)
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
        let fold = entry.is_dir.then(|| self.folds.is_expanded(&entry.path));
        let status = self.marks.get(&entry.path);
        ListRow {
            depth: entry.depth,
            fold,
            icon: Some(RowIcon::of_path(&name, fold)),
            key,
            label: name,
            mark: status.and_then(status_mark),
            muted: status == Some(GitStatus::Ignored),
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
            ListAct::Refresh => {
                self.read();
                return self.ask_status();
            }
            ListAct::OpenTool => {
                return EditorOutcome::OpenPath(OpenTarget::file(self.root.clone()));
            }
            ListAct::Down | ListAct::First | ListAct::Last | ListAct::Up => {}
        }
        EditorOutcome::Consumed
    }

    fn own_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        match table::find(&KEYS, key)? {
            FilesAct::Ignored => {
                self.show_ignored = !self.show_ignored;
                self.list_entries();
            }
        }
        Some(EditorOutcome::Consumed)
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        let JobReply::Command(output) = reply else {
            return EditorOutcome::Consumed;
        };
        match output.tag {
            TAG_ROOT => {
                let found = vcs::parse_root(&output.stdout).filter(|_| output.succeeded());
                self.repo = match found {
                    Some((top, git_dir)) => Repo::Found(FoundRepo { git_dir, top }),
                    None => Repo::Missing,
                };
                if self.shown {
                    return self.come_into_sight();
                }
            }
            TAG_STATUS => {
                self.reading = false;
                if let (true, Repo::Found(found)) = (output.succeeded(), &self.repo) {
                    self.marks = vcs::marks(&found.top, &output.stdout);
                    self.list_entries();
                }
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// Never out of sight; in sight, only when a watch has news, which
    /// wakes the window itself.
    fn next_tick(&self) -> Option<Instant> {
        let now = Instant::now();
        if !self.shown {
            return None;
        }
        if let Some(watch) = &self.watch {
            return watch.next_look(now);
        }
        match (&self.tree_watch, &self.tree_watch_starting) {
            (Some(watch), _) if !self.reading => watch.next_look(now),
            (Some(_), _) => None,
            // Taken over once its thread has answered, which wakes the window.
            (None, Some(starting)) => starting.is_done().then_some(now),
            (None, None) => None,
        }
    }

    fn set_shown(&mut self, shown: bool) -> EditorOutcome {
        self.shown = shown;
        match shown {
            true => self.come_into_sight(),
            false => {
                self.watch = None;
                self.tree_watch = None;
                self.tree_watch_starting = None;
                EditorOutcome::Consumed
            }
        }
    }

    fn on_tick(&mut self) -> EditorOutcome {
        let now = Instant::now();
        if self.watch.as_mut().is_some_and(|watch| watch.changed(now)) {
            self.read();
        }
        self.adopt_started_watch();
        let changed = match (self.tree_watch.as_mut(), self.reading) {
            (Some(watch), false) => watch.changed(now).is_some(),
            (Some(_), true) | (None, _) => false,
        };
        match changed {
            true => {
                self.read();
                self.ask_status()
            }
            false => EditorOutcome::Consumed,
        }
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

/// The mark of git's state `status` on an entry's row; none for an entry
/// git ignores, whose words are dimmed instead.
fn status_mark(status: GitStatus) -> Option<Mark> {
    let (letter, color) = match status {
        GitStatus::Added => ("A", ThemeToken::Accent),
        GitStatus::Conflict => ("U", ThemeToken::Danger),
        GitStatus::Deleted => ("D", ThemeToken::Danger),
        GitStatus::Ignored => return None,
        GitStatus::Modified => ("M", ThemeToken::Warning),
        GitStatus::Renamed => ("R", ThemeToken::Accent),
        GitStatus::Untracked => ("?", ThemeToken::Muted),
    };
    Some(Mark {
        color,
        icon: Some(IconKind::Git {
            status: status.icon_stem().to_string(),
        }),
        meaning: status.name().to_string(),
        text: letter.to_string(),
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::editor::CommandOutput;
    use winter_model::input::KeyCode;

    use super::*;

    /// What git answered `tag` with: `stdout`, and whether it succeeded.
    fn reply(tag: &'static str, ok: bool, stdout: &str) -> JobReply {
        JobReply::Command(CommandOutput {
            code: Some(if ok { 0 } else { 128 }),
            id: 0,
            stderr: String::new(),
            stdout: stdout.to_string(),
            tag,
        })
    }

    /// `view` told by git that its folder is in no repository.
    fn outside_git(view: &mut FilesView) {
        view.on_job(reply(TAG_ROOT, false, ""));
    }

    /// `view` told by git that its folder is the repository at `top`, whose
    /// status is `status`.
    fn in_git(view: &mut FilesView, top: &Path, status: &str) {
        let root = format!("{}\n{}\n", top.display(), top.join(".git").display());
        view.on_job(reply(TAG_ROOT, true, &root));
        view.tree_watch_starting = None;
        view.on_job(reply(TAG_STATUS, true, status));
    }

    fn press(view: &mut FilesView, code: KeyCode) {
        view.own_key(&Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        });
    }

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
        outside_git(&mut view);
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

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let root = project("named");
        let mut view = FilesView::new(root.0.clone());
        view.set_shown(true);
        view.act(ListAct::Expand);
        let (unnamed, unlit) = super::super::tests::control_faults_in(view);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }

    #[test]
    fn test_a_folder_and_a_file_side_by_side_put_their_icons_in_one_column() {
        let root = project("icons");
        let mut view = FilesView::new(root.0.clone());
        view.set_shown(true);
        let frame = super::super::tests::drawn_view(view);
        let folder = super::super::tests::lefts_of(&frame, "Folder");
        let file = super::super::tests::lefts_of(&frame, "File");
        assert_eq!(folder.len(), 1, "src: {folder:?}");
        assert_eq!(file.len(), 1, "README.md: {file:?}");
        assert_eq!(folder[0], file[0], "the leaf keeps the fold mark's place");
    }

    #[test]
    fn test_what_git_ignores_is_left_out_until_dot_shows_it_dimmed() {
        let root = project("ignored");
        fs::create_dir_all(root.0.join("target")).expect("target");
        let mut view = FilesView::new(root.0.clone());
        let asked = view.set_shown(true);
        assert!(
            matches!(asked, EditorOutcome::Job(_)),
            "where the repository is"
        );
        in_git(&mut view, &root.0, "! target/\0");
        let names = |view: &FilesView| -> Vec<String> {
            view.list.rows.iter().map(|row| row.label.clone()).collect()
        };
        assert_eq!(names(&view), ["src", "README.md"]);
        press(&mut view, KeyCode::Char('.'));
        assert_eq!(names(&view), ["src", "target", "README.md"]);
        let target = &view.list.rows[1];
        assert!(target.muted && target.mark.is_none(), "dimmed, not marked");
        press(&mut view, KeyCode::Char('.'));
        assert_eq!(names(&view), ["src", "README.md"], "hidden again");
    }

    #[test]
    fn test_a_changed_file_and_the_folders_holding_it_are_marked() {
        let root = project("marks");
        let mut view = FilesView::new(root.0.clone());
        view.set_shown(true);
        in_git(
            &mut view,
            &root.0,
            "1 .M N... 100644 100644 100644 a b src/main.rs\0",
        );
        view.act(ListAct::Expand);
        let marks: Vec<(String, Option<String>)> = view
            .list
            .rows
            .iter()
            .map(|row| {
                (
                    row.label.clone(),
                    row.mark.as_ref().map(|mark| mark.text.clone()),
                )
            })
            .collect();
        assert_eq!(
            marks,
            [
                ("src".to_string(), Some("M".to_string())),
                ("main.rs".to_string(), Some("M".to_string())),
                ("README.md".to_string(), None),
            ]
        );
    }

    #[test]
    fn test_git_is_asked_nothing_out_of_sight() {
        let root = project("quiet");
        let mut view = FilesView::new(root.0.clone());
        assert!(matches!(view.set_shown(false), EditorOutcome::Consumed));
        let top = format!("{}\n{}\n", root.0.display(), root.0.join(".git").display());
        let asked = view.on_job(reply(TAG_ROOT, true, &top));
        assert!(matches!(asked, EditorOutcome::Consumed), "no status asked");
        assert!(view.tree_watch_starting.is_none(), "no watch out of sight");
        assert_eq!(view.next_tick(), None);
        assert!(matches!(view.on_tick(), EditorOutcome::Consumed));
    }
}

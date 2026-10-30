//! The window's workspace: which one it is in, and going from one to another.
//!
//! A switch keeps the workspace left open, its layout put aside with every
//! tab in it still running, so going back to it finds its programs where
//! they were. The workspace switched to comes back the same way when it was
//! put aside, else as it was last saved, else as one terminal at its first
//! root. A layout in no workspace has nowhere to be kept: it is closed,
//! asking first as quitting does when that would lose something.
//!
//! Every workspace open in the window, put aside or not, is saved when the
//! window quits or reloads.
//!
//! The palette's `Workspace:` commands are here too: marking the focused
//! tab's project as a workspace, adding a folder to the one the window is in,
//! renaming it, and forgetting one.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

use crate::control::{WorkspaceAnswer, WorkspaceCommand};
use crate::model::layout::{Layout, TabId};
use crate::model::picker::{Picker, PickerEntry, PickerMode};
use crate::model::path::abbreviate_home;
use crate::project;
use crate::session::Session;
use crate::workspace::{self, Store, Workspace};

use crate::window::editor::Closing;
use crate::window::{content_rows, App, DEFAULT_COLS, DEFAULT_ROWS};

// ========================================================================
// Constants
// ========================================================================

/// Said by a command that works on the window's workspace, when it has none.
const NO_WORKSPACE: &str = "not in a workspace: mark a folder as one first";

/// Said when there is nowhere to keep workspaces.
const NO_STORE: &str = "no state directory to keep workspaces in";

/// Said when there are no workspaces to list.
const NONE_SAVED: &str = "no workspaces yet: mark a folder as one first";

// ========================================================================
// Data Structures
// ========================================================================

/// How a workspace command went: what it did, or why it did not.
pub(in crate::window) type Outcome = Result<String, String>;

/// Which workspace the window is in, where the workspaces are kept, and the
/// one a switch waiting on an answer is for.
pub(in crate::window) struct WorkspaceState {
    /// The workspace the window is in, by name; `None` for a window in none.
    pub(in crate::window) active: Option<String>,
    /// Where the workspaces are kept; `None` when there is no state directory.
    pub(in crate::window) store: Option<Store>,
    /// The workspace to go to once the question a switch asked is answered.
    pub(in crate::window) switch_to: Option<String>,
    /// What the name being typed into the palette is for.
    pub(in crate::window) naming: Option<Naming>,
    /// The layouts of the workspaces switched away from, by name, their
    /// tabs still open and running.
    pub(in crate::window) parked: HashMap<String, Layout>,
    /// The settings the window's workspace sets over `settings.kdl`, in
    /// force while it is the window's.
    pub(in crate::window) settings: BTreeMap<String, String>,
}

/// What a name typed into [`PickerMode::WorkspaceName`] is for.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(in crate::window) enum Naming {
    /// A new workspace of this one folder.
    Mark(PathBuf),
    /// The workspace now called this.
    Rename(String),
}

// ========================================================================
// WorkspaceState Implementations
// ========================================================================

impl Default for WorkspaceState {
    fn default() -> Self {
        Self {
            active: None,
            store: Store::open(),
            switch_to: None,
            naming: None,
            parked: HashMap::new(),
            settings: BTreeMap::new(),
        }
    }
}

// ========================================================================
// App: switching
// ========================================================================

impl App {
    /// Go to the workspace `name`. Leaving a layout in no workspace closes
    /// it, which is asked about first when that would lose something.
    pub(in crate::window) fn switch_workspace(&mut self, name: &str) -> Outcome {
        if self.workspace.active.as_deref() == Some(name) {
            return Ok(format!("already in workspace {name}"));
        }
        self.workspace.switch_to = Some(name.to_string());
        if self.workspace.active.is_none() && self.ask_before_switching() {
            return Ok(format!(
                "asked in the window before going to workspace {name}"
            ));
        }
        self.workspace.switch_to = None;
        self.switch_workspace_now(name)
    }

    /// The same, once the question has been answered or there was none.
    pub(in crate::window) fn switch_workspace_now(&mut self, name: &str) -> Outcome {
        let target = self
            .workspace
            .store
            .as_ref()
            .and_then(|s| s.load(name))
            .ok_or_else(|| format!("no workspace named {name}"))?;
        match self.workspace.active.take() {
            Some(left) => self.park_layout(left),
            None => self.close_everything(),
        }
        self.enter_workspace(Some(target.name.clone()));
        if let Some(parked) = self.workspace.parked.remove(name) {
            self.tabs.layout = parked;
        } else {
            let restored = target.session.clone().is_some_and(|mut session| {
                // Saved ids may be held by the tabs of a workspace put aside.
                session.renumber(|| self.alloc_tab_id().0 as usize);
                self.restore_session(session)
            });
            if !restored {
                self.open_fresh_workspace(&target);
            }
        }
        self.after_tab_change();
        self.resize_all_panes();
        self.dirty = true;
        Ok(format!("workspace {name}"))
    }

    /// Save every workspace open in the window, the one in front and those
    /// put aside, so each opens again as it is now.
    pub(in crate::window) fn save_open_workspaces(&mut self) {
        let mut open: Vec<(String, &Layout)> = self
            .workspace
            .parked
            .iter()
            .map(|(name, layout)| (name.clone(), layout))
            .collect();
        if let Some(active) = &self.workspace.active {
            open.push((active.clone(), &self.tabs.layout));
        }
        let sessions: Vec<(String, Session)> = open
            .into_iter()
            .map(|(name, layout)| {
                let session = self.capture_session_of(layout, Some(name.clone()));
                (name, session)
            })
            .collect();
        let Some(store) = &self.workspace.store else {
            return;
        };
        let mut failed = Vec::new();
        for (name, session) in sessions {
            // A workspace removed meanwhile (from another window) stays removed.
            let Some(mut workspace) = store.load(&name) else {
                continue;
            };
            workspace.session = Some(session);
            if let Err(e) = store.save(&workspace) {
                failed.push(format!("could not save workspace {name}: {e}"));
            }
        }
        for problem in failed {
            self.set_error(problem);
        }
    }

    /// Every tab open in the window: the layout's in front, then those of
    /// the workspaces put aside.
    pub(in crate::window) fn every_open_tab(&self) -> Vec<TabId> {
        let mut tabs = self.layout().members();
        for layout in self.workspace.parked.values() {
            tabs.extend(layout.members());
        }
        tabs
    }

    /// The tabs of the workspaces put aside, open but not in the window's
    /// layout.
    pub(in crate::window) fn parked_tabs(&self) -> Vec<TabId> {
        self.workspace
            .parked
            .values()
            .flat_map(Layout::members)
            .collect()
    }

    /// Close a tab of a workspace put aside, as a shell exiting in it does:
    /// with nothing left in it, the workspace is no longer open, and opens
    /// as it was saved when next switched to.
    pub(in crate::window) fn close_parked_tab(&mut self, tab: TabId) {
        let Some(name) = self
            .workspace
            .parked
            .iter()
            .find(|(_, layout)| layout.contains(tab))
            .map(|(name, _)| name.clone())
        else {
            return;
        };
        self.drop_tab_state(tab);
        let closed = self
            .workspace
            .parked
            .get_mut(&name)
            .is_some_and(|layout| layout.close(tab));
        if !closed {
            self.workspace.parked.remove(&name);
        }
    }

    /// Put the window's layout aside as the workspace `name`'s, its tabs left
    /// running, and leave the window an empty layout of a tab never opened.
    fn park_layout(&mut self, name: String) {
        let placeholder = Layout::with_root(self.alloc_tab_id());
        let layout = std::mem::replace(&mut self.tabs.layout, placeholder);
        self.workspace.parked.insert(name, layout);
    }

    /// Close every tab in the window without asking, as closing each one
    /// would: an editor's buffer is buried to come back to, everything else
    /// goes, and the window is left a layout of a tab never opened. Killed
    /// outright, clean editors were lost where every other close keeps them.
    fn close_everything(&mut self) {
        for tab in self.layout().members() {
            match self.buries(tab, Closing::Keep) {
                true => self.bury_tab(tab),
                false => self.drop_tab_state(tab),
            }
            self.host.commit_companions.remove(&tab);
        }
        self.tabs.layout = Layout::with_root(self.alloc_tab_id());
        self.watch_for_extensions();
    }

    /// Open `workspace` as one terminal at its first root, as a workspace
    /// never left opens.
    fn open_fresh_workspace(&mut self, workspace: &Workspace) {
        let (cols, rows) = match &self.renderer {
            Some(renderer) => renderer.grid_size(),
            None => (DEFAULT_COLS as usize, DEFAULT_ROWS as usize),
        };
        let shell = self.config.active_shell().map(String::from);
        let scrollback = self
            .config
            .scrollback_lines
            .unwrap_or(winter_core::MAX_SCROLLBACK);
        let cwd = workspace
            .roots
            .first()
            .and_then(|root| root.to_str())
            .map(str::to_string);
        let id = self.alloc_tab_id();
        self.tabs.layout = Layout::with_root(id);
        if let Some(terminal) = self.spawn_terminal_or_notify(
            cols.max(1),
            content_rows(rows).max(1),
            shell.as_deref(),
            scrollback,
            cwd.as_deref(),
        ) {
            self.open_tabs.insert_terminal(id, terminal);
        }
    }
}

// ========================================================================
// App: commands
// ========================================================================

impl App {
    /// Make the focused tab's project a workspace, asking what to call it.
    /// A project already in one says which, and the window goes on in it.
    pub(in crate::window) fn mark_workspace(&mut self) {
        let root = project::root(&self.focused_start_dir());
        let saved = match &self.workspace.store {
            Some(store) => store.list(),
            None => return self.set_error(NO_STORE),
        };
        if workspace::containing(&root, &saved).is_some() {
            let done = self.mark_workspace_at(&root, None);
            return self.report(done);
        }
        let suggested = workspace::suggest_name(&root, &saved);
        let prompt = format!(
            "Enter makes {} a workspace by this name",
            abbreviate_home(&root)
        );
        self.workspace.naming = Some(Naming::Mark(root));
        self.open_picker(Picker::open_workspace_name(
            &suggested,
            prompt,
            "Workspace: name it".to_string(),
        ));
        self.dirty = true;
    }

    /// Make the project `dir` is in a workspace called `name`, or by its
    /// folder's name, and put the window in it. A project already in one
    /// puts the window in that one, when it is in none.
    pub(in crate::window) fn mark_workspace_at(
        &mut self,
        dir: &Path,
        name: Option<&str>,
    ) -> Outcome {
        let root = project::root(dir);
        let store = self.workspace.store.as_ref().ok_or(NO_STORE)?;
        let saved = store.list();
        if let Some(owner) = workspace::containing(&root, &saved) {
            let owner = owner.name.clone();
            let said = format!("{} is in workspace {owner}", abbreviate_home(&root));
            if self.workspace.active.is_none() {
                self.enter_workspace(Some(owner));
            }
            return Ok(said);
        }
        let name = match name {
            Some(name) => name.trim().to_string(),
            None => workspace::suggest_name(&root, &saved),
        };
        self.create_workspace(root, &name)
    }

    /// Add the focused tab's project to the window's workspace, as another
    /// of its roots.
    pub(in crate::window) fn add_workspace_folder(&mut self) {
        let dir = self.focused_start_dir();
        let done = self.add_workspace_folder_at(&dir);
        self.report(done);
    }

    /// Add the project `dir` is in to the window's workspace.
    pub(in crate::window) fn add_workspace_folder_at(&mut self, dir: &Path) -> Outcome {
        let mut current = self.active_workspace()?;
        let root = project::root(dir);
        let shown = abbreviate_home(&root);
        if current.root_for(&root).is_some() {
            return Ok(format!("{shown} is already in workspace {}", current.name));
        }
        current.roots.push(root);
        let store = self.workspace.store.as_ref().ok_or(NO_STORE)?;
        store
            .save(&current)
            .map(|()| format!("added {shown} to workspace {}", current.name))
            .map_err(|e| format!("could not save workspace {}: {e}", current.name))
    }

    /// List the workspaces to switch to.
    pub(in crate::window) fn open_workspace_switcher(&mut self) {
        self.open_workspace_list(PickerMode::Workspaces, "Workspaces: Enter switches to one");
    }

    /// List the workspaces to forget one.
    pub(in crate::window) fn open_workspace_remover(&mut self) {
        self.open_workspace_list(
            PickerMode::WorkspaceRemove,
            "Workspaces: Enter forgets one, its folders untouched",
        );
    }

    /// Ask what to call the window's workspace instead.
    pub(in crate::window) fn rename_workspace(&mut self) {
        let current = match self.active_workspace() {
            Ok(current) => current,
            Err(problem) => return self.set_error(problem),
        };
        let prompt = format!("Enter renames workspace {}", current.name);
        self.open_picker(Picker::open_workspace_name(
            &current.name,
            prompt,
            "Workspace: rename it".to_string(),
        ));
        self.workspace.naming = Some(Naming::Rename(current.name));
        self.dirty = true;
    }

    /// Call the window's workspace `to` instead.
    pub(in crate::window) fn rename_workspace_to(&mut self, to: &str) -> Outcome {
        let from = self.active_workspace()?.name;
        let to = to.trim();
        if from == to {
            return Ok(format!("workspace {to}"));
        }
        let store = self.workspace.store.as_ref().ok_or(NO_STORE)?;
        store.rename(&from, to)?;
        self.workspace.active = Some(to.to_string());
        Ok(format!("workspace {from} is now {to}"))
    }

    /// Forget the workspace `name`. The window goes on as it is, in no
    /// workspace when it was that one.
    pub(in crate::window) fn remove_workspace(&mut self, name: &str) -> Outcome {
        let store = self.workspace.store.as_ref().ok_or(NO_STORE)?;
        store
            .remove(name)
            .map_err(|e| format!("could not remove workspace {name}: {e}"))?;
        if self.workspace.active.as_deref() == Some(name) {
            self.enter_workspace(None);
        }
        // Put aside, its tabs would run on with no way back to them.
        if let Some(parked) = self.workspace.parked.remove(name) {
            for tab in parked.members() {
                self.drop_tab_state(tab);
            }
        }
        Ok(format!(
            "forgot workspace {name}; its folders are untouched"
        ))
    }

    /// Take the name typed into [`PickerMode::WorkspaceName`], for the
    /// workspace being marked or renamed.
    pub(in crate::window) fn name_workspace(&mut self, typed: &str) {
        let done = match self.workspace.naming.take() {
            Some(Naming::Mark(root)) => self.create_workspace(root, typed.trim()),
            Some(Naming::Rename(_)) => self.rename_workspace_to(typed),
            None => return,
        };
        self.report(done);
    }

    /// Every workspace, one a line beside its folders, the window's own
    /// marked with a `*`.
    pub(in crate::window) fn list_workspaces(&self) -> Outcome {
        let store = self.workspace.store.as_ref().ok_or(NO_STORE)?;
        let lines: Vec<String> = store
            .list()
            .into_iter()
            .map(|workspace| {
                let mark = match self.workspace.active.as_deref() == Some(&workspace.name) {
                    true => '*',
                    false => ' ',
                };
                let roots: Vec<String> =
                    workspace.roots.iter().map(|r| abbreviate_home(r)).collect();
                format!("{mark} {}\t{}", workspace.name, roots.join(", "))
            })
            .collect();
        match lines.is_empty() {
            true => Ok(NONE_SAVED.to_string()),
            false => Ok(lines.join("\n")),
        }
    }

    /// Carry out what `winter workspace` asked, and say how it went: in the
    /// window as well, except for a list, which is only for the asker.
    pub(in crate::window) fn answer_workspace_command(
        &mut self,
        command: WorkspaceCommand,
    ) -> WorkspaceAnswer {
        let listing = command == WorkspaceCommand::List;
        let done = match command {
            WorkspaceCommand::List => self.list_workspaces(),
            WorkspaceCommand::Add { dir, name } => self.mark_workspace_at(&dir, name.as_deref()),
            WorkspaceCommand::AddFolder { dir } => self.add_workspace_folder_at(&dir),
            WorkspaceCommand::Remove { name } => self.remove_workspace(&name),
            WorkspaceCommand::Rename { name } => self.rename_workspace_to(&name),
            WorkspaceCommand::Switch { name } => self.switch_workspace(&name),
        };
        let answer = WorkspaceAnswer {
            ok: done.is_ok(),
            text: match &done {
                Ok(said) | Err(said) => said.clone(),
            },
        };
        if !listing {
            self.report(done);
            self.dirty = true;
        }
        answer
    }

    /// Show how a command went: what it did as a notice, why it did not as
    /// an error.
    pub(in crate::window) fn report(&mut self, done: Outcome) {
        match done {
            Ok(said) => self.set_notice(said),
            Err(problem) => self.set_error(problem),
        }
    }

    /// Save a new workspace called `name` of the one folder `root`, and put
    /// the window in it.
    fn create_workspace(&mut self, root: PathBuf, name: &str) -> Outcome {
        if let Some(problem) = workspace::invalid_name(name) {
            return Err(problem.to_string());
        }
        let store = self.workspace.store.as_ref().ok_or(NO_STORE)?;
        if store.load(name).is_some() {
            return Err(format!("a workspace named {name} already exists"));
        }
        store
            .save(&Workspace::new(name.to_string(), root))
            .map_err(|e| format!("could not save workspace {name}: {e}"))?;
        self.enter_workspace(Some(name.to_string()));
        Ok(format!("workspace {name}"))
    }

    /// The workspace the window is in, as it is saved.
    fn active_workspace(&self) -> Result<Workspace, String> {
        let name = self.workspace.active.as_deref().ok_or(NO_WORKSPACE)?;
        self.workspace
            .store
            .as_ref()
            .and_then(|s| s.load(name))
            .ok_or_else(|| format!("workspace {name} could not be read"))
    }

    /// The saved workspaces as a palette in `mode`, the window's own marked,
    /// each beside its folders.
    fn open_workspace_list(&mut self, mode: PickerMode, title: &str) {
        let saved = self
            .workspace
            .store
            .as_ref()
            .map(Store::list)
            .unwrap_or_default();
        if saved.is_empty() {
            self.set_notice(NONE_SAVED);
            return;
        }
        let entries = saved
            .into_iter()
            .map(|workspace| {
                let label = match self.workspace.active.as_deref() == Some(&workspace.name) {
                    true => format!("{} (current)", workspace.name),
                    false => workspace.name.clone(),
                };
                let roots: Vec<String> =
                    workspace.roots.iter().map(|r| abbreviate_home(r)).collect();
                PickerEntry::new(workspace.name, label, roots.join(", "))
            })
            .collect();
        self.open_picker(Picker::open_workspaces(mode, entries, title.to_string()));
        self.dirty = true;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use crate::model::layout::Direction;
    use crate::model::settings_page::Scope;
    use crate::window::editor::PendingClose;

    use super::*;

    /// An app whose workspaces are kept in a directory of their own, gone
    /// once the test is.
    pub(in crate::window) struct Scratch {
        pub(in crate::window) app: App,
        dir: PathBuf,
    }

    impl Scratch {
        pub(in crate::window) fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "winter-app-workspaces-{tag}-{}",
                std::process::id()
            ));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("creates the scratch dir");
            let mut app = App::new();
            app.workspace.store = Some(Store::at(dir.join("store")));
            Self { app, dir }
        }

        /// A folder of the scratch dir, made.
        pub(in crate::window) fn folder(&self, name: &str) -> PathBuf {
            let path = self.dir.join(name);
            fs::create_dir_all(&path).expect("creates the folder");
            path
        }

        pub(in crate::window) fn store(&self) -> &Store {
            self.app.workspace.store.as_ref().expect("a store")
        }

        /// Put a terminal in the focused tab, running a program in the way of
        /// closing it when `busy`.
        pub(in crate::window) fn shell(&mut self, busy: bool) {
            let id = self.app.layout().focused();
            let mut terminal = crate::terminal::session::Terminal::with_command(
                20,
                4,
                portable_pty::CommandBuilder::new("cat"),
                winter_core::MAX_SCROLLBACK,
            )
            .expect("spawns cat");
            if busy {
                terminal.feed_program_output(b"\x1b[?1049h");
            }
            self.app.open_tabs.insert_terminal(id, terminal);
        }

        /// Save a workspace of the one folder `root`, never opened.
        pub(in crate::window) fn add(&self, name: &str, root: PathBuf) {
            self.store()
                .save(&Workspace::new(name.to_string(), root))
                .expect("saves the workspace");
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.dir);
        }
    }

    #[test]
    fn test_leaving_a_window_in_no_workspace_buries_its_editors_as_closing_would() {
        // Every tab was killed outright: a clean editor was lost where any
        // other close keeps it to come back to.
        let mut scratch = Scratch::new("leave-buries");
        let file = scratch.folder("files").join("notes.txt");
        fs::write(&file, "notes\n").expect("file");
        let editor = crate::editors::text::TextEditor::new(file, None).expect("opens");
        let tab = scratch.app.show_editor(crate::window::editor::EditorKind::Text, Box::new(editor));
        scratch.app.close_everything();
        assert_eq!(scratch.app.buried_tabs(), [tab]);
        assert!(!scratch.app.layout().contains(tab));
        assert_eq!(scratch.app.layout().members().len(), 1, "a layout of a tab never opened");
        assert_eq!(scratch.app.stale_tab_ids(), [], "nothing kept for the tabs that went");
    }

    #[test]
    fn test_marking_offers_the_project_s_name_and_makes_it_the_window_s() {
        let mut scratch = Scratch::new("mark");
        let root = project::root(&scratch.app.focused_start_dir());
        let before = scratch.app.layout().members();

        scratch.app.mark_workspace();
        let palette = scratch.app.chrome.picker().expect("asks for a name");
        assert_eq!(palette.mode, PickerMode::WorkspaceName);
        let suggested = palette.query.clone();
        assert_eq!(suggested, workspace::suggest_name(&root, &[]));

        scratch.app.name_workspace(&suggested);
        assert_eq!(
            scratch.app.workspace.active.as_deref(),
            Some(suggested.as_str())
        );
        let saved = scratch.store().load(&suggested).expect("saved");
        assert_eq!(saved.roots, [root]);
        assert_eq!(
            scratch.app.layout().members(),
            before,
            "the window goes on as it is"
        );
    }

    #[test]
    fn test_marking_a_project_already_in_a_workspace_names_that_one() {
        let mut scratch = Scratch::new("mark-again");
        let root = project::root(&scratch.app.focused_start_dir());
        scratch.add("mine", root);

        scratch.app.mark_workspace();
        assert!(scratch.app.chrome.picker().is_none(), "nothing to name");
        assert_eq!(scratch.app.workspace.active.as_deref(), Some("mine"));
        assert_eq!(scratch.store().list().len(), 1);
    }

    #[test]
    fn test_a_name_taken_or_no_file_name_is_refused() {
        let mut scratch = Scratch::new("refuse");
        scratch.add("taken", scratch.folder("taken"));
        for typed in ["taken", "a/b", "  "] {
            scratch.app.workspace.naming = Some(Naming::Mark(scratch.folder("new")));
            scratch.app.name_workspace(typed);
            assert!(scratch.app.workspace.active.is_none(), "{typed:?}");
        }
        assert_eq!(scratch.store().list().len(), 1);
    }

    #[test]
    fn test_renaming_the_window_s_workspace_follows_it() {
        let mut scratch = Scratch::new("rename");
        scratch.add("old", scratch.folder("old"));
        scratch.app.workspace.active = Some("old".to_string());

        scratch.app.rename_workspace();
        scratch.app.name_workspace("new");
        assert_eq!(scratch.app.workspace.active.as_deref(), Some("new"));
        assert!(scratch.store().load("old").is_none());
        assert!(scratch.store().load("new").is_some());
    }

    #[test]
    fn test_adding_a_folder_adds_a_root_once() {
        let mut scratch = Scratch::new("add");
        scratch.add("w", scratch.folder("w"));
        scratch.app.workspace.active = Some("w".to_string());
        let here = project::root(&scratch.app.focused_start_dir());

        scratch.app.add_workspace_folder();
        scratch.app.add_workspace_folder();
        let roots = scratch.store().load("w").expect("w").roots;
        assert_eq!(roots, [scratch.folder("w"), here], "added, and only once");
    }

    #[test]
    fn test_the_switch_list_marks_the_window_s_workspace() {
        let mut scratch = Scratch::new("list");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("b".to_string());

        scratch.app.open_workspace_switcher();
        let palette = scratch.app.chrome.picker().expect("the list");
        let labels: Vec<&str> = palette.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["a", "b (current)"]);
        assert_eq!(palette.entries[1].action, "b", "picked by its plain name");
    }

    #[test]
    fn test_forgetting_the_window_s_workspace_leaves_the_window_as_it_is() {
        let mut scratch = Scratch::new("remove");
        let folder = scratch.folder("gone");
        scratch.add("gone", folder.clone());
        scratch.app.workspace.active = Some("gone".to_string());
        let before = scratch.app.layout().members();

        scratch.app.remove_workspace("gone").expect("goes through");
        assert!(scratch.app.workspace.active.is_none());
        assert!(scratch.store().list().is_empty());
        assert!(folder.exists(), "its folder is untouched");
        assert_eq!(scratch.app.layout().members(), before);
    }

    #[test]
    fn test_a_shell_s_add_names_the_workspace_and_its_list_marks_it() {
        let mut scratch = Scratch::new("shell");
        let dir = scratch.folder("proj");
        let added = scratch.app.answer_workspace_command(WorkspaceCommand::Add {
            dir: dir.clone(),
            name: Some("proj".to_string()),
        });
        assert!(added.ok, "{}", added.text);
        assert_eq!(scratch.app.workspace.active.as_deref(), Some("proj"));

        let listed = scratch.app.answer_workspace_command(WorkspaceCommand::List);
        assert!(listed.ok);
        assert!(listed.text.starts_with("* proj\t"), "{}", listed.text);

        let again = scratch.app.answer_workspace_command(WorkspaceCommand::Add {
            dir: dir.join("sub"),
            name: Some("other".to_string()),
        });
        assert!(again.ok, "a folder already in one says which");
        assert!(again.text.contains("in workspace proj"), "{}", again.text);
        assert_eq!(scratch.store().list().len(), 1);
    }

    #[test]
    fn test_a_shell_s_switch_to_no_such_workspace_fails() {
        let mut scratch = Scratch::new("shell-missing");
        let answer = scratch
            .app
            .answer_workspace_command(WorkspaceCommand::Switch {
                name: "nowhere".to_string(),
            });
        assert!(!answer.ok);
    }

    /// What `settings.kdl` says of whether the status bar shows the mode.
    fn user_show_mode() -> bool {
        crate::config::Config::load_checked().0.status_bar.show_mode
    }

    #[test]
    fn test_a_workspace_setting_is_in_force_only_while_it_is_the_window_s() {
        let mut scratch = Scratch::new("settings");
        let user = user_show_mode();
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.enter_workspace(Some("a".to_string()));

        let flipped = (!user).to_string();
        scratch
            .app
            .apply_scoped_edit(Scope::Workspace, "status.show_mode", &flipped);
        assert_eq!(scratch.app.config.status_bar.show_mode, !user, "in force");
        let saved = scratch.store().load("a").expect("a").settings;
        assert_eq!(saved.get("status.show_mode"), Some(&flipped), "kept in a");

        scratch.app.switch_workspace("b").expect("goes through");
        assert_eq!(
            scratch.app.config.status_bar.show_mode, user,
            "b has its own"
        );
        scratch.app.switch_workspace("a").expect("goes through");
        assert_eq!(scratch.app.config.status_bar.show_mode, !user, "a's again");
    }

    #[test]
    fn test_dropping_a_workspace_setting_puts_the_user_s_back() {
        let mut scratch = Scratch::new("settings-reset");
        let user = user_show_mode();
        scratch.add("a", scratch.folder("a"));
        scratch.app.enter_workspace(Some("a".to_string()));
        scratch
            .app
            .apply_scoped_edit(Scope::Workspace, "status.show_mode", &(!user).to_string());

        scratch.app.reset_workspace_setting("status.show_mode");
        assert_eq!(scratch.app.config.status_bar.show_mode, user);
        assert!(scratch.store().load("a").expect("a").settings.is_empty());
    }

    #[test]
    fn test_the_settings_page_offers_the_workspace_scope_and_marks_its_rows() {
        let mut scratch = Scratch::new("settings-page");
        scratch.add("a", scratch.folder("a"));
        assert!(
            scratch.app.build_settings_page().workspace.is_none(),
            "none yet"
        );

        scratch.app.enter_workspace(Some("a".to_string()));
        let shown = (!user_show_mode()).to_string();
        scratch
            .app
            .apply_scoped_edit(Scope::Workspace, "status.show_mode", &shown);
        let page = scratch.app.build_settings_page();
        assert_eq!(page.workspace.as_deref(), Some("a"));
        assert!(page.overridden.contains("status.show_mode"));
    }

    #[test]
    fn test_the_workspace_left_keeps_running_and_comes_back_as_it_was() {
        let mut scratch = Scratch::new("switch");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("a".to_string());
        scratch.shell(true);
        scratch.app.split_pane_with_terminal(Direction::Vertical);
        let a_tabs = scratch.app.layout().members();
        assert_eq!(a_tabs.len(), 2);

        // A program running in it is no reason to ask: nothing is closed.
        scratch.app.switch_workspace("b").expect("goes through");
        assert!(scratch.app.chrome.close_prompt().is_none());
        assert_eq!(scratch.app.workspace.active.as_deref(), Some("b"));
        assert_eq!(scratch.app.layout().members().len(), 1, "b's one shell");
        for tab in &a_tabs {
            assert!(
                scratch.app.open_tabs.terminal(tab).is_some(),
                "a's still runs"
            );
            assert!(!scratch.app.layout().contains(*tab), "out of the window");
        }

        scratch.app.switch_workspace("a").expect("goes through");
        assert_eq!(scratch.app.layout().members(), a_tabs, "the same tabs");
        assert!(
            scratch.app.workspace.parked.contains_key("b"),
            "b put aside"
        );
    }

    #[test]
    fn test_saving_writes_every_open_workspace_with_its_own_tabs() {
        let mut scratch = Scratch::new("save");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("a".to_string());
        scratch.shell(false);
        scratch.app.split_pane_with_terminal(Direction::Vertical);
        scratch.app.switch_workspace("b").expect("goes through");

        scratch.app.save_open_workspaces();
        let saved = |name: &str| {
            let workspace = scratch.store().load(name).expect("saved");
            workspace.session.expect("with a session")
        };
        assert_eq!(saved("a").terminals.len(), 2, "a put aside, with its two");
        assert_eq!(saved("b").terminals.len(), 1, "b in front, with its one");
        assert_eq!(saved("b").workspace.as_deref(), Some("b"));
    }

    #[test]
    fn test_a_saved_workspace_opens_with_ids_no_open_tab_holds() {
        let mut scratch = Scratch::new("renumber");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("a".to_string());
        scratch.shell(false);
        // b was saved holding the very id a's shell has now.
        let held = scratch.app.layout().focused();
        let mut b = scratch.store().load("b").expect("b");
        b.session = Some(scratch.app.capture_session());
        scratch.store().save(&b).expect("saves b");

        scratch.app.switch_workspace("b").expect("goes through");
        assert!(
            !scratch.app.layout().contains(held),
            "b's tab took a new id"
        );
        assert!(
            scratch.app.open_tabs.terminal(&held).is_some(),
            "a's shell is untouched"
        );
    }

    #[test]
    fn test_quitting_asks_about_a_program_in_a_workspace_put_aside() {
        let mut scratch = Scratch::new("quit");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("a".to_string());
        scratch.shell(true);
        scratch.app.switch_workspace("b").expect("goes through");
        assert!(scratch.app.ask_before_quitting(), "a's program would end");
    }

    #[test]
    fn test_a_last_shell_ending_in_a_workspace_put_aside_closes_that_workspace() {
        let mut scratch = Scratch::new("reap");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("a".to_string());
        scratch.shell(false);
        let a_shell = scratch.app.layout().focused();
        scratch.app.switch_workspace("b").expect("goes through");
        let b_tabs = scratch.app.layout().members();

        scratch.app.close_parked_tab(a_shell);
        assert!(scratch.app.open_tabs.terminal(&a_shell).is_none());
        assert!(
            !scratch.app.workspace.parked.contains_key("a"),
            "no longer open"
        );
        assert_eq!(
            scratch.app.layout().members(),
            b_tabs,
            "the window untouched"
        );
        assert!(!scratch.app.runtime.exit.requested);
    }

    #[test]
    fn test_forgetting_a_workspace_put_aside_closes_its_tabs() {
        let mut scratch = Scratch::new("forget-parked");
        scratch.add("a", scratch.folder("a"));
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.active = Some("a".to_string());
        scratch.shell(false);
        let a_shell = scratch.app.layout().focused();
        scratch.app.switch_workspace("b").expect("goes through");

        scratch.app.remove_workspace("a").expect("goes through");
        assert!(scratch.app.open_tabs.terminal(&a_shell).is_none());
        assert!(scratch.app.workspace.parked.is_empty());
    }

    #[test]
    fn test_a_workspace_never_opened_starts_one_terminal_at_its_root() {
        let mut scratch = Scratch::new("fresh");
        let root = scratch.folder("fresh");
        scratch.add("fresh", root.clone());
        // A plain shell, so no startup file of the user's moves it elsewhere.
        scratch.app.config.shell = Some("/bin/sh".to_string());
        scratch.app.config.shell_linux = None;

        scratch.app.switch_workspace("fresh").expect("goes through");
        let focused = scratch.app.layout().focused();
        let terminal = scratch.app.open_tabs.terminal(&focused).expect("a shell");
        assert_eq!(terminal.cwd().map(PathBuf::from), Some(root));
    }

    #[test]
    fn test_switching_over_a_running_program_asks_first_and_a_no_stays() {
        let mut scratch = Scratch::new("ask");
        scratch.add("b", scratch.folder("b"));
        scratch.shell(true);
        let before = scratch.app.layout().members();
        scratch.app.switch_workspace("b").expect("goes through");
        let asked = scratch.app.chrome.close_prompt().map(|prompt| prompt.pending);
        assert_eq!(asked, Some(PendingClose::Switch));
        assert!(scratch.app.workspace.active.is_none(), "not yet");
        scratch.app.chrome.overlay = None;

        scratch.app.answer_close_prompt(PendingClose::Switch, false);
        assert_eq!(scratch.app.layout().members(), before, "nothing closed");
        assert!(scratch.app.workspace.active.is_none());
        assert!(
            scratch.app.workspace.switch_to.is_none(),
            "the switch is off"
        );
    }

    #[test]
    fn test_a_yes_to_the_question_switches() {
        let mut scratch = Scratch::new("yes");
        scratch.add("b", scratch.folder("b"));
        scratch.app.workspace.switch_to = Some("b".to_string());
        scratch.app.answer_close_prompt(PendingClose::Switch, true);
        assert_eq!(scratch.app.workspace.active.as_deref(), Some("b"));
    }

    #[test]
    fn test_switching_to_the_active_workspace_leaves_the_window_alone() {
        let mut scratch = Scratch::new("same");
        scratch.add("a", scratch.folder("a"));
        scratch.app.workspace.active = Some("a".to_string());
        let before = scratch.app.layout().members();
        scratch.app.switch_workspace("a").expect("goes through");
        assert_eq!(scratch.app.layout().members(), before);
    }
}

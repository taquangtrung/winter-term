//! Builds in Compile tabs: choosing one from the palette, the LaTeX
//! document a `.tex` file belongs to, the builds a save starts, and what
//! the window does as one starts.
//!
//! Each task has a Compile tab of its own, found by the build it runs, so
//! a test run never replaces a build still going; running a task again
//! reuses its tab, the way Emacs reuses `*compilation*`. Builds start out
//! of the way: the tab is shown in the pane beside the work, and the focus
//! stays where it was.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config::{
    are_tasks_trusted, remember_compile_command, remembered_compile_command, trust_tasks,
};
use crate::editors::compile::task::{self, Task};
use crate::editors::compile::{
    CompileEditor, NEXT_PLACE_COMMAND, PREVIOUS_PLACE_COMMAND, RERUN_COMMAND,
};
use crate::editors::synctex;
use crate::model::editor::{BuildStart, SAVE_ALL};
use crate::model::layout::TabId;
use crate::model::picker::{Picker, PickerEntry};
use crate::project;

use super::EditorKind;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// What `Rust: Run Cargo Fix` runs: the fixes the compiler suggests, made in
/// place even over edits not yet committed.
const CARGO_FIX: &str = "cargo fix --allow-dirty --allow-staged";

/// What a project with nothing to suggest is built with.
const FALLBACK_COMMAND: &str = "make -k";

/// The command that trusts a project's tasks file to run on its own.
pub(in crate::window) const TRUST_TASKS_TITLE: &str = "Compile: Trust This Project's Tasks File";

/// The task list's last entry, which asks for a command line instead.
const TYPE_A_COMMAND: &str = "Type a command...";

/// The action of [`TYPE_A_COMMAND`]'s entry.
const TYPE_ACTION: &str = "type";

/// How the task list marks the task run last, and the project's default.
const LAST_RUN_MARK: &str = "last run";
const DEFAULT_MARK: &str = "default";

// ========================================================================
// Data Structures
// ========================================================================

/// What the window keeps of its builds.
#[derive(Debug, Default)]
pub(in crate::window) struct Builds {
    /// The build each Compile tab runs, by which a task finds its tab.
    by_tab: HashMap<TabId, BuildStart>,
    /// The tasks the open task list offers, in its order.
    choices: Option<Choices>,
    /// The Compile tab whose build was started last: the one rerun and
    /// next-error mean.
    pub(in crate::window) newest: Option<TabId>,
}

/// The tasks a task list is offering, and the project they are for.
#[derive(Debug)]
struct Choices {
    root: PathBuf,
    tasks: Vec<Task>,
}

// ========================================================================
// Builds
// ========================================================================

impl Builds {
    /// Every Compile tab a build is kept for.
    #[cfg(test)]
    pub(in crate::window) fn tabs(&self) -> Vec<TabId> {
        self.by_tab.keys().copied().chain(self.newest).collect()
    }

    /// Forget the build of tab `tab`, which has closed.
    pub(in crate::window) fn forget(&mut self, tab: TabId) {
        self.by_tab.remove(&tab);
        if self.newest == Some(tab) {
            self.newest = None;
        }
    }
}

// ========================================================================
// App: compile
// ========================================================================

impl App {
    // --------------------------------------------------------------------
    // Asking
    // --------------------------------------------------------------------

    /// Offer the focused project's tasks, the one run last first.
    pub(in crate::window) fn compile_task(&mut self) {
        let (root, file) = self.compile_context();
        let (tasks, problem) =
            task::tasks_for(&root, file.as_deref(), &self.config.compile.latex_command);
        if let Some(problem) = problem {
            self.set_error(problem);
        }
        // One thing to run is run, with nothing asked and nothing brought to
        // the front: its Compile tab waits in the strip, and how it ended is
        // said in the status bar.
        if let [task] = tasks.as_slice() {
            let task = task.clone();
            self.run_task_quietly(&root, task);
            return;
        }
        let (tasks, marked) = ordered(tasks, remembered_compile_command(&root), &root);
        let mut entries: Vec<PickerEntry> = tasks
            .iter()
            .enumerate()
            .map(|(index, task)| PickerEntry {
                action: index.to_string(),
                icon: None,
                label: task.name.clone(),
                match_positions: Vec::new(),
                shortcut: task_note(task, index == 0 && marked.is_some(), marked),
            })
            .collect();
        entries.push(PickerEntry {
            action: TYPE_ACTION.to_string(),
            icon: None,
            label: TYPE_A_COMMAND.to_string(),
            match_positions: Vec::new(),
            shortcut: String::new(),
        });
        let title = format!("Run a task in {}", project_name(&root));
        self.host.compile.choices = Some(Choices { root, tasks });
        self.open_picker(Picker::open_tasks(entries, title));
    }

    /// Ask for a command line to build the focused project with, starting
    /// from the one it was last built with.
    pub(in crate::window) fn compile_command(&mut self) {
        let (root, file) = self.compile_context();
        let initial = self.default_task(&root, file.as_deref()).command;
        let title = format!("Compile in {}", project_name(&root));
        let (tasks, _) =
            task::tasks_for(&root, file.as_deref(), &self.config.compile.latex_command);
        self.host.compile.choices = Some(Choices { root, tasks });
        self.open_picker(Picker::open_compile_command(&initial, title));
    }

    /// Run the task the task list's entry `action` stands for.
    pub(in crate::window) fn choose_task(&mut self, action: &str) {
        if action == TYPE_ACTION {
            let initial = self
                .host.compile
                .choices
                .as_ref()
                .and_then(|choices| choices.tasks.first())
                .map(|task| task.command.clone())
                .unwrap_or_default();
            let title = self
                .host.compile
                .choices
                .as_ref()
                .map(|choices| format!("Compile in {}", project_name(&choices.root)))
                .unwrap_or_default();
            self.open_picker(Picker::open_compile_command(&initial, title));
            return;
        }
        let chosen = self.host.compile.choices.take().and_then(|choices| {
            let task = action
                .parse::<usize>()
                .ok()
                .and_then(|index| choices.tasks.get(index).cloned())?;
            Some((choices.root, task))
        });
        if let Some((root, task)) = chosen {
            self.run_task(&root, task);
        }
    }

    /// Run the command line typed for the task list's project, as the task
    /// it names where it names one.
    pub(in crate::window) fn run_typed_command(&mut self, command: &str) {
        let command = command.trim();
        let Some(choices) = self.host.compile.choices.take() else {
            return;
        };
        if command.is_empty() {
            return;
        }
        let task = choices
            .tasks
            .iter()
            .find(|task| task.command == command)
            .cloned()
            .unwrap_or_else(|| Task::shell(command, choices.root.clone()));
        self.run_task(&choices.root, task);
    }

    // --------------------------------------------------------------------
    // Running
    // --------------------------------------------------------------------

    /// Run `command` through the shell in the focused file's project, in its
    /// Compile tab.
    pub(in crate::window) fn run_shell_task(&mut self, command: &str) {
        let (root, _) = self.compile_context();
        self.run_task(&root, Task::shell(command, root.clone()));
    }

    /// Run `cargo fix` over the focused file's project, in its Compile tab.
    pub(in crate::window) fn cargo_fix(&mut self) {
        let (root, _) = self.compile_context();
        self.save_edited_files();
        self.run_task(&root, Task::shell(CARGO_FIX, root.clone()));
    }

    /// Run the newest build again, from wherever the focus is; where none
    /// has run, the focused project's default. A buried one comes back
    /// where a new build would show, without taking the focus.
    pub(in crate::window) fn recompile(&mut self) {
        if let Some(tab) = self.newest_build() {
            self.show_without_focus(tab);
            self.run_compile_command(tab, RERUN_COMMAND);
            return;
        }
        let (root, file) = self.compile_context();
        let task = self.default_task(&root, file.as_deref());
        self.run_task(&root, task);
    }

    /// Build the LaTeX document the focused `.tex` file belongs to.
    pub(in crate::window) fn compile_latex(&mut self) {
        match self.focused_file().filter(|file| synctex::is_tex(file)) {
            Some(file) => self.build_document(&file),
            None => self.set_error("LaTeX: not a LaTeX file on this machine"),
        }
    }

    /// A file was saved: a `.tex` one rebuilds its document where the
    /// settings ask for that, and the project's `on-save` tasks run.
    pub(in crate::window) fn on_file_written(&mut self, path: &Path) {
        if crate::remote::parse(path).is_some() {
            return;
        }
        self.emit_extension_event(&winter_extension::Event::Saved(path.to_path_buf()));
        if self.config.compile.latex_on_save && synctex::is_tex(path) {
            self.build_document(path);
        }
        let start = path.parent().unwrap_or(path);
        let root = project::root(start);
        let on_save: Vec<Task> = task::project_tasks(&root)
            .unwrap_or_default()
            .into_iter()
            .filter(|task| task.on_save)
            .collect();
        if on_save.is_empty() {
            return;
        }
        // A cloned repository's tasks file is anyone's commands: they run
        // on a save only once the user has said this file may.
        let tasks = std::fs::read_to_string(root.join(task::PROJECT_TASKS)).unwrap_or_default();
        if !are_tasks_trusted(&root, &tasks) {
            self.set_error(format!(
                "on-save tasks in {} did not run: trust them with \"{TRUST_TASKS_TITLE}\"",
                task::PROJECT_TASKS
            ));
            return;
        }
        for task in on_save {
            self.run_task(&root, task);
        }
    }

    /// Trust the focused project's `.winter/tasks.kdl`, as it reads now, to
    /// run its `on-save` tasks on its own.
    pub(in crate::window) fn trust_project_tasks(&mut self) {
        let root = project::root(&self.focused_start_dir());
        match std::fs::read_to_string(root.join(task::PROJECT_TASKS)) {
            Ok(tasks) => {
                trust_tasks(&root, &tasks);
                self.set_notice(format!("trusted {}", root.join(task::PROJECT_TASKS).display()));
            }
            Err(_) => self.set_error(format!("no {} in {}", task::PROJECT_TASKS, root.display())),
        }
    }

    /// The Compile tab `tab` is starting `build`: save what is being
    /// edited first, and take it as the newest build.
    pub(in crate::window) fn on_build_start(&mut self, tab: TabId, build: BuildStart) {
        self.save_edited_files();
        remember_compile_command(&build.root, &build.command);
        self.host.compile.by_tab.insert(tab, build);
        self.host.compile.newest = Some(tab);
        self.dirty = true;
    }

    /// Open the next or previous place the Compile tab `tab`'s build
    /// pointed at. `false` when `tab` is no Compile tab.
    pub(in crate::window) fn step_compile_error(&mut self, tab: TabId, forward: bool) -> bool {
        if !self.is_compile_tab(tab) {
            return false;
        }
        let command = match forward {
            true => NEXT_PLACE_COMMAND,
            false => PREVIOUS_PLACE_COMMAND,
        };
        self.run_compile_command(tab, command);
        true
    }

    /// The Compile tab whose build was started last, while it is open.
    pub(in crate::window) fn newest_build(&self) -> Option<TabId> {
        self.host.compile.newest.filter(|tab| self.is_compile_tab(*tab))
    }

    /// Build the document `file` belongs to in its own Compile tab.
    fn build_document(&mut self, file: &Path) {
        let document = synctex::root_of(file);
        let root = project::root(document.parent().unwrap_or(file));
        let task = Task::latex(&self.config.compile.latex_command, &document);
        self.run_task(&root, task);
    }

    /// Run `task` of the project at `root` in its Compile tab, opening one
    /// beside the work where it has none. The focus stays where it is.
    fn run_task(&mut self, root: &Path, task: Task) {
        let build = task.build_start(root);
        let tab = match self.tab_running(&build) {
            Some(tab) => {
                self.show_without_focus(tab);
                tab
            }
            None => {
                let editor =
                    CompileEditor::new(root.to_path_buf(), task, self.config.compile.jump_to_error);
                let tab = match self.config.compile.dock {
                    true => self.show_in_dock_without_focus(Box::new(editor)),
                    false => self.show_beside_without_focus(Box::new(editor)),
                };
                self.host.compile.by_tab.insert(tab, build);
                tab
            }
        };
        self.run_compile_command(tab, RERUN_COMMAND);
    }

    /// Run `task` of the project at `root` without bringing its Compile tab
    /// to the front: in the tab that ran it before, where it is, put back in
    /// the strip behind the focused tab when it was buried, else in a new one
    /// left there.
    fn run_task_quietly(&mut self, root: &Path, task: Task) {
        let build = task.build_start(root);
        let tab = match self.tab_running(&build) {
            Some(tab) => {
                if self.open_tabs.unbury(tab) {
                    let focused = self.layout().focused();
                    self.layout_mut().add_background_tab(focused, tab);
                }
                tab
            }
            None => {
                let editor =
                    CompileEditor::new(root.to_path_buf(), task, self.config.compile.jump_to_error);
                let tab = self.add_background_editor(EditorKind::Compile, Box::new(editor));
                self.host.compile.by_tab.insert(tab, build);
                tab
            }
        };
        let command = self
            .host.compile
            .by_tab
            .get(&tab)
            .map(|build| build.command.clone())
            .unwrap_or_default();
        self.set_notice(format!("building: {command}"));
        self.run_compile_command(tab, RERUN_COMMAND);
    }

    /// Have the Compile tab `tab` carry out `command`.
    fn run_compile_command(&mut self, tab: TabId, command: &str) {
        let outcome = self
            .open_tabs
            .editor_mut(&tab)
            .and_then(|slot| slot.editor.run_command(command));
        if let Some(outcome) = outcome {
            self.act_on_editor_outcome(tab, outcome);
        }
        self.dirty = true;
    }

    /// Write every file a text editor holds edits to.
    fn save_edited_files(&mut self) {
        let edited: Vec<TabId> = self
            .open_tabs
            .editors()
            .filter(|(_, slot)| slot.kind() == EditorKind::Text && slot.editor.is_dirty())
            .map(|(tab, _)| *tab)
            .collect();
        for tab in edited {
            let outcome = self
                .open_tabs
                .editor_mut(&tab)
                .and_then(|slot| slot.editor.run_command(SAVE_ALL));
            if let Some(outcome) = outcome {
                self.act_on_editor_outcome(tab, outcome);
            }
        }
    }

    // --------------------------------------------------------------------
    // Tabs
    // --------------------------------------------------------------------

    /// Show `editor` in the dock panel along the bottom of the window,
    /// leaving focus where it was.
    fn show_in_dock_without_focus(&mut self, editor: Box<CompileEditor>) -> TabId {
        let back = self.layout().focused();
        let tab = self.show_editor_in_dock(EditorKind::Compile, editor);
        self.set_focus(back);
        tab
    }

    /// Show `editor` in a new tab in the pane beside the work, splitting
    /// one off where there is only one, and leave the focus where it was.
    fn show_beside_without_focus(&mut self, editor: Box<CompileEditor>) -> TabId {
        let back = self.layout().focused();
        self.prefer_other_pane();
        let tab = self.show_editor_beside(EditorKind::Compile, editor);
        self.set_focus(back);
        tab
    }

    /// Bring `tab` to the front of its pane, leaving the focus where it was.
    /// A buried one comes back where a new Compile tab would open: in the
    /// dock, or in the pane beside the work.
    fn show_without_focus(&mut self, tab: TabId) {
        let back = self.layout().focused();
        if back == tab {
            return;
        }
        match self.open_tabs.buried().contains(&tab) {
            true if self.config.compile.dock => self.show_buried_in_dock(tab),
            true => {
                self.prefer_other_pane();
                self.show_buried_beside(tab);
            }
            false => self.show_tab(tab),
        }
        self.set_focus(back);
    }

    /// The open Compile tab running `build`.
    fn tab_running(&self, build: &BuildStart) -> Option<TabId> {
        self.host.compile
            .by_tab
            .iter()
            .find(|(tab, known)| *known == build && self.is_compile_tab(**tab))
            .map(|(tab, _)| *tab)
    }

    fn is_compile_tab(&self, tab: TabId) -> bool {
        self.open_tabs
            .editor(&tab)
            .is_some_and(|slot| slot.kind() == EditorKind::Compile)
    }

    // --------------------------------------------------------------------
    // Context
    // --------------------------------------------------------------------

    /// The project a build asked for now belongs to, and the file the
    /// focused text editor has open there.
    fn compile_context(&self) -> (PathBuf, Option<PathBuf>) {
        let file = self.focused_file();
        let start = file
            .as_deref()
            .and_then(Path::parent)
            .map_or_else(|| self.focused_start_dir(), Path::to_path_buf);
        (project::root(&start), file)
    }

    /// The file the focused text editor has open, where it is on this
    /// machine.
    fn focused_file(&self) -> Option<PathBuf> {
        let focused = self.layout().focused();
        self.open_tabs
            .editor(&focused)
            .filter(|slot| slot.kind() == EditorKind::Text)
            .and_then(|slot| slot.editor.snapshot())
            .map(|target| target.path)
            .filter(|path| crate::remote::parse(path).is_none())
    }

    /// What a project builds with when nothing says otherwise: the
    /// document a `.tex` file belongs to, else the command it was last
    /// built with, else its default task, else the first it offers.
    fn default_task(&self, root: &Path, file: Option<&Path>) -> Task {
        let (tasks, _) = task::tasks_for(root, file, &self.config.compile.latex_command);
        if file.is_some_and(synctex::is_tex) {
            if let Some(document) = tasks.first().filter(|task| task.document.is_some()) {
                return document.clone();
            }
        }
        let (tasks, _) = ordered(tasks, remembered_compile_command(root), root);
        tasks
            .into_iter()
            .next()
            .unwrap_or_else(|| Task::shell(FALLBACK_COMMAND, root.to_path_buf()))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `tasks` with the one run last at the top, else the project's default,
/// and which of those leads, if either does. A command run last that no
/// task names leads as a task of its own.
fn ordered(
    mut tasks: Vec<Task>,
    last: Option<String>,
    root: &Path,
) -> (Vec<Task>, Option<&'static str>) {
    if let Some(command) = last {
        let task = match tasks.iter().position(|task| task.command == command) {
            Some(at) => tasks.remove(at),
            None => Task::shell(command, root.to_path_buf()),
        };
        tasks.insert(0, task);
        return (tasks, Some(LAST_RUN_MARK));
    }
    match tasks.iter().position(|task| task.default) {
        Some(at) => {
            let task = tasks.remove(at);
            tasks.insert(0, task);
            (tasks, Some(DEFAULT_MARK))
        }
        None => (tasks, None),
    }
}

/// What the task list says at a task's right: why it leads, where it
/// does, and its command where its name does not say it.
fn task_note(task: &Task, leads: bool, mark: Option<&str>) -> String {
    let command = (task.name != task.command).then_some(task.command.as_str());
    match (leads.then_some(mark).flatten(), command) {
        (Some(mark), Some(command)) => format!("{mark} · {command}"),
        (Some(mark), None) => mark.to_string(),
        (None, Some(command)) => command.to_string(),
        (None, None) => String::new(),
    }
}

/// A project's name, for the task list's title: its root's own name.
fn project_name(root: &Path) -> String {
    root.file_name().map_or_else(
        || root.display().to_string(),
        |name| name.to_string_lossy().to_string(),
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn shell(command: &str) -> Task {
        Task::shell(command, PathBuf::from("/p"))
    }

    #[test]
    fn test_the_task_run_last_leads_the_list_and_says_so() {
        let tasks = vec![shell("cargo build"), shell("cargo test")];
        let (ordered, mark) = ordered(tasks, Some("cargo test".into()), Path::new("/p"));
        assert_eq!(ordered[0].command, "cargo test");
        assert_eq!(ordered.len(), 2, "moved to the top, not added twice");
        assert_eq!(mark, Some(LAST_RUN_MARK));
    }

    #[test]
    fn test_a_command_run_last_that_no_task_names_leads_as_its_own() {
        let (ordered, _) = ordered(
            vec![shell("make")],
            Some("make -j8".into()),
            Path::new("/p"),
        );
        assert_eq!(
            ordered
                .iter()
                .map(|t| t.command.as_str())
                .collect::<Vec<_>>(),
            ["make -j8", "make"]
        );
    }

    #[test]
    fn test_with_nothing_run_yet_the_default_task_leads() {
        let mut default = shell("cargo check");
        default.default = true;
        let (ordered, mark) = ordered(vec![shell("cargo build"), default], None, Path::new("/p"));
        assert_eq!(ordered[0].command, "cargo check");
        assert_eq!(mark, Some(DEFAULT_MARK));
    }

    #[test]
    fn test_a_tasks_note_gives_why_it_leads_and_a_command_its_name_hides() {
        let mut named = shell("cargo build --workspace");
        named.name = "build".to_string();
        assert_eq!(
            task_note(&named, true, Some(LAST_RUN_MARK)),
            "last run · cargo build --workspace"
        );
        assert_eq!(task_note(&shell("make"), false, Some(LAST_RUN_MARK)), "");
    }
}

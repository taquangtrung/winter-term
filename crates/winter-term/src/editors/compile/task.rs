//! What can be built, and how: the tasks a project names in
//! `.winter/tasks.kdl`, the ones its files make obvious (a `Cargo.toml`
//! builds with cargo, a `Makefile` with make, a LaTeX document with
//! latexmk), and the command last run there.
//!
//! The project file is a list of named shell commands, each run in the
//! project's root or in the directory `cwd` names. `default=#true` marks
//! the one a project builds with until another is chosen, `on-save=#true`
//! one run whenever a file of the project is saved in Winter, and `env`
//! children set variables for it:
//!
//! ```kdl
//! task "build" "cargo build --workspace" default=#true
//! task "check" "cargo check" on-save=#true
//! task "docs" "make html" cwd="docs" {
//!     env SPHINXOPTS="-W"
//! }
//! ```

use std::fs;
use std::path::{Path, PathBuf};

use kdl::{KdlDocument, KdlNode};

use super::tex::latex_command;
use crate::editors::synctex;
use crate::model::editor::BuildStart;

// ========================================================================
// Constants
// ========================================================================

/// The project's own task list, under its root.
pub const PROJECT_TASKS: &str = ".winter/tasks.kdl";

/// The node a task is written as.
const TASK_NODE: &str = "task";

/// The property naming the directory a task runs in.
const CWD_PROPERTY: &str = "cwd";

/// The property marking the task a project builds with by default.
const DEFAULT_PROPERTY: &str = "default";

/// The property marking a task run whenever a file is saved.
const ON_SAVE_PROPERTY: &str = "on-save";

/// The child node setting environment variables, as its properties.
const ENV_NODE: &str = "env";

/// The makefiles GNU make reads, in its order.
const MAKEFILES: [&str; 3] = ["GNUmakefile", "makefile", "Makefile"];

/// The `package.json` scripts offered as tasks, where a project has them.
const NPM_SCRIPTS: [&str; 3] = ["build", "test", "lint"];

/// The shell a task's command line is handed to.
#[cfg(unix)]
const SHELL: (&str, &str) = ("sh", "-c");
#[cfg(windows)]
const SHELL: (&str, &str) = ("cmd", "/C");

// ========================================================================
// Data Structures
// ========================================================================

/// One command a build can be run with.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Task {
    /// The shell command line.
    pub command: String,
    /// Whether the project builds with it until another is chosen.
    pub default: bool,
    /// Where it runs.
    pub dir: PathBuf,
    /// The LaTeX document it builds, whose log is read once it is done and
    /// whose PDF it makes.
    pub document: Option<PathBuf>,
    /// Variables set for it over Winter's own environment.
    pub env: Vec<(String, String)>,
    /// What the task is called in a list of them.
    pub name: String,
    /// Whether saving a file of the project runs it.
    pub on_save: bool,
}

// ========================================================================
// Task
// ========================================================================

impl Task {
    /// A shell command run in `dir`, named for itself.
    pub fn shell(command: impl Into<String>, dir: PathBuf) -> Self {
        let command = command.into();
        Self {
            name: command.clone(),
            command,
            default: false,
            dir,
            document: None,
            env: Vec::new(),
            on_save: false,
        }
    }

    /// The build of the LaTeX document `root`, run beside it with
    /// `command`.
    pub fn latex(command: &str, root: &Path) -> Self {
        let dir = root.parent().map_or_else(PathBuf::new, Path::to_path_buf);
        let name = root
            .file_name()
            .map_or_else(String::new, |name| name.to_string_lossy().to_string());
        Self {
            command: latex_command(command, root),
            default: false,
            dir,
            document: Some(root.to_path_buf()),
            env: Vec::new(),
            name: format!("LaTeX: {name}"),
            on_save: false,
        }
    }

    /// The same task with another command line, as typed over it, in the
    /// same directory and environment. A typed command that no longer runs
    /// the document's build is a plain shell command.
    pub fn with_command(&self, command: String) -> Self {
        if command == self.command {
            return self.clone();
        }
        Self {
            env: self.env.clone(),
            ..Self::shell(command, self.dir.clone())
        }
    }

    /// The build as the host knows it, in the project at `root`.
    pub fn build_start(&self, root: &Path) -> BuildStart {
        BuildStart {
            command: self.command.clone(),
            dir: self.dir.clone(),
            root: root.to_path_buf(),
        }
    }

    /// The program and its arguments: the shell, handed the command line.
    pub fn argv(&self) -> (String, Vec<String>) {
        (
            SHELL.0.to_string(),
            vec![SHELL.1.to_string(), self.command.clone()],
        )
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The tasks a build could start with in `root`, most likely first: the
/// LaTeX document `file` belongs to, the project's own list, then the ones
/// its files make obvious. A task list that cannot be read is reported
/// beside what could be found.
pub fn tasks_for(root: &Path, file: Option<&Path>, latex: &str) -> (Vec<Task>, Option<String>) {
    let mut tasks = Vec::new();
    if let Some(file) = file.filter(|file| synctex::is_tex(file)) {
        tasks.push(Task::latex(latex, &synctex::root_of(file)));
    }
    let problem = match project_tasks(root) {
        Ok(listed) => {
            tasks.extend(listed);
            None
        }
        Err(problem) => Some(problem),
    };
    for task in detected_tasks(root) {
        if !tasks
            .iter()
            .any(|known| known.command == task.command && known.dir == task.dir)
        {
            tasks.push(task);
        }
    }
    (tasks, problem)
}

/// The tasks `root`'s `.winter/tasks.kdl` lists, none where it has no such
/// file, and why not where the file cannot be read.
pub fn project_tasks(root: &Path) -> Result<Vec<Task>, String> {
    let path = root.join(PROJECT_TASKS);
    let Ok(text) = fs::read_to_string(&path) else {
        return Ok(Vec::new());
    };
    let document: KdlDocument = text
        .parse()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    document
        .nodes()
        .iter()
        .filter(|node| node.name().value() == TASK_NODE)
        .map(|node| {
            listed_task(node, root).ok_or_else(|| {
                format!(
                    "{}: a task is written `task \"name\" \"command\" [cwd=\"dir\"]`",
                    path.display()
                )
            })
        })
        .collect()
}

/// A `task "name" "command" cwd="dir"` node as a task.
fn listed_task(node: &KdlNode, root: &Path) -> Option<Task> {
    let mut arguments = node
        .entries()
        .iter()
        .filter(|entry| entry.name().is_none())
        .filter_map(|entry| entry.value().as_string());
    let name = arguments.next()?.to_string();
    let command = arguments.next()?.to_string();
    let property = |key: &str| {
        node.entries()
            .iter()
            .find(|entry| entry.name().is_some_and(|name| name.value() == key))
            .map(|entry| entry.value())
    };
    let flag = |key: &str| {
        property(key)
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    };
    let cwd = property(CWD_PROPERTY).and_then(|value| value.as_string());
    let env = node
        .children()
        .map(|children| {
            children
                .nodes()
                .iter()
                .filter(|child| child.name().value() == ENV_NODE)
                .flat_map(|child| child.entries())
                .filter_map(|entry| {
                    let key = entry.name()?.value().to_string();
                    let value = entry.value();
                    let text = value
                        .as_string()
                        .map_or_else(|| value.to_string(), str::to_string);
                    Some((key, text))
                })
                .collect()
        })
        .unwrap_or_default();
    Some(Task {
        command,
        default: flag(DEFAULT_PROPERTY),
        dir: cwd.map_or_else(|| root.to_path_buf(), |cwd| root.join(cwd)),
        document: None,
        env,
        name,
        on_save: flag(ON_SAVE_PROPERTY),
    })
}

/// The builds `root`'s files make obvious.
pub fn detected_tasks(root: &Path) -> Vec<Task> {
    let has = |name: &str| root.join(name).is_file();
    let mut tasks = Vec::new();
    let mut add = |command: &str| tasks.push(Task::shell(command, root.to_path_buf()));
    if has("Cargo.toml") {
        add("cargo build");
        add("cargo test");
        add("cargo clippy --all-targets");
    }
    if MAKEFILES.iter().any(|name| has(name)) {
        add("make -k");
    }
    if has("go.mod") {
        add("go build ./...");
        add("go test ./...");
    }
    for script in npm_scripts(root) {
        add(&format!("npm run {script}"));
    }
    if has("latexmkrc") || has(".latexmkrc") {
        add("latexmk");
    }
    tasks
}

/// Which of [`NPM_SCRIPTS`] `root`'s `package.json` defines.
fn npm_scripts(root: &Path) -> Vec<&'static str> {
    let Ok(text) = fs::read_to_string(root.join("package.json")) else {
        return Vec::new();
    };
    let Ok(package) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let scripts = &package["scripts"];
    NPM_SCRIPTS
        .into_iter()
        .filter(|script| scripts.get(script).is_some())
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh directory for one test.
    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("winter-task-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("creates the directory");
        dir
    }

    #[test]
    fn test_the_project_list_comes_before_what_its_files_suggest() {
        let root = scratch("list");
        fs::create_dir_all(root.join(".winter")).expect("creates .winter");
        fs::write(
            root.join(PROJECT_TASKS),
            "task \"all\" \"cargo build --workspace\"\ntask \"docs\" \"make html\" cwd=\"docs\"\n",
        )
        .expect("writes the list");
        fs::write(root.join("Cargo.toml"), "").expect("writes Cargo.toml");
        let (tasks, problem) = tasks_for(&root, None, "latexmk");
        assert_eq!(problem, None);
        let names: Vec<&str> = tasks.iter().map(|task| task.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "all",
                "docs",
                "cargo build",
                "cargo test",
                "cargo clippy --all-targets"
            ]
        );
        assert_eq!(tasks[1].dir, root.join("docs"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_listed_task_carries_its_flags_and_its_environment() {
        let root = scratch("flags");
        fs::create_dir_all(root.join(".winter")).expect("creates .winter");
        fs::write(
            root.join(PROJECT_TASKS),
            "task \"check\" \"cargo check\" default=#true on-save=#true {\n    env RUST_LOG=\"debug\" LEVEL=3\n}\ntask \"plain\" \"true\"\n",
        )
        .expect("writes the list");
        let tasks = project_tasks(&root).expect("reads the list");
        assert!(tasks[0].default && tasks[0].on_save);
        assert_eq!(
            tasks[0].env,
            [
                ("RUST_LOG".to_string(), "debug".to_string()),
                ("LEVEL".to_string(), "3".to_string())
            ]
        );
        assert!(!tasks[1].default && !tasks[1].on_save && tasks[1].env.is_empty());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_list_that_does_not_parse_is_reported_and_detection_still_offers() {
        let root = scratch("broken");
        fs::create_dir_all(root.join(".winter")).expect("creates .winter");
        fs::write(root.join(PROJECT_TASKS), "task \"only a name\"\n").expect("writes the list");
        fs::write(root.join("Makefile"), "").expect("writes Makefile");
        let (tasks, problem) = tasks_for(&root, None, "latexmk");
        assert!(problem.is_some_and(|p| p.contains("task \"name\" \"command\"")));
        assert_eq!(tasks, [Task::shell("make -k", root.clone())]);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_a_tex_file_offers_its_documents_build_first() {
        let root = scratch("tex");
        fs::create_dir_all(root.join("sections")).expect("creates sections");
        fs::write(root.join("main.tex"), "\\documentclass{article}\n").expect("writes the root");
        let section = root.join("sections").join("intro.tex");
        fs::write(&section, "Intro.\n").expect("writes the section");
        let (tasks, _) = tasks_for(&root, Some(&section), "latexmk -pdf");
        assert_eq!(tasks[0].command, "latexmk -pdf main.tex");
        assert_eq!(tasks[0].dir, root);
        assert_eq!(tasks[0].document, Some(root.join("main.tex")));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn test_retyping_a_documents_command_keeps_it_a_document_build_only_unchanged() {
        let task = Task::latex("latexmk", Path::new("/doc/main.tex"));
        assert!(task.with_command(task.command.clone()).document.is_some());
        assert!(task.with_command("make".into()).document.is_none());
    }

    #[test]
    fn test_only_the_npm_scripts_a_package_defines_are_offered() {
        let root = scratch("npm");
        fs::write(
            root.join("package.json"),
            r#"{"scripts": {"test": "jest", "start": "node ."}}"#,
        )
        .expect("writes package.json");
        let commands: Vec<String> = detected_tasks(&root)
            .into_iter()
            .map(|t| t.command)
            .collect();
        assert_eq!(commands, ["npm run test"]);
        let _ = fs::remove_dir_all(&root);
    }
}

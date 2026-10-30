//! Workspaces: named sets of root folders, each with the layout it was last
//! left in, so a window can be switched from one project to another and find
//! each as it was.
//!
//! A workspace is Winter's own data, never a file in the project: each is one
//! `<name>.json` in the `workspaces` directory under the state directory
//! (see [`crate::paths::state_file`]). The file's name is the workspace's
//! name, so a rename is a rename of the file.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use crate::session::Session;

// ========================================================================
// Constants
// ========================================================================

/// The directory the workspaces are kept in, under the state directory.
const STORE_DIR_NAME: &str = "workspaces";

/// The extension of a workspace's file.
const EXTENSION: &str = "json";

/// What a workspace is called when its folder's name gives nothing to use.
const FALLBACK_NAME: &str = "workspace";

/// Said when a name is taken by another workspace.
const NAME_TAKEN: &str = "a workspace by that name already exists";

// ========================================================================
// Data Structures
// ========================================================================

/// A named set of root folders, with the layout it was last left in.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Workspace {
    /// What the workspace is called: its file's name, so not written in it.
    #[serde(skip)]
    pub name: String,
    /// The folders the workspace is made of, the first the one it opens in.
    pub roots: Vec<PathBuf>,
    /// The window as it was when the workspace was last left; `None` for one
    /// never left, which opens as one terminal at its first root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Session>,
    /// The settings this workspace changes over `settings.kdl`, by the key and
    /// value the settings page writes them with, in force while it is the
    /// window's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings: BTreeMap<String, String>,
}

/// Where the workspaces are kept: a directory of one file each.
pub struct Store {
    dir: PathBuf,
}

// ========================================================================
// Workspace Implementations
// ========================================================================

impl Workspace {
    /// A workspace of the one folder `root`, never opened yet.
    pub fn new(name: String, root: PathBuf) -> Self {
        Self {
            name,
            roots: vec![root],
            session: None,
            settings: BTreeMap::new(),
        }
    }

    /// The root of this workspace that `dir` is at or under, the deepest when
    /// roots nest.
    pub fn root_for(&self, dir: &Path) -> Option<&Path> {
        self.roots
            .iter()
            .filter(|root| dir.starts_with(root))
            .max_by_key(|root| root.components().count())
            .map(PathBuf::as_path)
    }
}

// ========================================================================
// Store Implementations
// ========================================================================

impl Store {
    /// The store in the state directory, or `None` when there is none.
    pub fn open() -> Option<Self> {
        crate::paths::state_file(STORE_DIR_NAME).map(Self::at)
    }

    /// The store kept in `dir`.
    pub fn at(dir: PathBuf) -> Self {
        Self { dir }
    }

    /// Every workspace, by name. A file that cannot be read as one is left
    /// out rather than failing the rest.
    pub fn list(&self) -> Vec<Workspace> {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut workspaces: Vec<Workspace> = entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let path = entry.path();
                let name = path.file_stem()?.to_str()?;
                // A write in flight is a hidden file beside the real one.
                let real = path.extension()? == EXTENSION && invalid_name(name).is_none();
                real.then(|| self.load(name)).flatten()
            })
            .collect();
        workspaces.sort_by(|a, b| a.name.cmp(&b.name));
        workspaces
    }

    /// The workspace called `name`, if there is one that can be read.
    pub fn load(&self, name: &str) -> Option<Workspace> {
        let text = fs::read_to_string(self.path_of(name).ok()?).ok()?;
        let mut workspace: Workspace = serde_json::from_str(&text).ok()?;
        workspace.name = name.to_string();
        Some(workspace)
    }

    /// Write `workspace` under its name in one step (see
    /// [`crate::persist::write_atomic`]).
    pub fn save(&self, workspace: &Workspace) -> io::Result<()> {
        let path = self.path_of(&workspace.name)?;
        let json = serde_json::to_string_pretty(workspace).map_err(io::Error::other)?;
        crate::persist::write_atomic(&path, json)
    }

    /// Call the workspace `from` by the name `to` instead. Refused when `to`
    /// is no name or is taken, so a rename never overwrites another.
    pub fn rename(&self, from: &str, to: &str) -> Result<(), String> {
        let source = self.path_of(from).map_err(|e| e.to_string())?;
        let target = self.path_of(to).map_err(|e| e.to_string())?;
        if target.exists() {
            return Err(NAME_TAKEN.to_string());
        }
        fs::rename(source, target).map_err(|e| e.to_string())
    }

    /// Forget the workspace `name`. Its folders are not touched.
    pub fn remove(&self, name: &str) -> io::Result<()> {
        fs::remove_file(self.path_of(name)?)
    }

    /// Where the workspace `name` is kept, refusing a name that is no
    /// workspace's: one arriving over the control socket as `../session`
    /// would otherwise reach a file outside the workspaces' directory.
    fn path_of(&self, name: &str) -> io::Result<PathBuf> {
        match invalid_name(name) {
            Some(problem) => Err(io::Error::other(problem)),
            None => Ok(self.dir.join(format!("{name}.{EXTENSION}"))),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The workspace `dir` belongs to: the one with the deepest root at or above
/// it.
pub fn containing<'a>(dir: &Path, workspaces: &'a [Workspace]) -> Option<&'a Workspace> {
    workspaces
        .iter()
        .filter_map(|workspace| Some((workspace.root_for(dir)?, workspace)))
        .max_by_key(|(root, _)| root.components().count())
        .map(|(_, workspace)| workspace)
}

/// Why `name` cannot name a workspace, or `None` when it can: it must be a
/// plain file name, and not a hidden one, which is how a write in flight is
/// told apart.
pub fn invalid_name(name: &str) -> Option<&'static str> {
    if name.trim().is_empty() {
        return Some("a workspace needs a name");
    }
    if name.starts_with('.') {
        return Some("a workspace name cannot start with a dot");
    }
    if name
        .chars()
        .any(|c| matches!(c, '/' | '\\' | ':') || c.is_control())
    {
        return Some("a workspace name cannot hold '/', '\\', ':', or control characters");
    }
    None
}

/// A free name for a workspace rooted at `root`: its folder's name, numbered
/// past the ones `taken` already has.
pub fn suggest_name(root: &Path, taken: &[Workspace]) -> String {
    let folder = root
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let cleaned: String = folder
        .trim_start_matches('.')
        .chars()
        .map(|c| match invalid_name(&c.to_string()) {
            Some(_) => '-',
            None => c,
        })
        .collect();
    let base = match cleaned.trim().is_empty() {
        true => FALLBACK_NAME.to_string(),
        false => cleaned,
    };
    let free = |name: &str| taken.iter().all(|workspace| workspace.name != name);
    if free(&base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base}-{n}"))
        .find(|name| free(name))
        .unwrap_or(base)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// A store in a directory of its own, gone once the test is.
    struct Scratch(Store);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("winter-workspaces-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            Self(Store::at(dir))
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0.dir);
        }
    }

    fn named(name: &str, roots: &[&str]) -> Workspace {
        Workspace {
            name: name.to_string(),
            roots: roots.iter().map(PathBuf::from).collect(),
            session: None,
            settings: BTreeMap::new(),
        }
    }

    #[test]
    fn test_a_saved_workspace_lists_and_loads_back_under_its_file_name() {
        let scratch = Scratch::new("round-trip");
        let store = &scratch.0;
        store.save(&named("web", &["/p/web"])).expect("saves web");
        store
            .save(&named("api", &["/p/api", "/p/shared"]))
            .expect("saves api");

        let names: Vec<String> = store.list().into_iter().map(|w| w.name).collect();
        assert_eq!(names, ["api", "web"], "by name");
        let api = store.load("api").expect("loads api");
        assert_eq!(
            api.roots,
            [PathBuf::from("/p/api"), PathBuf::from("/p/shared")]
        );
    }

    #[test]
    fn test_a_name_reaching_outside_the_store_is_refused_everywhere() {
        // `winter workspace remove ../session` over the control socket
        // deleted session.json beside the workspaces' directory.
        let scratch = Scratch::new("traversal");
        let store = &scratch.0;
        store.save(&named("web", &["/p/web"])).expect("saves web");
        let bait = format!("winter-bait-{}", std::process::id());
        let outside = store.dir.parent().expect("a parent").join(format!("{bait}.{EXTENSION}"));
        fs::write(&outside, "{}").expect("bait");
        let escape = format!("../{bait}");
        assert!(store.remove(&escape).is_err());
        assert!(outside.exists(), "the file outside is untouched");
        assert!(store.load(&escape).is_none());
        assert!(store.rename(&escape, "taken").is_err());
        assert!(outside.exists());
        let _ = fs::remove_file(&outside);
    }

    #[test]
    fn test_a_write_in_flight_is_not_listed() {
        let scratch = Scratch::new("in-flight");
        let store = &scratch.0;
        store.save(&named("web", &["/p/web"])).expect("saves web");
        fs::write(store.dir.join(".web.json.42.tmp"), "{").expect("writes a temp file");
        assert_eq!(store.list().len(), 1);
    }

    #[test]
    fn test_the_deepest_root_holding_a_folder_owns_it() {
        let workspaces = [named("outer", &["/p"]), named("inner", &["/q", "/p/sub"])];
        let owner = |dir: &str| containing(Path::new(dir), &workspaces).map(|w| w.name.as_str());
        assert_eq!(owner("/p/sub/x"), Some("inner"));
        assert_eq!(owner("/p/other"), Some("outer"));
        assert_eq!(owner("/elsewhere"), None);
        // A root is a path prefix by component, not by text.
        assert_eq!(owner("/pp"), None);
    }

    #[test]
    fn test_a_name_that_is_no_plain_file_name_is_refused() {
        for name in ["", "  ", ".", "..", ".hidden", "a/b", "a\\b", "c:d", "a\nb"] {
            assert!(invalid_name(name).is_some(), "{name:?}");
        }
        assert!(invalid_name("my app 2").is_none());
    }

    #[test]
    fn test_a_suggested_name_is_the_folder_numbered_past_ones_taken() {
        let taken = [named("app", &["/a"]), named("app-2", &["/b"])];
        assert_eq!(suggest_name(Path::new("/x/app"), &taken), "app-3");
        assert_eq!(suggest_name(Path::new("/x/site"), &taken), "site");
        assert_eq!(suggest_name(Path::new("/x/.dots"), &[]), "dots");
        assert_eq!(suggest_name(Path::new("/"), &[]), FALLBACK_NAME);
    }

    #[test]
    fn test_renaming_onto_a_taken_name_is_refused_and_keeps_both() {
        let scratch = Scratch::new("rename");
        let store = &scratch.0;
        store.save(&named("a", &["/a"])).expect("saves a");
        store.save(&named("b", &["/b"])).expect("saves b");

        assert!(store.rename("a", "b").is_err());
        assert_eq!(store.list().len(), 2);
        store.rename("a", "c").expect("renames to a free name");
        let c = store.load("c").expect("loads c");
        assert_eq!(c.roots, [PathBuf::from("/a")]);
        assert!(store.load("a").is_none());
    }
}

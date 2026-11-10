//! The project a directory belongs to: the one root every project-wide
//! command (the file picker, search, the Todo list, a build) works from.

use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// What a project's root holds, nearest first: a task list of Winter's
/// own, else a repository.
const ROOT_MARKERS: [&str; 2] = [".winter", ".git"];

// ========================================================================
// Functions
// ========================================================================

/// The root of the project `start` is in: the nearest directory at or
/// above it with a task list or a repository, else `start` itself.
///
/// The markers are looked for rather than git asked, so a root is known
/// without starting a program, and a worktree or a submodule, whose `.git`
/// is a file, is a root of its own as git would say.
pub fn root(start: &Path) -> PathBuf {
    marked_root(start).unwrap_or_else(|| start.to_path_buf())
}

/// The nearest directory at or above `start` with a task list or a
/// repository, or `None` where `start` is in no project at all.
pub fn marked_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| ROOT_MARKERS.iter().any(|marker| dir.join(marker).exists()))
        .map(Path::to_path_buf)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    use std::fs;

    /// A directory of its own under the system's temporary one, gone once
    /// the test is.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("winter-project-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("creates the scratch dir");
            Self(dir)
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_the_nearest_repository_above_is_the_root() {
        let tree = Scratch::new("repo");
        let deep = tree.0.join("a").join("b");
        fs::create_dir_all(&deep).expect("creates the tree");
        assert_eq!(root(&deep), deep, "under no marker, its own root");
        fs::create_dir_all(tree.0.join(".git")).expect("creates .git");
        assert_eq!(root(&deep), tree.0);
    }

    #[test]
    fn test_a_task_list_nearer_than_the_repository_wins() {
        let tree = Scratch::new("nearer");
        let sub = tree.0.join("sub");
        fs::create_dir_all(tree.0.join(".git")).expect("creates .git");
        fs::create_dir_all(sub.join(".winter")).expect("creates .winter");
        fs::create_dir_all(sub.join("x")).expect("creates sub/x");
        assert_eq!(root(&sub.join("x")), sub);
    }
}

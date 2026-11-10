//! What git says about the entries listed, drawn as an icon in the listing's
//! gutter: each changed file's state, each directory's most pressing change
//! within, and what git ignores, so a listing of a checkout shows where the
//! work is without opening the git view.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use winter_model::editor::CommandRequest;
use winter_vcs::exec::{Git, Pathspec};
use winter_vcs::parse::{parse_status, Section};

// ========================================================================
// Constants
// ========================================================================

/// The tag of the first command a refresh runs: where the repository's root
/// is, since git reports paths against it.
pub const TAG_ROOT: &str = "dir-git-root";

/// The tag of the second: what has changed.
pub const TAG_STATUS: &str = "dir-git-status";

/// The tag of a git command the actions menu runs on the entries: a move or
/// a forced add, whose answer is said and then read again from the status.
pub const TAG_ACTION: &str = "dir-git-action";

/// What starts a porcelain v2 record for a path git ignores.
const IGNORED_RECORD: &str = "! ";

/// What ends each porcelain record read with `-z`.
const RECORD_END: char = '\0';

// ========================================================================
// Data Structures
// ========================================================================

/// One entry's git state, the most pressing first.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum GitStatus {
    /// A merge conflict, which nothing proceeds past.
    Conflict,
    /// Changed in place.
    Modified,
    /// Deleted.
    Deleted,
    /// New to the index.
    Added,
    /// Renamed or copied.
    Renamed,
    /// Not tracked by git.
    Untracked,
    /// Ignored by git.
    Ignored,
}

/// Every listed path's git state, as read from one status.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct GitMarks {
    /// The state of each path git named, and of each directory above a change.
    marks: HashMap<PathBuf, GitStatus>,
    /// Directories git reported whole, untracked or ignored, without walking
    /// them: everything under one is in the same state.
    whole: HashMap<PathBuf, GitStatus>,
}

// ========================================================================
// GitStatus Implementations
// ========================================================================

impl GitStatus {
    /// The stem of the icon the bundled git set draws this state with.
    pub fn icon_stem(self) -> &'static str {
        match self {
            Self::Added => "git-added",
            Self::Conflict => "git-conflict",
            Self::Deleted => "git-deleted",
            Self::Ignored => "git-ignored",
            Self::Modified => "git-modified",
            Self::Renamed => "git-renamed",
            Self::Untracked => "git-untracked",
        }
    }

    /// The state of a change git reports in `section` with the one-letter
    /// `code`.
    fn of_change(section: Section, code: char) -> Self {
        match (section, code) {
            (Section::Unmerged, _) => Self::Conflict,
            (Section::Untracked, _) => Self::Untracked,
            (Section::Staged | Section::Unstaged, 'A') => Self::Added,
            (Section::Staged | Section::Unstaged, 'D') => Self::Deleted,
            (Section::Staged | Section::Unstaged, 'R' | 'C') => Self::Renamed,
            (Section::Staged | Section::Unstaged, _) => Self::Modified,
        }
    }
}

// ========================================================================
// GitMarks Implementations
// ========================================================================

impl GitMarks {
    /// Whether git named nothing under the listing: no checkout, or a clean
    /// one.
    pub fn is_empty(&self) -> bool {
        self.marks.is_empty() && self.whole.is_empty()
    }

    /// The git state of `path`: its own, else that of a directory above it
    /// git reported whole. `None` for a clean path.
    pub fn get(&self, path: &Path) -> Option<GitStatus> {
        if let Some(status) = self.marks.get(path) {
            return Some(*status);
        }
        path.ancestors()
            .skip(1)
            .find_map(|dir| self.whole.get(dir).copied())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// `git mv` of `sources` to `to`, run in `dir`: a new name for one, a
/// directory for several. Every path comes after `--end-of-options`, so one
/// starting with a dash is still a path.
pub fn move_request(dir: &Path, sources: &[PathBuf], to: &str) -> CommandRequest {
    let git = sources.iter().fold(Git::new(["mv"]), |git, source| {
        git.operand(&source.display().to_string())
    });
    git.operand(to).command(dir, TAG_ACTION)
}

/// `git add -f` of `paths`, run in `dir`: added though `.gitignore` leaves
/// them out.
pub fn force_add_request(dir: &Path, paths: &[PathBuf]) -> CommandRequest {
    let paths: Vec<String> = paths
        .iter()
        .map(|path| path.display().to_string())
        .collect();
    Git::new(["add", "-f"])
        .paths(&Pathspec::all(&paths))
        .command(dir, TAG_ACTION)
}

/// The command that finds the repository holding `dir`: its root, then
/// git's own directory, one a line (see [`parse_root`]).
pub fn root_request(dir: &Path) -> CommandRequest {
    Git::new(["rev-parse", "--show-toplevel", "--absolute-git-dir"]).command(dir, TAG_ROOT)
}

/// The repository's root and git's own directory, from what
/// [`root_request`] printed: separate for a worktree, whose git directory is
/// in the main repository's.
pub fn parse_root(stdout: &str) -> Option<(PathBuf, PathBuf)> {
    let mut lines = stdout
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty());
    let root = PathBuf::from(lines.next()?);
    let git_dir = PathBuf::from(lines.next()?);
    Some((root, git_dir))
}

/// The command that lists what has changed in the repository holding `dir`,
/// through the same builder the git view uses: paths come back whole, and
/// looking never rewrites the index, which a listing re-reading on every
/// change would otherwise do under whatever git the reader is running. An
/// untracked or ignored directory comes back as one record rather than every
/// file in it, which in a build directory would be thousands.
pub fn status_request(dir: &Path) -> CommandRequest {
    Git::new([
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "-z",
        "--untracked-files=normal",
        "--ignored=matching",
    ])
    .command(dir, TAG_STATUS)
}

/// The marks for every path under `top`, read from porcelain v2 status
/// output. A path changed in more than one way shows the most pressing way,
/// and each directory above a change shows the most pressing change within.
/// What git ignores is marked as such, and does not mark the directories
/// above it.
pub fn marks(top: &Path, status: &str) -> GitMarks {
    let mut found = GitMarks::default();
    for file in parse_status(status).files {
        let state = GitStatus::of_change(file.section, file.code);
        let whole = file.path.ends_with('/');
        let path = top.join(file.path.trim_end_matches('/'));
        if whole {
            found.whole.insert(path.clone(), state);
        }
        for at in path
            .ancestors()
            .take_while(|at| at.starts_with(top) && *at != top)
        {
            let held = found.marks.entry(at.to_path_buf()).or_insert(state);
            *held = (*held).min(state);
        }
    }
    for ignored in status
        .split(RECORD_END)
        .filter_map(|record| record.strip_prefix(IGNORED_RECORD))
    {
        let path = top.join(ignored.trim_end_matches('/'));
        if ignored.ends_with('/') {
            found.whole.insert(path.clone(), GitStatus::Ignored);
        }
        found.marks.insert(path, GitStatus::Ignored);
    }
    found
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_listings_status_read_rewrites_no_index_and_quotes_no_path() {
        // A listing re-reads on every change; with optional locks it would
        // take `index.lock` under whatever git the reader runs meanwhile.
        let request = status_request(Path::new("/repo"));
        assert!(request.args.contains(&"--no-optional-locks".to_string()));
        assert!(request.args.contains(&"-z".to_string()));
        assert!(request
            .env
            .contains(&("GIT_OPTIONAL_LOCKS".to_string(), "0".to_string())));
        let marks = marks(
            Path::new("/repo"),
            "1 .M N... 100644 100644 100644 a b caf\u{e9}.rs\0",
        );
        assert_eq!(
            marks.get(Path::new("/repo/caf\u{e9}.rs")),
            Some(GitStatus::Modified)
        );
    }

    #[test]
    fn test_a_change_marks_its_file_and_each_directory_with_the_most_pressing_within() {
        let top = Path::new("/repo");
        let status = "\
# branch.head main\0\
1 MM N... 100644 100644 100644 abc def src/lib.rs\0\
1 A. N... 000000 100644 100644 000 abc src/new.rs\0\
? docs/notes.txt\0\
u UU N... 100644 100644 100644 100644 a b c src/deep/merge.rs\0";
        let marks = marks(top, status);
        let at = |path: &str| marks.get(&top.join(path));
        assert_eq!(at("src/lib.rs"), Some(GitStatus::Modified));
        assert_eq!(at("src/new.rs"), Some(GitStatus::Added));
        assert_eq!(at("docs/notes.txt"), Some(GitStatus::Untracked));
        assert_eq!(
            at("docs"),
            Some(GitStatus::Untracked),
            "a directory of new files"
        );
        assert_eq!(
            at("src"),
            Some(GitStatus::Conflict),
            "the conflict deep inside wins"
        );
        assert_eq!(at("README.md"), None, "a clean file has no mark");
        assert_eq!(at(""), None, "the root itself is not marked");
    }

    #[test]
    fn test_what_lies_in_a_directory_reported_whole_takes_its_state() {
        // Untracked and ignored directories come back as one `dir/` record,
        // so a file listed inside one has no record of its own.
        let top = Path::new("/repo");
        let status = "? scratch/\0! target/\0! .env\0";
        let marks = marks(top, status);
        let at = |path: &str| marks.get(&top.join(path));
        assert_eq!(at("scratch/a/b.txt"), Some(GitStatus::Untracked));
        assert_eq!(at("target"), Some(GitStatus::Ignored));
        assert_eq!(at("target/debug/winter"), Some(GitStatus::Ignored));
        assert_eq!(at(".env"), Some(GitStatus::Ignored));
        assert_eq!(at("src/lib.rs"), None);
    }
}

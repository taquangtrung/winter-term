//! What git says about the entries listed: one letter beside each changed
//! file, and a dot on each directory holding one, so a listing of a checkout
//! shows where the work is without opening the git view.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::model::page::{CommandRequest, PageStyle};
use crate::tools::git::parse::{parse_status, Section};

// ========================================================================
// Constants
// ========================================================================

/// The tag of the first command a refresh runs: where the repository's root
/// is, since git reports paths against it.
pub const TAG_ROOT: &str = "dir-git-root";

/// The tag of the second: what has changed.
const TAG_STATUS: &str = "dir-git-status";

/// What a directory holding changes wears.
const DIRECTORY_MARK: char = '\u{2022}';

// ========================================================================
// Data Structures
// ========================================================================

/// One entry's git state, as the listing marks it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GitMark {
    /// The letter, or the dot of a directory holding changes.
    pub glyph: char,
    /// How it is painted, in the colors the git view uses for the same state.
    pub style: PageStyle,
}

// ========================================================================
// Functions
// ========================================================================

/// The command that finds the repository holding `dir`.
pub fn root_request(dir: &Path) -> CommandRequest {
    git_request(dir, &["rev-parse", "--show-toplevel"], TAG_ROOT)
}

/// The command that lists what has changed in the repository holding `dir`.
pub fn status_request(dir: &Path) -> CommandRequest {
    git_request(
        dir,
        &["status", "--porcelain=v2", "--untracked-files=normal"],
        TAG_STATUS,
    )
}

/// The marks for every changed path under `top`, read from porcelain v2
/// status output, with each directory above a change marked too. A path
/// changed in more than one way shows the most pressing: a conflict, then a
/// change not yet staged, then a staged one, then an untracked file.
pub fn marks(top: &Path, status: &str) -> HashMap<PathBuf, GitMark> {
    let mut marks: HashMap<PathBuf, (u8, GitMark)> = HashMap::new();
    for file in parse_status(status).files {
        let (rank, style) = match file.section {
            Section::Unmerged => (0, PageStyle::ChangeConflict),
            Section::Unstaged => (1, style_of(file.code)),
            Section::Staged => (2, style_of(file.code)),
            Section::Untracked => (3, PageStyle::HeadingUntracked),
        };
        let path = top.join(file.path.trim_end_matches('/'));
        let mark = GitMark {
            glyph: file.code,
            style,
        };
        let better = marks.get(&path).is_none_or(|(held, _)| rank < *held);
        if better {
            marks.insert(path.clone(), (rank, mark));
        }
        for dir in path.ancestors().skip(1).take_while(|dir| dir.starts_with(top) && *dir != top) {
            marks.entry(dir.to_path_buf()).or_insert((
                u8::MAX,
                GitMark {
                    glyph: DIRECTORY_MARK,
                    style: PageStyle::Dim,
                },
            ));
        }
    }
    marks.into_iter().map(|(path, (_, mark))| (path, mark)).collect()
}

fn git_request(dir: &Path, args: &[&str], tag: &'static str) -> CommandRequest {
    CommandRequest {
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: dir.to_path_buf(),
        env: Vec::new(),
        program: "git".to_string(),
        stdin: None,
        tag,
    }
}

/// The color the git view gives a change of kind `code`.
fn style_of(code: char) -> PageStyle {
    match code {
        'A' => PageStyle::ChangeAdded,
        'D' => PageStyle::ChangeDeleted,
        'R' | 'C' => PageStyle::ChangeRenamed,
        _ => PageStyle::ChangeModified,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_change_marks_its_file_with_the_most_pressing_state_and_its_directories() {
        let top = Path::new("/repo");
        let status = "\
# branch.head main
1 MM N... 100644 100644 100644 abc def src/lib.rs
1 A. N... 000000 100644 100644 000 abc src/new.rs
? notes.txt
";
        let marks = marks(top, status);
        let glyph = |path: &str| marks.get(&top.join(path)).map(|mark| mark.glyph);
        assert_eq!(glyph("src/lib.rs"), Some('M'), "the unstaged change wins");
        assert_eq!(glyph("src/new.rs"), Some('A'));
        assert_eq!(glyph("notes.txt"), Some('?'));
        assert_eq!(glyph("src"), Some(DIRECTORY_MARK));
        assert_eq!(glyph(""), None, "the root itself is not marked");
    }
}

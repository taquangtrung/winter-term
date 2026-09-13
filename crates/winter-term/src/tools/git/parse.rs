//! Reading git's own output: the porcelain status format, and a commit log.

// ========================================================================
// Constants
// ========================================================================

/// The code porcelain v2 uses where a file is unchanged on one side.
const UNCHANGED: char = '.';

// ========================================================================
// Data Structures
// ========================================================================

/// What `git status --porcelain=v2 --branch` reported.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Status {
    /// Commits the branch is ahead of its upstream.
    pub ahead: usize,
    /// Commits the branch is behind its upstream.
    pub behind: usize,
    /// The branch checked out, absent on a detached head.
    pub branch: Option<String>,
    /// Every reported path.
    pub files: Vec<FileStatus>,
    /// The upstream branch being tracked, if any.
    pub upstream: Option<String>,
}

/// One path's state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FileStatus {
    /// Where the path shows up in the view.
    pub section: Section,
    /// The path, relative to the repository root.
    pub path: String,
    /// The path this one was renamed from, when it was.
    pub renamed_from: Option<String>,
    /// The one-letter code for how the path changed.
    pub code: char,
}

/// Which part of the status view a path belongs to.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Section {
    /// Merge conflicts, first because nothing else can proceed past them.
    Unmerged,
    /// Files git does not track.
    Untracked,
    /// Changes in the working tree.
    Unstaged,
    /// Changes in the index.
    Staged,
}

/// One line of `git log --oneline`-shaped output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    /// The abbreviated hash.
    pub hash: String,
    /// The subject line.
    pub subject: String,
}

// ========================================================================
// Section
// ========================================================================

impl Section {
    /// The heading this section is drawn under.
    pub fn title(self) -> &'static str {
        match self {
            Section::Staged => "Staged changes",
            Section::Unmerged => "Unmerged paths",
            Section::Unstaged => "Unstaged changes",
            Section::Untracked => "Untracked files",
        }
    }

    /// Every section, in the order the view lays them out.
    pub fn all() -> [Section; 4] {
        [
            Section::Unmerged,
            Section::Untracked,
            Section::Unstaged,
            Section::Staged,
        ]
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Parse `git status --porcelain=v2 --branch -z`-shaped output, taking it
/// newline-separated. A path changed both in the index and in the working tree
/// is reported in both sections, which is what lets one be staged without the
/// other.
pub fn parse_status(text: &str) -> Status {
    let mut status = Status::default();
    for line in text.lines().filter(|line| !line.is_empty()) {
        match line.split_once(' ') {
            Some(("#", rest)) => read_header(&mut status, rest),
            Some(("1", rest)) => read_changed(&mut status, rest, false),
            Some(("2", rest)) => read_changed(&mut status, rest, true),
            Some(("u", rest)) => read_unmerged(&mut status, rest),
            Some(("?", path)) => status.files.push(FileStatus {
                section: Section::Untracked,
                path: path.to_string(),
                renamed_from: None,
                code: '?',
            }),
            // `!` is an ignored path, which the view does not list, and
            // anything else is a format this parser does not know.
            Some(_) | None => {}
        }
    }
    status
        .files
        .sort_by(|a, b| a.section.cmp(&b.section).then_with(|| a.path.cmp(&b.path)));
    status
}

/// Parse lines of `<hash> <subject>`, as `git log --oneline` writes them.
pub fn parse_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| match line.split_once(' ') {
            Some((hash, subject)) => Commit {
                hash: hash.to_string(),
                subject: subject.to_string(),
            },
            None => Commit {
                hash: line.to_string(),
                subject: String::new(),
            },
        })
        .collect()
}

fn read_header(status: &mut Status, rest: &str) {
    let Some((key, value)) = rest.split_once(' ') else {
        return;
    };
    match key {
        "branch.head" => {
            // A detached head reports the literal `(detached)`, which is a
            // state rather than a branch name.
            status.branch = (value != "(detached)").then(|| value.to_string());
        }
        "branch.upstream" => status.upstream = Some(value.to_string()),
        "branch.ab" => {
            let (ahead, behind) = read_ahead_behind(value);
            status.ahead = ahead;
            status.behind = behind;
        }
        _ => {}
    }
}

/// Read `+1 -2` into the pair it means.
fn read_ahead_behind(value: &str) -> (usize, usize) {
    let mut ahead = 0;
    let mut behind = 0;
    for field in value.split_whitespace() {
        let (sign, count) = field.split_at(1);
        let count = count.parse().unwrap_or(0);
        match sign {
            "+" => ahead = count,
            "-" => behind = count,
            _ => {}
        }
    }
    (ahead, behind)
}

/// Read a `1` (changed) or `2` (renamed) entry. Both start with the two status
/// codes; a renamed entry ends with the old path after a tab.
fn read_changed(status: &mut Status, rest: &str, renamed: bool) {
    let mut fields = rest.splitn(if renamed { 9 } else { 8 }, ' ');
    let Some(codes) = fields.next() else {
        return;
    };
    let Some(tail) = fields.last() else {
        return;
    };
    let (path, renamed_from) = if renamed {
        match tail.split_once('\t') {
            Some((path, from)) => (path.to_string(), Some(from.to_string())),
            None => (tail.to_string(), None),
        }
    } else {
        (tail.to_string(), None)
    };
    let mut codes = codes.chars();
    let staged = codes.next().unwrap_or(UNCHANGED);
    let unstaged = codes.next().unwrap_or(UNCHANGED);
    if staged != UNCHANGED {
        status.files.push(FileStatus {
            section: Section::Staged,
            path: path.clone(),
            renamed_from: renamed_from.clone(),
            code: staged,
        });
    }
    if unstaged != UNCHANGED {
        status.files.push(FileStatus {
            section: Section::Unstaged,
            path,
            renamed_from,
            code: unstaged,
        });
    }
}

fn read_unmerged(status: &mut Status, rest: &str) {
    let mut fields = rest.splitn(11, ' ');
    let Some(codes) = fields.next() else {
        return;
    };
    let Some(path) = fields.last() else {
        return;
    };
    status.files.push(FileStatus {
        section: Section::Unmerged,
        path: path.to_string(),
        renamed_from: None,
        code: codes.chars().next().unwrap_or('U'),
    });
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS: &str = "\
# branch.oid 8ea4e307c0f
# branch.head main
# branch.upstream origin/main
# branch.ab +2 -1
1 M. N... 100644 100644 100644 aaa bbb staged-only.rs
1 .M N... 100644 100644 100644 aaa bbb unstaged-only.rs
1 MM N... 100644 100644 100644 aaa bbb both.rs
2 R. N... 100644 100644 100644 aaa bbb R100 new-name.rs\told-name.rs
u UU N... 100644 100644 100644 100644 aaa bbb ccc conflicted.rs
? untracked.rs
! ignored.rs
";

    fn paths(status: &Status, section: Section) -> Vec<&str> {
        status
            .files
            .iter()
            .filter(|f| f.section == section)
            .map(|f| f.path.as_str())
            .collect()
    }

    #[test]
    fn test_branch_and_tracking_come_off_the_header() {
        let status = parse_status(STATUS);
        assert_eq!(status.branch.as_deref(), Some("main"));
        assert_eq!(status.upstream.as_deref(), Some("origin/main"));
        assert_eq!((status.ahead, status.behind), (2, 1));
    }

    #[test]
    fn test_a_detached_head_has_no_branch_name() {
        // `(detached)` is a state, and treating it as a branch would put it in
        // a push command.
        let status = parse_status("# branch.head (detached)\n");
        assert_eq!(status.branch, None);
    }

    #[test]
    fn test_a_file_changed_on_both_sides_appears_in_both_sections() {
        // This is the case the whole staging view exists for: staging the
        // indexed half must not imply the working-tree half.
        let status = parse_status(STATUS);
        assert!(paths(&status, Section::Staged).contains(&"both.rs"));
        assert!(paths(&status, Section::Unstaged).contains(&"both.rs"));
    }

    #[test]
    fn test_each_side_only_reports_what_actually_changed_there() {
        let status = parse_status(STATUS);
        assert!(!paths(&status, Section::Unstaged).contains(&"staged-only.rs"));
        assert!(!paths(&status, Section::Staged).contains(&"unstaged-only.rs"));
    }

    #[test]
    fn test_a_rename_reports_both_names() {
        let status = parse_status(STATUS);
        let renamed = status
            .files
            .iter()
            .find(|f| f.path == "new-name.rs")
            .expect("the renamed path");
        assert_eq!(renamed.renamed_from.as_deref(), Some("old-name.rs"));
        assert_eq!(renamed.code, 'R');
    }

    #[test]
    fn test_untracked_and_unmerged_paths_land_in_their_own_sections() {
        let status = parse_status(STATUS);
        assert_eq!(paths(&status, Section::Untracked), ["untracked.rs"]);
        assert_eq!(paths(&status, Section::Unmerged), ["conflicted.rs"]);
    }

    #[test]
    fn test_ignored_paths_are_not_listed() {
        let status = parse_status(STATUS);
        assert!(!status.files.iter().any(|f| f.path == "ignored.rs"));
    }

    #[test]
    fn test_a_path_with_spaces_survives_the_field_split() {
        // The path is the last field and can hold spaces, so splitting on
        // every space would truncate it.
        let line = "1 .M N... 100644 100644 100644 aaa bbb my notes file.md\n";
        let status = parse_status(line);
        assert_eq!(paths(&status, Section::Unstaged), ["my notes file.md"]);
    }

    #[test]
    fn test_empty_output_is_a_clean_tree_not_an_error() {
        assert_eq!(parse_status(""), Status::default());
    }

    #[test]
    fn test_log_lines_split_hash_from_subject() {
        let commits = parse_log("8ea4e30 add tree motions\n2f6e30a draw icons\n");
        assert_eq!(commits.len(), 2);
        assert_eq!(commits[0].hash, "8ea4e30");
        assert_eq!(commits[1].subject, "draw icons");
    }

    #[test]
    fn test_a_subject_holding_spaces_is_kept_whole() {
        let commits = parse_log("abc1234 fix: keep the whole subject line\n");
        assert_eq!(commits[0].subject, "fix: keep the whole subject line");
    }
}

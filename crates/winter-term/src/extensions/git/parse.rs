//! Reading git's own output: the porcelain status format, and a commit log.

// ========================================================================
// Constants
// ========================================================================

/// The code porcelain v2 uses where a file is unchanged on one side.
const UNCHANGED: char = '.';

/// Field separator in the decorated log format, matching the `%x1f` the log
/// request asks git for. US is chosen because it cannot occur in a hash, a
/// decoration, an author name, a timestamp, or a commit subject.
pub(super) const FIELD_SEP: char = '\x1f';

/// How git spells the checked-out branch in a `%d` decoration.
const HEAD_ARROW: &str = "->";

/// How git marks a tag in a `%d` decoration.
const TAG_MARK: &str = "tag:";

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

/// One entry of the stash list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Stash {
    /// What git calls it, e.g. `stash@{0}`, which is also what a command
    /// acting on it takes.
    pub name: String,
    /// The message it was pushed with.
    pub subject: String,
}

/// One commit of decorated log output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Commit {
    /// The abbreviated hash.
    pub hash: String,
    /// The subject line.
    pub subject: String,
    /// Who wrote it.
    pub author: String,
    /// When it was authored, in seconds since the Unix epoch. `0` when the log
    /// carried no timestamp, which reads as "no age known" rather than 1970.
    pub time: i64,
    /// The refs pointing at it, HEAD first.
    pub refs: Vec<Ref>,
}

/// What kind of thing a ref pointing at a commit is, which is what decides how
/// loudly it is drawn.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RefKind {
    /// The checked-out branch, or a bare detached `HEAD`.
    Head,
    /// A local branch that is not the checked-out one.
    Local,
    /// A branch on a remote.
    Remote,
    /// A tag.
    Tag,
}

/// One ref pointing at a commit.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Ref {
    /// Which kind of ref it is.
    pub kind: RefKind,
    /// Its name, without the `tag: ` marker git decorates tags with.
    pub name: String,
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

/// Parse the decorated log [`crate::tools::git::exec`] asks for: one commit per
/// line, fields separated by US (`\x1f`) as `hash·decoration·author·time·subject`.
///
/// A unit separator rather than git's own bracket decoration, because a subject
/// is arbitrary text: it can hold `[`, `]`, `(` and `)`, and a bracket-matching
/// parser mis-reads those as fields. US cannot appear in any of these fields, so
/// splitting on it needs no escaping and cannot be fooled by the commit message.
pub fn parse_decorated_log(text: &str) -> Vec<Commit> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(parse_decorated_commit)
        .collect()
}

/// The stash list, newest first, as git wrote it: name and message per line,
/// separated by the same unit separator the log format uses.
pub fn parse_stash_list(text: &str) -> Vec<Stash> {
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            let (name, subject) = line.split_once(FIELD_SEP).unwrap_or((line, ""));
            Stash {
                name: name.trim().to_string(),
                subject: subject.trim().to_string(),
            }
        })
        .collect()
}

/// One decorated log line as a [`Commit`]. A line missing later fields still
/// yields the fields it has, so a log format change degrades to less detail
/// rather than to no commits at all.
fn parse_decorated_commit(line: &str) -> Commit {
    let mut fields = line.split(FIELD_SEP);
    let hash = fields.next().unwrap_or_default().trim().to_string();
    let decoration = fields.next().unwrap_or_default();
    let author = fields.next().unwrap_or_default().trim().to_string();
    let time = fields
        .next()
        .unwrap_or_default()
        .trim()
        .parse::<i64>()
        .unwrap_or(0);
    // The subject is the remainder, not just the next field, so a subject that
    // somehow holds the separator keeps all of itself.
    let subject = fields.collect::<Vec<&str>>().join(&FIELD_SEP.to_string());
    Commit {
        hash,
        subject: subject.trim_end().to_string(),
        author,
        time,
        refs: classify_refs(decoration),
    }
}

/// The refs in a `%d` decoration, HEAD first.
///
/// git writes it as ` (HEAD -> main, origin/main, tag: v1.0)`, empty when
/// nothing points at the commit. HEAD is hoisted to the front because it is the
/// one ref a reader scans for, and git only happens to place it first.
pub fn classify_refs(decoration: &str) -> Vec<Ref> {
    let trimmed = decoration.trim();
    let inner = trimmed
        .strip_prefix('(')
        .and_then(|rest| rest.strip_suffix(')'))
        .unwrap_or("");
    let mut refs: Vec<Ref> = Vec::new();
    for token in inner.split(',') {
        let token = token.trim();
        if token.is_empty() {
            continue;
        }
        refs.push(classify_ref(token));
    }
    // HEAD first, order among the rest left as git reported it.
    refs.sort_by_key(|entry| entry.kind != RefKind::Head);
    refs
}

/// One decoration token as a [`Ref`].
fn classify_ref(token: &str) -> Ref {
    // `HEAD -> main`: the checked-out branch. Named for the branch, not for
    // HEAD, since the branch is what the reader is looking for.
    if let Some((_, branch)) = token.split_once(HEAD_ARROW) {
        return Ref {
            kind: RefKind::Head,
            name: branch.trim().to_string(),
        };
    }
    if let Some(tag) = token.strip_prefix(TAG_MARK) {
        return Ref {
            kind: RefKind::Tag,
            name: tag.trim().to_string(),
        };
    }
    // A detached HEAD points at a commit with no branch to name it.
    if token == "HEAD" {
        return Ref {
            kind: RefKind::Head,
            name: token.to_string(),
        };
    }
    // A remote-tracking branch is the only ref shaped `remote/name`. A local
    // branch may hold a slash too (`feature/x`), so this is a heuristic; git
    // gives no way to tell them apart in a decoration.
    let kind = if token.contains('/') {
        RefKind::Remote
    } else {
        RefKind::Local
    };
    Ref {
        kind,
        name: token.to_string(),
    }
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

    /// A decorated log line, assembled the way the log format writes it.
    fn log_line(hash: &str, decoration: &str, author: &str, time: &str, subject: &str) -> String {
        [hash, decoration, author, time, subject].join(&FIELD_SEP.to_string())
    }

    #[test]
    fn test_a_decorated_log_line_yields_every_field() {
        let text = log_line(
            "8ea4e30",
            " (HEAD -> main, origin/main)",
            "Jane Doe",
            "1700000000",
            "add tree motions",
        );
        let commits = parse_decorated_log(&text);
        assert_eq!(commits.len(), 1);
        let commit = &commits[0];
        assert_eq!(commit.hash, "8ea4e30");
        assert_eq!(commit.author, "Jane Doe");
        assert_eq!(commit.time, 1_700_000_000);
        assert_eq!(commit.subject, "add tree motions");
        assert_eq!(
            commit.refs,
            vec![
                Ref {
                    kind: RefKind::Head,
                    name: "main".to_string()
                },
                Ref {
                    kind: RefKind::Remote,
                    name: "origin/main".to_string()
                },
            ]
        );
    }

    #[test]
    fn test_a_subject_holding_spaces_and_brackets_is_kept_whole() {
        // The separator is why this works: a bracket-matching parser would read
        // `[skip ci]` as the author field.
        let subject = "fix: keep [skip ci] (and parens) whole";
        let commits = parse_decorated_log(&log_line("abc1234", "", "A", "1", subject));
        assert_eq!(commits[0].subject, subject);
        assert_eq!(commits[0].author, "A");
    }

    #[test]
    fn test_an_undecorated_commit_has_no_refs() {
        let commits = parse_decorated_log(&log_line("abc1234", "", "A", "1", "plain"));
        assert!(commits[0].refs.is_empty());
    }

    #[test]
    fn test_head_is_hoisted_ahead_of_the_refs_git_listed_first() {
        // git puts tags before HEAD here; the view wants HEAD first.
        let refs = classify_refs(" (tag: v1.0, HEAD -> main)");
        assert_eq!(refs[0].kind, RefKind::Head);
        assert_eq!(refs[0].name, "main");
        assert_eq!(refs[1].kind, RefKind::Tag);
        assert_eq!(refs[1].name, "v1.0");
    }

    #[test]
    fn test_each_kind_of_ref_is_told_apart() {
        let refs = classify_refs(" (HEAD -> main, feature, origin/main, tag: v2)");
        let kinds: Vec<RefKind> = refs.iter().map(|entry| entry.kind).collect();
        assert_eq!(
            kinds,
            vec![RefKind::Head, RefKind::Local, RefKind::Remote, RefKind::Tag]
        );
    }

    #[test]
    fn test_a_detached_head_is_named_head() {
        let refs = classify_refs(" (HEAD)");
        assert_eq!(refs[0].kind, RefKind::Head);
        assert_eq!(refs[0].name, "HEAD");
    }

    #[test]
    fn test_a_missing_timestamp_reads_as_no_age_rather_than_the_epoch() {
        // A truncated line still yields the fields it has.
        let commits = parse_decorated_log("abc1234");
        assert_eq!(commits[0].hash, "abc1234");
        assert_eq!(commits[0].time, 0);
        assert_eq!(commits[0].subject, "");
    }

    #[test]
    fn test_empty_log_output_is_no_commits() {
        assert!(parse_decorated_log("").is_empty());
        assert!(parse_decorated_log("\n\n").is_empty());
    }
}

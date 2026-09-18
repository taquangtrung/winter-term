//! The git command lines the view runs, as argument lists rather than strings,
//! so nothing goes through a shell and a path holding a space stays one path.

use std::path::{Path, PathBuf};

use crate::model::page::{CommandRequest, JobRequest};

// ========================================================================
// Constants
// ========================================================================

/// The program every request here runs.
const GIT: &str = "git";

/// Tag naming a commit request, so a reply says which command finished.
pub const TAG_COMMIT: &str = "commit";

/// Tag naming anything that changed the repository, which the view answers by
/// re-reading it.
pub const TAG_CHANGED: &str = "changed";

/// Tag naming a command whose output is the answer, shown as it came back.
pub const TAG_READ: &str = "read";

/// Tag naming a log read that replaces the log view.
pub const TAG_LOG_VIEW: &str = "log-view";

/// Tag naming a remote-URL lookup, for opening it in a browser.
pub const TAG_REMOTE_URL: &str = "remote-url";

/// Tag naming a blame read, which fills the view under its own title.
pub const TAG_BLAME: &str = "blame";

/// Tag naming a diff read.
pub const TAG_DIFF: &str = "diff";

/// Tag naming a whole-diff read, which fills the view as a decorated diff
/// rather than output shown as it came back.
pub const TAG_DIFF_VIEW: &str = "diff-view";

/// Tag naming a discard request.
pub const TAG_DISCARD: &str = "discard";

/// Tag naming one commit's details, which fill the view under its own title.
pub const TAG_SHOW: &str = "show";

/// Tag naming a fetch request.
pub const TAG_FETCH: &str = "fetch";

/// Tag naming a log request.
pub const TAG_LOG: &str = "log";

/// Tag naming a pull request.
pub const TAG_PULL: &str = "pull";

/// Tag naming a push request.
pub const TAG_PUSH: &str = "push";

/// Tag naming the repository-root lookup.
pub const TAG_ROOT: &str = "root";

/// Tag naming a stage request.
pub const TAG_STAGE: &str = "stage";

/// Tag naming a status request.
pub const TAG_STATUS: &str = "status";

/// Tag naming an unstage request.
pub const TAG_UNSTAGE: &str = "unstage";

/// How far a reset moves, and what it keeps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ResetMode {
    /// Throw away the index and the working tree.
    Hard,
    /// Keep the working tree, reset the index.
    Mixed,
    /// Keep both; only the branch pointer moves.
    Soft,
}

impl ResetMode {
    fn flag(self) -> &'static str {
        match self {
            ResetMode::Hard => "--hard",
            ResetMode::Mixed => "--mixed",
            ResetMode::Soft => "--soft",
        }
    }
}

/// Whether a sequence in progress should carry on or stop.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SequenceStep {
    /// Give up and put the repository back.
    Abort,
    /// Carry on with the next commit.
    Continue,
    /// Leave this commit out and carry on.
    Skip,
}

impl SequenceStep {
    fn flag(self) -> &'static str {
        match self {
            SequenceStep::Abort => "--abort",
            SequenceStep::Continue => "--continue",
            SequenceStep::Skip => "--skip",
        }
    }
}

/// What a log read covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogScope {
    /// Every ref, not just this branch.
    AllRefs,
    /// The current branch.
    Branch,
    /// One path's history.
    File(String),
}

/// Where a patch is applied, and in which direction.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ApplyTarget {
    /// Add the change to the index: staging a hunk.
    Index,
    /// Take the change out of the index: unstaging a hunk.
    IndexReverse,
    /// Take the change out of the working tree: discarding a hunk.
    WorktreeReverse,
}

impl ApplyTarget {
    /// The tag the reply carries, so the view reports the right verb.
    pub fn tag(self) -> &'static str {
        match self {
            ApplyTarget::Index => TAG_STAGE,
            ApplyTarget::IndexReverse => TAG_UNSTAGE,
            ApplyTarget::WorktreeReverse => TAG_DISCARD,
        }
    }
}

/// How many recent commits the status view lists.
const RECENT_COMMITS: usize = 10;

// ========================================================================
// Functions
// ========================================================================

/// Ask where the repository containing `cwd` is rooted. Every other command
/// runs from there, so a listing opened deep in a tree still stages by
/// repository-relative path.
pub fn repo_root(cwd: &Path) -> JobRequest {
    request(cwd, TAG_ROOT, ["rev-parse", "--show-toplevel"])
}

/// The working tree's state, with branch and tracking information.
pub fn status(root: &Path) -> JobRequest {
    request(
        root,
        TAG_STATUS,
        [
            "status",
            "--porcelain=v2",
            "--branch",
            "--untracked-files=all",
        ],
    )
}

/// The log format both log requests ask for: abbreviated hash, ref decoration,
/// author, author time, and subject, separated by US (`%x1f`).
///
/// A separator rather than git's own spacing because every field after the hash
/// is arbitrary text. `--date-order` is not set: the default reverse-chronology
/// is what a "recent commits" tail means.
const LOG_FORMAT: &str = "--format=%h%x1f%d%x1f%an%x1f%at%x1f%s";

/// The most recent commits, for the view's own tail.
pub fn recent_log(root: &Path) -> JobRequest {
    request(
        root,
        TAG_LOG,
        [
            "log",
            LOG_FORMAT,
            "--decorate=short",
            &format!("--max-count={RECENT_COMMITS}"),
        ],
    )
}

/// Stage `paths`. An empty list stages everything, which is what the
/// stage-all key asks for.
pub fn stage(root: &Path, paths: &[String]) -> JobRequest {
    let mut args = vec!["add".to_string(), "--".to_string()];
    if paths.is_empty() {
        args.push(".".to_string());
    } else {
        args.extend(paths.iter().cloned());
    }
    owned_request(root, TAG_STAGE, args)
}

/// Stage every tracked change, the way Magic's stage-all does outside the
/// untracked section. `-u` is what keeps untracked files out: those are
/// staged only by name, so nothing lands in the index unread.
pub fn stage_all_tracked(root: &Path) -> JobRequest {
    request(root, TAG_STAGE, ["add", "-u"])
}

/// Unstage `paths`, leaving the working tree alone. An empty list unstages
/// everything.
pub fn unstage(root: &Path, paths: &[String]) -> JobRequest {
    let mut args = vec!["restore".to_string(), "--staged".to_string()];
    args.push("--".to_string());
    if paths.is_empty() {
        args.push(".".to_string());
    } else {
        args.extend(paths.iter().cloned());
    }
    owned_request(root, TAG_UNSTAGE, args)
}

/// Throw away working-tree changes to `paths`. Nothing recovers this, which is
/// why the view confirms first.
pub fn discard(root: &Path, paths: &[String]) -> JobRequest {
    let mut args = vec!["restore".to_string(), "--worktree".to_string()];
    args.push("--".to_string());
    args.extend(paths.iter().cloned());
    owned_request(root, TAG_DISCARD, args)
}

/// Delete untracked `paths`, which `restore` cannot touch.
pub fn remove_untracked(root: &Path, paths: &[String]) -> JobRequest {
    let mut args = vec![
        "clean".to_string(),
        "--force".to_string(),
        "-d".to_string(),
        "--".to_string(),
    ];
    args.extend(paths.iter().cloned());
    owned_request(root, TAG_DISCARD, args)
}

/// Commit what is staged, with `message` as the whole message.
pub fn commit(root: &Path, message: &str) -> JobRequest {
    owned_request(
        root,
        TAG_COMMIT,
        vec![
            "commit".to_string(),
            "--message".to_string(),
            message.to_string(),
        ],
    )
}

/// The diff for one path, from the index when `staged` or from the working
/// tree otherwise. Context is trimmed to what a reviewer needs, and rename
/// detection is off so a hunk maps to one path.
pub fn diff_file(root: &Path, path: &str, staged: bool) -> JobRequest {
    let mut args = vec!["diff".to_string(), "--no-color".to_string()];
    if staged {
        args.push("--cached".to_string());
    }
    args.push("--no-ext-diff".to_string());
    args.push("--no-renames".to_string());
    args.push("--".to_string());
    args.push(path.to_string());
    owned_request(root, TAG_DIFF, args)
}

/// The diff for an untracked path, which git will only produce against the
/// empty tree.
pub fn diff_untracked(root: &Path, path: &str) -> JobRequest {
    owned_request(
        root,
        TAG_DIFF,
        vec![
            "diff".to_string(),
            "--no-color".to_string(),
            "--no-index".to_string(),
            "--".to_string(),
            "/dev/null".to_string(),
            path.to_string(),
        ],
    )
}

/// Apply `patch` to the index, or reverse it out of the index or the working
/// tree. This is what makes staging a single hunk possible: the patch is fed
/// on standard input rather than written to a file.
pub fn apply_patch(root: &Path, patch: String, target: ApplyTarget) -> JobRequest {
    let mut args = vec!["apply".to_string(), "--unidiff-zero".to_string()];
    match target {
        ApplyTarget::Index => args.push("--cached".to_string()),
        ApplyTarget::IndexReverse => {
            args.push("--cached".to_string());
            args.push("--reverse".to_string());
        }
        ApplyTarget::WorktreeReverse => args.push("--reverse".to_string()),
    }
    args.push("-".to_string());
    piped_request(root, target.tag(), args, Some(patch))
}

/// Push the current branch.
pub fn push(root: &Path) -> JobRequest {
    request(root, TAG_PUSH, ["push"])
}

/// Pull with rebase, which is what a status view's "pull" should mean: a merge
/// commit nobody asked for is worse than a refusal.
pub fn pull(root: &Path) -> JobRequest {
    request(root, TAG_PULL, ["pull", "--rebase"])
}

/// Fetch every remote.
pub fn fetch(root: &Path) -> JobRequest {
    request(root, TAG_FETCH, ["fetch", "--all", "--prune"])
}

/// Check out an existing branch or revision.
pub fn checkout(root: &Path, rev: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["checkout", rev])
}

/// Create a branch, checking it out when asked.
pub fn branch_create(root: &Path, name: &str, checkout: bool) -> JobRequest {
    if checkout {
        owned(root, TAG_CHANGED, vec!["checkout", "-b", name])
    } else {
        owned(root, TAG_CHANGED, vec!["branch", name])
    }
}

/// Delete a branch. `force` deletes one whose work is not merged.
pub fn branch_delete(root: &Path, name: &str, force: bool) -> JobRequest {
    let flag = if force { "-D" } else { "-d" };
    owned(root, TAG_CHANGED, vec!["branch", flag, name])
}

/// Merge a branch into the current one.
pub fn merge(root: &Path, rev: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["merge", "--no-edit", rev])
}

/// Continue or abandon whatever sequence is in progress. One command shape
/// covers merge, rebase, cherry-pick, and revert, which is why the verb is a
/// parameter rather than five near-identical functions.
pub fn sequence(root: &Path, verb: &str, step: SequenceStep) -> JobRequest {
    owned(root, TAG_CHANGED, vec![verb, step.flag()])
}

/// Replay commits onto `upstream`, in an editor when interactive.
pub fn rebase(root: &Path, upstream: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["rebase", upstream])
}

/// Stash everything, with an optional message.
pub fn stash_push(root: &Path, message: &str) -> JobRequest {
    let mut args = vec!["stash".to_string(), "push".to_string()];
    if !message.trim().is_empty() {
        args.push("--message".to_string());
        args.push(message.to_string());
    }
    owned_request(root, TAG_CHANGED, args)
}

/// Act on the newest stash entry.
pub fn stash(root: &Path, verb: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["stash", verb])
}

/// List the stash, for reading rather than changing anything.
pub fn stash_list(root: &Path) -> JobRequest {
    owned(root, TAG_READ, vec!["stash", "list"])
}

/// Create a tag at the current commit.
pub fn tag_create(root: &Path, name: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["tag", name])
}

/// Delete a tag.
pub fn tag_delete(root: &Path, name: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["tag", "--delete", name])
}

/// List tags.
pub fn tag_list(root: &Path) -> JobRequest {
    owned(root, TAG_READ, vec!["tag", "--list"])
}

/// List remotes with their URLs.
pub fn remote_list(root: &Path) -> JobRequest {
    owned(root, TAG_READ, vec!["remote", "--verbose"])
}

/// Add a remote, taking `name url` as one answer.
pub fn remote_add(root: &Path, name: &str, url: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["remote", "add", name, url])
}

/// Remove a remote.
pub fn remote_remove(root: &Path, name: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["remote", "remove", name])
}

/// Drop remote-tracking refs the remote no longer has.
pub fn remote_prune(root: &Path, name: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["remote", "prune", name])
}

/// The URL a remote fetches from, for opening it in a browser.
pub fn remote_url(root: &Path, name: &str) -> JobRequest {
    owned(root, TAG_REMOTE_URL, vec!["remote", "get-url", name])
}

/// Replay one commit onto the current branch.
pub fn cherry_pick(root: &Path, rev: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["cherry-pick", rev])
}

/// Undo a commit with a new one.
pub fn revert(root: &Path, rev: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["revert", "--no-edit", rev])
}

/// Move the branch pointer, keeping as much as `mode` says.
pub fn reset(root: &Path, mode: ResetMode, rev: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["reset", mode.flag(), rev])
}

/// Worktrees, listed.
pub fn worktree_list(root: &Path) -> JobRequest {
    owned(root, TAG_READ, vec!["worktree", "list"])
}

/// Add a worktree at `path`, on a new branch named after it.
pub fn worktree_add(root: &Path, path: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["worktree", "add", path])
}

/// Remove a worktree.
pub fn worktree_remove(root: &Path, path: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["worktree", "remove", path])
}

/// Start a bisect, or answer its question.
pub fn bisect(root: &Path, verb: &str) -> JobRequest {
    owned(root, TAG_CHANGED, vec!["bisect", verb])
}

/// The log for a path, or for every ref.
pub fn log(root: &Path, scope: LogScope, count: usize) -> JobRequest {
    let count = format!("--max-count={count}");
    let mut args = vec![
        "log".to_string(),
        LOG_FORMAT.to_string(),
        "--decorate=short".to_string(),
        count,
    ];
    match scope {
        LogScope::AllRefs => args.push("--all".to_string()),
        LogScope::Branch => {}
        LogScope::File(path) => {
            args.push("--".to_string());
            args.push(path);
        }
    }
    owned_request(root, TAG_LOG_VIEW, args)
}

/// Every ref, with what it points at.
pub fn show_refs(root: &Path) -> JobRequest {
    owned(root, TAG_READ, vec!["show-ref", "--abbrev"])
}

/// Who last touched each line of a path.
pub fn blame(root: &Path, path: &str) -> JobRequest {
    owned(
        root,
        TAG_BLAME,
        vec!["blame", "--date=short", "--abbrev=8", "--", path],
    )
}

/// One commit in full: its author, date and message, the files it touched, and
/// the patch itself.
///
/// No `--shortstat`: the view counts the commit's own hunks instead, which is
/// the unit it lets the reader open, where git's line totals count something
/// nothing in the view acts on.
pub fn show_commit(root: &Path, rev: &str) -> JobRequest {
    owned(
        root,
        TAG_SHOW,
        vec![
            "show",
            "--no-color",
            "--no-ext-diff",
            "--patch",
            "--date=iso",
            rev,
        ],
    )
}

/// A whole diff, for reading rather than staging.
pub fn diff_all(root: &Path, staged: bool, rev: Option<&str>) -> JobRequest {
    let mut args = vec!["diff".to_string(), "--no-color".to_string()];
    if staged {
        args.push("--cached".to_string());
    }
    if let Some(rev) = rev {
        args.push(rev.to_string());
    }
    owned_request(root, TAG_DIFF_VIEW, args)
}

/// Whatever the user typed, split on spaces: the escape hatch for the commands
/// this view does not offer a key for.
pub fn custom(root: &Path, line: &str) -> JobRequest {
    let args = line.split_whitespace().map(str::to_string).collect();
    owned_request(root, TAG_CHANGED, args)
}

fn owned(root: &Path, tag: &'static str, args: Vec<&str>) -> JobRequest {
    owned_request(root, tag, args.into_iter().map(str::to_string).collect())
}

fn request<const N: usize>(cwd: &Path, tag: &'static str, args: [&str; N]) -> JobRequest {
    owned_request(cwd, tag, args.iter().map(|a| a.to_string()).collect())
}

fn owned_request(cwd: &Path, tag: &'static str, args: Vec<String>) -> JobRequest {
    piped_request(cwd, tag, args, None)
}

fn piped_request(
    cwd: &Path,
    tag: &'static str,
    args: Vec<String>,
    stdin: Option<String>,
) -> JobRequest {
    JobRequest::Command(CommandRequest {
        args,
        cwd: PathBuf::from(cwd),
        program: GIT.to_string(),
        stdin,
        tag,
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn args_of(request: &JobRequest) -> Vec<String> {
        match request {
            JobRequest::Command(command) => command.args.clone(),
            JobRequest::DirSize(_) | JobRequest::Search(_) => panic!("expected a command"),
        }
    }

    #[test]
    fn test_staging_nothing_means_staging_everything() {
        // The stage-all key passes no paths, and `add --` with no pathspec is
        // a no-op rather than an error, which would silently do nothing.
        let all = args_of(&stage(Path::new("/repo"), &[]));
        assert_eq!(all, ["add", "--", "."]);
    }

    #[test]
    fn test_paths_are_passed_after_a_separator() {
        // Without `--`, a file named like an option is read as one.
        let one = args_of(&stage(Path::new("/repo"), &["--force".to_string()]));
        assert_eq!(one, ["add", "--", "--force"]);
    }

    #[test]
    fn test_stage_all_tracked_leaves_untracked_files_alone() {
        // `add .` would sweep untracked files into the index unread, which is
        // what the plain stage-everything key is for.
        let args = args_of(&stage_all_tracked(Path::new("/repo")));
        assert_eq!(args, ["add", "-u"]);
    }

    #[test]
    fn test_unstage_leaves_the_working_tree_alone() {
        // `reset` would be the usual reflex, but `restore --staged` cannot
        // touch the file on disk, which is the one thing unstaging must not do.
        let args = args_of(&unstage(Path::new("/repo"), &["a.rs".to_string()]));
        assert_eq!(args, ["restore", "--staged", "--", "a.rs"]);
    }

    #[test]
    fn test_discard_names_the_working_tree_explicitly() {
        let args = args_of(&discard(Path::new("/repo"), &["a.rs".to_string()]));
        assert_eq!(args, ["restore", "--worktree", "--", "a.rs"]);
    }

    #[test]
    fn test_a_message_is_one_argument_however_it_reads() {
        // Passed through a shell, a message holding quotes or newlines would
        // break the command apart.
        let args = args_of(&commit(Path::new("/repo"), "fix: don't \"quote\" me"));
        assert_eq!(args[2], "fix: don't \"quote\" me");
    }

    #[test]
    fn test_pull_rebases_rather_than_merging() {
        assert_eq!(args_of(&pull(Path::new("/repo"))), ["pull", "--rebase"]);
    }
}

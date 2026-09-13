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

/// Tag naming a discard request.
pub const TAG_DISCARD: &str = "discard";

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

/// The most recent commits, for the view's own tail.
pub fn recent_log(root: &Path) -> JobRequest {
    request(
        root,
        TAG_LOG,
        [
            "log",
            "--oneline",
            "--no-decorate",
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

fn request<const N: usize>(cwd: &Path, tag: &'static str, args: [&str; N]) -> JobRequest {
    owned_request(cwd, tag, args.iter().map(|a| a.to_string()).collect())
}

fn owned_request(cwd: &Path, tag: &'static str, args: Vec<String>) -> JobRequest {
    JobRequest::Command(CommandRequest {
        args,
        cwd: PathBuf::from(cwd),
        program: GIT.to_string(),
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
            JobRequest::DirSize(_) => panic!("expected a command"),
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

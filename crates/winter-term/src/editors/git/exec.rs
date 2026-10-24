//! The git command lines the view runs, as argument lists rather than strings,
//! so nothing goes through a shell and a path holding a space stays one path.
//!
//! Every line is put together by one builder, [`Git`], which is where the
//! rules that keep user text from being read as something else live: a
//! revision is a [`Rev`], which can never pass for an option, a path is a
//! [`Pathspec`], which always follows a `--` and is matched literally, and
//! names typed by the reader follow `--end-of-options`. The builder also sets
//! what every request needs: paths reported verbatim (`core.quotePath=false`),
//! no index rewrite behind the reader's back (`GIT_OPTIONAL_LOCKS=0`), output
//! in one locale (`LC_ALL=C`), and no credential prompt with no terminal to
//! answer it (`GIT_TERMINAL_PROMPT=0`).

use std::path::{Path, PathBuf};
use std::slice;

use crate::model::editor::{CommandOutput, CommandRequest, JobRequest};

// ========================================================================
// Constants
// ========================================================================

/// The program every request here runs.
const GIT: &str = "git";

/// The setting every request is run under, so a path holding a byte past
/// ASCII comes back as itself rather than as an octal escape in quotes.
const QUOTE_PATH_OFF: [&str; 2] = ["-c", "core.quotePath=false"];

/// How long a hash an interactive plan writes while a commit in it is being
/// reworded: a whole SHA-1 name, and as much of a SHA-256 one as is needed to
/// match it, whatever `core.abbrev` would shorten it to.
const PLAN_HASH_LEN: usize = 40;

/// The setting that makes a plan write hashes [`PLAN_HASH_LEN`] long.
const PLAN_ABBREV: &str = "core.abbrev=40";

/// What ends a command's options, so a name typed after it is never read as
/// one even when it starts with a dash.
const END_OF_OPTIONS: &str = "--end-of-options";

/// What ends a command's revisions, so a path after it is never read as one.
const END_OF_REVISIONS: &str = "--";

/// The pathspec naming the whole tree, for the keys that act on everything.
const EVERYTHING: &str = ".";

/// The environment every request runs in.
const ENV: [(&str, &str); 3] = [
    ("GIT_OPTIONAL_LOCKS", "0"),
    ("GIT_TERMINAL_PROMPT", "0"),
    ("LC_ALL", "C"),
];

/// Set for every request but the reader's own command line: a path a view
/// passes is a file's name, and a file named `*.rs` must not stage every
/// other one.
const LITERAL_PATHSPECS: (&str, &str) = ("GIT_LITERAL_PATHSPECS", "1");

/// The exit codes a request answers with when it did what it was asked.
const SUCCESS: &[i32] = &[0];

/// The exit codes of a diff that compares two files outside the index, which
/// says "they differ" with a 1 rather than failing.
const DIFF_NO_INDEX_SUCCESS: &[i32] = &[0, 1];

/// Tag naming anything that changed the repository, which the view answers by
/// re-reading it.
pub const TAG_CHANGED: &str = "changed";

/// Tag naming a command whose output is the answer, shown as it came back.
pub const TAG_READ: &str = "read";

/// Tag naming a log read that replaces the log view.
pub const TAG_LOG_VIEW: &str = "log-view";

/// Tags naming a read whose answer fills the output in place of the status:
/// what the view reads again when a command changes the repository under it.
pub const VIEW_TAGS: [&str; 5] = [TAG_BLAME, TAG_DIFF_VIEW, TAG_LOG_VIEW, TAG_READ, TAG_SHOW];

/// Tag naming a remote-URL lookup, for opening it in a browser.
pub const TAG_REMOTE_URL: &str = "remote-url";

/// Tag naming a blame read, which fills the view under its own title.
pub const TAG_BLAME: &str = "blame";

/// Tag naming a diff read.
pub const TAG_DIFF: &str = "diff";

/// Tag naming the diff of an untracked file against nothing, which answers
/// a difference with exit code 1.
pub const TAG_DIFF_UNTRACKED: &str = "diff-untracked";

/// Tag naming a whole-diff read, which fills the view as a decorated diff
/// rather than output shown as it came back.
pub const TAG_DIFF_VIEW: &str = "diff-view";

/// Tag naming a commit that waits on its message in Winter's own editor.
pub const TAG_COMMIT: &str = "commit";

/// Tag naming a discard request.
pub const TAG_DISCARD: &str = "discard";

/// Tag naming one commit's details, which fill the view under its own title.
pub const TAG_SHOW: &str = "show";

/// Tag naming a fetch request.
pub const TAG_FETCH: &str = "fetch";

/// Tag naming a log request.
pub const TAG_LOG: &str = "log";

/// Tag naming the read of earlier commit messages, for recalling one into
/// the message being written.
pub const TAG_MESSAGES: &str = "messages";

/// Tag naming a pull request.
pub const TAG_PULL: &str = "pull";

/// Tag naming a push request.
pub const TAG_PUSH: &str = "push";

/// Tag naming the read of where the branch pushes to and which remotes
/// there are, which the push menu is drawn from.
pub const TAG_PUSH_CONTEXT: &str = "push-context";

/// Tag naming the setting of a branch's push remote, which a push to it
/// follows.
pub const TAG_PUSH_REMOTE_SET: &str = "push-remote-set";

/// Tag naming the lookup of a commit's parents that an interactive rebase
/// from that commit starts with.
pub const TAG_REBASE_FROM: &str = "rebase-from";

/// Tag naming the lookup of a commit's full name and parents that a reword
/// of that commit starts with.
pub const TAG_REWORD_FROM: &str = "reword-from";

/// Tag naming the repository-root lookup, which also reports where the
/// repository keeps its own files.
pub const TAG_ROOT: &str = "root";

/// Tag naming a list of names gathered for the reader to choose from.
pub const TAG_CANDIDATES: &str = "candidates";

/// Tag naming the stash list the status view holds a section for.
pub const TAG_STASHES: &str = "stashes";

/// Tag naming the read of what the upstream has and this branch does not.
pub const TAG_UNPULLED: &str = "unpulled";

/// Tag naming a stage request.
pub const TAG_STAGE: &str = "stage";

/// Tag naming a conflict resolved by taking one side whole.
pub const TAG_RESOLVE: &str = "resolve";

/// Tag naming a status request.
pub const TAG_STATUS: &str = "status";

/// Tag naming an unstage request.
pub const TAG_UNSTAGE: &str = "unstage";

/// Tags naming a request that changes the repository or its settings, which
/// the status bar names while it runs. Reads are left out: the view makes
/// them on every change to the files, and naming each would only flicker.
pub const CHANGE_TAGS: [&str; 10] = [
    TAG_CHANGED,
    TAG_COMMIT,
    TAG_DISCARD,
    TAG_FETCH,
    TAG_PULL,
    TAG_PUSH,
    TAG_PUSH_REMOTE_SET,
    TAG_RESOLVE,
    TAG_STAGE,
    TAG_UNSTAGE,
];

/// Longest command summary the status bar shows before it is cut short.
const SUMMARY_MAX_CHARS: usize = 72;

/// Words every request carries that say nothing about what it does: the
/// separators that keep typed text from being read as options.
const SUMMARY_SKIPPED: [&str; 2] = [END_OF_OPTIONS, END_OF_REVISIONS];

/// How many recent commits the status view lists to begin with.
pub const RECENT_COMMITS: usize = 10;

/// The log format both log requests ask for: abbreviated hash, ref decoration,
/// author, author time, and subject, separated by US (`%x1f`).
///
/// A separator rather than git's own spacing because every field after the hash
/// is arbitrary text. `--date-order` is not set: the default reverse-chronology
/// is what a "recent commits" tail means.
const LOG_FORMAT: &str = "--format=%h%x1f%d%x1f%an%x1f%at%x1f%s";

// ========================================================================
// Data Structures
// ========================================================================

/// A revision or a ref's name, as the reader typed or chose it. It can never
/// start with a dash, so wherever it goes on a command line it cannot be read
/// as an option (`-f` passed to a checkout would throw work away). The lone
/// `-`, git's "the branch before this one", is the one exception.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Rev(String);

/// A path as git reports it, relative to the repository's root. It always
/// goes after a `--` and is matched literally, so a file named like an option
/// or holding a glob character names that file and nothing else.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Pathspec(String);

/// One git command line being put together: its options, then its operands,
/// then its paths, in the order git reads them.
#[derive(Clone, Debug)]
pub struct Git {
    /// Everything after the program and the global settings, in order.
    args: Vec<String>,
    /// Whether paths are matched literally, as everywhere but the reader's
    /// own command line.
    literal: bool,
    /// Whether `--end-of-options` has gone in yet.
    options_ended: bool,
}

/// What a commit written in an editor makes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CommitMode {
    /// Replace the last commit with one holding what is staged as well.
    Amend,
    /// A new commit of what is staged.
    New,
    /// Rewrite the last commit's message only, leaving anything staged out
    /// of it (magit's `c w`).
    Reword,
}

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

/// What a log read covers.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LogScope {
    /// Every ref, not just this branch.
    AllRefs,
    /// The current branch.
    Branch,
    /// One path's history.
    File(Pathspec),
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

/// Which names a question can be answered with.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Candidates {
    /// Every branch, local and on a remote: what a checkout or a merge takes.
    Branches,
    /// The branches in this repository, which are the ones that can be
    /// deleted.
    LocalBranches,
    /// The remotes.
    Remotes,
    /// The branches on the remotes, where a push can land.
    RemoteBranches,
    /// The tags.
    Tags,
}

// ========================================================================
// Rev
// ========================================================================

impl Rev {
    /// `text` as a revision, or why it cannot be one: empty, starting with a
    /// dash, or holding a character no ref name or revision can.
    pub fn parse(text: &str) -> Result<Self, String> {
        let text = text.trim();
        if text.is_empty() {
            return Err("no revision given".to_string());
        }
        if text.starts_with('-') && text != "-" {
            return Err(format!("not a revision: {text}"));
        }
        if text.chars().any(char::is_control) {
            return Err(format!("not a revision: {text:?}"));
        }
        Ok(Self(text.to_string()))
    }

    /// The revision as git is given it.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

// ========================================================================
// Pathspec
// ========================================================================

impl Pathspec {
    /// A path as git reported it, relative to the repository's root.
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Each of `paths`, as pathspecs.
    pub fn all(paths: &[String]) -> Vec<Self> {
        paths.iter().cloned().map(Self).collect()
    }
}

// ========================================================================
// Git
// ========================================================================

impl Git {
    /// A command line starting with `words`: the subcommand and whatever
    /// fixed options come with it.
    pub fn new<const N: usize>(words: [&str; N]) -> Self {
        Self {
            args: words.iter().map(|word| word.to_string()).collect(),
            literal: true,
            options_ended: false,
        }
    }

    /// The reader's own command line, taken as typed: the escape hatch for
    /// commands the view offers no key for. Paths in it are matched the way
    /// git matches them anywhere, globs included, since that is what someone
    /// typing `git add *.rs` means.
    pub fn typed(words: Vec<String>) -> Self {
        Self {
            args: words,
            literal: false,
            options_ended: false,
        }
    }

    /// Add an option the view itself chose.
    pub fn flag(mut self, flag: impl Into<String>) -> Self {
        self.args.push(flag.into());
        self
    }

    /// Add a revision. No `--end-of-options` is needed for one, since a
    /// [`Rev`] cannot start with a dash; leaving it out keeps the commands
    /// that do not accept it (`checkout`, `reset`) working.
    pub fn rev(mut self, rev: &Rev) -> Self {
        self.args.push(rev.0.clone());
        self
    }

    /// Add a name or a path the reader typed, after `--end-of-options`, so
    /// one starting with a dash is still taken as itself.
    pub fn operand(mut self, operand: &str) -> Self {
        if !self.options_ended {
            self.args.push(END_OF_OPTIONS.to_string());
            self.options_ended = true;
        }
        self.args.push(operand.to_string());
        self
    }

    /// Add `paths` after a `--`, the end of everything that is not a path.
    pub fn paths(mut self, paths: &[Pathspec]) -> Self {
        self.args.push(END_OF_REVISIONS.to_string());
        self.args.extend(paths.iter().map(|path| path.0.clone()));
        self
    }

    /// Add `paths`, or the whole tree when there are none: `add --` with no
    /// pathspec is a no-op rather than an error, which would silently do
    /// nothing.
    fn paths_or_everything(self, paths: &[Pathspec]) -> Self {
        match paths.is_empty() {
            true => self.paths(&[Pathspec::new(EVERYTHING)]),
            false => self.paths(paths),
        }
    }

    /// End the revisions with nothing after them, so a revision that is also
    /// a file's name is still read as the revision.
    pub fn end_revisions(mut self) -> Self {
        self.args.push(END_OF_REVISIONS.to_string());
        self
    }

    /// The command line, run from `cwd` and answered under `tag`.
    pub fn command(self, cwd: &Path, tag: &'static str) -> CommandRequest {
        let mut env: Vec<(String, String)> = ENV
            .iter()
            .map(|(name, value)| (name.to_string(), value.to_string()))
            .collect();
        if self.literal {
            env.push((
                LITERAL_PATHSPECS.0.to_string(),
                LITERAL_PATHSPECS.1.to_string(),
            ));
        }
        let args = QUOTE_PATH_OFF
            .iter()
            .map(|arg| arg.to_string())
            .chain(self.args)
            .collect();
        CommandRequest {
            args,
            cwd: PathBuf::from(cwd),
            env,
            program: GIT.to_string(),
            stdin: None,
            tag,
        }
    }

    /// The same, as the job an editor asks the host for.
    pub fn job(self, cwd: &Path, tag: &'static str) -> JobRequest {
        JobRequest::Command(self.command(cwd, tag))
    }
}

// ========================================================================
// ResetMode
// ========================================================================

impl ResetMode {
    fn flag(self) -> &'static str {
        match self {
            ResetMode::Hard => "--hard",
            ResetMode::Mixed => "--mixed",
            ResetMode::Soft => "--soft",
        }
    }
}

// ========================================================================
// SequenceStep
// ========================================================================

impl SequenceStep {
    fn flag(self) -> &'static str {
        match self {
            SequenceStep::Abort => "--abort",
            SequenceStep::Continue => "--continue",
            SequenceStep::Skip => "--skip",
        }
    }
}

// ========================================================================
// ApplyTarget
// ========================================================================

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

// ========================================================================
// Functions: answers
// ========================================================================

/// Whether a finished request did what it was asked, by the exit codes its
/// tag answers success with: most only with 0, a diff of two files outside
/// the index with 1 as well.
pub fn succeeded(output: &CommandOutput) -> bool {
    let codes = match output.tag {
        TAG_DIFF_UNTRACKED => DIFF_NO_INDEX_SUCCESS,
        _ => SUCCESS,
    };
    output.code.is_some_and(|code| codes.contains(&code))
}

/// The command line `request` runs, as a reader would type it: the settings
/// every request carries and the option separators left out, and cut short
/// to fit the status bar (`git push origin main`).
pub fn summary(request: &CommandRequest) -> String {
    // Every request here is put together by [`Git::command`], which puts
    // the settings first.
    let line = std::iter::once(request.program.as_str())
        .chain(
            request
                .args
                .iter()
                .skip(QUOTE_PATH_OFF.len())
                .map(String::as_str)
                .filter(|arg| !SUMMARY_SKIPPED.contains(arg)),
        )
        .collect::<Vec<_>>()
        .join(" ");
    if line.chars().count() <= SUMMARY_MAX_CHARS {
        return line;
    }
    let kept: String = line.chars().take(SUMMARY_MAX_CHARS).collect();
    format!("{kept}...")
}

// ========================================================================
// Functions: reads
// ========================================================================

/// Where the repository is, and where it keeps its own files: two lines, the
/// working tree's root and the directory holding `HEAD` and the state files
/// naming whatever git is part-way through. A worktree and a submodule put
/// the second somewhere the first does not imply, so it is asked for rather
/// than assumed.
pub fn repo_root(cwd: &Path) -> JobRequest {
    Git::new(["rev-parse", "--show-toplevel", "--absolute-git-dir"]).job(cwd, TAG_ROOT)
}

/// The names a question can be answered with, for the reader to pick from
/// rather than type: [`Candidates`] says which kind.
pub fn candidates(root: &Path, kind: Candidates) -> JobRequest {
    match kind {
        Candidates::Branches => refs(root, &["refs/heads", "refs/remotes"]),
        Candidates::LocalBranches => refs(root, &["refs/heads"]),
        Candidates::Remotes => Git::new(["remote"]).job(root, TAG_CANDIDATES),
        Candidates::RemoteBranches => refs(root, &["refs/remotes"]),
        Candidates::Tags => refs(root, &["refs/tags"]),
    }
}

/// Every ref under `namespaces`, by its short name.
fn refs(root: &Path, namespaces: &[&str]) -> JobRequest {
    let git = Git::new([
        "for-each-ref",
        "--format=%(refname:short)",
        "--sort=-committerdate",
    ]);
    namespaces
        .iter()
        .fold(git, |git, space| git.flag(*space))
        .job(root, TAG_CANDIDATES)
}

/// The stashes, as the status view's own section lists them: each one's name
/// and the message it was pushed with.
pub fn stash_entries(root: &Path) -> JobRequest {
    Git::new(["stash", "list", "--format=%gd%x1f%s"]).job(root, TAG_STASHES)
}

/// The commits the upstream has that this branch does not, newest first.
///
/// Fails when there is no upstream at all, which is not an error worth
/// reporting: a branch that tracks nothing is behind nothing.
pub fn unpulled(root: &Path) -> JobRequest {
    Git::new(["log", LOG_FORMAT, "--decorate=short", "HEAD..@{upstream}"]).job(root, TAG_UNPULLED)
}

/// The working tree's state, with branch and tracking information, each
/// record ended by a NUL so no path needs quoting to be read back.
///
/// Read without taking optional locks, so looking at the tree never rewrites
/// the index: that write would be seen by the watch as a change, and the next
/// look would start from it.
pub fn status(root: &Path) -> JobRequest {
    Git::new([
        "--no-optional-locks",
        "status",
        "--porcelain=v2",
        "-z",
        "--branch",
        "--untracked-files=all",
    ])
    .job(root, TAG_STATUS)
}

/// How many commits to ask for to show `count`: one more, so that whether
/// the answer holds it tells whether there are more to load. A history of
/// exactly `count` commits, its first commit already shown, then offers no
/// more.
pub fn with_peek(count: usize) -> usize {
    count + 1
}

/// The `count` most recent commits, for the view's own tail, and one past
/// them when there is one: see [`with_peek`].
pub fn recent_log(root: &Path, count: usize) -> JobRequest {
    Git::new(["log", LOG_FORMAT, "--decorate=short"])
        .flag(format!("--max-count={}", with_peek(count)))
        .job(root, TAG_LOG)
}

/// The messages of the `count` most recent commits, each ended by
/// `separator`, for recalling one into the message being written.
pub fn messages(cwd: &Path, count: usize, separator: char) -> JobRequest {
    Git::new(["log"])
        .flag(format!("--max-count={count}"))
        .flag(format!("--format=%B{separator}"))
        .job(cwd, TAG_MESSAGES)
}

/// The diff for one path, from the index when `staged` or from the working
/// tree otherwise. Context is trimmed to what a reviewer needs, and rename
/// detection is off so a hunk maps to one path.
pub fn diff_file(root: &Path, path: &Pathspec, staged: bool) -> JobRequest {
    let mut git = Git::new(["diff", "--no-color"]);
    if staged {
        git = git.flag("--cached");
    }
    git.flag("--no-ext-diff")
        .flag("--no-renames")
        .paths(slice::from_ref(path))
        .job(root, TAG_DIFF)
}

/// The diff for an untracked path, which git will only produce against the
/// empty tree. It answers 1 for "they differ", which is the whole point, so
/// it carries a tag of its own (see [`succeeded`]).
pub fn diff_untracked(root: &Path, path: &Pathspec) -> JobRequest {
    Git::new(["diff", "--no-color", "--no-index"])
        .paths(&[Pathspec::new("/dev/null"), path.clone()])
        .job(root, TAG_DIFF_UNTRACKED)
}

/// Every ref, with what it points at.
pub fn show_refs(root: &Path) -> JobRequest {
    Git::new(["show-ref", "--abbrev"]).job(root, TAG_READ)
}

/// Who last touched each line of a path.
pub fn blame(root: &Path, path: &Pathspec) -> JobRequest {
    Git::new(["blame", "--date=short", "--abbrev=8"])
        .paths(slice::from_ref(path))
        .job(root, TAG_BLAME)
}

/// One commit in full: its author, date and message, the files it touched, and
/// the patch itself.
///
/// No `--shortstat`: the view counts the commit's own hunks instead, which is
/// the unit it lets the reader open, where git's line totals count something
/// nothing in the view acts on.
pub fn show_commit(root: &Path, rev: &Rev) -> JobRequest {
    Git::new([
        "show",
        "--no-color",
        "--no-ext-diff",
        "--patch",
        "--date=iso",
    ])
    .rev(rev)
    .end_revisions()
    .job(root, TAG_SHOW)
}

/// What one stash holds, as a diff the view paints the way it paints any
/// other: a stash is a commit under a different name.
pub fn stash_show(root: &Path, name: &Rev) -> JobRequest {
    Git::new(["stash", "show", "--no-color", "--patch"])
        .rev(name)
        .job(root, TAG_DIFF_VIEW)
}

/// A whole diff, for reading rather than staging.
pub fn diff_all(root: &Path, staged: bool, rev: Option<&Rev>) -> JobRequest {
    let mut git = Git::new(["diff", "--no-color"]);
    if staged {
        git = git.flag("--cached");
    }
    if let Some(rev) = rev {
        git = git.rev(rev).end_revisions();
    }
    git.job(root, TAG_DIFF_VIEW)
}

/// The log for a path, or for every ref: `count` commits, and one past them
/// when there is one (see [`with_peek`]).
pub fn log(root: &Path, scope: LogScope, count: usize) -> JobRequest {
    let git = Git::new(["log", LOG_FORMAT, "--decorate=short"])
        .flag(format!("--max-count={}", with_peek(count)));
    let git = match scope {
        LogScope::AllRefs => git.flag("--all"),
        LogScope::Branch => git,
        LogScope::File(path) => git.paths(&[path]),
    };
    git.job(root, TAG_LOG_VIEW)
}

/// List the stash, for reading rather than changing anything.
pub fn stash_list(root: &Path) -> JobRequest {
    Git::new(["stash", "list"]).job(root, TAG_READ)
}

/// List tags.
pub fn tag_list(root: &Path) -> JobRequest {
    Git::new(["tag", "--list"]).job(root, TAG_READ)
}

/// List remotes with their URLs.
pub fn remote_list(root: &Path) -> JobRequest {
    Git::new(["remote", "--verbose"]).job(root, TAG_READ)
}

/// The URL a remote fetches from, for opening it in a browser.
pub fn remote_url(root: &Path, name: &str) -> JobRequest {
    Git::new(["remote", "get-url"])
        .operand(name)
        .job(root, TAG_REMOTE_URL)
}

/// Worktrees, listed.
pub fn worktree_list(root: &Path) -> JobRequest {
    Git::new(["worktree", "list"]).job(root, TAG_READ)
}

// ========================================================================
// Functions: changes
// ========================================================================

/// `git commit` as `mode` says, with its message written in the editor
/// `editor` names (see `git_editor_command`). Git runs headless: the
/// editor is a tab of the running window that git waits on, so no terminal
/// tab is needed, and what git reports once it finishes comes back as the
/// answer.
pub fn commit(root: &Path, mode: CommitMode, editor: Option<String>) -> JobRequest {
    let git = match mode {
        CommitMode::Amend => Git::new(["commit", "--amend"]),
        CommitMode::New => Git::new(["commit"]),
        // `--only` with no paths commits none of the index: the message is
        // all that changes.
        CommitMode::Reword => Git::new(["commit", "--amend", "--only"]),
    };
    let mut command = git.command(root, TAG_COMMIT);
    command
        .env
        .extend(editor.map(|editor| ("GIT_EDITOR".to_string(), editor)));
    JobRequest::Command(command)
}

/// Amend the last commit with what is staged, keeping its message.
pub fn amend_keeping_message(root: &Path) -> JobRequest {
    Git::new(["commit", "--amend", "--no-edit"]).job(root, TAG_CHANGED)
}

/// Stage `paths`. An empty list stages everything, which is what the
/// stage-all key asks for.
pub fn stage(root: &Path, paths: &[Pathspec]) -> JobRequest {
    Git::new(["add"])
        .paths_or_everything(paths)
        .job(root, TAG_STAGE)
}

/// Settle a conflicted file by taking one side of it whole: `ours`, the
/// branch being merged into, or theirs. Staging it afterwards is what marks
/// the conflict resolved.
pub fn resolve_conflict(root: &Path, path: &Pathspec, ours: bool) -> JobRequest {
    let side = match ours {
        true => "--ours",
        false => "--theirs",
    };
    Git::new(["checkout", side])
        .paths(slice::from_ref(path))
        .job(root, TAG_RESOLVE)
}

/// Stage every tracked change, the way Magic's stage-all does outside the
/// untracked section. `-u` is what keeps untracked files out: those are
/// staged only by name, so nothing lands in the index unread.
pub fn stage_all_tracked(root: &Path) -> JobRequest {
    Git::new(["add", "-u"]).job(root, TAG_STAGE)
}

/// Unstage `paths`, leaving the working tree alone. An empty list unstages
/// everything.
pub fn unstage(root: &Path, paths: &[Pathspec]) -> JobRequest {
    Git::new(["restore", "--staged"])
        .paths_or_everything(paths)
        .job(root, TAG_UNSTAGE)
}

/// Throw away working-tree changes to `paths`. Nothing recovers this, which is
/// why the view confirms first.
pub fn discard(root: &Path, paths: &[Pathspec]) -> JobRequest {
    Git::new(["restore", "--worktree"])
        .paths(paths)
        .job(root, TAG_DISCARD)
}

/// Delete untracked `paths`, which `restore` cannot touch.
pub fn remove_untracked(root: &Path, paths: &[Pathspec]) -> JobRequest {
    Git::new(["clean", "--force", "-d"])
        .paths(paths)
        .job(root, TAG_DISCARD)
}

/// Apply `patch` to the index, or reverse it out of the index or the working
/// tree. This is what makes staging a single hunk possible: the patch is fed
/// on standard input rather than written to a file.
pub fn apply_patch(root: &Path, patch: String, target: ApplyTarget) -> JobRequest {
    let git = Git::new(["apply", "--unidiff-zero"]);
    let git = match target {
        ApplyTarget::Index => git.flag("--cached"),
        ApplyTarget::IndexReverse => git.flag("--cached").flag("--reverse"),
        ApplyTarget::WorktreeReverse => git.flag("--reverse"),
    };
    let mut command = git.flag("-").command(root, target.tag());
    command.stdin = Some(patch);
    JobRequest::Command(command)
}

/// Read what the push menu shows for `branch`: every remote's URL setting,
/// which names the remotes, and where the branch pushes when that is set,
/// its own `pushRemote` or the repository's `remote.pushDefault`.
pub fn push_context(root: &Path, branch: Option<&str>) -> JobRequest {
    let mut pattern = String::from(r"^remote\..*\.url$|^remote\.pushdefault$");
    if let Some(branch) = branch {
        pattern.push_str(&format!(r"|^branch\.{}\.pushremote$", escape_ere(branch)));
    }
    Git::new(["config", "--get-regexp"])
        .operand(&pattern)
        .job(root, TAG_PUSH_CONTEXT)
}

/// Push `refspec` to `remote`, with `switches` (`--force-with-lease`, ...).
pub fn push_to(root: &Path, switches: &[&str], remote: &str, refspec: &str) -> JobRequest {
    switches
        .iter()
        .fold(Git::new(["push"]), |git, switch| git.flag(*switch))
        .operand(remote)
        .operand(refspec)
        .job(root, TAG_PUSH)
}

/// Push `branch` to `name` on `remote` and make that its upstream.
pub fn push_set_upstream(
    root: &Path,
    switches: &[&str],
    remote: &str,
    branch: &str,
    name: &str,
) -> JobRequest {
    switches
        .iter()
        .fold(Git::new(["push", "--set-upstream"]), |git, switch| {
            git.flag(*switch)
        })
        .operand(remote)
        .operand(&format!("{branch}:{name}"))
        .job(root, TAG_PUSH)
}

/// Push every tag to `remote`.
pub fn push_tags(root: &Path, switches: &[&str], remote: &str) -> JobRequest {
    switches
        .iter()
        .fold(Git::new(["push", "--tags"]), |git, switch| {
            git.flag(*switch)
        })
        .operand(remote)
        .job(root, TAG_PUSH)
}

/// Make `remote` where `branch` pushes, as its own `pushRemote`.
pub fn set_push_remote(root: &Path, branch: &str, remote: &str) -> JobRequest {
    Git::new(["config"])
        .operand(&format!("branch.{branch}.pushRemote"))
        .operand(remote)
        .job(root, TAG_PUSH_REMOTE_SET)
}

/// `text` with every character a POSIX extended regex gives a meaning to
/// escaped, for matching a branch's name in `git config --get-regexp`.
fn escape_ere(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for c in text.chars() {
        if r".[]{}()\*+?|^$".contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    escaped
}

/// Push `branch` to `remote`, setting it as the branch's upstream so a branch
/// the remote has never seen is created there and later pushes need no arguments.
pub fn push_upstream(root: &Path, remote: &str, branch: &str) -> JobRequest {
    Git::new(["push", "--set-upstream"])
        .operand(remote)
        .operand(branch)
        .job(root, TAG_PUSH)
}

/// Pull with rebase, which is what a status view's "pull" should mean: a merge
/// commit nobody asked for is worse than a refusal.
pub fn pull(root: &Path) -> JobRequest {
    Git::new(["pull", "--rebase"]).job(root, TAG_PULL)
}

/// Pull `branch` from `remote` with rebase, naming both so a branch that
/// tracks nothing can still be brought up to date.
pub fn pull_from(root: &Path, remote: &str, branch: &str) -> JobRequest {
    Git::new(["pull", "--rebase"])
        .operand(remote)
        .operand(branch)
        .job(root, TAG_PULL)
}

/// Fetch every remote.
pub fn fetch(root: &Path) -> JobRequest {
    Git::new(["fetch", "--all", "--prune"]).job(root, TAG_FETCH)
}

/// Check out an existing branch or revision. The `--` after it is what keeps
/// a name that is also a file's from being read as that file, which would
/// throw away its changes rather than switch anywhere.
pub fn checkout(root: &Path, rev: &Rev) -> JobRequest {
    Git::new(["checkout"])
        .rev(rev)
        .end_revisions()
        .job(root, TAG_CHANGED)
}

/// Create a branch, checking it out when asked.
pub fn branch_create(root: &Path, name: &Rev, checkout: bool) -> JobRequest {
    let git = match checkout {
        true => Git::new(["checkout", "-b"]).rev(name).end_revisions(),
        false => Git::new(["branch"]).rev(name),
    };
    git.job(root, TAG_CHANGED)
}

/// Delete a branch. `force` deletes one whose work is not merged.
pub fn branch_delete(root: &Path, name: &Rev, force: bool) -> JobRequest {
    let flag = if force { "-D" } else { "-d" };
    Git::new(["branch", flag]).rev(name).job(root, TAG_CHANGED)
}

/// Merge a branch into the current one.
pub fn merge(root: &Path, rev: &Rev) -> JobRequest {
    Git::new(["merge", "--no-edit"])
        .rev(rev)
        .job(root, TAG_CHANGED)
}

/// Continue or abandon whatever sequence is in progress. One command shape
/// covers merge, rebase, cherry-pick, and revert, which is why the verb is a
/// parameter rather than five near-identical functions.
pub fn sequence(root: &Path, verb: &str, step: SequenceStep) -> JobRequest {
    Git::new([verb, step.flag()]).job(root, TAG_CHANGED)
}

/// `commit` with its parents, one line, for an interactive rebase that keeps
/// `commit` in its plan: it starts at the first parent, or at the root when
/// there is none. `rev-list` answers either way, where asking for `commit^`
/// fails on a root commit and the failure would end the read.
pub fn rebase_from(root: &Path, commit: &Rev) -> JobRequest {
    Git::new(["rev-list", "--parents", "--max-count=1"])
        .rev(commit)
        .job(root, TAG_REBASE_FROM)
}

/// The arguments of an interactive rebase that keeps the commit a
/// [`rebase_from`] answer names, and every commit after it, in its plan.
pub fn interactive_rebase_args(parents: &str) -> Vec<String> {
    let start = match parents.split_whitespace().nth(1) {
        Some(parent) => parent.to_string(),
        None => "--root".to_string(),
    };
    vec!["rebase".to_string(), "--interactive".to_string(), start]
}

/// Look up `commit`'s full name and its parents, which a reword of it starts
/// from (see [`reword_commit`]).
pub fn reword_from(root: &Path, commit: &Rev) -> JobRequest {
    Git::new(["rev-list", "--parents", "--max-count=1"])
        .rev(commit)
        .job(root, TAG_REWORD_FROM)
}

/// Rewrite the message of the commit a [`reword_from`] answer names, wherever
/// it is on the branch, in `editor` (see `git_editor_command`). An interactive
/// rebase from its parent whose plan marks that one commit `reword` by itself,
/// through a `sed` sequence editor, so git goes straight to the message. Merges
/// after it are kept as merges, and uncommitted changes are stashed around it.
/// `None` for an answer naming no commit.
pub fn reword_commit(root: &Path, parents: &str, editor: Option<String>) -> Option<JobRequest> {
    let mut names = parents.split_whitespace();
    let commit = names.next()?;
    if commit.len() < PLAN_HASH_LEN || !commit.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let start = names.next().unwrap_or("--root");
    let git = Git::new(["-c", PLAN_ABBREV, "rebase", "--interactive"])
        .flag("--autostash")
        .flag("--rebase-merges")
        .flag(start);
    let mut command = git.command(root, TAG_COMMIT);
    command.env.push((
        "GIT_SEQUENCE_EDITOR".to_string(),
        reword_plan_editor(&commit[..PLAN_HASH_LEN]),
    ));
    command
        .env
        .extend(editor.map(|editor| ("GIT_EDITOR".to_string(), editor)));
    Some(JobRequest::Command(command))
}

/// The sequence editor that turns the plan's `pick` (or `p`) of the commit
/// whose name starts with `hash` into a `reword`, and leaves the rest as git
/// wrote it. `-i.winter` rather than a bare `-i`, which BSD `sed` reads as
/// taking the next word as the backup suffix.
fn reword_plan_editor(hash: &str) -> String {
    format!(r"sed -i.winter -e 's/^p\(ick\)\{{0,1\}} \({hash}[0-9a-f]*\) /reword \2 /'")
}

/// Replay commits onto `upstream`.
pub fn rebase(root: &Path, upstream: &Rev) -> JobRequest {
    Git::new(["rebase"]).rev(upstream).job(root, TAG_CHANGED)
}

/// Stash everything, with an optional message.
pub fn stash_push(root: &Path, message: &str) -> JobRequest {
    let mut git = Git::new(["stash", "push"]);
    if !message.trim().is_empty() {
        git = git.flag("--message").flag(message);
    }
    git.job(root, TAG_CHANGED)
}

/// Act on the newest stash entry.
pub fn stash(root: &Path, verb: &str) -> JobRequest {
    Git::new(["stash", verb]).job(root, TAG_CHANGED)
}

/// Create a tag at the current commit.
pub fn tag_create(root: &Path, name: &Rev) -> JobRequest {
    Git::new(["tag"]).rev(name).job(root, TAG_CHANGED)
}

/// Delete a tag.
pub fn tag_delete(root: &Path, name: &Rev) -> JobRequest {
    Git::new(["tag", "--delete"])
        .rev(name)
        .job(root, TAG_CHANGED)
}

/// Add a remote, taking `name url` as one answer.
pub fn remote_add(root: &Path, name: &str, url: &str) -> JobRequest {
    Git::new(["remote", "add"])
        .operand(name)
        .operand(url)
        .job(root, TAG_CHANGED)
}

/// Remove a remote.
pub fn remote_remove(root: &Path, name: &str) -> JobRequest {
    Git::new(["remote", "remove"])
        .operand(name)
        .job(root, TAG_CHANGED)
}

/// Drop remote-tracking refs the remote no longer has.
pub fn remote_prune(root: &Path, name: &str) -> JobRequest {
    Git::new(["remote", "prune"])
        .operand(name)
        .job(root, TAG_CHANGED)
}

/// Replay one commit onto the current branch.
pub fn cherry_pick(root: &Path, rev: &Rev) -> JobRequest {
    Git::new(["cherry-pick"]).rev(rev).job(root, TAG_CHANGED)
}

/// Undo a commit with a new one.
pub fn revert(root: &Path, rev: &Rev) -> JobRequest {
    Git::new(["revert", "--no-edit"])
        .rev(rev)
        .job(root, TAG_CHANGED)
}

/// Move the branch pointer, keeping as much as `mode` says. The `--` keeps a
/// revision that is also a file's name from turning the reset into an
/// unstaging of that file.
pub fn reset(root: &Path, mode: ResetMode, rev: &Rev) -> JobRequest {
    Git::new(["reset", mode.flag()])
        .rev(rev)
        .end_revisions()
        .job(root, TAG_CHANGED)
}

/// Add a worktree at `path`, on a new branch named after it.
pub fn worktree_add(root: &Path, path: &str) -> JobRequest {
    Git::new(["worktree", "add"])
        .operand(path)
        .job(root, TAG_CHANGED)
}

/// Remove a worktree.
pub fn worktree_remove(root: &Path, path: &str) -> JobRequest {
    Git::new(["worktree", "remove"])
        .operand(path)
        .job(root, TAG_CHANGED)
}

/// Start a bisect, or answer its question.
pub fn bisect(root: &Path, verb: &str) -> JobRequest {
    Git::new(["bisect", verb]).job(root, TAG_CHANGED)
}

/// Whatever the user typed, split on spaces: the escape hatch for the commands
/// this view does not offer a key for.
pub fn custom(root: &Path, line: &str) -> JobRequest {
    Git::typed(line.split_whitespace().map(str::to_string).collect()).job(root, TAG_CHANGED)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io;
    use std::process::{Command, Output, Stdio};

    /// Run `request` to the end, the way the host's runner would.
    fn run_blocking(request: &CommandRequest) -> io::Result<Output> {
        Command::new(&request.program)
            .args(&request.args)
            .current_dir(&request.cwd)
            .envs(request.env.iter().map(|(name, value)| (name, value)))
            .stdin(Stdio::null())
            .output()
    }

    fn command_of(request: &JobRequest) -> &CommandRequest {
        match request {
            JobRequest::Command(command) => command,
            JobRequest::DirSize(_)
            | JobRequest::FileOps(_)
            | JobRequest::Processes
            | JobRequest::ReadFiles(_)
            | JobRequest::Search(_)
            | JobRequest::System => panic!("expected a command"),
        }
    }

    /// The arguments after the settings every request carries.
    fn args_of(request: &JobRequest) -> Vec<String> {
        command_of(request).args[QUOTE_PATH_OFF.len()..].to_vec()
    }

    fn env_of(request: &JobRequest, name: &str) -> Option<String> {
        command_of(request)
            .env
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }

    fn rev(text: &str) -> Rev {
        Rev::parse(text).expect("a revision")
    }

    fn path(text: &str) -> Pathspec {
        Pathspec::new(text)
    }

    #[test]
    fn test_a_summary_reads_as_typed_without_the_settings_or_separators() {
        let staged = stage(Path::new("/repo"), &[path("src/a b.rs")]);
        assert_eq!(summary(command_of(&staged)), "git add src/a b.rs");
        let checkout = checkout(Path::new("/repo"), &rev("main"));
        assert!(
            !summary(command_of(&checkout)).contains("--"),
            "{checkout:?}"
        );
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
        let one = args_of(&stage(Path::new("/repo"), &[path("--force")]));
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
        let args = args_of(&unstage(Path::new("/repo"), &[path("a.rs")]));
        assert_eq!(args, ["restore", "--staged", "--", "a.rs"]);
    }

    #[test]
    fn test_discard_names_the_working_tree_explicitly() {
        let args = args_of(&discard(Path::new("/repo"), &[path("a.rs")]));
        assert_eq!(args, ["restore", "--worktree", "--", "a.rs"]);
    }

    #[test]
    fn test_pull_rebases_rather_than_merging() {
        assert_eq!(args_of(&pull(Path::new("/repo"))), ["pull", "--rebase"]);
    }

    #[test]
    fn test_every_request_reports_paths_unquoted_and_takes_no_lock() {
        let request = status(Path::new("/repo"));
        assert_eq!(
            command_of(&request).args[..QUOTE_PATH_OFF.len()],
            QUOTE_PATH_OFF
        );
        assert_eq!(env_of(&request, "GIT_OPTIONAL_LOCKS").as_deref(), Some("0"));
        assert_eq!(env_of(&request, "LC_ALL").as_deref(), Some("C"));
        assert!(
            args_of(&request).contains(&"-z".to_string()),
            "records end in NUL"
        );
    }

    #[test]
    fn test_a_path_holding_a_glob_names_only_that_file() {
        let staged = stage(Path::new("/repo"), &[path("*.rs")]);
        assert_eq!(
            env_of(&staged, "GIT_LITERAL_PATHSPECS").as_deref(),
            Some("1")
        );
        // The reader's own command line keeps git's globbing.
        let typed = custom(Path::new("/repo"), "add *.rs");
        assert_eq!(env_of(&typed, "GIT_LITERAL_PATHSPECS"), None);
    }

    #[test]
    fn test_a_revision_never_starts_with_a_dash() {
        assert!(Rev::parse("-f").is_err(), "an option is no revision");
        assert!(Rev::parse("--orphan=x").is_err());
        assert!(Rev::parse("  ").is_err());
        assert!(Rev::parse("a\nb").is_err());
        assert_eq!(rev(" main ").as_str(), "main");
        assert_eq!(rev("-").as_str(), "-", "the previous branch");
    }

    #[test]
    fn test_checkout_ends_its_revision_so_a_file_of_that_name_is_safe() {
        // `git checkout foo` with no branch `foo` restores the file `foo`,
        // throwing its changes away; with `--` after it, it refuses instead.
        let args = args_of(&checkout(Path::new("/repo"), &rev("foo")));
        assert_eq!(args, ["checkout", "foo", "--"]);
        let args = args_of(&reset(Path::new("/repo"), ResetMode::Mixed, &rev("foo")));
        assert_eq!(args, ["reset", "--mixed", "foo", "--"]);
    }

    #[test]
    fn test_typed_names_follow_the_end_of_the_options() {
        let args = args_of(&worktree_add(Path::new("/repo"), "-x"));
        assert_eq!(args, ["worktree", "add", "--end-of-options", "-x"]);
        let args = args_of(&remote_add(Path::new("/repo"), "up", "--upload-pack=x"));
        assert_eq!(
            args,
            ["remote", "add", "--end-of-options", "up", "--upload-pack=x"],
            "one marker covers every operand after it"
        );
    }

    #[test]
    fn test_an_untracked_diff_that_finds_a_difference_has_succeeded() {
        let answer = |tag, code| CommandOutput {
            code: Some(code),
            tag,
            ..CommandOutput::default()
        };
        assert!(
            succeeded(&answer(TAG_DIFF_UNTRACKED, 1)),
            "1 means they differ"
        );
        assert!(!succeeded(&answer(TAG_DIFF_UNTRACKED, 128)));
        assert!(!succeeded(&answer(TAG_DIFF, 1)), "elsewhere 1 is a failure");
        assert!(succeeded(&answer(TAG_DIFF, 0)));
    }

    #[test]
    fn test_git_answers_an_untracked_difference_with_exit_code_one() {
        let dir = std::env::temp_dir().join(format!("winter-exec-noindex-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("new.txt"), "hi\n").expect("write");
        let JobRequest::Command(request) = diff_untracked(&dir, &path("new.txt")) else {
            panic!("a command");
        };
        let output = run_blocking(&request).expect("git runs");
        let answer = CommandOutput {
            code: output.status.code(),
            tag: request.tag,
            ..CommandOutput::default()
        };
        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(answer.code, Some(1));
        assert!(succeeded(&answer));
    }

    #[test]
    fn test_checking_out_a_name_that_is_only_a_file_keeps_its_changes() {
        let dir = std::env::temp_dir().join(format!("winter-exec-checkout-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs")
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("foo"), "one\n").expect("write");
        git(&["add", "foo"]);
        git(&["commit", "-qm", "base"]);
        std::fs::write(dir.join("foo"), "work in progress\n").expect("write");
        let JobRequest::Command(request) = checkout(&dir, &rev("foo")) else {
            panic!("a command");
        };
        let output = run_blocking(&request).expect("git runs");
        let kept = std::fs::read_to_string(dir.join("foo")).expect("read");
        let _ = std::fs::remove_dir_all(&dir);
        assert!(!output.status.success(), "no branch foo: refused");
        assert_eq!(kept, "work in progress\n", "the file was left alone");
    }

    #[cfg(unix)]
    #[test]
    fn test_an_interactive_rebase_from_a_commit_plans_it_and_what_follows() {
        // The plan has to start with the commit asked for: rebasing onto the
        // branch itself, the only way in before, planned nothing at all.
        let dir = std::env::temp_dir().join(format!("winter-exec-rebase-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let plan = dir.join("plan");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&dir)
                // Save the plan git opens instead of editing it, and leave
                // it as it is, so the rebase replays every commit unchanged.
                .env(
                    "GIT_SEQUENCE_EDITOR",
                    format!("f() {{ cp \"$1\" '{}'; }}; f", plan.display()),
                )
                .output()
                .expect("git runs")
        };
        let head = |rev: &str| {
            String::from_utf8_lossy(&git(&["rev-parse", "--short", rev]).stdout)
                .trim()
                .to_string()
        };
        git(&["init", "-q"]);
        for name in ["one", "two", "three"] {
            std::fs::write(dir.join(name), name).expect("write");
            git(&["add", name]);
            git(&["commit", "-qm", name]);
        }
        let planned = |commit: &str| {
            let JobRequest::Command(request) = rebase_from(&dir, &rev(commit)) else {
                panic!("a command");
            };
            let parents = run_blocking(&request).expect("git runs");
            assert!(parents.status.success(), "answers for a root commit too");
            let args = interactive_rebase_args(&String::from_utf8_lossy(&parents.stdout));
            let args: Vec<&str> = args.iter().map(String::as_str).collect();
            assert!(git(&args).status.success(), "the rebase runs: {args:?}");
            std::fs::read_to_string(&plan).expect("a plan was written")
        };

        let (one, two, three) = (head("HEAD~2"), head("HEAD~1"), head("HEAD"));
        let from_two = planned(&two);
        assert!(from_two.contains(&format!("pick {two}")), "{from_two}");
        assert!(from_two.contains(&format!("pick {three}")), "{from_two}");
        assert!(!from_two.contains(&format!("pick {one}")), "{from_two}");

        let from_root = planned(&one);
        assert!(
            from_root.contains(&format!("pick {one}")),
            "the root commit, via --root"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn test_rewording_an_older_commit_changes_only_its_message() {
        let dir =
            std::env::temp_dir().join(format!("winter-exec-reword-old-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let identity = [
            ("GIT_AUTHOR_NAME", "t"),
            ("GIT_AUTHOR_EMAIL", "t@t"),
            ("GIT_COMMITTER_NAME", "t"),
            ("GIT_COMMITTER_EMAIL", "t@t"),
        ];
        let git = |args: &[&str]| {
            Command::new("git")
                .args(args)
                .envs(identity)
                .current_dir(&dir)
                .output()
                .expect("git runs")
        };
        let text = |output: Output| String::from_utf8_lossy(&output.stdout).trim().to_string();
        git(&["init", "-q"]);
        for (file, message) in [("a", "first"), ("b", "second"), ("c", "third")] {
            std::fs::write(dir.join(file), message).expect("write");
            git(&["add", file]);
            git(&["commit", "-qm", message]);
        }
        std::fs::write(dir.join("a"), "staged").expect("write");
        git(&["add", "a"]);

        let middle = Rev::parse("HEAD~1").expect("a revision");
        let lookup = run_blocking(command_of(&reword_from(&dir, &middle))).expect("git runs");
        let editor = "f() { echo reworded > \"$1\"; }; f".to_string();
        let parents = String::from_utf8_lossy(&lookup.stdout).to_string();
        let job = reword_commit(&dir, &parents, Some(editor)).expect("a commit to reword");
        let mut request = command_of(&job).clone();
        request.env.extend(
            identity
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string())),
        );
        let output = run_blocking(&request).expect("git runs");
        let messages = text(git(&["log", "--format=%s"]));
        // `--autostash` puts the change back in the tree, not the index.
        let changed = text(git(&["diff", "HEAD", "--name-only"]));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(messages, "third\nreworded\nfirst", "only the middle one");
        assert_eq!(changed, "a", "the uncommitted change is put back");
    }

    #[test]
    fn test_a_reword_needs_a_whole_commit_name() {
        assert!(reword_commit(Path::new("/r"), "abc123", None).is_none());
        assert!(reword_commit(Path::new("/r"), "", None).is_none());
        let full = "a".repeat(PLAN_HASH_LEN);
        let job = reword_commit(Path::new("/r"), &full, None).expect("a job");
        assert!(
            command_of(&job).args.contains(&"--root".to_string()),
            "no parent: the root"
        );
    }

    #[cfg(unix)]
    #[test]
    fn test_rewording_changes_the_message_and_leaves_the_staged_change_out() {
        // An amend would take what is staged into the commit; a reword is
        // only ever about the message.
        let dir = std::env::temp_dir().join(format!("winter-exec-reword-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let git = |args: &[&str]| {
            Command::new("git")
                .args(["-c", "user.name=t", "-c", "user.email=t@t"])
                .args(args)
                .current_dir(&dir)
                .output()
                .expect("git runs")
        };
        git(&["init", "-q"]);
        std::fs::write(dir.join("a"), "one").expect("write");
        git(&["add", "a"]);
        git(&["commit", "-qm", "first draft"]);
        std::fs::write(dir.join("b"), "two").expect("write");
        git(&["add", "b"]);

        let editor = "f() { echo reworded > \"$1\"; }; f".to_string();
        let JobRequest::Command(mut request) = commit(&dir, CommitMode::Reword, Some(editor))
        else {
            panic!("a command");
        };
        // The identity a commit needs, which this machine's config may lack.
        request.env.extend([
            ("GIT_AUTHOR_NAME".to_string(), "t".to_string()),
            ("GIT_AUTHOR_EMAIL".to_string(), "t@t".to_string()),
            ("GIT_COMMITTER_NAME".to_string(), "t".to_string()),
            ("GIT_COMMITTER_EMAIL".to_string(), "t@t".to_string()),
        ]);
        let output = run_blocking(&request).expect("git runs");
        let message = String::from_utf8_lossy(&git(&["log", "-1", "--format=%s"]).stdout)
            .trim()
            .to_string();
        let in_commit = String::from_utf8_lossy(&git(&["show", "--name-only", "--format="]).stdout)
            .trim()
            .to_string();
        let staged = String::from_utf8_lossy(&git(&["diff", "--cached", "--name-only"]).stdout)
            .trim()
            .to_string();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(message, "reworded");
        assert_eq!(in_commit, "a", "the commit holds what it held");
        assert_eq!(staged, "b", "and what was staged is still staged");
    }
}

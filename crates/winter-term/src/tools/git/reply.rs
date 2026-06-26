//! What each finished git command means for the view.

use std::path::PathBuf;

use crate::model::tool::{CommandOutput, JobRequest, Notice, ToolOutcome};

use super::commit::CommitContent;
use super::exec::{self, Pathspec};
use super::{diff, parse, progress, rows, watch};
use super::{now_unix, GitTool, COMMIT_TITLE_HASH_LEN, NOTHING_TO_SHOW};

// ========================================================================
// GitTool: replies
// ========================================================================

impl GitTool {
    /// What a finished command means for the view.
    pub(super) fn on_command(&mut self, output: CommandOutput) -> ToolOutcome {
        // The upstream read fails on a branch that tracks nothing, which is
        // its answer rather than a failure: the chain carries on past it.
        // So does the push menu's read of the config, which finds nothing to
        // print in a repository with no remotes and says so by failing.
        let answered = matches!(output.tag, exec::TAG_UNPULLED | exec::TAG_PUSH_CONTEXT);
        if !exec::succeeded(&output) && !answered {
            // Whichever read failed, nothing is coming to fill the view in, and
            // the placeholder would hide the failure it is waiting out.
            self.loaded = true;
            self.watching = None;
            self.notice = Some(Notice::error(format!("failed: {}", output.failure())));
            return ToolOutcome::Consumed;
        }
        match output.tag {
            exec::TAG_ROOT => {
                // Two lines: the working tree, then where the repository keeps
                // its own files, which a worktree or a submodule puts outside
                // the tree entirely.
                let mut lines = output.stdout.lines();
                let root = PathBuf::from(lines.next().unwrap_or_default().trim());
                self.git_dir = lines
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from);
                self.root = Some(root.clone());
                if !self.staged_only {
                    self.tree_watch_starting = Some(watch::TreeWatch::start_in_background(
                        &root,
                        self.git_dir.as_deref(),
                    ));
                }
                if self.staged_only {
                    return ToolOutcome::Job(exec::diff_all(&root, true, None));
                }
                ToolOutcome::Job(exec::status(&root))
            }
            exec::TAG_STATUS => {
                let watched = self.watching.take();
                // A read made only to notice a change to the files redraws
                // nothing when it finds none. An open diff is read from the
                // files themselves, which the status does not show changing,
                // so it always reads on; and a change inside git's directory
                // (a commit, a fetch, a stash) reads everything whatever the
                // status says, since none of it shows there.
                let unchanged = self.last_status.as_deref() == Some(output.stdout.as_str());
                self.last_status = Some(output.stdout.clone());
                let files_only = watched == Some(watch::Read::Status);
                if files_only && unchanged && self.expanded.is_empty() {
                    return ToolOutcome::Consumed;
                }
                self.status = parse::parse_status(&output.stdout);
                self.loaded = true;
                // A diff read before the change is now wrong, and staging a
                // hunk from a stale diff applies it to the wrong lines.
                self.diffs.clear();
                self.forget_unlisted_files();
                self.rebuild();
                // Files changing changes nothing past the status but the open
                // diffs: the commits, the stashes, and the refs are as they were.
                if files_only {
                    return self.request_stale_diff();
                }
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::recent_log(root, self.recent_count)),
                    None => ToolOutcome::Consumed,
                }
            }
            exec::TAG_RESOLVE => match (&self.root, self.resolving.take()) {
                (Some(root), Some(path)) => {
                    self.notice = Some(Notice::info(format!("resolved {path}")));
                    ToolOutcome::Job(exec::stage(root, &[Pathspec::new(path)]))
                }
                _ => ToolOutcome::Consumed,
            },
            exec::TAG_DIFF | exec::TAG_DIFF_UNTRACKED => {
                if let Some(file) = self.pending_diff.take() {
                    self.diffs
                        .insert(file.clone(), diff::parse_diff(&output.stdout));
                    self.rebuild();
                }
                self.request_stale_diff()
            }
            // The status refresh is a chain: each read asks for the next, so
            // the view fills in from one pass rather than firing five jobs at
            // once and racing them.
            exec::TAG_LOG => {
                let mut commits = parse::parse_decorated_log(&output.stdout);
                self.more_commits = commits.len() > self.recent_count;
                commits.truncate(self.recent_count);
                self.commits = commits;
                self.rebuild();
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::stash_entries(root)),
                    None => ToolOutcome::Consumed,
                }
            }
            exec::TAG_PUSH_CONTEXT => self.open_push_menu(&output.stdout),
            exec::TAG_PUSH_REMOTE_SET => match self.root.clone() {
                Some(root) => self.push_after_setting_remote(&root),
                None => ToolOutcome::Consumed,
            },
            exec::TAG_CANDIDATES => {
                let Some(question) = self.pending_pick.take() else {
                    return ToolOutcome::Consumed;
                };
                let listed = self
                    .push_destinations(question.tag, &output.stdout)
                    .unwrap_or_else(|| output.stdout.clone());
                // A remote's own `HEAD` is a pointer at one of the names
                // already listed, never an answer of its own.
                match question.over_lines(&listed, |name| !name.ends_with("/HEAD")) {
                    Some(request) => ToolOutcome::Pick(request),
                    // With nothing to choose from there is still a question
                    // to answer, so it falls back to being typed.
                    None => self.ask(question.tag, &format!("{}: ", question.label)),
                }
            }
            exec::TAG_STASHES => {
                self.stashes = parse::parse_stash_list(&output.stdout);
                self.rebuild();
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::unpulled(root)),
                    None => ToolOutcome::Consumed,
                }
            }
            exec::TAG_UNPULLED => {
                // A branch tracking nothing makes this fail, which is not a
                // failure worth reporting: it is behind nothing.
                self.unpulled = match exec::succeeded(&output) {
                    true => parse::parse_decorated_log(&output.stdout),
                    false => Vec::new(),
                };
                self.rebuild();
                match &self.git_dir {
                    Some(dir) => {
                        ToolOutcome::Job(JobRequest::ReadFiles(progress::state_files(dir)))
                    }
                    None => ToolOutcome::Consumed,
                }
            }
            // A blame and a plain read differ only in what the view is called.
            exec::TAG_BLAME | exec::TAG_READ => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    let title = if output.tag == exec::TAG_BLAME {
                        "Blame"
                    } else {
                        "Output"
                    };
                    let blamed = (output.tag == exec::TAG_BLAME)
                        .then(|| self.pending_blame.take())
                        .flatten();
                    self.show_output(title, &output.stdout, false);
                    if let Some(output) = self.output.as_mut() {
                        output.blamed = blamed;
                    }
                }
                ToolOutcome::Consumed
            }
            // A whole diff fills the view decorated, line for line.
            exec::TAG_DIFF_VIEW => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    self.show_painted("Diff", rows::diff_view_rows(&lines));
                }
                ToolOutcome::Consumed
            }
            // The log view lists commits the way the status tail does, rather
            // than as the raw lines git wrote, so both read the same.
            exec::TAG_LOG_VIEW => {
                let mut commits = parse::parse_decorated_log(&output.stdout);
                if commits.is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    // The commit past the page only says whether there are
                    // more: shown, it would be one the next page repeats.
                    let more = commits.len() > self.log_count;
                    commits.truncate(self.log_count);
                    let painted = rows::log_rows(&commits, self.root.as_deref(), now_unix(), more);
                    self.show_painted("Log", painted);
                    // The load-more key only means anything while a log shows.
                    if let Some(output) = &mut self.output {
                        output.more = more;
                    }
                }
                ToolOutcome::Consumed
            }
            // A commit fills the view as its content: the summary it was shown
            // with, then its files banded and their hunks decorated, the way
            // the working tree reads.
            exec::TAG_SHOW => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    let title = commit_title(&output.stdout);
                    self.show_commit(&title, CommitContent::parse(&lines));
                }
                ToolOutcome::Consumed
            }
            exec::TAG_REBASE_FROM => match self.root.clone() {
                Some(root) => {
                    let args = exec::interactive_rebase_args(&output.stdout);
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    self.spawn_git(&root, &args)
                }
                None => ToolOutcome::Consumed,
            },
            exec::TAG_REWORD_FROM => {
                let Some(root) = self.root.clone() else {
                    return ToolOutcome::Consumed;
                };
                let editor = crate::terminal::session::git_editor_command();
                match exec::reword_commit(&root, &output.stdout, editor) {
                    Some(job) => ToolOutcome::Job(job),
                    None => {
                        self.notice = Some(Notice::error("no commit there to reword"));
                        ToolOutcome::Consumed
                    }
                }
            }
            exec::TAG_REMOTE_URL => {
                let url = browser_url(output.stdout.trim());
                match url {
                    Some(url) => ToolOutcome::OpenExternal(std::path::PathBuf::from(url)),
                    None => {
                        self.notice = Some(Notice::error("no web URL for that remote"));
                        ToolOutcome::Consumed
                    }
                }
            }
            // Anything that changed the repository: report it and re-read
            // what is showing, the output if one is up.
            tag => {
                self.notice = report_for(tag, &output).map(Notice::info);
                self.reread()
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Name the view showing one commit, from the `git show` output itself.
///
/// The abbreviated hash and the subject, which is what identifies a commit to a
/// reader. Taken from the output rather than from the row the cursor was on, so
/// the title cannot disagree with what is underneath it.
fn commit_title(shown: &str) -> String {
    let mut hash = String::new();
    let mut subject = String::new();
    for line in shown.lines() {
        match line.strip_prefix("commit ") {
            Some(rest) if hash.is_empty() => {
                hash = rest.split_whitespace().next().unwrap_or(rest).to_string();
                hash.truncate(COMMIT_TITLE_HASH_LEN);
            }
            // The subject is the first indented line of the message block, which
            // is the first thing after the headers that is neither blank nor a
            // header of its own.
            _ => {
                if !hash.is_empty() && subject.is_empty() {
                    if let Some(text) = line.strip_prefix("    ") {
                        subject = text.trim().to_string();
                    }
                }
            }
        }
    }
    match (hash.is_empty(), subject.is_empty()) {
        (true, _) => "Commit".to_string(),
        (false, true) => format!("Commit {hash}"),
        (false, false) => format!("Commit {hash}{}{subject}", super::SUBJECT_GAP),
    }
}

/// Turn a git remote URL into one a browser can open: an `ssh` remote becomes
/// its `https` form, and anything else is left alone if it already is one.
pub(super) fn browser_url(remote: &str) -> Option<String> {
    if remote.starts_with("http://") || remote.starts_with("https://") {
        return Some(remote.trim_end_matches(".git").to_string());
    }
    // `git@host:owner/repo.git`, the form every forge hands out.
    let rest = remote.strip_prefix("git@")?;
    let (host, path) = rest.split_once(':')?;
    Some(format!("https://{host}/{}", path.trim_end_matches(".git")))
}

/// What to say in the header after a command that changed something, or
/// `None` where the re-read view already says it: staging and unstaging move
/// the file between the section headings, and discarding drops it from the
/// list.
fn report_for(tag: &'static str, output: &CommandOutput) -> Option<String> {
    let detail = output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match tag {
        exec::TAG_FETCH => Some("fetched".to_string()),
        exec::TAG_PULL => Some(format!("pulled: {detail}")),
        exec::TAG_PUSH => Some("pushed".to_string()),
        exec::TAG_DISCARD | exec::TAG_STAGE | exec::TAG_UNSTAGE => None,
        _ => Some(detail.to_string()),
    }
}

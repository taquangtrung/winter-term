//! What each finished git command means for the view.

use std::path::PathBuf;

use crate::editors::git_exec::{self, Pathspec};
use crate::model::editor::{CommandOutput, EditorOutcome, JobRequest, Notice};

use super::commit::CommitContent;
use super::requests::{Asked, DiffAsked, SpinOffAsked};
use super::{diff, progress, rows, watch};
use crate::editors::git_parse;
use super::{now_unix, GitEditor, COMMIT_TITLE_HASH_LEN, NOTHING_TO_SHOW, NO_DIFFERENCES, REFS_TITLE};

// ========================================================================
// GitEditor: replies
// ========================================================================

impl GitEditor {
    /// What a finished command means for the view.
    pub(super) fn on_command(&mut self, output: CommandOutput) -> EditorOutcome {
        // What this request was asked for, taken whatever the answer: a
        // request that failed is no longer waited on either.
        let asked = self.in_flight.answer(output.id);
        // The upstream read fails on a branch that tracks nothing, which is
        // its answer rather than a failure: the chain carries on past it.
        // So does the push menu's read of the config, which finds nothing to
        // print in a repository with no remotes and says so by failing.
        // And reading `HEAD`'s copy fails for a file the last commit does not
        // have, which the diff tool compares with nothing.
        let answered = matches!(
            output.tag,
            git_exec::TAG_UNPULLED | git_exec::TAG_PUSH_CONTEXT | git_exec::TAG_SHOW_HEAD
        );
        if !git_exec::succeeded(&output) && !answered {
            // Whichever read failed, nothing is coming to fill the view in, and
            // the placeholder would hide the failure it is waiting out.
            self.repo.loaded = true;
            self.watch.watching = None;
            self.notice = Some(Notice::error(format!("failed: {}", output.failure())));
            return EditorOutcome::Consumed;
        }
        match output.tag {
            git_exec::TAG_ROOT => {
                // Two lines: the working tree, then where the repository keeps
                // its own files, which a worktree or a submodule puts outside
                // the tree entirely.
                let mut lines = output.stdout.lines();
                let root = PathBuf::from(lines.next().unwrap_or_default().trim());
                self.repo.git_dir = lines
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from);
                self.repo.root = Some(root.clone());
                if !self.staged_only {
                    self.watch.tree_watch_starting = Some(watch::TreeWatch::start_in_background(
                        &root,
                        self.repo.git_dir.as_deref(),
                    ));
                }
                if self.staged_only {
                    return EditorOutcome::Job(git_exec::diff_all(&root, true, None));
                }
                EditorOutcome::Job(git_exec::status(&root))
            }
            git_exec::TAG_STATUS => {
                let watched = self.watch.watching.take();
                // A read made only to notice a change to the files redraws
                // nothing when it finds none. An open diff is read from the
                // files themselves, which the status does not show changing,
                // so it always reads on; and a change inside git's directory
                // (a commit, a fetch, a stash) reads everything whatever the
                // status says, since none of it shows there.
                let unchanged = self.repo.last_status.as_deref() == Some(output.stdout.as_str());
                self.repo.last_status = Some(output.stdout.clone());
                let files_only = watched == Some(watch::Read::Status);
                if files_only && unchanged && self.expanded.is_empty() {
                    return EditorOutcome::Consumed;
                }
                self.repo.status = git_parse::parse_status(&output.stdout);
                self.repo.loaded = true;
                // A diff read before the change is now wrong, and staging a
                // hunk from a stale diff applies it to the wrong lines.
                self.repo.diffs.clear();
                self.repo.status_reads += 1;
                self.forget_unlisted_files();
                self.rebuild();
                // Files changing changes nothing past the status but the open
                // diffs: the commits, the stashes, and the refs are as they were.
                if files_only {
                    return self.request_stale_diff();
                }
                match &self.repo.root {
                    Some(root) => EditorOutcome::Job(git_exec::recent_log(root, self.recent_count)),
                    None => EditorOutcome::Consumed,
                }
            }
            git_exec::TAG_RESOLVE => match (&self.repo.root, asked) {
                (Some(root), Some(Asked::Resolve(path))) => {
                    self.notice = Some(Notice::info(format!("resolved {path}")));
                    EditorOutcome::Job(git_exec::stage(root, &[Pathspec::new(path)]))
                }
                _ => EditorOutcome::Consumed,
            },
            git_exec::TAG_DIFF | git_exec::TAG_DIFF_UNTRACKED => {
                // An answer asked for before the status last changed is
                // dropped; the file is still open, so it is asked for again.
                if let Some(Asked::Diff(DiffAsked { file, under })) = asked {
                    if under == self.repo.status_reads {
                        self.repo.diffs.insert(file, diff::parse_diff(&output.stdout));
                        self.rebuild();
                    }
                }
                self.request_stale_diff()
            }
            // The status refresh is a chain: each read asks for the next, so
            // the view fills in from one pass rather than firing five jobs at
            // once and racing them.
            git_exec::TAG_LOG => {
                let mut commits = git_parse::parse_decorated_log(&output.stdout);
                self.repo.more_commits = commits.len() > self.recent_count;
                commits.truncate(self.recent_count);
                self.repo.commits = commits;
                self.rebuild();
                match &self.repo.root {
                    Some(root) => EditorOutcome::Job(git_exec::stash_entries(root)),
                    None => EditorOutcome::Consumed,
                }
            }
            git_exec::TAG_PUSH_CONTEXT => self.open_push_menu(&output.stdout),
            git_exec::TAG_PUSH_REMOTE_SET => match self.repo.root.clone() {
                Some(root) => self.push_after_setting_remote(&root),
                None => EditorOutcome::Consumed,
            },
            git_exec::TAG_CANDIDATES => {
                let Some(question) = self.menus.pending_pick.take() else {
                    return EditorOutcome::Consumed;
                };
                let listed = self
                    .push_destinations(question.tag, &output.stdout)
                    .unwrap_or_else(|| output.stdout.clone());
                // A remote's own `HEAD` is a pointer at one of the names
                // already listed, never an answer of its own.
                match question.over_lines(&listed, |name| !name.ends_with("/HEAD")) {
                    Some(request) => EditorOutcome::Pick(request),
                    // With nothing to choose from there is still a question
                    // to answer, so it falls back to being typed.
                    None => self.ask(question.tag, &format!("{}: ", question.label)),
                }
            }
            git_exec::TAG_STASHES => {
                self.repo.stashes = git_parse::parse_stash_list(&output.stdout);
                self.rebuild();
                match &self.repo.root {
                    Some(root) => EditorOutcome::Job(git_exec::remotes(root)),
                    None => EditorOutcome::Consumed,
                }
            }
            git_exec::TAG_REMOTES => {
                self.repo.remotes = output.stdout.lines().map(str::trim).filter(|name| !name.is_empty()).map(str::to_string).collect();
                match &self.repo.root {
                    Some(root) => EditorOutcome::Job(git_exec::unpulled(root)),
                    None => EditorOutcome::Consumed,
                }
            }
            git_exec::TAG_UNPULLED => {
                // A branch tracking nothing makes this fail, which is not a
                // failure worth reporting: it is behind nothing.
                self.repo.unpulled = match git_exec::succeeded(&output) {
                    true => git_parse::parse_decorated_log(&output.stdout),
                    false => Vec::new(),
                };
                self.rebuild();
                match &self.repo.git_dir {
                    Some(dir) => {
                        EditorOutcome::Job(JobRequest::ReadFiles(progress::state_files(dir)))
                    }
                    None => EditorOutcome::Consumed,
                }
            }
            // A blame and a plain read differ only in what the view is called.
            git_exec::TAG_BLAME | git_exec::TAG_READ => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    let title = if output.tag == git_exec::TAG_BLAME {
                        "Blame"
                    } else {
                        "Output"
                    };
                    let blamed = match asked {
                        Some(Asked::Blame(path)) => Some(path),
                        _ => None,
                    };
                    self.show_output(title, &output.stdout, false);
                    if let Some(output) = self.output.as_mut() {
                        output.blamed = blamed;
                    }
                }
                EditorOutcome::Consumed
            }
            // A whole diff fills the view decorated, line for line.
            // A comparison fills the view as a commit does, opened all the
            // way, since it is the whole point of the view; two files that
            // are the same say so in place of a diff.
            git_exec::TAG_SHOW_HEAD => match asked {
                Some(Asked::Head(path)) => self.head_read(&output, path),
                _ => EditorOutcome::Consumed,
            },
            // The branches and tags fill their view a row a ref, each one
            // to act on.
            git_exec::TAG_REFS => {
                let rows = rows::ref_rows(&output.stdout);
                match rows.is_empty() {
                    true => self.notice = Some(Notice::error(NOTHING_TO_SHOW)),
                    false => self.show_painted(REFS_TITLE, rows),
                }
                EditorOutcome::Consumed
            }
            git_exec::TAG_COMPARE => {
                self.repo.loaded = true;
                let title = self
                    .comparison
                    .as_ref()
                    .map(|comparison| comparison.title().to_string())
                    .unwrap_or_default();
                if output.code == Some(0) {
                    self.show_output(&title, NO_DIFFERENCES, false);
                    return EditorOutcome::Consumed;
                }
                let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                self.show_commit(&title, CommitContent::parse(&lines));
                self.open_whole_commit();
                EditorOutcome::Consumed
            }
            git_exec::TAG_DIFF_VIEW => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    self.show_painted("Diff", rows::diff_view_rows(&lines));
                }
                EditorOutcome::Consumed
            }
            // The log view lists commits the way the status tail does, rather
            // than as the raw lines git wrote, so both read the same.
            git_exec::TAG_LOG_VIEW => {
                let mut commits = git_parse::parse_decorated_log(&output.stdout);
                if commits.is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    // The commit past the page only says whether there are
                    // more: shown, it would be one the next page repeats.
                    let more = commits.len() > self.log_count;
                    commits.truncate(self.log_count);
                    let painted = rows::log_rows(
                        &commits,
                        self.repo.root.as_deref(),
                        now_unix(),
                        more,
                        (self.cols > 0).then_some(self.cols),
                    );
                    self.show_painted("Log", painted);
                    // The load-more key only means anything while a log shows.
                    if let Some(output) = &mut self.output {
                        output.more = more;
                        output.log_commits = Some(commits);
                    }
                }
                EditorOutcome::Consumed
            }
            // A commit fills the view as its content: the summary it was shown
            // with, then its files banded and their hunks decorated, the way
            // the working tree reads.
            git_exec::TAG_SHOW => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(Notice::error(NOTHING_TO_SHOW));
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    let title = commit_title(&output.stdout);
                    self.show_commit(&title, CommitContent::parse(&lines));
                }
                EditorOutcome::Consumed
            }
            git_exec::TAG_REBASE_FROM => match self.repo.root.clone() {
                Some(root) => {
                    let args = git_exec::interactive_rebase_args(&output.stdout);
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    self.spawn_git(&root, &args)
                }
                None => EditorOutcome::Consumed,
            },
            git_exec::TAG_REWORD_FROM => {
                let Some(root) = self.repo.root.clone() else {
                    return EditorOutcome::Consumed;
                };
                let editor = crate::terminal::session::git_editor_command();
                match git_exec::reword_commit(&root, &output.stdout, editor) {
                    Some(job) => EditorOutcome::Job(job),
                    None => {
                        self.notice = Some(Notice::error("no commit there to reword"));
                        EditorOutcome::Consumed
                    }
                }
            }
            // The new branch is checked out: put the one left behind back
            // where its upstream is.
            git_exec::TAG_SPIN_OFF => match (self.repo.root.clone(), asked) {
                (Some(root), Some(Asked::SpinOff(SpinOffAsked { from, upstream }))) => {
                    EditorOutcome::Job(git_exec::branch_reset(&root, &from, &upstream))
                }
                _ => self.reread(),
            },
            // The fixup is made: find where the commit it fixes starts from,
            // then fold it in from there.
            git_exec::TAG_INSTANT_FIXUP => match (self.repo.root.clone(), asked) {
                (Some(root), Some(Asked::Squash(rev))) => EditorOutcome::Job(git_exec::squash_from(&root, &rev)),
                _ => self.reread(),
            },
            git_exec::TAG_SQUASH_FROM => match self.repo.root.clone() {
                Some(root) => EditorOutcome::Job(git_exec::autosquash(&root, &output.stdout)),
                None => self.reread(),
            },
            git_exec::TAG_REMOTE_URL => {
                let url = browser_url(output.stdout.trim());
                match url {
                    Some(url) => EditorOutcome::OpenExternal(std::path::PathBuf::from(url)),
                    None => {
                        self.notice = Some(Notice::error("no web URL for that remote"));
                        EditorOutcome::Consumed
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
        git_exec::TAG_FETCH => Some("fetched".to_string()),
        git_exec::TAG_PULL => Some(format!("pulled: {detail}")),
        git_exec::TAG_PUSH => Some("pushed".to_string()),
        git_exec::TAG_DISCARD | git_exec::TAG_STAGE | git_exec::TAG_UNSTAGE => None,
        _ => Some(detail.to_string()),
    }
}

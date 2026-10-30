//! Pushing, pulling, branches and refs, stashes, and making commits and fixups.

use std::path::Path;

use crate::editors::git_exec::{self, CommitMode, LogScope, Rev};
use crate::editors::git_parse::{RefKind, Section};
use crate::model::editor::{EditorOutcome, JobRequest, Notice};

use super::popup::Popup;
use super::requests::{Asked, SpinOffAsked};
use super::rows::{Item, RefRow};
use super::{Discard, GitEditor, ASK_FIXUP, ASK_INSTANT_FIXUP, DEFAULT_REMOTE, LOG_PAGE};

// ========================================================================
// GitEditor: branches, remotes, and commits
// ========================================================================

impl GitEditor {
    /// `P`: open the push menu, once where the branch pushes and which
    /// remotes there are has been read, since the menu names them.
    pub(super) fn push(&mut self) -> EditorOutcome {
        match &self.repo.root {
            Some(root) => {
                EditorOutcome::Job(git_exec::push_context(root, self.repo.status.branch.as_deref()))
            }
            None => EditorOutcome::Consumed,
        }
    }

    /// `C-S-p`: push the current branch without asking where. It goes to
    /// the remote the branch already tracks, else `origin`, and is set to track
    /// it, so a branch the remote has never seen is created there.
    pub(super) fn quick_push(&mut self) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let Some(branch) = self.repo.status.branch.clone() else {
            self.notice = Some(Notice::error("no branch to push"));
            return EditorOutcome::Consumed;
        };
        // To the branch it tracks, by that branch's name: pushed under its
        // own name, a `feat` tracking `origin/feature` made a new
        // `origin/feat` and moved its upstream there.
        let (remote, refspec) = match self.repo.upstream() {
            Some((remote, tracked)) if tracked != branch => (remote, format!("{branch}:{tracked}")),
            Some((remote, _)) => (remote, branch),
            None => (DEFAULT_REMOTE, branch),
        };
        EditorOutcome::Job(git_exec::push_upstream(&root, remote, &refspec))
    }

    /// `C-S-M-f`: pull the current branch with rebase without asking where.
    /// It comes from the branch the current one tracks, else the branch of the
    /// same name on `origin`, so a branch that tracks nothing still pulls.
    /// The branch or tag the cursor is on in the list of them.
    pub(super) fn ref_at_cursor(&self) -> Option<RefRow> {
        let output = self.output.as_ref()?;
        match &output.rows.get(output.list.cursor())?.item {
            Item::Ref(entry) => Some(entry.clone()),
            _ => None,
        }
    }

    /// `x` over a branch or a tag: delete it, once confirmed.
    pub(super) fn ask_delete_ref(&mut self) -> EditorOutcome {
        let Some(entry) = self.ref_at_cursor() else {
            return EditorOutcome::Consumed;
        };
        self.menus.pending_discard = Some(Discard::Ref(entry));
        self.ask_discard_pending()
    }

    /// What deletes `entry`: a local branch only when it is merged, a remote
    /// one by pushing its deletion there, a tag.
    pub(super) fn delete_ref(&self, root: &Path, entry: &RefRow) -> Option<JobRequest> {
        match entry.kind {
            RefKind::Head | RefKind::Local => {
                Some(git_exec::branch_delete(root, &Rev::parse(&entry.name).ok()?, false))
            }
            RefKind::Remote => {
                let (remote, branch) = self.repo.split_remote(&entry.name)?;
                Some(git_exec::push_delete(root, remote, branch))
            }
            RefKind::Tag => Some(git_exec::tag_delete(root, &Rev::parse(&entry.name).ok()?)),
        }
    }

    /// Spin off a branch named `name`: made at HEAD and checked out, the
    /// branch left put back where its upstream is, as magit's spin-off does.
    /// A branch tracking nothing stays where it is.
    pub(super) fn spin_off(&mut self, root: &Path, name: &Rev) -> EditorOutcome {
        let from = self.repo.status.branch.as_deref().and_then(|branch| Rev::parse(branch).ok());
        let upstream = self.repo.status.upstream.as_deref().and_then(|upstream| Rev::parse(upstream).ok());
        let job = git_exec::spin_off(root, name);
        match from.zip(upstream) {
            Some((from, upstream)) => EditorOutcome::Job(self.in_flight.ask(job, Asked::SpinOff(SpinOffAsked { from, upstream }))),
            None => EditorOutcome::Job(job),
        }
    }

    /// The stash the cursor is on, as git names it: `stash@{2}`.
    pub(super) fn stash_at_cursor(&self) -> Option<Rev> {
        let Some(Item::Stash(index)) = self.selected().map(|row| row.item.clone()) else {
            return None;
        };
        Rev::parse(&self.repo.stashes.get(index)?.name).ok()
    }

    pub(super) fn quick_pull(&mut self) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let Some(branch) = self.repo.status.branch.clone() else {
            self.notice = Some(Notice::error("no branch to pull"));
            return EditorOutcome::Consumed;
        };
        let (remote, source) = self.repo.upstream().unwrap_or((DEFAULT_REMOTE, &branch));
        EditorOutcome::Job(git_exec::pull_from(&root, remote, source))
    }

    /// Commit what is staged, writing the message in the editor: the commit
    /// popup's `c`, on a chord of its own. Nothing staged is refused up front,
    /// since `git commit` with an empty index only fails with a wall of advice.
    pub(super) fn commit_in_editor(&mut self) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        if self.repo.paths_in(Section::Staged).is_empty() {
            self.notice = Some(Notice::error("nothing staged to commit"));
            return EditorOutcome::Consumed;
        }
        self.commit(&root, CommitMode::New)
    }

    /// Commit as `mode` says, writing the message in the editor tab git waits
    /// on, with no terminal tab of its own.
    pub(super) fn commit(&self, root: &std::path::Path, mode: CommitMode) -> EditorOutcome {
        let editor = crate::terminal::session::git_editor_command();
        EditorOutcome::Job(git_exec::commit(root, mode, &self.menus.flags_for(Popup::Commit), editor))
    }

    /// A log of `scope` from its first page, with the log menu's switches
    /// and `more` flags after them.
    pub(super) fn log_of(&mut self, root: &Path, scope: LogScope, more: &[String]) -> EditorOutcome {
        self.log_count = LOG_PAGE;
        let mut flags: Vec<String> = self.menus.flags_for(Popup::Log).into_iter().map(str::to_string).collect();
        flags.extend_from_slice(more);
        EditorOutcome::Job(git_exec::log(root, scope, self.log_count, &flags))
    }

    /// `c f` and `c F`: a fixup of the commit at the cursor, else of one
    /// asked for; `F` folds it in at once.
    pub(super) fn fixup(&mut self, root: &Path, instant: bool) -> EditorOutcome {
        let tag = if instant { ASK_INSTANT_FIXUP } else { ASK_FIXUP };
        match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
            Some(Ok(rev)) => self.fixup_of(root, &rev, instant),
            Some(Err(_)) | None => self.ask(tag, "Fixup: "),
        }
    }

    /// Commit what is staged as a fixup of `rev`, and with `instant`, fold
    /// it in once that is done.
    pub(super) fn fixup_of(&mut self, root: &Path, rev: &Rev, instant: bool) -> EditorOutcome {
        let flags = self.menus.flags_for(Popup::Commit);
        match instant {
            true => {
                let job = git_exec::fixup(root, rev, &flags, git_exec::TAG_INSTANT_FIXUP);
                EditorOutcome::Job(self.in_flight.ask(job, Asked::Squash(rev.clone())))
            }
            false => EditorOutcome::Job(git_exec::fixup(root, rev, &flags, git_exec::TAG_CHANGED)),
        }
    }
}

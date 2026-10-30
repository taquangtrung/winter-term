//! The questions the view asks, and what each answer runs.

use std::path::Path;

use crate::editors::git_exec::{self, LogScope, ResetMode, Rev};
use crate::model::editor::{
    EditorOutcome, Notice, PickQuestion, PromptMode, PromptReply, PromptRequest,
};

use super::popup::Popup;
use super::{
    GitEditor, ASK_BRANCH_CHECKOUT, ASK_BRANCH_CREATE, ASK_BRANCH_CREATE_HERE, ASK_BRANCH_DELETE,
    ASK_BRANCH_DELETE_FORCE, ASK_CHERRY_PICK, ASK_CUSTOM, ASK_DIFF_REV, ASK_DISCARD,
    ASK_BRANCH_RENAME, ASK_DIFF_BRANCH, ASK_DIFF_RANGE, ASK_MERGE_EDIT, ASK_MERGE_NO_COMMIT,
    ASK_MERGE_SQUASH, ASK_SHOW_COMMIT, ASK_SPIN_OFF,
    ASK_FETCH_REMOTE, ASK_FIXUP, ASK_INSTANT_FIXUP, ASK_LOG_OTHER, ASK_LOG_PICKAXE, ASK_LOG_REGEX, ASK_MERGE, ASK_PULL_FROM,
    ASK_REBASE, ASK_REMOTE_ADD, ASK_REMOTE_PRUNE, ASK_REMOTE_REMOVE, ASK_RESET_HARD,
    ASK_RESET_INDEX, ASK_RESET_MIXED, ASK_RESET_SOFT, ASK_RESET_WORKTREE, ASK_REVERT, ASK_SEARCH, ASK_STASH, ASK_TAG_CREATE,
    ASK_SUBMODULE_ADD, ASK_SUBMODULE_DEINIT, ASK_TAG_DELETE, ASK_WORKTREE_ADD,
    ASK_WORKTREE_REMOVE,
};

// ========================================================================
// GitEditor: questions
// ========================================================================

impl GitEditor {
    /// Ask the same question [`ask`](Self::ask) does, but over the names it
    /// can be answered with: the reader filters a list instead of spelling a
    /// branch out. The names are read first, so this hands back the job that
    /// reads them and the answer comes through the same tag either way.
    pub(super) fn pick(
        &mut self,
        tag: &'static str,
        label: &str,
        kind: git_exec::Candidates,
    ) -> EditorOutcome {
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        self.menus.pending_pick = Some(PickQuestion::new(tag, label));
        EditorOutcome::Job(git_exec::candidates(&root, kind))
    }

    /// Ask a one-line question, tagged so the answer finds its way back.
    pub(super) fn ask(&self, tag: &'static str, label: &str) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag,
        })
    }

    /// Ask what to move to, highlighting what it matches as it is typed.
    pub(super) fn ask_search(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH))
    }

    /// Run what an answered question asks for.
    pub(super) fn answer_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.release_wheel();
        let Some(answer) = reply.answer else {
            // A discard turned down is gone with the question that asked.
            if reply.tag == ASK_DISCARD {
                self.menus.pending_discard = None;
            }
            self.notice = Some(Notice::info("cancelled"));
            return EditorOutcome::Consumed;
        };
        // A search is the one question that asks nothing of git, so it is
        // answered before the repository is required.
        if reply.tag == ASK_SEARCH {
            return self.search_for(&answer);
        }
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let answer = answer.trim().to_string();
        match reply.tag {
            // A confirmation's answer is only its yes; what it confirms was
            // decided when it was asked.
            ASK_DISCARD => return self.discard(),
            // The one question an empty answer means something for.
            ASK_STASH => return EditorOutcome::Job(git_exec::stash_push(&root, &answer)),
            _ => {}
        }
        if answer.is_empty() {
            return EditorOutcome::Consumed;
        }
        if let Some(outcome) = self.answer_push(&root, reply.tag, &answer) {
            return outcome;
        }
        match reply.tag {
            ASK_CUSTOM => {
                let editor = crate::terminal::session::git_editor_command();
                match git_exec::custom(&root, &answer, editor) {
                    Ok(job) => EditorOutcome::Job(job),
                    Err(why) => {
                        self.notice = Some(Notice::error(format!("failed: {why}")));
                        EditorOutcome::Consumed
                    }
                }
            }
            ASK_REMOTE_ADD => match answer.split_once(char::is_whitespace) {
                Some((name, url)) => {
                    EditorOutcome::Job(git_exec::remote_add(&root, name.trim(), url.trim()))
                }
                None => {
                    self.notice = Some(Notice::error("failed: expected `name url`"));
                    EditorOutcome::Consumed
                }
            },
            ASK_FETCH_REMOTE => EditorOutcome::Job(git_exec::fetch_remote(&root, &answer)),
            // A pattern is the flag's own value, joined to it, so one that
            // starts with `-` is never read as an option of its own.
            ASK_LOG_REGEX => self.log_of(&root, LogScope::Branch, &[format!("-G{answer}")]),
            ASK_LOG_PICKAXE => self.log_of(&root, LogScope::Branch, &[format!("-S{answer}")]),
            // A remote branch is named as its remote and its branch, which a
            // pull takes apart: `origin/feature` is `origin` and `feature`.
            ASK_PULL_FROM => match self.repo.split_remote(&answer) {
                Some((remote, branch)) => EditorOutcome::Job(git_exec::pull_from(&root, remote, branch)),
                None => {
                    self.notice = Some(Notice::error("failed: expected `remote/branch`"));
                    EditorOutcome::Consumed
                }
            },
            ASK_REMOTE_PRUNE => EditorOutcome::Job(git_exec::remote_prune(&root, &answer)),
            ASK_REMOTE_REMOVE => EditorOutcome::Job(git_exec::remote_remove(&root, &answer)),
            ASK_SUBMODULE_ADD => {
                let mut words = answer.split_whitespace();
                match words.next() {
                    Some(url) => EditorOutcome::Job(git_exec::submodule_add(&root, url, words.next())),
                    None => EditorOutcome::Consumed,
                }
            }
            ASK_SUBMODULE_DEINIT => EditorOutcome::Job(git_exec::submodule_deinit(&root, &answer)),
            ASK_WORKTREE_ADD => EditorOutcome::Job(git_exec::worktree_add(&root, &answer)),
            ASK_WORKTREE_REMOVE => EditorOutcome::Job(git_exec::worktree_remove(&root, &answer)),
            // Everything else is answered with a revision or a ref's name,
            // which is refused when it could be read as an option instead.
            tag => match Rev::parse(&answer) {
                Ok(rev) => self.run_with_rev(&root, tag, &rev),
                Err(reason) => {
                    self.notice = Some(Notice::error(format!("failed: {reason}")));
                    EditorOutcome::Consumed
                }
            },
        }
    }

    /// Run the command a question answered with a revision asks for.
    fn run_with_rev(&mut self, root: &Path, tag: &'static str, rev: &Rev) -> EditorOutcome {
        let job = match tag {
            ASK_BRANCH_CHECKOUT => git_exec::checkout(root, rev),
            ASK_BRANCH_CREATE => git_exec::branch_create(root, rev, true),
            ASK_BRANCH_CREATE_HERE => git_exec::branch_create(root, rev, false),
            ASK_BRANCH_DELETE => git_exec::branch_delete(root, rev, false),
            ASK_BRANCH_DELETE_FORCE => git_exec::branch_delete(root, rev, true),
            ASK_CHERRY_PICK => git_exec::cherry_pick(root, rev),
            ASK_DIFF_REV => git_exec::diff_all(root, false, Some(rev)),
            ASK_MERGE => git_exec::merge(root, rev, &self.menus.flags_for(Popup::Merge)),
            ASK_MERGE_EDIT => git_exec::merge_with(
                root,
                rev,
                "--edit",
                &self.menus.flags_for(Popup::Merge),
                crate::terminal::session::git_editor_command(),
            ),
            ASK_MERGE_NO_COMMIT => git_exec::merge_with(root, rev, "--no-commit", &self.menus.flags_for(Popup::Merge), None),
            ASK_MERGE_SQUASH => git_exec::merge_with(root, rev, "--squash", &self.menus.flags_for(Popup::Merge), None),
            ASK_BRANCH_RENAME => git_exec::branch_rename(root, rev),
            ASK_SPIN_OFF => return self.spin_off(root, rev),
            ASK_DIFF_RANGE => git_exec::diff_all(root, false, Some(rev)),
            // What the branch has done since it left the other, as a pull
            // request would show it.
            ASK_DIFF_BRANCH => match Rev::parse(&format!("{}...HEAD", rev.as_str())) {
                Ok(range) => git_exec::diff_all(root, false, Some(&range)),
                Err(_) => return EditorOutcome::Consumed,
            },
            ASK_SHOW_COMMIT => git_exec::show_commit(root, rev),
            ASK_REBASE => {
                return self.spawn_git(root, &["rebase", "--interactive", rev.as_str()]);
            }
            ASK_FIXUP => return self.fixup_of(root, rev, false),
            ASK_LOG_OTHER => return self.log_of(root, LogScope::Other(rev.clone()), &[]),
            ASK_INSTANT_FIXUP => return self.fixup_of(root, rev, true),
            ASK_RESET_HARD => git_exec::reset(root, ResetMode::Hard, rev),
            ASK_RESET_INDEX => git_exec::reset_index(root, rev),
            ASK_RESET_WORKTREE => git_exec::reset_worktree(root, rev),
            ASK_RESET_MIXED => git_exec::reset(root, ResetMode::Mixed, rev),
            ASK_RESET_SOFT => git_exec::reset(root, ResetMode::Soft, rev),
            ASK_REVERT => git_exec::revert(root, rev),
            ASK_TAG_CREATE => git_exec::tag_create(root, rev),
            ASK_TAG_DELETE => git_exec::tag_delete(root, rev),
            _ => return EditorOutcome::Consumed,
        };
        EditorOutcome::Job(job)
    }
}

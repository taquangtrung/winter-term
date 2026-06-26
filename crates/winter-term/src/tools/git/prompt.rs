//! The questions the view asks, and what each answer runs.

use std::path::Path;

use crate::model::tool::{
    Notice, PickQuestion, PromptMode, PromptReply, PromptRequest, ToolOutcome,
};

use super::exec::{self, ResetMode, Rev};
use super::{
    GitTool, ASK_BRANCH_CHECKOUT, ASK_BRANCH_CREATE, ASK_BRANCH_CREATE_HERE, ASK_BRANCH_DELETE,
    ASK_BRANCH_DELETE_FORCE, ASK_CHERRY_PICK, ASK_CUSTOM, ASK_DIFF_REV, ASK_DISCARD, ASK_MERGE,
    ASK_REBASE, ASK_REMOTE_ADD, ASK_REMOTE_PRUNE, ASK_REMOTE_REMOVE, ASK_RESET_HARD,
    ASK_RESET_MIXED, ASK_RESET_SOFT, ASK_REVERT, ASK_SEARCH, ASK_STASH, ASK_TAG_CREATE,
    ASK_TAG_DELETE, ASK_WORKTREE_ADD, ASK_WORKTREE_REMOVE,
};

// ========================================================================
// GitTool: questions
// ========================================================================

impl GitTool {
    /// Ask the same question [`ask`](Self::ask) does, but over the names it
    /// can be answered with: the reader filters a list instead of spelling a
    /// branch out. The names are read first, so this hands back the job that
    /// reads them and the answer comes through the same tag either way.
    pub(super) fn pick(
        &mut self,
        tag: &'static str,
        label: &str,
        kind: exec::Candidates,
    ) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        self.pending_pick = Some(PickQuestion::new(tag, label));
        ToolOutcome::Job(exec::candidates(&root, kind))
    }

    /// Ask a one-line question, tagged so the answer finds its way back.
    pub(super) fn ask(&self, tag: &'static str, label: &str) -> ToolOutcome {
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag,
        })
    }

    /// Ask what to move to, highlighting what it matches as it is typed.
    pub(super) fn ask_search(&self) -> ToolOutcome {
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "/".to_string(),
            mode: PromptMode::Search { reverse: false },
            tag: ASK_SEARCH,
        })
    }

    /// Run what an answered question asks for.
    pub(super) fn answer_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        self.release_wheel();
        let Some(answer) = reply.answer else {
            // A discard turned down is gone with the question that asked.
            if reply.tag == ASK_DISCARD {
                self.pending_discard = None;
            }
            self.notice = Some(Notice::info("cancelled"));
            return ToolOutcome::Consumed;
        };
        // A search is the one question that asks nothing of git, so it is
        // answered before the repository is required.
        if reply.tag == ASK_SEARCH {
            return self.search_for(&answer);
        }
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let answer = answer.trim().to_string();
        match reply.tag {
            // A confirmation's answer is only its yes; what it confirms was
            // decided when it was asked.
            ASK_DISCARD => return self.discard(),
            // The one question an empty answer means something for.
            ASK_STASH => return ToolOutcome::Job(exec::stash_push(&root, &answer)),
            _ => {}
        }
        if answer.is_empty() {
            return ToolOutcome::Consumed;
        }
        if let Some(outcome) = self.answer_push(&root, reply.tag, &answer) {
            return outcome;
        }
        match reply.tag {
            ASK_CUSTOM => ToolOutcome::Job(exec::custom(&root, &answer)),
            ASK_REMOTE_ADD => match answer.split_once(char::is_whitespace) {
                Some((name, url)) => {
                    ToolOutcome::Job(exec::remote_add(&root, name.trim(), url.trim()))
                }
                None => {
                    self.notice = Some(Notice::error("failed: expected `name url`"));
                    ToolOutcome::Consumed
                }
            },
            ASK_REMOTE_PRUNE => ToolOutcome::Job(exec::remote_prune(&root, &answer)),
            ASK_REMOTE_REMOVE => ToolOutcome::Job(exec::remote_remove(&root, &answer)),
            ASK_WORKTREE_ADD => ToolOutcome::Job(exec::worktree_add(&root, &answer)),
            ASK_WORKTREE_REMOVE => ToolOutcome::Job(exec::worktree_remove(&root, &answer)),
            // Everything else is answered with a revision or a ref's name,
            // which is refused when it could be read as an option instead.
            tag => match Rev::parse(&answer) {
                Ok(rev) => self.run_with_rev(&root, tag, &rev),
                Err(reason) => {
                    self.notice = Some(Notice::error(format!("failed: {reason}")));
                    ToolOutcome::Consumed
                }
            },
        }
    }

    /// Run the command a question answered with a revision asks for.
    fn run_with_rev(&self, root: &Path, tag: &'static str, rev: &Rev) -> ToolOutcome {
        let job = match tag {
            ASK_BRANCH_CHECKOUT => exec::checkout(root, rev),
            ASK_BRANCH_CREATE => exec::branch_create(root, rev, true),
            ASK_BRANCH_CREATE_HERE => exec::branch_create(root, rev, false),
            ASK_BRANCH_DELETE => exec::branch_delete(root, rev, false),
            ASK_BRANCH_DELETE_FORCE => exec::branch_delete(root, rev, true),
            ASK_CHERRY_PICK => exec::cherry_pick(root, rev),
            ASK_DIFF_REV => exec::diff_all(root, false, Some(rev)),
            ASK_MERGE => exec::merge(root, rev),
            ASK_REBASE => {
                return self.spawn_git(root, &["rebase", "--interactive", rev.as_str()]);
            }
            ASK_RESET_HARD => exec::reset(root, ResetMode::Hard, rev),
            ASK_RESET_MIXED => exec::reset(root, ResetMode::Mixed, rev),
            ASK_RESET_SOFT => exec::reset(root, ResetMode::Soft, rev),
            ASK_REVERT => exec::revert(root, rev),
            ASK_TAG_CREATE => exec::tag_create(root, rev),
            ASK_TAG_DELETE => exec::tag_delete(root, rev),
            _ => return ToolOutcome::Consumed,
        };
        ToolOutcome::Job(job)
    }
}

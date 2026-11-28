//! What the Git view tells the user, kept typed until the window takes it,
//! and what it asks before throwing something away.
//!
//! The view stores a [`GitNotice`] and turns it into a misty [`Notice`] only
//! in `take_notice`, and asks a [`GitConfirmation`] as a misty
//! [`Confirmation`], so the wording and severity are decided in one `match`
//! each and tests compare variants rather than sentences.

use winter_model::editor::{Confirmation, Notice};
use winter_model::notice::SearchMiss;

use super::ACTION_ABORT;

// ========================================================================
// Constants
// ========================================================================

/// What a failure is called when no command was named as it started.
const GIT_FAILED: &str = "git failed";

// ========================================================================
// Data Structures
// ========================================================================

/// What the view asks before throwing something away, by what goes.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum GitConfirmation {
    /// A branch of this repository's, by its name.
    DeleteBranch(String),
    /// A branch on a remote, deleted there for everyone.
    DeleteRemoteBranch {
        /// The branch's name on the remote.
        branch: String,
        /// The remote's name.
        remote: String,
    },
    /// A tag, by its name.
    DeleteTag(String),
    /// Untracked files, deleted: the one file's path, or how many.
    DeleteUntracked(Files),
    /// Changes to tracked files, restored from the index: the one file's
    /// path, or how many.
    DiscardChanges(Files),
    /// One hunk of the file at this path.
    DiscardHunk(String),
    /// The selected lines of a hunk.
    DiscardLines,
    /// One stash, by its name.
    DropStash(String),
    /// Every stash.
    DropStashes,
}

/// The files a confirmation is about: one by its path, or how many.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum Files {
    /// Several, by how many.
    Many(usize),
    /// One, by its path.
    One(String),
}

/// One message for the status bar.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum GitNotice {
    /// The user aborted a change in the editor git opened for it (`commit`,
    /// `rebase`).
    Aborted(&'static str),
    /// A change starting, named as it runs (`git push origin main`), and
    /// left out of the history, which hears how it ended.
    Announced(String),
    /// A question was cancelled.
    Cancelled,
    /// A change that finished, with the first line git said of it.
    Changed(String),
    /// A selection mixes tracked and untracked files, which discard apart.
    DiscardMixed,
    /// A staged change, which discarding would revert the wrong way.
    DiscardStaged,
    /// A remote was answered without its URL.
    ExpectedNameAndUrl,
    /// A pull was answered with something not `remote/branch`.
    ExpectedRemoteBranch,
    /// A command git failed.
    Failed(GitFailure),
    /// A fetch finished.
    Fetched,
    /// A pattern was appended to `.gitignore`.
    Ignored(String),
    /// What was typed could not be run, for the reason given.
    Invalid(String),
    /// No branch is checked out to pull.
    NoBranchToPull,
    /// No branch is checked out to push.
    NoBranchToPush,
    /// No change on the lines selected.
    NoChangeOnLines,
    /// No commit at the cursor to reword.
    NoCommitToReword,
    /// No file at the cursor.
    NoFile,
    /// No file at the cursor to blame.
    NoFileToBlame,
    /// No file at the cursor to compare.
    NoFileToCompare,
    /// No stash at the cursor to drop.
    NoStashToDrop,
    /// No remote to push tags to.
    NoTagRemote,
    /// The branch tracks nothing to fetch from.
    NoUpstream,
    /// The remote has no web page to open.
    NoWebUrl,
    /// A file asked to resolve has no conflict.
    NotInConflict(String),
    /// What was typed names no branch on a remote.
    NotRemoteBranch(String),
    /// The cursor's section holds nothing to stage, unstage, or discard.
    NothingInSection(&'static str),
    /// The selection holds nothing to stage, unstage, or discard.
    NothingSelected(&'static str),
    /// Nothing is staged to commit.
    NothingStaged,
    /// Git finished what the abort action would have given up.
    NothingToAbort,
    /// A read the view would fill itself with came back empty.
    NothingToShow,
    /// A pull finished, with the first line git said of it.
    Pulled(String),
    /// A push finished.
    Pushed,
    /// A conflicted file was marked resolved.
    Resolved(String),
    /// A search over the rows that moved nowhere.
    Search(SearchMiss),
    /// The repository cannot be watched, so changes need a refresh.
    Unwatched,
}

/// A command git failed, as its notice shows it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct GitFailure {
    /// The label of the button giving up what git is stopped in, if any.
    pub(crate) abort: Option<&'static str>,
    /// The command as it was named when it started, if it was.
    pub(crate) command: Option<String>,
    /// What git said, cut to fit.
    pub(crate) detail: String,
}

// ========================================================================
// GitConfirmation
// ========================================================================

impl GitConfirmation {
    /// The question the window asks: the one place its wording is decided.
    /// Every one throws something away, so every one warns.
    pub(crate) fn to_confirmation(&self) -> Confirmation {
        let (title, message, yes) = match self {
            Self::DeleteBranch(name) => (
                "Delete Branch?",
                format!("{name} is deleted from this repository."),
                "Delete",
            ),
            Self::DeleteRemoteBranch { branch, remote } => (
                "Delete Branch on the Server?",
                format!("{branch} is deleted on {remote}, for everyone who fetches from it."),
                "Delete",
            ),
            Self::DeleteTag(name) => (
                "Delete Tag?",
                format!("{name} is deleted from this repository."),
                "Delete",
            ),
            Self::DeleteUntracked(files) => (
                "Delete Untracked Files?",
                match files {
                    Files::One(path) => format!("{path} is deleted; git cannot bring it back."),
                    Files::Many(count) => {
                        format!("{count} files are deleted; git cannot bring them back.")
                    }
                },
                "Delete",
            ),
            Self::DiscardChanges(files) => (
                "Discard Changes?",
                match files {
                    Files::One(path) => format!("The changes to {path} are lost."),
                    Files::Many(count) => format!("The changes to {count} files are lost."),
                },
                "Discard",
            ),
            Self::DiscardHunk(path) => (
                "Discard Hunk?",
                format!("This change to {path} is lost."),
                "Discard",
            ),
            Self::DiscardLines => (
                "Discard Lines?",
                "The changes on the selected lines are lost.".to_string(),
                "Discard",
            ),
            Self::DropStash(name) => (
                "Drop Stash?",
                format!("{name} and the changes it keeps are lost."),
                "Drop",
            ),
            Self::DropStashes => (
                "Drop Every Stash?",
                "Every stash and the changes they keep are lost.".to_string(),
                "Drop All",
            ),
        };
        Confirmation::destructive(title, message, yes)
    }
}

// ========================================================================
// GitNotice
// ========================================================================

impl GitNotice {
    /// The notice the status bar shows: the one place its wording and
    /// severity are decided.
    pub(crate) fn into_notice(self) -> Notice {
        match self {
            Self::Aborted(what) => Notice::info(format!("{what} aborted")),
            Self::Announced(command) => Notice::info(command).unlogged(),
            Self::Cancelled => Notice::info("cancelled"),
            Self::Changed(detail) => Notice::info(detail),
            Self::DiscardMixed => {
                Notice::refusal("select tracked and untracked files apart to discard them")
            }
            Self::DiscardStaged => {
                Notice::refusal("nothing to discard in this section: unstage it first")
            }
            Self::ExpectedNameAndUrl => Notice::refusal("failed: expected `name url`"),
            Self::ExpectedRemoteBranch => Notice::refusal("failed: expected `remote/branch`"),
            Self::Failed(failure) => failure.into_notice(),
            Self::Fetched => Notice::info("fetched"),
            Self::Ignored(pattern) => Notice::info(format!("ignored {pattern}")),
            Self::Invalid(reason) => Notice::error(format!("failed: {reason}")),
            Self::NoBranchToPull => Notice::refusal("no branch to pull"),
            Self::NoBranchToPush => Notice::refusal("no branch to push"),
            Self::NoChangeOnLines => Notice::refusal("no change on those lines"),
            Self::NoCommitToReword => Notice::refusal("no commit there to reword"),
            Self::NoFile => Notice::refusal("no file here"),
            Self::NoFileToBlame => Notice::refusal("no file to blame here"),
            Self::NoFileToCompare => Notice::refusal("no file to compare here"),
            Self::NoStashToDrop => Notice::refusal("no stash to drop"),
            Self::NoTagRemote => Notice::refusal("no upstream or push remote to push tags to"),
            Self::NoUpstream => Notice::refusal("the branch tracks nothing to fetch from"),
            Self::NoWebUrl => Notice::refusal("no web URL for that remote"),
            Self::NotInConflict(path) => Notice::refusal(format!("{path} is not in conflict")),
            Self::NotRemoteBranch(answer) => {
                Notice::refusal(format!("not a branch on a remote: {answer}"))
            }
            Self::NothingInSection(verb) => {
                Notice::refusal(format!("nothing to {verb} in this section"))
            }
            Self::NothingSelected(verb) => Notice::refusal(format!(
                "nothing selected to {verb}: select files, or lines of one hunk"
            )),
            Self::NothingStaged => Notice::refusal("nothing staged to commit"),
            Self::NothingToAbort => Notice::refusal("nothing to abort"),
            Self::NothingToShow => Notice::refusal("nothing to show"),
            Self::Pulled(detail) => Notice::info(format!("pulled: {detail}")),
            Self::Pushed => Notice::info("pushed"),
            Self::Resolved(path) => Notice::info(format!("resolved {path}")),
            Self::Search(miss) => miss.into_notice(),
            Self::Unwatched => Notice::warning(
                "git: this repository cannot be watched for changes; refresh to read it",
            ),
        }
    }
}

// ========================================================================
// GitFailure
// ========================================================================

impl GitFailure {
    /// The error, named by its command (`git rebase main failed`), with what
    /// git said under it and the abort button where git is stopped part-way.
    fn into_notice(self) -> Notice {
        let summary = match self.command {
            Some(command) => format!("{command} failed"),
            None => GIT_FAILED.to_string(),
        };
        let notice = Notice::error(summary).with_detail(self.detail);
        match self.abort {
            Some(label) => notice.with_action(ACTION_ABORT, label),
            None => notice,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::notify::Severity;

    use super::*;

    /// One of each variant, so a wording rule is checked against them all.
    fn every_notice() -> Vec<GitNotice> {
        let failure = GitFailure {
            abort: Some("Abort Rebase"),
            command: Some("git rebase main".to_string()),
            detail: "CONFLICT".to_string(),
        };
        vec![
            GitNotice::Announced("git push".to_string()),
            GitNotice::Cancelled,
            GitNotice::Changed("done".to_string()),
            GitNotice::DiscardMixed,
            GitNotice::DiscardStaged,
            GitNotice::ExpectedNameAndUrl,
            GitNotice::ExpectedRemoteBranch,
            GitNotice::Failed(failure),
            GitNotice::Fetched,
            GitNotice::Ignored("*.log".to_string()),
            GitNotice::Invalid("not a revision".to_string()),
            GitNotice::NoBranchToPull,
            GitNotice::NoBranchToPush,
            GitNotice::NoChangeOnLines,
            GitNotice::NoCommitToReword,
            GitNotice::NoFile,
            GitNotice::NoFileToBlame,
            GitNotice::NoFileToCompare,
            GitNotice::NoStashToDrop,
            GitNotice::NoTagRemote,
            GitNotice::NoUpstream,
            GitNotice::NoWebUrl,
            GitNotice::NotInConflict("a.rs".to_string()),
            GitNotice::NotRemoteBranch("main".to_string()),
            GitNotice::NothingInSection("stage"),
            GitNotice::NothingSelected("stage"),
            GitNotice::NothingStaged,
            GitNotice::NothingToAbort,
            GitNotice::NothingToShow,
            GitNotice::Pulled("Fast-forward".to_string()),
            GitNotice::Pushed,
            GitNotice::Resolved("a.rs".to_string()),
            GitNotice::Search(SearchMiss::NoSearch),
            GitNotice::Unwatched,
        ]
    }

    #[test]
    fn test_every_notice_keeps_winter_s_wording_rules() {
        for notice in every_notice() {
            let text = notice.clone().into_notice().text;
            assert!(!text.is_empty(), "{notice:?} says nothing");
            assert!(
                !text.contains("..."),
                "{notice:?} writes `...`, not `…`: {text}"
            );
            assert!(!text.ends_with('.'), "{notice:?} ends in a period: {text}");
        }
    }

    #[test]
    fn test_a_failure_is_named_by_its_command_and_offers_its_abort() {
        let failure = GitFailure {
            abort: Some("Abort Rebase"),
            command: Some("git rebase main".to_string()),
            detail: "CONFLICT".to_string(),
        };
        let notice = GitNotice::Failed(failure).into_notice();
        assert_eq!(notice.severity, Severity::Error);
        assert_eq!(notice.text, "git rebase main failed");
        assert_eq!(notice.detail.as_deref(), Some("CONFLICT"));
        let labels: Vec<&str> = notice.actions.iter().map(|a| a.label.as_str()).collect();
        assert_eq!(labels, ["Abort Rebase"]);
    }

    #[test]
    fn test_a_command_named_as_it_starts_stays_out_of_the_history() {
        let notice = GitNotice::Announced("git add a.rs".to_string()).into_notice();
        assert_eq!(notice.text, "git add a.rs");
        assert!(!notice.logged, "the history hears how it ended instead");
    }

    #[test]
    fn test_a_failure_never_named_as_it_started_is_git_s() {
        let failure = GitFailure {
            abort: None,
            command: None,
            detail: String::new(),
        };
        assert_eq!(GitNotice::Failed(failure).into_notice().text, GIT_FAILED);
    }
}

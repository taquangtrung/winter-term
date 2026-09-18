//! What git is in the middle of, read from the state files it keeps beside the
//! repository.
//!
//! A merge, a rebase, a cherry-pick, a revert and a bisect each leave the tree
//! in a state that only the commands continuing them get out of, and git says
//! so nowhere a porcelain status reports: `git status --porcelain` is silent
//! about all five. The files themselves are the record, and they are small
//! enough to read on every refresh.
//!
//! Parsing is pure so it can be tested without a repository half-way through
//! anything: [`state_files`] names what to read and [`parse`] reads what came
//! back.

use std::path::{Path, PathBuf};

// ========================================================================
// Constants
// ========================================================================

/// Written while a merge is unfinished; holds the commits being merged in.
const MERGE_HEAD: &str = "MERGE_HEAD";

/// The message a finished merge would commit, which names what is being
/// merged in better than its hash does.
const MERGE_MSG: &str = "MERGE_MSG";

/// The directory an interactive rebase keeps its state in.
const REBASE_MERGE: &str = "rebase-merge";

/// The directory a patch-applying rebase keeps its state in.
const REBASE_APPLY: &str = "rebase-apply";

/// Within either rebase directory: the ref the rebase started from.
const HEAD_NAME: &str = "head-name";

/// Within either rebase directory: the commit being rebased onto.
const ONTO: &str = "onto";

/// Within an interactive rebase: which step is being replayed, and how many
/// there are.
const MSGNUM: &str = "msgnum";
const END: &str = "end";

/// Within a patch-applying rebase: the same pair, under its own names.
const NEXT: &str = "next";
const LAST: &str = "last";

/// Written while a cherry-pick is unfinished.
const CHERRY_PICK_HEAD: &str = "CHERRY_PICK_HEAD";

/// Written while a revert is unfinished.
const REVERT_HEAD: &str = "REVERT_HEAD";

/// Written for the length of a bisect, one line per answer given.
const BISECT_LOG: &str = "BISECT_LOG";

/// The ref the bisect started from, which is where `reset` returns to.
const BISECT_START: &str = "BISECT_START";

/// How much of a hash to show, matching the abbreviation the log rows use.
const SHORT_HASH: usize = 7;

/// The prefix a ref file carries that a reader does not need.
const BRANCH_PREFIX: &str = "refs/heads/";

// ========================================================================
// Data Structures
// ========================================================================

/// What git is part-way through.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    /// `git bisect`, narrowing down a commit.
    Bisect,
    /// `git cherry-pick`, stopped on a commit it could not apply.
    CherryPick,
    /// `git merge`, stopped before it could commit.
    Merge,
    /// `git rebase`, part-way through replaying commits.
    Rebase,
    /// `git revert`, stopped on a commit it could not undo.
    Revert,
}

impl Operation {
    /// What the view calls it.
    pub fn title(self) -> &'static str {
        match self {
            Operation::Bisect => "Bisecting",
            Operation::CherryPick => "Cherry-picking",
            Operation::Merge => "Merging",
            Operation::Rebase => "Rebasing",
            Operation::Revert => "Reverting",
        }
    }

    /// The keys that finish it, as the view spells them: what to press to
    /// carry on, and what to press to give up.
    pub fn keys(self) -> &'static str {
        match self {
            Operation::Bisect => "b g good, b b bad, b r reset",
            Operation::CherryPick => "A c continue, A x abort",
            Operation::Merge => "m c continue, m x abort",
            Operation::Rebase => "r c continue, r s skip, r x abort",
            Operation::Revert => "V c continue, V x abort",
        }
    }
}

/// An operation git has not finished, as the header block reports it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Progress {
    /// Which operation.
    pub operation: Operation,
    /// What it is working on: the branch being merged, the commits a rebase
    /// runs between, the commit a pick stopped on. Empty when the state files
    /// say only that it is happening.
    pub detail: String,
    /// How far along it is, where git counts the steps.
    pub step: Option<(usize, usize)>,
}

// ========================================================================
// Functions
// ========================================================================

/// The files to read to know what git is doing, all under `git_dir`. Most are
/// absent most of the time, which is the answer "nothing is in progress".
pub fn state_files(git_dir: &Path) -> Vec<PathBuf> {
    let rebase_merge = git_dir.join(REBASE_MERGE);
    let rebase_apply = git_dir.join(REBASE_APPLY);
    vec![
        git_dir.join(MERGE_HEAD),
        git_dir.join(MERGE_MSG),
        git_dir.join(CHERRY_PICK_HEAD),
        git_dir.join(REVERT_HEAD),
        git_dir.join(BISECT_LOG),
        git_dir.join(BISECT_START),
        rebase_merge.join(HEAD_NAME),
        rebase_merge.join(ONTO),
        rebase_merge.join(MSGNUM),
        rebase_merge.join(END),
        rebase_apply.join(HEAD_NAME),
        rebase_apply.join(ONTO),
        rebase_apply.join(NEXT),
        rebase_apply.join(LAST),
    ]
}

/// What the state files say git is doing, or `None` when they say nothing.
///
/// A rebase is reported ahead of the others: a conflicted pick inside one
/// writes `CHERRY_PICK_HEAD` too, and the rebase is the operation the reader
/// is actually in.
pub fn parse(files: &[(PathBuf, String)]) -> Option<Progress> {
    let read = |dir: &str, name: &str| -> Option<&str> {
        files.iter().find_map(|(path, text)| {
            let named = path.file_name()?.to_str()? == name;
            let under = dir.is_empty()
                || path
                    .parent()
                    .and_then(|parent| parent.file_name())
                    .and_then(|parent| parent.to_str())
                    == Some(dir);
            (named && under).then_some(text.trim())
        })
    };

    for (dir, step_from, step_to) in [(REBASE_MERGE, MSGNUM, END), (REBASE_APPLY, NEXT, LAST)] {
        let Some(head) = read(dir, HEAD_NAME) else {
            continue;
        };
        let onto = read(dir, ONTO).map(short_hash).unwrap_or_default();
        let branch = head.strip_prefix(BRANCH_PREFIX).unwrap_or(head);
        return Some(Progress {
            operation: Operation::Rebase,
            detail: match onto.is_empty() {
                true => branch.to_string(),
                false => format!("{branch} onto {onto}"),
            },
            step: step(read(dir, step_from), read(dir, step_to)),
        });
    }

    if read("", MERGE_HEAD).is_some() {
        // The message names the branches; the hash is what is left when a
        // merge was started without one.
        let detail = read("", MERGE_MSG)
            .and_then(|msg| msg.lines().next())
            .map(str::to_string)
            .or_else(|| read("", MERGE_HEAD).map(short_hash))
            .unwrap_or_default();
        return Some(Progress {
            operation: Operation::Merge,
            detail,
            step: None,
        });
    }

    for (name, operation) in [
        (CHERRY_PICK_HEAD, Operation::CherryPick),
        (REVERT_HEAD, Operation::Revert),
    ] {
        if let Some(head) = read("", name) {
            return Some(Progress {
                operation,
                detail: short_hash(head),
                step: None,
            });
        }
    }

    if read("", BISECT_LOG).is_some() {
        // The start file holds the ref the bisect will return to, which is
        // the only part of a bisect's state that reads as a place.
        let detail = read("", BISECT_START)
            .map(|start| format!("started from {start}"))
            .unwrap_or_default();
        return Some(Progress {
            operation: Operation::Bisect,
            detail,
            step: None,
        });
    }

    None
}

/// A step pair, where both halves are numbers and the count is not zero.
fn step(at: Option<&str>, of: Option<&str>) -> Option<(usize, usize)> {
    let at = at?.parse().ok()?;
    let of = of?.parse().ok()?;
    (of > 0).then_some((at, of))
}

/// A hash cut to the length the log rows abbreviate to. Left whole when it is
/// already shorter, which is what a ref name that landed here would be.
fn short_hash(hash: &str) -> String {
    let hash = hash.trim();
    match hash.char_indices().nth(SHORT_HASH) {
        Some((at, _)) => hash[..at].to_string(),
        None => hash.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn files(pairs: &[(&str, &str)]) -> Vec<(PathBuf, String)> {
        pairs
            .iter()
            .map(|(path, text)| (PathBuf::from("/repo/.git").join(path), text.to_string()))
            .collect()
    }

    #[test]
    fn test_nothing_in_progress_reads_as_nothing() {
        assert_eq!(parse(&files(&[])), None);
    }

    #[test]
    fn test_an_interactive_rebase_names_its_ends_and_counts_its_steps() {
        let state = files(&[
            ("rebase-merge/head-name", "refs/heads/feature\n"),
            ("rebase-merge/onto", "abc1234567890\n"),
            ("rebase-merge/msgnum", "3\n"),
            ("rebase-merge/end", "7\n"),
        ]);
        assert_eq!(
            parse(&state),
            Some(Progress {
                operation: Operation::Rebase,
                detail: "feature onto abc1234".to_string(),
                step: Some((3, 7)),
            })
        );
    }

    #[test]
    fn test_a_patch_rebase_reads_from_its_own_files() {
        // The two rebase kinds keep the same facts under different names.
        let state = files(&[
            ("rebase-apply/head-name", "refs/heads/topic\n"),
            ("rebase-apply/onto", "999888777\n"),
            ("rebase-apply/next", "1\n"),
            ("rebase-apply/last", "2\n"),
        ]);
        let progress = parse(&state).expect("a rebase");
        assert_eq!(progress.operation, Operation::Rebase);
        assert_eq!(progress.detail, "topic onto 9998887");
        assert_eq!(progress.step, Some((1, 2)));
    }

    #[test]
    fn test_a_rebase_outranks_the_pick_it_stopped_on() {
        // A conflicted step of a rebase writes CHERRY_PICK_HEAD as well, and
        // the rebase is the operation the reader has to get out of.
        let state = files(&[
            ("rebase-merge/head-name", "refs/heads/feature\n"),
            ("rebase-merge/onto", "abc1234\n"),
            ("CHERRY_PICK_HEAD", "deadbeef\n"),
        ]);
        assert_eq!(
            parse(&state).map(|progress| progress.operation),
            Some(Operation::Rebase)
        );
    }

    #[test]
    fn test_a_merge_reads_the_message_that_names_the_branches() {
        let state = files(&[
            ("MERGE_HEAD", "abc1234567890\n"),
            (
                "MERGE_MSG",
                "Merge branch 'feature' into main\n\n# a comment\n",
            ),
        ]);
        let progress = parse(&state).expect("a merge");
        assert_eq!(progress.operation, Operation::Merge);
        assert_eq!(progress.detail, "Merge branch 'feature' into main");

        // Without a message the hash is what is left to say.
        let bare = files(&[("MERGE_HEAD", "abc1234567890\n")]);
        assert_eq!(parse(&bare).map(|p| p.detail), Some("abc1234".to_string()));
    }

    #[test]
    fn test_a_pick_a_revert_and_a_bisect_each_report_themselves() {
        let pick = files(&[("CHERRY_PICK_HEAD", "abc1234567890\n")]);
        assert_eq!(
            parse(&pick).map(|p| (p.operation, p.detail)),
            Some((Operation::CherryPick, "abc1234".to_string()))
        );

        let revert = files(&[("REVERT_HEAD", "fedcba9876543\n")]);
        assert_eq!(parse(&revert).map(|p| p.operation), Some(Operation::Revert));

        let bisect = files(&[
            ("BISECT_LOG", "git bisect start\n"),
            ("BISECT_START", "main\n"),
        ]);
        assert_eq!(
            parse(&bisect).map(|p| (p.operation, p.detail)),
            Some((Operation::Bisect, "started from main".to_string()))
        );
    }

    #[test]
    fn test_the_state_files_cover_both_rebase_directories() {
        let asked = state_files(Path::new("/repo/.git"));
        for name in [REBASE_MERGE, REBASE_APPLY] {
            assert!(
                asked
                    .iter()
                    .any(|path| path.starts_with(Path::new("/repo/.git").join(name))),
                "{name} is read"
            );
        }
        assert!(asked.iter().any(|path| path.ends_with(MERGE_HEAD)));
    }
}

//! The undo tree's other branches: what was undone and then left for an
//! edit of its own, offered in a list newest first (`C-M-/`), any of them
//! had back by choosing it. `u` and `C-r` go on as Vim's do, along the
//! branch the text is on; nothing undone is lost by typing after it.

use std::time::SystemTime;

use winter_model::editor::{EditorOutcome, Notice, PickPlacement, PickRequest};
use winter_model::paint::IconKind;
use winter_model::textedit::buffer::BranchInfo;

use super::TextEditor;

// ========================================================================
// Constants
// ========================================================================

/// The question the list of branches answers.
pub(super) const ASK_BRANCH: &str = "undo_branch";

/// What the list is headed with, and each branch wears.
const BRANCHES_LABEL: &str = "Undone Branches";
const BRANCH_ICON: IconKind = IconKind::Symbol { name: "git-branch" };

/// What is said where there is no branch to have back.
const NO_BRANCHES: &str = "nothing undone was left for another edit";

/// The seconds in a minute, an hour, and a day, for saying how long ago.
const MINUTE: u64 = 60;
const HOUR: u64 = 60 * MINUTE;
const DAY: u64 = 24 * HOUR;

// ========================================================================
// TextEditor: undone branches
// ========================================================================

impl TextEditor {
    /// Offer the branches undone and left, newest first.
    pub(super) fn ask_branch(&mut self) -> EditorOutcome {
        let branches = self.buffer_mut().branches();
        if branches.is_empty() {
            self.modal.report(Notice::info(NO_BRANCHES).unlogged());
            return EditorOutcome::Consumed;
        }
        let now = SystemTime::now();
        let items: Vec<String> = branches
            .iter()
            .enumerate()
            .map(|(at, branch)| label_of(at, branch, now))
            .collect();
        EditorOutcome::Pick(PickRequest {
            icons: vec![BRANCH_ICON; items.len()],
            items,
            label: BRANCHES_LABEL.to_string(),
            placement: PickPlacement::Cursor,
            tag: ASK_BRANCH,
        })
    }

    /// Have back the branch the list came back with, named by its label.
    pub(super) fn branch_chosen(&mut self, chosen: &str) -> EditorOutcome {
        let index = chosen
            .split_once('.')
            .and_then(|(number, _)| number.parse::<usize>().ok())
            .and_then(|number| number.checked_sub(1));
        let Some(index) = index else {
            return EditorOutcome::Consumed;
        };
        if self.refuse_edits().is_some() {
            return EditorOutcome::Consumed;
        }
        let said = match self.buffer_mut().restore_branch(index) {
            true => "the undone branch is back; what it was left for is a branch in its place",
            false => "that branch can no longer be reached",
        };
        self.modal.report(Notice::info(said).unlogged());
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What the list says of the branch at `at`: its place, how many steps it
/// holds, the line it starts on, and how long ago it was left.
fn label_of(at: usize, branch: &BranchInfo, now: SystemTime) -> String {
    let steps = match branch.steps {
        1 => "1 change".to_string(),
        n => format!("{n} changes"),
    };
    let ago = now
        .duration_since(branch.left)
        .map_or(0, |ago| ago.as_secs());
    format!(
        "{}. {steps} from line {}, left {}",
        at + 1,
        branch.row + 1,
        said_ago(ago)
    )
}

/// `seconds` ago, as a person says it.
fn said_ago(seconds: u64) -> String {
    match seconds {
        0..MINUTE => "just now".to_string(),
        MINUTE..HOUR => format!("{} min ago", seconds / MINUTE),
        HOUR..DAY => format!("{} h ago", seconds / HOUR),
        _ => format!("{} days ago", seconds / DAY),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_branch_is_listed_by_place_size_line_and_age() {
        let now = SystemTime::now();
        let branch = BranchInfo {
            left: now - std::time::Duration::from_secs(3 * MINUTE),
            row: 11,
            steps: 2,
        };
        assert_eq!(
            label_of(0, &branch, now),
            "1. 2 changes from line 12, left 3 min ago"
        );
    }
}

//! Transient menus: a key opens one, a second key picks from it. The menu
//! itself knows only what it offers; what each choice does belongs to the view.

use crate::model::page::{PageRow, PageSpan, PageStyle};

// ========================================================================
// Constants
// ========================================================================

/// Separates a choice's key from its label.
const KEY_GAP: &str = "  ";

// ========================================================================
// Data Structures
// ========================================================================

/// An open menu: which one, so the view knows how to read the key that
/// follows.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Popup {
    /// Bisecting a range.
    Bisect,
    /// Branch checkout, creation, and deletion.
    Branch,
    /// Replaying commits onto here.
    CherryPick,
    /// Committing what is staged.
    Commit,
    /// Reading diffs.
    Diff,
    /// Adding paths to `.gitignore`.
    Ignore,
    /// Reading the log.
    Log,
    /// Merging a branch in.
    Merge,
    /// Remotes.
    Remote,
    /// Moving the branch pointer.
    Reset,
    /// Undoing commits with new ones.
    Revert,
    /// Replaying commits onto another base.
    Rebase,
    /// The stash.
    Stash,
    /// Tags.
    Tag,
    /// Worktrees.
    Worktree,
}

// ========================================================================
// Popup
// ========================================================================

impl Popup {
    /// The menu's title.
    pub fn title(self) -> &'static str {
        match self {
            Popup::Bisect => "Bisect",
            Popup::Branch => "Branch",
            Popup::CherryPick => "Cherry-pick",
            Popup::Commit => "Commit",
            Popup::Diff => "Diff",
            Popup::Ignore => "Ignore",
            Popup::Log => "Log",
            Popup::Merge => "Merge",
            Popup::Rebase => "Rebase",
            Popup::Remote => "Remote",
            Popup::Reset => "Reset",
            Popup::Revert => "Revert",
            Popup::Stash => "Stash",
            Popup::Tag => "Tag",
            Popup::Worktree => "Worktree",
        }
    }

    /// What the menu offers, as key and label pairs.
    pub fn choices(self) -> &'static [(char, &'static str)] {
        match self {
            Popup::Bisect => &[
                ('s', "start"),
                ('g', "mark good"),
                ('b', "mark bad"),
                ('r', "reset"),
            ],
            Popup::Branch => &[
                ('b', "checkout branch"),
                ('c', "create and checkout"),
                ('n', "create, stay here"),
                ('d', "delete"),
                ('D', "delete, unmerged"),
            ],
            Popup::CherryPick => &[('a', "pick a commit"), ('c', "continue"), ('x', "abort")],
            Popup::Commit => &[
                ('c', "commit, in an editor"),
                ('m', "commit, one line"),
                ('a', "amend, in an editor"),
                ('e', "amend, keep the message"),
            ],
            Popup::Diff => &[
                ('d', "working tree"),
                ('s', "staged"),
                ('r', "against a revision"),
            ],
            Popup::Ignore => &[('i', "this path"), ('e', "this extension")],
            Popup::Log => &[('l', "this branch"), ('a', "all refs"), ('f', "this file")],
            Popup::Merge => &[('m', "merge a branch"), ('c', "continue"), ('x', "abort")],
            Popup::Rebase => &[
                ('i', "interactive, onto a revision"),
                ('u', "onto the upstream"),
                ('c', "continue"),
                ('s', "skip"),
                ('x', "abort"),
            ],
            Popup::Remote => &[('v', "list"), ('a', "add"), ('d', "remove"), ('p', "prune")],
            Popup::Reset => &[
                ('m', "mixed, keep the working tree"),
                ('s', "soft, keep the index"),
                ('h', "hard, throw everything away"),
            ],
            Popup::Revert => &[('v', "revert a commit"), ('c', "continue"), ('x', "abort")],
            Popup::Stash => &[
                ('z', "stash everything"),
                ('p', "pop the newest"),
                ('a', "apply the newest"),
                ('d', "drop the newest"),
                ('l', "list"),
            ],
            Popup::Tag => &[('t', "create"), ('d', "delete"), ('l', "list")],
            Popup::Worktree => &[('l', "list"), ('a', "add"), ('d', "remove")],
        }
    }

    /// The menu as rows, drawn under the view it belongs to.
    pub fn rows(self) -> Vec<PageRow> {
        let mut rows = vec![
            Vec::new(),
            vec![PageSpan::new(
                PageStyle::Header,
                format!("{}:", self.title()),
            )],
        ];
        rows.extend(self.choices().iter().map(|(key, label)| {
            vec![
                PageSpan::new(PageStyle::Accent, key.to_string()),
                PageSpan::new(PageStyle::Dim, format!("{KEY_GAP}{label}")),
            ]
        }));
        rows
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every menu the view can open.
    const EVERY: [Popup; 15] = [
        Popup::Bisect,
        Popup::Branch,
        Popup::CherryPick,
        Popup::Commit,
        Popup::Diff,
        Popup::Ignore,
        Popup::Log,
        Popup::Merge,
        Popup::Rebase,
        Popup::Remote,
        Popup::Reset,
        Popup::Revert,
        Popup::Stash,
        Popup::Tag,
        Popup::Worktree,
    ];

    #[test]
    fn test_no_menu_offers_the_same_key_twice() {
        // A duplicate key makes one of the two choices unreachable, and which
        // one depends on the order they happen to be listed in.
        for popup in EVERY {
            let mut keys: Vec<char> = popup.choices().iter().map(|(key, _)| *key).collect();
            let count = keys.len();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), count, "{:?} repeats a key", popup);
        }
    }

    #[test]
    fn test_every_menu_offers_something() {
        for popup in EVERY {
            assert!(!popup.choices().is_empty(), "{popup:?} is empty");
        }
    }

    #[test]
    fn test_a_menu_draws_its_title_and_every_choice() {
        let rows = Popup::Stash.rows();
        let text: Vec<String> = rows
            .iter()
            .map(|row| row.iter().map(|span| span.text.clone()).collect())
            .collect();
        assert!(text.iter().any(|line| line.contains("Stash:")));
        let choices = Popup::Stash.choices();
        for (key, label) in choices {
            assert!(
                text.iter().any(|line| line.contains(*label)),
                "{label} is not drawn"
            );
            assert!(
                text.iter().any(|line| line.contains(&key.to_string())),
                "{key} is not drawn"
            );
        }
    }
}

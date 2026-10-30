//! Transient menus: a key opens one, a second key picks from it. The menu
//! itself knows only what it offers; what each choice does belongs to the view.

// ========================================================================
// Constants
// ========================================================================

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
    /// Fetching: from where.
    Fetch,
    /// What can be done with the file under the cursor.
    File,
    /// Adding paths to `.gitignore`.
    Ignore,
    /// Folding what the cursor is on, and scrolling it into place: vim's
    /// `z` keys.
    Fold,
    /// Jumping to a part of the view, and re-reading it: vim's `g` keys.
    Jump,
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
    /// Pulling: from where, and how.
    Pull,
    /// Pushing: where to, and with which switches.
    Push,
    /// The switches a push goes with, toggled one key at a time.
    PushOptions,
    /// Replaying commits onto another base.
    Rebase,
    /// The stash.
    Stash,
    /// Submodules: listing, updating, adding, syncing, and taking them out.
    Submodule,
    /// Tags.
    Tag,
    /// Worktrees.
    Worktree,
    /// Copying what the cursor is on: vim's `y` keys.
    Yank,
}

/// A switch a menu's command goes with, as magit's transients have them:
/// `-` and then `key` turn it on or off while the menu stays up.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Switch {
    /// The flag it adds to the command.
    pub flag: &'static str,
    /// The key pressed after `-`.
    pub key: char,
    /// What it does.
    pub label: &'static str,
}

/// The log menu's switches.
const LOG_SWITCHES: &[Switch] = &[
    Switch { flag: "--first-parent", key: 'f', label: "Follow only the first parent" },
    Switch { flag: "--no-merges", key: 'm', label: "Leave merges out" },
    Switch { flag: "--simplify-by-decoration", key: 'd', label: "Only commits a ref points at" },
    Switch { flag: "--reverse", key: 'r', label: "Oldest first" },
];

/// The merge menu's switches.
const MERGE_SWITCHES: &[Switch] = &[
    Switch { flag: "--ff-only", key: 'f', label: "Fast-forward only" },
    Switch { flag: "--no-ff", key: 'n', label: "Always make a merge commit" },
];

/// The commit menu's switches.
const COMMIT_SWITCHES: &[Switch] = &[
    Switch { flag: "--all", key: 'a', label: "Stage all modified and deleted files" },
    Switch { flag: "--allow-empty", key: 'e', label: "Allow an empty commit" },
    Switch { flag: "--signoff", key: 's', label: "Add a Signed-off-by line" },
    Switch { flag: "--no-verify", key: 'n', label: "Disable hooks" },
    Switch { flag: "--gpg-sign", key: 'S', label: "Sign with gpg" },
];

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
            Popup::Fetch => "Fetching",
            Popup::File => "File",
            Popup::Ignore => "Ignore",
            Popup::Fold => "Fold",
            Popup::Jump => "Jump",
            Popup::Log => "Log",
            Popup::Merge => "Merge",
            Popup::Pull => "Pulling",
            Popup::Push => "Pushing",
            Popup::PushOptions => "Push options",
            Popup::Rebase => "Rebase",
            Popup::Remote => "Remote",
            Popup::Reset => "Reset",
            Popup::Revert => "Revert",
            Popup::Stash => "Stash",
            Popup::Tag => "Tag",
            Popup::Submodule => "Submodule",
            Popup::Worktree => "Worktree",
            Popup::Yank => "Yank",
        }
    }

    /// The switches the menu's commands go with, `-` and a key each.
    pub fn switches(self) -> &'static [Switch] {
        match self {
            Popup::Commit => COMMIT_SWITCHES,
            Popup::Log => LOG_SWITCHES,
            Popup::Merge => MERGE_SWITCHES,
            _ => &[],
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
                ('m', "rename this branch"),
                ('s', "spin off: a new branch here, this one back to its upstream"),
            ],
            Popup::CherryPick => &[('a', "pick a commit"), ('c', "continue"), ('x', "abort")],
            Popup::Commit => &[
                ('c', "commit, in an editor"),
                ('a', "amend, in an editor"),
                ('e', "amend, keep the message"),
                ('w', "reword the commit at point, else the last"),
                ('f', "fixup the commit at point, else one asked for"),
                ('F', "fixup and squash it in at once"),
            ],
            Popup::Diff => &[
                ('d', "working tree"),
                ('s', "staged"),
                ('r', "against a revision"),
                ('R', "a range, a..b"),
                ('b', "this branch against another"),
                ('c', "a commit"),
            ],
            // Everything that acts on one file, gathered where the cursor
            // already is: the same commands their own keys run, asked for
            // without having to recall which key each one was.
            Popup::File => &[
                ('s', "stage it"),
                ('u', "unstage it"),
                ('x', "discard it"),
                ('d', "diff it"),
                ('l', "log it"),
                ('b', "blame it"),
                ('o', "resolve a conflict with ours"),
                ('t', "resolve a conflict with theirs"),
            ],
            Popup::Ignore => &[('i', "this path"), ('e', "this extension")],
            // The `g` leader, which moves rather than runs: a menu all the
            // same, so a sequence with a second key says what it offers.
            Popup::Fold => &[
                ('a', "fold or unfold this"),
                ('o', "unfold this"),
                ('c', "fold this"),
                ('z', "scroll this to the middle"),
                ('t', "scroll this to the top"),
                ('b', "scroll this to the bottom"),
            ],
            Popup::Jump => &[
                ('g', "the top"),
                ('n', "untracked files"),
                ('u', "unstaged changes"),
                ('s', "staged changes"),
                ('z', "stashes"),
                ('c', "recent commits"),
                ('j', "next entry"),
                ('k', "previous entry"),
                ('r', "re-read the working tree"),
            ],
            Popup::Log => &[
                ('l', "this branch"),
                ('o', "another branch or revision"),
                ('L', "every local branch"),
                ('b', "every branch, local and remote"),
                ('a', "all refs"),
                ('f', "this file"),
                ('g', "this branch, changes matching a regex (-G)"),
                ('s', "this branch, changes adding or removing a string (-S)"),
            ],
            Popup::Merge => &[
                ('m', "merge a branch"),
                ('e', "merge, editing the message"),
                ('n', "merge, without committing"),
                ('s', "squash it in, without committing"),
                ('c', "continue"),
                ('x', "abort"),
            ],
            // The view says these with what it knows, where the branch pushes
            // and which switches are on (see `push::PushMenu`).
            Popup::Push => &[
                ('-', "options"),
                ('p', "to the push remote"),
                ('u', "to the upstream"),
                ('e', "elsewhere"),
                ('o', "another branch/commit"),
                ('T', "a tag"),
                ('t', "all tags"),
            ],
            Popup::PushOptions => &[
                ('f', "force with lease"),
                ('F', "force"),
                ('i', "force if includes"),
                ('h', "disable hooks"),
                ('d', "dry run"),
            ],
            Popup::Rebase => &[
                ('i', "interactive, from the commit at point"),
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
                ('i', "the index only"),
                ('w', "the working tree only, throwing its changes away"),
            ],
            Popup::Revert => &[('v', "revert a commit"), ('c', "continue"), ('x', "abort")],
            Popup::Stash => &[
                ('z', "stash everything"),
                ('i', "stash the index"),
                ('x', "stash everything, keeping the index"),
                ('u', "stash everything, untracked files too"),
                ('p', "pop the one at the cursor, else the newest"),
                ('a', "apply the one at the cursor, else the newest"),
                ('d', "drop the one at the cursor, else the newest"),
                ('l', "list"),
            ],
            Popup::Tag => &[('t', "create"), ('d', "delete"), ('l', "list")],
            Popup::Fetch => &[
                ('u', "from the upstream's remote"),
                ('e', "from elsewhere"),
                ('a', "from every remote"),
                ('s', "from every remote, with submodules"),
            ],
            Popup::Pull => &[
                ('u', "from the upstream, rebasing"),
                ('m', "from the upstream, merging"),
                ('e', "from elsewhere, rebasing"),
            ],
            Popup::Submodule => &[
                ('l', "list"),
                ('u', "update, init, recursive"),
                ('a', "add"),
                ('s', "sync the URLs"),
                ('d', "deinit"),
            ],
            Popup::Worktree => &[('l', "list"), ('a', "add"), ('d', "remove")],
            Popup::Yank => &[
                ('y', "the whole line"),
                ('s', "the hash or path"),
                ('r', "branches and tags, to read or delete"),
            ],
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Every menu the view can open.
    const EVERY: [Popup; 24] = [
        Popup::Bisect,
        Popup::Branch,
        Popup::CherryPick,
        Popup::Commit,
        Popup::Diff,
        Popup::Fetch,
        Popup::File,
        Popup::Fold,
        Popup::Ignore,
        Popup::Jump,
        Popup::Log,
        Popup::Merge,
        Popup::Pull,
        Popup::Push,
        Popup::PushOptions,
        Popup::Rebase,
        Popup::Remote,
        Popup::Reset,
        Popup::Revert,
        Popup::Stash,
        Popup::Submodule,
        Popup::Tag,
        Popup::Worktree,
        Popup::Yank,
    ];

    #[test]
    fn test_no_menu_s_switch_keys_repeat() {
        for popup in EVERY {
            let mut keys: Vec<char> = popup.switches().iter().map(|switch| switch.key).collect();
            let count = keys.len();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), count, "{:?} repeats a switch", popup);
        }
    }

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
}

# Git

Git (`C-S-g`, or `Git: Status`) shows the repository the pane is in as foldable sections: the changes, the stashes, what the upstream has that you do not, and the recent commits. It refreshes itself when the repository changes, and opens in a neighboring pane so your work stays in view.

`Sidebar: Git` keeps the branch and the changed files in a sidebar, where `s` stages and `u` unstages a file; `t` there opens this page (see [Sidebars](basics.md#sidebars)).

## Keys

| Key | Action |
|---|---|
| `j` `k` / `]` `[` | Move, next or previous entry |
| `g` then a letter | Jump: `s` staged, `u` unstaged, `n` untracked, `z` stashes, `c` commits |
| `Tab` / `S-Tab` | Fold the section or file, show or hide every diff |
| `Enter` | Open the file (at the hunk's line), or read a commit or stash |
| `C-o` | Open the file in `$EDITOR` |
| `M-S-r` / `C-r` / `g r` | Read the status, or the output showing, again |
| `s` / `S` | Stage, stage everything |
| `u` / `U` | Unstage, unstage everything |
| `x` | Discard (asks first); on a stash, drop it |
| `a` / `-` | Apply, reverse the hunk |
| `.` | What can be done with the file |
| `=` | Compare the file with the last commit in [Diff](diff.md) |
| `y` | Copy: `s` the hash or path, `y` the line, `r` the branches and tags |
| `C-S-c` | Commit what is staged |
| `C-S-s` / `C-S-a` | Stage all, stage every untracked file |
| `C-S-p` | Push the branch, no questions |
| `C-S-M-f` | Pull with rebase, no questions |
| `+` | Show more commits |
| `!` | Run any git command |
| `M-b` | Blame the file |
| `C-g` | Stop the running git command |

`s`, `u`, and `x` act on what is under the cursor: a section, a file, a hunk, or the lines of a `v`/`V` selection.

## Menus

These keys open a menu; the next key picks from it, and a card shows the choices.

| Key | Menu |
|---|---|
| `P` or `p` | Push |
| `F` | Pull |
| `f` | Fetch |
| `b` | Branch: checkout, create, delete, rename, spin off |
| `c` | Commit: commit, amend, reword, fixup, squash |
| `d` | Diff: working tree, staged, a revision, a range, a branch, a commit |
| `m` | Merge |
| `r` | Rebase, including interactive from the commit under the cursor |
| `t` | Tag |
| `Z` | Stash |
| `A` | Cherry-pick |
| `B` | Bisect |
| `M` | Remotes |
| `O` | Reset |
| `_` | Revert |
| `%` | Worktrees |
| `'` | Submodules |
| `i` | Ignore this path or extension |
| `C-l` | Log: this branch, another, every branch, this file, or a search |

In a menu, `-` then a letter turns on a switch such as `--force-with-lease`.

A commit, log, blame, or diff opens in a Git tab of its own. In it, `Enter` on a commit reads it, `y` copies the hash, and `q` goes back.

While a merge, rebase, cherry-pick, revert, or bisect is under way, a `State:` line says how far it has come and which keys finish it.

## Commit messages

In a Winter pane, `git commit` opens its message in the text editor, ready to type, with the staged diff in the next tab.

| Key | Action |
|---|---|
| `C-c C-c`, `C-S-c`, or `ZZ` | Commit |
| `C-c C-k`, `C-S-k`, or `ZQ` | Cancel the commit |
| `C-M-p` / `C-M-n` | An earlier message from the history, back again |

An interactive rebase's plan opens the same way. In Normal mode, `p` `r` `e` `s` `f` `d` set the line's action (pick, reword, edit, squash, fixup, drop), `C-c j` and `C-c k` move the line, and `C-c C-c` or `ZZ` finishes. Closing the plan any other way (`C-S-q`, `ZQ`, or `C-c C-k`) aborts the rebase.

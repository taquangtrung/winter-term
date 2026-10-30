//! The status view's keys: what each key does over the status, over the
//! output that replaces it, and as the second key of a menu.
//!
//! # Magit's keys, as evil-collection has them beside Vim's
//!
//! The view keeps magit's letters wherever Vim gives a list of rows nothing
//! better to do with them, and moves the rest as evil-collection-magit does:
//! `b` branch, `B` bisect, `M` remote, `p` push beside `P`; `x` discards and
//! `o` resets HEAD at the cursor; revert moves to `_` (`V` selects) and the
//! stash to `Z`, with the worktree on `%`. Three letters open small menus as
//! vim's prefixes: `g` jumps (`gs` staged, `gu` unstaged, `gn` untracked,
//! `gz` stashes, `gc` recent commits, `gj` `gk` the next and previous entry,
//! `gg` the top) and re-reads (`gr`); `y` copies (`yy` the line, `ys` the
//! hash or path, `yr` the branches and tags); `z` folds (`za` `zo` `zc`) and
//! scrolls (`zz` `zt` `zb`). `]` and `[` step between sections. `C-l` stays
//! the log menu.
//!
//! The row motions (`j` `k`, `gg` `G`, the paging chords, `H` `L`) come from
//! the shared layer (`model::vim::nav`); `h` `l`, `0` `^` `$`, and `w` `e`
//! move the column cursor the window keeps on every editor's cursor row,
//! where `b` `B` and `M` are magit's here; `v` and `V` select from there, and
//! `s` `u` `x` over a selection stage, unstage, or discard the lines in it.

use crate::editors::git_exec::{
    self, ApplyTarget, CommitMode, LogScope, Pathspec, ResetMode, Rev, SequenceStep,
};
use crate::model::editor::{EditorMenuItem, EditorOutcome, JobRequest, Notice};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::{CursorMove, Key, KeyCode, Mods};
use crate::model::vim::nav::{VimKey, VimNav};

use super::output::repaint_commit;
use crate::editors::git_parse::Section;
use super::popup::Popup;
use super::requests::Asked;
use super::rows::Item;
use super::{
    Discard, GitEditor, ASK_BRANCH_CHECKOUT, ASK_BRANCH_CREATE, ASK_BRANCH_CREATE_HERE, ASK_BRANCH_DELETE,
    ASK_BRANCH_DELETE_FORCE, ASK_CHERRY_PICK, ASK_CUSTOM, ASK_DIFF_REV, ASK_FETCH_REMOTE,
    ASK_BRANCH_RENAME, ASK_DIFF_BRANCH, ASK_DIFF_RANGE, ASK_LOG_OTHER, ASK_LOG_PICKAXE,
    ASK_LOG_REGEX, ASK_MERGE, ASK_MERGE_EDIT, ASK_MERGE_NO_COMMIT, ASK_MERGE_SQUASH,
    ASK_SHOW_COMMIT, ASK_SPIN_OFF,
    ASK_PULL_FROM, ASK_REMOTE_ADD,
    ASK_REMOTE_PRUNE, ASK_REMOTE_REMOVE, ASK_RESET_HARD, ASK_RESET_INDEX, ASK_RESET_MIXED,
    ASK_RESET_SOFT, ASK_RESET_WORKTREE,
    ASK_REVERT, ASK_STASH, ASK_SUBMODULE_ADD, ASK_SUBMODULE_DEINIT, ASK_TAG_CREATE, ASK_TAG_DELETE,
    ASK_WORKTREE_ADD, ASK_WORKTREE_REMOVE,
    LOG_PAGE, UPSTREAM,
};

// ========================================================================
// Constants
// ========================================================================

/// The status view's own keys, ahead of the shared motions.
pub(super) const STATUS_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Open,
        keys: &["RET"],
        label: "Open the file, read the commit or stash",
    },
    Binding {
        action: Act::OpenExternal,
        keys: &["C-o"],
        label: "Open the file in $EDITOR",
    },
    // `.` is "this one": what can be done with the file the cursor is
    // already on, without recalling which key each of them was.
    Binding {
        action: Act::Menu(Popup::File),
        keys: &["."],
        label: "File: what can be done with the file here",
    },
    Binding {
        action: Act::ToggleFold,
        keys: &["TAB"],
        label: "Fold the section, show a file's diff",
    },
    Binding {
        action: Act::ToggleFoldAll,
        keys: &["S-TAB"],
        label: "Show or hide every file's diff",
    },
    Binding {
        action: Act::Stage(false),
        keys: &["s"],
        label: "Stage the target",
    },
    Binding {
        action: Act::Stage(true),
        keys: &["S"],
        label: "Stage everything",
    },
    Binding {
        action: Act::Unstage(false),
        keys: &["u"],
        label: "Unstage the target",
    },
    Binding {
        action: Act::Unstage(true),
        keys: &["U"],
        label: "Unstage everything",
    },
    Binding {
        action: Act::Discard,
        keys: &["x"],
        label: "Discard the target, or drop the stash, after confirming",
    },
    Binding {
        action: Act::Apply(ApplyTarget::Index),
        keys: &["a"],
        label: "Apply the hunk, the stash, or the commit's changes",
    },
    Binding {
        action: Act::Apply(ApplyTarget::IndexReverse),
        keys: &["-"],
        label: "Reverse the hunk, or the commit's changes",
    },
    Binding {
        action: Act::Menu(Popup::Yank),
        keys: &["y"],
        label: "Yank...",
    },
    Binding {
        action: Act::StageAll,
        keys: &["C-S-s"],
        label: "Stage all: untracked here, else tracked changes",
    },
    Binding {
        action: Act::StageUntracked,
        keys: &["C-S-a"],
        label: "Stage every untracked file",
    },
    Binding {
        action: Act::CommitInEditor,
        keys: &["C-S-c"],
        label: "Commit what is staged, in the editor",
    },
    Binding {
        action: Act::Push,
        keys: &["P", "p"],
        label: "Push...",
    },
    Binding {
        action: Act::QuickPush,
        keys: &["C-S-p"],
        label: "Quick push: to the tracked remote, else origin",
    },
    Binding {
        action: Act::Pull,
        keys: &["F"],
        label: "Pull...",
    },
    Binding {
        action: Act::QuickPull,
        keys: &["C-S-M-f"],
        label: "Quick pull: from the tracked branch, else origin",
    },
    Binding {
        action: Act::Fetch,
        keys: &["f"],
        label: "Fetch...",
    },
    // The load-more key of a log, for the recent commits listed here.
    Binding {
        action: Act::MoreCommits,
        keys: &["+"],
        label: "Show more of the recent commits",
    },
    Binding {
        action: Act::Custom,
        keys: &["!"],
        label: "Run any git command",
    },
    Binding {
        action: Act::Blame,
        keys: &["M-b"],
        label: "Blame the file",
    },
    // magit's ediff, on dired's compare key `=` (magit's `e` and `E` are
    // the motions along a row here): the file as the last commit has it
    // against the working tree, in the diff tool, where the working tree's
    // side is edited.
    Binding {
        action: Act::DiffTool,
        keys: &["="],
        label: "Compare the file with HEAD in the diff tool",
    },
    Binding {
        action: Act::NextEntity(true),
        keys: &["]"],
        label: "Next section",
    },
    Binding {
        action: Act::NextEntity(false),
        keys: &["["],
        label: "Previous section",
    },
    Binding {
        action: Act::Menu(Popup::Jump),
        keys: &["g"],
        label: "Jump...",
    },
    Binding {
        action: Act::Menu(Popup::Fold),
        keys: &["z"],
        label: "Fold...",
    },
    Binding {
        action: Act::Menu(Popup::Log),
        keys: &["C-l"],
        label: "Log...",
    },
    // Menus, each opening on the key it does in the configured keymap.
    Binding {
        action: Act::Menu(Popup::Branch),
        keys: &["b"],
        label: "Branch...",
    },
    Binding {
        action: Act::Menu(Popup::Commit),
        keys: &["c"],
        label: "Commit...",
    },
    Binding {
        action: Act::Menu(Popup::Diff),
        keys: &["d"],
        label: "Diff...",
    },
    Binding {
        action: Act::Menu(Popup::Ignore),
        keys: &["i", "I"],
        label: "Ignore...",
    },
    Binding {
        action: Act::Menu(Popup::Merge),
        keys: &["m"],
        label: "Merge...",
    },
    Binding {
        action: Act::Menu(Popup::Rebase),
        keys: &["r"],
        label: "Rebase...",
    },
    Binding {
        action: Act::Menu(Popup::Tag),
        keys: &["t"],
        label: "Tag...",
    },
    Binding {
        action: Act::Menu(Popup::Stash),
        keys: &["Z"],
        label: "Stash...",
    },
    Binding {
        action: Act::Menu(Popup::CherryPick),
        keys: &["A"],
        label: "Cherry-pick...",
    },
    Binding {
        action: Act::Menu(Popup::Bisect),
        keys: &["B"],
        label: "Bisect...",
    },
    Binding {
        action: Act::Menu(Popup::Remote),
        keys: &["M"],
        label: "Remote...",
    },
    Binding {
        action: Act::Menu(Popup::Reset),
        keys: &["O"],
        label: "Reset...",
    },
    Binding {
        action: Act::Menu(Popup::Revert),
        keys: &["_"],
        label: "Revert...",
    },
    Binding {
        action: Act::Menu(Popup::Worktree),
        keys: &["%"],
        label: "Worktree...",
    },
    // Magit's key for its submodule menu.
    Binding {
        action: Act::Menu(Popup::Submodule),
        keys: &["'"],
        label: "Submodule...",
    },
    // Reset to a revision, straight from the cursor: the keymap's direct
    // mixed-reset key.
    Binding {
        action: Act::ResetHere,
        keys: &["o"],
        label: "Reset HEAD to the cursor, mixed",
    },
];

/// The keys while output (a commit, the log, blame) fills the view.
pub(super) const OUTPUT_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Open,
        keys: &["RET"],
        label: "Open the file the row belongs to, read the commit, or the branch's log",
    },
    Binding {
        action: Act::OpenExternal,
        keys: &["C-o"],
        label: "Open that file in $EDITOR",
    },
    Binding {
        action: Act::Discard,
        keys: &["x"],
        label: "Delete the branch or tag, after confirming",
    },
    Binding {
        action: Act::Down,
        keys: &["j", "<down>"],
        label: "Down",
    },
    Binding {
        action: Act::Up,
        keys: &["k", "<up>"],
        label: "Up",
    },
    Binding {
        action: Act::First,
        keys: &["<home>"],
        label: "Top",
    },
    Binding {
        action: Act::Last,
        keys: &["<end>"],
        label: "Bottom",
    },
    // A log's commits reword and rebase the way the status view's do: `c w`
    // and `r i` act on the commit at point.
    Binding {
        action: Act::Menu(Popup::Commit),
        keys: &["c"],
        label: "Commit...",
    },
    Binding {
        action: Act::Menu(Popup::Rebase),
        keys: &["r"],
        label: "Rebase...",
    },
    // The keymap's load-more key, which only a log can answer.
    Binding {
        action: Act::MoreCommits,
        keys: &["+"],
        label: "Show more of the log",
    },
    Binding {
        action: Act::Yank,
        keys: &["y"],
        label: "Copy the first word of the line",
    },
    Binding {
        action: Act::ToggleFoldAll,
        keys: &["S-TAB"],
        label: "Fold or unfold the whole commit",
    },
    Binding {
        action: Act::ToggleFold,
        keys: &["TAB"],
        label: "Fold or unfold the file or hunk",
    },
    Binding {
        action: Act::Back,
        keys: &["q", "ESC"],
        label: "Back to the status",
    },
];

/// The search keys, offered to both views before their own keys so that one
/// query walks whichever rows are showing.
pub(super) const SEARCH_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Search the view",
    },
    Binding {
        action: Act::SearchStep(true),
        keys: &["n"],
        label: "Next match",
    },
    Binding {
        action: Act::SearchStep(false),
        keys: &["N"],
        label: "Previous match",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the view's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Act {
    Apply(ApplyTarget),
    /// The file at point against `HEAD`, in the diff tool.
    DiffTool,
    Back,
    Blame,
    CommitInEditor,
    Custom,
    Discard,
    Down,
    Fetch,
    First,
    Last,
    Menu(Popup),
    MoreCommits,
    /// The next entry when set, else the previous one.
    NextEntity(bool),
    Open,
    OpenExternal,
    Pull,
    Push,
    QuickPull,
    QuickPush,
    ResetHere,
    Search,
    /// The next match when set, else the previous one.
    SearchStep(bool),
    /// Stage the target, or everything when set.
    Stage(bool),
    StageAll,
    StageUntracked,
    ToggleFold,
    ToggleFoldAll,
    /// Unstage the target, or everything when set.
    Unstage(bool),
    Up,
    /// Copy the first word of the output line.
    Yank,
}

// ========================================================================
// Functions
// ========================================================================

/// A menu entry over a row: its key from the status table, under a name that
/// says what it does to that kind of row.
pub(super) fn menu_item(act: Act, label: &str) -> Option<EditorMenuItem> {
    table::menu_entry(STATUS_KEYS, act).map(|(key, _)| EditorMenuItem::new(key, label))
}

/// A menu entry run by `choice` in the view's own menu `popup`: the key that
/// opens the menu, from the status table, and then the choice.
pub(super) fn menu_choice(popup: Popup, choice: char, label: &str) -> Option<EditorMenuItem> {
    table::menu_entry(STATUS_KEYS, Act::Menu(popup))
        .map(|(key, _)| EditorMenuItem::typed(vec![key, Key::plain(KeyCode::Char(choice))], label))
}

// ========================================================================
// GitEditor: keys
// ========================================================================

impl GitEditor {
    /// One key over whichever view is up: the open menu first, then the
    /// output, then the status view's own keys.
    pub(super) fn dispatch_key(&mut self, key: &Key) -> EditorOutcome {
        self.release_wheel();
        if let Some(popup) = self.menus.popup.take() {
            return match key.chord() {
                (Mods::NONE, code) => self.on_popup_key(popup, code),
                // A chord names no entry: the options menu stays up for one
                // that does, and any other menu closes.
                _ if popup == Popup::PushOptions => self.open_popup(Popup::PushOptions),
                _ => EditorOutcome::Consumed,
            };
        }
        // A chord with Space may arrive as the character it types.
        let key = &match key.code { // key-exact: rewrites the code alone, keeping every modifier
            KeyCode::Char(' ') => Key {
                code: KeyCode::Space,
                ..*key
            },
            _ => *key,
        };
        if self.output.is_some() {
            return self.on_output_key(key);
        }
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        match table::find(STATUS_KEYS, key).and_then(|act| self.on_status_act(act)) {
            Some(outcome) => outcome,
            // Everything unclaimed falls through to the shared Vim motion
            // layer, whose motions this view interprets over its rows; the
            // window keeps the rest.
            None => match self.nav.row_key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    EditorOutcome::Consumed
                }
                VimKey::Pending => EditorOutcome::Consumed,
                VimKey::Unhandled => EditorOutcome::Ignored,
            },
        }
    }

    /// Every key the view answers in the state it is in.
    pub(super) fn key_help(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(SEARCH_KEYS);
        match self.output {
            Some(_) => rows.extend(table::help(OUTPUT_KEYS)),
            None => {
                rows.extend(table::help(STATUS_KEYS));
                rows.extend(VimNav::help());
            }
        }
        rows
    }

    /// Do what a key of the status view's own asks. `None` for a key with
    /// nothing to do here, which then reaches the motion layer.
    fn on_status_act(&mut self, act: Act) -> Option<EditorOutcome> {
        let outcome = match act {
            Act::Open => self.open_or_read(),
            Act::OpenExternal => self.open_external(),
            Act::Menu(Popup::File) => match self.path_at_cursor() {
                Some(_) => self.open_popup(Popup::File),
                None => {
                    self.notice = Some(Notice::error("no file here"));
                    EditorOutcome::Consumed
                }
            },
            Act::Menu(popup) => self.open_popup(popup),
            Act::ToggleFold => self.toggle_fold(),
            Act::ToggleFoldAll => self.toggle_fold_all(),
            Act::Stage(all) => self.stage(all),
            Act::Unstage(all) => self.unstage(all),
            Act::Discard => self.ask_discard(),
            Act::Apply(target) => self.apply_at_point(target),
            Act::StageAll => self.stage_all(),
            Act::StageUntracked => self.stage_untracked(),
            Act::CommitInEditor => self.commit_in_editor(),
            Act::Push => self.push(),
            Act::QuickPush => self.quick_push(),
            Act::Pull => self.open_popup(Popup::Pull),
            Act::QuickPull => self.quick_pull(),
            Act::Fetch => self.open_popup(Popup::Fetch),
            Act::MoreCommits if self.repo.more_commits => {
                self.recent_count += LOG_PAGE;
                match &self.repo.root {
                    Some(root) => EditorOutcome::Job(git_exec::recent_log(root, self.recent_count)),
                    None => EditorOutcome::Consumed,
                }
            }
            Act::MoreCommits => return None,
            Act::Custom => self.ask(ASK_CUSTOM, "git "),
            Act::Blame => self.blame_at_point(),
            Act::DiffTool => self.diff_tool_at_point(),
            Act::NextEntity(forward) => {
                self.move_to_entity(forward);
                EditorOutcome::Consumed
            }
            Act::ResetHere => self.reset_at_point(ResetMode::Mixed),
            // The search keys are read before this, and the rest are the
            // output view's.
            Act::Search
            | Act::SearchStep(_)
            | Act::Back
            | Act::Down
            | Act::Up
            | Act::First
            | Act::Last
            | Act::Yank => return None,
        };
        Some(outcome)
    }

    /// `Enter` over the status: anything that belongs to a file opens that
    /// file, and a commit or a stash, which have none, are read instead.
    pub(super) fn open_or_read(&mut self) -> EditorOutcome {
        match self.selected().map(|row| row.item.clone()) {
            // Anything that belongs to a file opens that file: a path in a
            // section, a hunk of its diff, and the same two inside the
            // patch a commit is being read as.
            Some(Item::File(_))
            | Some(Item::Hunk(_))
            | Some(Item::CommitFile(_))
            | Some(Item::CommitHunk(_, _)) => self.open_at_point(),
            // A commit has no file to open, so Enter reads it instead:
            // what it changed, and the patch that changed it.
            Some(Item::Commit(hash)) => match (self.repo.root.clone(), Rev::parse(&hash)) {
                (Some(root), Ok(rev)) => EditorOutcome::Job(git_exec::show_commit(&root, &rev)),
                _ => EditorOutcome::Consumed,
            },
            // A stash reads as the diff it holds, the way a commit reads
            // as the patch it is.
            Some(Item::Stash(index)) => {
                let stash = self.repo.stashes.get(index).map(|stash| Rev::parse(&stash.name));
                match (self.repo.root.clone(), stash) {
                    (Some(root), Some(Ok(name))) => {
                        EditorOutcome::Job(git_exec::stash_show(&root, &name))
                    }
                    _ => EditorOutcome::Consumed,
                }
            }
            _ => EditorOutcome::Consumed,
        }
    }

    /// The search keys, offered to both views before their own keys so that one
    /// query walks whichever rows are showing. `None` for anything else.
    pub(super) fn on_search_key(&mut self, key: &Key) -> Option<EditorOutcome> {
        match table::find(SEARCH_KEYS, key)? {
            Act::Search => Some(self.ask_search()),
            Act::SearchStep(forward) => Some(self.search_step(forward)),
            _ => None,
        }
    }

    /// Keys while output fills the view: move, ask for more, or go back.
    pub(super) fn on_output_key(&mut self, key: &Key) -> EditorOutcome {
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        let Some(act) = table::find(OUTPUT_KEYS, key) else {
            // Everything unclaimed falls through to the shared Vim layer for
            // the motions between rows (`gg`, `G`, the paging chords, `H`
            // `M` `L`); the ones along a row are the window's column cursor's.
            let step = self.nav.row_key(key);
            let Some(output) = self.output.as_mut() else {
                return EditorOutcome::Consumed;
            };
            return match step {
                VimKey::Motion(motion) => {
                    output.list.apply_motion(motion);
                    EditorOutcome::Consumed
                }
                VimKey::Pending => EditorOutcome::Consumed,
                VimKey::Unhandled => EditorOutcome::Ignored,
            };
        };
        // A row of a commit's patch belongs to a file the same way a row of
        // the status view does, so it opens the same way, and a log's commit
        // is read as the status's is. A view whose rows stand for nothing (a
        // blame) keeps the key.
        match act {
            Act::Menu(popup) => return self.open_popup(popup),
            Act::Discard => return self.ask_delete_ref(),
            Act::Open => {
                if let Some(target) = self.file_at_point() {
                    return EditorOutcome::OpenPath(target);
                }
                // A branch or a tag is read as its log.
                if let Some(entry) = self.ref_at_cursor() {
                    return match (self.repo.root.clone(), Rev::parse(&entry.name)) {
                        (Some(root), Ok(rev)) => self.log_of(&root, LogScope::Other(rev), &[]),
                        _ => EditorOutcome::Consumed,
                    };
                }
                let commit = self
                    .output
                    .as_ref()
                    .and_then(|output| output.rows.get(output.list.cursor()))
                    .and_then(|row| match &row.item {
                        Item::Commit(hash) => Rev::parse(hash).ok(),
                        _ => None,
                    });
                if let (Some(rev), Some(root)) = (commit, self.repo.root.as_ref()) {
                    return EditorOutcome::Job(git_exec::show_commit(root, &rev));
                }
            }
            Act::OpenExternal => {
                if let Some(target) = self.file_at_point() {
                    return EditorOutcome::SpawnEditor(target);
                }
            }
            _ => {}
        }
        let Some(output) = self.output.as_mut() else {
            return EditorOutcome::Consumed;
        };
        let motion = match act {
            Act::Down => Some(CursorMove::Down),
            Act::Up => Some(CursorMove::Up),
            Act::First => Some(CursorMove::Top),
            Act::Last => Some(CursorMove::Bottom),
            _ => None,
        };
        if let Some(motion) = motion {
            output.list.apply_motion(motion);
            return EditorOutcome::Consumed;
        }
        match act {
            // More of the same log, whatever it is a log of.
            Act::MoreCommits if output.more => {
                self.log_count += LOG_PAGE;
                match self.view.clone() {
                    Some(JobRequest::Command(request)) if request.tag == git_exec::TAG_LOG_VIEW => {
                        EditorOutcome::Job(JobRequest::Command(git_exec::with_log_count(request, self.log_count)))
                    }
                    _ => EditorOutcome::Consumed,
                }
            }
            Act::Yank => {
                let line = output
                    .lines
                    .get(output.list.cursor())
                    .cloned()
                    .unwrap_or_default();
                let value = line.split_whitespace().next().unwrap_or("").to_string();
                if value.is_empty() {
                    EditorOutcome::Consumed
                } else {
                    EditorOutcome::Yank(value)
                }
            }
            // Fold or unfold the whole commit at once, every file and every
            // hunk, the same key that shows or hides every diff of the status.
            // Anything folded means the view is part-way shut, so the key
            // opens all of it; nothing folded means it shuts every file down
            // to its band, which is the commit read as a list of what it
            // touched, and every hunk with it, so a file opened again shows
            // its hunks' headers first.
            Act::ToggleFoldAll => {
                let Some(hunks) = output.commit.as_ref().map(|commit| {
                    commit
                        .files
                        .iter()
                        .map(|file| file.hunks.len())
                        .collect::<Vec<_>>()
                }) else {
                    // No commit, so nothing foldable: a log, a blame, or a
                    // diff is the lines it came back as.
                    return EditorOutcome::Consumed;
                };
                let open = !output.changes_shut
                    && output.folded_files.is_empty()
                    && output.folded_hunks.is_empty();
                output.changes_shut = false;
                output.folded_files.clear();
                output.folded_hunks.clear();
                if open {
                    // Shut to the file list rather than to the heading: the
                    // list of what the commit touched is what a shut commit is
                    // read for, and the heading alone says nothing.
                    output.folded_files.extend(0..hunks.len());
                    output.folded_hunks.extend(
                        hunks
                            .iter()
                            .enumerate()
                            .flat_map(|(file, &count)| (0..count).map(move |hunk| (file, hunk))),
                    );
                }
                repaint_commit(output);
                EditorOutcome::Consumed
            }
            // Fold or unfold what the cursor is on. A file row shuts its whole
            // diff away; a hunk row shuts the hunk's body but keeps the header,
            // so the file still says what it contains.
            Act::ToggleFold => {
                match output
                    .rows
                    .get(output.list.cursor())
                    .map(|row| row.item.clone())
                {
                    Some(Item::CommitChanges) => output.changes_shut = !output.changes_shut,
                    Some(Item::CommitFile(file)) => {
                        if !output.folded_files.remove(&file) {
                            output.folded_files.insert(file);
                        }
                    }
                    Some(Item::CommitHunk(file, hunk)) => {
                        if !output.folded_hunks.remove(&(file, hunk)) {
                            output.folded_hunks.insert((file, hunk));
                        }
                    }
                    // Nothing foldable under the cursor.
                    _ => return EditorOutcome::Consumed,
                }
                // The cursor stays on the row it was on: folding a file leaves
                // its band in place, so the reader does not lose their spot.
                repaint_commit(output);
                EditorOutcome::Consumed
            }
            // Back at the status, it is read again at once: what changed while
            // a log or a commit was up was waited out, not read.
            // A comparison, and a view in a tab of its own, have no status
            // to go back to: leaving one closes its tab, which goes back to
            // the tab it was opened from.
            Act::Back if self.comparison.is_some() || self.view_tab => EditorOutcome::Close,
            Act::Back => {
                self.output = None;
                self.view = None;
                self.refresh()
            }
            _ => EditorOutcome::Ignored,
        }
    }

    /// Act on a choice from the open menu. An unknown key closes the menu
    /// without doing anything, so a mistyped second key is harmless.
    pub(super) fn on_popup_key(&mut self, popup: Popup, code: KeyCode) -> EditorOutcome {
        // The options menu reads `RET` too, and stays up until it is pressed.
        if popup == Popup::PushOptions {
            return self.on_push_option_key(code);
        }
        let KeyCode::Char(choice) = code else {
            self.menus.switch_next = false;
            return EditorOutcome::Consumed;
        };
        // `-` and a key turn a switch on or off, and the menu stays up.
        if !popup.switches().is_empty() {
            if std::mem::take(&mut self.menus.switch_next) {
                self.menus.toggle_switch(popup, choice);
                return self.open_popup(popup);
            }
            if choice == '-' {
                self.menus.switch_next = true;
                return self.open_popup(popup);
            }
        }
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        match (popup, choice) {
            // A jump needs no repository: it moves within what is drawn.
            (Popup::Jump, _) => self.jump_to_section(code),
            (Popup::Fold, _) => self.fold_key(choice),
            (Popup::Yank, 'y') => self.yank_line(),
            (Popup::Yank, 's') => self.yank_at_point(),
            (Popup::Yank, 'r') => EditorOutcome::Job(git_exec::ref_list(&root)),
            (Popup::Push, _) => self.on_push_choice(&root, choice),
            (Popup::Branch, 'b') => {
                self.pick(ASK_BRANCH_CHECKOUT, "Checkout", git_exec::Candidates::Branches)
            }
            (Popup::Branch, 'c') => self.ask(ASK_BRANCH_CREATE, "Create and checkout: "),
            (Popup::Branch, 'n') => self.ask(ASK_BRANCH_CREATE_HERE, "Create branch: "),
            (Popup::Branch, 'd') => self.pick(
                ASK_BRANCH_DELETE,
                "Delete branch",
                git_exec::Candidates::LocalBranches,
            ),
            (Popup::Branch, 'D') => self.pick(
                ASK_BRANCH_DELETE_FORCE,
                "Delete unmerged branch",
                git_exec::Candidates::LocalBranches,
            ),
            (Popup::Branch, 'm') => self.ask(ASK_BRANCH_RENAME, "Rename this branch to: "),
            (Popup::Branch, 's') => self.ask(ASK_SPIN_OFF, "Spin off a branch named: "),

            (Popup::Commit, 'c') => self.commit(&root, CommitMode::New),
            (Popup::Commit, 'a') => self.commit(&root, CommitMode::Amend),
            // On a commit, that commit's message; anywhere else, the last.
            (Popup::Commit, 'w') => match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
                Some(Ok(commit)) => EditorOutcome::Job(git_exec::reword_from(&root, &commit)),
                Some(Err(_)) | None => self.commit(&root, CommitMode::Reword),
            },
            (Popup::Commit, 'e') => {
                EditorOutcome::Job(git_exec::amend_keeping_message(&root, &self.menus.flags_for(Popup::Commit)))
            }
            (Popup::Commit, 'f') => self.fixup(&root, false),
            (Popup::Commit, 'F') => self.fixup(&root, true),

            (Popup::Fetch, 'u') => match self.repo.upstream_remote() {
                Some(remote) => EditorOutcome::Job(git_exec::fetch_remote(&root, remote)),
                None => {
                    self.notice = Some(Notice::error("the branch tracks nothing to fetch from"));
                    EditorOutcome::Consumed
                }
            },
            (Popup::Fetch, 'e') => {
                self.pick(ASK_FETCH_REMOTE, "Fetch from", git_exec::Candidates::Remotes)
            }
            (Popup::Fetch, 'a') => EditorOutcome::Job(git_exec::fetch(&root)),
            (Popup::Fetch, 's') => EditorOutcome::Job(git_exec::fetch_with_submodules(&root)),

            (Popup::Pull, 'u') => EditorOutcome::Job(git_exec::pull(&root)),
            (Popup::Pull, 'm') => EditorOutcome::Job(git_exec::pull_merging(&root)),
            (Popup::Pull, 'e') => {
                self.pick(ASK_PULL_FROM, "Pull from", git_exec::Candidates::RemoteBranches)
            }

            (Popup::Merge, 'm') => self.pick(ASK_MERGE, "Merge", git_exec::Candidates::Branches),
            (Popup::Merge, 'e') => {
                self.pick(ASK_MERGE_EDIT, "Merge, editing the message", git_exec::Candidates::Branches)
            }
            (Popup::Merge, 'n') => {
                self.pick(ASK_MERGE_NO_COMMIT, "Merge without committing", git_exec::Candidates::Branches)
            }
            (Popup::Merge, 's') => self.pick(ASK_MERGE_SQUASH, "Squash in", git_exec::Candidates::Branches),
            (Popup::Merge, 'c') => {
                EditorOutcome::Job(git_exec::sequence(&root, "merge", SequenceStep::Continue))
            }
            (Popup::Merge, 'x') => {
                EditorOutcome::Job(git_exec::sequence(&root, "merge", SequenceStep::Abort))
            }

            // Interactive rebase runs in a terminal tab of its own, where git
            // waits on the plan it opens in the editor.
            (Popup::Rebase, 'i') => self.rebase_interactively(&root),
            (Popup::Rebase, 'u') => match Rev::parse(UPSTREAM) {
                Ok(upstream) => EditorOutcome::Job(git_exec::rebase(&root, &upstream)),
                Err(_) => EditorOutcome::Consumed,
            },
            (Popup::Rebase, 'c') => {
                EditorOutcome::Job(git_exec::sequence(&root, "rebase", SequenceStep::Continue))
            }
            (Popup::Rebase, 's') => {
                EditorOutcome::Job(git_exec::sequence(&root, "rebase", SequenceStep::Skip))
            }
            (Popup::Rebase, 'x') => {
                EditorOutcome::Job(git_exec::sequence(&root, "rebase", SequenceStep::Abort))
            }

            (Popup::Stash, 'z') => self.ask(ASK_STASH, "Stash message (optional): "),
            (Popup::Stash, 'i') => EditorOutcome::Job(git_exec::stash_push_with(&root, "--staged")),
            (Popup::Stash, 'x') => EditorOutcome::Job(git_exec::stash_push_with(&root, "--keep-index")),
            (Popup::Stash, 'u') => {
                EditorOutcome::Job(git_exec::stash_push_with(&root, "--include-untracked"))
            }
            // The stash at the cursor where there is one, as magit has it,
            // else the newest, as git does with none named.
            // Dropping cannot be taken back, so it asks first, as `x` does.
            (Popup::Stash, 'd') => {
                let newest = self.repo.stashes.first().and_then(|stash| Rev::parse(&stash.name).ok());
                match self.stash_at_cursor().or(newest) {
                    Some(name) => {
                        self.menus.pending_discard = Some(Discard::Stash(name));
                        self.ask_discard_pending()
                    }
                    None => {
                        self.notice = Some(Notice::error("no stash to drop"));
                        EditorOutcome::Consumed
                    }
                }
            }
            (Popup::Stash, verb @ ('p' | 'a')) => EditorOutcome::Job(match (verb, self.stash_at_cursor()) {
                ('p', Some(name)) => git_exec::stash_pop(&root, &name),
                (_, Some(name)) => git_exec::stash_apply(&root, &name),
                ('p', None) => git_exec::stash(&root, "pop"),
                (_, None) => git_exec::stash(&root, "apply"),
            }),
            (Popup::Stash, 'l') => EditorOutcome::Job(git_exec::stash_list(&root)),

            (Popup::Tag, 't') => self.ask(ASK_TAG_CREATE, "Tag name: "),
            (Popup::Tag, 'd') => self.pick(ASK_TAG_DELETE, "Delete tag", git_exec::Candidates::Tags),
            (Popup::Tag, 'l') => EditorOutcome::Job(git_exec::tag_list(&root)),

            (Popup::Remote, 'v') => EditorOutcome::Job(git_exec::remote_list(&root)),
            (Popup::Remote, 'a') => self.ask(ASK_REMOTE_ADD, "Remote, as `name url`: "),
            (Popup::Remote, 'd') => self.pick(
                ASK_REMOTE_REMOVE,
                "Remove remote",
                git_exec::Candidates::Remotes,
            ),
            (Popup::Remote, 'p') => {
                self.pick(ASK_REMOTE_PRUNE, "Prune remote", git_exec::Candidates::Remotes)
            }

            (Popup::CherryPick, 'a') => self.ask(ASK_CHERRY_PICK, "Cherry-pick: "),
            (Popup::CherryPick, 'c') => {
                EditorOutcome::Job(git_exec::sequence(&root, "cherry-pick", SequenceStep::Continue))
            }
            (Popup::CherryPick, 'x') => {
                EditorOutcome::Job(git_exec::sequence(&root, "cherry-pick", SequenceStep::Abort))
            }

            (Popup::Revert, 'v') => self.ask(ASK_REVERT, "Revert: "),
            (Popup::Revert, 'c') => {
                EditorOutcome::Job(git_exec::sequence(&root, "revert", SequenceStep::Continue))
            }
            (Popup::Revert, 'x') => {
                EditorOutcome::Job(git_exec::sequence(&root, "revert", SequenceStep::Abort))
            }

            (Popup::Reset, 'm') => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
            (Popup::Reset, 's') => self.ask(ASK_RESET_SOFT, "Reset (soft) to: "),
            // Hard reset throws away work with no way back, so it asks in the
            // same words as a delete and takes the revision as the answer.
            (Popup::Reset, 'h') => self.ask(ASK_RESET_HARD, "Reset HARD, losing everything, to: "),
            (Popup::Reset, 'i') => self.ask(ASK_RESET_INDEX, "Reset the index to: "),
            (Popup::Reset, 'w') => {
                self.ask(ASK_RESET_WORKTREE, "Reset the working tree, losing its changes, to: ")
            }

            (Popup::Diff, 'd') => EditorOutcome::Job(git_exec::diff_all(&root, false, None)),
            (Popup::Diff, 's') => EditorOutcome::Job(git_exec::diff_all(&root, true, None)),
            (Popup::Diff, 'r') => self.ask(ASK_DIFF_REV, "Diff against: "),
            (Popup::Diff, 'R') => self.ask(ASK_DIFF_RANGE, "Diff the range (a..b): "),
            (Popup::Diff, 'b') => {
                self.pick(ASK_DIFF_BRANCH, "Diff this branch against", git_exec::Candidates::Branches)
            }
            (Popup::Diff, 'c') => self.ask(ASK_SHOW_COMMIT, "Read the commit: "),

            // Every file choice acts on the row the popup was opened over.
            (Popup::File, side @ ('o' | 't')) => {
                let Some(path) = self.path_at_cursor() else {
                    return EditorOutcome::Consumed;
                };
                if self.section_at_cursor() != Some(Section::Unmerged) {
                    self.notice = Some(Notice::error(format!("{path} is not in conflict")));
                    return EditorOutcome::Consumed;
                }
                // Each answer stages the path its own request named, so
                // conflicts taken one after another each settle their own.
                let job = git_exec::resolve_conflict(&root, &Pathspec::new(path.clone()), side == 'o');
                EditorOutcome::Job(self.in_flight.ask(job, Asked::Resolve(path)))
            }
            (Popup::File, choice @ ('s' | 'u' | 'x' | 'd' | 'l' | 'b')) => {
                let Some(path) = self.path_at_cursor() else {
                    return EditorOutcome::Consumed;
                };
                match choice {
                    's' => self.stage(false),
                    'u' => self.unstage(false),
                    // Discarding cannot be taken back, so the menu asks
                    // first, the same as the key it stands for.
                    'x' => self.ask_discard(),
                    'b' => {
                        let job = git_exec::blame(&root, &Pathspec::new(path.clone()));
                        EditorOutcome::Job(self.in_flight.ask(job, Asked::Blame(path)))
                    }
                    'd' => self.toggle_fold(),
                    _ => self.log_of(&root, LogScope::File(Pathspec::new(path)), &[]),
                }
            }
            (Popup::Log, 'l') => self.log_of(&root, LogScope::Branch, &[]),
            (Popup::Log, 'o') => self.pick(ASK_LOG_OTHER, "Log", git_exec::Candidates::Branches),
            (Popup::Log, 'L') => self.log_of(&root, LogScope::LocalBranches, &[]),
            (Popup::Log, 'b') => self.log_of(&root, LogScope::Branches, &[]),
            (Popup::Log, 'a') => self.log_of(&root, LogScope::AllRefs, &[]),
            (Popup::Log, 'f') => match self.path_at_cursor() {
                Some(path) => self.log_of(&root, LogScope::File(Pathspec::new(path)), &[]),
                None => EditorOutcome::Consumed,
            },
            (Popup::Log, 'g') => self.ask(ASK_LOG_REGEX, "Log of changes matching (regex): "),
            (Popup::Log, 's') => self.ask(ASK_LOG_PICKAXE, "Log of changes adding or removing: "),

            (Popup::Ignore, 'i') => self.ignore_path(false),
            (Popup::Ignore, 'e') => self.ignore_path(true),

            (Popup::Submodule, 'l') => EditorOutcome::Job(git_exec::submodule_status(&root)),
            (Popup::Submodule, 'u') => EditorOutcome::Job(git_exec::submodule_update(&root)),
            (Popup::Submodule, 's') => EditorOutcome::Job(git_exec::submodule_sync(&root)),
            (Popup::Submodule, 'a') => {
                self.ask(ASK_SUBMODULE_ADD, "Submodule URL, then a path if not its name: ")
            }
            (Popup::Submodule, 'd') => self.ask(ASK_SUBMODULE_DEINIT, "Deinit submodule at: "),

            (Popup::Worktree, 'l') => EditorOutcome::Job(git_exec::worktree_list(&root)),
            (Popup::Worktree, 'a') => self.ask(ASK_WORKTREE_ADD, "Worktree path: "),
            (Popup::Worktree, 'd') => self.ask(ASK_WORKTREE_REMOVE, "Remove worktree: "),

            (Popup::Bisect, 's') => EditorOutcome::Job(git_exec::bisect(&root, "start")),
            (Popup::Bisect, 'g') => EditorOutcome::Job(git_exec::bisect(&root, "good")),
            (Popup::Bisect, 'b') => EditorOutcome::Job(git_exec::bisect(&root, "bad")),
            (Popup::Bisect, 'r') => EditorOutcome::Job(git_exec::bisect(&root, "reset")),

            _ => EditorOutcome::Consumed,
        }
    }
}

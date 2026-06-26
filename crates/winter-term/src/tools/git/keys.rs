//! The status view's keys: what each key does over the status, over the
//! output that replaces it, and as the second key of a menu.
//!
//! # Known conflicts with the shared Vim motions
//!
//! Every other tool leaves the shared motion keys (`model::vim::nav`) to
//! mean their motions and re-reads on `C-r`. This view still follows magit
//! instead, and is to be brought in line:
//!
//! - `G` refreshes, where it is the bottom everywhere else. Refresh moves to
//!   `C-r`.
//! - `M` opens the remote menu, so `H` and `L` work and `M` does not. The
//!   menu moves to `R`, which is free here.
//! - `V` opens the revert menu, where it starts a whole-line selection in
//!   every other tool. The menu moves to `_`.
//! - `o` resets HEAD at the cursor with no menu in between, so one stray key
//!   moves the branch. It is dropped; the reset menu (`O m`) still does it.
//! - `h` and `l` do nothing, where Dir and Processes fold and unfold with
//!   them. They collapse and expand a section, beside `Tab`.
//! - The output view (a commit, the log, blame) answers only `j`, `k`, `Home`
//!   and `End`: it is to fall through to the shared layer for `gg`, `G`, the
//!   paging chords, and `H`/`M`/`L`, as Grep's results now do.
//!
//! `z` stays the stash menu, after magit, though Dir folds with `z`: the two
//! tools are apart, and magit's habit is the stronger one in this view.

use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::{CursorMove, Key, KeyCode};
use crate::model::tool::{Notice, ToolMenuItem, ToolOutcome};
use crate::model::vim::nav::{buffer_end, VimKey, VimNav};

use super::exec::{
    self, ApplyTarget, CommitMode, LogScope, Pathspec, ResetMode, Rev, SequenceStep,
};
use super::output::repaint_commit;
use super::parse::Section;
use super::popup::Popup;
use super::rows::Item;
use super::{
    GitTool, ASK_BRANCH_CHECKOUT, ASK_BRANCH_CREATE, ASK_BRANCH_CREATE_HERE, ASK_BRANCH_DELETE,
    ASK_BRANCH_DELETE_FORCE, ASK_CHERRY_PICK, ASK_CUSTOM, ASK_DIFF_REV, ASK_MERGE, ASK_REMOTE_ADD,
    ASK_REMOTE_PRUNE, ASK_REMOTE_REMOVE, ASK_RESET_HARD, ASK_RESET_MIXED, ASK_RESET_SOFT,
    ASK_REVERT, ASK_STASH, ASK_TAG_CREATE, ASK_TAG_DELETE, ASK_WORKTREE_ADD, ASK_WORKTREE_REMOVE,
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
        label: "Fold everything",
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
        label: "Discard the target, after confirming",
    },
    Binding {
        action: Act::Apply(ApplyTarget::Index),
        keys: &["a"],
        label: "Apply the hunk",
    },
    Binding {
        action: Act::Apply(ApplyTarget::IndexReverse),
        keys: &["-"],
        label: "Reverse the hunk",
    },
    Binding {
        action: Act::Yank,
        keys: &["y"],
        label: "Copy the hash or path",
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
        action: Act::MarkLine,
        keys: &["C-SPC"],
        label: "Set the mark on a diff line, or clear it",
    },
    Binding {
        action: Act::ApplyLines(false),
        keys: &["M-s"],
        label: "Stage only some lines",
    },
    Binding {
        action: Act::ApplyLines(true),
        keys: &["M-u"],
        label: "Unstage only some lines",
    },
    Binding {
        action: Act::CommitInEditor,
        keys: &["C-S-c"],
        label: "Commit what is staged, in the editor",
    },
    Binding {
        action: Act::Push,
        keys: &["P"],
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
        label: "Pull, rebasing",
    },
    Binding {
        action: Act::QuickPull,
        keys: &["C-S-M-f"],
        label: "Quick pull: from the tracked branch, else origin",
    },
    Binding {
        action: Act::Fetch,
        keys: &["f"],
        label: "Fetch all",
    },
    Binding {
        action: Act::Refresh,
        keys: &["G"],
        label: "Re-read the working tree",
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
    Binding {
        action: Act::ShowRefs,
        keys: &["M-y"],
        label: "Show every ref",
    },
    Binding {
        action: Act::NextEntity(true),
        keys: &["M-n"],
        label: "Next entry",
    },
    Binding {
        action: Act::NextEntity(false),
        keys: &["M-p"],
        label: "Previous entry",
    },
    Binding {
        action: Act::Menu(Popup::Jump),
        keys: &["g"],
        label: "Jump...",
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
        keys: &["z"],
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
        keys: &["V"],
        label: "Revert...",
    },
    Binding {
        action: Act::Menu(Popup::Worktree),
        keys: &["Z"],
        label: "Worktree...",
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
        label: "Open the file the row belongs to",
    },
    Binding {
        action: Act::OpenExternal,
        keys: &["C-o"],
        label: "Open that file in $EDITOR",
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
    /// Stage only some lines, or unstage them when set.
    ApplyLines(bool),
    Back,
    Blame,
    CommitInEditor,
    Custom,
    Discard,
    Down,
    Fetch,
    First,
    Last,
    MarkLine,
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
    Refresh,
    ResetHere,
    Search,
    /// The next match when set, else the previous one.
    SearchStep(bool),
    ShowRefs,
    /// Stage the target, or everything when set.
    Stage(bool),
    StageAll,
    StageUntracked,
    ToggleFold,
    ToggleFoldAll,
    /// Unstage the target, or everything when set.
    Unstage(bool),
    Up,
    Yank,
}

// ========================================================================
// Functions
// ========================================================================

/// A menu entry over a row: its key from the status table, under a name that
/// says what it does to that kind of row.
pub(super) fn menu_item(act: Act, label: &str) -> Option<ToolMenuItem> {
    table::menu_entry(STATUS_KEYS, act).map(|(key, _)| ToolMenuItem::new(key, label))
}

// ========================================================================
// GitTool: keys
// ========================================================================

impl GitTool {
    /// One key over whichever view is up: the open menu first, then the
    /// output, then the status view's own keys.
    pub(super) fn dispatch_key(&mut self, key: &Key) -> ToolOutcome {
        self.release_wheel();
        if let Some(popup) = self.popup.take() {
            return self.on_popup_key(popup, key.glyph());
        }
        // A chord with Space may arrive as the character it types.
        let key = &match key.code {
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
            None => match self.nav.key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    ToolOutcome::Consumed
                }
                VimKey::Pending => ToolOutcome::Consumed,
                VimKey::Unhandled => ToolOutcome::Ignored,
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
    fn on_status_act(&mut self, act: Act) -> Option<ToolOutcome> {
        let outcome = match act {
            Act::Open => self.open_or_read(),
            Act::OpenExternal => self.open_external(),
            Act::Menu(Popup::File) => match self.path_at_cursor() {
                Some(_) => self.open_popup(Popup::File),
                None => {
                    self.notice = Some(Notice::error("no file here"));
                    ToolOutcome::Consumed
                }
            },
            Act::Menu(popup) => self.open_popup(popup),
            Act::ToggleFold => self.toggle_fold(),
            Act::ToggleFoldAll => {
                self.toggle_fold_all();
                ToolOutcome::Consumed
            }
            Act::Stage(all) => self.stage(all),
            Act::Unstage(all) => self.unstage(all),
            Act::Discard => self.ask_discard(),
            Act::Apply(target) => self.apply_at_point(target),
            Act::Yank => self.yank_at_point(),
            Act::StageAll => self.stage_all(),
            Act::StageUntracked => self.stage_untracked(),
            Act::MarkLine => self.toggle_line_mark(),
            Act::ApplyLines(reverse) => self.apply_lines(reverse),
            Act::CommitInEditor => self.commit_in_editor(),
            Act::Push => self.push(),
            Act::QuickPush => self.quick_push(),
            Act::Pull => match &self.root {
                Some(root) => ToolOutcome::Job(exec::pull(root)),
                None => ToolOutcome::Consumed,
            },
            Act::QuickPull => self.quick_pull(),
            Act::Fetch => match &self.root {
                Some(root) => ToolOutcome::Job(exec::fetch(root)),
                None => ToolOutcome::Consumed,
            },
            Act::Refresh => self.refresh(),
            Act::MoreCommits if self.more_commits => {
                self.recent_count += LOG_PAGE;
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::recent_log(root, self.recent_count)),
                    None => ToolOutcome::Consumed,
                }
            }
            Act::MoreCommits => return None,
            Act::Custom => self.ask(ASK_CUSTOM, "git "),
            Act::Blame => self.blame_at_point(),
            Act::ShowRefs => match &self.root {
                Some(root) => ToolOutcome::Job(exec::show_refs(root)),
                None => ToolOutcome::Consumed,
            },
            Act::NextEntity(forward) => {
                self.move_to_entity(forward);
                ToolOutcome::Consumed
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
            | Act::Last => return None,
        };
        Some(outcome)
    }

    /// `Enter` over the status: anything that belongs to a file opens that
    /// file, and a commit or a stash, which have none, are read instead.
    fn open_or_read(&mut self) -> ToolOutcome {
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
            Some(Item::Commit(hash)) => match (self.root.clone(), Rev::parse(&hash)) {
                (Some(root), Ok(rev)) => ToolOutcome::Job(exec::show_commit(&root, &rev)),
                _ => ToolOutcome::Consumed,
            },
            // A stash reads as the diff it holds, the way a commit reads
            // as the patch it is.
            Some(Item::Stash(index)) => {
                let stash = self.stashes.get(index).map(|stash| Rev::parse(&stash.name));
                match (self.root.clone(), stash) {
                    (Some(root), Some(Ok(name))) => {
                        ToolOutcome::Job(exec::stash_show(&root, &name))
                    }
                    _ => ToolOutcome::Consumed,
                }
            }
            _ => ToolOutcome::Consumed,
        }
    }

    /// The search keys, offered to both views before their own keys so that one
    /// query walks whichever rows are showing. `None` for anything else.
    pub(super) fn on_search_key(&mut self, key: &Key) -> Option<ToolOutcome> {
        match table::find(SEARCH_KEYS, key)? {
            Act::Search => Some(self.ask_search()),
            Act::SearchStep(forward) => Some(self.search_step(forward)),
            _ => None,
        }
    }

    /// Keys while output fills the view: move, ask for more, or go back.
    pub(super) fn on_output_key(&mut self, key: &Key) -> ToolOutcome {
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        let Some(act) = table::find(OUTPUT_KEYS, key) else {
            let Some(output) = self.output.as_mut() else {
                return ToolOutcome::Consumed;
            };
            return match buffer_end(key) {
                Some(motion) => {
                    output.list.apply_motion(motion);
                    ToolOutcome::Consumed
                }
                None => ToolOutcome::Ignored,
            };
        };
        // A row of a commit's patch belongs to a file the same way a row of
        // the status view does, so it opens the same way. A view whose rows
        // stand for nothing (a log, a blame) keeps the key.
        match act {
            Act::Menu(popup) => return self.open_popup(popup),
            Act::Open => {
                if let Some(target) = self.file_at_point() {
                    return ToolOutcome::OpenPath(target);
                }
            }
            Act::OpenExternal => {
                if let Some(target) = self.file_at_point() {
                    return ToolOutcome::SpawnEditor(target);
                }
            }
            _ => {}
        }
        let Some(output) = self.output.as_mut() else {
            return ToolOutcome::Consumed;
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
            return ToolOutcome::Consumed;
        }
        match act {
            Act::MoreCommits if output.more => {
                self.log_count += LOG_PAGE;
                match self.root.clone() {
                    Some(root) => {
                        ToolOutcome::Job(exec::log(&root, LogScope::Branch, self.log_count))
                    }
                    None => ToolOutcome::Consumed,
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
                    ToolOutcome::Consumed
                } else {
                    ToolOutcome::Yank(value)
                }
            }
            // Fold or unfold the whole commit at once, the same key the status
            // view folds its sections with. Anything folded means the view is
            // part-way shut, so the key opens all of it; nothing folded means
            // it shuts every file down to its band, which is the commit read
            // as a list of what it touched.
            Act::ToggleFoldAll => {
                let Some(count) = output.commit.as_ref().map(|commit| commit.files.len()) else {
                    // No commit, so nothing foldable: a log, a blame, or a
                    // diff is the lines it came back as.
                    return ToolOutcome::Consumed;
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
                    output.folded_files.extend(0..count);
                }
                repaint_commit(output);
                ToolOutcome::Consumed
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
                    _ => return ToolOutcome::Consumed,
                }
                // The cursor stays on the row it was on: folding a file leaves
                // its band in place, so the reader does not lose their spot.
                repaint_commit(output);
                ToolOutcome::Consumed
            }
            // Back at the status, it is read again at once: what changed while
            // a log or a commit was up was waited out, not read.
            Act::Back => {
                self.output = None;
                self.view = None;
                self.refresh()
            }
            _ => ToolOutcome::Ignored,
        }
    }

    /// Act on a choice from the open menu. An unknown key closes the menu
    /// without doing anything, so a mistyped second key is harmless.
    pub(super) fn on_popup_key(&mut self, popup: Popup, code: KeyCode) -> ToolOutcome {
        // The options menu reads `RET` too, and stays up until it is pressed.
        if popup == Popup::PushOptions {
            return self.on_push_option_key(code);
        }
        let KeyCode::Char(choice) = code else {
            return ToolOutcome::Consumed;
        };
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        match (popup, choice) {
            // A jump needs no repository: it moves within what is drawn.
            (Popup::Jump, _) => self.jump_to_section(code),
            (Popup::Push, _) => self.on_push_choice(&root, choice),
            (Popup::Branch, 'b') => {
                self.pick(ASK_BRANCH_CHECKOUT, "Checkout", exec::Candidates::Branches)
            }
            (Popup::Branch, 'c') => self.ask(ASK_BRANCH_CREATE, "Create and checkout: "),
            (Popup::Branch, 'n') => self.ask(ASK_BRANCH_CREATE_HERE, "Create branch: "),
            (Popup::Branch, 'd') => self.pick(
                ASK_BRANCH_DELETE,
                "Delete branch",
                exec::Candidates::LocalBranches,
            ),
            (Popup::Branch, 'D') => self.pick(
                ASK_BRANCH_DELETE_FORCE,
                "Delete unmerged branch",
                exec::Candidates::LocalBranches,
            ),

            (Popup::Commit, 'c') => self.commit(&root, CommitMode::New),
            (Popup::Commit, 'a') => self.commit(&root, CommitMode::Amend),
            // On a commit, that commit's message; anywhere else, the last.
            (Popup::Commit, 'w') => match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
                Some(Ok(commit)) => ToolOutcome::Job(exec::reword_from(&root, &commit)),
                Some(Err(_)) | None => self.commit(&root, CommitMode::Reword),
            },
            (Popup::Commit, 'e') => ToolOutcome::Job(exec::amend_keeping_message(&root)),

            (Popup::Merge, 'm') => self.pick(ASK_MERGE, "Merge", exec::Candidates::Branches),
            (Popup::Merge, 'c') => {
                ToolOutcome::Job(exec::sequence(&root, "merge", SequenceStep::Continue))
            }
            (Popup::Merge, 'x') => {
                ToolOutcome::Job(exec::sequence(&root, "merge", SequenceStep::Abort))
            }

            // Interactive rebase runs in a terminal tab of its own, where git
            // waits on the plan it opens in the editor.
            (Popup::Rebase, 'i') => self.rebase_interactively(&root),
            (Popup::Rebase, 'u') => match Rev::parse(UPSTREAM) {
                Ok(upstream) => ToolOutcome::Job(exec::rebase(&root, &upstream)),
                Err(_) => ToolOutcome::Consumed,
            },
            (Popup::Rebase, 'c') => {
                ToolOutcome::Job(exec::sequence(&root, "rebase", SequenceStep::Continue))
            }
            (Popup::Rebase, 's') => {
                ToolOutcome::Job(exec::sequence(&root, "rebase", SequenceStep::Skip))
            }
            (Popup::Rebase, 'x') => {
                ToolOutcome::Job(exec::sequence(&root, "rebase", SequenceStep::Abort))
            }

            (Popup::Stash, 'z') => self.ask(ASK_STASH, "Stash message (optional): "),
            (Popup::Stash, 'p') => ToolOutcome::Job(exec::stash(&root, "pop")),
            (Popup::Stash, 'a') => ToolOutcome::Job(exec::stash(&root, "apply")),
            (Popup::Stash, 'd') => ToolOutcome::Job(exec::stash(&root, "drop")),
            (Popup::Stash, 'l') => ToolOutcome::Job(exec::stash_list(&root)),

            (Popup::Tag, 't') => self.ask(ASK_TAG_CREATE, "Tag name: "),
            (Popup::Tag, 'd') => self.pick(ASK_TAG_DELETE, "Delete tag", exec::Candidates::Tags),
            (Popup::Tag, 'l') => ToolOutcome::Job(exec::tag_list(&root)),

            (Popup::Remote, 'v') => ToolOutcome::Job(exec::remote_list(&root)),
            (Popup::Remote, 'a') => self.ask(ASK_REMOTE_ADD, "Remote, as `name url`: "),
            (Popup::Remote, 'd') => self.pick(
                ASK_REMOTE_REMOVE,
                "Remove remote",
                exec::Candidates::Remotes,
            ),
            (Popup::Remote, 'p') => {
                self.pick(ASK_REMOTE_PRUNE, "Prune remote", exec::Candidates::Remotes)
            }

            (Popup::CherryPick, 'a') => self.ask(ASK_CHERRY_PICK, "Cherry-pick: "),
            (Popup::CherryPick, 'c') => {
                ToolOutcome::Job(exec::sequence(&root, "cherry-pick", SequenceStep::Continue))
            }
            (Popup::CherryPick, 'x') => {
                ToolOutcome::Job(exec::sequence(&root, "cherry-pick", SequenceStep::Abort))
            }

            (Popup::Revert, 'v') => self.ask(ASK_REVERT, "Revert: "),
            (Popup::Revert, 'c') => {
                ToolOutcome::Job(exec::sequence(&root, "revert", SequenceStep::Continue))
            }
            (Popup::Revert, 'x') => {
                ToolOutcome::Job(exec::sequence(&root, "revert", SequenceStep::Abort))
            }

            (Popup::Reset, 'm') => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
            (Popup::Reset, 's') => self.ask(ASK_RESET_SOFT, "Reset (soft) to: "),
            // Hard reset throws away work with no way back, so it asks in the
            // same words as a delete and takes the revision as the answer.
            (Popup::Reset, 'h') => self.ask(ASK_RESET_HARD, "Reset HARD, losing everything, to: "),

            (Popup::Diff, 'd') => ToolOutcome::Job(exec::diff_all(&root, false, None)),
            (Popup::Diff, 's') => ToolOutcome::Job(exec::diff_all(&root, true, None)),
            (Popup::Diff, 'r') => self.ask(ASK_DIFF_REV, "Diff against: "),

            // Every file choice acts on the row the popup was opened over.
            (Popup::File, side @ ('o' | 't')) => {
                let Some(path) = self.path_at_cursor() else {
                    return ToolOutcome::Consumed;
                };
                if self.section_at_cursor() != Some(Section::Unmerged) {
                    self.notice = Some(Notice::error(format!("{path} is not in conflict")));
                    return ToolOutcome::Consumed;
                }
                self.resolving = Some(path.clone());
                ToolOutcome::Job(exec::resolve_conflict(
                    &root,
                    &Pathspec::new(path),
                    side == 'o',
                ))
            }
            (Popup::File, choice @ ('s' | 'u' | 'x' | 'd' | 'l' | 'b')) => {
                let Some(path) = self.path_at_cursor() else {
                    return ToolOutcome::Consumed;
                };
                match choice {
                    's' => self.stage(false),
                    'u' => self.unstage(false),
                    // Discarding cannot be taken back, so the menu asks
                    // first, the same as the key it stands for.
                    'x' => self.ask_discard(),
                    'b' => {
                        self.pending_blame = Some(path.clone());
                        ToolOutcome::Job(exec::blame(&root, &Pathspec::new(path)))
                    }
                    'd' => self.toggle_fold(),
                    _ => {
                        self.log_count = LOG_PAGE;
                        let scope = LogScope::File(Pathspec::new(path));
                        ToolOutcome::Job(exec::log(&root, scope, self.log_count))
                    }
                }
            }
            (Popup::Log, 'l') => {
                self.log_count = LOG_PAGE;
                ToolOutcome::Job(exec::log(&root, LogScope::Branch, self.log_count))
            }
            (Popup::Log, 'a') => {
                self.log_count = LOG_PAGE;
                ToolOutcome::Job(exec::log(&root, LogScope::AllRefs, self.log_count))
            }
            (Popup::Log, 'f') => match self.path_at_cursor() {
                Some(path) => {
                    self.log_count = LOG_PAGE;
                    let scope = LogScope::File(Pathspec::new(path));
                    ToolOutcome::Job(exec::log(&root, scope, self.log_count))
                }
                None => ToolOutcome::Consumed,
            },

            (Popup::Ignore, 'i') => self.ignore_path(false),
            (Popup::Ignore, 'e') => self.ignore_path(true),

            (Popup::Worktree, 'l') => ToolOutcome::Job(exec::worktree_list(&root)),
            (Popup::Worktree, 'a') => self.ask(ASK_WORKTREE_ADD, "Worktree path: "),
            (Popup::Worktree, 'd') => self.ask(ASK_WORKTREE_REMOVE, "Remove worktree: "),

            (Popup::Bisect, 's') => ToolOutcome::Job(exec::bisect(&root, "start")),
            (Popup::Bisect, 'g') => ToolOutcome::Job(exec::bisect(&root, "good")),
            (Popup::Bisect, 'b') => ToolOutcome::Job(exec::bisect(&root, "bad")),
            (Popup::Bisect, 'r') => ToolOutcome::Job(exec::bisect(&root, "reset")),

            _ => ToolOutcome::Consumed,
        }
    }
}

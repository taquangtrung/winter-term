//! Git: the working tree's state in a pane, and the keys that change it.
//!
//! - [`diff`]: reading a unified diff, and writing one hunk back out.
//! - [`git_exec`](crate::editors::git_exec), shared with the editors that run git too: the git command lines the view runs.
//! - [`git_parse`](crate::editors::git_parse), shared: reading git's own output.
//! - [`popup`]: the transient menus a key opens.
//! - `requests`: what each request in flight was asked for, by the id its
//!   reply carries.
//! - [`rows`]: painting the view, and what each row stands for.
//! - [`watch`]: noticing that the repository changed under the view.
//! - [`word_diff`](crate::editors::word_diff), shared: the word-level diff that decorates a hunk's changed lines.
//!
//! The editor's state is grouped by what it holds: what git last said of the
//! repository (`RepoState`), the menus and questions in progress
//! (`MenuState`), the watch over the tree (`WatchState`), and the requests in
//! flight; the rest is the view.
//!
//! The view itself is split by what it answers: `keys` (a key press),
//! `prompt` (an answered question), `reply` (a finished command), and
//! `output` (the log, blame, or commit that fills the view in place of the
//! status). What the keys do is split by what they act on: `staging`
//! (staging, unstaging, and discarding), `refs` (branches, remotes, stashes,
//! and commits), and `at_point` (whatever the cursor is on).

mod at_point;
pub mod commit;
mod compare;
pub mod diff;
mod keys;
mod output;
pub mod popup;
pub mod progress;
mod prompt;
mod push;
mod refs;
mod reply;
mod requests;
pub mod rows;
mod staging;
pub mod watch;

use std::collections::{HashMap, HashSet};
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::editors::background::{self, Background};
use crate::editors::git_exec;
use crate::editors::list::{ListPlace, ListView};
use crate::bytes_text;
use crate::editors::text_file;
use crate::model::editor::{
    menu_groups, CommandRequest, CompareRequest, Editor, EditorCommand, EditorHint, EditorMenuItem, EditorOutcome,
    GitViewRequest, JobReply,
    JobRequest, JumpStep, JumpTarget, Notice, OpenTarget, PickQuestion, PromptReply, SearchReport, TokenInspectData, HIDE_SCOPES, SHOW_SCOPES,
};
use crate::model::input::table::KeyHelp;
use crate::model::input::{CursorMove, Key, KeyCode, Mods};
use crate::model::paint::{row_height, row_text, Frame, Icon, Point, Span, Style};
use crate::model::vim::nav::VimNav;

use commit::CommitFile;
use compare::Comparison;
use diff::FileDiff;
use crate::editors::git_exec::{ApplyTarget, Pathspec, Rev};
use keys::{menu_choice, menu_item, Act};
use output::Output;
use crate::editors::git_parse::{Commit, Section, Stash, Status};
use popup::Popup;
use progress::Progress;
use requests::{Asked, DiffAsked, InFlight};
use rows::{Block, FileRow, Item, RefRow, ViewRow};

// ========================================================================
// Constants
// ========================================================================

/// The thread the diff tool's working-tree copy is read on.
const WORKTREE_READ_THREAD: &str = "winter diff read";

/// The commands the `git` block of `keybindings.kdl` can bind.
pub const COMMANDS: &[EditorCommand] = &[EditorCommand {
    label: "Open Remote in Browser",
    name: OPEN_REMOTE,
}];

/// The name `open_remote` is bound under.
const OPEN_REMOTE: &str = "open_remote";

/// Prompt tags, one per question the view asks.
/// Characters of a commit hash shown in a view's title: enough to identify it
/// at a glance without crowding out the subject beside it.
const COMMIT_TITLE_HASH_LEN: usize = 8;

/// Said once when the repository cannot be watched, which leaves the view
/// read again only when asked.
const UNWATCHED: &str = "git: this repository cannot be watched for changes; refresh to read it";

const ASK_BRANCH_CHECKOUT: &str = "branch-checkout";
const ASK_BRANCH_CREATE: &str = "branch-create";
const ASK_BRANCH_CREATE_HERE: &str = "branch-create-here";
const ASK_BRANCH_DELETE: &str = "branch-delete";
const ASK_BRANCH_DELETE_FORCE: &str = "branch-delete-force";
const ASK_CHERRY_PICK: &str = "cherry-pick";
const ASK_CUSTOM: &str = "custom";
const ASK_DIFF_REV: &str = "diff-rev";
const ASK_DISCARD: &str = "discard";
const ASK_FETCH_REMOTE: &str = "fetch-remote";
const ASK_FIXUP: &str = "fixup";
const ASK_LOG_OTHER: &str = "log-other";
const ASK_LOG_PICKAXE: &str = "log-pickaxe";
const ASK_LOG_REGEX: &str = "log-regex";
const ASK_INSTANT_FIXUP: &str = "instant-fixup";
const ASK_MERGE: &str = "merge";
const ASK_MERGE_EDIT: &str = "merge-edit";
const ASK_MERGE_NO_COMMIT: &str = "merge-no-commit";
const ASK_MERGE_SQUASH: &str = "merge-squash";
const ASK_BRANCH_RENAME: &str = "branch-rename";
const ASK_SPIN_OFF: &str = "spin-off";
const ASK_DIFF_RANGE: &str = "diff-range";
const ASK_DIFF_BRANCH: &str = "diff-branch";
const ASK_SHOW_COMMIT: &str = "show-commit";
const ASK_REBASE: &str = "rebase";
const ASK_REMOTE_ADD: &str = "remote-add";
const ASK_REMOTE_PRUNE: &str = "remote-prune";
const ASK_REMOTE_REMOVE: &str = "remote-remove";
/// What every git tab's title starts with.
const TITLE_PREFIX: &str = "Git";

/// What a tab showing the working tree's state is called.
const STATUS_TITLE: &str = "Status";

/// What the view of the branches and tags is called.
const REFS_TITLE: &str = "Refs";

/// What parts a commit view's title from the subject after it.
const SUBJECT_GAP: &str = "  ";

const ASK_PULL_FROM: &str = "pull-from";
const ASK_PUSH_ELSEWHERE: &str = "push-elsewhere";
const ASK_PUSH_OTHER: &str = "push-other";
const ASK_PUSH_OTHER_TARGET: &str = "push-other-target";
const ASK_PUSH_REMOTE: &str = "push-remote";
const ASK_PUSH_TAG: &str = "push-tag";
const ASK_PUSH_UPSTREAM: &str = "push-upstream";
const ASK_RESET_HARD: &str = "reset-hard";
const ASK_RESET_INDEX: &str = "reset-index";
const ASK_RESET_MIXED: &str = "reset-mixed";
const ASK_RESET_SOFT: &str = "reset-soft";
const ASK_RESET_WORKTREE: &str = "reset-worktree";
const ASK_REVERT: &str = "revert";
const ASK_SEARCH: &str = "search";
const ASK_STASH: &str = "stash";
const ASK_TAG_CREATE: &str = "tag-create";
const ASK_TAG_DELETE: &str = "tag-delete";
const ASK_SUBMODULE_ADD: &str = "submodule-add";
const ASK_SUBMODULE_DEINIT: &str = "submodule-deinit";
const ASK_WORKTREE_ADD: &str = "worktree-add";
const ASK_WORKTREE_REMOVE: &str = "worktree-remove";

/// Commits the log view asks for at a time, and adds on each request for more.
const LOG_PAGE: usize = 50;

/// The remote a browser is pointed at, whose refs are pruned by default, and
/// that a branch tracking nothing is pushed to.
const DEFAULT_REMOTE: &str = "origin";

/// What a comparison's tab is called, before what it compares.
const COMPARE_TITLE: &str = "Diff";

/// What a comparison of two files that are the same shows.
const NO_DIFFERENCES: &str = "no differences: the two are the same";

/// Shown while the first status has not come back yet.
const LOADING: &str = "reading the repository...";

/// Shown when the directory the view opened in is not in a repository.
const NOT_A_REPO: &str = "not a git repository";

/// Shown when the repository was found but reading its status failed.
const UNREADABLE: &str = "the repository could not be read";

/// Reported when a read the view would fill itself with came back empty.
const NOTHING_TO_SHOW: &str = "nothing to show";

/// What the thread appending to `.gitignore` is called.
const IGNORE_THREAD: &str = "winter git ignore";

/// How long the key waits for its `.gitignore` line to be written before
/// the frame is drawn without the answer.
#[cfg(not(test))]
const IGNORE_WAIT: Duration = Duration::from_millis(40);

/// Long enough that a test's append always lands with its key.
#[cfg(test)]
const IGNORE_WAIT: Duration = Duration::from_secs(10);

/// What a `.gitignore` pattern reads as more than itself.
const IGNORE_SPECIAL: &str = "*?[\\";

/// What git calls the branch the current one tracks.
const UPSTREAM: &str = "@{upstream}";

/// Rows of header above the first section.
const HEADER_ROWS: usize = 1;

/// What the menu opened over a row calls each of the entries it offers. A key
/// acts on the marked targets when there are any, so these name the command
/// rather than the row it was opened on.
const LABEL_APPLY_HUNK: &str = "Apply This Hunk";
const LABEL_BLAME: &str = "Blame";
const LABEL_COMMIT: &str = "Commit Staged...";
const LABEL_COPY_HASH: &str = "Copy Hash";
const LABEL_COPY_PATH: &str = "Copy Path";
const LABEL_DIFF: &str = "Show Diff";
const LABEL_DISCARD: &str = "Discard...";
const LABEL_DISCARD_HUNK: &str = "Discard This Hunk...";
const LABEL_FETCH: &str = "Fetch...";
const LABEL_FOLD: &str = "Fold or Unfold";
const LABEL_OPEN: &str = "Open";
const LABEL_OPEN_AT_LINE: &str = "Open at This Line";
const LABEL_OPEN_IN_EDITOR: &str = "Open in $EDITOR";
const LABEL_PULL: &str = "Pull...";
const LABEL_PUSH: &str = "Push";
const LABEL_READ_COMMIT: &str = "Read This Commit";
const LABEL_READ_STASH: &str = "Read This Stash";
const LABEL_RELOAD: &str = "Reload";
const LABEL_REVERSE_HUNK: &str = "Reverse This Hunk";
const LABEL_STAGE: &str = "Stage";
const LABEL_STAGE_ALL: &str = "Stage Everything";
const LABEL_STAGE_HUNK: &str = "Stage This Hunk";
const LABEL_STASH_MENU: &str = "Stash...";
const LABEL_UNSTAGE: &str = "Unstage";
const LABEL_UNSTAGE_ALL: &str = "Unstage Everything";
const LABEL_UNSTAGE_HUNK: &str = "Unstage This Hunk";

// ========================================================================
// Data Structures
// ========================================================================

/// The working tree's state, and the cursor over it.
pub struct GitEditor {
    /// Noticing that the repository changed.
    watch: WatchState,
    /// The menus and questions in progress.
    menus: MenuState,
    /// What git last said about the repository.
    repo: RepoState,
    /// Sections the user has shut. `None` stands for the recent-commits
    /// section, which has no `Section` of its own.
    collapsed: HashSet<Block>,
    /// Files whose diff is showing.
    expanded: HashSet<FileRow>,
    /// The cursor over the status rows, kept on its row across re-reads.
    list: ListView<RowKey>,
    /// What the last command reported, waiting for the status bar to take it.
    notice: Option<Notice>,
    /// Available pane width in columns.
    pub(super) cols: usize,
    rows: Vec<ViewRow>,
    /// An append to `.gitignore` still being written.
    ignoring: Option<Background<Result<String, String>>>,
    /// Whether token and scope inspection is active over the cursor.
    inspect_scopes: bool,
    /// The diff tool's working-tree read under way, with `HEAD`'s copy it
    /// is compared with once it lands.
    head_compare: Option<HeadCompare>,
    /// What each request in flight was asked for, by the id its reply
    /// carries: a diff's file, a blame's path, a resolve's conflict.
    in_flight: InFlight,
    /// Output shown in place of the status view: a log, a blame, a listing.
    /// `None` when the status view is showing.
    output: Option<Output>,
    /// The request the output was read with, asked again when a command run
    /// from it (a reword, a rebase from a log's commit) changes the
    /// repository, so the output never goes on showing what was.
    view: Option<JobRequest>,
    /// Whether the output coming back is the one showing read again, which
    /// keeps the cursor and the folds where they were.
    keep_place: bool,
    /// How many commits the log view last asked for.
    log_count: usize,
    /// How many recent commits the status view asks for, which `+` raises.
    recent_count: usize,
    /// Where the editor was opened, which is where the root is looked up from.
    start: PathBuf,
    /// The shared Vim motion state: the layer the view's unclaimed keys fall
    /// through to. Its own `g` leader claims the prefix before this sees it.
    nav: VimNav,
    /// Whether this editor only shows what is staged, as a diff, rather than
    /// the working tree's status: the view beside a commit message.
    staged_only: bool,
    /// The two files this editor compares, for a view of no repository at
    /// all but of the differences between them.
    comparison: Option<Comparison>,
    /// Whether this editor is a view in a tab of its own (a commit, a log),
    /// opened from the status or another view, rather than the status: it
    /// reads its view where the status would read the working tree, and
    /// leaving it closes the tab.
    view_tab: bool,
}

/// A file's copy in `HEAD`, waiting for the working tree's to be read for
/// the diff tool to compare them.
struct HeadCompare {
    /// Where the working tree's copy is.
    file: PathBuf,
    /// `HEAD`'s copy, one string per line.
    head: Vec<String>,
    /// The file, from the repository's root, as the sides are labelled.
    path: String,
    /// The working tree's copy being read: `None` for one with no text.
    read: Background<Option<text_file::LoadedFile>>,
}

/// What git last said about the repository: where it is, its status, its commits and stashes, what it is part-way through, and the diffs read of its files.
#[derive(Default)]
struct RepoState {
    commits: Vec<Commit>,
    /// Diffs already read, keyed by the row that asked for one. A file's
    /// working-tree and index diffs are different things, so the section is
    /// part of the key.
    diffs: HashMap<FileRow, FileDiff>,
    /// Where the repository keeps its own files, which is where the state of
    /// whatever git is part-way through is written.
    git_dir: Option<PathBuf>,
    /// The last status git answered with, as it came back.
    last_status: Option<String>,
    /// Set once a status has come back, so an empty view can say which it is:
    /// a clean tree, or one still being read.
    loaded: bool,
    /// Whether the history goes on past the recent commits shown, so `+`
    /// has more to load.
    more_commits: bool,
    /// What git is part-way through, when it is part-way through anything.
    progress: Option<Progress>,
    /// Where the repository is rooted, once git has said.
    root: Option<PathBuf>,
    /// The stashes, newest first.
    stashes: Vec<Stash>,
    status: Status,
    /// Counts status reads that threw the diffs away.
    status_reads: u64,
    /// What the upstream has and this branch does not, newest first.
    unpulled: Vec<Commit>,
    /// Every remote, by name, read with each refresh: a ref is split into
    /// its remote and its branch at the longest one it starts with.
    remotes: Vec<String>,
}

/// The menus and questions in progress: the menu waiting for its second key and its switches, the push menu, and what a pick or a confirmation is asking about.
#[derive(Default)]
struct MenuState {
    /// The discard a confirmation is asking about, taken when it was asked
    /// so the answer acts on exactly what the question named, whatever a
    /// re-read has done to the rows since.
    pending_discard: Option<Discard>,
    /// The question a list of names is being gathered for.
    pending_pick: Option<PickQuestion>,
    /// The menu waiting for its second key.
    popup: Option<Popup>,
    /// The push menu while it is open or a push it started is being
    /// answered: where the branch pushes, and the switches turned on.
    push: Option<push::PushMenu>,
    /// Whether `-` was just pressed in the open menu, so the next key turns
    /// one of its switches on or off.
    switch_next: bool,
    /// The switches turned on, by menu and key, kept while the view is.
    switches_on: Vec<(Popup, char)>,
}

/// Noticing that the repository changed: the watch over the tree, one being set up, and what a change asks to be read again.
#[derive(Default)]
struct WatchState {
    /// The repository's file notifications, which say when to read it again;
    /// nothing reads it on a timer.
    tree_watch: Option<watch::TreeWatch>,
    /// The tree watch being set up on a thread of its own (see
    /// [`watch::TreeWatch::start_in_background`]).
    tree_watch_starting: Option<Background<Option<watch::TreeWatch>>>,
    /// What the status read in flight was the watch's for, if it was its at
    /// all, so an answer that changed nothing can be told from one that must
    /// redraw, and a change to the files alone reads no more than the status.
    watching: Option<watch::Read>,
}

/// What names a status row across re-reads: what it stands for, and which of
/// the rows standing for that it is, since every line of a hunk stands for
/// the hunk and every blank line for nothing.
#[derive(Clone, Debug, Eq, PartialEq)]
struct RowKey {
    /// What the row stands for.
    item: Item,
    /// How many rows before it stand for the same.
    nth: usize,
}

/// A discard the reader has been asked to confirm.
#[derive(Clone, Debug, Eq, PartialEq)]
enum Discard {
    /// Reverse one hunk out of the working tree, by its own patch.
    Hunk(String),
    /// Reverse some lines of a hunk out of the working tree, by their patch.
    Lines(String),
    /// Delete a branch, local or on its remote, or a tag.
    Ref(RefRow),
    /// Drop one stash.
    Stash(Rev),
    /// Drop every stash.
    Stashes,
    /// Restore tracked files from the index.
    Tracked(Vec<Pathspec>),
    /// Delete untracked files.
    Untracked(Vec<Pathspec>),
}

/// What `s`, `u`, or `x` does to the rows a selection covers.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RowsAct {
    Discard,
    Stage,
    Unstage,
}

/// A command the `git` block of `keybindings.kdl` can bind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum GitCommand {
    /// Open the remote in a browser.
    OpenRemote,
}

// ========================================================================
// GitCommand
// ========================================================================

impl GitCommand {
    /// The command a `git` block names, or `None` for a name it does not know.
    fn from_name(name: &str) -> Option<Self> {
        match name {
            OPEN_REMOTE => Some(GitCommand::OpenRemote),
            _ => None,
        }
    }
}

// ========================================================================
// GitEditor
// ========================================================================

impl GitEditor {
    /// A view of the repository containing `start`, which is asked for first.
    pub fn new(start: PathBuf) -> Self {
        Self {
            menus: MenuState::default(),
            repo: RepoState::default(),
            watch: WatchState::default(),
            collapsed: HashSet::new(),
            expanded: HashSet::new(),
            list: ListView::new(),
            notice: None,
            log_count: LOG_PAGE,
            recent_count: git_exec::RECENT_COMMITS,
            output: None,
            view: None,
            keep_place: false,
            head_compare: None,
            in_flight: InFlight::default(),
            ignoring: None,
            inspect_scopes: false,
            cols: 0,
            rows: Vec::new(),
            start,
            nav: VimNav::new(),
            staged_only: false,
            comparison: None,
            view_tab: false,
        }
    }

    /// A view of what is staged in the repository containing `start`, as the
    /// diff a commit is about to record, in place of the working tree's status.
    pub fn staged_diff(start: PathBuf) -> Self {
        Self {
            staged_only: true,
            ..Self::new(start)
        }
    }

    /// A view of the differences between the two files `request` names,
    /// rather than of a repository: the diff, folded and read as a commit is.
    pub fn comparing(request: CompareRequest) -> Self {
        let start = request.old.parent().map(Path::to_path_buf).unwrap_or_default();
        let comparison = Comparison::new(request);
        Self {
            view: Some(comparison.diff_request()),
            comparison: Some(comparison),
            ..Self::new(start)
        }
    }

    /// A view in a tab of its own, reading `view`: a commit, a log, a blame.
    pub fn viewing(view: GitViewRequest) -> Self {
        Self {
            repo: RepoState {
                git_dir: view.git_dir,
                loaded: true,
                root: Some(view.root.clone()),
                ..RepoState::default()
            },
            view: Some(view.request),
            view_tab: true,
            ..Self::new(view.root)
        }
    }

    /// Remember the request a view filling the output is read with, so a
    /// command that changes the repository under it can read it again. A
    /// view asked for afresh starts at its top.
    ///
    /// The status reads no view in place: each opens in a tab of its own, as
    /// does one a view asks for of another kind, a commit from a log. A view
    /// read again as itself, a log loading more, stays where it is.
    fn note_view(&mut self, outcome: EditorOutcome) -> EditorOutcome {
        if let EditorOutcome::Job(JobRequest::Command(request)) = &outcome {
            if let Some(view) = self.view_elsewhere(request) {
                return EditorOutcome::GitView(view);
            }
            if git_exec::VIEW_TAGS.contains(&request.tag) {
                self.view = Some(JobRequest::Command(request.clone()));
                self.keep_place = false;
            }
        }
        self.announce(outcome)
    }

    /// The tab of its own `request` is to be read in, where it is a view this
    /// editor does not read in place.
    fn view_elsewhere(&self, request: &CommandRequest) -> Option<GitViewRequest> {
        if !git_exec::VIEW_TAGS.contains(&request.tag) || self.staged_only || self.comparison.is_some() {
            return None;
        }
        let own_kind = self.view.as_ref().is_some_and(|view| {
            matches!(view, JobRequest::Command(own) if own.tag == request.tag)
        });
        if self.view_tab && own_kind {
            return None;
        }
        Some(GitViewRequest {
            git_dir: self.repo.git_dir.clone(),
            request: JobRequest::Command(request.clone()),
            root: self.repo.root.clone()?,
        })
    }

    /// Name a command that changes the repository in the status bar as it
    /// starts (`git push origin main`), so what a key ran is never a guess.
    /// Left out of the notification history, which hears how it ended.
    fn announce(&mut self, outcome: EditorOutcome) -> EditorOutcome {
        if let EditorOutcome::Job(JobRequest::Command(request)) = &outcome {
            if git_exec::CHANGE_TAGS.contains(&request.tag) {
                self.notice = Some(Notice::info(git_exec::summary(request)).unlogged());
            }
        }
        outcome
    }

    /// What to read after a command changed the repository: the output
    /// showing, read again where it stays up, else the status. Leaving the
    /// output reads the status anyway.
    fn reread(&mut self) -> EditorOutcome {
        match self.output.is_some().then(|| self.view.clone()).flatten() {
            Some(view) => {
                self.keep_place = true;
                EditorOutcome::Job(view)
            }
            None => self.refresh(),
        }
    }

    /// The first request the view makes: where the repository is, or the
    /// differences between the two files a comparison is of.
    pub fn initial_request(&self) -> JobRequest {
        match &self.comparison {
            Some(comparison) => comparison.diff_request(),
            None => git_exec::repo_root(&self.start),
        }
    }

    /// Re-read the working tree.
    fn refresh(&self) -> EditorOutcome {
        if let (true, Some(view)) = (self.view_tab, &self.view) {
            return EditorOutcome::Job(view.clone());
        }
        match &self.repo.root {
            Some(root) => EditorOutcome::Job(git_exec::status(root)),
            None => EditorOutcome::Job(self.initial_request()),
        }
    }

    fn rebuild(&mut self) {
        let collapsed = self.collapsed.clone();
        let expanded = self.expanded.clone();
        let diffs = self.repo.diffs.clone();
        self.rows = rows::build(
            rows::StatusContent {
                status: &self.repo.status,
                commits: &self.repo.commits,
                more_commits: self.repo.more_commits,
                unpulled: &self.repo.unpulled,
                stashes: &self.repo.stashes,
                progress: self.repo.progress.as_ref(),
                root: self.repo.root.as_deref(),
                now: now_unix(),
                cols: (self.cols > 0).then_some(self.cols),
            },
            &|block| collapsed.contains(&block),
            &|file| {
                expanded
                    .contains(file)
                    .then(|| diffs.get(file).cloned())
                    .flatten()
            },
        );
        self.list.set_keys(row_keys(&self.rows));
    }

    fn selected(&self) -> Option<&ViewRow> {
        self.rows.get(self.list.cursor())
    }

    /// A key or an answer arrived: the cursor places the view again, in
    /// whichever view the wheel was holding.
    fn release_wheel(&mut self) {
        self.list.view_mut().release();
        if let Some(output) = self.output.as_mut() {
            output.list.view_mut().release();
        }
    }

    /// The paths a key acts on: the file under the cursor, or every file in the
    /// section whose heading it is on.
    fn targets(&self) -> Vec<String> {
        match self.selected().map(|row| &row.item) {
            Some(Item::File(file)) => vec![file.path.clone()],
            Some(Item::Hunk(hunk)) => vec![hunk.path.clone()],
            Some(Item::Heading(section)) => self.repo.paths_in(*section),
            // The commit-view items only ever appear in an output view, which
            // has its own key handling and never asks for working-tree targets.
            Some(Item::Commit(_))
            | Some(Item::CommitChanges)
            | Some(Item::CommitFile(_))
            | Some(Item::CommitHunk(_, _))
            | Some(Item::RecentHeading)
            | Some(Item::Stash(_))
            | Some(Item::StashHeading)
            | Some(Item::UnpulledHeading)
            | Some(Item::None)
            | Some(Item::Ref(_))
            | None => Vec::new(),
        }
    }

    /// Which section the cursor is in, taken from the row itself so a heading
    /// and its entries agree.
    fn section_at_cursor(&self) -> Option<Section> {
        match self.selected().map(|row| &row.item) {
            Some(Item::File(file)) => Some(file.section),
            Some(Item::Hunk(hunk)) => Some(hunk.section),
            Some(Item::Heading(section)) => Some(*section),
            _ => None,
        }
    }

    /// Move to the next or previous row that stands for something, skipping the
    /// blank lines between sections.
    fn move_to_entity(&mut self, forward: bool) {
        let at = self.list.cursor();
        let found = match forward {
            true => (at + 1..self.rows.len()).find(|&i| self.rows[i].item != Item::None),
            false => (0..at).rev().find(|&i| self.rows[i].item != Item::None),
        };
        if let Some(index) = found {
            self.list.set_cursor(index);
        }
    }

    /// Remember `query` and move to the first row holding it.
    fn search_for(&mut self, query: &str) -> EditorOutcome {
        self.search_with(query, true)
    }

    /// Move to the next row matching the last search, in whichever view is up:
    /// one query serves the status and the output it opens.
    fn search_step(&mut self, forward: bool) -> EditorOutcome {
        let query = self.list.search().to_string();
        self.search_with(&query, forward)
    }

    /// Step to the next or previous row holding `query`, remembering it.
    fn search_with(&mut self, query: &str, forward: bool) -> EditorOutcome {
        let notice = match self.output.as_mut() {
            Some(output) => {
                // The status list keeps the query, so `n` after the output
                // closes carries on with it.
                self.list.set_search(query);
                let labels = output.lines.clone();
                output.list.search_with(&labels, query, forward)
            }
            None => {
                let labels: Vec<String> =
                    self.rows.iter().map(|row| row_text(&row.spans)).collect();
                self.list.search_with(&labels, query, forward)
            }
        };
        if notice.is_some() {
            self.notice = notice;
        }
        EditorOutcome::Consumed
    }

    /// The next or previous section heading in the direction of `forward`,
    /// from wherever the cursor sits: the view's paragraph motion, since the
    /// blank lines between sections are its paragraph boundaries.
    fn move_to_boundary(&mut self, forward: bool) {
        let is_heading = |row: &ViewRow| matches!(row.item, Item::Heading(_) | Item::RecentHeading);
        let at = self.list.cursor();
        let target = match forward {
            true => (at + 1..self.rows.len()).find(|&i| is_heading(&self.rows[i])),
            false => (0..at).rev().find(|&i| is_heading(&self.rows[i])),
        };
        if let Some(index) = target {
            self.list.set_cursor(index);
        }
    }

    /// Interpret one of the shared Vim motions over the view's rows: the
    /// section headings are the paragraphs. The motions along a row are the
    /// window's column cursor's.
    fn apply_motion(&mut self, motion: CursorMove) {
        if self.list.apply_motion(motion) {
            return;
        }
        match motion {
            CursorMove::ParagraphForward => self.move_to_boundary(true),
            CursorMove::ParagraphBack => self.move_to_boundary(false),
            _ => {}
        }
    }

    /// Jump to a section's heading, which is how the `g` keys navigate.
    fn move_to_section(&mut self, section: Option<Section>) {
        let target = self.rows.iter().position(|row| match (&row.item, section) {
            (Item::Heading(found), Some(want)) => *found == want,
            (Item::RecentHeading, None) => true,
            _ => false,
        });
        if let Some(index) = target {
            self.list.set_cursor(index);
        }
    }

    /// Fold what the cursor is on: a heading shuts the block it heads, a file
    /// shows or hides its own diff, and a hunk folds the file it belongs to.
    fn toggle_fold(&mut self) -> EditorOutcome {
        let item = self.selected().map(|row| row.item.clone());
        if let Some(block) = item.as_ref().and_then(rows::fold_block) {
            self.toggle_block(block);
            return EditorOutcome::Consumed;
        }
        match item {
            Some(Item::File(file)) => self.toggle_file(file),
            Some(Item::Hunk(hunk)) => {
                let file = FileRow {
                    path: hunk.path,
                    section: hunk.section,
                };
                self.expanded.remove(&file);
                self.rebuild();
                EditorOutcome::Consumed
            }
            // The headings resolved above, and a commit view folds through
            // its own handler over its own fold sets.
            _ => EditorOutcome::Consumed,
        }
    }

    fn toggle_block(&mut self, block: Block) {
        if !self.collapsed.remove(&block) {
            self.collapsed.insert(block);
        }
        self.rebuild();
    }

    /// Show or hide one file's diff, reading it the first time it is asked for.
    fn toggle_file(&mut self, file: FileRow) -> EditorOutcome {
        if self.expanded.remove(&file) {
            self.rebuild();
            return EditorOutcome::Consumed;
        }
        self.expanded.insert(file);
        self.rebuild();
        self.request_stale_diff()
    }

    /// Show every listed file's diff, in every section, or shut them all
    /// again, the way magic-vscode's recursive toggle does: the first file
    /// listed says which way. Showing them opens the sections they are in,
    /// and reads each diff not read yet, one at a time.
    fn toggle_fold_all(&mut self) -> EditorOutcome {
        let files: Vec<FileRow> = Section::all()
            .into_iter()
            .flat_map(|section| {
                self.repo.status
                    .files
                    .iter()
                    .filter(move |file| file.section == section)
                    .map(|file| FileRow {
                        path: file.path.clone(),
                        section: file.section,
                    })
            })
            .collect();
        let Some(first) = files.first() else {
            return EditorOutcome::Consumed;
        };
        if self.expanded.contains(first) {
            self.expanded.clear();
            self.rebuild();
            return EditorOutcome::Consumed;
        }
        for file in &files {
            self.collapsed.remove(&Block::Files(file.section));
        }
        self.expanded.extend(files);
        self.rebuild();
        self.request_stale_diff()
    }

    /// Ask for the next open diff that has not been read since the last
    /// status, one at a time so a view of twenty open files does not run twenty
    /// commands at once. While one is being read, nothing: its answer asks
    /// for the next when it lands.
    fn request_stale_diff(&mut self) -> EditorOutcome {
        if self.in_flight.is_asking(|asked| matches!(asked, Asked::Diff(_))) {
            return EditorOutcome::Consumed;
        }
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        let stale = self
            .rows
            .iter()
            .filter_map(|row| match &row.item {
                Item::File(file) => Some(file.clone()),
                _ => None,
            })
            .find(|file| self.expanded.contains(file) && !self.repo.diffs.contains_key(file));
        match stale {
            Some(file) => {
                let request = diff_request(&root, &file);
                let asked = Asked::Diff(DiffAsked { file, under: self.repo.status_reads });
                EditorOutcome::Job(self.in_flight.ask(request, asked))
            }
            None => EditorOutcome::Consumed,
        }
    }

    /// Drop the expansion of a file the status no longer lists: staged away, or
    /// committed, it has no diff to show.
    fn forget_unlisted_files(&mut self) {
        let listed: HashSet<FileRow> = self
            .repo.status
            .files
            .iter()
            .map(|file| FileRow {
                path: file.path.clone(),
                section: file.section,
            })
            .collect();
        self.expanded.retain(|file| listed.contains(file));
    }

    /// Take the tree watch over once its thread has set it up, saying so
    /// when the repository could not be watched.
    fn adopt_started_watch(&mut self) {
        let Some(starting) = self.watch.tree_watch_starting.take_if(|s| s.is_done()) else {
            return;
        };
        self.watch.tree_watch = starting.poll().flatten();
        if self.watch.tree_watch.is_none() {
            self.notice = Some(Notice::warning(UNWATCHED));
        }
    }

    /// Resolve the key after `g`: each names a section to jump to, and
    /// anything else abandons the sequence.
    fn jump_to_section(&mut self, code: KeyCode) -> EditorOutcome {
        match code {
            // `gg` is the one Vim motion the view's own leader resolves,
            // because the leader claims the prefix first.
            KeyCode::Char('g') => self.list.set_cursor(0),
            KeyCode::Char('n') => self.move_to_section(Some(Section::Untracked)),
            KeyCode::Char('u') => self.move_to_section(Some(Section::Unstaged)),
            KeyCode::Char('s') => self.move_to_section(Some(Section::Staged)),
            KeyCode::Char('z') => self.move_to_stashes(),
            KeyCode::Char('c') => self.move_to_section(None),
            KeyCode::Char('j') => self.move_to_entity(true),
            KeyCode::Char('k') => self.move_to_entity(false),
            KeyCode::Char('r') => return self.refresh(),
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// Put the cursor on the stash list's heading, where there is one.
    fn move_to_stashes(&mut self) {
        if let Some(index) = self.rows.iter().position(|row| row.item == Item::StashHeading) {
            self.list.set_cursor(index);
        }
    }

    /// Vim's `z` keys after the fold menu's `z`: fold what the cursor is on
    /// (`a` either way, `o` open, `c` shut), or scroll its row into place.
    fn fold_key(&mut self, choice: char) -> EditorOutcome {
        let motion = match choice {
            'a' => return self.toggle_fold(),
            'o' => return self.set_fold(true),
            'c' => return self.set_fold(false),
            'z' => CursorMove::LineToCenter,
            't' => CursorMove::LineToTop,
            'b' => CursorMove::LineToBottom,
            _ => return EditorOutcome::Consumed,
        };
        self.apply_motion(motion);
        EditorOutcome::Consumed
    }

    /// Whether what the cursor is on shows what folds under it; `None` where
    /// nothing folds.
    fn is_fold_open(&self) -> Option<bool> {
        let item = self.selected().map(|row| row.item.clone())?;
        if let Some(block) = rows::fold_block(&item) {
            return Some(!self.collapsed.contains(&block));
        }
        match item {
            Item::File(file) => Some(self.expanded.contains(&file)),
            // A hunk shows: folding it shuts the file it belongs to.
            Item::Hunk(_) => Some(true),
            _ => None,
        }
    }

    /// Open (or, not `open`, shut) the fold the cursor is on, leaving one
    /// already so as it is.
    fn set_fold(&mut self, open: bool) -> EditorOutcome {
        match self.is_fold_open() {
            Some(shown) if shown != open => self.toggle_fold(),
            Some(_) | None => EditorOutcome::Consumed,
        }
    }

    /// Open the file at point, at whatever line the row knows about.
    fn open_at_point(&self) -> EditorOutcome {
        match self.file_at_point() {
            Some(target) => EditorOutcome::OpenPath(target),
            None => EditorOutcome::Consumed,
        }
    }

    /// Hand the file at point to `$EDITOR`, in a pane of its own, for the
    /// editing the app's own editor deliberately cannot do.
    fn open_external(&self) -> EditorOutcome {
        match self.file_at_point() {
            Some(target) => EditorOutcome::SpawnEditor(target),
            None => EditorOutcome::Consumed,
        }
    }

    /// What the row under the cursor stands for, in whichever view is
    /// showing: a commit's patch has rows and a cursor of its own.
    fn item_at_point(&self) -> Option<Item> {
        match self.output.as_ref() {
            Some(output) => output
                .rows
                .get(output.list.cursor())
                .map(|row| row.item.clone()),
            None => self.selected().map(|row| row.item.clone()),
        }
    }

    /// How far the cursor sits into the run of rows standing for the same
    /// item as the row at point: zero on a hunk's header, one on its first
    /// line, and so on.
    fn offset_into_item(&self) -> usize {
        let (rows, cursor) = match self.output.as_ref() {
            Some(output) => (&output.rows, output.list.cursor()),
            None => (&self.rows, self.list.cursor()),
        };
        let Some(item) = rows.get(cursor).map(|row| &row.item) else {
            return 0;
        };
        rows[..cursor]
            .iter()
            .rev()
            .take_while(|row| &row.item == item)
            .count()
    }

    /// One file of the commit being read, by its index among the files it
    /// touched.
    fn commit_file(&self, index: usize) -> Option<&CommitFile> {
        self.output.as_ref()?.commit.as_ref()?.files.get(index)
    }

    /// The working-tree file the row at point belongs to, and where in it to
    /// land: a hunk opens at the line under the cursor, or where the hunk
    /// starts from its header, rather than at the top, in a commit's patch as
    /// much as in the working tree's own.
    ///
    /// A row standing for a commit, a stash, or a section heading belongs to
    /// no one file, and a commit's file may no longer be in the working tree
    /// at all, which the editor reports when it cannot read it.
    fn file_at_point(&self) -> Option<OpenTarget> {
        let target = self.target_at_point()?;
        match &self.comparison {
            Some(comparison) => Some(OpenTarget {
                path: comparison.standing_for(&target.path),
                ..target
            }),
            None => Some(target),
        }
    }

    /// The place the row under the cursor names, read from the repository's
    /// root, or from the filesystem's for a comparison's paths.
    fn target_at_point(&self) -> Option<OpenTarget> {
        let root = match &self.comparison {
            Some(comparison) => comparison.root(),
            None => self.repo.root.clone().unwrap_or_default(),
        };
        // A blame is one row per line of one file, so the row under the
        // cursor names a place in it as surely as a hunk header does, even
        // though the rows themselves stand for nothing foldable.
        if let Some(output) = self.output.as_ref() {
            if let Some(path) = &output.blamed {
                return Some(OpenTarget::at_line(
                    root.join(bytes_text::to_path(path)),
                    output.list.cursor() + 1,
                ));
            }
        }
        match self.item_at_point()? {
            Item::File(file) => Some(OpenTarget::file(root.join(bytes_text::to_path(&file.path)))),
            Item::Hunk(hunk) => {
                let path = root.join(bytes_text::to_path(&hunk.path));
                let key = FileRow {
                    path: hunk.path.clone(),
                    section: hunk.section,
                };
                let line = self
                    .repo.diffs
                    .get(&key)
                    .and_then(|diff| diff.hunks.get(hunk.index))
                    .and_then(|found| found.new_line_at(self.offset_into_item()));
                Some(at_line_or_top(path, line))
            }
            Item::CommitFile(index) => {
                let file = self.commit_file(index)?;
                Some(OpenTarget::file(root.join(bytes_text::to_path(&file.path))))
            }
            Item::CommitHunk(file_index, hunk_index) => {
                let file = self.commit_file(file_index)?;
                let line = file
                    .hunks
                    .get(hunk_index)
                    .and_then(|found| found.new_line_at(self.offset_into_item()));
                Some(at_line_or_top(root.join(bytes_text::to_path(&file.path)), line))
            }
            // Everything else names a commit, a stash, or a heading: things
            // with no one file behind them.
            Item::Commit(_)
            | Item::CommitChanges
            | Item::Heading(_)
            | Item::None
            | Item::RecentHeading
            | Item::Ref(_)
            | Item::Stash(_)
            | Item::StashHeading
            | Item::UnpulledHeading => None,
        }
    }

    /// Which list row sits at screen row `screen_row`, taking line wrapping into
    /// account.
    fn row_at_screen(&self, screen_row: usize) -> Option<usize> {
        let cols = self.cols;
        let start = self.list.view().scroll();
        let mut top = 0;
        for index in start..self.rows.len() {
            let text = row_text(&self.rows[index].spans);
            let height = row_height(&text, cols, true, self.rows[index].wrap_indent);
            if screen_row < top + height {
                return Some(index);
            }
            top += height;
        }
        None
    }

    /// Which line of output sits at screen row `screen_row`, taking the header
    /// and line wrapping into account.
    fn output_row_at_screen(&self, screen_row: usize) -> Option<usize> {
        let output = self.output.as_ref()?;
        if screen_row < HEADER_ROWS {
            return None;
        }
        let content_row = screen_row - HEADER_ROWS;
        let cols = self.cols;
        let start = output.list.view().scroll();
        let mut top = 0;
        for index in start..output.lines.len() {
            let indent = output.rows.get(index).map_or(0, |row| row.wrap_indent);
            let height = row_height(&output.lines[index], cols, true, indent);
            if content_row < top + height {
                return Some(index);
            }
            top += height;
        }
        None
    }
}

// ========================================================================
// GitEditor: what only the window asks of it
// ========================================================================

impl GitEditor {
    /// Whether this editor is currently showing git status.
    pub(crate) fn is_git_status(&self) -> bool {
        !self.staged_only && self.comparison.is_none() && !self.view_tab && self.output.is_none()
    }

    /// Whether this editor is a repository git editor (status, log, blame),
    /// not a companion diff or file comparison.
    pub(crate) fn is_git_editor(&self) -> bool {
        !self.staged_only && self.comparison.is_none()
    }

    /// Return to the status view if this editor is showing an output view.
    pub(crate) fn show_status(&mut self) -> EditorOutcome {
        if self.output.is_some() {
            self.output = None;
            self.view = None;
        }
        self.refresh()
    }

    /// Whether this is a tab of its own reading `request`, so asking for the
    /// same view again goes to it rather than opening a second.
    pub(crate) fn shows_view(&self, request: &JobRequest) -> bool {
        self.view_tab && self.view.as_ref() == Some(request)
    }
}

impl Editor for GitEditor {
    fn on_selected_rows_key(
        &mut self,
        key: &Key,
        rows: RangeInclusive<usize>,
    ) -> Option<EditorOutcome> {
        // A view's rows are no working tree's to stage.
        if self.output.is_some() || self.view_tab {
            return None;
        }
        let act = match key.chord() {
            (Mods::NONE, KeyCode::Char('s')) => RowsAct::Stage,
            (Mods::NONE, KeyCode::Char('u')) => RowsAct::Unstage,
            (Mods::NONE, KeyCode::Char('x')) => RowsAct::Discard,
            _ => return None,
        };
        Some(self.act_on_selected_rows(rows, act))
    }

    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.list.saved_place()))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(place) = state.downcast::<ListPlace<RowKey>>() {
            self.list.restore_place(*place);
        }
    }

    /// What the tab is: which of git's views it shows (the status, a log, a
    /// commit, a blame), and of which project, so two git tabs read apart.
    fn title(&self) -> String {
        if let Some(comparison) = &self.comparison {
            return format!("{COMPARE_TITLE}: {}", comparison.title());
        }
        let Some(root) = self.repo.root.as_deref() else {
            return TITLE_PREFIX.to_string();
        };
        let project = root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let Some(output) = &self.output else {
            return match self.view_tab {
                // A view still being read is not the status.
                true => format!("{TITLE_PREFIX}: {project}"),
                false => format!("{TITLE_PREFIX} {STATUS_TITLE}: {project}"),
            };
        };
        // A commit's own title runs on to its subject, which a tab has no
        // room for: `Commit abc1234`.
        if output.commit.is_some() {
            let commit = output.title.split(SUBJECT_GAP).next().unwrap_or_default();
            return format!("{TITLE_PREFIX} {commit}: {project}");
        }
        match &output.blamed {
            Some(path) => format!("{TITLE_PREFIX} {}: {path}", output.title),
            None => format!("{TITLE_PREFIX} {}: {project}", output.title),
        }
    }

    /// The repository, as the `Repo:` row writes it.
    fn address(&self) -> Option<String> {
        self.repo.root
            .as_deref()
            .map(crate::model::path::abbreviate_home)
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.list.view_mut().set_margin(rows);
        if let Some(output) = self.output.as_mut() {
            output.list.view_mut().set_margin(rows);
        }
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        // The view moves and the cursor stays, as in an editor: the open log,
        // blame, or diff when there is one, else the status rows.
        match self.output.as_mut() {
            Some(output) => output.list.view_mut().wheel(lines),
            None => self.list.view_mut().wheel(lines),
        }
        EditorOutcome::Consumed
    }

    /// A press on a row puts the cursor on it; drags are ignored to let the
    /// window's selection run across cells.
    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        self.release_wheel();
        if self.output.is_some() {
            if let Some(index) = self.output_row_at_screen(at.row) {
                if let Some(output) = self.output.as_mut() {
                    output.list.set_cursor(index);
                    return EditorOutcome::Consumed;
                }
            }
            return EditorOutcome::Ignored;
        }
        if let Some(index) = self.row_at_screen(at.row) {
            self.list.set_cursor(index);
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Ignored
    }

    /// A double-click opens the file, hunk, or commit under the pointer.
    fn on_double_click(&mut self, at: Point) -> EditorOutcome {
        if self.on_mouse(at) == EditorOutcome::Consumed {
            self.open_or_read()
        } else {
            EditorOutcome::Ignored
        }
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        if cols > 0 && self.cols != cols {
            self.cols = cols;
            if !self.rows.is_empty() {
                self.rebuild();
            }
            if let Some(output) = &mut self.output {
                if let Some(commits) = &output.log_commits {
                    let painted = rows::log_rows(
                        commits,
                        self.repo.root.as_deref(),
                        now_unix(),
                        output.more,
                        Some(cols),
                    );
                    output.lines = painted.iter().map(|row| row_text(&row.spans)).collect();
                    output.rows = painted;
                }
            }
        }
        // Remember the viewport for the paging motions, which key handling
        // needs between paints.
        let viewport = rows.saturating_sub(HEADER_ROWS).max(1);
        self.list.view_mut().set_rows(viewport);
        if let Some(output) = self.output.as_mut() {
            output.list.view_mut().set_rows(viewport);
        }
        if let Some(frame) = self.paint_output(rows, cols, wrap) {
            return frame;
        }
        if self.rows.is_empty() {
            // A loaded view with no rows means git answered and had nothing
            // to report, which only happens outside a repository: a clean tree
            // still has a header row, unless the read itself failed. What git
            // said went to the status bar.
            let note = match (self.repo.loaded, &self.repo.root) {
                (false, _) => LOADING,
                (true, None) => NOT_A_REPO,
                (true, Some(_)) => UNREADABLE,
            };
            return Frame::new(vec![vec![Span::new(Style::Dim, note)]]);
        }
        let indents: Vec<usize> = self.rows.iter().map(|row| row.wrap_indent).collect();
        let texts: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
        let visible = rows.saturating_sub(HEADER_ROWS);
        let height = |index: usize| row_height(&texts[index], cols, wrap, indents[index]);
        let placed = self.list.place(visible, height);
        let window = placed.window;
        let mut painted: Vec<Vec<Span>> = Vec::new();
        let mut wrap_indents: Vec<usize> = Vec::new();
        let mut icons: Vec<Icon> = Vec::new();
        for row in self
            .rows
            .iter()
            .skip(window.start)
            .take(window.count.max(1))
        {
            if let Some(icon) = &row.icon {
                icons.push(Icon {
                    row: painted.len(),
                    ..icon.clone()
                });
            }
            painted.push(row.spans.clone());
            wrap_indents.push(row.wrap_indent);
        }
        let content = Frame::new(painted)
            .with_icons(icons)
            .with_wrap_indents(wrap_indents);
        placed.apply(content, 0)
    }

    fn hint(&self) -> Option<EditorHint> {
        let popup = self.menus.popup?;
        let mut items: Vec<(String, String)> = match (popup, &self.menus.push) {
            (Popup::Push, Some(menu)) => menu.choices(
                self.repo.status.branch.as_deref(),
                self.repo.status.upstream.as_deref(),
            ),
            (Popup::PushOptions, Some(menu)) => menu.option_choices(),
            _ => popup
                .choices()
                .iter()
                .map(|(key, what)| (key.to_string(), what.to_string()))
                .collect(),
        };
        // The switches after the choices, each saying whether it is on;
        // after `-`, only the switches, since only they answer.
        let switches = popup.switches().iter().map(|switch| {
            let on = match self.menus.switches_on.contains(&(popup, switch.key)) {
                true => "  [on]",
                false => "",
            };
            (format!("-{}", switch.key), format!("{} ({}){on}", switch.label, switch.flag))
        });
        match self.menus.switch_next {
            true => items = switches.collect(),
            false => items.extend(switches),
        }
        Some(EditorHint {
            items,
            menu: true,
            title: popup.title().to_string(),
        })
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        let output = self
            .output
            .as_mut()
            .and_then(|output| output.list.take_search());
        output.or_else(|| self.list.take_search())
    }

    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        if name == SHOW_SCOPES || name == HIDE_SCOPES {
            self.inspect_scopes = name == SHOW_SCOPES && !self.inspect_scopes;
            return Some(EditorOutcome::Consumed);
        }
        match GitCommand::from_name(name)? {
            GitCommand::OpenRemote => Some(self.open_in_remote()),
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if self.inspect_scopes && key.chord() == (Mods::NONE, KeyCode::Escape) {
            self.inspect_scopes = false;
            return EditorOutcome::Consumed;
        }
        let outcome = self.dispatch_key(key);
        self.note_view(outcome)
    }

    fn keys(&self) -> Vec<KeyHelp> {
        self.key_help()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        let outcome = self.answer_prompt(reply);
        self.note_view(outcome)
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_SEARCH && !query.is_empty() {
            self.search_for(query)
        } else {
            EditorOutcome::Ignored
        }
    }

    /// A buried view lets go of its watch, which stops the platform's
    /// notifications and the reads they would wake the window for; shown
    /// again, it watches anew, and the resume reads what changed meanwhile.
    fn set_shown(&mut self, shown: bool) {
        if !shown {
            self.watch.tree_watch = None;
            self.watch.tree_watch_starting = None;
            return;
        }
        let unwatched = self.watch.tree_watch.is_none() && self.watch.tree_watch_starting.is_none();
        let is_status = !self.staged_only && !self.view_tab;
        if let (true, true, Some(root)) = (unwatched, is_status, self.repo.root.as_ref()) {
            self.watch.tree_watch_starting = Some(watch::TreeWatch::start_in_background(
                root,
                self.repo.git_dir.as_deref(),
            ));
        }
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // Work asked for before the editor was covered was cancelled with it.
        self.watch.watching = None;
        // The file that was just being edited is very likely one of the ones
        // this view reports on.
        self.refresh()
    }

    fn next_tick(&self) -> Option<Instant> {
        // A working-tree read for the diff tool wakes the window as it
        // lands, and is taken then.
        if self.head_compare.as_ref().is_some_and(|compare| compare.read.is_done()) {
            return Some(Instant::now());
        }
        // An append to `.gitignore` under way is looked for soon.
        if self.ignoring.is_some() {
            return Some(Instant::now() + background::POLL);
        }
        // Nothing to watch until the repository is known and read once, and a
        // view the user is in the middle of (output, a popup, a question)
        // is not redrawn under them.
        if !self.repo.loaded || self.output.is_some() || self.menus.popup.is_some() {
            return None;
        }
        // A read already out sees what has happened by now; whatever happens
        // after it is looked for once it has answered.
        if self.menus.pending_pick.is_some() || self.staged_only || self.watch.watching.is_some() {
            return None;
        }
        match (&self.watch.tree_watch, &self.watch.tree_watch_starting) {
            (Some(tree), _) => tree.next_look(Instant::now()),
            // Taken over once its thread has answered, which wakes the window.
            (None, Some(starting)) => starting.is_done().then(Instant::now),
            // Not watched: read again only when asked.
            (None, None) => None,
        }
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if let Some(compare) = self.take_head_compare() {
            return compare;
        }
        if let Some(written) = self.ignoring.as_ref().and_then(Background::poll) {
            self.ignoring = None;
            return self.take_ignored(written);
        }
        self.adopt_started_watch();
        let Some(root) = self.repo.root.clone() else {
            return EditorOutcome::Consumed;
        };
        // A read already out will see what has happened by now; whatever
        // changes after it is looked for once it has answered.
        let due = match (self.watch.tree_watch.as_mut(), self.watch.watching) {
            (Some(tree), None) => tree.changed(now),
            (Some(_), Some(_)) | (None, _) => None,
        };
        let Some(read) = due else {
            return EditorOutcome::Consumed;
        };
        self.watch.watching = Some(read);
        EditorOutcome::Job(git_exec::status(&root))
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match reply {
            // A read can lead on to a change: the push menu's read of the
            // config, a reword's read of the commit.
            JobReply::Command(output) => {
                let outcome = self.on_command(output);
                self.announce(outcome)
            }
            JobReply::Files(files) => {
                // What git is part-way through, read straight from the files
                // it writes it in: nothing about it reaches a porcelain
                // status, so there is nothing to run for it.
                self.repo.progress = progress::parse(&files);
                self.rebuild();
                EditorOutcome::Consumed
            }
            // The view asks for no directory walks, no searches, and no
            // listings of the machine.
            JobReply::DirSize { .. }
            | JobReply::FileOps(_)
            | JobReply::Processes(_)
            | JobReply::Search(_)
            | JobReply::System(_) => EditorOutcome::Consumed,
        }
    }

    /// The repository, for the status view; the staged diff that sits beside
    /// a commit message goes when the message does, so it is not reopened.
    fn snapshot(&self) -> Option<OpenTarget> {
        if self.staged_only || self.comparison.is_some() || self.view_tab {
            return None;
        }
        Some(OpenTarget::file(
            self.repo.root.clone().unwrap_or_else(|| self.start.clone()),
        ))
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The file under the cursor decides it, so an editor opened from a diff
        // starts beside that file rather than at the top of the repository.
        // Rows standing for a commit or a section name no file and leave the
        // repository root as the answer.
        self.file_at_point()
            .and_then(|target| target.path.parent().map(PathBuf::from))
            .or_else(|| self.repo.root.clone())
    }

    fn root_dir(&self) -> Option<PathBuf> {
        self.repo.root.clone().or_else(|| Some(self.start.clone()))
    }

    fn file_reference(&self) -> Option<String> {
        if let Some(target) = self.file_at_point() {
            let base = crate::model::path::abbreviate_home(&target.path);
            if let Some(line) = target.line {
                return Some(format!("{base}:{line}"));
            }
            return Some(base);
        }
        self.repo.root
            .as_ref()
            .map(|r| crate::model::path::abbreviate_home(r))
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        // What a row offers is what that kind of row can do, which is why the
        // menu is built per item rather than as one list with things greyed
        // out: a commit cannot be staged and a heading has no path to copy.
        // Each list is in groups: reading the row, changing the index, and
        // copying what names it.
        fn group<const N: usize>(items: [Option<EditorMenuItem>; N]) -> Vec<EditorMenuItem> {
            items.into_iter().flatten().collect()
        }
        match self.item_at_point() {
            Some(Item::File(_)) | Some(Item::CommitFile(_)) => menu_groups([
                group([
                    menu_item(Act::Open, LABEL_OPEN),
                    menu_item(Act::OpenExternal, LABEL_OPEN_IN_EDITOR),
                    menu_item(Act::ToggleFold, LABEL_DIFF),
                    menu_item(Act::Blame, LABEL_BLAME),
                ]),
                group([
                    menu_item(Act::Stage(false), LABEL_STAGE),
                    menu_item(Act::Unstage(false), LABEL_UNSTAGE),
                    menu_item(Act::Discard, LABEL_DISCARD),
                ]),
                group([menu_choice(Popup::Yank, 's', LABEL_COPY_PATH)]),
            ]),
            Some(Item::Hunk(_)) | Some(Item::CommitHunk(_, _)) => menu_groups([
                group([menu_item(Act::Open, LABEL_OPEN_AT_LINE)]),
                group([
                    menu_item(Act::Stage(false), LABEL_STAGE_HUNK),
                    menu_item(Act::Unstage(false), LABEL_UNSTAGE_HUNK),
                    menu_item(Act::Discard, LABEL_DISCARD_HUNK),
                ]),
                group([
                    menu_item(Act::Apply(ApplyTarget::Index), LABEL_APPLY_HUNK),
                    menu_item(Act::Apply(ApplyTarget::IndexReverse), LABEL_REVERSE_HUNK),
                ]),
                group([menu_choice(Popup::Yank, 's', LABEL_COPY_PATH)]),
            ]),
            Some(Item::Commit(_)) => menu_groups([
                group([menu_item(Act::Open, LABEL_READ_COMMIT)]),
                group([menu_choice(Popup::Yank, 's', LABEL_COPY_HASH)]),
            ]),
            Some(Item::Stash(_)) => group([
                menu_item(Act::Open, LABEL_READ_STASH),
                menu_item(Act::Menu(Popup::Stash), LABEL_STASH_MENU),
            ]),
            Some(Item::Heading(_))
            | Some(Item::CommitChanges)
            | Some(Item::StashHeading)
            | Some(Item::UnpulledHeading)
            | Some(Item::RecentHeading) => menu_groups([
                group([menu_item(Act::ToggleFold, LABEL_FOLD)]),
                group([
                    menu_item(Act::Stage(true), LABEL_STAGE_ALL),
                    menu_item(Act::Unstage(true), LABEL_UNSTAGE_ALL),
                ]),
                group([menu_choice(Popup::Jump, 'r', LABEL_RELOAD)]),
            ]),
            // The list of branches and tags answers keys of its own view,
            // which the status's menu entries do not name.
            Some(Item::Ref(_)) => Vec::new(),
            Some(Item::None) | None => menu_groups([
                group([menu_item(Act::CommitInEditor, LABEL_COMMIT)]),
                group([
                    menu_item(Act::Push, LABEL_PUSH),
                    menu_item(Act::Pull, LABEL_PULL),
                    menu_item(Act::Fetch, LABEL_FETCH),
                ]),
                group([menu_choice(Popup::Jump, 'r', LABEL_RELOAD)]),
            ]),
        }
    }

    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        let label = |index: usize, text: &str| {
            JumpTarget::new(index, format!("{:>5}  {}", index + 1, text))
        };
        Some(match &self.output {
            Some(output) => output
                .lines
                .iter()
                .enumerate()
                .map(|(index, line)| label(index, line))
                .collect(),
            // A file row carries the icon the view draws beside it.
            None => self
                .rows
                .iter()
                .enumerate()
                .map(|(index, row)| {
                    let target = label(index, &row_text(&row.spans));
                    match &row.icon {
                        Some(icon) => target.with_icon(icon.kind.clone()),
                        None => target,
                    }
                })
                .collect(),
        })
    }

    fn jump_cursor(&self) -> Option<usize> {
        Some(match &self.output {
            Some(output) => output.list.cursor(),
            None => self.list.cursor(),
        })
    }

    fn on_jump(&mut self, step: JumpStep) {
        match self.output.as_mut() {
            Some(output) => output.list.on_jump(step),
            None => self.list.on_jump(step),
        }
    }

    fn token_inspect(&mut self) -> Option<TokenInspectData> {
        if !self.inspect_scopes {
            return None;
        }
        let cursor = match self.output.as_ref() {
            Some(output) => output.list.cursor(),
            None => self.list.cursor(),
        };
        let item = self.item_at_point();
        let (ch, scopes, color_rule, style) = match item.as_ref() {
            Some(Item::Hunk(hunk)) => {
                let is_add = hunk.section == Section::Staged || hunk.section == Section::Unstaged;
                (
                    if is_add { '+' } else { '-' },
                    vec!["source.diff".to_string(), "meta.diff.range.unified".to_string()],
                    Some("git.hunk".to_string()),
                    Some("bold".to_string()),
                )
            }
            Some(Item::File(file)) => (
                file.path.chars().next().unwrap_or('f'),
                vec!["source.git".to_string(), "entity.name.file".to_string()],
                Some("git.file".to_string()),
                None,
            ),
            Some(Item::Heading(_)) => (
                '#',
                vec!["source.git".to_string(), "meta.header.git".to_string()],
                Some("git.heading".to_string()),
                Some("bold".to_string()),
            ),
            Some(Item::Commit(hash)) => (
                hash.chars().next().unwrap_or('*'),
                vec!["source.git".to_string(), "constant.numeric.git".to_string()],
                Some("git.commit".to_string()),
                Some("bold".to_string()),
            ),
            _ => (
                ' ',
                vec!["source.git".to_string()],
                Some("normal".to_string()),
                None,
            ),
        };
        let character = TokenInspectData::format_char_with_tail(ch, None);
        Some(TokenInspectData {
            character,
            color_rule,
            foreground_hex: None,
            language: "Git".to_string(),
            position: TokenInspectData::format_position(cursor, 0),
            scopes,
            style,
        })
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// `path` at `line` where one is known, and at the top where none is: a hunk
/// whose header git wrote in some unreadable shape still opens its file.
fn at_line_or_top(path: PathBuf, line: Option<usize>) -> OpenTarget {
    match line {
        Some(line) => OpenTarget::at_line(path, line),
        None => OpenTarget::file(path),
    }
}

/// The current time, in seconds since the Unix epoch, for the commit rows to
/// measure a commit's age against.
///
/// A clock read before the system clock was set, which cannot be represented as
/// "seconds since the epoch", reads as `0`, and every commit then shows no
/// meaningful age rather than the view failing to draw.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// The key each of `rows` keeps the cursor on it by across a re-read.
fn row_keys(rows: &[ViewRow]) -> Vec<RowKey> {
    let mut seen: HashMap<&Item, usize> = HashMap::new();
    rows.iter()
        .map(|row| {
            let nth = seen.entry(&row.item).or_insert(0);
            let key = RowKey {
                item: row.item.clone(),
                nth: *nth,
            };
            *nth += 1;
            key
        })
        .collect()
}

/// The `.gitignore` line that ignores `path`, a repository-relative path, or
/// with `by_extension` every file sharing its extension.
///
/// A path is anchored to the root with a leading `/`, since a bare `out.log`
/// would ignore an `out.log` in every directory, and the characters a
/// pattern gives a meaning to (`*`, `?`, `[`, `\`, trailing spaces) are
/// escaped so the line names that one file. The extension is the file name's
/// own, never a dot in a directory above it.
fn ignore_pattern(path: &str, by_extension: bool) -> String {
    let extension = Path::new(path)
        .file_name()
        .map(Path::new)
        .and_then(Path::extension)
        .and_then(|extension| extension.to_str());
    match (by_extension, extension) {
        (true, Some(extension)) => format!("*.{}", escape_ignore(extension)),
        (true, None) | (false, _) => format!("/{}", escape_ignore(path)),
    }
}

/// `text` with every character a `.gitignore` pattern reads specially
/// escaped, trailing spaces included, which git would otherwise drop.
fn escape_ignore(text: &str) -> String {
    let kept = text.trim_end_matches(' ');
    let mut escaped = String::with_capacity(text.len());
    for c in kept.chars() {
        if IGNORE_SPECIAL.contains(c) {
            escaped.push('\\');
        }
        escaped.push(c);
    }
    for _ in kept.len()..text.len() {
        escaped.push_str("\\ ");
    }
    escaped
}

/// Append `line` to a file, creating it and starting a new line first when it
/// does not already end with one.
fn append_line(path: &std::path::Path, line: &str) -> std::io::Result<()> {
    use std::io::Write;

    let existing = std::fs::read_to_string(path).unwrap_or_default();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    if !existing.is_empty() && !existing.ends_with('\n') {
        file.write_all(b"\n")?;
    }
    writeln!(file, "{line}")
}

/// The diff command for one row: an untracked file has no indexed counterpart,
/// so it is diffed against nothing at all.
fn diff_request(root: &std::path::Path, file: &FileRow) -> JobRequest {
    match file.section {
        Section::Untracked => git_exec::diff_untracked(root, &Pathspec::new(file.path.clone())),
        Section::Staged => git_exec::diff_file(root, &Pathspec::new(file.path.clone()), true),
        Section::Unmerged | Section::Unstaged => {
            git_exec::diff_file(root, &Pathspec::new(file.path.clone()), false)
        }
    }
}

// ========================================================================
// RepoState
// ========================================================================

impl RepoState {
    fn paths_in(&self, section: Section) -> Vec<String> {
        self.status
            .files
            .iter()
            .filter(|file| file.section == section)
            .map(|file| file.path.clone())
            .collect()
    }

    /// The remote the branch's upstream is on, `origin` of `origin/main`.
    fn upstream_remote(&self) -> Option<&str> {
        self.upstream().map(|(remote, _)| remote)
    }

    /// The branch's upstream as its remote and its branch there.
    fn upstream(&self) -> Option<(&str, &str)> {
        self.split_remote(self.status.upstream.as_deref()?)
    }

    /// `name` (`origin/feature`) as its remote and its branch there (see
    /// [`push::split_remote`]).
    fn split_remote<'a>(&self, name: &'a str) -> Option<(&'a str, &'a str)> {
        push::split_remote(&self.remotes, name)
    }
}

// ========================================================================
// MenuState
// ========================================================================

impl MenuState {
    /// The flags of `popup`'s switches turned on, in the order it lists them.
    fn flags_for(&self, popup: Popup) -> Vec<&'static str> {
        popup
            .switches()
            .iter()
            .filter(|switch| self.switches_on.contains(&(popup, switch.key)))
            .map(|switch| switch.flag)
            .collect()
    }

    /// Turn `popup`'s switch on `key` on or off, where it has one.
    fn toggle_switch(&mut self, popup: Popup, key: char) {
        if !popup.switches().iter().any(|switch| switch.key == key) {
            return;
        }
        match self.switches_on.iter().position(|on| *on == (popup, key)) {
            Some(at) => {
                self.switches_on.remove(at);
            }
            None => self.switches_on.push((popup, key)),
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

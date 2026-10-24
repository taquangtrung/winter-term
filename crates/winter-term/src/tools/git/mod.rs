//! Git: the working tree's state in a pane, and the keys that change it.
//!
//! - [`diff`]: reading a unified diff, and writing one hunk back out.
//! - [`exec`]: the git command lines the view runs.
//! - [`parse`]: reading git's own output.
//! - [`popup`]: the transient menus a key opens.
//! - [`rows`]: painting the view, and what each row stands for.
//! - [`watch`]: noticing that the repository changed under the view.
//! - [`words`]: the word-level diff that decorates a hunk's changed lines.
//!
//! The view itself is split by what it answers: `keys` (a key press),
//! `prompt` (an answered question), `reply` (a finished command), and
//! `output` (the log, blame, or commit that fills the view in place of the
//! status).

pub mod commit;
pub mod diff;
pub mod exec;
mod keys;
mod output;
pub mod parse;
pub mod popup;
pub mod progress;
mod prompt;
mod push;
mod reply;
pub mod rows;
pub mod watch;
pub mod words;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::model::input::table::KeyHelp;
use crate::model::input::{CursorMove, Key, KeyCode};
use crate::model::paint::{row_height, row_text, Frame, Icon, Span, Style};
use crate::model::tool::{
    JobReply, JobRequest, JumpTarget, Notice, OpenTarget, PickQuestion, PromptMode, PromptReply,
    PromptRequest, SearchReport, SpawnRequest, Tool, ToolCommand, ToolHint, ToolMenuItem,
    menu_groups,
    ToolOutcome,
};
use crate::model::vim::nav::VimNav;
use crate::tools::background::{self, Background};
use crate::tools::list::ListView;

use commit::CommitFile;
use diff::{FileDiff, Hunk};
use exec::{ApplyTarget, CommitMode, Pathspec, ResetMode, Rev};
use keys::{menu_item, Act};
use output::Output;
use parse::{Commit, Section, Stash, Status};
use popup::Popup;
use progress::Progress;
use rows::{Block, FileRow, HunkRow, Item, ViewRow};

// ========================================================================
// Constants
// ========================================================================

/// The commands the `git` block of `keybindings.kdl` can bind.
pub const COMMANDS: &[ToolCommand] = &[ToolCommand {
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
const ASK_MERGE: &str = "merge";
const ASK_REBASE: &str = "rebase";
const ASK_REMOTE_ADD: &str = "remote-add";
const ASK_REMOTE_PRUNE: &str = "remote-prune";
const ASK_REMOTE_REMOVE: &str = "remote-remove";
/// What every git tab's title starts with.
const TITLE_PREFIX: &str = "Git";

/// What a tab showing the working tree's state is called.
const STATUS_TITLE: &str = "Status";

/// What parts a commit view's title from the subject after it.
const SUBJECT_GAP: &str = "  ";

const ASK_PUSH_ELSEWHERE: &str = "push-elsewhere";
const ASK_PUSH_OTHER: &str = "push-other";
const ASK_PUSH_OTHER_TARGET: &str = "push-other-target";
const ASK_PUSH_REMOTE: &str = "push-remote";
const ASK_PUSH_TAG: &str = "push-tag";
const ASK_PUSH_UPSTREAM: &str = "push-upstream";
const ASK_RESET_HARD: &str = "reset-hard";
const ASK_RESET_MIXED: &str = "reset-mixed";
const ASK_RESET_SOFT: &str = "reset-soft";
const ASK_REVERT: &str = "revert";
const ASK_SEARCH: &str = "search";
const ASK_STASH: &str = "stash";
const ASK_TAG_CREATE: &str = "tag-create";
const ASK_TAG_DELETE: &str = "tag-delete";
const ASK_WORKTREE_ADD: &str = "worktree-add";
const ASK_WORKTREE_REMOVE: &str = "worktree-remove";

/// Commits the log view asks for at a time, and adds on each request for more.
const LOG_PAGE: usize = 50;

/// The remote a browser is pointed at, whose refs are pruned by default, and
/// that a branch tracking nothing is pushed to.
const DEFAULT_REMOTE: &str = "origin";

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
const LABEL_FETCH: &str = "Fetch All";
const LABEL_FOLD: &str = "Fold or Unfold";
const LABEL_OPEN: &str = "Open";
const LABEL_OPEN_AT_LINE: &str = "Open at This Line";
const LABEL_OPEN_IN_EDITOR: &str = "Open in $EDITOR";
const LABEL_PULL: &str = "Pull, Rebasing";
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
pub struct GitTool {
    /// Sections the user has shut. `None` stands for the recent-commits
    /// section, which has no `Section` of its own.
    collapsed: HashSet<Block>,
    commits: Vec<Commit>,
    /// Where the repository keeps its own files, which is where the state of
    /// whatever git is part-way through is written.
    git_dir: Option<PathBuf>,
    /// What git is part-way through, when it is part-way through anything.
    progress: Option<Progress>,
    /// The stashes, newest first.
    stashes: Vec<Stash>,
    /// What the upstream has and this branch does not, newest first.
    unpulled: Vec<Commit>,
    /// Diffs already read, keyed by the row that asked for one. A file's
    /// working-tree and index diffs are different things, so the section is
    /// part of the key.
    diffs: HashMap<FileRow, FileDiff>,
    /// Files whose diff is showing.
    expanded: HashSet<FileRow>,
    /// The cursor over the status rows, kept on its row across re-reads.
    list: ListView<RowKey>,
    /// What the last command reported, waiting for the status bar to take it.
    notice: Option<Notice>,
    /// The conflicted file a resolve is settling, staged once git has
    /// written the side that was taken.
    resolving: Option<String>,
    /// The diff line `C-SPC` set the mark on, which `M-s` and `M-u`
    /// stage or unstage from, to the line under the cursor.
    line_mark: Option<usize>,
    /// Where the repository is rooted, once git has said.
    root: Option<PathBuf>,
    rows: Vec<ViewRow>,
    /// The discard a confirmation is asking about, taken when it was asked
    /// so the answer acts on exactly what the question named, whatever a
    /// re-read has done to the rows since.
    pending_discard: Option<Discard>,
    /// An append to `.gitignore` still being written.
    ignoring: Option<Background<Result<String, String>>>,
    /// The file whose diff was asked for and has not come back yet.
    pending_diff: Option<FileRow>,
    /// The menu waiting for its second key.
    popup: Option<Popup>,
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
    /// Whether the history goes on past the recent commits shown, so `+`
    /// has more to load.
    more_commits: bool,
    /// The path a blame was asked for, held until the answer comes back,
    /// since the reply says only that it is a blame and not what of.
    pending_blame: Option<String>,
    /// Where the tool was opened, which is where the root is looked up from.
    start: PathBuf,
    status: Status,
    /// Set once a status has come back, so an empty view can say which it is:
    /// a clean tree, or one still being read.
    loaded: bool,
    /// The shared Vim motion state: the layer the view's unclaimed keys fall
    /// through to. Its own `g` leader claims the prefix before this sees it.
    nav: VimNav,
    /// The question a list of names is being gathered for.
    pending_pick: Option<PickQuestion>,
    /// The push menu while it is open or a push it started is being
    /// answered: where the branch pushes, and the switches turned on.
    push: Option<push::PushMenu>,
    /// The repository's file notifications, which say when to read it again;
    /// nothing reads it on a timer.
    tree_watch: Option<watch::TreeWatch>,
    /// The tree watch being set up on a thread of its own (see
    /// [`watch::TreeWatch::start_in_background`]).
    tree_watch_starting: Option<watch::StartingWatch>,
    /// What the status read in flight was the watch's for, if it was its at
    /// all, so an answer that changed nothing can be told from one that must
    /// redraw, and a change to the files alone reads no more than the status.
    watching: Option<watch::Read>,
    /// The last status git answered with, as it came back.
    last_status: Option<String>,
    /// Whether this tool only shows what is staged, as a diff, rather than
    /// the working tree's status: the view beside a commit message.
    staged_only: bool,
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
    /// Restore tracked files from the index.
    Tracked(Vec<Pathspec>),
    /// Delete untracked files.
    Untracked(Vec<Pathspec>),
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
// GitTool
// ========================================================================

impl GitTool {
    /// A view of the repository containing `start`, which is asked for first.
    pub fn new(start: PathBuf) -> Self {
        Self {
            collapsed: HashSet::new(),
            git_dir: None,
            pending_blame: None,
            progress: None,
            stashes: Vec::new(),
            unpulled: Vec::new(),
            commits: Vec::new(),
            diffs: HashMap::new(),
            expanded: HashSet::new(),
            list: ListView::new(),
            loaded: false,
            notice: None,
            line_mark: None,
            resolving: None,
            log_count: LOG_PAGE,
            recent_count: exec::RECENT_COMMITS,
            more_commits: false,
            output: None,
            view: None,
            keep_place: false,
            pending_diff: None,
            popup: None,
            root: None,
            pending_discard: None,
            ignoring: None,
            rows: Vec::new(),
            start,
            status: Status::default(),
            nav: VimNav::new(),
            pending_pick: None,
            push: None,
            staged_only: false,
            tree_watch: None,
            tree_watch_starting: None,
            watching: None,
            last_status: None,
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

    /// Remember the request a view filling the output is read with, so a
    /// command that changes the repository under it can read it again. A
    /// view asked for afresh starts at its top.
    fn note_view(&mut self, outcome: ToolOutcome) -> ToolOutcome {
        if let ToolOutcome::Job(JobRequest::Command(request)) = &outcome {
            if exec::VIEW_TAGS.contains(&request.tag) {
                self.view = Some(JobRequest::Command(request.clone()));
                self.keep_place = false;
            }
        }
        self.announce(outcome)
    }

    /// Name a command that changes the repository in the status bar as it
    /// starts (`git push origin main`), so what a key ran is never a guess.
    /// Left out of the notification history, which hears how it ended.
    fn announce(&mut self, outcome: ToolOutcome) -> ToolOutcome {
        if let ToolOutcome::Job(JobRequest::Command(request)) = &outcome {
            if exec::CHANGE_TAGS.contains(&request.tag) {
                self.notice = Some(Notice::info(exec::summary(request)).unlogged());
            }
        }
        outcome
    }

    /// What to read after a command changed the repository: the output
    /// showing, read again where it stays up, else the status. Leaving the
    /// output reads the status anyway.
    fn reread(&mut self) -> ToolOutcome {
        match self.output.is_some().then(|| self.view.clone()).flatten() {
            Some(view) => {
                self.keep_place = true;
                ToolOutcome::Job(view)
            }
            None => self.refresh(),
        }
    }

    /// The first request the view makes: where the repository is.
    pub fn initial_request(&self) -> JobRequest {
        exec::repo_root(&self.start)
    }

    /// Re-read the working tree.
    fn refresh(&self) -> ToolOutcome {
        match &self.root {
            Some(root) => ToolOutcome::Job(exec::status(root)),
            None => ToolOutcome::Job(self.initial_request()),
        }
    }

    fn rebuild(&mut self) {
        let collapsed = self.collapsed.clone();
        let expanded = self.expanded.clone();
        let diffs = self.diffs.clone();
        self.rows = rows::build(
            rows::StatusContent {
                status: &self.status,
                commits: &self.commits,
                more_commits: self.more_commits,
                unpulled: &self.unpulled,
                stashes: &self.stashes,
                progress: self.progress.as_ref(),
                root: self.root.as_deref(),
                now: now_unix(),
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
            Some(Item::Heading(section)) => self.paths_in(*section),
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
            | None => Vec::new(),
        }
    }

    fn paths_in(&self, section: Section) -> Vec<String> {
        self.status
            .files
            .iter()
            .filter(|file| file.section == section)
            .map(|file| file.path.clone())
            .collect()
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
    fn search_for(&mut self, query: &str) -> ToolOutcome {
        self.search_with(query, true)
    }

    /// Move to the next row matching the last search, in whichever view is up:
    /// one query serves the status and the output it opens.
    fn search_step(&mut self, forward: bool) -> ToolOutcome {
        let query = self.list.search().to_string();
        self.search_with(&query, forward)
    }

    /// Step to the next or previous row holding `query`, remembering it.
    fn search_with(&mut self, query: &str, forward: bool) -> ToolOutcome {
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
        ToolOutcome::Consumed
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

    /// Interpret one of the shared Vim motions over the view's rows. The
    /// entities are the words (the blank separators between sections are what
    /// `w`/`b` step over), the section headings are the paragraphs, and a
    /// view has no columns, so the line motions mean its ends.
    fn apply_motion(&mut self, motion: CursorMove) {
        if self.list.apply_motion(motion) {
            return;
        }
        match motion {
            CursorMove::WordForward | CursorMove::WordForwardBig => self.move_to_entity(true),
            CursorMove::WordBack | CursorMove::WordBackBig => self.move_to_entity(false),
            CursorMove::WordEnd | CursorMove::WordEndBig => self.move_to_entity(true),
            CursorMove::ParagraphForward => self.move_to_boundary(true),
            CursorMove::ParagraphBack => self.move_to_boundary(false),
            // The column motions and the rest have no meaning over rows; the
            // keys that reach them here are the ones the view has not claimed
            // for something of its own.
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
    fn toggle_fold(&mut self) -> ToolOutcome {
        let item = self.selected().map(|row| row.item.clone());
        if let Some(block) = item.as_ref().and_then(rows::fold_block) {
            self.toggle_block(block);
            return ToolOutcome::Consumed;
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
                ToolOutcome::Consumed
            }
            // The headings resolved above, and a commit view folds through
            // its own handler over its own fold sets.
            _ => ToolOutcome::Consumed,
        }
    }

    fn toggle_block(&mut self, block: Block) {
        if !self.collapsed.remove(&block) {
            self.collapsed.insert(block);
        }
        self.rebuild();
    }

    /// Show or hide one file's diff, reading it the first time it is asked for.
    fn toggle_file(&mut self, file: FileRow) -> ToolOutcome {
        if self.expanded.remove(&file) {
            self.rebuild();
            return ToolOutcome::Consumed;
        }
        self.expanded.insert(file.clone());
        if self.diffs.contains_key(&file) {
            self.rebuild();
            return ToolOutcome::Consumed;
        }
        self.pending_diff = Some(file.clone());
        match &self.root {
            Some(root) => ToolOutcome::Job(diff_request(root, &file)),
            None => ToolOutcome::Consumed,
        }
    }

    /// The hunk row `row` is in, and which line of that hunk's body it is;
    /// `None` for the hunk's own header line, or a row that is no hunk's.
    fn hunk_line_at(&self, row: usize) -> Option<(HunkRow, usize)> {
        let Some(Item::Hunk(hunk)) = self.rows.get(row).map(|view| view.item.clone()) else {
            return None;
        };
        let first = (0..=row)
            .rev()
            .take_while(|at| self.rows[*at].item == Item::Hunk(hunk.clone()))
            .last()?;
        let body = (row - first).checked_sub(1)?;
        Some((hunk, body))
    }

    /// Set the mark on the diff line under the cursor, or take it off again.
    fn toggle_line_mark(&mut self) -> ToolOutcome {
        let at = self.list.cursor();
        if self.line_mark == Some(at) || self.hunk_line_at(at).is_none() {
            self.line_mark = None;
            self.notice = Some(Notice::info("mark cleared"));
            return ToolOutcome::Consumed;
        }
        self.line_mark = Some(at);
        self.notice = Some(Notice::info("mark set; M-s or M-u takes the lines to here"));
        ToolOutcome::Consumed
    }

    /// Stage (`M-s`) or unstage (`M-u`) only some lines of a hunk: from
    /// the mark to the cursor where both are in one hunk, or the line under the
    /// cursor alone.
    fn apply_lines(&mut self, unstage: bool) -> ToolOutcome {
        let Some((hunk, here)) = self.hunk_line_at(self.list.cursor()) else {
            self.notice = Some(Notice::error("on a line of a diff, that takes one line"));
            return ToolOutcome::Consumed;
        };
        let wanted = match unstage {
            true => Section::Staged,
            false => Section::Unstaged,
        };
        if hunk.section != wanted {
            let verb = if unstage { "unstage" } else { "stage" };
            self.notice = Some(Notice::error(format!("nothing to {verb} in this section")));
            return ToolOutcome::Consumed;
        }
        let from = match self
            .line_mark
            .take()
            .and_then(|mark| self.hunk_line_at(mark))
        {
            Some((marked, at)) if marked == hunk => at,
            _ => here,
        };
        let file = FileRow {
            path: hunk.path.clone(),
            section: hunk.section,
        };
        let (Some(root), Some(diff)) = (self.root.as_ref(), self.diffs.get(&file)) else {
            return ToolOutcome::Consumed;
        };
        let Some(patch) =
            diff.patch_for_lines(hunk.index, from.min(here)..=from.max(here), unstage)
        else {
            self.notice = Some(Notice::error("no change on those lines"));
            return ToolOutcome::Consumed;
        };
        let target = match unstage {
            true => exec::ApplyTarget::IndexReverse,
            false => exec::ApplyTarget::Index,
        };
        ToolOutcome::Job(exec::apply_patch(root, patch, target))
    }

    /// Stage, unstage, or discard the hunk under the cursor by applying its own
    /// patch, which leaves the rest of the file untouched.
    fn apply_hunk(&self, target: exec::ApplyTarget) -> Option<ToolOutcome> {
        let Some(Item::Hunk(hunk)) = self.selected().map(|row| row.item.clone()) else {
            return None;
        };
        let root = self.root.as_ref()?;
        let file = FileRow {
            path: hunk.path.clone(),
            section: hunk.section,
        };
        let patch = self.diffs.get(&file)?.patch_for(hunk.index)?;
        Some(ToolOutcome::Job(exec::apply_patch(root, patch, target)))
    }

    fn toggle_fold_all(&mut self) {
        if self.collapsed.is_empty() {
            self.collapsed = Section::all().into_iter().map(Block::Files).collect();
            self.collapsed.insert(Block::Recent);
            self.collapsed.insert(Block::Stashes);
            self.collapsed.insert(Block::Unpulled);
        } else {
            self.collapsed.clear();
        }
        self.rebuild();
    }

    /// Stage what the cursor is on. Untracked files are staged by the same key,
    /// since "add this" is what the user means either way.
    fn stage(&self, all: bool) -> ToolOutcome {
        if !all {
            if let Some(outcome) = self.apply_hunk(exec::ApplyTarget::Index) {
                return outcome;
            }
        }
        let Some(root) = &self.root else {
            return ToolOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return ToolOutcome::Consumed;
        }
        ToolOutcome::Job(exec::stage(root, &Pathspec::all(&paths)))
    }

    /// Stage everything, the way Magic's stage-all does: every untracked file
    /// when the cursor is in that section, every tracked change anywhere
    /// else. Splitting the two halves keeps an unasked-for file from being
    /// swept into the index unread.
    fn stage_all(&self) -> ToolOutcome {
        let Some(root) = &self.root else {
            return ToolOutcome::Consumed;
        };
        if self.section_at_cursor() == Some(Section::Untracked) {
            return self.stage_untracked();
        }
        ToolOutcome::Job(exec::stage_all_tracked(root))
    }

    /// Stage every untracked file the view lists, wherever the cursor is,
    /// by name: only what is on screen goes in, never a file that appeared
    /// since the last read.
    fn stage_untracked(&self) -> ToolOutcome {
        let Some(root) = &self.root else {
            return ToolOutcome::Consumed;
        };
        let paths = self.paths_in(Section::Untracked);
        if paths.is_empty() {
            return ToolOutcome::Consumed;
        }
        ToolOutcome::Job(exec::stage(root, &Pathspec::all(&paths)))
    }

    fn unstage(&self, all: bool) -> ToolOutcome {
        if !all {
            if let Some(outcome) = self.apply_hunk(exec::ApplyTarget::IndexReverse) {
                return outcome;
            }
        }
        let Some(root) = &self.root else {
            return ToolOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return ToolOutcome::Consumed;
        }
        ToolOutcome::Job(exec::unstage(root, &Pathspec::all(&paths)))
    }

    /// Ask before throwing away work, naming what will go: nothing recovers a
    /// discarded working-tree change. What will go is decided now, while the
    /// question names it, and kept for the answer: the tree may be read again
    /// while the question is open, and a re-read that moved the rows must not
    /// turn a yes about one file into a discard of another.
    fn ask_discard(&mut self) -> ToolOutcome {
        let Some(discard) = self.discard_at_point() else {
            return ToolOutcome::Consumed;
        };
        let paths = self.targets();
        let label = match (&discard, paths.len()) {
            (Discard::Hunk(_), _) => format!(
                "Discard this hunk of {}? (y/n) ",
                paths.first().map_or("the file", String::as_str)
            ),
            (_, 1) => format!("Discard changes to {}? (y/n) ", paths[0]),
            (_, count) => format!("Discard changes to {count} files? (y/n) "),
        };
        self.pending_discard = Some(discard);
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag: ASK_DISCARD,
        })
    }

    /// What discarding at the cursor would throw away: the hunk under it, or
    /// the files it is on, deleted where git does not track them.
    fn discard_at_point(&self) -> Option<Discard> {
        if let Some(Item::Hunk(hunk)) = self.selected().map(|row| row.item.clone()) {
            let file = FileRow {
                path: hunk.path.clone(),
                section: hunk.section,
            };
            return self
                .diffs
                .get(&file)?
                .patch_for(hunk.index)
                .map(Discard::Hunk);
        }
        let paths = Pathspec::all(&self.targets());
        if paths.is_empty() {
            return None;
        }
        // Untracked files are not in the index, so `restore` cannot reach
        // them: deleting is what discarding means for those.
        Some(match self.section_at_cursor() == Some(Section::Untracked) {
            true => Discard::Untracked(paths),
            false => Discard::Tracked(paths),
        })
    }

    /// Carry out the discard the reader said yes to.
    fn discard(&mut self) -> ToolOutcome {
        let (Some(root), Some(discard)) = (self.root.clone(), self.pending_discard.take()) else {
            return ToolOutcome::Consumed;
        };
        ToolOutcome::Job(match discard {
            Discard::Hunk(patch) => {
                exec::apply_patch(&root, patch, exec::ApplyTarget::WorktreeReverse)
            }
            Discard::Tracked(paths) => exec::discard(&root, &paths),
            Discard::Untracked(paths) => exec::remove_untracked(&root, &paths),
        })
    }

    /// `P`: open the push menu, once where the branch pushes and which
    /// remotes there are has been read, since the menu names them.
    fn push(&mut self) -> ToolOutcome {
        match &self.root {
            Some(root) => ToolOutcome::Job(exec::push_context(root, self.status.branch.as_deref())),
            None => ToolOutcome::Consumed,
        }
    }

    /// `C-S-p`: push the current branch without asking where. It goes to
    /// the remote the branch already tracks, else `origin`, and is set to track
    /// it, so a branch the remote has never seen is created there.
    fn quick_push(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let Some(branch) = self.status.branch.clone() else {
            self.notice = Some(Notice::error("no branch to push"));
            return ToolOutcome::Consumed;
        };
        let remote = self
            .status
            .upstream
            .as_deref()
            .and_then(|upstream| upstream.split_once('/'))
            .map_or(DEFAULT_REMOTE, |(remote, _)| remote);
        ToolOutcome::Job(exec::push_upstream(&root, remote, &branch))
    }

    /// `C-S-M-f`: pull the current branch with rebase without asking where.
    /// It comes from the branch the current one tracks, else the branch of the
    /// same name on `origin`, so a branch that tracks nothing still pulls.
    fn quick_pull(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let Some(branch) = self.status.branch.clone() else {
            self.notice = Some(Notice::error("no branch to pull"));
            return ToolOutcome::Consumed;
        };
        let (remote, source) = self
            .status
            .upstream
            .as_deref()
            .and_then(|upstream| upstream.split_once('/'))
            .unwrap_or((DEFAULT_REMOTE, &branch));
        ToolOutcome::Job(exec::pull_from(&root, remote, source))
    }

    /// Commit what is staged, writing the message in the editor: the commit
    /// popup's `c`, on a chord of its own. Nothing staged is refused up front,
    /// since `git commit` with an empty index only fails with a wall of advice.
    fn commit_in_editor(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        if self.paths_in(Section::Staged).is_empty() {
            self.notice = Some(Notice::error("nothing staged to commit"));
            return ToolOutcome::Consumed;
        }
        self.commit(&root, CommitMode::New)
    }

    /// Commit as `mode` says, writing the message in the editor tab git waits
    /// on, with no terminal tab of its own.
    fn commit(&self, root: &std::path::Path, mode: CommitMode) -> ToolOutcome {
        let editor = crate::terminal::session::git_editor_command();
        ToolOutcome::Job(exec::commit(root, mode, editor))
    }

    /// Ask for the next open diff that has not been read since the last
    /// status, one at a time so a view of twenty open files does not run twenty
    /// commands at once.
    fn request_stale_diff(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let stale = self
            .rows
            .iter()
            .filter_map(|row| match &row.item {
                Item::File(file) => Some(file.clone()),
                _ => None,
            })
            .find(|file| self.expanded.contains(file) && !self.diffs.contains_key(file));
        match stale {
            Some(file) => {
                self.pending_diff = Some(file.clone());
                ToolOutcome::Job(diff_request(&root, &file))
            }
            None => ToolOutcome::Consumed,
        }
    }

    /// Drop the expansion of a file the status no longer lists: staged away, or
    /// committed, it has no diff to show.
    fn forget_unlisted_files(&mut self) {
        let listed: HashSet<FileRow> = self
            .status
            .files
            .iter()
            .map(|file| FileRow {
                path: file.path.clone(),
                section: file.section,
            })
            .collect();
        self.expanded.retain(|file| listed.contains(file));
    }

    /// Open a menu, which owns the next keystroke.
    fn open_popup(&mut self, popup: Popup) -> ToolOutcome {
        self.popup = Some(popup);
        ToolOutcome::Consumed
    }

    /// The keymap's `a` and `-`: apply or reverse the hunk under the cursor,
    /// doing nothing when the cursor is not on one.
    fn apply_at_point(&self, target: exec::ApplyTarget) -> ToolOutcome {
        self.apply_hunk(target).unwrap_or(ToolOutcome::Consumed)
    }

    /// Reset to the commit under the cursor, which is the only revision the
    /// view can name without asking.
    fn reset_at_point(&mut self, mode: ResetMode) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
            Some(Ok(rev)) => ToolOutcome::Job(exec::reset(&root, mode, &rev)),
            Some(Err(_)) | None => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
        }
    }

    /// Rebase interactively from the commit the cursor is on, keeping it and
    /// every commit after it in the plan, as magit's `r i` does. Off a commit
    /// there is nothing to start from, so the plan is of what the branch has
    /// over one picked from a list instead.
    ///
    /// Picking the checked-out branch itself, the only way in before, gives
    /// git an empty plan: rewriting the last few commits needs a start below
    /// them, which only a commit can say.
    fn rebase_interactively(&mut self, root: &Path) -> ToolOutcome {
        match self.revision_at_cursor().map(|rev| Rev::parse(&rev)) {
            Some(Ok(commit)) => ToolOutcome::Job(exec::rebase_from(root, &commit)),
            Some(Err(_)) | None => self.pick(ASK_REBASE, "Rebase onto", exec::Candidates::Branches),
        }
    }

    /// Copy what the cursor is on: a commit hash, or a path.
    fn yank_at_point(&mut self) -> ToolOutcome {
        let value = self.revision_at_cursor().or_else(|| self.path_at_cursor());
        match value {
            Some(value) => ToolOutcome::Yank(value),
            None => ToolOutcome::Consumed,
        }
    }

    /// Blame the file the cursor is on, or on one of its hunks: a heading or a
    /// commit has no single path to run against.
    fn blame_at_point(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        match self.selected().map(|row| row.item.clone()) {
            Some(Item::File(FileRow { path, .. })) | Some(Item::Hunk(HunkRow { path, .. })) => {
                self.pending_blame = Some(path.clone());
                ToolOutcome::Job(exec::blame(&root, &Pathspec::new(path)))
            }
            _ => {
                self.notice = Some(Notice::error("no file to blame here"));
                ToolOutcome::Consumed
            }
        }
    }

    /// Take the tree watch over once its thread has set it up, saying so
    /// when the repository could not be watched.
    fn adopt_started_watch(&mut self) {
        let Some(starting) = self.tree_watch_starting.take_if(|s| s.is_ready()) else {
            return;
        };
        self.tree_watch = starting.take();
        if self.tree_watch.is_none() {
            self.notice = Some(Notice::warning(UNWATCHED));
        }
    }

    /// Ask for the remote's URL, which the reply turns into a browser tool.
    fn open_in_remote(&self) -> ToolOutcome {
        match &self.root {
            Some(root) => ToolOutcome::Job(exec::remote_url(root, DEFAULT_REMOTE)),
            None => ToolOutcome::Consumed,
        }
    }

    /// Run git in a pane of its own, for a command that wants a terminal.
    fn spawn_git(&self, root: &std::path::Path, args: &[&str]) -> ToolOutcome {
        ToolOutcome::Spawn(SpawnRequest {
            args: args.iter().map(|a| a.to_string()).collect(),
            cwd: root.to_path_buf(),
            program: "git".to_string(),
        })
    }

    /// The path the cursor is on, whatever kind of row it is.
    fn path_at_cursor(&self) -> Option<String> {
        match self.selected().map(|row| &row.item) {
            Some(Item::File(file)) => Some(file.path.clone()),
            Some(Item::Hunk(hunk)) => Some(hunk.path.clone()),
            _ => None,
        }
    }

    /// The revision the cursor is on, for a command that takes one: in the
    /// output while one fills the view (a log's commits), else the status.
    fn revision_at_cursor(&self) -> Option<String> {
        let row = match &self.output {
            Some(output) => output.rows.get(output.list.cursor()),
            None => self.selected(),
        };
        match row.map(|row| &row.item) {
            Some(Item::Commit(hash)) => Some(hash.clone()),
            _ => None,
        }
    }

    /// Append the path under the cursor to `.gitignore`, by extension when
    /// asked. Writing the file directly is simpler than any git command for it.
    fn ignore_path(&mut self, by_extension: bool) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let Some(path) = self.path_at_cursor() else {
            return ToolOutcome::Consumed;
        };
        let pattern = ignore_pattern(&path, by_extension);
        let file = root.join(".gitignore");
        // Written off the window's thread: the file may sit on a slow mount
        // like any other in the tree.
        let work = Background::start(IGNORE_THREAD, move |_| {
            append_line(&file, &pattern)
                .map(|()| pattern)
                .map_err(|e| e.to_string())
        });
        match work.wait(IGNORE_WAIT) {
            Some(written) => self.take_ignored(written),
            None => {
                self.ignoring = Some(work);
                ToolOutcome::Consumed
            }
        }
    }

    /// Report what appending to `.gitignore` came to, and read the tree
    /// again so the ignored file leaves the view.
    fn take_ignored(&mut self, written: Result<String, String>) -> ToolOutcome {
        match written {
            Ok(pattern) => {
                self.notice = Some(Notice::info(format!("ignored {pattern}")));
                self.refresh()
            }
            Err(e) => {
                self.notice = Some(Notice::error(format!("failed: {e}")));
                ToolOutcome::Consumed
            }
        }
    }

    /// Resolve the key after `g`: each names a section to jump to, and
    /// anything else abandons the sequence.
    fn jump_to_section(&mut self, code: KeyCode) -> ToolOutcome {
        match code {
            // `gg` is the one Vim motion the view's own leader resolves,
            // because the leader claims the prefix first.
            KeyCode::Char('g') => self.list.set_cursor(0),
            KeyCode::Char('t') => self.move_to_section(Some(Section::Untracked)),
            KeyCode::Char('u') => self.move_to_section(Some(Section::Unstaged)),
            KeyCode::Char('s') => self.move_to_section(Some(Section::Staged)),
            KeyCode::Char('r') => self.move_to_section(None),
            KeyCode::Char('j') => self.move_to_entity(true),
            KeyCode::Char('k') => self.move_to_entity(false),
            _ => {}
        }
        ToolOutcome::Consumed
    }

    /// Open the file at point, at whatever line the row knows about.
    fn open_at_point(&self) -> ToolOutcome {
        match self.file_at_point() {
            Some(target) => ToolOutcome::OpenPath(target),
            None => ToolOutcome::Consumed,
        }
    }

    /// Hand the file at point to `$EDITOR`, in a pane of its own, for the
    /// editing the app's own editor deliberately cannot do.
    fn open_external(&self) -> ToolOutcome {
        match self.file_at_point() {
            Some(target) => ToolOutcome::SpawnEditor(target),
            None => ToolOutcome::Consumed,
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

    /// One file of the commit being read, by its index among the files it
    /// touched.
    fn commit_file(&self, index: usize) -> Option<&CommitFile> {
        self.output.as_ref()?.commit.as_ref()?.files.get(index)
    }

    /// The working-tree file the row at point belongs to, and where in it to
    /// land: a hunk opens at the line it changes rather than at the top, in a
    /// commit's patch as much as in the working tree's own.
    ///
    /// A row standing for a commit, a stash, or a section heading belongs to
    /// no one file, and a commit's file may no longer be in the working tree
    /// at all, which the editor reports when it cannot read it.
    fn file_at_point(&self) -> Option<OpenTarget> {
        let root = self.root.clone().unwrap_or_default();
        // A blame is one row per line of one file, so the row under the
        // cursor names a place in it as surely as a hunk header does, even
        // though the rows themselves stand for nothing foldable.
        if let Some(output) = self.output.as_ref() {
            if let Some(path) = &output.blamed {
                return Some(OpenTarget::at_line(
                    root.join(path),
                    output.list.cursor() + 1,
                ));
            }
        }
        match self.item_at_point()? {
            Item::File(file) => Some(OpenTarget::file(root.join(&file.path))),
            Item::Hunk(hunk) => {
                let path = root.join(&hunk.path);
                let key = FileRow {
                    path: hunk.path.clone(),
                    section: hunk.section,
                };
                let line = self
                    .diffs
                    .get(&key)
                    .and_then(|diff| diff.hunks.get(hunk.index))
                    .and_then(Hunk::new_start);
                Some(at_line_or_top(path, line))
            }
            Item::CommitFile(index) => {
                let file = self.commit_file(index)?;
                Some(OpenTarget::file(root.join(&file.path)))
            }
            Item::CommitHunk(file_index, hunk_index) => {
                let file = self.commit_file(file_index)?;
                let line = file.hunks.get(hunk_index).and_then(Hunk::new_start);
                Some(at_line_or_top(root.join(&file.path), line))
            }
            // Everything else names a commit, a stash, or a heading: things
            // with no one file behind them.
            Item::Commit(_)
            | Item::CommitChanges
            | Item::Heading(_)
            | Item::None
            | Item::RecentHeading
            | Item::Stash(_)
            | Item::StashHeading
            | Item::UnpulledHeading => None,
        }
    }
}

impl Tool for GitTool {
    /// What the tab is: which of git's views it shows (the status, a log, a
    /// commit, a blame), and of which project, so two git tabs read apart.
    fn title(&self) -> String {
        let Some(root) = self.root.as_deref() else {
            return TITLE_PREFIX.to_string();
        };
        let project = root.file_name().map_or_else(
            || root.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        let Some(output) = &self.output else {
            return format!("{TITLE_PREFIX} {STATUS_TITLE}: {project}");
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
        self.root
            .as_deref()
            .map(crate::model::path::abbreviate_home)
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        // The view moves and the cursor stays, as in an editor: the open log,
        // blame, or diff when there is one, else the status rows.
        match self.output.as_mut() {
            Some(output) => {
                let total = output.lines.len();
                output.list.view_mut().wheel(lines, total);
            }
            None => self.list.view_mut().wheel(lines, self.rows.len()),
        }
        ToolOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
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
            let note = match (self.loaded, &self.root) {
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
        match placed.shows_cursor {
            true => content.with_cursor_line(window.cursor),
            false => content,
        }
    }

    fn hint(&self) -> Option<ToolHint> {
        let popup = self.popup?;
        let items = match (popup, &self.push) {
            (Popup::Push, Some(menu)) => menu.choices(
                self.status.branch.as_deref(),
                self.status.upstream.as_deref(),
            ),
            (Popup::PushOptions, Some(menu)) => menu.option_choices(),
            _ => popup
                .choices()
                .iter()
                .map(|(key, what)| (key.to_string(), what.to_string()))
                .collect(),
        };
        Some(ToolHint {
            items,
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

    fn run_command(&mut self, name: &str) -> Option<ToolOutcome> {
        match GitCommand::from_name(name)? {
            GitCommand::OpenRemote => Some(self.open_in_remote()),
        }
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        let outcome = self.dispatch_key(key);
        self.note_view(outcome)
    }

    fn keys(&self) -> Vec<KeyHelp> {
        self.key_help()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        let outcome = self.answer_prompt(reply);
        self.note_view(outcome)
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> ToolOutcome {
        if tag == ASK_SEARCH && !query.is_empty() {
            self.search_for(query)
        } else {
            ToolOutcome::Ignored
        }
    }

    fn on_resume(&mut self) -> ToolOutcome {
        // Work asked for before the tool was covered was cancelled with it.
        self.watching = None;
        // The file that was just being edited is very likely one of the ones
        // this view reports on.
        self.refresh()
    }

    fn next_tick(&self) -> Option<Instant> {
        // An append to `.gitignore` under way is looked for soon.
        if self.ignoring.is_some() {
            return Some(Instant::now() + background::POLL);
        }
        // Nothing to watch until the repository is known and read once, and a
        // view the user is in the middle of (output, a popup, a question)
        // is not redrawn under them.
        if !self.loaded || self.output.is_some() || self.popup.is_some() {
            return None;
        }
        // A read already out sees what has happened by now; whatever happens
        // after it is looked for once it has answered.
        if self.pending_pick.is_some() || self.staged_only || self.watching.is_some() {
            return None;
        }
        match (&self.tree_watch, &self.tree_watch_starting) {
            (Some(tree), _) => tree.next_look(Instant::now()),
            // Taken over once its thread has answered, which wakes the window.
            (None, Some(starting)) => starting.is_ready().then(Instant::now),
            // Not watched: read again only when asked.
            (None, None) => None,
        }
    }

    fn on_tick(&mut self, now: Instant) -> ToolOutcome {
        if let Some(written) = self.ignoring.as_ref().and_then(Background::poll) {
            self.ignoring = None;
            return self.take_ignored(written);
        }
        self.adopt_started_watch();
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        // A read already out will see what has happened by now; whatever
        // changes after it is looked for once it has answered.
        let due = match (self.tree_watch.as_mut(), self.watching) {
            (Some(tree), None) => tree.changed(now),
            (Some(_), Some(_)) | (None, _) => None,
        };
        let Some(read) = due else {
            return ToolOutcome::Consumed;
        };
        self.watching = Some(read);
        ToolOutcome::Job(exec::status(&root))
    }

    fn on_job(&mut self, reply: JobReply) -> ToolOutcome {
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
                self.progress = progress::parse(&files);
                self.rebuild();
                ToolOutcome::Consumed
            }
            // The view asks for no directory walks, no searches, and no
            // listings of the machine.
            JobReply::DirSize { .. }
            | JobReply::FileOps(_)
            | JobReply::Processes(_)
            | JobReply::Search(_)
            | JobReply::System(_) => ToolOutcome::Consumed,
        }
    }

    /// The repository, for the status view; the staged diff that sits beside
    /// a commit message goes when the message does, so it is not reopened.
    fn snapshot(&self) -> Option<OpenTarget> {
        if self.staged_only {
            return None;
        }
        Some(OpenTarget::file(
            self.root.clone().unwrap_or_else(|| self.start.clone()),
        ))
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The file under the cursor decides it, so a tool opened from a diff
        // starts beside that file rather than at the top of the repository.
        // Rows standing for a commit or a section name no file and leave the
        // repository root as the answer.
        self.file_at_point()
            .and_then(|target| target.path.parent().map(PathBuf::from))
            .or_else(|| self.root.clone())
    }

    fn root_dir(&self) -> Option<PathBuf> {
        self.root.clone().or_else(|| Some(self.start.clone()))
    }

    fn file_reference(&self) -> Option<String> {
        if let Some(target) = self.file_at_point() {
            let base = crate::model::path::abbreviate_home(&target.path);
            if let Some(line) = target.line {
                return Some(format!("{base}:{line}"));
            }
            return Some(base);
        }
        self.root
            .as_ref()
            .map(|r| crate::model::path::abbreviate_home(r))
    }

    fn context_items(&self) -> Vec<ToolMenuItem> {
        // What a row offers is what that kind of row can do, which is why the
        // menu is built per item rather than as one list with things greyed
        // out: a commit cannot be staged and a heading has no path to copy.
        // Each list is in groups: reading the row, changing the index, and
        // copying what names it.
        fn group<const N: usize>(items: [Option<ToolMenuItem>; N]) -> Vec<ToolMenuItem> {
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
                group([menu_item(Act::Yank, LABEL_COPY_PATH)]),
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
                group([menu_item(Act::Yank, LABEL_COPY_PATH)]),
            ]),
            Some(Item::Commit(_)) => menu_groups([
                group([menu_item(Act::Open, LABEL_READ_COMMIT)]),
                group([menu_item(Act::Yank, LABEL_COPY_HASH)]),
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
                group([menu_item(Act::Refresh, LABEL_RELOAD)]),
            ]),
            Some(Item::None) | None => menu_groups([
                group([menu_item(Act::CommitInEditor, LABEL_COMMIT)]),
                group([
                    menu_item(Act::Push, LABEL_PUSH),
                    menu_item(Act::Pull, LABEL_PULL),
                    menu_item(Act::Fetch, LABEL_FETCH),
                ]),
                group([menu_item(Act::Refresh, LABEL_RELOAD)]),
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

    fn jump_to(&mut self, target: usize) {
        match self.output.as_mut() {
            Some(output) => output.list.jump_to(target),
            None => self.list.jump_to(target),
        }
    }

    fn jump_cancel(&mut self) {
        match self.output.as_mut() {
            Some(output) => output.list.jump_cancel(),
            None => self.list.jump_cancel(),
        }
    }

    fn jump_confirm(&mut self, target: usize) {
        match self.output.as_mut() {
            Some(output) => output.list.jump_confirm(target),
            None => self.list.jump_confirm(target),
        }
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
        Section::Untracked => exec::diff_untracked(root, &Pathspec::new(file.path.clone())),
        Section::Staged => exec::diff_file(root, &Pathspec::new(file.path.clone()), true),
        Section::Unmerged | Section::Unstaged => {
            exec::diff_file(root, &Pathspec::new(file.path.clone()), false)
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

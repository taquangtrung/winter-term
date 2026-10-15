//! Git: the working tree's state in a pane, and the keys that change it.
//!
//! - [`diff`]: reading a unified diff, and writing one hunk back out.
//! - [`exec`]: the git command lines the view runs.
//! - [`parse`]: reading git's own output.
//! - [`popup`]: the transient menus a key opens.
//! - [`rows`]: painting the view, and what each row stands for.
//! - [`watch`]: noticing that the repository changed under the view.
//! - [`words`]: the word-level diff that decorates a hunk's changed lines.

pub mod commit;
pub mod diff;
pub mod exec;
pub mod parse;
pub mod popup;
pub mod progress;
pub mod rows;
pub mod watch;
pub mod words;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::model::input::CursorMove;
use crate::model::input::{Key, KeyCode};
use crate::model::paint::{
    row_height, row_text, wheel_scroll, window_anchor, wrap_window, Frame, Icon, Span, Style,
};
use crate::model::tool::{
    find_match, CommandOutput, JobReply, JobRequest, OpenTarget, PickQuestion, PromptMode,
    PromptReply, PromptRequest, SpawnRequest, Tool, ToolHint, ToolMenuItem, ToolNotice,
    ToolOutcome,
};
use crate::model::vim::nav::{buffer_end, VimKey, VimNav};
use crate::tools::refresh::RefreshTimer;

use commit::{CommitContent, CommitFile};
use diff::{FileDiff, Hunk};
use exec::{LogScope, ResetMode, SequenceStep};
use parse::{Commit, Section, Stash, Status};
use popup::Popup;
use progress::Progress;
use rows::{Block, CommitFolds, FileRow, HunkRow, Item, ViewRow};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question the view asks.
/// Characters of a commit hash shown in a view's title: enough to identify it
/// at a glance without crowding out the subject beside it.
const COMMIT_TITLE_HASH_LEN: usize = 8;

/// How long after the last answer the working tree is read again to notice
/// changes made outside the view, where the platform cannot be watched.
const WATCH_INTERVAL: Duration = Duration::from_secs(3);

/// How long a watch read may stay unanswered before it is asked for again.
const WATCH_STALL: Duration = Duration::from_secs(20);

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

/// Output filling the view in place of the status: what it is, its lines,
/// and, for a view that paints its own, the rows it paints.
#[derive(Clone, Debug)]
pub struct Output {
    /// What produced it, for the title.
    title: String,
    /// The text a search reads over the view: the lines as they came back,
    /// or one entry per painted row for the views that paint their own.
    lines: Vec<String>,
    /// The painted rows, one per line, for output that paints itself; empty for
    /// everything else, which paints straight from its lines.
    ///
    /// A [`ViewRow`] rather than bare spans because a painted view may also
    /// carry an icon per row and an [`Item`] saying what the row stands for,
    /// which is what lets the commit view fold at the cursor.
    rows: Vec<ViewRow>,
    /// Cursor over the lines.
    cursor: usize,
    /// Whether more can be asked for, which only the log offers.
    more: bool,
    /// The commit being shown, kept so folding a file or a hunk can redraw at a
    /// different depth without re-running `git show`. `None` for every other
    /// kind of output.
    commit: Option<CommitContent>,
    /// The repository-relative path a blame is of, since a blame is one row
    /// per line of one file and so every row names a place in it. `None` for
    /// every other kind of output.
    blamed: Option<String>,
    /// Whether the heading over `commit`'s files is shut, which hides every
    /// one of them.
    changes_shut: bool,
    /// Files of `commit` showing no diff.
    folded_files: HashSet<usize>,
    /// `(file, hunk)` pairs of `commit` showing only their header.
    folded_hunks: HashSet<(usize, usize)>,
}

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
    /// Where the cursor and scroll were before jump previewing started: (cursor, scroll).
    jump_origin: Option<(usize, usize)>,
    cursor: usize,
    /// What the last command reported, waiting for the status bar to take it.
    notice: Option<ToolNotice>,
    /// The conflicted file a resolve is settling, staged once git has
    /// written the side that was taken.
    resolving: Option<String>,
    /// The diff line `Ctrl-Space` set the mark on, which `Alt-s` and `Alt-u`
    /// stage or unstage from, to the line under the cursor.
    line_mark: Option<usize>,
    /// Where the repository is rooted, once git has said.
    root: Option<PathBuf>,
    rows: Vec<ViewRow>,
    scroll: usize,
    /// Whether the wheel, not the cursor, last placed the view: the view then
    /// stays where it was put until a key brings the cursor back into it.
    wheel: bool,
    /// The last text searched for, repeated by the next and previous keys.
    search: String,
    /// The file whose diff was asked for and has not come back yet.
    pending_diff: Option<FileRow>,
    /// The menu waiting for its second key.
    popup: Option<Popup>,
    /// Output shown in place of the status view: a log, a blame, a listing.
    /// `None` when the status view is showing.
    output: Option<Output>,
    /// How many commits the log view last asked for.
    log_count: usize,
    /// How many recent commits the status view asks for, which `+` raises.
    recent_count: usize,
    /// The path a blame was asked for, held until the answer comes back,
    /// since the reply says only that it is a blame and not what of.
    pending_blame: Option<String>,
    /// Where the tool was opened, which is where the root is looked up from.
    start: PathBuf,
    status: Status,
    /// Set once a status has come back, so an empty view can say which it is:
    /// a clean tree, or one still being read.
    loaded: bool,
    /// The pane height the last paint saw, in view rows, for the paging
    /// motions. Zero until the first paint, when there is nothing to page.
    viewport: usize,
    /// The shared Vim motion state: the layer the view's unclaimed keys fall
    /// through to. Its own `g` leader claims the prefix before this sees it.
    nav: VimNav,
    /// The question a list of names is being gathered for.
    pending_pick: Option<PickQuestion>,
    /// When the working tree is next read to notice changes made elsewhere,
    /// for a platform where it cannot be watched.
    watch: RefreshTimer,
    /// The repository's file notifications, which say when to read it again.
    tree_watch: Option<watch::TreeWatch>,
    /// Whether the status read in flight was the watch's, so an answer that
    /// changed nothing can be told from one that must redraw.
    watching: bool,
    /// The last status git answered with, as it came back.
    last_status: Option<String>,
    /// Whether this tool only shows what is staged, as a diff, rather than
    /// the working tree's status: the view beside a commit message.
    staged_only: bool,
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
            jump_origin: None,
            cursor: 0,
            loaded: false,
            notice: None,
            line_mark: None,
            resolving: None,
            log_count: LOG_PAGE,
            recent_count: exec::RECENT_COMMITS,
            output: None,
            pending_diff: None,
            popup: None,
            root: None,
            rows: Vec::new(),
            scroll: 0,
            wheel: false,
            search: String::new(),
            start,
            status: Status::default(),
            viewport: 0,
            nav: VimNav::new(),
            pending_pick: None,
            staged_only: false,
            tree_watch: None,
            watch: RefreshTimer::new(WATCH_INTERVAL, WATCH_STALL),
            watching: false,
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
                more_commits: self.commits.len() >= self.recent_count,
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
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&ViewRow> {
        self.rows.get(self.cursor)
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

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
    }

    /// Move to the next or previous row that stands for something, skipping the
    /// blank lines between sections.
    fn move_to_entity(&mut self, forward: bool) {
        let step: isize = if forward { 1 } else { -1 };
        let mut index = self.cursor as isize;
        loop {
            index += step;
            if index < 0 || index as usize >= self.rows.len() {
                return;
            }
            if self.rows[index as usize].item != Item::None {
                self.cursor = index as usize;
                return;
            }
        }
    }

    /// Remember `query` and move to the first row holding it.
    fn search_for(&mut self, query: &str) -> ToolOutcome {
        self.search = query.to_string();
        self.search_step(true)
    }

    /// Move to the next row matching the last search, in whichever view is up:
    /// one query serves the status and the output it opens.
    fn search_step(&mut self, forward: bool) -> ToolOutcome {
        if self.search.is_empty() {
            self.notice = Some(ToolNotice::error("no search"));
            return ToolOutcome::Consumed;
        }
        if let Some(output) = self.output.as_mut() {
            if let Some(index) = find_match(&output.lines, &self.search, output.cursor, forward) {
                output.cursor = index;
                return ToolOutcome::Consumed;
            }
        } else {
            let labels: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
            if let Some(index) = find_match(&labels, &self.search, self.cursor, forward) {
                self.cursor = index;
                return ToolOutcome::Consumed;
            }
        }
        self.notice = Some(ToolNotice::error(format!("not found: {}", self.search)));
        ToolOutcome::Consumed
    }

    /// The next or previous section heading in the direction of `forward`,
    /// from wherever the cursor sits: the view's paragraph motion, since the
    /// blank lines between sections are its paragraph boundaries.
    fn move_to_boundary(&mut self, forward: bool) {
        let is_heading = |row: &ViewRow| matches!(row.item, Item::Heading(_) | Item::RecentHeading);
        let target = if forward {
            self.rows
                .iter()
                .enumerate()
                .skip(self.cursor + 1)
                .find(|(_, row)| is_heading(row))
                .map(|(i, _)| i)
        } else {
            (0..self.cursor).rev().find(|&i| is_heading(&self.rows[i]))
        };
        if let Some(index) = target {
            self.cursor = index;
        }
    }

    /// Interpret one of the shared Vim motions over the view's rows. The
    /// entities are the words (the blank separators between sections are what
    /// `w`/`b` step over), the section headings are the paragraphs, and a
    /// view has no columns, so the line motions mean its ends.
    fn apply_motion(&mut self, motion: CursorMove) {
        let last = self.rows.len().saturating_sub(1);
        match motion {
            CursorMove::Down => self.move_by(1),
            CursorMove::Up => self.move_by(-1),
            CursorMove::WordForward | CursorMove::WordForwardBig => self.move_to_entity(true),
            CursorMove::WordBack | CursorMove::WordBackBig => self.move_to_entity(false),
            CursorMove::WordEnd | CursorMove::WordEndBig => self.move_to_entity(true),
            CursorMove::ParagraphForward => self.move_to_boundary(true),
            CursorMove::ParagraphBack => self.move_to_boundary(false),
            CursorMove::Top | CursorMove::LineStart | CursorMove::FirstNonBlank => {
                self.cursor = 0;
            }
            CursorMove::Bottom | CursorMove::LineEnd => self.cursor = last,
            CursorMove::HalfPageDown => self.page_by(self.viewport / 2, true),
            CursorMove::HalfPageUp => self.page_by(self.viewport / 2, false),
            CursorMove::PageDown => self.page_by(self.viewport, true),
            CursorMove::PageUp => self.page_by(self.viewport, false),
            CursorMove::ScreenTop => self.cursor = self.scroll.min(last),
            CursorMove::ScreenMiddle => self.cursor = (self.scroll + self.viewport / 2).min(last),
            CursorMove::ScreenBottom => {
                self.cursor = self
                    .scroll
                    .saturating_add(self.viewport.saturating_sub(1))
                    .min(last);
            }
            // The column motions and the rest have no meaning over rows; the
            // keys that reach them here are the ones the view has not claimed
            // for something of its own.
            _ => {}
        }
    }

    /// `rows` view rows down or up, clamped to the view.
    fn page_by(&mut self, rows: usize, down: bool) {
        let step = rows.max(1) as isize;
        self.move_by(if down { step } else { -step });
    }

    /// Jump to a section's heading, which is how the `g` keys navigate.
    fn move_to_section(&mut self, section: Option<Section>) {
        let target = self.rows.iter().position(|row| match (&row.item, section) {
            (Item::Heading(found), Some(want)) => *found == want,
            (Item::RecentHeading, None) => true,
            _ => false,
        });
        if let Some(index) = target {
            self.cursor = index;
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
        if self.line_mark == Some(self.cursor) || self.hunk_line_at(self.cursor).is_none() {
            self.line_mark = None;
            self.notice = Some(ToolNotice::info("mark cleared"));
            return ToolOutcome::Consumed;
        }
        self.line_mark = Some(self.cursor);
        self.notice = Some(ToolNotice::info(
            "mark set; Alt-s or Alt-u takes the lines to here",
        ));
        ToolOutcome::Consumed
    }

    /// Stage (`Alt-s`) or unstage (`Alt-u`) only some lines of a hunk: from
    /// the mark to the cursor where both are in one hunk, or the line under the
    /// cursor alone.
    fn apply_lines(&mut self, unstage: bool) -> ToolOutcome {
        let Some((hunk, here)) = self.hunk_line_at(self.cursor) else {
            self.notice = Some(ToolNotice::error(
                "on a line of a diff, that takes one line",
            ));
            return ToolOutcome::Consumed;
        };
        let wanted = match unstage {
            true => Section::Staged,
            false => Section::Unstaged,
        };
        if hunk.section != wanted {
            let verb = if unstage { "unstage" } else { "stage" };
            self.notice = Some(ToolNotice::error(format!(
                "nothing to {verb} in this section"
            )));
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
            self.notice = Some(ToolNotice::error("no change on those lines"));
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
        ToolOutcome::Job(exec::stage(root, &paths))
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
            let paths = self.paths_in(Section::Untracked);
            if paths.is_empty() {
                return ToolOutcome::Consumed;
            }
            return ToolOutcome::Job(exec::stage(root, &paths));
        }
        ToolOutcome::Job(exec::stage_all_tracked(root))
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
        ToolOutcome::Job(exec::unstage(root, &paths))
    }

    /// Ask before throwing away work, naming what will go: nothing recovers a
    /// discarded working-tree change.
    fn ask_discard(&self) -> ToolOutcome {
        let paths = self.targets();
        let label = match paths.len() {
            0 => return ToolOutcome::Consumed,
            1 => format!("Discard changes to {}? (y/n) ", paths[0]),
            count => format!("Discard changes to {count} files? (y/n) "),
        };
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag: ASK_DISCARD,
        })
    }

    fn discard(&self) -> ToolOutcome {
        if let Some(outcome) = self.apply_hunk(exec::ApplyTarget::WorktreeReverse) {
            return outcome;
        }
        let Some(root) = &self.root else {
            return ToolOutcome::Consumed;
        };
        let paths = self.targets();
        if paths.is_empty() {
            return ToolOutcome::Consumed;
        }
        // Untracked files are not in the index, so `restore` cannot reach
        // them: deleting is what discarding means for those.
        if self.section_at_cursor() == Some(Section::Untracked) {
            return ToolOutcome::Job(exec::remove_untracked(root, &paths));
        }
        ToolOutcome::Job(exec::discard(root, &paths))
    }

    /// Push the current branch the way git is told to by `P`: to wherever it
    /// already goes.
    fn push(&self) -> ToolOutcome {
        match &self.root {
            Some(root) => ToolOutcome::Job(exec::push(root)),
            None => ToolOutcome::Consumed,
        }
    }

    /// `Ctrl-Shift-P`: push the current branch without asking where. It goes to
    /// the remote the branch already tracks, else `origin`, and is set to track
    /// it, so a branch the remote has never seen is created there.
    fn quick_push(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let Some(branch) = self.status.branch.clone() else {
            self.notice = Some(ToolNotice::error("no branch to push"));
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

    /// Commit what is staged, writing the message in the editor: the commit
    /// popup's `c`, on a chord of its own. Nothing staged is refused up front,
    /// since `git commit` with an empty index only fails with a wall of advice.
    fn commit_in_editor(&mut self) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        if self.paths_in(Section::Staged).is_empty() {
            self.notice = Some(ToolNotice::error("nothing staged to commit"));
            return ToolOutcome::Consumed;
        }
        self.commit(&root, false)
    }

    /// Commit what is staged, writing the message in the editor tab git waits
    /// on, with no terminal tab of its own.
    fn commit(&self, root: &std::path::Path, amend: bool) -> ToolOutcome {
        let editor = crate::terminal::session::git_editor_command();
        ToolOutcome::Job(exec::commit(root, amend, editor))
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
        match self.revision_at_cursor() {
            Some(rev) => ToolOutcome::Job(exec::reset(&root, mode, &rev)),
            None => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
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
            Some(Item::File(file)) => {
                self.pending_blame = Some(file.path.clone());
                ToolOutcome::Job(exec::blame(&root, &file.path))
            }
            Some(Item::Hunk(hunk)) => {
                self.pending_blame = Some(hunk.path.clone());
                ToolOutcome::Job(exec::blame(&root, &hunk.path))
            }
            _ => {
                self.notice = Some(ToolNotice::error("no file to blame here"));
                ToolOutcome::Consumed
            }
        }
    }

    /// Ask for the remote's URL, which the reply turns into a browser tool.
    fn open_in_remote(&self) -> ToolOutcome {
        match &self.root {
            Some(root) => ToolOutcome::Job(exec::remote_url(root, DEFAULT_REMOTE)),
            None => ToolOutcome::Consumed,
        }
    }

    /// The search keys, offered to both views before their own keys so that one
    /// query walks whichever rows are showing. `None` for anything else.
    fn on_search_key(&mut self, key: &Key) -> Option<ToolOutcome> {
        if key.alt || key.ctrl {
            return None;
        }
        match key.code {
            KeyCode::Char('/') => Some(self.ask(ASK_SEARCH, "/")),
            KeyCode::Char('n') => Some(self.search_step(true)),
            KeyCode::Char('N') => Some(self.search_step(false)),
            _ => None,
        }
    }

    /// Keys while output fills the view: move, ask for more, or go back.
    fn on_output_key(&mut self, key: &Key) -> ToolOutcome {
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        // A row of a commit's patch belongs to a file the same way a row of
        // the status view does, so it opens the same way. A view whose rows
        // stand for nothing (a log, a blame) keeps the key.
        if key.code == KeyCode::Enter && !key.ctrl {
            if let Some(target) = self.file_at_point() {
                return ToolOutcome::OpenPath(target);
            }
        }
        if key.ctrl && key.code == KeyCode::Char('o') {
            if let Some(target) = self.file_at_point() {
                return ToolOutcome::SpawnEditor(target);
            }
        }
        let Some(output) = self.output.as_mut() else {
            return ToolOutcome::Consumed;
        };
        let last = output.lines.len().saturating_sub(1);
        if let Some(motion) = buffer_end(key) {
            output.cursor = match motion {
                CursorMove::Top => 0,
                _ => last,
            };
            return ToolOutcome::Consumed;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                output.cursor = (output.cursor + 1).min(last);
                ToolOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                output.cursor = output.cursor.saturating_sub(1);
                ToolOutcome::Consumed
            }
            KeyCode::Home => {
                output.cursor = 0;
                ToolOutcome::Consumed
            }
            KeyCode::End => {
                output.cursor = last;
                ToolOutcome::Consumed
            }
            // The keymap's load-more key, which only a log can answer.
            KeyCode::Char('+') if output.more => {
                self.log_count += LOG_PAGE;
                match self.root.clone() {
                    Some(root) => {
                        ToolOutcome::Job(exec::log(&root, LogScope::Branch, self.log_count))
                    }
                    None => ToolOutcome::Consumed,
                }
            }
            KeyCode::Char('y') => {
                let line = output.lines.get(output.cursor).cloned().unwrap_or_default();
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
            KeyCode::Tab if key.shift => {
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
            KeyCode::Tab => {
                match output.rows.get(output.cursor).map(|row| row.item.clone()) {
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
                // Keep the cursor on the row it was on: folding a file leaves
                // its band in place, so the reader does not lose their spot.
                let at = output.cursor;
                repaint_commit(output);
                output.cursor = at.min(output.rows.len().saturating_sub(1));
                ToolOutcome::Consumed
            }
            KeyCode::Char('q') | KeyCode::Escape => {
                self.output = None;
                ToolOutcome::Consumed
            }
            _ => ToolOutcome::Ignored,
        }
    }

    /// Show `text` in place of the status view, as it came back.
    fn show_output(&mut self, title: &str, text: &str, more: bool) {
        self.output = Some(Output {
            cursor: 0,
            lines: text.lines().map(str::to_string).collect(),
            more,
            rows: Vec::new(),
            title: title.to_string(),
            commit: None,
            blamed: None,
            changes_shut: false,
            folded_files: HashSet::new(),
            folded_hunks: HashSet::new(),
        });
    }

    /// Show painted rows in place of the status view, with the text a search
    /// reads over them — one entry per row, since a painted view may show
    /// more rows than the text it came from had, or fewer.
    fn show_painted(&mut self, title: &str, rows: Vec<ViewRow>) {
        let lines: Vec<String> = rows.iter().map(|row| row_text(&row.spans)).collect();
        self.output = Some(Output {
            cursor: 0,
            lines,
            more: false,
            rows,
            title: title.to_string(),
            commit: None,
            blamed: None,
            changes_shut: false,
            folded_files: HashSet::new(),
            folded_hunks: HashSet::new(),
        });
    }

    /// Show one commit, kept as content rather than as lines so its files and
    /// hunks can be folded and unfolded in place.
    ///
    /// It opens shut: every file down to its band and every hunk down to its
    /// header, so a commit reads first as the list of what it touched and the
    /// patch is asked for a piece at a time. The status view opens the same
    /// way, and a commit touching thirty files is unreadable as a wall of
    /// diff. `Tab` opens what the cursor is on, `Shift-Tab` all of it.
    fn show_commit(&mut self, title: &str, content: CommitContent) {
        let folded_files = (0..content.files.len()).collect();
        let folded_hunks = content
            .files
            .iter()
            .enumerate()
            .flat_map(|(file, entry)| (0..entry.hunks.len()).map(move |hunk| (file, hunk)))
            .collect();
        let mut output = Output {
            cursor: 0,
            lines: Vec::new(),
            more: false,
            rows: Vec::new(),
            title: title.to_string(),
            commit: Some(content),
            blamed: None,
            changes_shut: false,
            folded_files,
            folded_hunks,
        };
        repaint_commit(&mut output);
        self.output = Some(output);
    }

    /// Act on a choice from the open menu. An unknown key closes the menu
    /// without doing anything, so a mistyped second key is harmless.
    fn on_popup_key(&mut self, popup: Popup, code: KeyCode) -> ToolOutcome {
        let KeyCode::Char(choice) = code else {
            return ToolOutcome::Consumed;
        };
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        match (popup, choice) {
            // A jump needs no repository: it moves within what is drawn.
            (Popup::Jump, _) => self.jump_to_section(code),
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

            (Popup::Commit, 'c') => self.commit(&root, false),
            (Popup::Commit, 'a') => self.commit(&root, true),
            (Popup::Commit, 'e') => {
                ToolOutcome::Job(exec::custom(&root, "commit --amend --no-edit"))
            }

            (Popup::Merge, 'm') => self.pick(ASK_MERGE, "Merge", exec::Candidates::Branches),
            (Popup::Merge, 'c') => {
                ToolOutcome::Job(exec::sequence(&root, "merge", SequenceStep::Continue))
            }
            (Popup::Merge, 'x') => {
                ToolOutcome::Job(exec::sequence(&root, "merge", SequenceStep::Abort))
            }

            // Interactive rebase needs a terminal for its todo list, so it goes
            // to a pane rather than being captured.
            (Popup::Rebase, 'i') => {
                self.pick(ASK_REBASE, "Rebase onto", exec::Candidates::Branches)
            }
            (Popup::Rebase, 'u') => ToolOutcome::Job(exec::rebase(&root, "@{upstream}")),
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
                    self.notice = Some(ToolNotice::error(format!("{path} is not in conflict")));
                    return ToolOutcome::Consumed;
                }
                self.resolving = Some(path.clone());
                ToolOutcome::Job(exec::resolve_conflict(&root, &path, side == 'o'))
            }
            (Popup::File, choice @ ('s' | 'u' | 'x' | 'd' | 'l' | 'b')) => {
                let Some(path) = self.path_at_cursor() else {
                    return ToolOutcome::Consumed;
                };
                match choice {
                    's' => self.stage(false),
                    'u' => self.unstage(false),
                    'x' => self.discard(),
                    'b' => ToolOutcome::Job(exec::blame(&root, &path)),
                    'd' => self.toggle_fold(),
                    _ => {
                        self.log_count = LOG_PAGE;
                        ToolOutcome::Job(exec::log(&root, LogScope::File(path), self.log_count))
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
                    ToolOutcome::Job(exec::log(&root, LogScope::File(path), self.log_count))
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

    /// Ask a one-line question, tagged so the answer finds its way back.
    /// Ask the same question [`ask`](Self::ask) does, but over the names it
    /// can be answered with: the reader filters a list instead of spelling a
    /// branch out. The names are read first, so this hands back the job that
    /// reads them and the answer comes through the same tag either way.
    fn pick(&mut self, tag: &'static str, label: &str, kind: exec::Candidates) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        self.pending_pick = Some(PickQuestion::new(tag, label));
        ToolOutcome::Job(exec::candidates(&root, kind))
    }

    fn ask(&self, tag: &'static str, label: &str) -> ToolOutcome {
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag,
        })
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

    /// The revision the cursor is on, for a command that takes one.
    fn revision_at_cursor(&self) -> Option<String> {
        match self.selected().map(|row| &row.item) {
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
        let pattern = if by_extension {
            match path.rsplit_once('.') {
                Some((_, extension)) => format!("*.{extension}"),
                None => path.clone(),
            }
        } else {
            path.clone()
        };
        match append_line(&root.join(".gitignore"), &pattern) {
            Ok(()) => {
                self.notice = Some(ToolNotice::info(format!("ignored {pattern}")));
                self.refresh()
            }
            Err(e) => {
                self.notice = Some(ToolNotice::error(format!("failed: {e}")));
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
            KeyCode::Char('g') => self.cursor = 0,
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
            Some(output) => output.rows.get(output.cursor).map(|row| row.item.clone()),
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
                return Some(OpenTarget::at_line(root.join(path), output.cursor + 1));
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

    /// What a finished command means for the view.
    fn on_command(&mut self, output: CommandOutput) -> ToolOutcome {
        if !output.succeeded() {
            // Whichever read failed, nothing is coming to fill the view in, and
            // the placeholder would hide the failure it is waiting out.
            self.loaded = true;
            self.watching = false;
            self.watch.on_answer(Instant::now());
            self.notice = Some(ToolNotice::error(format!("failed: {}", output.failure())));
            return ToolOutcome::Consumed;
        }
        match output.tag {
            exec::TAG_ROOT => {
                // Two lines: the working tree, then where the repository keeps
                // its own files, which a worktree or a submodule puts outside
                // the tree entirely.
                let mut lines = output.stdout.lines();
                let root = PathBuf::from(lines.next().unwrap_or_default().trim());
                self.git_dir = lines
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(PathBuf::from);
                self.root = Some(root.clone());
                if !self.staged_only {
                    self.tree_watch = watch::TreeWatch::start(&root, self.git_dir.as_deref());
                }
                if self.staged_only {
                    return ToolOutcome::Job(exec::diff_all(&root, true, None));
                }
                ToolOutcome::Job(exec::status(&root))
            }
            exec::TAG_STATUS => {
                let watched = std::mem::take(&mut self.watching);
                self.watch.on_answer(Instant::now());
                // A read made only to notice changes redraws nothing when it
                // finds none. An open diff is read from the files themselves,
                // which the status does not show changing, so it always reads on.
                let unchanged = self.last_status.as_deref() == Some(output.stdout.as_str());
                self.last_status = Some(output.stdout.clone());
                if watched && unchanged && self.expanded.is_empty() {
                    return ToolOutcome::Consumed;
                }
                self.status = parse::parse_status(&output.stdout);
                self.loaded = true;
                // A diff read before the change is now wrong, and staging a
                // hunk from a stale diff applies it to the wrong lines.
                self.diffs.clear();
                self.forget_unlisted_files();
                self.rebuild();
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::recent_log(root, self.recent_count)),
                    None => ToolOutcome::Consumed,
                }
            }
            exec::TAG_RESOLVE => match (&self.root, self.resolving.take()) {
                (Some(root), Some(path)) => {
                    self.notice = Some(ToolNotice::info(format!("resolved {path}")));
                    ToolOutcome::Job(exec::stage(root, &[path]))
                }
                _ => ToolOutcome::Consumed,
            },
            exec::TAG_DIFF => {
                if let Some(file) = self.pending_diff.take() {
                    self.diffs
                        .insert(file.clone(), diff::parse_diff(&output.stdout));
                    self.rebuild();
                }
                self.request_stale_diff()
            }
            // The status refresh is a chain: each read asks for the next, so
            // the view fills in from one pass rather than firing five jobs at
            // once and racing them.
            exec::TAG_LOG => {
                self.commits = parse::parse_decorated_log(&output.stdout);
                self.rebuild();
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::stash_entries(root)),
                    None => ToolOutcome::Consumed,
                }
            }
            exec::TAG_CANDIDATES => {
                let Some(question) = self.pending_pick.take() else {
                    return ToolOutcome::Consumed;
                };
                // A remote's own `HEAD` is a pointer at one of the names
                // already listed, never an answer of its own.
                match question.over_lines(&output.stdout, |name| !name.ends_with("/HEAD")) {
                    Some(request) => ToolOutcome::Pick(request),
                    // With nothing to choose from there is still a question
                    // to answer, so it falls back to being typed.
                    None => self.ask(question.tag, &format!("{}: ", question.label)),
                }
            }
            exec::TAG_STASHES => {
                self.stashes = parse::parse_stash_list(&output.stdout);
                self.rebuild();
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::unpulled(root)),
                    None => ToolOutcome::Consumed,
                }
            }
            exec::TAG_UNPULLED => {
                // A branch tracking nothing makes this fail, which is not a
                // failure worth reporting: it is behind nothing.
                self.unpulled = match output.succeeded() {
                    true => parse::parse_decorated_log(&output.stdout),
                    false => Vec::new(),
                };
                self.rebuild();
                match &self.git_dir {
                    Some(dir) => {
                        ToolOutcome::Job(JobRequest::ReadFiles(progress::state_files(dir)))
                    }
                    None => ToolOutcome::Consumed,
                }
            }
            // A blame and a plain read differ only in what the view is called.
            exec::TAG_BLAME | exec::TAG_READ => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(ToolNotice::error(NOTHING_TO_SHOW));
                } else {
                    let title = if output.tag == exec::TAG_BLAME {
                        "Blame"
                    } else {
                        "Output"
                    };
                    let blamed = (output.tag == exec::TAG_BLAME)
                        .then(|| self.pending_blame.take())
                        .flatten();
                    self.show_output(title, &output.stdout, false);
                    if let Some(output) = self.output.as_mut() {
                        output.blamed = blamed;
                    }
                }
                ToolOutcome::Consumed
            }
            // A whole diff fills the view decorated, line for line.
            exec::TAG_DIFF_VIEW => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(ToolNotice::error(NOTHING_TO_SHOW));
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    self.show_painted("Diff", rows::diff_view_rows(&lines));
                }
                ToolOutcome::Consumed
            }
            // The log view lists commits the way the status tail does, rather
            // than as the raw lines git wrote, so both read the same.
            exec::TAG_LOG_VIEW => {
                let commits = parse::parse_decorated_log(&output.stdout);
                if commits.is_empty() {
                    self.notice = Some(ToolNotice::error(NOTHING_TO_SHOW));
                } else {
                    let painted = rows::log_rows(&commits, self.root.as_deref(), now_unix(), true);
                    self.show_painted("Log", painted);
                    // The load-more key only means anything while a log shows.
                    if let Some(output) = &mut self.output {
                        output.more = true;
                    }
                }
                ToolOutcome::Consumed
            }
            // A commit fills the view as its content: the summary it was shown
            // with, then its files banded and their hunks decorated, the way
            // the working tree reads.
            exec::TAG_SHOW => {
                if output.stdout.trim().is_empty() {
                    self.notice = Some(ToolNotice::error(NOTHING_TO_SHOW));
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    let title = commit_title(&output.stdout);
                    self.show_commit(&title, CommitContent::parse(&lines));
                }
                ToolOutcome::Consumed
            }
            exec::TAG_REMOTE_URL => {
                let url = browser_url(output.stdout.trim());
                match url {
                    Some(url) => ToolOutcome::OpenExternal(std::path::PathBuf::from(url)),
                    None => {
                        self.notice = Some(ToolNotice::error("no web URL for that remote"));
                        ToolOutcome::Consumed
                    }
                }
            }
            // Anything that changed the repository: report it and re-read.
            tag => {
                self.notice = report_for(tag, &output).map(ToolNotice::info);
                self.refresh()
            }
        }
    }
}

impl Tool for GitTool {
    fn title(&self) -> String {
        "Git".to_string()
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
        let total = match &self.output {
            Some(output) => output.lines.len(),
            None => self.rows.len(),
        };
        self.scroll = wheel_scroll(self.scroll, lines, total, self.viewport);
        self.wheel = true;
        ToolOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        // Remember the viewport for the paging motions, which key handling
        // needs between paints.
        self.viewport = rows.saturating_sub(HEADER_ROWS).max(1);
        if let Some(output) = &self.output {
            let visible = rows.saturating_sub(HEADER_ROWS).max(1);
            let height = |index: usize| {
                row_height(
                    &output.lines[index],
                    cols,
                    wrap,
                    output
                        .rows
                        .get(index)
                        .map(|row| row.wrap_indent)
                        .unwrap_or(0),
                )
            };
            let anchor = window_anchor(self.wheel, self.scroll, output.cursor, visible);
            let window = wrap_window(self.scroll, anchor, output.lines.len(), visible, height);
            self.scroll = window.start;
            let header = vec![
                Span::new(Style::Header, output.title.clone()),
                Span::new(Style::Dim, "  q to go back".to_string()),
            ];
            let mut painted = vec![header];
            let mut wrap_indents = vec![0];
            let mut icons: Vec<Icon> = Vec::new();
            for (index, line) in output
                .lines
                .iter()
                .enumerate()
                .skip(window.start)
                .take(window.count)
            {
                match output.rows.get(index) {
                    Some(row) => {
                        if let Some(icon) = &row.icon {
                            icons.push(Icon {
                                row: painted.len(),
                                ..icon.clone()
                            });
                        }
                        painted.push(row.spans.clone());
                        wrap_indents.push(row.wrap_indent);
                    }
                    // Output that never painted itself shows as plain text.
                    None => {
                        painted.push(vec![Span::plain(line.clone())]);
                        wrap_indents.push(0);
                    }
                }
            }
            let content = Frame::new(painted)
                .with_icons(icons)
                .with_wrap_indents(wrap_indents);
            return match anchor == output.cursor {
                true => content.with_cursor_line(HEADER_ROWS + window.cursor),
                false => content,
            };
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
        let anchor = window_anchor(self.wheel, self.scroll, self.cursor, visible);
        let window = wrap_window(self.scroll, anchor, self.rows.len(), visible, height);
        self.scroll = window.start;
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
        match anchor == self.cursor {
            true => content.with_cursor_line(window.cursor),
            false => content,
        }
    }

    fn hint(&self) -> Option<ToolHint> {
        let popup = self.popup?;
        Some(ToolHint {
            items: popup
                .choices()
                .iter()
                .map(|(key, what)| (key.to_string(), what.to_string()))
                .collect(),
            title: popup.title().to_string(),
        })
    }

    fn take_notice(&mut self) -> Option<ToolNotice> {
        self.notice.take()
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        self.wheel = false;
        if let Some(popup) = self.popup.take() {
            return self.on_popup_key(popup, key.code);
        }
        if self.output.is_some() {
            return self.on_output_key(key);
        }
        if key.alt {
            if let Some(motion) = buffer_end(key) {
                self.apply_motion(motion);
                return ToolOutcome::Consumed;
            }
            return match key.code {
                KeyCode::Char('n') => {
                    self.move_to_entity(true);
                    ToolOutcome::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_to_entity(false);
                    ToolOutcome::Consumed
                }
                KeyCode::Char('y') => match &self.root {
                    Some(root) => ToolOutcome::Job(exec::show_refs(root)),
                    None => ToolOutcome::Consumed,
                },
                KeyCode::Char('b') => self.blame_at_point(),
                KeyCode::Char('g') => self.open_in_remote(),
                KeyCode::Char('s') => self.apply_lines(false),
                KeyCode::Char('u') => self.apply_lines(true),
                _ => ToolOutcome::Ignored,
            };
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('C') | KeyCode::Char('c') if key.shift => self.commit_in_editor(),
                KeyCode::Char('S') | KeyCode::Char('s') if key.shift => self.stage_all(),
                KeyCode::Char('P') | KeyCode::Char('p') if key.shift => self.quick_push(),
                KeyCode::Char('l') => self.open_popup(Popup::Log),
                KeyCode::Char('o') => self.open_external(),
                KeyCode::Space | KeyCode::Char(' ') => self.toggle_line_mark(),
                // The paging chords the shared layer binds fall through to it;
                // the rest stay with the window.
                _ => match self.nav.key(key) {
                    VimKey::Motion(motion) => {
                        self.apply_motion(motion);
                        ToolOutcome::Consumed
                    }
                    VimKey::Pending => ToolOutcome::Consumed,
                    VimKey::Unhandled => ToolOutcome::Ignored,
                },
            };
        }
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        match key.code {
            KeyCode::Tab if key.shift => {
                self.toggle_fold_all();
                ToolOutcome::Consumed
            }
            KeyCode::Tab => self.toggle_fold(),
            KeyCode::Enter => match self.selected().map(|row| row.item.clone()) {
                // Anything that belongs to a file opens that file: a path in a
                // section, a hunk of its diff, and the same two inside the
                // patch a commit is being read as.
                Some(Item::File(_))
                | Some(Item::Hunk(_))
                | Some(Item::CommitFile(_))
                | Some(Item::CommitHunk(_, _)) => self.open_at_point(),
                // A commit has no file to open, so Enter reads it instead:
                // what it changed, and the patch that changed it.
                Some(Item::Commit(hash)) => match self.root.clone() {
                    Some(root) => ToolOutcome::Job(exec::show_commit(&root, &hash)),
                    None => ToolOutcome::Consumed,
                },
                // A stash reads as the diff it holds, the way a commit reads
                // as the patch it is.
                Some(Item::Stash(index)) => match (self.root.clone(), self.stashes.get(index)) {
                    (Some(root), Some(stash)) => {
                        ToolOutcome::Job(exec::stash_show(&root, &stash.name))
                    }
                    _ => ToolOutcome::Consumed,
                },
                _ => ToolOutcome::Consumed,
            },
            KeyCode::Char('g') => self.open_popup(Popup::Jump),
            KeyCode::Char('s') => self.stage(false),
            KeyCode::Char('S') => self.stage(true),
            KeyCode::Char('u') => self.unstage(false),
            KeyCode::Char('U') => self.unstage(true),
            KeyCode::Char('x') => self.ask_discard(),
            KeyCode::Char('a') => self.apply_at_point(exec::ApplyTarget::Index),
            KeyCode::Char('-') => self.apply_at_point(exec::ApplyTarget::IndexReverse),
            KeyCode::Char('G') => self.refresh(),
            KeyCode::Char('y') => self.yank_at_point(),
            // Menus, each opening on the key it does in the configured keymap.
            KeyCode::Char('b') => self.open_popup(Popup::Branch),
            KeyCode::Char('c') => self.open_popup(Popup::Commit),
            KeyCode::Char('d') => self.open_popup(Popup::Diff),
            KeyCode::Char('i') | KeyCode::Char('I') => self.open_popup(Popup::Ignore),
            KeyCode::Char('m') => self.open_popup(Popup::Merge),
            KeyCode::Char('r') => self.open_popup(Popup::Rebase),
            KeyCode::Char('t') => self.open_popup(Popup::Tag),
            KeyCode::Char('z') => self.open_popup(Popup::Stash),
            KeyCode::Char('A') => self.open_popup(Popup::CherryPick),
            KeyCode::Char('B') => self.open_popup(Popup::Bisect),
            KeyCode::Char('M') => self.open_popup(Popup::Remote),
            KeyCode::Char('O') => self.open_popup(Popup::Reset),
            KeyCode::Char('V') => self.open_popup(Popup::Revert),
            KeyCode::Char('Z') => self.open_popup(Popup::Worktree),
            // Reset to a revision, straight from the cursor: the keymap's
            // direct mixed-reset key.
            KeyCode::Char('o') => self.reset_at_point(ResetMode::Mixed),
            // `.` is "this one": what can be done with the file the cursor is
            // already on, without recalling which key each of them was.
            KeyCode::Char('.') => match self.path_at_cursor() {
                Some(_) => self.open_popup(Popup::File),
                None => {
                    self.notice = Some(ToolNotice::error("no file here"));
                    ToolOutcome::Consumed
                }
            },
            KeyCode::Char('!') => self.ask(ASK_CUSTOM, "git "),
            KeyCode::Char('P') => self.push(),
            KeyCode::Char('F') => match &self.root {
                Some(root) => ToolOutcome::Job(exec::pull(root)),
                None => ToolOutcome::Consumed,
            },
            KeyCode::Char('f') => match &self.root {
                Some(root) => ToolOutcome::Job(exec::fetch(root)),
                None => ToolOutcome::Consumed,
            },
            // The load-more key of a log, for the recent commits listed here.
            KeyCode::Char('+') if self.commits.len() >= self.recent_count => {
                self.recent_count += LOG_PAGE;
                match &self.root {
                    Some(root) => ToolOutcome::Job(exec::recent_log(root, self.recent_count)),
                    None => ToolOutcome::Consumed,
                }
            }
            // Everything unclaimed falls through to the shared Vim motion
            // layer, whose motions this view interprets over its rows.
            _ => match self.nav.key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    ToolOutcome::Consumed
                }
                VimKey::Pending => ToolOutcome::Consumed,
                VimKey::Unhandled => ToolOutcome::Ignored,
            },
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        self.wheel = false;
        let Some(answer) = reply.answer else {
            self.notice = Some(ToolNotice::info("cancelled"));
            return ToolOutcome::Consumed;
        };
        // A search is the one question that asks nothing of git, so it is
        // answered before the repository is required.
        if reply.tag == ASK_SEARCH {
            return self.search_for(&answer);
        }
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let answer = answer.trim().to_string();
        // Every question but the stash message needs an answer to act on.
        if answer.is_empty() && reply.tag != ASK_STASH {
            return ToolOutcome::Consumed;
        }
        match reply.tag {
            ASK_BRANCH_CHECKOUT => ToolOutcome::Job(exec::checkout(&root, &answer)),
            ASK_BRANCH_CREATE => ToolOutcome::Job(exec::branch_create(&root, &answer, true)),
            ASK_BRANCH_CREATE_HERE => ToolOutcome::Job(exec::branch_create(&root, &answer, false)),
            ASK_BRANCH_DELETE => ToolOutcome::Job(exec::branch_delete(&root, &answer, false)),
            ASK_BRANCH_DELETE_FORCE => ToolOutcome::Job(exec::branch_delete(&root, &answer, true)),
            ASK_CHERRY_PICK => ToolOutcome::Job(exec::cherry_pick(&root, &answer)),
            ASK_CUSTOM => ToolOutcome::Job(exec::custom(&root, &answer)),
            ASK_DIFF_REV => ToolOutcome::Job(exec::diff_all(&root, false, Some(&answer))),
            ASK_DISCARD => self.discard(),
            ASK_MERGE => ToolOutcome::Job(exec::merge(&root, &answer)),
            ASK_REBASE => self.spawn_git(&root, &["rebase", "--interactive", &answer]),
            ASK_REMOTE_ADD => match answer.split_once(char::is_whitespace) {
                Some((name, url)) => {
                    ToolOutcome::Job(exec::remote_add(&root, name.trim(), url.trim()))
                }
                None => {
                    self.notice = Some(ToolNotice::error("failed: expected `name url`"));
                    ToolOutcome::Consumed
                }
            },
            ASK_REMOTE_PRUNE => ToolOutcome::Job(exec::remote_prune(&root, &answer)),
            ASK_REMOTE_REMOVE => ToolOutcome::Job(exec::remote_remove(&root, &answer)),
            ASK_RESET_HARD => ToolOutcome::Job(exec::reset(&root, ResetMode::Hard, &answer)),
            ASK_RESET_MIXED => ToolOutcome::Job(exec::reset(&root, ResetMode::Mixed, &answer)),
            ASK_RESET_SOFT => ToolOutcome::Job(exec::reset(&root, ResetMode::Soft, &answer)),
            ASK_REVERT => ToolOutcome::Job(exec::revert(&root, &answer)),
            ASK_STASH => ToolOutcome::Job(exec::stash_push(&root, &answer)),
            ASK_TAG_CREATE => ToolOutcome::Job(exec::tag_create(&root, &answer)),
            ASK_TAG_DELETE => ToolOutcome::Job(exec::tag_delete(&root, &answer)),
            ASK_WORKTREE_ADD => ToolOutcome::Job(exec::worktree_add(&root, &answer)),
            ASK_WORKTREE_REMOVE => ToolOutcome::Job(exec::worktree_remove(&root, &answer)),
            _ => ToolOutcome::Consumed,
        }
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
        self.watching = false;
        self.watch.restart(Instant::now());
        // The file that was just being edited is very likely one of the ones
        // this view reports on.
        self.refresh()
    }

    fn next_tick(&self) -> Option<Instant> {
        // Nothing to watch until the repository is known and read once, and a
        // view the user is in the middle of (output, a popup, a question)
        // is not redrawn under them.
        if !self.loaded || self.output.is_some() || self.popup.is_some() {
            return None;
        }
        if self.pending_pick.is_some() || self.staged_only {
            return None;
        }
        match &self.tree_watch {
            Some(tree) => Some(tree.next_look()),
            None => self.watch.next_tick(),
        }
    }

    fn on_tick(&mut self, now: Instant) -> ToolOutcome {
        let Some(root) = self.root.clone() else {
            return ToolOutcome::Consumed;
        };
        let due = match self.tree_watch.as_mut() {
            // A read already out will see what has happened by now; whatever
            // changes after it is looked for once it has answered.
            Some(_) if self.watching => false,
            Some(tree) => tree.changed(now),
            None => self.watch.on_tick(now),
        };
        if !due {
            return ToolOutcome::Consumed;
        }
        self.watching = true;
        ToolOutcome::Job(exec::status(&root))
    }

    fn on_job(&mut self, reply: JobReply) -> ToolOutcome {
        match reply {
            JobReply::Command(output) => self.on_command(output),
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
        match self.item_at_point() {
            Some(Item::File(_)) | Some(Item::CommitFile(_)) => vec![
                ToolMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN),
                ToolMenuItem::new(Key::with_ctrl(KeyCode::Char('o')), LABEL_OPEN_IN_EDITOR),
                ToolMenuItem::new(Key::plain(KeyCode::Tab), LABEL_DIFF),
                ToolMenuItem::new(Key::plain(KeyCode::Char('s')), LABEL_STAGE),
                ToolMenuItem::new(Key::plain(KeyCode::Char('u')), LABEL_UNSTAGE),
                ToolMenuItem::new(Key::plain(KeyCode::Char('x')), LABEL_DISCARD),
                ToolMenuItem::new(Key::with_alt(KeyCode::Char('b')), LABEL_BLAME),
                ToolMenuItem::new(Key::plain(KeyCode::Char('y')), LABEL_COPY_PATH),
            ],
            Some(Item::Hunk(_)) | Some(Item::CommitHunk(_, _)) => vec![
                ToolMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN_AT_LINE),
                ToolMenuItem::new(Key::plain(KeyCode::Char('s')), LABEL_STAGE_HUNK),
                ToolMenuItem::new(Key::plain(KeyCode::Char('u')), LABEL_UNSTAGE_HUNK),
                ToolMenuItem::new(Key::plain(KeyCode::Char('x')), LABEL_DISCARD_HUNK),
                ToolMenuItem::new(Key::plain(KeyCode::Char('a')), LABEL_APPLY_HUNK),
                ToolMenuItem::new(Key::plain(KeyCode::Char('-')), LABEL_REVERSE_HUNK),
                ToolMenuItem::new(Key::plain(KeyCode::Char('y')), LABEL_COPY_PATH),
            ],
            Some(Item::Commit(_)) => vec![
                ToolMenuItem::new(Key::plain(KeyCode::Enter), LABEL_READ_COMMIT),
                ToolMenuItem::new(Key::plain(KeyCode::Char('y')), LABEL_COPY_HASH),
            ],
            Some(Item::Stash(_)) => vec![
                ToolMenuItem::new(Key::plain(KeyCode::Enter), LABEL_READ_STASH),
                ToolMenuItem::new(Key::plain(KeyCode::Char('z')), LABEL_STASH_MENU),
            ],
            Some(Item::Heading(_))
            | Some(Item::CommitChanges)
            | Some(Item::StashHeading)
            | Some(Item::UnpulledHeading)
            | Some(Item::RecentHeading) => vec![
                ToolMenuItem::new(Key::plain(KeyCode::Tab), LABEL_FOLD),
                ToolMenuItem::new(Key::plain(KeyCode::Char('S')), LABEL_STAGE_ALL),
                ToolMenuItem::new(Key::plain(KeyCode::Char('U')), LABEL_UNSTAGE_ALL),
                ToolMenuItem::new(Key::plain(KeyCode::Char('G')), LABEL_RELOAD),
            ],
            Some(Item::None) | None => vec![
                ToolMenuItem::new(Key::with_ctrl_shift(KeyCode::Char('C')), LABEL_COMMIT),
                ToolMenuItem::new(Key::plain(KeyCode::Char('P')), LABEL_PUSH),
                ToolMenuItem::new(Key::plain(KeyCode::Char('F')), LABEL_PULL),
                ToolMenuItem::new(Key::plain(KeyCode::Char('f')), LABEL_FETCH),
                ToolMenuItem::new(Key::plain(KeyCode::Char('G')), LABEL_RELOAD),
            ],
        }
    }

    fn jump_targets(&self) -> Option<Vec<(usize, String)>> {
        if let Some(output) = &self.output {
            let targets = output
                .lines
                .iter()
                .enumerate()
                .map(|(idx, line)| (idx, format!("{:>5}  {}", idx + 1, line)))
                .collect();
            Some(targets)
        } else {
            let targets = self
                .rows
                .iter()
                .enumerate()
                .map(|(idx, row)| (idx, format!("{:>5}  {}", idx + 1, row_text(&row.spans))))
                .collect();
            Some(targets)
        }
    }

    fn jump_cursor(&self) -> Option<usize> {
        if let Some(output) = &self.output {
            Some(output.cursor)
        } else {
            Some(self.cursor)
        }
    }

    fn jump_to(&mut self, target: usize) {
        if self.jump_origin.is_none() {
            let cur = if let Some(output) = &self.output {
                output.cursor
            } else {
                self.cursor
            };
            self.jump_origin = Some((cur, self.scroll));
        }
        if let Some(output) = self.output.as_mut() {
            output.cursor = target.min(output.lines.len().saturating_sub(1));
        } else {
            self.cursor = target.min(self.rows.len().saturating_sub(1));
        }
        self.scroll = target.saturating_sub(self.viewport / 2);
    }

    fn jump_cancel(&mut self) {
        if let Some((origin, scroll)) = self.jump_origin.take() {
            if let Some(output) = self.output.as_mut() {
                output.cursor = origin.min(output.lines.len().saturating_sub(1));
            } else {
                self.cursor = origin.min(self.rows.len().saturating_sub(1));
            }
            self.scroll = scroll;
        }
    }

    fn jump_confirm(&mut self, target: usize) {
        self.jump_origin = None;
        if let Some(output) = self.output.as_mut() {
            output.cursor = target.min(output.lines.len().saturating_sub(1));
        } else {
            self.cursor = target.min(self.rows.len().saturating_sub(1));
        }
        self.scroll = target.saturating_sub(self.viewport / 2);
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

/// Redraw a commit output's rows at its current fold depth, keeping the search
/// text in step with them.
///
/// Called after every fold change rather than the rows being edited in place:
/// folding a file removes a variable number of rows, and rebuilding from the
/// content is the only way the row list, the search text, and the fold sets
/// cannot drift apart.
fn repaint_commit(output: &mut Output) {
    let Some(content) = &output.commit else {
        return;
    };
    output.rows = rows::commit_view_rows(
        content,
        CommitFolds {
            shut: output.changes_shut,
            files: &output.folded_files,
            hunks: &output.folded_hunks,
        },
    );
    output.lines = output.rows.iter().map(|row| row_text(&row.spans)).collect();
    output.cursor = output.cursor.min(output.rows.len().saturating_sub(1));
}

/// The current time, in seconds since the Unix epoch, for the commit rows to
/// measure a commit's age against.
///
/// A clock read before the system clock was set — which cannot be represented as
/// "seconds since the epoch" — reads as `0`, and every commit then shows no
/// meaningful age rather than the view failing to draw.
fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// Name the view showing one commit, from the `git show` output itself.
///
/// The abbreviated hash and the subject, which is what identifies a commit to a
/// reader. Taken from the output rather than from the row the cursor was on, so
/// the title cannot disagree with what is underneath it.
fn commit_title(shown: &str) -> String {
    let mut hash = String::new();
    let mut subject = String::new();
    for line in shown.lines() {
        match line.strip_prefix("commit ") {
            Some(rest) if hash.is_empty() => {
                hash = rest.split_whitespace().next().unwrap_or(rest).to_string();
                hash.truncate(COMMIT_TITLE_HASH_LEN);
            }
            // The subject is the first indented line of the message block, which
            // is the first thing after the headers that is neither blank nor a
            // header of its own.
            _ => {
                if !hash.is_empty() && subject.is_empty() {
                    if let Some(text) = line.strip_prefix("    ") {
                        subject = text.trim().to_string();
                    }
                }
            }
        }
    }
    match (hash.is_empty(), subject.is_empty()) {
        (true, _) => "Commit".to_string(),
        (false, true) => format!("Commit {hash}"),
        (false, false) => format!("Commit {hash}  {subject}"),
    }
}

/// Turn a git remote URL into one a browser can open: an `ssh` remote becomes
/// its `https` form, and anything else is left alone if it already is one.
fn browser_url(remote: &str) -> Option<String> {
    if remote.starts_with("http://") || remote.starts_with("https://") {
        return Some(remote.trim_end_matches(".git").to_string());
    }
    // `git@host:owner/repo.git`, the form every forge hands out.
    let rest = remote.strip_prefix("git@")?;
    let (host, path) = rest.split_once(':')?;
    Some(format!("https://{host}/{}", path.trim_end_matches(".git")))
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
        Section::Untracked => exec::diff_untracked(root, &file.path),
        Section::Staged => exec::diff_file(root, &file.path, true),
        Section::Unmerged | Section::Unstaged => exec::diff_file(root, &file.path, false),
    }
}

/// What to say in the header after a command that changed something, or
/// `None` where the re-read view already says it: staging and unstaging move
/// the file between the section headings, and discarding drops it from the
/// list.
fn report_for(tag: &'static str, output: &CommandOutput) -> Option<String> {
    let detail = output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match tag {
        exec::TAG_FETCH => Some("fetched".to_string()),
        exec::TAG_PULL => Some(format!("pulled: {detail}")),
        exec::TAG_PUSH => Some("pushed".to_string()),
        exec::TAG_DISCARD | exec::TAG_STAGE | exec::TAG_UNSTAGE => None,
        _ => Some(detail.to_string()),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::tool::{CommandRequest, NoticeKind};

    const STATUS_OUTPUT: &str = "\
# branch.head main
# branch.upstream origin/main
# branch.ab +1 -0
1 M. N... 100644 100644 100644 aaa bbb staged.rs
1 .M N... 100644 100644 100644 aaa bbb working.rs
? new.rs
";

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn alt(code: KeyCode) -> Key {
        Key {
            alt: true,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn ctrl_shift(c: char) -> Key {
        Key {
            alt: false,
            code: KeyCode::Char(c),
            ctrl: true,
            shift: true,
        }
    }

    fn reply(tag: &'static str, stdout: &str) -> JobReply {
        JobReply::Command(CommandOutput {
            code: Some(0),
            stderr: String::new(),
            stdout: stdout.to_string(),
            tag,
        })
    }

    fn command_of(outcome: &ToolOutcome) -> CommandRequest {
        match outcome {
            ToolOutcome::Job(JobRequest::Command(command)) => command.clone(),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    /// A tool that has heard back about its root, its status, and its log.
    fn ctrl(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
        }
    }

    /// One line of the decorated log format the view asks git for, so a test
    /// feeds the shape [`parse::parse_decorated_log`] actually reads.
    fn log_output(commits: &[(&str, &str)]) -> String {
        let sep = parse::FIELD_SEP;
        commits
            .iter()
            .map(|(hash, subject)| {
                format!("{hash}{sep}{sep}Someone{sep}1700000000{sep}{subject}\n")
            })
            .collect()
    }

    fn loaded_tool() -> GitTool {
        let mut tool = GitTool::new(PathBuf::from("/repo/sub"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        tool.on_job(reply(
            exec::TAG_LOG,
            &log_output(&[("abc1234", "do the thing")]),
        ));
        tool
    }

    #[test]
    fn test_the_address_is_the_repository_whatever_the_cursor_is_on() {
        // Before the repository is known there is nothing to name; afterwards
        // the title bar names the repository, the same text the `Repo:` row
        // carries, and not the folder of the file the cursor rests on.
        let mut tool = GitTool::new(PathBuf::from("/repo/sub"));
        assert_eq!(tool.address(), None);
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        assert_eq!(tool.address().as_deref(), Some("/repo"));
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "staged.rs");
        assert_eq!(tool.address().as_deref(), Some("/repo"));
    }

    #[test]
    fn test_tag_root_failure_shows_error_instead_of_loading() {
        let mut tool = GitTool::new(PathBuf::from("/not-a-repo"));
        tool.on_job(JobReply::Command(CommandOutput {
            code: Some(128),
            stderr: "fatal: not a git repository (or any of the parent directories): .git"
                .to_string(),
            stdout: String::new(),
            tag: exec::TAG_ROOT,
        }));
        let text = painted_text(&mut tool);
        assert!(text.contains(NOT_A_REPO), "{text}");
        assert!(!text.contains("fatal"), "git's words go to the status bar");
        let notice = tool.take_notice().expect("a failure is reported");
        assert_eq!(notice.kind, NoticeKind::Error);
        assert!(
            notice
                .text
                .starts_with("failed: fatal: not a git repository"),
            "{}",
            notice.text
        );
    }

    #[test]
    fn test_a_status_that_times_out_shows_the_failure_not_the_loading_note() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(JobReply::Command(CommandOutput {
            code: None,
            stderr: "command timed out after 15s".to_string(),
            stdout: String::new(),
            tag: exec::TAG_STATUS,
        }));
        let text = painted_text(&mut tool);
        assert!(text.contains(UNREADABLE), "{text}");
        assert!(!text.contains(LOADING));
        let notice = notice_text(&mut tool).unwrap_or_default();
        assert!(notice.contains("command timed out after 15s"), "{notice}");
    }

    #[test]
    fn test_a_report_goes_to_the_status_bar_not_the_header() {
        // The header is the repository's state; a report drawn there pushed
        // the sections down a row and stayed until the next key.
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_FETCH, ""));
        assert!(!painted_text(&mut tool).contains("fetched"));
        let notice = tool.take_notice().expect("the fetch is reported");
        assert_eq!(notice, ToolNotice::info("fetched"));
        assert_eq!(tool.take_notice(), None, "a report is shown once");
    }

    /// Everything the tool paints at a usual size, joined.
    fn painted_text(tool: &mut GitTool) -> String {
        tool.content(24, 80, false)
            .rows
            .iter()
            .flatten()
            .map(|span| span.text.as_str())
            .collect()
    }

    /// The text of what the tool has to report, taking it.
    fn notice_text(tool: &mut GitTool) -> Option<String> {
        tool.take_notice().map(|notice| notice.text)
    }

    #[test]
    fn test_the_wheel_moves_the_view_not_the_cursor_and_a_key_returns_to_it() {
        // Like scrolling an editor: the cursor stays on its row while the
        // view moves, hidden when it has left the screen, and the next key
        // brings the view back to it.
        let mut tool = loaded_tool();
        tool.cursor = 0;
        tool.content(8, 80, false);
        assert_eq!(tool.on_scroll(-3), ToolOutcome::Consumed);
        let shown = tool.content(8, 80, false);
        assert_eq!(tool.scroll, 3);
        assert_eq!(tool.cursor, 0, "the cursor did not move");
        assert_eq!(shown.cursor_line, None, "and it is off screen");
        tool.on_scroll(3);
        tool.content(8, 80, false);
        assert_eq!(tool.scroll, 0);
        tool.on_scroll(-3);
        tool.content(8, 80, false);
        tool.on_key(&press_char('j'));
        let shown = tool.content(8, 80, false);
        assert_eq!(
            tool.scroll, tool.cursor,
            "a key reveals the cursor at the edge"
        );
        assert!(shown.cursor_line.is_some());
    }

    fn cursor_on(tool: &mut GitTool, path: &str) {
        let index = tool
            .rows
            .iter()
            .position(|row| matches!(&row.item, Item::File(file) if file.path == path))
            .unwrap_or_else(|| panic!("no row for {path}"));
        tool.cursor = index;
    }

    #[test]
    fn test_resolving_a_conflict_takes_one_side_then_stages_the_file() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(
            exec::TAG_STATUS,
            "# branch.head main\nu UU N... 100644 100644 100644 100644 a b c fought.rs\n",
        ));
        cursor_on(&mut tool, "fought.rs");
        tool.on_key(&press(KeyCode::Char('.')));
        let command = command_of(&tool.on_key(&press(KeyCode::Char('t'))));
        assert_eq!(command.args, ["checkout", "--theirs", "--", "fought.rs"]);
        let next = command_of(&tool.on_job(reply(exec::TAG_RESOLVE, "")));
        assert_eq!(
            next.args,
            ["add", "--", "fought.rs"],
            "staging marks it resolved"
        );
    }

    #[test]
    fn test_every_menu_entry_runs_a_key_the_view_binds() {
        // The menu offers keys rather than commands of its own, so an entry
        // naming a chord the tool does not match does nothing when chosen.
        // The commit entry is `Ctrl-Shift-C`, which is exactly the shape that
        // gets written with a modifier missing.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let over_file = tool.context_items();

        let mut header = loaded_tool();
        header.cursor = 0;
        let over_header = header.context_items();

        for (items, on_file) in [(over_file, true), (over_header, false)] {
            assert!(!items.is_empty());
            for item in items {
                let mut probe = loaded_tool();
                match on_file {
                    true => cursor_on(&mut probe, "working.rs"),
                    false => probe.cursor = 0,
                }
                assert_ne!(
                    probe.on_key(&item.key),
                    ToolOutcome::Ignored,
                    "the menu offers {:?}, which the view does not bind",
                    item.label
                );
            }
        }
    }

    #[test]
    fn test_the_menu_offers_each_row_what_that_row_can_do() {
        // A commit cannot be staged and a file has no hash to copy, so a menu
        // built without looking at the row under it offers both everywhere.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let labels: Vec<String> = tool
            .context_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(&LABEL_STAGE.to_string()));
        assert!(!labels.contains(&LABEL_COPY_HASH.to_string()));

        let commit = tool
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("the log put a commit in the view");
        tool.cursor = commit;
        let labels: Vec<String> = tool
            .context_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(&LABEL_COPY_HASH.to_string()));
        assert!(!labels.contains(&LABEL_STAGE.to_string()));
    }

    #[test]
    fn test_the_root_lookup_leads_to_a_status_read() {
        // Every later command runs from the root, so the view cannot do
        // anything until that answer arrives.
        let mut tool = GitTool::new(PathBuf::from("/repo/sub"));
        let next = tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        let command = command_of(&next);
        assert_eq!(command.tag, exec::TAG_STATUS);
        assert_eq!(command.cwd, PathBuf::from("/repo"), "not the subdirectory");
    }

    #[test]
    fn test_a_status_read_is_followed_by_the_recent_log() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        let next = tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        assert_eq!(command_of(&next).tag, exec::TAG_LOG);
    }

    #[test]
    fn test_plus_asks_the_status_view_for_more_recent_commits() {
        // Only a list that filled its request can have more behind it; a short
        // one has shown everything there is, so the key has nothing to ask.
        let mut tool = loaded_tool();
        let before = tool.recent_count;
        let short = tool.on_key(&press_char('+'));
        assert!(!matches!(short, ToolOutcome::Job(_)), "nothing to ask for");
        assert_eq!(tool.recent_count, before, "one commit is not a full list");

        let full: Vec<(String, String)> = (0..before)
            .map(|n| (format!("c{n:06}"), format!("commit {n}")))
            .collect();
        let refs: Vec<(&str, &str)> = full
            .iter()
            .map(|(hash, subject)| (hash.as_str(), subject.as_str()))
            .collect();
        tool.on_job(reply(exec::TAG_LOG, &log_output(&refs)));
        let more = tool.on_key(&press_char('+'));
        assert!(tool.recent_count > before, "asked for more");
        let command = command_of(&more);
        assert_eq!(command.tag, exec::TAG_LOG);
        assert!(
            command
                .args
                .contains(&format!("--max-count={}", tool.recent_count)),
            "{:?}",
            command.args
        );
    }

    #[test]
    fn test_staging_acts_on_the_path_under_the_cursor() {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["add", "--", "working.rs"]);
    }

    #[test]
    fn test_staging_on_a_heading_acts_on_that_whole_section() {
        // The heading stands for its section, which is what makes "stage this
        // lot" one keystroke.
        let mut tool = loaded_tool();
        tool.cursor = tool
            .rows
            .iter()
            .position(|row| row.item == Item::Heading(Section::Unstaged))
            .expect("the unstaged heading");
        let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["add", "--", "working.rs"]);
    }

    #[test]
    fn test_stage_all_from_the_untracked_section_stages_only_untracked_files() {
        // Magic's stage-all splits by section: from untracked it stages the
        // files that section lists, by name.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "new.rs");
        let command = command_of(&tool.on_key(&ctrl_shift('S')));
        assert_eq!(command.args, ["add", "--", "new.rs"]);
    }

    #[test]
    fn test_stage_all_elsewhere_stages_tracked_changes_only() {
        // Anywhere else it is `add -u`, so an untracked file stays that way
        // until asked for by name.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let command = command_of(&tool.on_key(&ctrl_shift('S')));
        assert_eq!(command.args, ["add", "-u"]);
    }

    #[test]
    fn test_unstaging_uses_the_staged_copy_of_a_file_changed_on_both_sides() {
        // `staged.rs` and `working.rs` differ, but a file in both sections must
        // unstage from the staged row and stage from the unstaged one.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "staged.rs");
        let command = command_of(&tool.on_key(&press(KeyCode::Char('u'))));
        assert_eq!(command.args, ["restore", "--staged", "--", "staged.rs"]);
    }

    #[test]
    fn test_discarding_asks_first_and_names_the_file() {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let ToolOutcome::Prompt(request) = tool.on_key(&press(KeyCode::Char('x'))) else {
            panic!("expected a confirmation");
        };
        assert_eq!(request.mode, PromptMode::Confirm);
        assert!(
            request.label.contains("working.rs"),
            "got {:?}",
            request.label
        );
    }

    #[test]
    fn test_discarding_an_untracked_file_deletes_it_instead_of_restoring() {
        // `restore` cannot reach a path that is not in the index, so the view
        // would report success while leaving the file exactly where it was.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "new.rs");
        tool.on_key(&press(KeyCode::Char('x')));
        let command = command_of(&tool.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, ["clean", "--force", "-d", "--", "new.rs"]);
    }

    #[test]
    fn test_declining_the_confirmation_runs_nothing() {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        tool.on_key(&press(KeyCode::Char('x')));
        let outcome = tool.on_prompt(PromptReply {
            answer: None,
            tag: ASK_DISCARD,
        });
        assert_eq!(outcome, ToolOutcome::Consumed);
    }

    #[test]
    fn test_committing_nothing_is_refused_before_it_asks() {
        // `git commit` with an empty index fails with a wall of advice; not
        // asking for a message is the better answer.
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(exec::TAG_STATUS, "# branch.head main\n"));
        let outcome = tool.on_key(&Key {
            alt: false,
            code: KeyCode::Char('c'),
            ctrl: true,
            shift: true,
        });
        assert_eq!(outcome, ToolOutcome::Consumed);
    }

    #[test]
    fn test_a_command_that_changed_the_tree_re_reads_it() {
        // Without the re-read the view shows the state before the change, and
        // the next key acts on rows that no longer describe the repository.
        let mut tool = loaded_tool();
        let next = tool.on_job(reply(exec::TAG_STAGE, ""));
        assert_eq!(command_of(&next).tag, exec::TAG_STATUS);
        assert_eq!(
            tool.take_notice(),
            None,
            "the section headings already say it"
        );
    }

    #[test]
    fn test_a_failed_command_reports_and_does_not_re_read() {
        let mut tool = loaded_tool();
        let outcome = tool.on_job(JobReply::Command(CommandOutput {
            code: Some(1),
            stderr: "error: pathspec 'nope' did not match\n".to_string(),
            stdout: String::new(),
            tag: exec::TAG_STAGE,
        }));
        assert_eq!(outcome, ToolOutcome::Consumed);
        let message = notice_text(&mut tool).unwrap_or_default();
        assert!(message.starts_with("failed:"), "got {message:?}");
        assert!(message.contains("pathspec"), "got {message:?}");
    }

    const DIFF_OUTPUT: &str = "\
diff --git a/working.rs b/working.rs
index aaa..bbb 100644
--- a/working.rs
+++ b/working.rs
@@ -1,3 +1,3 @@
 fn main() {
-    old();
+    new();
@@ -10,2 +10,3 @@ fn other() {
     keep();
+    added();
";

    fn cursor_on_hunk(tool: &mut GitTool, index: usize) {
        let position = tool
            .rows
            .iter()
            .position(|row| matches!(&row.item, Item::Hunk(hunk) if hunk.index == index))
            .unwrap_or_else(|| panic!("no row for hunk {index}"));
        tool.cursor = position;
    }

    /// A tool with `working.rs` expanded and its diff read.
    fn tool_with_diff() -> GitTool {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        tool.on_key(&press(KeyCode::Tab));
        tool.on_job(reply(exec::TAG_DIFF, DIFF_OUTPUT));
        tool
    }

    #[test]
    fn test_enter_on_a_hunk_opens_its_file_at_the_line_the_hunk_changes() {
        // Opening the file at line one from a hunk throws away the one thing
        // the cursor's position said: which part of it is being read.
        let mut tool = tool_with_diff();
        cursor_on_hunk(&mut tool, 1);
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 10))
        );

        // And the file's own band still opens it at the top, since it names
        // no line in particular.
        cursor_on(&mut tool, "working.rs");
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/working.rs")))
        );
    }

    #[test]
    fn test_enter_in_a_commits_patch_opens_the_file_that_row_belongs_to() {
        // A commit is read as its patch, and every file band and hunk in it
        // stands for a file in the working tree: Enter did nothing there.
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));

        output_cursor_on(&mut tool, "src/a.rs");
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/src/a.rs"))),
            "the file band opens the file"
        );

        // Open the file's diff, then land on its second hunk.
        tool.on_key(&press(KeyCode::Tab));
        let output = tool.output.as_mut().expect("an output view");
        output.cursor = output
            .rows
            .iter()
            .position(|row| row.item == Item::CommitHunk(0, 1))
            .expect("a row for the second hunk");
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 9)),
            "and a hunk opens it at the line it changes"
        );
    }

    #[test]
    fn test_enter_in_a_blame_opens_the_file_at_the_line_under_the_cursor() {
        // A blame is one row per line of the file, so the row the cursor is
        // on is the line to land on: opening at the top would throw away the
        // only thing the reader was pointing at.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        tool.on_key(&alt(KeyCode::Char('b')));
        tool.on_job(reply(
            exec::TAG_BLAME,
            "abc1234 (Someone 2026-09-18 1) fn main() {\n             abc1234 (Someone 2026-09-18 2)     old();\n             def5678 (Another 2026-09-18 3) }\n",
        ));

        let output = tool.output.as_mut().expect("the blame");
        output.cursor = 2;
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 3))
        );
    }

    #[test]
    fn test_enter_on_a_row_belonging_to_no_file_is_left_alone() {
        // A log's rows stand for nothing to open, and swallowing the key
        // there would take it from the window for no gain.
        let mut tool = loaded_tool();
        tool.on_job(reply(
            exec::TAG_LOG,
            &log_output(&[("abc1234", "a commit")]),
        ));
        tool.on_key(&ctrl_shift('l'));
        if tool.output.is_some() {
            assert_eq!(tool.on_key(&press(KeyCode::Enter)), ToolOutcome::Ignored);
        }

        // In the status view, a section heading names a set of files rather
        // than one, so Enter has nothing to open there either.
        let mut status = loaded_tool();
        let heading = status
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Heading(_)))
            .expect("a heading row");
        status.cursor = heading;
        assert_eq!(status.on_key(&press(KeyCode::Enter)), ToolOutcome::Consumed);
    }

    #[test]
    fn test_unfolding_a_file_in_a_short_pane_keeps_its_row_where_it_was() {
        // Unfolding used to scroll the opened row to the top of the pane
        // whenever its diff ran past the bottom, moving the line the reader
        // had just pressed Tab on.
        let mut tool = tool_with_diff();
        tool.on_key(&press(KeyCode::Tab));
        tool.content(6, 80, false);
        let (cursor, scroll) = (tool.cursor, tool.scroll);
        tool.on_key(&press(KeyCode::Tab));
        tool.content(6, 80, false);
        assert_eq!(
            (tool.cursor, tool.scroll),
            (cursor, scroll),
            "the row stays on the same line of the pane"
        );
    }

    #[test]
    fn test_opening_a_file_already_showing_in_full_leaves_the_tool_alone() {
        // The scroll only moves for a block that runs past the pane's bottom:
        // one that fits must not shift the tool under the reader.
        let mut tool = tool_with_diff();
        tool.content(40, 80, false);
        let before = tool.scroll;
        tool.on_key(&press(KeyCode::Tab));
        tool.content(40, 80, false);
        tool.on_key(&press(KeyCode::Tab));
        tool.content(40, 80, false);
        assert_eq!(tool.scroll, before, "a pane with room to spare stays put");
    }

    #[test]
    fn test_unfolding_a_commits_file_in_a_short_pane_keeps_its_row_where_it_was() {
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        output_cursor_on(&mut tool, "src/a.rs");
        tool.content(5, 80, false);
        let scroll = tool.scroll;
        tool.on_key(&press(KeyCode::Tab));
        tool.content(5, 80, false);
        assert_eq!(tool.scroll, scroll, "the opened file's row does not move");
    }

    #[test]
    fn test_expanding_a_file_reads_its_diff_from_the_right_side() {
        // A staged file's diff is the index one; asking for the working-tree
        // diff there would show changes that are not staged.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "staged.rs");
        let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
        assert!(
            command.args.contains(&"--cached".to_string()),
            "{:?}",
            command.args
        );

        cursor_on(&mut tool, "working.rs");
        let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
        assert!(
            !command.args.contains(&"--cached".to_string()),
            "{:?}",
            command.args
        );
    }

    #[test]
    fn test_an_untracked_file_is_diffed_against_nothing() {
        // git has no indexed copy to compare against, so a plain `diff` prints
        // nothing at all and the file looks unchanged.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "new.rs");
        let command = command_of(&tool.on_key(&press(KeyCode::Tab)));
        assert!(
            command.args.contains(&"--no-index".to_string()),
            "{:?}",
            command.args
        );
    }

    #[test]
    fn test_an_expanded_file_lists_its_hunks_and_lines() {
        let tool = tool_with_diff();
        let hunks: Vec<usize> = tool
            .rows
            .iter()
            .filter_map(|row| match &row.item {
                Item::Hunk(hunk) => Some(hunk.index),
                _ => None,
            })
            .collect();
        assert!(hunks.contains(&0) && hunks.contains(&1), "got {hunks:?}");
        assert!(hunks.len() > 2, "the lines belong to their hunk too");
    }

    #[test]
    fn test_an_expanded_files_diff_decorates_the_edited_words() {
        // The whole point of the word diff: a replaced line paints its edit in
        // the line's style and the words the edit left alone recede.
        let tool = tool_with_diff();
        let added = tool
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("new();"))
            .expect("the added line");
        assert!(
            added
                .spans
                .iter()
                .any(|span| span.style == Style::AddedEdit),
            "got {:?}",
            added.spans
        );
        assert!(
            added.spans.iter().any(|span| span.style == Style::Added),
            "got {:?}",
            added.spans
        );
    }

    #[test]
    fn test_wrapping_puts_the_cursor_on_its_wrapped_screen_row() {
        // A changed line wider than the pane takes several screen rows, so the
        // band must follow the wrapped offset of the cursor's row, not its
        // row number, or it lights up the wrong line.
        let long = "x".repeat(45);
        let text = format!(
            "diff --git a/working.rs b/working.rs\n--- a/working.rs\n+++ b/working.rs\n@@ -1 +1 @@\n-{long}\n+{long}y\n"
        );
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        tool.on_key(&press(KeyCode::Tab));
        tool.on_job(reply(exec::TAG_DIFF, &text));
        let added = tool
            .rows
            .iter()
            .position(|row| row_text(&row.spans).contains(&format!("{long}y")))
            .expect("the added line");
        tool.cursor = added;
        // Stated relative to the cursor's row index: the status header is a
        // labelled block whose height can change, and an absolute screen row
        // here would be measuring the header rather than the wrapping.
        let flat = tool.content(20, 80, false);
        assert_eq!(
            flat.cursor_line,
            Some(added),
            "unwrapped, every row is one screen row, so the band sits on its own index"
        );
        let wrapped = tool.content(20, 32, true);
        assert_eq!(
            wrapped.cursor_line,
            Some(added + 1),
            "the removed line's wrapped rows push the band down"
        );
        assert!(
            wrapped.wrap_indents.contains(&1),
            "the hunk's lines carry their marker column, got {:?}",
            wrapped.wrap_indents
        );
    }

    #[test]
    fn test_the_diff_popup_fills_the_view_with_a_decorated_diff() {
        let mut tool = loaded_tool();
        tool.on_key(&press(KeyCode::Char('d')));
        let outcome = tool.on_key(&press(KeyCode::Char('d')));
        assert_eq!(command_of(&outcome).args[0], "diff");
        tool.on_job(reply(exec::TAG_DIFF_VIEW, DIFF_OUTPUT));
        let output = tool.output.as_ref().expect("the diff view");
        assert_eq!(output.title, "Diff");
        assert_eq!(output.rows.len(), output.lines.len());
        let added = output
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("new();"))
            .expect("the added line");
        assert!(
            added
                .spans
                .iter()
                .any(|span| span.style == Style::AddedEdit),
            "got {added:?}"
        );
        assert!(
            added.spans.iter().any(|span| span.style == Style::Added),
            "got {added:?}"
        );
    }

    #[test]
    fn test_staging_on_a_hunk_applies_that_hunk_alone() {
        // The whole point of hunk staging: the second hunk must not be in the
        // patch, and the patch goes in on standard input.
        let mut tool = tool_with_diff();
        cursor_on_hunk(&mut tool, 0);
        let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["apply", "--unidiff-zero", "--cached", "-"]);
        let patch = command.stdin.clone().expect("the patch");
        assert!(patch.contains("-    old();"), "got {patch}");
        assert!(!patch.contains("added()"), "got {patch}");
    }

    #[test]
    fn test_alt_s_stages_only_the_line_under_the_cursor() {
        let mut tool = tool_with_diff();
        cursor_on_hunk(&mut tool, 0);
        // Header, ` fn main() {`, then `-    old();`.
        tool.cursor += 2;
        let alt_s = Key {
            alt: true,
            ..press(KeyCode::Char('s'))
        };
        let command = command_of(&tool.on_key(&alt_s));
        let patch = command.stdin.clone().expect("the patch");
        assert!(patch.contains("@@ -1,2 +1,1 @@"), "recounted: {patch}");
        assert!(
            patch.contains("\n-    old();\n"),
            "the chosen removal: {patch}"
        );
        assert!(
            !patch.contains("new();"),
            "the unchosen addition is dropped: {patch}"
        );
    }

    #[test]
    fn test_the_mark_and_alt_s_stage_the_lines_between_them() {
        let mut tool = tool_with_diff();
        cursor_on_hunk(&mut tool, 0);
        tool.cursor += 2;
        tool.on_key(&ctrl(KeyCode::Space));
        tool.cursor += 1;
        let alt_s = Key {
            alt: true,
            ..press(KeyCode::Char('s'))
        };
        let patch = command_of(&tool.on_key(&alt_s)).stdin.expect("the patch");
        assert!(
            patch.contains("-    old();\n+    new();"),
            "both lines: {patch}"
        );
        assert_eq!(tool.line_mark, None, "the mark is spent");
    }

    #[test]
    fn test_a_key_inside_a_hunk_acts_on_that_hunk() {
        // Only the header row would be a cruel target; every line of the hunk
        // stands for it.
        let mut tool = tool_with_diff();
        let line = tool
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(&row.item, Item::Hunk(hunk) if hunk.index == 1))
            .nth(1)
            .map(|(index, _)| index)
            .expect("a line inside the second hunk");
        tool.cursor = line;
        let command = command_of(&tool.on_key(&press(KeyCode::Char('s'))));
        let patch = command.stdin.clone().expect("the patch");
        assert!(patch.contains("+    added();"), "got {patch}");
    }

    #[test]
    fn test_unstaging_a_hunk_reverses_it_out_of_the_index() {
        let mut tool = tool_with_diff();
        cursor_on_hunk(&mut tool, 0);
        let command = command_of(&tool.on_key(&press(KeyCode::Char('u'))));
        assert_eq!(
            command.args,
            ["apply", "--unidiff-zero", "--cached", "--reverse", "-"]
        );
    }

    #[test]
    fn test_discarding_a_hunk_reverses_it_out_of_the_working_tree() {
        let mut tool = tool_with_diff();
        cursor_on_hunk(&mut tool, 0);
        tool.on_key(&press(KeyCode::Char('x')));
        let command = command_of(&tool.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, ["apply", "--unidiff-zero", "--reverse", "-"]);
    }

    #[test]
    fn test_a_new_status_drops_the_diffs_it_invalidated() {
        // Staging a hunk changes the file's diff; applying a patch built from
        // the old one would hit the wrong lines.
        let mut tool = tool_with_diff();
        assert!(!tool.diffs.is_empty());
        tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        assert!(tool.diffs.is_empty(), "the stale diff is gone");
    }

    #[test]
    fn test_a_refresh_reads_everything_the_view_shows_in_one_chain() {
        // Each read asks for the next, so one refresh fills the whole view
        // rather than five jobs racing each other into it.
        let mut tool = tool_with_diff();
        tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        let after_log = tool.on_job(reply(exec::TAG_LOG, ""));
        assert_eq!(command_of(&after_log).tag, exec::TAG_STASHES);
        let after_stashes = tool.on_job(reply(exec::TAG_STASHES, ""));
        assert_eq!(command_of(&after_stashes).tag, exec::TAG_UNPULLED);

        // The chain ends on the state files, which no command reports.
        let after_unpulled = tool.on_job(reply(exec::TAG_UNPULLED, ""));
        let ToolOutcome::Job(JobRequest::ReadFiles(paths)) = after_unpulled else {
            panic!("expected the state files, got {after_unpulled:?}");
        };
        assert!(paths.iter().any(|path| path.ends_with("MERGE_HEAD")));

        // And the open diff is what still needs reading.
        let stale = tool.request_stale_diff();
        assert_eq!(command_of(&stale).tag, exec::TAG_DIFF);
    }

    #[test]
    fn test_a_file_the_status_no_longer_lists_stops_being_expanded() {
        let mut tool = tool_with_diff();
        tool.on_job(reply(
            exec::TAG_STATUS,
            "# branch.head main
",
        ));
        assert!(tool.expanded.is_empty());
    }

    #[test]
    fn test_folding_an_expanded_file_hides_its_diff_without_re_reading() {
        let mut tool = tool_with_diff();
        cursor_on(&mut tool, "working.rs");
        let outcome = tool.on_key(&press(KeyCode::Tab));
        assert_eq!(outcome, ToolOutcome::Consumed, "nothing to run");
        assert!(!tool
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::Hunk(_))));
    }

    fn shift(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: true,
        }
    }

    fn choose(tool: &mut GitTool, opener: char, choice: char) -> ToolOutcome {
        tool.on_key(&press_char(opener));
        tool.on_key(&press_char(choice))
    }

    fn press_char(c: char) -> Key {
        press(KeyCode::Char(c))
    }

    fn alt_char(c: char) -> Key {
        Key {
            alt: true,
            code: KeyCode::Char(c),
            ctrl: false,
            shift: false,
        }
    }

    /// Ask for `text` through the search prompt, the way a key does.
    fn search(tool: &mut GitTool, text: &str) -> ToolOutcome {
        let outcome = tool.on_key(&press_char('/'));
        let ToolOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_SEARCH);
        tool.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        })
    }

    fn row_under_cursor(tool: &GitTool) -> String {
        tool.rows
            .get(tool.cursor)
            .map(|row| row_text(&row.spans))
            .unwrap_or_default()
    }

    #[test]
    fn test_a_menu_key_opens_a_menu_and_hands_it_to_the_host_to_draw() {
        // The menu is the host's own key-hint card, centred over the window,
        // so the view keeps every row of the pane to itself.
        let mut tool = loaded_tool();
        tool.on_key(&press_char('z'));
        assert_eq!(tool.popup, Some(Popup::Stash));

        let hint = tool.hint().expect("a hint while the menu is open");
        assert_eq!(hint.title, "Stash");
        assert!(
            hint.items
                .iter()
                .any(|(key, what)| key == "p" && what == "pop the newest"),
            "got {:?}",
            hint.items
        );

        let painted = tool.content(80, 80, false);
        let text: Vec<String> = painted
            .rows
            .iter()
            .map(|row| row.iter().map(|span| span.text.clone()).collect())
            .collect();
        assert!(
            !text.iter().any(|line| line.contains("pop the newest")),
            "and nothing of it is drawn into the view, got {text:?}"
        );
    }

    #[test]
    fn test_an_unknown_second_key_closes_the_menu_without_running_anything() {
        // A mistyped follow key must not fall through to the view's own keys,
        // where it could stage or discard something.
        let mut tool = loaded_tool();
        let outcome = choose(&mut tool, 'z', '@');
        assert_eq!(outcome, ToolOutcome::Consumed);
        assert_eq!(tool.popup, None);
    }

    #[test]
    fn test_stash_choices_run_the_matching_command() {
        let mut tool = loaded_tool();
        assert_eq!(
            command_of(&choose(&mut tool, 'z', 'p')).args,
            ["stash", "pop"]
        );
        assert_eq!(
            command_of(&choose(&mut tool, 'z', 'd')).args,
            ["stash", "drop"]
        );
        assert_eq!(
            command_of(&choose(&mut tool, 'z', 'l')).args,
            ["stash", "list"]
        );
    }

    #[test]
    fn test_a_branch_choice_asks_for_the_name_then_runs() {
        let mut tool = loaded_tool();
        let ToolOutcome::Prompt(request) = choose(&mut tool, 'b', 'c') else {
            panic!("expected a prompt");
        };
        let command = command_of(&tool.on_prompt(PromptReply {
            answer: Some("feature".to_string()),
            tag: request.tag,
        }));
        assert_eq!(command.args, ["checkout", "-b", "feature"]);
    }

    #[test]
    fn test_an_empty_answer_runs_nothing() {
        // `git checkout -b ""` fails with a confusing message; not running is
        // the better answer.
        let mut tool = loaded_tool();
        let ToolOutcome::Prompt(request) = choose(&mut tool, 'b', 'c') else {
            panic!("expected a prompt");
        };
        let outcome = tool.on_prompt(PromptReply {
            answer: Some("   ".to_string()),
            tag: request.tag,
        });
        assert_eq!(outcome, ToolOutcome::Consumed);
    }

    #[test]
    fn test_a_stash_message_may_be_empty_because_it_is_optional() {
        let mut tool = loaded_tool();
        let ToolOutcome::Prompt(request) = choose(&mut tool, 'z', 'z') else {
            panic!("expected a prompt");
        };
        let command = command_of(&tool.on_prompt(PromptReply {
            answer: Some(String::new()),
            tag: request.tag,
        }));
        assert_eq!(command.args, ["stash", "push"], "no empty --message");
    }

    #[test]
    fn test_a_file_changed_on_disk_makes_the_tool_read_the_status_again() {
        let dir = std::env::temp_dir().join(format!(
            "winter-gitpage-watch-{:?}",
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".git")).expect("git dir");
        let mut tool = GitTool::new(dir.clone());
        let root = format!("{}\n{}\n", dir.display(), dir.join(".git").display());
        tool.on_job(reply(exec::TAG_ROOT, &root));
        tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        assert!(tool.tree_watch.is_some(), "the platform can watch it");
        std::thread::sleep(Duration::from_millis(150));

        std::fs::write(dir.join("new.rs"), "fn main() {}").expect("write");
        std::thread::sleep(Duration::from_millis(300));

        let now = Instant::now();
        assert_eq!(
            tool.on_tick(now),
            ToolOutcome::Consumed,
            "waiting out the burst"
        );
        let asked = tool.on_tick(now + Duration::from_secs(4));
        assert_eq!(command_of(&asked).tag, exec::TAG_STATUS);
        assert_eq!(
            tool.on_tick(now + Duration::from_secs(8)),
            ToolOutcome::Consumed,
            "one read at a time"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_quiet_tree_is_read_again_without_redrawing_anything() {
        let mut tool = loaded_tool();
        let now = Instant::now();
        let asked = tool.on_tick(now);
        assert_eq!(command_of(&asked).tag, exec::TAG_STATUS);

        // Nothing changed: no rows rebuilt, no log or stash reads chained on.
        let rows = tool.rows.len();
        assert_eq!(
            tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT)),
            ToolOutcome::Consumed
        );
        assert_eq!(tool.rows.len(), rows);
    }

    #[test]
    fn test_a_change_made_elsewhere_is_picked_up_by_the_watch() {
        let mut tool = loaded_tool();
        tool.on_tick(Instant::now());

        let next = tool.on_job(reply(exec::TAG_STATUS, "# branch.head main\n"));

        assert_eq!(
            command_of(&next).tag,
            exec::TAG_LOG,
            "the refresh chain runs"
        );
        assert!(
            tool.status.files.is_empty(),
            "the new status replaced the old"
        );
    }

    #[test]
    fn test_the_watch_leaves_alone_a_view_the_user_is_reading() {
        let mut tool = loaded_tool();
        assert!(tool.next_tick().is_some());
        tool.show_output("Log", "abc one\n", false);
        assert_eq!(tool.next_tick(), None);
    }

    fn quick_push_key() -> Key {
        Key {
            alt: false,
            code: KeyCode::Char('p'),
            ctrl: true,
            shift: true,
        }
    }

    #[test]
    fn test_ctrl_shift_p_pushes_to_the_tracked_remote_and_keeps_tracking() {
        let mut tool = loaded_tool();
        let command = command_of(&tool.on_key(&quick_push_key()));
        assert_eq!(command.args, ["push", "--set-upstream", "origin", "main"]);
    }

    #[test]
    fn test_ctrl_shift_p_uses_the_remote_the_branch_tracks_not_always_origin() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(
            exec::TAG_STATUS,
            "# branch.head feature/x\n# branch.upstream fork/feature/x\n",
        ));
        let command = command_of(&tool.on_key(&quick_push_key()));
        assert_eq!(
            command.args,
            ["push", "--set-upstream", "fork", "feature/x"]
        );
    }

    #[test]
    fn test_ctrl_shift_p_creates_the_branch_on_origin_when_nothing_is_tracked() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(exec::TAG_STATUS, "# branch.head topic\n"));
        let command = command_of(&tool.on_key(&quick_push_key()));
        assert_eq!(command.args, ["push", "--set-upstream", "origin", "topic"]);
    }

    #[test]
    fn test_ctrl_shift_p_on_a_detached_head_says_so_and_runs_nothing() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(exec::TAG_STATUS, "# branch.head (detached)\n"));
        assert_eq!(tool.on_key(&quick_push_key()), ToolOutcome::Consumed);
        assert_eq!(notice_text(&mut tool).as_deref(), Some("no branch to push"));
    }

    #[test]
    fn test_ctrl_shift_c_with_nothing_staged_says_so_rather_than_doing_nothing() {
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        tool.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        tool.on_job(reply(exec::TAG_STATUS, "# branch.head main\n? new.rs\n"));
        let outcome = tool.on_key(&Key {
            alt: false,
            code: KeyCode::Char('c'),
            ctrl: true,
            shift: true,
        });
        assert_eq!(outcome, ToolOutcome::Consumed);
        assert_eq!(
            notice_text(&mut tool).as_deref(),
            Some("nothing staged to commit")
        );
    }

    #[test]
    fn test_ctrl_shift_c_commits_in_the_editor_when_something_is_staged() {
        let mut tool = loaded_tool();
        let outcome = tool.on_key(&Key {
            alt: false,
            code: KeyCode::Char('c'),
            ctrl: true,
            shift: true,
        });
        let request = command_of(&outcome);
        assert_eq!(request.tag, exec::TAG_COMMIT);
        assert_eq!(request.args, ["commit"]);
    }

    #[test]
    fn test_a_commit_runs_headless_with_winters_editor_and_no_terminal_tab() {
        // The editor is a tab of this window that git waits on, so the commit
        // needs no terminal of its own, and a spawn would leave an empty one
        // sitting beside the message.
        let mut tool = loaded_tool();
        let outcome = choose(&mut tool, 'c', 'c');
        assert!(matches!(outcome, ToolOutcome::Job(_)), "{outcome:?}");
        let request = command_of(&outcome);
        assert_eq!(request.args, ["commit"]);
        assert_eq!(request.cwd, PathBuf::from("/repo"));
        assert!(
            request.env.iter().any(|(name, _)| name == "GIT_EDITOR"),
            "git is pointed at Winter's editor: {:?}",
            request.env
        );
        let amend = command_of(&choose(&mut tool, 'c', 'a'));
        assert_eq!(amend.args, ["commit", "--amend"]);
    }

    #[test]
    fn test_an_interactive_rebase_also_goes_to_a_pane() {
        let mut tool = loaded_tool();
        // The branch to rebase onto is chosen from a list, so the key asks
        // for the names first and the answer arrives under the same tag.
        let listing = choose(&mut tool, 'r', 'i');
        assert_eq!(command_of(&listing).tag, exec::TAG_CANDIDATES);
        let ToolOutcome::Pick(request) =
            tool.on_job(reply(exec::TAG_CANDIDATES, "main\nfeature\n"))
        else {
            panic!("expected a list to pick from");
        };
        let outcome = tool.on_prompt(PromptReply {
            answer: Some("HEAD~3".to_string()),
            tag: request.tag,
        });
        let ToolOutcome::Spawn(spawn) = outcome else {
            panic!("expected a spawn, got {outcome:?}");
        };
        assert_eq!(spawn.args, ["rebase", "--interactive", "HEAD~3"]);
    }

    #[test]
    fn test_a_remote_add_answer_splits_into_name_and_url() {
        let mut tool = loaded_tool();
        let ToolOutcome::Prompt(request) = choose(&mut tool, 'M', 'a') else {
            panic!("expected a prompt");
        };
        let command = command_of(&tool.on_prompt(PromptReply {
            answer: Some("upstream git@example.com:o/r.git".to_string()),
            tag: request.tag,
        }));
        assert_eq!(
            command.args,
            ["remote", "add", "upstream", "git@example.com:o/r.git"]
        );
    }

    #[test]
    fn test_a_malformed_remote_add_reports_instead_of_running() {
        let mut tool = loaded_tool();
        let ToolOutcome::Prompt(request) = choose(&mut tool, 'M', 'a') else {
            panic!("expected a prompt");
        };
        let outcome = tool.on_prompt(PromptReply {
            answer: Some("justaname".to_string()),
            tag: request.tag,
        });
        assert_eq!(outcome, ToolOutcome::Consumed);
        assert!(notice_text(&mut tool)
            .unwrap_or_default()
            .starts_with("failed:"));
    }

    #[test]
    fn test_enter_on_a_commit_reads_it() {
        // Enter opens the file under the cursor, and a commit row has no file:
        // it used to be swallowed, so a commit was the one row Enter did
        // nothing on.
        let mut tool = loaded_tool();
        tool.cursor = tool
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("a commit row");
        let command = command_of(&tool.on_key(&press(KeyCode::Enter)));
        assert_eq!(command.tag, exec::TAG_SHOW);
        assert_eq!(command.args.first().map(String::as_str), Some("show"));
        assert_eq!(command.args.last().map(String::as_str), Some("abc1234"));
        assert!(
            !command.args.iter().any(|arg| arg == "--shortstat"),
            "no line totals: the view counts the commit's own hunks, {:?}",
            command.args
        );
    }

    #[test]
    fn test_a_commit_read_fills_the_view_under_its_own_title() {
        let mut tool = loaded_tool();
        // Joined rather than written as one literal: `git show` output is
        // indentation-sensitive (the subject is the message block's four-space
        // indent), and a continued string literal cannot carry that faithfully.
        let shown = [
            "commit abc1234567890",
            "Author: Someone <a@b.c>",
            "Date:   2026-09-15 12:00:00 +0000",
            "",
            "    do the thing",
            "",
            " src/a.rs | 2 +-",
        ]
        .join("\n");
        assert_eq!(
            tool.on_job(reply(exec::TAG_SHOW, &shown)),
            ToolOutcome::Consumed
        );
        let output = tool
            .output
            .as_ref()
            .expect("the commit replaced the status view");
        assert_eq!(output.title, "Commit abc12345  do the thing");
        assert!(output.lines.iter().any(|line| line.contains("src/a.rs")));
    }

    #[test]
    fn test_a_commit_content_view_bands_its_files() {
        // Enter on a commit shows its content the way the working tree reads:
        // the summary receded, then a band per file and decorated hunks
        // under it — no `diff --git` or `index` lines.
        let shown = [
            "commit abc1234567890",
            "Author: Someone <a@b.c>",
            "Date:   2026-09-15 12:00:00 +0000",
            "",
            "    do the thing",
            "",
            "diff --git a/src/a.rs b/src/a.rs",
            "index 111..222 100644",
            "--- a/src/a.rs",
            "+++ b/src/a.rs",
            "@@ -1,2 +1,2 @@",
            " fn main() {",
            "-    old();",
            "+    new();",
        ]
        .join("\n");
        let mut tool = loaded_tool();
        assert_eq!(
            tool.on_job(reply(exec::TAG_SHOW, &shown)),
            ToolOutcome::Consumed
        );
        // A commit opens shut, so open all of it to read what it paints.
        tool.on_key(&shift(KeyCode::Tab));
        let output = tool
            .output
            .as_ref()
            .expect("the commit replaced the status view");
        assert_eq!(
            output.rows[0].spans,
            vec![Span::new(Style::Header, "commit abc1234567890")]
        );
        let band = output
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("src/a.rs"))
            .expect("the file band");
        assert!(
            band.spans.iter().any(|span| span.style == Style::Section),
            "got {band:?}"
        );
        assert!(
            output
                .rows
                .iter()
                .all(|row| !row_text(&row.spans).contains("diff --git")),
            "the patch's plumbing lines are gone"
        );
        let added = output
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("new();"))
            .expect("the added line");
        assert!(added
            .spans
            .iter()
            .any(|span| span.style == Style::AddedEdit));
        assert_eq!(
            output.lines.len(),
            output.rows.len(),
            "the search text matches the painted rows"
        );
    }

    /// A `git show` of one file with two hunks, for the fold tests.
    const SHOWN_TWO_HUNKS: &str = "commit abc1234567890\n\n    do the thing\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-    old();\n+    new();\n@@ -9,2 +9,2 @@\n-    second();\n+    third();\n";

    /// A commit touching two files, so folding every file at once is visible
    /// as more than folding the one.
    const SHOWN_TWO_FILES: &str = "commit abc1234567890\n\n    do the thing\ndiff --git a/src/a.rs b/src/a.rs\n--- a/src/a.rs\n+++ b/src/a.rs\n@@ -1,2 +1,2 @@\n-    old();\n+    new();\ndiff --git a/src/b.rs b/src/b.rs\n--- a/src/b.rs\n+++ b/src/b.rs\n@@ -1,2 +1,2 @@\n-    second();\n+    third();\n";

    /// Put the cursor on the first row whose text contains `needle`.
    fn output_cursor_on(tool: &mut GitTool, needle: &str) {
        let output = tool.output.as_mut().expect("an output view");
        output.cursor = output
            .lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no output row for {needle}"));
    }

    fn output_has(tool: &GitTool, needle: &str) -> bool {
        tool.output
            .as_ref()
            .is_some_and(|output| output.lines.iter().any(|line| line.contains(needle)))
    }

    #[test]
    fn test_tab_on_a_commits_heading_shuts_every_file_under_it() {
        // The heading is the commit's own section, the way the working tree's
        // sections are: one key takes the whole file list away and brings it
        // back, with the count left saying what is behind it.
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
        assert!(
            output_has(&tool, "src/a.rs"),
            "the files show to begin with"
        );

        output_cursor_on(&mut tool, "Changes");
        assert_eq!(tool.on_key(&press(KeyCode::Tab)), ToolOutcome::Consumed);
        assert!(
            !output_has(&tool, "src/a.rs") && !output_has(&tool, "src/b.rs"),
            "the file list is away"
        );
        assert!(
            output_has(&tool, "Changes (2)"),
            "the heading stays, still counting what it holds"
        );

        tool.on_key(&press(KeyCode::Tab));
        assert!(
            output_has(&tool, "src/a.rs") && output_has(&tool, "src/b.rs"),
            "and comes back"
        );
    }

    #[test]
    fn test_a_commit_opens_shut_and_tab_opens_it_one_layer_at_a_time() {
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        assert!(
            output_has(&tool, "src/a.rs"),
            "the commit opens as the files it touched"
        );
        assert!(
            !output_has(&tool, "@@") && !output_has(&tool, "new();"),
            "with neither their hunks nor their diffs"
        );

        output_cursor_on(&mut tool, "src/a.rs");
        let at = tool.output.as_ref().unwrap().cursor;
        assert_eq!(tool.on_key(&press(KeyCode::Tab)), ToolOutcome::Consumed);
        assert!(
            output_has(&tool, "@@"),
            "the file opens to its hunk headers"
        );
        assert!(
            !output_has(&tool, "new();"),
            "which are themselves still shut"
        );
        assert_eq!(
            tool.output.as_ref().unwrap().cursor,
            at,
            "the cursor stays on the band it acted on"
        );

        output_cursor_on(&mut tool, "@@ -1,2 +1,2 @@");
        tool.on_key(&press(KeyCode::Tab));
        assert!(output_has(&tool, "new();"), "the hunk opens to its body");

        output_cursor_on(&mut tool, "src/a.rs");
        tool.on_key(&press(KeyCode::Tab));
        assert!(
            !output_has(&tool, "@@"),
            "the file shuts all of it away again"
        );
        assert!(
            output_has(&tool, "src/a.rs"),
            "its band stays, so it can be opened again"
        );
    }

    #[test]
    fn test_tab_on_a_commits_hunk_folds_only_that_hunk() {
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        tool.on_key(&shift(KeyCode::Tab));

        output_cursor_on(&mut tool, "@@ -1,2 +1,2 @@");
        tool.on_key(&press(KeyCode::Tab));

        assert!(
            output_has(&tool, "@@ -1,2 +1,2 @@"),
            "a folded hunk keeps its header"
        );
        assert!(!output_has(&tool, "new();"), "but drops its body");
        assert!(output_has(&tool, "third();"), "the other hunk is untouched");
    }

    #[test]
    fn test_tab_on_a_commits_summary_does_nothing() {
        // The summary has nothing to fold, so Tab must not disturb the view.
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        output_cursor_on(&mut tool, "do the thing");
        let before = tool.output.as_ref().unwrap().lines.clone();

        assert_eq!(tool.on_key(&press(KeyCode::Tab)), ToolOutcome::Consumed);
        assert_eq!(
            tool.output.as_ref().unwrap().lines,
            before,
            "the view is unchanged"
        );
    }

    #[test]
    fn test_shift_tab_folds_every_file_of_a_commit_and_opens_them_again() {
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
        assert!(
            !output_has(&tool, "new();") && !output_has(&tool, "third();"),
            "the commit opens shut"
        );

        assert_eq!(tool.on_key(&shift(KeyCode::Tab)), ToolOutcome::Consumed);
        assert!(
            output_has(&tool, "new();") && output_has(&tool, "third();"),
            "one key opens every file and hunk of it"
        );

        tool.on_key(&shift(KeyCode::Tab));
        assert!(
            !output_has(&tool, "new();") && !output_has(&tool, "third();"),
            "a second Shift-Tab shuts all of it again"
        );
        assert!(
            output_has(&tool, "src/a.rs") && output_has(&tool, "src/b.rs"),
            "the bands stay, so the commit still says what it touched"
        );
    }

    #[test]
    fn test_shift_tab_opens_a_part_way_folded_commit_rather_than_shutting_it() {
        // One hunk folded by hand means the view is already part-way shut, so
        // the key that acts on all of it opens it rather than folding further.
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        tool.on_key(&shift(KeyCode::Tab));
        output_cursor_on(&mut tool, "@@ -1,2 +1,2 @@");
        tool.on_key(&press(KeyCode::Tab));
        assert!(!output_has(&tool, "new();"), "that hunk's body is away");

        tool.on_key(&shift(KeyCode::Tab));
        assert!(
            output_has(&tool, "new();") && output_has(&tool, "third();"),
            "every hunk is back"
        );
    }

    #[test]
    fn test_shift_tab_leaves_an_output_view_with_nothing_to_fold_alone() {
        // A blame, a log, or a listing is the lines it came back as: there is
        // no commit under it to fold, and the key must not disturb the view.
        let mut tool = loaded_tool();
        tool.on_job(reply(
            exec::TAG_BLAME,
            "abc1234 (Someone 2026-09-18) fn main() {",
        ));
        let before = tool.output.as_ref().expect("the blame").lines.clone();

        assert_eq!(tool.on_key(&shift(KeyCode::Tab)), ToolOutcome::Consumed);
        assert_eq!(tool.output.as_ref().unwrap().lines, before);
    }

    #[test]
    fn test_a_commits_file_band_carries_an_icon() {
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        let output = tool.output.as_ref().expect("the commit view");
        let band = output
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("src/a.rs"))
            .expect("the file band");
        assert_eq!(
            band.icon.as_ref().map(|icon| &icon.kind),
            Some(&crate::model::paint::IconKind::File {
                name: "a.rs".to_string()
            }),
            "the band names its icon for the file's leaf"
        );
        assert!(
            output.rows.iter().filter(|row| row.icon.is_some()).count() == 1,
            "only the file band carries one; hunk lines do not"
        );
    }

    #[test]
    fn test_folding_reaches_the_painted_content_the_pane_draws() {
        // The fold has to change what `content` returns, not just the row list,
        // or the pane keeps drawing the unfolded view.
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        tool.on_key(&shift(KeyCode::Tab));
        let drawn = |tool: &mut GitTool| -> String {
            tool.content(40, 100, false)
                .rows
                .iter()
                .map(row_text)
                .collect::<Vec<String>>()
                .join("\n")
        };
        assert!(drawn(&mut tool).contains("new();"));

        output_cursor_on(&mut tool, "src/a.rs");
        tool.on_key(&press(KeyCode::Tab));
        let after = drawn(&mut tool);
        assert!(
            !after.contains("new();"),
            "the pane draws the folded view, got {after:?}"
        );
        assert!(after.contains("src/a.rs"), "the band is still drawn");
    }

    #[test]
    fn test_an_empty_commit_read_says_so_instead_of_blanking_the_view() {
        let mut tool = loaded_tool();
        tool.on_job(reply(
            exec::TAG_SHOW,
            "   
",
        ));
        assert!(tool.output.is_none(), "the status view stays");
        assert_eq!(notice_text(&mut tool).as_deref(), Some("nothing to show"));
    }

    #[test]
    fn test_reset_on_a_commit_uses_that_commit_without_asking() {
        let mut tool = loaded_tool();
        tool.cursor = tool
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("a commit row");
        let command = command_of(&tool.on_key(&press_char('o')));
        assert_eq!(command.args, ["reset", "--mixed", "abc1234"]);
    }

    #[test]
    fn test_reset_away_from_a_commit_asks_which_one() {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let ToolOutcome::Prompt(request) = tool.on_key(&press_char('o')) else {
            panic!("expected a prompt");
        };
        assert_eq!(request.tag, ASK_RESET_MIXED);
    }

    #[test]
    fn test_a_hard_reset_says_what_it_costs() {
        let mut tool = loaded_tool();
        let ToolOutcome::Prompt(request) = choose(&mut tool, 'O', 'h') else {
            panic!("expected a prompt");
        };
        assert!(request.label.contains("HARD"), "got {:?}", request.label);
        assert!(
            request.label.to_lowercase().contains("losing"),
            "got {:?}",
            request.label
        );
    }

    #[test]
    fn test_yanking_copies_the_hash_on_a_commit_and_the_path_on_a_file() {
        let mut tool = loaded_tool();
        tool.cursor = tool
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("a commit row");
        assert_eq!(
            tool.on_key(&press_char('y')),
            ToolOutcome::Yank("abc1234".to_string())
        );

        cursor_on(&mut tool, "working.rs");
        assert_eq!(
            tool.on_key(&press_char('y')),
            ToolOutcome::Yank("working.rs".to_string())
        );
    }

    #[test]
    fn test_ignoring_by_extension_writes_a_wildcard() {
        let tree = std::env::temp_dir().join(format!("winter-gitignore-{}", std::process::id()));
        std::fs::create_dir_all(&tree).expect("temp dir");
        let mut tool = GitTool::new(tree.clone());
        tool.on_job(reply(exec::TAG_ROOT, &format!("{}\n", tree.display())));
        tool.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        cursor_on(&mut tool, "working.rs");

        choose(&mut tool, 'i', 'e');
        let written = std::fs::read_to_string(tree.join(".gitignore")).unwrap_or_default();
        assert_eq!(written.trim(), "*.rs");
        std::fs::remove_dir_all(&tree).ok();
    }

    #[test]
    fn test_a_log_fills_the_view_and_can_ask_for_more() {
        let mut tool = loaded_tool();
        choose(&mut tool, 'l', 'l');
        let outcome = tool.on_job(reply(
            exec::TAG_LOG_VIEW,
            &log_output(&[("aaa", "one"), ("bbb", "two")]),
        ));
        assert_eq!(outcome, ToolOutcome::Consumed);
        assert!(tool.output.is_some(), "the log replaced the status view");

        let before = tool.log_count;
        let more = tool.on_key(&press_char('+'));
        assert!(tool.log_count > before, "asked for more");
        assert_eq!(command_of(&more).tag, exec::TAG_LOG_VIEW);
    }

    #[test]
    fn test_q_leaves_the_log_for_the_status_view_rather_than_closing() {
        // Closing the whole tool on `q` from a log would lose the view the user
        // came from.
        let mut tool = loaded_tool();
        choose(&mut tool, 'l', 'l');
        tool.on_job(reply(exec::TAG_LOG_VIEW, &log_output(&[("aaa", "one")])));
        let outcome = tool.on_key(&press_char('q'));
        assert_eq!(outcome, ToolOutcome::Consumed);
        assert!(tool.output.is_none());
        assert_ne!(tool.on_key(&press(KeyCode::Escape)), ToolOutcome::Close);
    }

    #[test]
    fn test_read_only_output_with_nothing_in_it_says_so() {
        let mut tool = loaded_tool();
        tool.on_job(reply(exec::TAG_READ, "\n"));
        assert!(tool.output.is_none());
        assert_eq!(notice_text(&mut tool).as_deref(), Some("nothing to show"));
    }

    #[test]
    fn test_an_ssh_remote_becomes_an_https_url() {
        assert_eq!(
            browser_url("git@github.com:owner/repo.git").as_deref(),
            Some("https://github.com/owner/repo")
        );
        assert_eq!(
            browser_url("https://gitlab.com/owner/repo.git").as_deref(),
            Some("https://gitlab.com/owner/repo")
        );
        assert_eq!(browser_url("/srv/git/bare.git"), None);
    }

    #[test]
    fn test_the_keymaps_apply_keys_act_on_a_hunk_only() {
        // `a` and `-` are hunk keys; on a file row there is no patch to apply,
        // and guessing one would stage the whole file by surprise.
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        assert_eq!(tool.on_key(&press_char('a')), ToolOutcome::Consumed);
        assert_eq!(tool.on_key(&press_char('-')), ToolOutcome::Consumed);
    }

    #[test]
    fn test_shift_tab_still_folds_while_a_menu_has_never_opened() {
        let mut tool = loaded_tool();
        tool.on_key(&shift(KeyCode::Tab));
        assert!(!tool
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
    }

    #[test]
    fn test_a_menu_shows_however_long_the_view_is() {
        // The card is drawn over the window rather than under the rows, so a
        // status longer than the pane cannot push it off the bottom, which is
        // where a menu drawn into the view ended up.
        let mut tool = loaded_tool();
        tool.on_key(&press_char('b'));
        let rows = 4;
        let painted = tool.content(rows, 80, false);

        assert!(tool.hint().is_some(), "the menu is there to draw");
        assert!(
            painted.rows.len() <= rows,
            "and the view still fits its pane, got {}",
            painted.rows.len()
        );
    }

    #[test]
    fn test_the_jump_leader_says_where_it_can_go() {
        // `g` waits for a second key like every other prefix, so it says what
        // the second key may be the same way they do.
        let mut tool = loaded_tool();
        assert_eq!(tool.on_key(&press_char('g')), ToolOutcome::Consumed);
        let hint = tool.hint().expect("a hint while the leader is open");
        assert_eq!(hint.title, "Jump");
        assert!(
            hint.items
                .iter()
                .any(|(key, what)| key == "s" && what == "staged changes"),
            "got {:?}",
            hint.items
        );

        // And the second key still jumps.
        tool.on_key(&press_char('s'));
        assert_eq!(
            tool.selected().map(|row| row.item.clone()),
            Some(Item::Heading(Section::Staged))
        );
    }

    #[test]
    fn test_switching_branch_offers_the_branches_rather_than_a_blank_line() {
        let mut tool = loaded_tool();
        let listing = choose(&mut tool, 'b', 'b');
        let command = command_of(&listing);
        assert_eq!(command.tag, exec::TAG_CANDIDATES);
        assert_eq!(command.args[0], "for-each-ref");
        assert!(
            command.args.iter().any(|arg| arg == "refs/remotes"),
            "a checkout takes a remote branch too, got {:?}",
            command.args
        );

        let outcome = tool.on_job(reply(
            exec::TAG_CANDIDATES,
            "main\nfeature\norigin/main\norigin/HEAD\n",
        ));
        let ToolOutcome::Pick(request) = outcome else {
            panic!("expected a list to pick from, got {outcome:?}");
        };
        assert_eq!(request.label, "Checkout");
        assert_eq!(request.items, ["main", "feature", "origin/main"]);

        // Choosing one answers the question the key asked.
        let ran = tool.on_prompt(PromptReply {
            answer: Some("feature".to_string()),
            tag: request.tag,
        });
        let command = command_of(&ran);
        assert_eq!(command.args, ["checkout", "feature"]);
    }

    #[test]
    fn test_deleting_a_branch_offers_only_the_ones_that_can_be_deleted() {
        // A branch on a remote is not this repository's to delete, so the
        // list stops at the local ones.
        let mut tool = loaded_tool();
        let command = command_of(&choose(&mut tool, 'b', 'd'));
        assert!(
            command.args.iter().any(|arg| arg == "refs/heads")
                && !command.args.iter().any(|arg| arg == "refs/remotes"),
            "got {:?}",
            command.args
        );
    }

    #[test]
    fn test_a_question_with_nothing_to_choose_from_falls_back_to_typing() {
        // A repository with no tags still has a delete-tag key; it asks for
        // one to be spelled out rather than opening an empty list.
        let mut tool = loaded_tool();
        choose(&mut tool, 't', 'd');
        let outcome = tool.on_job(reply(exec::TAG_CANDIDATES, "\n"));
        let ToolOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.label, "Delete tag: ");
    }

    #[test]
    fn test_the_file_popup_acts_on_the_row_it_was_opened_over() {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        assert_eq!(tool.on_key(&press_char('.')), ToolOutcome::Consumed);
        assert_eq!(tool.popup, Some(Popup::File), "the menu is open");

        let command = command_of(&tool.on_key(&press_char('b')));
        assert_eq!(command.args[0], "blame");
        assert!(
            command.args.iter().any(|arg| arg == "working.rs"),
            "on the file the cursor was on, got {:?}",
            command.args
        );
    }

    #[test]
    fn test_the_file_popup_stays_shut_where_there_is_no_file() {
        // On a heading or a commit there is nothing for it to act on, and a
        // menu whose every choice does nothing is worse than none.
        let mut tool = loaded_tool();
        tool.cursor = 0;
        assert_eq!(tool.on_key(&press_char('.')), ToolOutcome::Consumed);
        assert_eq!(tool.popup, None);
        assert_eq!(notice_text(&mut tool).as_deref(), Some("no file here"));
    }

    #[test]
    fn test_enter_on_a_stash_reads_it_as_a_diff() {
        let mut tool = loaded_tool();
        tool.on_job(reply(
            exec::TAG_STASHES,
            "stash@{0}\u{1f}WIP on main: a thing\n",
        ));
        let at = tool
            .rows
            .iter()
            .position(|row| row.item == Item::Stash(0))
            .expect("the stash row");
        tool.cursor = at;

        let command = command_of(&tool.on_key(&press(KeyCode::Enter)));
        assert_eq!(command.args[0], "stash");
        assert!(
            command.args.iter().any(|arg| arg == "stash@{0}"),
            "the stash under the cursor, got {:?}",
            command.args
        );
    }

    #[test]
    fn test_the_emacs_buffer_ends_reach_both_ends_of_either_view() {
        // `M->` and `M-<` land on the last and first row, in the status view
        // and in an output view alike: both are read top to bottom, and the
        // Vim `G`/`gg` pair is otherwise the only way there.
        let mut tool = loaded_tool();
        tool.cursor = 0;
        tool.on_key(&alt_char('>'));
        assert_eq!(tool.cursor, tool.rows.len() - 1, "the last row");
        tool.on_key(&alt_char('<'));
        assert_eq!(tool.cursor, 0, "and back to the first");

        tool.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
        tool.on_key(&alt_char('>'));
        let output = tool.output.as_ref().expect("the commit view");
        assert_eq!(output.cursor, output.lines.len() - 1);
        tool.on_key(&alt_char('<'));
        assert_eq!(tool.output.as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn test_the_shared_word_motions_step_between_entities() {
        // `w`/`b` land on the things the view rows stand for, stepping over
        // the blank separators, through the shared Vim layer.
        let mut tool = loaded_tool();
        tool.cursor = 0;
        tool.on_key(&press(KeyCode::Char('w')));
        assert!(
            tool.selected().is_some_and(|row| row.item != Item::None),
            "never lands on a separator"
        );
        tool.on_key(&press(KeyCode::Char('b')));
        assert!(
            tool.selected().is_some_and(|row| row.item != Item::None),
            "and back, the same way"
        );
    }

    #[test]
    fn test_the_braces_jump_between_section_headings() {
        let mut tool = loaded_tool();
        tool.on_key(&press(KeyCode::Char('}')));
        assert!(matches!(
            tool.selected().map(|row| &row.item),
            Some(Item::Heading(_) | Item::RecentHeading)
        ));
        let first = tool.cursor;
        tool.on_key(&press(KeyCode::Char('}')));
        assert!(tool.cursor > first, "each brace reaches the next heading");
        tool.on_key(&press(KeyCode::Char('{')));
        assert_eq!(tool.cursor, first, "and back to it");
    }

    #[test]
    fn test_gg_and_the_paging_motions_work_over_the_view() {
        let mut tool = loaded_tool();
        tool.on_key(&press(KeyCode::Char('g')));
        tool.on_key(&press(KeyCode::Char('g')));
        assert_eq!(tool.cursor, 0, "gg through the view's own g leader");

        tool.on_key(&press(KeyCode::Char('$')));
        assert_eq!(tool.cursor, tool.rows.len() - 1, "$ is the last row");

        tool.on_key(&press(KeyCode::Char('0')));
        assert_eq!(tool.cursor, 0);
        tool.content(10, 80, false);
        tool.on_key(&ctrl(KeyCode::Char('d')));
        assert_eq!(tool.cursor, 4, "half the viewport down");
        tool.on_key(&ctrl(KeyCode::Char('u')));
        assert_eq!(tool.cursor, 0, "and back up");
    }

    #[test]
    fn test_the_views_own_keys_still_win_over_the_shared_layer() {
        // The override contract: what the view claims never reaches the
        // shared layer. `G` refreshes rather than going to the bottom, and
        // `g` opens the section leader rather than arming `gg` there.
        let mut tool = loaded_tool();
        tool.on_key(&press(KeyCode::Char('$')));
        let before = tool.rows.len();
        tool.on_key(&press(KeyCode::Char('G')));
        assert!(tool.rows.len() >= before, "`G` refreshed, it did not move");
    }

    #[test]
    fn test_the_g_keys_jump_between_sections() {
        let mut tool = loaded_tool();
        tool.on_key(&press(KeyCode::Char('g')));
        tool.on_key(&press(KeyCode::Char('s')));
        assert_eq!(
            tool.selected().map(|row| &row.item),
            Some(&Item::Heading(Section::Staged))
        );

        tool.on_key(&press(KeyCode::Char('g')));
        tool.on_key(&press(KeyCode::Char('t')));
        assert_eq!(
            tool.selected().map(|row| &row.item),
            Some(&Item::Heading(Section::Untracked))
        );
    }

    #[test]
    fn test_entity_motion_skips_the_blank_lines_between_sections() {
        // Plain `j` walks through the gaps; `Alt-n` is what steps from one
        // thing to the next thing.
        let mut tool = loaded_tool();
        tool.cursor = 0;
        tool.on_key(&Key {
            alt: true,
            code: KeyCode::Char('n'),
            ctrl: false,
            shift: false,
        });
        assert_ne!(tool.selected().map(|row| &row.item), Some(&Item::None));
    }

    #[test]
    fn test_folding_a_section_hides_its_files_but_not_the_others() {
        let mut tool = loaded_tool();
        tool.cursor = tool
            .rows
            .iter()
            .position(|row| row.item == Item::Heading(Section::Untracked))
            .expect("the untracked heading");
        tool.on_key(&press(KeyCode::Tab));

        let listed: Vec<String> = tool
            .rows
            .iter()
            .filter_map(|row| match &row.item {
                Item::File(file) => Some(file.path.clone()),
                _ => None,
            })
            .collect();
        assert!(!listed.contains(&"new.rs".to_string()), "got {listed:?}");
        assert!(listed.contains(&"working.rs".to_string()), "got {listed:?}");
    }

    #[test]
    fn test_shift_tab_shuts_everything_then_opens_it_again() {
        let mut tool = loaded_tool();
        let shift_tab = Key {
            alt: false,
            code: KeyCode::Tab,
            ctrl: false,
            shift: true,
        };
        tool.on_key(&shift_tab);
        assert!(!tool
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
        tool.on_key(&shift_tab);
        assert!(tool
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
    }

    #[test]
    fn test_a_push_runs_from_the_repository_root() {
        let mut tool = loaded_tool();
        let command = command_of(&tool.on_key(&press(KeyCode::Char('P'))));
        assert_eq!(command.args, ["push"]);
        assert_eq!(command.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn test_a_search_moves_the_cursor_to_the_row_holding_the_text() {
        let mut tool = loaded_tool();
        search(&mut tool, "working");
        assert!(row_under_cursor(&tool).contains("working.rs"));
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let mut tool = loaded_tool();
        search(&mut tool, ".rs");
        let first = row_under_cursor(&tool);

        tool.on_key(&press_char('n'));
        let second = row_under_cursor(&tool);
        assert_ne!(first, second, "the next match is a different row");

        tool.on_key(&press_char('N'));
        assert_eq!(row_under_cursor(&tool), first, "and back again");
    }

    #[test]
    fn test_a_search_walks_the_output_view_while_one_is_up() {
        // The status rows are not on screen here, so searching them would move a
        // cursor nobody can see and leave the visible one where it was.
        let mut tool = loaded_tool();
        choose(&mut tool, 'l', 'l');
        tool.on_job(reply(
            exec::TAG_LOG_VIEW,
            &log_output(&[("aaa", "one"), ("bbb", "two"), ("ccc", "three")]),
        ));
        let before = tool.cursor;

        search(&mut tool, "ccc");
        // Located rather than hardcoded: the log view heads its list, and each
        // commit takes an identity row and an authorship row, so a literal index
        // here would only re-encode the current layout.
        let expected = tool
            .output
            .as_ref()
            .and_then(|output| output.lines.iter().position(|line| line.contains("ccc")));
        assert!(expected.is_some(), "the log lists the commit searched for");
        assert_eq!(tool.output.as_ref().map(|output| output.cursor), expected);
        assert_eq!(tool.cursor, before);
    }

    #[test]
    fn test_a_search_that_finds_nothing_reports_it_instead_of_going_quiet() {
        let mut tool = loaded_tool();
        tool.on_key(&press_char('n'));
        assert_eq!(tool.take_notice(), Some(ToolNotice::error("no search")));

        search(&mut tool, "absent");
        assert_eq!(
            tool.take_notice(),
            Some(ToolNotice::error("not found: absent"))
        );
    }

    #[test]
    fn test_a_search_is_answered_before_git_has_said_where_the_root_is() {
        // Every other question needs a repository and is dropped without one,
        // which would swallow a search the user typed while the status loaded.
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        search(&mut tool, "anything");
        assert_eq!(
            tool.take_notice(),
            Some(ToolNotice::error("not found: anything"))
        );
    }

    #[test]
    fn test_blame_runs_against_the_file_under_the_cursor() {
        let mut tool = loaded_tool();
        cursor_on(&mut tool, "working.rs");
        let command = command_of(&tool.on_key(&alt_char('b')));
        assert_eq!(command.tag, exec::TAG_BLAME);
        assert_eq!(command.args.last().map(String::as_str), Some("working.rs"));
        assert_eq!(command.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn test_blame_off_a_file_says_there_is_nothing_to_blame() {
        // A heading and a commit have no single path, and running blame on the
        // repository root would report a git error the user did not ask for.
        let mut tool = loaded_tool();
        tool.cursor = 0;
        assert_eq!(tool.on_key(&alt_char('b')), ToolOutcome::Consumed);
        assert_eq!(
            notice_text(&mut tool).as_deref(),
            Some("no file to blame here")
        );
    }

    #[test]
    fn test_a_blame_fills_the_view_under_its_own_title() {
        let mut tool = loaded_tool();
        tool.on_job(reply(
            exec::TAG_BLAME,
            "abc12345 (me 2026-01-01 1) fn main\n",
        ));
        assert_eq!(
            tool.output.as_ref().map(|output| output.title.as_str()),
            Some("Blame")
        );
    }

    #[test]
    fn test_keys_before_the_root_is_known_do_nothing() {
        // The view is up as soon as the key is pressed, so every command has to
        // survive being asked before git has answered.
        let mut tool = GitTool::new(PathBuf::from("/repo"));
        for code in [
            KeyCode::Char('s'),
            KeyCode::Char('u'),
            KeyCode::Char('P'),
            KeyCode::Char('F'),
            KeyCode::Char('f'),
        ] {
            assert_eq!(tool.on_key(&press(code)), ToolOutcome::Consumed);
        }
    }

    #[test]
    fn test_git_jump_status_rows_and_output_lines() {
        let mut tool = loaded_tool();
        let targets = tool.jump_targets().expect("status jump targets");
        assert!(!targets.is_empty());
        assert_eq!(tool.jump_cursor(), Some(0));

        // Jump preview on status view
        tool.viewport = 10;
        tool.jump_to(1);
        assert_eq!(tool.cursor, 1);
        assert_eq!(tool.scroll, 1_usize.saturating_sub(10 / 2));
        tool.jump_cancel();
        assert_eq!(tool.cursor, 0);
        assert_eq!(tool.scroll, 0);

        // Jump preview and confirm on status view
        tool.jump_to(1);
        tool.jump_confirm(1);
        assert_eq!(tool.cursor, 1);
        assert_eq!(tool.scroll, 1_usize.saturating_sub(10 / 2));
        assert!(tool.jump_origin.is_none());

        // Now with output (e.g. blame)
        tool.on_job(reply(exec::TAG_BLAME, "line 1\nline 2\nline 3\n"));
        let output_targets = tool.jump_targets().expect("output jump targets");
        assert_eq!(output_targets.len(), 3);
        assert_eq!(tool.jump_cursor(), Some(0));

        tool.jump_to(2);
        assert_eq!(tool.output.as_ref().unwrap().cursor, 2);
        tool.jump_cancel();
        assert_eq!(tool.output.as_ref().unwrap().cursor, 0);

        tool.jump_to(2);
        tool.jump_confirm(2);
        assert_eq!(tool.output.as_ref().unwrap().cursor, 2);
    }
}

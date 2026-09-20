//! Git: the working tree's state in a pane, and the keys that change it.
//!
//! - [`diff`]: reading a unified diff, and writing one hunk back out.
//! - [`exec`]: the git command lines the view runs.
//! - [`parse`]: reading git's own output.
//! - [`popup`]: the transient menus a key opens.
//! - [`rows`]: painting the view, and what each row stands for.
//! - [`words`]: the word-level diff that decorates a hunk's changed lines.

pub mod commit;
pub mod diff;
pub mod exec;
pub mod parse;
pub mod popup;
pub mod progress;
pub mod rows;
pub mod words;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use crate::model::input::CursorMove;
use crate::model::input::{Key, KeyCode};
use crate::model::page::{
    find_match, row_height, row_text, wrap_window, CommandOutput, JobReply, JobRequest, OpenTarget,
    Page, PageContent, PageHint, PageIcon, PageMenuItem, PageOutcome, PageSpan, PageStyle,
    PageWindow, PickQuestion, PromptMode, PromptReply, PromptRequest, SpawnRequest,
};
use crate::model::vim::nav::{buffer_end, VimKey, VimNav};

use commit::{CommitContent, CommitFile};
use diff::{FileDiff, Hunk};
use exec::{LogScope, ResetMode, SequenceStep};
use parse::{Commit, Section, Stash, Status};
use popup::Popup;
use progress::Progress;
use rows::{Block, CommitFolds, FileRow, Item, ViewRow};

// ========================================================================
// Constants
// ========================================================================

/// Prompt tags, one per question the view asks.
/// Characters of a commit hash shown in a view's title: enough to identify it
/// at a glance without crowding out the subject beside it.
const COMMIT_TITLE_HASH_LEN: usize = 8;

const ASK_BRANCH_CHECKOUT: &str = "branch-checkout";
const ASK_BRANCH_CREATE: &str = "branch-create";
const ASK_BRANCH_CREATE_HERE: &str = "branch-create-here";
const ASK_BRANCH_DELETE: &str = "branch-delete";
const ASK_BRANCH_DELETE_FORCE: &str = "branch-delete-force";
const ASK_CHERRY_PICK: &str = "cherry-pick";
const ASK_COMMIT: &str = "commit";
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

/// The remote a browser is pointed at, and whose refs are pruned by default.
const DEFAULT_REMOTE: &str = "origin";

/// Shown while the first status has not come back yet.
const LOADING: &str = "reading the repository...";

/// Shown when the directory the view opened in is not in a repository.
const NOT_A_REPO: &str = "not a git repository";

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
pub struct GitPage {
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
    cursor: usize,
    /// What the last command reported, shown in the header.
    message: Option<String>,
    /// Where the repository is rooted, once git has said.
    root: Option<PathBuf>,
    rows: Vec<ViewRow>,
    scroll: usize,
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
    /// The path a blame was asked for, held until the answer comes back,
    /// since the reply says only that it is a blame and not what of.
    pending_blame: Option<String>,
    /// Where the page was opened, which is where the root is looked up from.
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
    /// A row just opened whose contents the next paint should bring into
    /// view, if they do not already fit under it. Held until the paint, since
    /// what fits is only known once the pane's width and height are.
    reveal: Option<usize>,
    /// The question a list of names is being gathered for.
    pending_pick: Option<PickQuestion>,
}

// ========================================================================
// GitPage
// ========================================================================

impl GitPage {
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
            cursor: 0,
            loaded: false,
            message: None,
            log_count: LOG_PAGE,
            output: None,
            pending_diff: None,
            popup: None,
            root: None,
            rows: Vec::new(),
            scroll: 0,
            search: String::new(),
            start,
            status: Status::default(),
            viewport: 0,
            nav: VimNav::new(),
            reveal: None,
            pending_pick: None,
        }
    }

    /// The first request the view makes: where the repository is.
    pub fn initial_request(&self) -> JobRequest {
        exec::repo_root(&self.start)
    }

    /// Re-read the working tree.
    fn refresh(&self) -> PageOutcome {
        match &self.root {
            Some(root) => PageOutcome::Job(exec::status(root)),
            None => PageOutcome::Job(self.initial_request()),
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
                unpulled: &self.unpulled,
                stashes: &self.stashes,
                progress: self.progress.as_ref(),
                root: self.root.as_deref(),
                message: self.message.as_deref(),
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
    fn search_for(&mut self, query: &str) {
        self.search = query.to_string();
        self.search_step(true);
    }

    /// Move to the next row matching the last search, in whichever view is up:
    /// one query serves the status and the output it opens.
    fn search_step(&mut self, forward: bool) {
        if self.search.is_empty() {
            self.message = Some("no search".to_string());
            self.rebuild();
            return;
        }
        if let Some(output) = self.output.as_mut() {
            if let Some(index) = find_match(&output.lines, &self.search, output.cursor, forward) {
                output.cursor = index;
                return;
            }
        } else {
            let labels: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
            if let Some(index) = find_match(&labels, &self.search, self.cursor, forward) {
                self.cursor = index;
                return;
            }
        }
        self.message = Some(format!("not found: {}", self.search));
        self.rebuild();
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
    fn toggle_fold(&mut self) -> PageOutcome {
        let item = self.selected().map(|row| row.item.clone());
        if let Some(block) = item.as_ref().and_then(rows::fold_block) {
            self.toggle_block(block);
            return PageOutcome::Consumed;
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
                PageOutcome::Consumed
            }
            // The headings resolved above, and a commit view folds through
            // its own handler over its own fold sets.
            _ => PageOutcome::Consumed,
        }
    }

    /// The row a file's band sits on, if the view is showing one for it.
    fn row_of_file(&self, file: &FileRow) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| matches!(&row.item, Item::File(row_file) if row_file == file))
    }

    fn toggle_block(&mut self, block: Block) {
        if self.collapsed.remove(&block) {
            self.reveal = Some(self.cursor);
        } else {
            self.collapsed.insert(block);
        }
        self.rebuild();
    }

    /// Show or hide one file's diff, reading it the first time it is asked for.
    fn toggle_file(&mut self, file: FileRow) -> PageOutcome {
        if self.expanded.remove(&file) {
            self.rebuild();
            return PageOutcome::Consumed;
        }
        self.expanded.insert(file.clone());
        self.reveal = Some(self.cursor);
        if self.diffs.contains_key(&file) {
            self.rebuild();
            return PageOutcome::Consumed;
        }
        self.pending_diff = Some(file.clone());
        match &self.root {
            Some(root) => PageOutcome::Job(diff_request(root, &file)),
            None => PageOutcome::Consumed,
        }
    }

    /// Stage, unstage, or discard the hunk under the cursor by applying its own
    /// patch, which leaves the rest of the file untouched.
    fn apply_hunk(&self, target: exec::ApplyTarget) -> Option<PageOutcome> {
        let Some(Item::Hunk(hunk)) = self.selected().map(|row| row.item.clone()) else {
            return None;
        };
        let root = self.root.as_ref()?;
        let file = FileRow {
            path: hunk.path.clone(),
            section: hunk.section,
        };
        let patch = self.diffs.get(&file)?.patch_for(hunk.index)?;
        Some(PageOutcome::Job(exec::apply_patch(root, patch, target)))
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
    fn stage(&self, all: bool) -> PageOutcome {
        if !all {
            if let Some(outcome) = self.apply_hunk(exec::ApplyTarget::Index) {
                return outcome;
            }
        }
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return PageOutcome::Consumed;
        }
        PageOutcome::Job(exec::stage(root, &paths))
    }

    /// Stage everything, the way Magic's stage-all does: every untracked file
    /// when the cursor is in that section, every tracked change anywhere
    /// else. Splitting the two halves keeps an unasked-for file from being
    /// swept into the index unread.
    fn stage_all(&self) -> PageOutcome {
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        if self.section_at_cursor() == Some(Section::Untracked) {
            let paths = self.paths_in(Section::Untracked);
            if paths.is_empty() {
                return PageOutcome::Consumed;
            }
            return PageOutcome::Job(exec::stage(root, &paths));
        }
        PageOutcome::Job(exec::stage_all_tracked(root))
    }

    fn unstage(&self, all: bool) -> PageOutcome {
        if !all {
            if let Some(outcome) = self.apply_hunk(exec::ApplyTarget::IndexReverse) {
                return outcome;
            }
        }
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        let paths = if all { Vec::new() } else { self.targets() };
        if !all && paths.is_empty() {
            return PageOutcome::Consumed;
        }
        PageOutcome::Job(exec::unstage(root, &paths))
    }

    /// Ask before throwing away work, naming what will go: nothing recovers a
    /// discarded working-tree change.
    fn ask_discard(&self) -> PageOutcome {
        let paths = self.targets();
        let label = match paths.len() {
            0 => return PageOutcome::Consumed,
            1 => format!("Discard changes to {}? (y/n) ", paths[0]),
            count => format!("Discard changes to {count} files? (y/n) "),
        };
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag: ASK_DISCARD,
        })
    }

    fn discard(&self) -> PageOutcome {
        if let Some(outcome) = self.apply_hunk(exec::ApplyTarget::WorktreeReverse) {
            return outcome;
        }
        let Some(root) = &self.root else {
            return PageOutcome::Consumed;
        };
        let paths = self.targets();
        if paths.is_empty() {
            return PageOutcome::Consumed;
        }
        // Untracked files are not in the index, so `restore` cannot reach
        // them: deleting is what discarding means for those.
        if self.section_at_cursor() == Some(Section::Untracked) {
            return PageOutcome::Job(exec::remove_untracked(root, &paths));
        }
        PageOutcome::Job(exec::discard(root, &paths))
    }

    /// Ask for a one-line message. The long-form editor is the `c` key.
    fn ask_commit_message(&self) -> PageOutcome {
        if self.paths_in(Section::Staged).is_empty() {
            return PageOutcome::Consumed;
        }
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "Commit message: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_COMMIT,
        })
    }

    /// Ask for the next open diff that has not been read since the last
    /// status, one at a time so a view of twenty open files does not run twenty
    /// commands at once.
    fn request_stale_diff(&mut self) -> PageOutcome {
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
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
                PageOutcome::Job(diff_request(&root, &file))
            }
            None => PageOutcome::Consumed,
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
    fn open_popup(&mut self, popup: Popup) -> PageOutcome {
        self.popup = Some(popup);
        PageOutcome::Consumed
    }

    /// The keymap's `a` and `-`: apply or reverse the hunk under the cursor,
    /// doing nothing when the cursor is not on one.
    fn apply_at_point(&self, target: exec::ApplyTarget) -> PageOutcome {
        self.apply_hunk(target).unwrap_or(PageOutcome::Consumed)
    }

    /// Reset to the commit under the cursor, which is the only revision the
    /// view can name without asking.
    fn reset_at_point(&mut self, mode: ResetMode) -> PageOutcome {
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
        };
        match self.revision_at_cursor() {
            Some(rev) => PageOutcome::Job(exec::reset(&root, mode, &rev)),
            None => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
        }
    }

    /// Copy what the cursor is on: a commit hash, or a path.
    fn yank_at_point(&mut self) -> PageOutcome {
        let value = self.revision_at_cursor().or_else(|| self.path_at_cursor());
        match value {
            Some(value) => PageOutcome::Yank(value),
            None => PageOutcome::Consumed,
        }
    }

    /// Blame the file the cursor is on, or on one of its hunks: a heading or a
    /// commit has no single path to run against.
    fn blame_at_point(&mut self) -> PageOutcome {
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
        };
        match self.selected().map(|row| row.item.clone()) {
            Some(Item::File(file)) => {
                self.pending_blame = Some(file.path.clone());
                PageOutcome::Job(exec::blame(&root, &file.path))
            }
            Some(Item::Hunk(hunk)) => {
                self.pending_blame = Some(hunk.path.clone());
                PageOutcome::Job(exec::blame(&root, &hunk.path))
            }
            _ => {
                self.message = Some("no file to blame here".to_string());
                self.rebuild();
                PageOutcome::Consumed
            }
        }
    }

    /// Ask for the remote's URL, which the reply turns into a browser page.
    fn open_in_remote(&self) -> PageOutcome {
        match &self.root {
            Some(root) => PageOutcome::Job(exec::remote_url(root, DEFAULT_REMOTE)),
            None => PageOutcome::Consumed,
        }
    }

    /// The search keys, offered to both views before their own keys so that one
    /// query walks whichever rows are showing. `None` for anything else.
    fn on_search_key(&mut self, key: &Key) -> Option<PageOutcome> {
        if key.alt || key.ctrl {
            return None;
        }
        match key.code {
            KeyCode::Char('/') => Some(self.ask(ASK_SEARCH, "/")),
            KeyCode::Char('n') => {
                self.search_step(true);
                Some(PageOutcome::Consumed)
            }
            KeyCode::Char('N') => {
                self.search_step(false);
                Some(PageOutcome::Consumed)
            }
            _ => None,
        }
    }

    /// Keys while output fills the view: move, ask for more, or go back.
    fn on_output_key(&mut self, key: &Key) -> PageOutcome {
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        // A row of a commit's patch belongs to a file the same way a row of
        // the status view does, so it opens the same way. A view whose rows
        // stand for nothing (a log, a blame) keeps the key.
        if key.code == KeyCode::Enter && !key.ctrl {
            if let Some(target) = self.file_at_point() {
                return PageOutcome::OpenPath(target);
            }
        }
        if key.ctrl && key.code == KeyCode::Char('o') {
            if let Some(target) = self.file_at_point() {
                return PageOutcome::SpawnEditor(target);
            }
        }
        let Some(output) = self.output.as_mut() else {
            return PageOutcome::Consumed;
        };
        let last = output.lines.len().saturating_sub(1);
        if let Some(motion) = buffer_end(key) {
            output.cursor = match motion {
                CursorMove::Top => 0,
                _ => last,
            };
            return PageOutcome::Consumed;
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                output.cursor = (output.cursor + 1).min(last);
                PageOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                output.cursor = output.cursor.saturating_sub(1);
                PageOutcome::Consumed
            }
            KeyCode::Home => {
                output.cursor = 0;
                PageOutcome::Consumed
            }
            KeyCode::End => {
                output.cursor = last;
                PageOutcome::Consumed
            }
            // The keymap's load-more key, which only a log can answer.
            KeyCode::Char('+') if output.more => {
                self.log_count += LOG_PAGE;
                match self.root.clone() {
                    Some(root) => {
                        PageOutcome::Job(exec::log(&root, LogScope::Branch, self.log_count))
                    }
                    None => PageOutcome::Consumed,
                }
            }
            KeyCode::Char('y') => {
                let line = output.lines.get(output.cursor).cloned().unwrap_or_default();
                let value = line.split_whitespace().next().unwrap_or("").to_string();
                if value.is_empty() {
                    PageOutcome::Consumed
                } else {
                    PageOutcome::Yank(value)
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
                    return PageOutcome::Consumed;
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
                PageOutcome::Consumed
            }
            // Fold or unfold what the cursor is on. A file row shuts its whole
            // diff away; a hunk row shuts the hunk's body but keeps the header,
            // so the file still says what it contains.
            KeyCode::Tab => {
                let opened = match output.rows.get(output.cursor).map(|row| row.item.clone()) {
                    Some(Item::CommitChanges) => {
                        output.changes_shut = !output.changes_shut;
                        !output.changes_shut
                    }
                    Some(Item::CommitFile(file)) => {
                        let opened = output.folded_files.remove(&file);
                        if !opened {
                            output.folded_files.insert(file);
                        }
                        opened
                    }
                    Some(Item::CommitHunk(file, hunk)) => {
                        let opened = output.folded_hunks.remove(&(file, hunk));
                        if !opened {
                            output.folded_hunks.insert((file, hunk));
                        }
                        opened
                    }
                    // Nothing foldable under the cursor.
                    _ => return PageOutcome::Consumed,
                };
                if opened {
                    self.reveal = Some(output.cursor);
                }
                // Keep the cursor on the row it was on: folding a file leaves
                // its band in place, so the reader does not lose their spot.
                let at = output.cursor;
                repaint_commit(output);
                output.cursor = at.min(output.rows.len().saturating_sub(1));
                PageOutcome::Consumed
            }
            KeyCode::Char('q') | KeyCode::Escape => {
                self.output = None;
                PageOutcome::Consumed
            }
            _ => PageOutcome::Ignored,
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
    fn on_popup_key(&mut self, popup: Popup, code: KeyCode) -> PageOutcome {
        let KeyCode::Char(choice) = code else {
            return PageOutcome::Consumed;
        };
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
        };
        match (popup, choice) {
            // A jump needs no repository: it moves within what is drawn.
            (Popup::Jump, _) => self.jump_to(code),
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

            (Popup::Commit, 'c') => self.spawn_git(&root, &["commit"]),
            (Popup::Commit, 'm') => self.ask_commit_message(),
            (Popup::Commit, 'a') => self.spawn_git(&root, &["commit", "--amend"]),
            (Popup::Commit, 'e') => {
                PageOutcome::Job(exec::custom(&root, "commit --amend --no-edit"))
            }

            (Popup::Merge, 'm') => self.pick(ASK_MERGE, "Merge", exec::Candidates::Branches),
            (Popup::Merge, 'c') => {
                PageOutcome::Job(exec::sequence(&root, "merge", SequenceStep::Continue))
            }
            (Popup::Merge, 'x') => {
                PageOutcome::Job(exec::sequence(&root, "merge", SequenceStep::Abort))
            }

            // Interactive rebase needs a terminal for its todo list, so it goes
            // to a pane rather than being captured.
            (Popup::Rebase, 'i') => {
                self.pick(ASK_REBASE, "Rebase onto", exec::Candidates::Branches)
            }
            (Popup::Rebase, 'u') => PageOutcome::Job(exec::rebase(&root, "@{upstream}")),
            (Popup::Rebase, 'c') => {
                PageOutcome::Job(exec::sequence(&root, "rebase", SequenceStep::Continue))
            }
            (Popup::Rebase, 's') => {
                PageOutcome::Job(exec::sequence(&root, "rebase", SequenceStep::Skip))
            }
            (Popup::Rebase, 'x') => {
                PageOutcome::Job(exec::sequence(&root, "rebase", SequenceStep::Abort))
            }

            (Popup::Stash, 'z') => self.ask(ASK_STASH, "Stash message (optional): "),
            (Popup::Stash, 'p') => PageOutcome::Job(exec::stash(&root, "pop")),
            (Popup::Stash, 'a') => PageOutcome::Job(exec::stash(&root, "apply")),
            (Popup::Stash, 'd') => PageOutcome::Job(exec::stash(&root, "drop")),
            (Popup::Stash, 'l') => PageOutcome::Job(exec::stash_list(&root)),

            (Popup::Tag, 't') => self.ask(ASK_TAG_CREATE, "Tag name: "),
            (Popup::Tag, 'd') => self.pick(ASK_TAG_DELETE, "Delete tag", exec::Candidates::Tags),
            (Popup::Tag, 'l') => PageOutcome::Job(exec::tag_list(&root)),

            (Popup::Remote, 'v') => PageOutcome::Job(exec::remote_list(&root)),
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
                PageOutcome::Job(exec::sequence(&root, "cherry-pick", SequenceStep::Continue))
            }
            (Popup::CherryPick, 'x') => {
                PageOutcome::Job(exec::sequence(&root, "cherry-pick", SequenceStep::Abort))
            }

            (Popup::Revert, 'v') => self.ask(ASK_REVERT, "Revert: "),
            (Popup::Revert, 'c') => {
                PageOutcome::Job(exec::sequence(&root, "revert", SequenceStep::Continue))
            }
            (Popup::Revert, 'x') => {
                PageOutcome::Job(exec::sequence(&root, "revert", SequenceStep::Abort))
            }

            (Popup::Reset, 'm') => self.ask(ASK_RESET_MIXED, "Reset (mixed) to: "),
            (Popup::Reset, 's') => self.ask(ASK_RESET_SOFT, "Reset (soft) to: "),
            // Hard reset throws away work with no way back, so it asks in the
            // same words as a delete and takes the revision as the answer.
            (Popup::Reset, 'h') => self.ask(ASK_RESET_HARD, "Reset HARD, losing everything, to: "),

            (Popup::Diff, 'd') => PageOutcome::Job(exec::diff_all(&root, false, None)),
            (Popup::Diff, 's') => PageOutcome::Job(exec::diff_all(&root, true, None)),
            (Popup::Diff, 'r') => self.ask(ASK_DIFF_REV, "Diff against: "),

            // Every file choice acts on the row the popup was opened over.
            (Popup::File, choice @ ('s' | 'u' | 'x' | 'd' | 'l' | 'b')) => {
                let Some(path) = self.path_at_cursor() else {
                    return PageOutcome::Consumed;
                };
                match choice {
                    's' => self.stage(false),
                    'u' => self.unstage(false),
                    'x' => self.discard(),
                    'b' => PageOutcome::Job(exec::blame(&root, &path)),
                    'd' => self.toggle_fold(),
                    _ => {
                        self.log_count = LOG_PAGE;
                        PageOutcome::Job(exec::log(&root, LogScope::File(path), self.log_count))
                    }
                }
            }
            (Popup::Log, 'l') => {
                self.log_count = LOG_PAGE;
                PageOutcome::Job(exec::log(&root, LogScope::Branch, self.log_count))
            }
            (Popup::Log, 'a') => {
                self.log_count = LOG_PAGE;
                PageOutcome::Job(exec::log(&root, LogScope::AllRefs, self.log_count))
            }
            (Popup::Log, 'f') => match self.path_at_cursor() {
                Some(path) => {
                    self.log_count = LOG_PAGE;
                    PageOutcome::Job(exec::log(&root, LogScope::File(path), self.log_count))
                }
                None => PageOutcome::Consumed,
            },

            (Popup::Ignore, 'i') => self.ignore_path(false),
            (Popup::Ignore, 'e') => self.ignore_path(true),

            (Popup::Worktree, 'l') => PageOutcome::Job(exec::worktree_list(&root)),
            (Popup::Worktree, 'a') => self.ask(ASK_WORKTREE_ADD, "Worktree path: "),
            (Popup::Worktree, 'd') => self.ask(ASK_WORKTREE_REMOVE, "Remove worktree: "),

            (Popup::Bisect, 's') => PageOutcome::Job(exec::bisect(&root, "start")),
            (Popup::Bisect, 'g') => PageOutcome::Job(exec::bisect(&root, "good")),
            (Popup::Bisect, 'b') => PageOutcome::Job(exec::bisect(&root, "bad")),
            (Popup::Bisect, 'r') => PageOutcome::Job(exec::bisect(&root, "reset")),

            _ => PageOutcome::Consumed,
        }
    }

    /// Ask a one-line question, tagged so the answer finds its way back.
    /// Ask the same question [`ask`](Self::ask) does, but over the names it
    /// can be answered with: the reader filters a list instead of spelling a
    /// branch out. The names are read first, so this hands back the job that
    /// reads them and the answer comes through the same tag either way.
    fn pick(&mut self, tag: &'static str, label: &str, kind: exec::Candidates) -> PageOutcome {
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
        };
        self.pending_pick = Some(PickQuestion::new(tag, label));
        PageOutcome::Job(exec::candidates(&root, kind))
    }

    fn ask(&self, tag: &'static str, label: &str) -> PageOutcome {
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: label.to_string(),
            mode: PromptMode::Text,
            tag,
        })
    }

    /// Run git in a pane of its own, for a command that wants a terminal.
    fn spawn_git(&self, root: &std::path::Path, args: &[&str]) -> PageOutcome {
        PageOutcome::Spawn(SpawnRequest {
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
    fn ignore_path(&mut self, by_extension: bool) -> PageOutcome {
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
        };
        let Some(path) = self.path_at_cursor() else {
            return PageOutcome::Consumed;
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
                self.message = Some(format!("ignored {pattern}"));
                self.refresh()
            }
            Err(e) => {
                self.message = Some(format!("failed: {e}"));
                self.rebuild();
                PageOutcome::Consumed
            }
        }
    }

    /// Resolve the key after `g`: each names a section to jump to, and
    /// anything else abandons the sequence.
    fn jump_to(&mut self, code: KeyCode) -> PageOutcome {
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
        PageOutcome::Consumed
    }

    /// Open the file at point, at whatever line the row knows about.
    fn open_at_point(&self) -> PageOutcome {
        match self.file_at_point() {
            Some(target) => PageOutcome::OpenPath(target),
            None => PageOutcome::Consumed,
        }
    }

    /// Hand the file at point to `$EDITOR`, in a pane of its own, for the
    /// editing the app's own editor deliberately cannot do.
    fn open_external(&self) -> PageOutcome {
        match self.file_at_point() {
            Some(target) => PageOutcome::SpawnEditor(target),
            None => PageOutcome::Consumed,
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
    fn on_command(&mut self, output: CommandOutput) -> PageOutcome {
        if !output.succeeded() {
            self.message = Some(format!("failed: {}", output.failure()));
            self.rebuild();
            return PageOutcome::Consumed;
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
                PageOutcome::Job(exec::status(&root))
            }
            exec::TAG_STATUS => {
                self.status = parse::parse_status(&output.stdout);
                self.loaded = true;
                // A diff read before the change is now wrong, and staging a
                // hunk from a stale diff applies it to the wrong lines.
                self.diffs.clear();
                self.forget_unlisted_files();
                self.rebuild();
                match &self.root {
                    Some(root) => PageOutcome::Job(exec::recent_log(root)),
                    None => PageOutcome::Consumed,
                }
            }
            exec::TAG_DIFF => {
                if let Some(file) = self.pending_diff.take() {
                    self.diffs
                        .insert(file.clone(), diff::parse_diff(&output.stdout));
                    self.rebuild();
                    // The diff is only now there to show, so the row that
                    // asked for it takes its turn at being brought into view.
                    self.reveal = self.row_of_file(&file);
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
                    Some(root) => PageOutcome::Job(exec::stash_entries(root)),
                    None => PageOutcome::Consumed,
                }
            }
            exec::TAG_CANDIDATES => {
                let Some(question) = self.pending_pick.take() else {
                    return PageOutcome::Consumed;
                };
                // A remote's own `HEAD` is a pointer at one of the names
                // already listed, never an answer of its own.
                match question.over_lines(&output.stdout, |name| !name.ends_with("/HEAD")) {
                    Some(request) => PageOutcome::Pick(request),
                    // With nothing to choose from there is still a question
                    // to answer, so it falls back to being typed.
                    None => self.ask(question.tag, &format!("{}: ", question.label)),
                }
            }
            exec::TAG_STASHES => {
                self.stashes = parse::parse_stash_list(&output.stdout);
                self.rebuild();
                match &self.root {
                    Some(root) => PageOutcome::Job(exec::unpulled(root)),
                    None => PageOutcome::Consumed,
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
                        PageOutcome::Job(JobRequest::ReadFiles(progress::state_files(dir)))
                    }
                    None => PageOutcome::Consumed,
                }
            }
            // A blame and a plain read differ only in what the view is called.
            exec::TAG_BLAME | exec::TAG_READ => {
                if output.stdout.trim().is_empty() {
                    self.message = Some("nothing to show".to_string());
                    self.rebuild();
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
                PageOutcome::Consumed
            }
            // A whole diff fills the view decorated, line for line.
            exec::TAG_DIFF_VIEW => {
                if output.stdout.trim().is_empty() {
                    self.message = Some("nothing to show".to_string());
                    self.rebuild();
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    self.show_painted("Diff", rows::diff_view_rows(&lines));
                }
                PageOutcome::Consumed
            }
            // The log view lists commits the way the status tail does, rather
            // than as the raw lines git wrote, so both read the same.
            exec::TAG_LOG_VIEW => {
                let commits = parse::parse_decorated_log(&output.stdout);
                if commits.is_empty() {
                    self.message = Some("nothing to show".to_string());
                    self.rebuild();
                } else {
                    let painted = rows::log_rows(&commits, self.root.as_deref(), now_unix(), true);
                    self.show_painted("Log", painted);
                    // The load-more key only means anything while a log shows.
                    if let Some(output) = &mut self.output {
                        output.more = true;
                    }
                }
                PageOutcome::Consumed
            }
            // A commit fills the view as its content: the summary it was shown
            // with, then its files banded and their hunks decorated, the way
            // the working tree reads.
            exec::TAG_SHOW => {
                if output.stdout.trim().is_empty() {
                    self.message = Some("nothing to show".to_string());
                    self.rebuild();
                } else {
                    let lines: Vec<String> = output.stdout.lines().map(str::to_string).collect();
                    let title = commit_title(&output.stdout);
                    self.show_commit(&title, CommitContent::parse(&lines));
                }
                PageOutcome::Consumed
            }
            exec::TAG_REMOTE_URL => {
                let url = browser_url(output.stdout.trim());
                match url {
                    Some(url) => PageOutcome::OpenExternal(std::path::PathBuf::from(url)),
                    None => {
                        self.message = Some("no web URL for that remote".to_string());
                        self.rebuild();
                        PageOutcome::Consumed
                    }
                }
            }
            // Anything that changed the repository: report it and re-read.
            tag => {
                self.message = Some(report_for(tag, &output));
                self.refresh()
            }
        }
    }
}

impl Page for GitPage {
    fn title(&self) -> String {
        "Git".to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
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
            let mut window = wrap_window(
                self.scroll,
                output.cursor,
                output.lines.len(),
                visible,
                height,
            );
            if let Some(start) = reveal_start(&output.rows, &window, self.reveal.take()) {
                window = wrap_window(start, output.cursor, output.lines.len(), visible, height);
            }
            self.scroll = window.start;
            let mut header = vec![
                PageSpan::new(PageStyle::Header, output.title.clone()),
                PageSpan::new(PageStyle::Dim, "  q to go back".to_string()),
            ];
            // The status header is not on screen here, so what a command
            // reported has to be said in this one or not at all.
            if let Some(message) = &self.message {
                header.push(PageSpan::new(PageStyle::Dim, format!("  {message}")));
            }
            let mut painted = vec![header];
            let mut wrap_indents = vec![0];
            let mut icons: Vec<PageIcon> = Vec::new();
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
                            icons.push(PageIcon {
                                row: painted.len(),
                                ..icon.clone()
                            });
                        }
                        painted.push(row.spans.clone());
                        wrap_indents.push(row.wrap_indent);
                    }
                    // Output that never painted itself shows as plain text.
                    None => {
                        painted.push(vec![PageSpan::plain(line.clone())]);
                        wrap_indents.push(0);
                    }
                }
            }
            return PageContent::new(painted)
                .with_icons(icons)
                .with_cursor_line(HEADER_ROWS + window.cursor)
                .with_wrap_indents(wrap_indents);
        }
        if self.rows.is_empty() {
            // A loaded view with no rows means git answered and had nothing
            // to report, which only happens outside a repository: a clean tree
            // still has a header row.
            let note = if self.loaded { NOT_A_REPO } else { LOADING };
            return PageContent::new(vec![vec![PageSpan::new(PageStyle::Dim, note)]]);
        }
        let indents: Vec<usize> = self.rows.iter().map(|row| row.wrap_indent).collect();
        let texts: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
        let visible = rows.saturating_sub(HEADER_ROWS);
        let height = |index: usize| row_height(&texts[index], cols, wrap, indents[index]);
        let mut window = wrap_window(self.scroll, self.cursor, self.rows.len(), visible, height);
        if let Some(start) = reveal_start(&self.rows, &window, self.reveal.take()) {
            window = wrap_window(start, self.cursor, self.rows.len(), visible, height);
        }
        self.scroll = window.start;
        let mut painted: Vec<Vec<PageSpan>> = Vec::new();
        let mut wrap_indents: Vec<usize> = Vec::new();
        let mut icons: Vec<PageIcon> = Vec::new();
        for row in self
            .rows
            .iter()
            .skip(window.start)
            .take(window.count.max(1))
        {
            if let Some(icon) = &row.icon {
                icons.push(PageIcon {
                    row: painted.len(),
                    ..icon.clone()
                });
            }
            painted.push(row.spans.clone());
            wrap_indents.push(row.wrap_indent);
        }
        PageContent::new(painted)
            .with_icons(icons)
            .with_cursor_line(window.cursor)
            .with_wrap_indents(wrap_indents)
    }

    fn hint(&self) -> Option<PageHint> {
        let popup = self.popup?;
        Some(PageHint {
            items: popup
                .choices()
                .iter()
                .map(|(key, what)| (key.to_string(), what.to_string()))
                .collect(),
            title: popup.title().to_string(),
        })
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        if let Some(popup) = self.popup.take() {
            return self.on_popup_key(popup, key.code);
        }
        if self.output.is_some() {
            return self.on_output_key(key);
        }
        if key.alt {
            if let Some(motion) = buffer_end(key) {
                self.apply_motion(motion);
                return PageOutcome::Consumed;
            }
            return match key.code {
                KeyCode::Char('n') => {
                    self.move_to_entity(true);
                    PageOutcome::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_to_entity(false);
                    PageOutcome::Consumed
                }
                KeyCode::Char('y') => match &self.root {
                    Some(root) => PageOutcome::Job(exec::show_refs(root)),
                    None => PageOutcome::Consumed,
                },
                KeyCode::Char('b') => self.blame_at_point(),
                KeyCode::Char('g') => self.open_in_remote(),
                _ => PageOutcome::Ignored,
            };
        }
        if key.ctrl {
            return match key.code {
                KeyCode::Char('C') | KeyCode::Char('c') if key.shift => self.ask_commit_message(),
                KeyCode::Char('S') | KeyCode::Char('s') if key.shift => self.stage_all(),
                KeyCode::Char('l') => self.open_popup(Popup::Log),
                KeyCode::Char('o') => self.open_external(),
                // The paging chords the shared layer binds fall through to it;
                // the rest stay with the window.
                _ => match self.nav.key(key) {
                    VimKey::Motion(motion) => {
                        self.apply_motion(motion);
                        PageOutcome::Consumed
                    }
                    VimKey::Pending => PageOutcome::Consumed,
                    VimKey::Unhandled => PageOutcome::Ignored,
                },
            };
        }
        if let Some(outcome) = self.on_search_key(key) {
            return outcome;
        }
        match key.code {
            KeyCode::Tab if key.shift => {
                self.toggle_fold_all();
                PageOutcome::Consumed
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
                    Some(root) => PageOutcome::Job(exec::show_commit(&root, &hash)),
                    None => PageOutcome::Consumed,
                },
                // A stash reads as the diff it holds, the way a commit reads
                // as the patch it is.
                Some(Item::Stash(index)) => match (self.root.clone(), self.stashes.get(index)) {
                    (Some(root), Some(stash)) => {
                        PageOutcome::Job(exec::stash_show(&root, &stash.name))
                    }
                    _ => PageOutcome::Consumed,
                },
                _ => PageOutcome::Consumed,
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
                    self.message = Some("no file here".to_string());
                    self.rebuild();
                    PageOutcome::Consumed
                }
            },
            KeyCode::Char('!') => self.ask(ASK_CUSTOM, "git "),
            KeyCode::Char('P') => match &self.root {
                Some(root) => PageOutcome::Job(exec::push(root)),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('F') => match &self.root {
                Some(root) => PageOutcome::Job(exec::pull(root)),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('f') => match &self.root {
                Some(root) => PageOutcome::Job(exec::fetch(root)),
                None => PageOutcome::Consumed,
            },
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            // Everything unclaimed falls through to the shared Vim motion
            // layer, whose motions this view interprets over its rows.
            _ => match self.nav.key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    PageOutcome::Consumed
                }
                VimKey::Pending => PageOutcome::Consumed,
                VimKey::Unhandled => PageOutcome::Ignored,
            },
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        let Some(answer) = reply.answer else {
            self.message = Some("cancelled".to_string());
            self.rebuild();
            return PageOutcome::Consumed;
        };
        // A search is the one question that asks nothing of git, so it is
        // answered before the repository is required.
        if reply.tag == ASK_SEARCH {
            self.search_for(&answer);
            return PageOutcome::Consumed;
        }
        let Some(root) = self.root.clone() else {
            return PageOutcome::Consumed;
        };
        let answer = answer.trim().to_string();
        // Every question but the stash message needs an answer to act on.
        if answer.is_empty() && reply.tag != ASK_STASH {
            return PageOutcome::Consumed;
        }
        match reply.tag {
            ASK_BRANCH_CHECKOUT => PageOutcome::Job(exec::checkout(&root, &answer)),
            ASK_BRANCH_CREATE => PageOutcome::Job(exec::branch_create(&root, &answer, true)),
            ASK_BRANCH_CREATE_HERE => PageOutcome::Job(exec::branch_create(&root, &answer, false)),
            ASK_BRANCH_DELETE => PageOutcome::Job(exec::branch_delete(&root, &answer, false)),
            ASK_BRANCH_DELETE_FORCE => PageOutcome::Job(exec::branch_delete(&root, &answer, true)),
            ASK_CHERRY_PICK => PageOutcome::Job(exec::cherry_pick(&root, &answer)),
            ASK_COMMIT => PageOutcome::Job(exec::commit(&root, &answer)),
            ASK_CUSTOM => PageOutcome::Job(exec::custom(&root, &answer)),
            ASK_DIFF_REV => PageOutcome::Job(exec::diff_all(&root, false, Some(&answer))),
            ASK_DISCARD => self.discard(),
            ASK_MERGE => PageOutcome::Job(exec::merge(&root, &answer)),
            ASK_REBASE => self.spawn_git(&root, &["rebase", "--interactive", &answer]),
            ASK_REMOTE_ADD => match answer.split_once(char::is_whitespace) {
                Some((name, url)) => {
                    PageOutcome::Job(exec::remote_add(&root, name.trim(), url.trim()))
                }
                None => {
                    self.message = Some("failed: expected `name url`".to_string());
                    self.rebuild();
                    PageOutcome::Consumed
                }
            },
            ASK_REMOTE_PRUNE => PageOutcome::Job(exec::remote_prune(&root, &answer)),
            ASK_REMOTE_REMOVE => PageOutcome::Job(exec::remote_remove(&root, &answer)),
            ASK_RESET_HARD => PageOutcome::Job(exec::reset(&root, ResetMode::Hard, &answer)),
            ASK_RESET_MIXED => PageOutcome::Job(exec::reset(&root, ResetMode::Mixed, &answer)),
            ASK_RESET_SOFT => PageOutcome::Job(exec::reset(&root, ResetMode::Soft, &answer)),
            ASK_REVERT => PageOutcome::Job(exec::revert(&root, &answer)),
            ASK_STASH => PageOutcome::Job(exec::stash_push(&root, &answer)),
            ASK_TAG_CREATE => PageOutcome::Job(exec::tag_create(&root, &answer)),
            ASK_TAG_DELETE => PageOutcome::Job(exec::tag_delete(&root, &answer)),
            ASK_WORKTREE_ADD => PageOutcome::Job(exec::worktree_add(&root, &answer)),
            ASK_WORKTREE_REMOVE => PageOutcome::Job(exec::worktree_remove(&root, &answer)),
            _ => PageOutcome::Consumed,
        }
    }

    fn on_resume(&mut self) -> PageOutcome {
        // The file that was just being edited is very likely one of the ones
        // this view reports on.
        self.refresh()
    }

    fn on_job(&mut self, reply: JobReply) -> PageOutcome {
        match reply {
            JobReply::Command(output) => self.on_command(output),
            JobReply::Files(files) => {
                // What git is part-way through, read straight from the files
                // it writes it in: nothing about it reaches a porcelain
                // status, so there is nothing to run for it.
                self.progress = progress::parse(&files);
                self.rebuild();
                PageOutcome::Consumed
            }
            // The view asks for no directory walks and no searches.
            JobReply::DirSize { .. } | JobReply::Search(_) => PageOutcome::Consumed,
        }
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

    fn context_items(&self) -> Vec<PageMenuItem> {
        // What a row offers is what that kind of row can do, which is why the
        // menu is built per item rather than as one list with things greyed
        // out: a commit cannot be staged and a heading has no path to copy.
        match self.item_at_point() {
            Some(Item::File(_)) | Some(Item::CommitFile(_)) => vec![
                PageMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN),
                PageMenuItem::new(Key::with_ctrl(KeyCode::Char('o')), LABEL_OPEN_IN_EDITOR),
                PageMenuItem::new(Key::plain(KeyCode::Tab), LABEL_DIFF),
                PageMenuItem::new(Key::plain(KeyCode::Char('s')), LABEL_STAGE),
                PageMenuItem::new(Key::plain(KeyCode::Char('u')), LABEL_UNSTAGE),
                PageMenuItem::new(Key::plain(KeyCode::Char('x')), LABEL_DISCARD),
                PageMenuItem::new(Key::with_alt(KeyCode::Char('b')), LABEL_BLAME),
                PageMenuItem::new(Key::plain(KeyCode::Char('y')), LABEL_COPY_PATH),
            ],
            Some(Item::Hunk(_)) | Some(Item::CommitHunk(_, _)) => vec![
                PageMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN_AT_LINE),
                PageMenuItem::new(Key::plain(KeyCode::Char('s')), LABEL_STAGE_HUNK),
                PageMenuItem::new(Key::plain(KeyCode::Char('u')), LABEL_UNSTAGE_HUNK),
                PageMenuItem::new(Key::plain(KeyCode::Char('x')), LABEL_DISCARD_HUNK),
                PageMenuItem::new(Key::plain(KeyCode::Char('a')), LABEL_APPLY_HUNK),
                PageMenuItem::new(Key::plain(KeyCode::Char('-')), LABEL_REVERSE_HUNK),
                PageMenuItem::new(Key::plain(KeyCode::Char('y')), LABEL_COPY_PATH),
            ],
            Some(Item::Commit(_)) => vec![
                PageMenuItem::new(Key::plain(KeyCode::Enter), LABEL_READ_COMMIT),
                PageMenuItem::new(Key::plain(KeyCode::Char('y')), LABEL_COPY_HASH),
            ],
            Some(Item::Stash(_)) => vec![
                PageMenuItem::new(Key::plain(KeyCode::Enter), LABEL_READ_STASH),
                PageMenuItem::new(Key::plain(KeyCode::Char('z')), LABEL_STASH_MENU),
            ],
            Some(Item::Heading(_))
            | Some(Item::CommitChanges)
            | Some(Item::StashHeading)
            | Some(Item::UnpulledHeading)
            | Some(Item::RecentHeading) => vec![
                PageMenuItem::new(Key::plain(KeyCode::Tab), LABEL_FOLD),
                PageMenuItem::new(Key::plain(KeyCode::Char('S')), LABEL_STAGE_ALL),
                PageMenuItem::new(Key::plain(KeyCode::Char('U')), LABEL_UNSTAGE_ALL),
                PageMenuItem::new(Key::plain(KeyCode::Char('G')), LABEL_RELOAD),
            ],
            Some(Item::None) | None => vec![
                PageMenuItem::new(Key::with_ctrl_shift(KeyCode::Char('C')), LABEL_COMMIT),
                PageMenuItem::new(Key::plain(KeyCode::Char('P')), LABEL_PUSH),
                PageMenuItem::new(Key::plain(KeyCode::Char('F')), LABEL_PULL),
                PageMenuItem::new(Key::plain(KeyCode::Char('f')), LABEL_FETCH),
                PageMenuItem::new(Key::plain(KeyCode::Char('G')), LABEL_RELOAD),
            ],
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

/// Redraw a commit output's rows at its current fold depth, keeping the search
/// text in step with them.
///
/// Called after every fold change rather than the rows being edited in place:
/// folding a file removes a variable number of rows, and rebuilding from the
/// content is the only way the row list, the search text, and the fold sets
/// cannot drift apart.
/// Where to scroll so a just-opened row shows what it opened: the row itself,
/// once the block it heads runs past the window's bottom, since from there the
/// most of that block fits. `None` leaves the window alone, which is the case
/// whenever the block already shows in full — an unfold near the top of the
/// pane must not shift the page out from under the reader — and whenever
/// nothing was opened at all.
fn reveal_start(rows: &[ViewRow], window: &PageWindow, at: Option<usize>) -> Option<usize> {
    let at = at?;
    let end = rows::block_end(rows, at);
    (end >= window.start + window.count).then_some(at)
}

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

/// What to say in the header after a command that changed something.
fn report_for(tag: &'static str, output: &CommandOutput) -> String {
    let detail = output
        .stdout
        .lines()
        .chain(output.stderr.lines())
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");
    match tag {
        exec::TAG_COMMIT => format!("committed: {detail}"),
        exec::TAG_DISCARD => "discarded".to_string(),
        exec::TAG_FETCH => "fetched".to_string(),
        exec::TAG_PULL => format!("pulled: {detail}"),
        exec::TAG_PUSH => "pushed".to_string(),
        exec::TAG_STAGE => "staged".to_string(),
        exec::TAG_UNSTAGE => "unstaged".to_string(),
        _ => detail.to_string(),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::page::CommandRequest;

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

    fn command_of(outcome: &PageOutcome) -> CommandRequest {
        match outcome {
            PageOutcome::Job(JobRequest::Command(command)) => command.clone(),
            other => panic!("expected a command, got {other:?}"),
        }
    }

    /// A page that has heard back about its root, its status, and its log.
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

    fn loaded_page() -> GitPage {
        let mut page = GitPage::new(PathBuf::from("/repo/sub"));
        page.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        page.on_job(reply(
            exec::TAG_LOG,
            &log_output(&[("abc1234", "do the thing")]),
        ));
        page
    }

    fn cursor_on(page: &mut GitPage, path: &str) {
        let index = page
            .rows
            .iter()
            .position(|row| matches!(&row.item, Item::File(file) if file.path == path))
            .unwrap_or_else(|| panic!("no row for {path}"));
        page.cursor = index;
    }

    #[test]
    fn test_every_menu_entry_runs_a_key_the_view_binds() {
        // The menu offers keys rather than commands of its own, so an entry
        // naming a chord the page does not match does nothing when chosen.
        // The commit entry is `Ctrl-Shift-C`, which is exactly the shape that
        // gets written with a modifier missing.
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let over_file = page.context_items();

        let mut header = loaded_page();
        header.cursor = 0;
        let over_header = header.context_items();

        for (items, on_file) in [(over_file, true), (over_header, false)] {
            assert!(!items.is_empty());
            for item in items {
                let mut probe = loaded_page();
                match on_file {
                    true => cursor_on(&mut probe, "working.rs"),
                    false => probe.cursor = 0,
                }
                assert_ne!(
                    probe.on_key(&item.key),
                    PageOutcome::Ignored,
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
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let labels: Vec<String> = page
            .context_items()
            .into_iter()
            .map(|item| item.label)
            .collect();
        assert!(labels.contains(&LABEL_STAGE.to_string()));
        assert!(!labels.contains(&LABEL_COPY_HASH.to_string()));

        let commit = page
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("the log put a commit in the view");
        page.cursor = commit;
        let labels: Vec<String> = page
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
        let mut page = GitPage::new(PathBuf::from("/repo/sub"));
        let next = page.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        let command = command_of(&next);
        assert_eq!(command.tag, exec::TAG_STATUS);
        assert_eq!(command.cwd, PathBuf::from("/repo"), "not the subdirectory");
    }

    #[test]
    fn test_a_status_read_is_followed_by_the_recent_log() {
        let mut page = GitPage::new(PathBuf::from("/repo"));
        page.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        let next = page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        assert_eq!(command_of(&next).tag, exec::TAG_LOG);
    }

    #[test]
    fn test_staging_acts_on_the_path_under_the_cursor() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["add", "--", "working.rs"]);
    }

    #[test]
    fn test_staging_on_a_heading_acts_on_that_whole_section() {
        // The heading stands for its section, which is what makes "stage this
        // lot" one keystroke.
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| row.item == Item::Heading(Section::Unstaged))
            .expect("the unstaged heading");
        let command = command_of(&page.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["add", "--", "working.rs"]);
    }

    #[test]
    fn test_stage_all_from_the_untracked_section_stages_only_untracked_files() {
        // Magic's stage-all splits by section: from untracked it stages the
        // files that section lists, by name.
        let mut page = loaded_page();
        cursor_on(&mut page, "new.rs");
        let command = command_of(&page.on_key(&ctrl_shift('S')));
        assert_eq!(command.args, ["add", "--", "new.rs"]);
    }

    #[test]
    fn test_stage_all_elsewhere_stages_tracked_changes_only() {
        // Anywhere else it is `add -u`, so an untracked file stays that way
        // until asked for by name.
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let command = command_of(&page.on_key(&ctrl_shift('S')));
        assert_eq!(command.args, ["add", "-u"]);
    }

    #[test]
    fn test_unstaging_uses_the_staged_copy_of_a_file_changed_on_both_sides() {
        // `staged.rs` and `working.rs` differ, but a file in both sections must
        // unstage from the staged row and stage from the unstaged one.
        let mut page = loaded_page();
        cursor_on(&mut page, "staged.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Char('u'))));
        assert_eq!(command.args, ["restore", "--staged", "--", "staged.rs"]);
    }

    #[test]
    fn test_discarding_asks_first_and_names_the_file() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('x'))) else {
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
        let mut page = loaded_page();
        cursor_on(&mut page, "new.rs");
        page.on_key(&press(KeyCode::Char('x')));
        let command = command_of(&page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, ["clean", "--force", "-d", "--", "new.rs"]);
    }

    #[test]
    fn test_declining_the_confirmation_runs_nothing() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        page.on_key(&press(KeyCode::Char('x')));
        let outcome = page.on_prompt(PromptReply {
            answer: None,
            tag: ASK_DISCARD,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
    }

    #[test]
    fn test_committing_nothing_is_refused_before_it_asks() {
        // `git commit` with an empty index fails with a wall of advice; not
        // asking for a message is the better answer.
        let mut page = GitPage::new(PathBuf::from("/repo"));
        page.on_job(reply(exec::TAG_ROOT, "/repo\n/repo/.git\n"));
        page.on_job(reply(exec::TAG_STATUS, "# branch.head main\n"));
        let outcome = page.on_key(&Key {
            alt: false,
            code: KeyCode::Char('c'),
            ctrl: true,
            shift: true,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
    }

    #[test]
    fn test_a_command_that_changed_the_tree_re_reads_it() {
        // Without the re-read the view shows the state before the change, and
        // the next key acts on rows that no longer describe the repository.
        let mut page = loaded_page();
        let next = page.on_job(reply(exec::TAG_STAGE, ""));
        assert_eq!(command_of(&next).tag, exec::TAG_STATUS);
        assert_eq!(page.message.as_deref(), Some("staged"));
    }

    #[test]
    fn test_a_failed_command_reports_and_does_not_re_read() {
        let mut page = loaded_page();
        let outcome = page.on_job(JobReply::Command(CommandOutput {
            code: Some(1),
            stderr: "error: pathspec 'nope' did not match\n".to_string(),
            stdout: String::new(),
            tag: exec::TAG_STAGE,
        }));
        assert_eq!(outcome, PageOutcome::Consumed);
        let message = page.message.clone().unwrap_or_default();
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

    fn cursor_on_hunk(page: &mut GitPage, index: usize) {
        let position = page
            .rows
            .iter()
            .position(|row| matches!(&row.item, Item::Hunk(hunk) if hunk.index == index))
            .unwrap_or_else(|| panic!("no row for hunk {index}"));
        page.cursor = position;
    }

    /// A page with `working.rs` expanded and its diff read.
    fn page_with_diff() -> GitPage {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        page.on_key(&press(KeyCode::Tab));
        page.on_job(reply(exec::TAG_DIFF, DIFF_OUTPUT));
        page
    }

    #[test]
    fn test_enter_on_a_hunk_opens_its_file_at_the_line_the_hunk_changes() {
        // Opening the file at line one from a hunk throws away the one thing
        // the cursor's position said: which part of it is being read.
        let mut page = page_with_diff();
        cursor_on_hunk(&mut page, 1);
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 10))
        );

        // And the file's own band still opens it at the top, since it names
        // no line in particular.
        cursor_on(&mut page, "working.rs");
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/working.rs")))
        );
    }

    #[test]
    fn test_enter_in_a_commits_patch_opens_the_file_that_row_belongs_to() {
        // A commit is read as its patch, and every file band and hunk in it
        // stands for a file in the working tree: Enter did nothing there.
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));

        output_cursor_on(&mut page, "src/a.rs");
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::file(PathBuf::from("/repo/src/a.rs"))),
            "the file band opens the file"
        );

        // Open the file's diff, then land on its second hunk.
        page.on_key(&press(KeyCode::Tab));
        let output = page.output.as_mut().expect("an output view");
        output.cursor = output
            .rows
            .iter()
            .position(|row| row.item == Item::CommitHunk(0, 1))
            .expect("a row for the second hunk");
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 9)),
            "and a hunk opens it at the line it changes"
        );
    }

    #[test]
    fn test_enter_in_a_blame_opens_the_file_at_the_line_under_the_cursor() {
        // A blame is one row per line of the file, so the row the cursor is
        // on is the line to land on: opening at the top would throw away the
        // only thing the reader was pointing at.
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        page.on_key(&alt(KeyCode::Char('b')));
        page.on_job(reply(
            exec::TAG_BLAME,
            "abc1234 (Someone 2026-09-18 1) fn main() {\n             abc1234 (Someone 2026-09-18 2)     old();\n             def5678 (Another 2026-09-18 3) }\n",
        ));

        let output = page.output.as_mut().expect("the blame");
        output.cursor = 2;
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/working.rs"), 3))
        );
    }

    #[test]
    fn test_enter_on_a_row_belonging_to_no_file_is_left_alone() {
        // A log's rows stand for nothing to open, and swallowing the key
        // there would take it from the window for no gain.
        let mut page = loaded_page();
        page.on_job(reply(
            exec::TAG_LOG,
            &log_output(&[("abc1234", "a commit")]),
        ));
        page.on_key(&ctrl_shift('l'));
        if page.output.is_some() {
            assert_eq!(page.on_key(&press(KeyCode::Enter)), PageOutcome::Ignored);
        }

        // In the status view, a section heading names a set of files rather
        // than one, so Enter has nothing to open there either.
        let mut status = loaded_page();
        let heading = status
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Heading(_)))
            .expect("a heading row");
        status.cursor = heading;
        assert_eq!(status.on_key(&press(KeyCode::Enter)), PageOutcome::Consumed);
    }

    #[test]
    fn test_opening_a_file_scrolls_its_diff_into_a_short_pane() {
        // Unfolding at the bottom of the pane used to leave the diff below
        // the fold: the cursor stayed on the band, which was already visible,
        // so nothing scrolled and the rows just opened were off screen.
        let mut page = page_with_diff();
        let painted = |page: &mut GitPage| -> Vec<String> {
            page.content(6, 80, false)
                .rows
                .iter()
                .map(row_text)
                .collect()
        };
        // Fold it away, scroll so the band sits at the pane's bottom, then
        // open it again.
        page.on_key(&press(KeyCode::Tab));
        painted(&mut page);
        page.on_key(&press(KeyCode::Tab));
        let rows = painted(&mut page);
        assert!(
            rows.iter().any(|row| row.contains("@@")),
            "the diff the fold opened is on screen, got {rows:?}"
        );
    }

    #[test]
    fn test_opening_a_file_already_showing_in_full_leaves_the_page_alone() {
        // The scroll only moves for a block that runs past the pane's bottom:
        // one that fits must not shift the page under the reader.
        let mut page = page_with_diff();
        page.content(40, 80, false);
        let before = page.scroll;
        page.on_key(&press(KeyCode::Tab));
        page.content(40, 80, false);
        page.on_key(&press(KeyCode::Tab));
        page.content(40, 80, false);
        assert_eq!(page.scroll, before, "a pane with room to spare stays put");
    }

    #[test]
    fn test_opening_a_commits_file_scrolls_its_hunks_into_a_short_pane() {
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        output_cursor_on(&mut page, "src/a.rs");
        page.content(5, 80, false);
        page.on_key(&press(KeyCode::Tab));
        let rows: Vec<String> = page
            .content(5, 80, false)
            .rows
            .iter()
            .map(row_text)
            .collect();
        assert!(
            rows.iter().any(|row| row.contains("@@")),
            "the hunks the fold opened are on screen, got {rows:?}"
        );
    }

    #[test]
    fn test_expanding_a_file_reads_its_diff_from_the_right_side() {
        // A staged file's diff is the index one; asking for the working-tree
        // diff there would show changes that are not staged.
        let mut page = loaded_page();
        cursor_on(&mut page, "staged.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Tab)));
        assert!(
            command.args.contains(&"--cached".to_string()),
            "{:?}",
            command.args
        );

        cursor_on(&mut page, "working.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Tab)));
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
        let mut page = loaded_page();
        cursor_on(&mut page, "new.rs");
        let command = command_of(&page.on_key(&press(KeyCode::Tab)));
        assert!(
            command.args.contains(&"--no-index".to_string()),
            "{:?}",
            command.args
        );
    }

    #[test]
    fn test_an_expanded_file_lists_its_hunks_and_lines() {
        let page = page_with_diff();
        let hunks: Vec<usize> = page
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
        let page = page_with_diff();
        let added = page
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("new();"))
            .expect("the added line");
        assert!(
            added
                .spans
                .iter()
                .any(|span| span.style == PageStyle::AddedEdit),
            "got {:?}",
            added.spans
        );
        assert!(
            added
                .spans
                .iter()
                .any(|span| span.style == PageStyle::Added),
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
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        page.on_key(&press(KeyCode::Tab));
        page.on_job(reply(exec::TAG_DIFF, &text));
        let added = page
            .rows
            .iter()
            .position(|row| row_text(&row.spans).contains(&format!("{long}y")))
            .expect("the added line");
        page.cursor = added;
        // Stated relative to the cursor's row index: the status header is a
        // labelled block whose height can change, and an absolute screen row
        // here would be measuring the header rather than the wrapping.
        let flat = page.content(20, 80, false);
        assert_eq!(
            flat.cursor_line,
            Some(added),
            "unwrapped, every row is one screen row, so the band sits on its own index"
        );
        let wrapped = page.content(20, 32, true);
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
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('d')));
        let outcome = page.on_key(&press(KeyCode::Char('d')));
        assert_eq!(command_of(&outcome).args[0], "diff");
        page.on_job(reply(exec::TAG_DIFF_VIEW, DIFF_OUTPUT));
        let output = page.output.as_ref().expect("the diff view");
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
                .any(|span| span.style == PageStyle::AddedEdit),
            "got {added:?}"
        );
        assert!(
            added
                .spans
                .iter()
                .any(|span| span.style == PageStyle::Added),
            "got {added:?}"
        );
    }

    #[test]
    fn test_staging_on_a_hunk_applies_that_hunk_alone() {
        // The whole point of hunk staging: the second hunk must not be in the
        // patch, and the patch goes in on standard input.
        let mut page = page_with_diff();
        cursor_on_hunk(&mut page, 0);
        let command = command_of(&page.on_key(&press(KeyCode::Char('s'))));
        assert_eq!(command.args, ["apply", "--unidiff-zero", "--cached", "-"]);
        let patch = command.stdin.clone().expect("the patch");
        assert!(patch.contains("-    old();"), "got {patch}");
        assert!(!patch.contains("added()"), "got {patch}");
    }

    #[test]
    fn test_a_key_inside_a_hunk_acts_on_that_hunk() {
        // Only the header row would be a cruel target; every line of the hunk
        // stands for it.
        let mut page = page_with_diff();
        let line = page
            .rows
            .iter()
            .enumerate()
            .filter(|(_, row)| matches!(&row.item, Item::Hunk(hunk) if hunk.index == 1))
            .nth(1)
            .map(|(index, _)| index)
            .expect("a line inside the second hunk");
        page.cursor = line;
        let command = command_of(&page.on_key(&press(KeyCode::Char('s'))));
        let patch = command.stdin.clone().expect("the patch");
        assert!(patch.contains("+    added();"), "got {patch}");
    }

    #[test]
    fn test_unstaging_a_hunk_reverses_it_out_of_the_index() {
        let mut page = page_with_diff();
        cursor_on_hunk(&mut page, 0);
        let command = command_of(&page.on_key(&press(KeyCode::Char('u'))));
        assert_eq!(
            command.args,
            ["apply", "--unidiff-zero", "--cached", "--reverse", "-"]
        );
    }

    #[test]
    fn test_discarding_a_hunk_reverses_it_out_of_the_working_tree() {
        let mut page = page_with_diff();
        cursor_on_hunk(&mut page, 0);
        page.on_key(&press(KeyCode::Char('x')));
        let command = command_of(&page.on_prompt(PromptReply {
            answer: Some("y".to_string()),
            tag: ASK_DISCARD,
        }));
        assert_eq!(command.args, ["apply", "--unidiff-zero", "--reverse", "-"]);
    }

    #[test]
    fn test_a_new_status_drops_the_diffs_it_invalidated() {
        // Staging a hunk changes the file's diff; applying a patch built from
        // the old one would hit the wrong lines.
        let mut page = page_with_diff();
        assert!(!page.diffs.is_empty());
        page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        assert!(page.diffs.is_empty(), "the stale diff is gone");
    }

    #[test]
    fn test_a_refresh_reads_everything_the_view_shows_in_one_chain() {
        // Each read asks for the next, so one refresh fills the whole view
        // rather than five jobs racing each other into it.
        let mut page = page_with_diff();
        page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        let after_log = page.on_job(reply(exec::TAG_LOG, ""));
        assert_eq!(command_of(&after_log).tag, exec::TAG_STASHES);
        let after_stashes = page.on_job(reply(exec::TAG_STASHES, ""));
        assert_eq!(command_of(&after_stashes).tag, exec::TAG_UNPULLED);

        // The chain ends on the state files, which no command reports.
        let after_unpulled = page.on_job(reply(exec::TAG_UNPULLED, ""));
        let PageOutcome::Job(JobRequest::ReadFiles(paths)) = after_unpulled else {
            panic!("expected the state files, got {after_unpulled:?}");
        };
        assert!(paths.iter().any(|path| path.ends_with("MERGE_HEAD")));

        // And the open diff is what still needs reading.
        let stale = page.request_stale_diff();
        assert_eq!(command_of(&stale).tag, exec::TAG_DIFF);
    }

    #[test]
    fn test_a_file_the_status_no_longer_lists_stops_being_expanded() {
        let mut page = page_with_diff();
        page.on_job(reply(
            exec::TAG_STATUS,
            "# branch.head main
",
        ));
        assert!(page.expanded.is_empty());
    }

    #[test]
    fn test_folding_an_expanded_file_hides_its_diff_without_re_reading() {
        let mut page = page_with_diff();
        cursor_on(&mut page, "working.rs");
        let outcome = page.on_key(&press(KeyCode::Tab));
        assert_eq!(outcome, PageOutcome::Consumed, "nothing to run");
        assert!(!page
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

    fn choose(page: &mut GitPage, opener: char, choice: char) -> PageOutcome {
        page.on_key(&press_char(opener));
        page.on_key(&press_char(choice))
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
    fn search(page: &mut GitPage, text: &str) {
        let outcome = page.on_key(&press_char('/'));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_SEARCH);
        page.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        });
    }

    fn row_under_cursor(page: &GitPage) -> String {
        page.rows
            .get(page.cursor)
            .map(|row| row_text(&row.spans))
            .unwrap_or_default()
    }

    #[test]
    fn test_a_menu_key_opens_a_menu_and_hands_it_to_the_host_to_draw() {
        // The menu is the host's own key-hint card, centred over the window,
        // so the view keeps every row of the pane to itself.
        let mut page = loaded_page();
        page.on_key(&press_char('z'));
        assert_eq!(page.popup, Some(Popup::Stash));

        let hint = page.hint().expect("a hint while the menu is open");
        assert_eq!(hint.title, "Stash");
        assert!(
            hint.items
                .iter()
                .any(|(key, what)| key == "p" && what == "pop the newest"),
            "got {:?}",
            hint.items
        );

        let painted = page.content(80, 80, false);
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
        let mut page = loaded_page();
        let outcome = choose(&mut page, 'z', '@');
        assert_eq!(outcome, PageOutcome::Consumed);
        assert_eq!(page.popup, None);
    }

    #[test]
    fn test_stash_choices_run_the_matching_command() {
        let mut page = loaded_page();
        assert_eq!(
            command_of(&choose(&mut page, 'z', 'p')).args,
            ["stash", "pop"]
        );
        assert_eq!(
            command_of(&choose(&mut page, 'z', 'd')).args,
            ["stash", "drop"]
        );
        assert_eq!(
            command_of(&choose(&mut page, 'z', 'l')).args,
            ["stash", "list"]
        );
    }

    #[test]
    fn test_a_branch_choice_asks_for_the_name_then_runs() {
        let mut page = loaded_page();
        let PageOutcome::Prompt(request) = choose(&mut page, 'b', 'c') else {
            panic!("expected a prompt");
        };
        let command = command_of(&page.on_prompt(PromptReply {
            answer: Some("feature".to_string()),
            tag: request.tag,
        }));
        assert_eq!(command.args, ["checkout", "-b", "feature"]);
    }

    #[test]
    fn test_an_empty_answer_runs_nothing() {
        // `git checkout -b ""` fails with a confusing message; not running is
        // the better answer.
        let mut page = loaded_page();
        let PageOutcome::Prompt(request) = choose(&mut page, 'b', 'c') else {
            panic!("expected a prompt");
        };
        let outcome = page.on_prompt(PromptReply {
            answer: Some("   ".to_string()),
            tag: request.tag,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
    }

    #[test]
    fn test_a_stash_message_may_be_empty_because_it_is_optional() {
        let mut page = loaded_page();
        let PageOutcome::Prompt(request) = choose(&mut page, 'z', 'z') else {
            panic!("expected a prompt");
        };
        let command = command_of(&page.on_prompt(PromptReply {
            answer: Some(String::new()),
            tag: request.tag,
        }));
        assert_eq!(command.args, ["stash", "push"], "no empty --message");
    }

    #[test]
    fn test_committing_in_an_editor_goes_to_a_pane_not_a_captured_command() {
        // A captured `git commit` would hang forever waiting on an editor that
        // has no terminal.
        let mut page = loaded_page();
        let outcome = choose(&mut page, 'c', 'c');
        let PageOutcome::Spawn(request) = outcome else {
            panic!("expected a spawn, got {outcome:?}");
        };
        assert_eq!(request.args, ["commit"]);
        assert_eq!(request.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn test_an_interactive_rebase_also_goes_to_a_pane() {
        let mut page = loaded_page();
        // The branch to rebase onto is chosen from a list, so the key asks
        // for the names first and the answer arrives under the same tag.
        let listing = choose(&mut page, 'r', 'i');
        assert_eq!(command_of(&listing).tag, exec::TAG_CANDIDATES);
        let PageOutcome::Pick(request) =
            page.on_job(reply(exec::TAG_CANDIDATES, "main\nfeature\n"))
        else {
            panic!("expected a list to pick from");
        };
        let outcome = page.on_prompt(PromptReply {
            answer: Some("HEAD~3".to_string()),
            tag: request.tag,
        });
        let PageOutcome::Spawn(spawn) = outcome else {
            panic!("expected a spawn, got {outcome:?}");
        };
        assert_eq!(spawn.args, ["rebase", "--interactive", "HEAD~3"]);
    }

    #[test]
    fn test_a_remote_add_answer_splits_into_name_and_url() {
        let mut page = loaded_page();
        let PageOutcome::Prompt(request) = choose(&mut page, 'M', 'a') else {
            panic!("expected a prompt");
        };
        let command = command_of(&page.on_prompt(PromptReply {
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
        let mut page = loaded_page();
        let PageOutcome::Prompt(request) = choose(&mut page, 'M', 'a') else {
            panic!("expected a prompt");
        };
        let outcome = page.on_prompt(PromptReply {
            answer: Some("justaname".to_string()),
            tag: request.tag,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
        assert!(page
            .message
            .clone()
            .unwrap_or_default()
            .starts_with("failed:"));
    }

    #[test]
    fn test_enter_on_a_commit_reads_it() {
        // Enter opens the file under the cursor, and a commit row has no file:
        // it used to be swallowed, so a commit was the one row Enter did
        // nothing on.
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("a commit row");
        let command = command_of(&page.on_key(&press(KeyCode::Enter)));
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
        let mut page = loaded_page();
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
            page.on_job(reply(exec::TAG_SHOW, &shown)),
            PageOutcome::Consumed
        );
        let output = page
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
        let mut page = loaded_page();
        assert_eq!(
            page.on_job(reply(exec::TAG_SHOW, &shown)),
            PageOutcome::Consumed
        );
        // A commit opens shut, so open all of it to read what it paints.
        page.on_key(&shift(KeyCode::Tab));
        let output = page
            .output
            .as_ref()
            .expect("the commit replaced the status view");
        assert_eq!(
            output.rows[0].spans,
            vec![PageSpan::new(PageStyle::Header, "commit abc1234567890")]
        );
        let band = output
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("src/a.rs"))
            .expect("the file band");
        assert!(
            band.spans
                .iter()
                .any(|span| span.style == PageStyle::Section),
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
            .any(|span| span.style == PageStyle::AddedEdit));
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
    fn output_cursor_on(page: &mut GitPage, needle: &str) {
        let output = page.output.as_mut().expect("an output view");
        output.cursor = output
            .lines
            .iter()
            .position(|line| line.contains(needle))
            .unwrap_or_else(|| panic!("no output row for {needle}"));
    }

    fn output_has(page: &GitPage, needle: &str) -> bool {
        page.output
            .as_ref()
            .is_some_and(|output| output.lines.iter().any(|line| line.contains(needle)))
    }

    #[test]
    fn test_tab_on_a_commits_heading_shuts_every_file_under_it() {
        // The heading is the commit's own section, the way the working tree's
        // sections are: one key takes the whole file list away and brings it
        // back, with the count left saying what is behind it.
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
        assert!(
            output_has(&page, "src/a.rs"),
            "the files show to begin with"
        );

        output_cursor_on(&mut page, "Changes");
        assert_eq!(page.on_key(&press(KeyCode::Tab)), PageOutcome::Consumed);
        assert!(
            !output_has(&page, "src/a.rs") && !output_has(&page, "src/b.rs"),
            "the file list is away"
        );
        assert!(
            output_has(&page, "Changes (2)"),
            "the heading stays, still counting what it holds"
        );

        page.on_key(&press(KeyCode::Tab));
        assert!(
            output_has(&page, "src/a.rs") && output_has(&page, "src/b.rs"),
            "and comes back"
        );
    }

    #[test]
    fn test_a_commit_opens_shut_and_tab_opens_it_one_layer_at_a_time() {
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        assert!(
            output_has(&page, "src/a.rs"),
            "the commit opens as the files it touched"
        );
        assert!(
            !output_has(&page, "@@") && !output_has(&page, "new();"),
            "with neither their hunks nor their diffs"
        );

        output_cursor_on(&mut page, "src/a.rs");
        let at = page.output.as_ref().unwrap().cursor;
        assert_eq!(page.on_key(&press(KeyCode::Tab)), PageOutcome::Consumed);
        assert!(
            output_has(&page, "@@"),
            "the file opens to its hunk headers"
        );
        assert!(
            !output_has(&page, "new();"),
            "which are themselves still shut"
        );
        assert_eq!(
            page.output.as_ref().unwrap().cursor,
            at,
            "the cursor stays on the band it acted on"
        );

        output_cursor_on(&mut page, "@@ -1,2 +1,2 @@");
        page.on_key(&press(KeyCode::Tab));
        assert!(output_has(&page, "new();"), "the hunk opens to its body");

        output_cursor_on(&mut page, "src/a.rs");
        page.on_key(&press(KeyCode::Tab));
        assert!(
            !output_has(&page, "@@"),
            "the file shuts all of it away again"
        );
        assert!(
            output_has(&page, "src/a.rs"),
            "its band stays, so it can be opened again"
        );
    }

    #[test]
    fn test_tab_on_a_commits_hunk_folds_only_that_hunk() {
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        page.on_key(&shift(KeyCode::Tab));

        output_cursor_on(&mut page, "@@ -1,2 +1,2 @@");
        page.on_key(&press(KeyCode::Tab));

        assert!(
            output_has(&page, "@@ -1,2 +1,2 @@"),
            "a folded hunk keeps its header"
        );
        assert!(!output_has(&page, "new();"), "but drops its body");
        assert!(output_has(&page, "third();"), "the other hunk is untouched");
    }

    #[test]
    fn test_tab_on_a_commits_summary_does_nothing() {
        // The summary has nothing to fold, so Tab must not disturb the view.
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        output_cursor_on(&mut page, "do the thing");
        let before = page.output.as_ref().unwrap().lines.clone();

        assert_eq!(page.on_key(&press(KeyCode::Tab)), PageOutcome::Consumed);
        assert_eq!(
            page.output.as_ref().unwrap().lines,
            before,
            "the view is unchanged"
        );
    }

    #[test]
    fn test_shift_tab_folds_every_file_of_a_commit_and_opens_them_again() {
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
        assert!(
            !output_has(&page, "new();") && !output_has(&page, "third();"),
            "the commit opens shut"
        );

        assert_eq!(page.on_key(&shift(KeyCode::Tab)), PageOutcome::Consumed);
        assert!(
            output_has(&page, "new();") && output_has(&page, "third();"),
            "one key opens every file and hunk of it"
        );

        page.on_key(&shift(KeyCode::Tab));
        assert!(
            !output_has(&page, "new();") && !output_has(&page, "third();"),
            "a second Shift-Tab shuts all of it again"
        );
        assert!(
            output_has(&page, "src/a.rs") && output_has(&page, "src/b.rs"),
            "the bands stay, so the commit still says what it touched"
        );
    }

    #[test]
    fn test_shift_tab_opens_a_part_way_folded_commit_rather_than_shutting_it() {
        // One hunk folded by hand means the view is already part-way shut, so
        // the key that acts on all of it opens it rather than folding further.
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        page.on_key(&shift(KeyCode::Tab));
        output_cursor_on(&mut page, "@@ -1,2 +1,2 @@");
        page.on_key(&press(KeyCode::Tab));
        assert!(!output_has(&page, "new();"), "that hunk's body is away");

        page.on_key(&shift(KeyCode::Tab));
        assert!(
            output_has(&page, "new();") && output_has(&page, "third();"),
            "every hunk is back"
        );
    }

    #[test]
    fn test_shift_tab_leaves_an_output_view_with_nothing_to_fold_alone() {
        // A blame, a log, or a listing is the lines it came back as: there is
        // no commit under it to fold, and the key must not disturb the view.
        let mut page = loaded_page();
        page.on_job(reply(
            exec::TAG_BLAME,
            "abc1234 (Someone 2026-09-18) fn main() {",
        ));
        let before = page.output.as_ref().expect("the blame").lines.clone();

        assert_eq!(page.on_key(&shift(KeyCode::Tab)), PageOutcome::Consumed);
        assert_eq!(page.output.as_ref().unwrap().lines, before);
    }

    #[test]
    fn test_a_commits_file_band_carries_an_icon() {
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        let output = page.output.as_ref().expect("the commit view");
        let band = output
            .rows
            .iter()
            .find(|row| row_text(&row.spans).contains("src/a.rs"))
            .expect("the file band");
        assert_eq!(
            band.icon.as_ref().map(|icon| &icon.kind),
            Some(&crate::model::page::PageIconKind::File {
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
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_HUNKS));
        page.on_key(&shift(KeyCode::Tab));
        let drawn = |page: &mut GitPage| -> String {
            page.content(40, 100, false)
                .rows
                .iter()
                .map(row_text)
                .collect::<Vec<String>>()
                .join("\n")
        };
        assert!(drawn(&mut page).contains("new();"));

        output_cursor_on(&mut page, "src/a.rs");
        page.on_key(&press(KeyCode::Tab));
        let after = drawn(&mut page);
        assert!(
            !after.contains("new();"),
            "the pane draws the folded view, got {after:?}"
        );
        assert!(after.contains("src/a.rs"), "the band is still drawn");
    }

    #[test]
    fn test_an_empty_commit_read_says_so_instead_of_blanking_the_view() {
        let mut page = loaded_page();
        page.on_job(reply(
            exec::TAG_SHOW,
            "   
",
        ));
        assert!(page.output.is_none(), "the status view stays");
        assert_eq!(page.message.as_deref(), Some("nothing to show"));
    }

    #[test]
    fn test_reset_on_a_commit_uses_that_commit_without_asking() {
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("a commit row");
        let command = command_of(&page.on_key(&press_char('o')));
        assert_eq!(command.args, ["reset", "--mixed", "abc1234"]);
    }

    #[test]
    fn test_reset_away_from_a_commit_asks_which_one() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let PageOutcome::Prompt(request) = page.on_key(&press_char('o')) else {
            panic!("expected a prompt");
        };
        assert_eq!(request.tag, ASK_RESET_MIXED);
    }

    #[test]
    fn test_a_hard_reset_says_what_it_costs() {
        let mut page = loaded_page();
        let PageOutcome::Prompt(request) = choose(&mut page, 'O', 'h') else {
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
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| matches!(row.item, Item::Commit(_)))
            .expect("a commit row");
        assert_eq!(
            page.on_key(&press_char('y')),
            PageOutcome::Yank("abc1234".to_string())
        );

        cursor_on(&mut page, "working.rs");
        assert_eq!(
            page.on_key(&press_char('y')),
            PageOutcome::Yank("working.rs".to_string())
        );
    }

    #[test]
    fn test_ignoring_by_extension_writes_a_wildcard() {
        let tree = std::env::temp_dir().join(format!("winter-gitignore-{}", std::process::id()));
        std::fs::create_dir_all(&tree).expect("temp dir");
        let mut page = GitPage::new(tree.clone());
        page.on_job(reply(exec::TAG_ROOT, &format!("{}\n", tree.display())));
        page.on_job(reply(exec::TAG_STATUS, STATUS_OUTPUT));
        cursor_on(&mut page, "working.rs");

        choose(&mut page, 'i', 'e');
        let written = std::fs::read_to_string(tree.join(".gitignore")).unwrap_or_default();
        assert_eq!(written.trim(), "*.rs");
        std::fs::remove_dir_all(&tree).ok();
    }

    #[test]
    fn test_a_log_fills_the_view_and_can_ask_for_more() {
        let mut page = loaded_page();
        choose(&mut page, 'l', 'l');
        let outcome = page.on_job(reply(
            exec::TAG_LOG_VIEW,
            &log_output(&[("aaa", "one"), ("bbb", "two")]),
        ));
        assert_eq!(outcome, PageOutcome::Consumed);
        assert!(page.output.is_some(), "the log replaced the status view");

        let before = page.log_count;
        let more = page.on_key(&press_char('+'));
        assert!(page.log_count > before, "asked for more");
        assert_eq!(command_of(&more).tag, exec::TAG_LOG_VIEW);
    }

    #[test]
    fn test_q_leaves_the_log_for_the_status_view_rather_than_closing() {
        // Closing the whole tool on `q` from a log would lose the view the user
        // came from.
        let mut page = loaded_page();
        choose(&mut page, 'l', 'l');
        page.on_job(reply(exec::TAG_LOG_VIEW, &log_output(&[("aaa", "one")])));
        let outcome = page.on_key(&press_char('q'));
        assert_eq!(outcome, PageOutcome::Consumed);
        assert!(page.output.is_none());
        assert_eq!(page.on_key(&press_char('q')), PageOutcome::Close);
    }

    #[test]
    fn test_read_only_output_with_nothing_in_it_says_so() {
        let mut page = loaded_page();
        page.on_job(reply(exec::TAG_READ, "\n"));
        assert!(page.output.is_none());
        assert_eq!(page.message.as_deref(), Some("nothing to show"));
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
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        assert_eq!(page.on_key(&press_char('a')), PageOutcome::Consumed);
        assert_eq!(page.on_key(&press_char('-')), PageOutcome::Consumed);
    }

    #[test]
    fn test_shift_tab_still_folds_while_a_menu_has_never_opened() {
        let mut page = loaded_page();
        page.on_key(&shift(KeyCode::Tab));
        assert!(!page
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
    }

    #[test]
    fn test_a_menu_shows_however_long_the_view_is() {
        // The card is drawn over the window rather than under the rows, so a
        // status longer than the pane cannot push it off the bottom, which is
        // where a menu drawn into the view ended up.
        let mut page = loaded_page();
        page.on_key(&press_char('b'));
        let rows = 4;
        let painted = page.content(rows, 80, false);

        assert!(page.hint().is_some(), "the menu is there to draw");
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
        let mut page = loaded_page();
        assert_eq!(page.on_key(&press_char('g')), PageOutcome::Consumed);
        let hint = page.hint().expect("a hint while the leader is open");
        assert_eq!(hint.title, "Jump");
        assert!(
            hint.items
                .iter()
                .any(|(key, what)| key == "s" && what == "staged changes"),
            "got {:?}",
            hint.items
        );

        // And the second key still jumps.
        page.on_key(&press_char('s'));
        assert_eq!(
            page.selected().map(|row| row.item.clone()),
            Some(Item::Heading(Section::Staged))
        );
    }

    #[test]
    fn test_switching_branch_offers_the_branches_rather_than_a_blank_line() {
        let mut page = loaded_page();
        let listing = choose(&mut page, 'b', 'b');
        let command = command_of(&listing);
        assert_eq!(command.tag, exec::TAG_CANDIDATES);
        assert_eq!(command.args[0], "for-each-ref");
        assert!(
            command.args.iter().any(|arg| arg == "refs/remotes"),
            "a checkout takes a remote branch too, got {:?}",
            command.args
        );

        let outcome = page.on_job(reply(
            exec::TAG_CANDIDATES,
            "main\nfeature\norigin/main\norigin/HEAD\n",
        ));
        let PageOutcome::Pick(request) = outcome else {
            panic!("expected a list to pick from, got {outcome:?}");
        };
        assert_eq!(request.label, "Checkout");
        assert_eq!(request.items, ["main", "feature", "origin/main"]);

        // Choosing one answers the question the key asked.
        let ran = page.on_prompt(PromptReply {
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
        let mut page = loaded_page();
        let command = command_of(&choose(&mut page, 'b', 'd'));
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
        let mut page = loaded_page();
        choose(&mut page, 't', 'd');
        let outcome = page.on_job(reply(exec::TAG_CANDIDATES, "\n"));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.label, "Delete tag: ");
    }

    #[test]
    fn test_the_file_popup_acts_on_the_row_it_was_opened_over() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        assert_eq!(page.on_key(&press_char('.')), PageOutcome::Consumed);
        assert_eq!(page.popup, Some(Popup::File), "the menu is open");

        let command = command_of(&page.on_key(&press_char('b')));
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
        let mut page = loaded_page();
        page.cursor = 0;
        assert_eq!(page.on_key(&press_char('.')), PageOutcome::Consumed);
        assert_eq!(page.popup, None);
        assert_eq!(page.message.as_deref(), Some("no file here"));
    }

    #[test]
    fn test_enter_on_a_stash_reads_it_as_a_diff() {
        let mut page = loaded_page();
        page.on_job(reply(
            exec::TAG_STASHES,
            "stash@{0}\u{1f}WIP on main: a thing\n",
        ));
        let at = page
            .rows
            .iter()
            .position(|row| row.item == Item::Stash(0))
            .expect("the stash row");
        page.cursor = at;

        let command = command_of(&page.on_key(&press(KeyCode::Enter)));
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
        let mut page = loaded_page();
        page.cursor = 0;
        page.on_key(&alt_char('>'));
        assert_eq!(page.cursor, page.rows.len() - 1, "the last row");
        page.on_key(&alt_char('<'));
        assert_eq!(page.cursor, 0, "and back to the first");

        page.on_job(reply(exec::TAG_SHOW, SHOWN_TWO_FILES));
        page.on_key(&alt_char('>'));
        let output = page.output.as_ref().expect("the commit view");
        assert_eq!(output.cursor, output.lines.len() - 1);
        page.on_key(&alt_char('<'));
        assert_eq!(page.output.as_ref().unwrap().cursor, 0);
    }

    #[test]
    fn test_the_shared_word_motions_step_between_entities() {
        // `w`/`b` land on the things the view rows stand for, stepping over
        // the blank separators, through the shared Vim layer.
        let mut page = loaded_page();
        page.cursor = 0;
        page.on_key(&press(KeyCode::Char('w')));
        assert!(
            page.selected().is_some_and(|row| row.item != Item::None),
            "never lands on a separator"
        );
        page.on_key(&press(KeyCode::Char('b')));
        assert!(
            page.selected().is_some_and(|row| row.item != Item::None),
            "and back, the same way"
        );
    }

    #[test]
    fn test_the_braces_jump_between_section_headings() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('}')));
        assert!(matches!(
            page.selected().map(|row| &row.item),
            Some(Item::Heading(_) | Item::RecentHeading)
        ));
        let first = page.cursor;
        page.on_key(&press(KeyCode::Char('}')));
        assert!(page.cursor > first, "each brace reaches the next heading");
        page.on_key(&press(KeyCode::Char('{')));
        assert_eq!(page.cursor, first, "and back to it");
    }

    #[test]
    fn test_gg_and_the_paging_motions_work_over_the_view() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('g')));
        assert_eq!(page.cursor, 0, "gg through the view's own g leader");

        page.on_key(&press(KeyCode::Char('$')));
        assert_eq!(page.cursor, page.rows.len() - 1, "$ is the last row");

        page.on_key(&press(KeyCode::Char('0')));
        assert_eq!(page.cursor, 0);
        page.content(10, 80, false);
        page.on_key(&ctrl(KeyCode::Char('d')));
        assert_eq!(page.cursor, 4, "half the viewport down");
        page.on_key(&ctrl(KeyCode::Char('u')));
        assert_eq!(page.cursor, 0, "and back up");
    }

    #[test]
    fn test_the_views_own_keys_still_win_over_the_shared_layer() {
        // The override contract: what the view claims never reaches the
        // shared layer. `G` refreshes rather than going to the bottom, and
        // `g` opens the section leader rather than arming `gg` there.
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('$')));
        let before = page.rows.len();
        page.on_key(&press(KeyCode::Char('G')));
        assert!(page.rows.len() >= before, "`G` refreshed, it did not move");
    }

    #[test]
    fn test_the_g_keys_jump_between_sections() {
        let mut page = loaded_page();
        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('s')));
        assert_eq!(
            page.selected().map(|row| &row.item),
            Some(&Item::Heading(Section::Staged))
        );

        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('t')));
        assert_eq!(
            page.selected().map(|row| &row.item),
            Some(&Item::Heading(Section::Untracked))
        );
    }

    #[test]
    fn test_entity_motion_skips_the_blank_lines_between_sections() {
        // Plain `j` walks through the gaps; `Alt-n` is what steps from one
        // thing to the next thing.
        let mut page = loaded_page();
        page.cursor = 0;
        page.on_key(&Key {
            alt: true,
            code: KeyCode::Char('n'),
            ctrl: false,
            shift: false,
        });
        assert_ne!(page.selected().map(|row| &row.item), Some(&Item::None));
    }

    #[test]
    fn test_folding_a_section_hides_its_files_but_not_the_others() {
        let mut page = loaded_page();
        page.cursor = page
            .rows
            .iter()
            .position(|row| row.item == Item::Heading(Section::Untracked))
            .expect("the untracked heading");
        page.on_key(&press(KeyCode::Tab));

        let listed: Vec<String> = page
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
        let mut page = loaded_page();
        let shift_tab = Key {
            alt: false,
            code: KeyCode::Tab,
            ctrl: false,
            shift: true,
        };
        page.on_key(&shift_tab);
        assert!(!page
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
        page.on_key(&shift_tab);
        assert!(page
            .rows
            .iter()
            .any(|row| matches!(row.item, Item::File(_))));
    }

    #[test]
    fn test_a_push_runs_from_the_repository_root() {
        let mut page = loaded_page();
        let command = command_of(&page.on_key(&press(KeyCode::Char('P'))));
        assert_eq!(command.args, ["push"]);
        assert_eq!(command.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn test_a_search_moves_the_cursor_to_the_row_holding_the_text() {
        let mut page = loaded_page();
        search(&mut page, "working");
        assert!(row_under_cursor(&page).contains("working.rs"));
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let mut page = loaded_page();
        search(&mut page, ".rs");
        let first = row_under_cursor(&page);

        page.on_key(&press_char('n'));
        let second = row_under_cursor(&page);
        assert_ne!(first, second, "the next match is a different row");

        page.on_key(&press_char('N'));
        assert_eq!(row_under_cursor(&page), first, "and back again");
    }

    #[test]
    fn test_a_search_walks_the_output_view_while_one_is_up() {
        // The status rows are not on screen here, so searching them would move a
        // cursor nobody can see and leave the visible one where it was.
        let mut page = loaded_page();
        choose(&mut page, 'l', 'l');
        page.on_job(reply(
            exec::TAG_LOG_VIEW,
            &log_output(&[("aaa", "one"), ("bbb", "two"), ("ccc", "three")]),
        ));
        let before = page.cursor;

        search(&mut page, "ccc");
        // Located rather than hardcoded: the log view heads its list, and each
        // commit takes an identity row and an authorship row, so a literal index
        // here would only re-encode the current layout.
        let expected = page
            .output
            .as_ref()
            .and_then(|output| output.lines.iter().position(|line| line.contains("ccc")));
        assert!(expected.is_some(), "the log lists the commit searched for");
        assert_eq!(page.output.as_ref().map(|output| output.cursor), expected);
        assert_eq!(page.cursor, before);
    }

    #[test]
    fn test_a_search_that_finds_nothing_reports_it_instead_of_going_quiet() {
        let mut page = loaded_page();
        page.on_key(&press_char('n'));
        assert_eq!(page.message.as_deref(), Some("no search"));

        search(&mut page, "absent");
        assert_eq!(page.message.as_deref(), Some("not found: absent"));
    }

    #[test]
    fn test_a_search_is_answered_before_git_has_said_where_the_root_is() {
        // Every other question needs a repository and is dropped without one,
        // which would swallow a search the user typed while the status loaded.
        let mut page = GitPage::new(PathBuf::from("/repo"));
        search(&mut page, "anything");
        assert_eq!(page.message.as_deref(), Some("not found: anything"));
    }

    #[test]
    fn test_blame_runs_against_the_file_under_the_cursor() {
        let mut page = loaded_page();
        cursor_on(&mut page, "working.rs");
        let command = command_of(&page.on_key(&alt_char('b')));
        assert_eq!(command.tag, exec::TAG_BLAME);
        assert_eq!(command.args.last().map(String::as_str), Some("working.rs"));
        assert_eq!(command.cwd, PathBuf::from("/repo"));
    }

    #[test]
    fn test_blame_off_a_file_says_there_is_nothing_to_blame() {
        // A heading and a commit have no single path, and running blame on the
        // repository root would report a git error the user did not ask for.
        let mut page = loaded_page();
        page.cursor = 0;
        assert_eq!(page.on_key(&alt_char('b')), PageOutcome::Consumed);
        assert_eq!(page.message.as_deref(), Some("no file to blame here"));
    }

    #[test]
    fn test_a_blame_fills_the_view_under_its_own_title() {
        let mut page = loaded_page();
        page.on_job(reply(
            exec::TAG_BLAME,
            "abc12345 (me 2026-01-01 1) fn main\n",
        ));
        assert_eq!(
            page.output.as_ref().map(|output| output.title.as_str()),
            Some("Blame")
        );
    }

    #[test]
    fn test_keys_before_the_root_is_known_do_nothing() {
        // The view is up as soon as the key is pressed, so every command has to
        // survive being asked before git has answered.
        let mut page = GitPage::new(PathBuf::from("/repo"));
        for code in [
            KeyCode::Char('s'),
            KeyCode::Char('u'),
            KeyCode::Char('P'),
            KeyCode::Char('F'),
            KeyCode::Char('f'),
        ] {
            assert_eq!(page.on_key(&press(code)), PageOutcome::Consumed);
        }
    }
}

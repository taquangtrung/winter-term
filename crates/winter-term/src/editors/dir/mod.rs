//! Dir: a keyboard-driven directory listing in a pane.
//!
//! - [`entry`]: what a listing is made of.
//! - [`listing`]: ordering and filtering.
//! - [`marks`]: which entries an operation acts on.
//! - [`ops`]: creating, moving, copying, and deleting.
//! - [`rows`]: painting a listing.
//! - [`source`]: reading the filesystem.
//! - [`trash`]: the desktop trash deletes go to by default.
//! - [`tree`]: expanded directories and row depth.
//! - [`vcs`]: what git says about each entry listed.
//!
//! The listing itself is split by what it does: `keys` (a key press),
//! `transfer` (the operations on entries and the questions they ask), `edit`
//! (renaming in place), and `sizes` (directory totals).

mod edit;
pub mod entry;
mod keys;
pub mod listing;
pub mod marks;
pub mod ops;
pub mod rows;
mod sizes;
pub mod source;
mod transfer;
pub mod trash;
pub mod tree;
pub mod vcs;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use crate::editors::background::{self, Background};
use crate::editors::list::ListView;
use crate::model::editor::{
    Editor, EditorMenuItem, EditorOutcome, EditorStatus, JobReply, JobRequest, JumpTarget, Notice,
    OpenTarget, PromptMode, PromptReply, PromptRequest, SearchReport,
};
use crate::model::input::table::KeyHelp;
use crate::model::input::CursorMove;
use crate::model::input::Key;
use crate::model::paint::{row_height, row_text, Frame, GutterSide, Icon, RowWindow, Span, Style};
use crate::model::path::{abbreviate_home, format_full_path, normalize_path};
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditMode;
use crate::model::vim::nav::VimNav;

use listing::SortKey;
use marks::Marks;
use source::{Listing, Opening, ReadPlan};
use tree::{Folds, Row};
use vcs::GitMarks;

// ========================================================================
// Constants
// ========================================================================

/// Rows of header above the first entry.
const HEADER_ROWS: usize = 1;

/// Shown in place of the listing when a directory has nothing to show.
const EMPTY_NOTE: &str = "  (empty)";

/// Shown in place of the listing while the first read of it is under way.
const READING_NOTE: &str = "  reading...";

/// What the thread reading a listing is called.
const READER_THREAD: &str = "winter dir read";

/// How long a key waits for the listing it asked for before the frame is
/// drawn without it. A local directory reads well inside this, so its rows
/// land with the key; a slow mount or a huge tree finishes on its own while
/// the window stays live.
#[cfg(not(test))]
const LISTING_WAIT: Duration = Duration::from_millis(40);

/// Long enough that a test's read always lands with the key that asked for
/// it, as a fast disk's does, so tests read the listing straight away.
#[cfg(test)]
const LISTING_WAIT: Duration = Duration::from_secs(10);

/// Prompt tags, one per question the listing asks.
const ASK_COPY: &str = "copy";
const ASK_APPLY_EDITS: &str = "apply-edits";
const ASK_DELETE: &str = "delete";
const ASK_TRASH: &str = "trash";
const ASK_MKDIR: &str = "mkdir";
const ASK_MODE: &str = "mode";
const ASK_MOVE: &str = "move";
const ASK_NEW_FILE: &str = "new-file";
const ASK_RENAME: &str = "rename";
const ASK_SEARCH: &str = "search";

/// Directories remembered in each direction of the visit history. Matches the
/// jumplist's own depth, for the same reason: enough to walk back through a
/// session's wandering, bounded so it cannot grow without limit.
const MAX_HISTORY: usize = 100;

// ========================================================================
// Data Structures
// ========================================================================

/// A directory listing the keyboard drives: move, fold, descend, and open.
#[derive(Debug)]
pub struct DirEditor {
    /// Directories left behind, most recent last.
    back: Vec<PathBuf>,
    /// The names as editable text while renaming entries in place, in the
    /// manner of Emacs' `wdired`: one line per row, Vim keys over them and
    /// Emacs keys while typing, as in the editor.
    edit: Option<TextField>,
    folds: Folds,
    /// Directories stepped back out of, most recent last.
    forward: Vec<PathBuf>,
    /// When to ask git again what has changed, set by every re-read so the
    /// marks follow the listing.
    git_due: Option<Instant>,
    /// What git says about each changed path under the listing.
    git_marks: GitMarks,
    /// Which edge the gutter of git states and marks stands at.
    gutter_side: GutterSide,
    /// The repository root git reports paths against, once it has said.
    git_top: Option<PathBuf>,
    /// The cursor over the rows, kept on the same entry, by its path, when
    /// the listing is read again.
    list: ListView<PathBuf>,
    /// How long a key waits for the listing it asked for (see
    /// [`LISTING_WAIT`]).
    listing_wait: Duration,
    marks: Marks,
    /// The directories the other panes on screen are looking at, offered
    /// first as where a copy or a move goes.
    neighbors: Vec<PathBuf>,
    /// What the last operation reported, waiting for the status bar to take it.
    notice: Option<Notice>,
    /// The shared Vim motion state: the `g` prefix, and the layer the
    /// listing's unclaimed keys fall through to.
    nav: VimNav,
    /// The first key of a two-key sequence, waiting for its second.
    pending: Option<Key>,
    /// The entries a question about an operation named, taken when it was
    /// asked, so the answer acts on exactly those whatever a re-read has done
    /// to the rows since.
    pending_targets: Option<Vec<PathBuf>>,
    /// The read of the listing still running, which a newer read replaces.
    reading: Option<Background<Listing>>,
    /// A checked batch of renames waiting on the yes that runs it.
    rename_plan: Option<Vec<ops::Rename>>,
    /// A directory just opened whose children the next paint should bring
    /// into view, if they do not already fit under it. Held until the paint,
    /// since what fits is only known once the pane's height is.
    reveal: Option<PathBuf>,
    root: PathBuf,
    rows: Vec<Row>,
    /// The entry to put the cursor on once the read under way lands: one
    /// just created, or the directory just stepped out of.
    select_after_read: Option<PathBuf>,
    show_details: bool,
    show_hidden: bool,
    /// Whether directory sizes are shown, which each one has to be walked for.
    show_sizes: bool,
    /// Totals already walked, kept across reloads since they rarely change and
    /// re-walking on every keystroke would be the expensive mistake.
    sizes: HashMap<PathBuf, u64>,
    sort: SortKey,
}

// ========================================================================
// DirEditor
// ========================================================================

impl DirEditor {
    // --------------------------------------------------------------------
    // Reading
    // --------------------------------------------------------------------

    /// A listing of `root`, collapsed, sorted by name, hiding dotfiles.
    pub fn new(root: PathBuf) -> Self {
        let root = normalize_path(root);
        let mut editor = Self {
            back: Vec::new(),
            edit: None,
            folds: Folds::new(),
            forward: Vec::new(),
            git_due: None,
            git_marks: GitMarks::default(),
            gutter_side: GutterSide::default(),
            git_top: None,
            list: ListView::new(),
            listing_wait: LISTING_WAIT,
            marks: Marks::new(),
            neighbors: Vec::new(),
            notice: None,
            nav: VimNav::new(),
            pending: None,
            pending_targets: None,
            reading: None,
            rename_plan: None,
            reveal: None,
            root,
            rows: Vec::new(),
            select_after_read: None,
            show_details: false,
            show_hidden: false,
            show_sizes: false,
            sizes: HashMap::new(),
            sort: SortKey::default(),
        };
        editor.reload();
        editor
    }

    /// Re-read the listing, keeping the cursor on the entry it was on.
    fn reload(&mut self) {
        self.read(Opening::AsFolded);
    }

    /// Read the listing off the window's thread, opening what `opening`
    /// says first. The answer is waited on briefly, so a quick read lands
    /// with the key that asked for it; a slow one lands on a later tick,
    /// and the listing shows what it had meanwhile. Starting a read stops
    /// the one before it, whose answer is then never taken.
    fn read(&mut self, opening: Opening) {
        self.reading = None;
        let plan = ReadPlan {
            folds: self.folds.clone(),
            opening,
            root: self.root.clone(),
            show_hidden: self.show_hidden,
            sort: self.sort,
        };
        let work = Background::start(READER_THREAD, move |cancel| {
            source::read_listing(plan, cancel)
        });
        self.git_due = Some(Instant::now());
        match work.wait(self.listing_wait) {
            Some(listing) => self.take_listing(listing),
            None => self.reading = Some(work),
        }
    }

    /// Show what a read found.
    fn take_listing(&mut self, listing: Listing) {
        self.folds = listing.folds;
        self.rows = listing.rows;
        self.list
            .set_keys(self.rows.iter().map(|row| row.entry.path.clone()).collect());
        if let Some(path) = self.select_after_read.take() {
            self.list.select(&path);
        }
        if listing.truncated {
            self.notice = Some(Notice::info(format!(
                "listed the first {} entries",
                self.rows.len()
            )));
        }
    }

    /// Whether a read is still running.
    fn is_reading(&self) -> bool {
        self.reading.is_some()
    }

    /// Take git's answer to one of the two refresh commands: the root, which
    /// sends for the status, or the status, which becomes the marks. A
    /// listing outside any repository has no marks.
    fn on_git_reply(&mut self, tag: &str, succeeded: bool, stdout: &str) -> EditorOutcome {
        if !succeeded {
            self.git_marks = GitMarks::default();
            self.git_top = None;
            return EditorOutcome::Consumed;
        }
        if tag == vcs::TAG_ROOT {
            self.git_top = Some(PathBuf::from(stdout.trim()));
            return EditorOutcome::Job(JobRequest::Command(vcs::status_request(&self.root)));
        }
        if let Some(top) = &self.git_top {
            self.git_marks = vcs::marks(top, stdout);
        }
        EditorOutcome::Consumed
    }

    fn selected(&self) -> Option<&Row> {
        self.rows.get(self.list.cursor())
    }

    // --------------------------------------------------------------------
    // Moving through the tree
    // --------------------------------------------------------------------

    /// Enter what the cursor is on: a directory becomes the new root, a file is
    /// handed to the host to open. A link to a directory is entered like the
    /// directory, since that is where it leads; it is only never expanded in
    /// place, which is what keeps a tree of links from looping.
    fn enter(&mut self) -> EditorOutcome {
        let Some(row) = self.selected() else {
            return EditorOutcome::Consumed;
        };
        let target = row.entry.path.clone();
        if !row.entry.is_dir() && !is_dir_through_link(&target) {
            return EditorOutcome::OpenPath(OpenTarget::file(target));
        }
        self.set_root(target);
        EditorOutcome::Consumed
    }

    /// Hand the file under the cursor to `$EDITOR`, in a pane of its own, for
    /// the editing the app's own editor deliberately cannot do.
    fn open_external(&self) -> EditorOutcome {
        match self.selected() {
            Some(row) if !row.entry.is_dir() => {
                EditorOutcome::SpawnEditor(OpenTarget::file(row.entry.path.clone()))
            }
            _ => EditorOutcome::Consumed,
        }
    }

    /// Move the listing to the parent directory, keeping the cursor on the
    /// directory just left so stepping up and down again lands where it started.
    fn ascend(&mut self) {
        let Some(parent) = self.root.parent().map(PathBuf::from) else {
            return;
        };
        self.select_after_read = Some(self.root.clone());
        self.set_root(parent);
    }

    fn set_root(&mut self, root: PathBuf) {
        let root = normalize_path(root);
        if root == self.root {
            return;
        }
        self.back.push(self.root.clone());
        if self.back.len() > MAX_HISTORY {
            self.back.remove(0);
        }
        // A fresh move is a new branch of the history: what was stepped back
        // out of is no longer ahead of anywhere.
        self.forward.clear();
        self.move_root_to(root);
    }

    /// Change the root without touching the history, so a history step does not
    /// record itself.
    fn move_root_to(&mut self, root: PathBuf) {
        self.folds.retain_under(&root);
        // The visible set changes wholesale, so a mark held over would count
        // toward an operation aimed at a listing it was never part of.
        self.marks.clear();
        self.sizes.clear();
        self.root = root;
        self.list.set_cursor(0);
        self.list.view_mut().set_scroll(0);
        self.reload();
    }

    /// Return to the previous directory in the visit history.
    fn go_back(&mut self) {
        let Some(previous) = self.back.pop() else {
            return;
        };
        self.forward.push(self.root.clone());
        self.move_root_to(previous);
    }

    /// Undo a step taken with [`Self::go_back`].
    fn go_forward(&mut self) {
        let Some(next) = self.forward.pop() else {
            return;
        };
        self.back.push(self.root.clone());
        self.move_root_to(next);
    }

    /// The row holding the directory `index` sits inside, if any.
    fn parent_row(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        (0..index).rev().find(|&i| self.rows[i].depth < depth)
    }

    /// The next row at the same depth, stopping at the end of the subtree the
    /// cursor is in rather than escaping into the next one.
    fn sibling_after(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        self.rows
            .iter()
            .enumerate()
            .skip(index + 1)
            .take_while(|(_, row)| row.depth >= depth)
            .find(|(_, row)| row.depth == depth)
            .map(|(i, _)| i)
    }

    /// The previous row at the same depth, within the same subtree.
    fn sibling_before(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        (0..index)
            .rev()
            .take_while(|&i| self.rows[i].depth >= depth)
            .find(|&i| self.rows[i].depth == depth)
    }

    /// The first row inside the expanded directory at `index`.
    fn first_child(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        self.rows
            .get(index + 1)
            .filter(|row| row.depth == depth + 1)
            .map(|_| index + 1)
    }

    /// Put the cursor on `index`, where there is one: the shape every
    /// structural motion's answer comes in.
    fn move_to(&mut self, index: Option<usize>) {
        if let Some(index) = index {
            self.list.set_cursor(index);
        }
    }

    // --------------------------------------------------------------------
    // Folding
    // --------------------------------------------------------------------

    /// One key for stepping out: collapse an expanded directory, else move to
    /// the directory this row sits in, else leave the root itself.
    fn fold_or_step_out(&mut self) {
        let expanded = self.selected().is_some_and(|row| row.expanded);
        if expanded {
            self.toggle_fold();
            return;
        }
        match self.parent_row(self.list.cursor()) {
            Some(parent) => self.list.set_cursor(parent),
            None => self.ascend(),
        }
    }

    /// Expand every collapsed directory currently listed. Repeating it reaches
    /// one level deeper each time, so the whole tree is never read at once.
    fn expand_one_level(&mut self) {
        let collapsed: Vec<PathBuf> = self
            .rows
            .iter()
            .filter(|row| row.entry.is_dir() && !row.expanded)
            .map(|row| row.entry.path.clone())
            .collect();
        if collapsed.is_empty() {
            return;
        }
        for path in collapsed {
            self.folds.expand(&path);
        }
        self.reload();
    }

    /// Collapse everything if anything is open, else open one level. One key
    /// that always does the thing the listing is not already showing.
    fn toggle_fold_all(&mut self) {
        let any_open = self.rows.iter().any(|row| row.expanded);
        if any_open {
            self.folds.collapse_all();
            self.reload();
        } else {
            self.expand_one_level();
        }
    }

    /// Expand every directory down to `depth` levels below the root, reading
    /// only as far as asked: depth 0 collapses everything. The levels are
    /// read off the window's thread, to the read's own budget.
    fn expand_to_depth(&mut self, depth: usize) {
        self.read(Opening::Depth(depth));
    }

    /// Open the whole subtree under the cursor, off the window's thread and
    /// to the read's own budget, so opening a home directory whole cannot
    /// hold the window or read without end.
    fn expand_subtree(&mut self) {
        let Some(root) = self.selected().map(|row| row.entry.path.clone()) else {
            return;
        };
        self.read(Opening::Subtree(root));
    }

    /// Close the whole subtree under the cursor, the directory included.
    fn collapse_subtree(&mut self) {
        let Some(root) = self.selected().map(|row| row.entry.path.clone()) else {
            return;
        };
        self.folds.collapse_under(&root);
        self.reload();
    }

    /// Open the subtree under the cursor, or close it when it is already open.
    fn toggle_subtree(&mut self) {
        let open = self.selected().is_some_and(|row| row.expanded);
        if open {
            self.collapse_subtree();
        } else {
            self.expand_subtree();
        }
    }

    fn toggle_fold(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        if !row.entry.is_dir() {
            return;
        }
        let opening = !row.expanded;
        let path = row.entry.path.clone();
        self.folds.toggle(&path);
        if opening {
            self.reveal = Some(path);
        }
        self.reload();
    }

    /// Expand or collapse the directory under the cursor, whichever `expand`
    /// asks for, leaving it alone when it is already that way.
    fn expand_selected(&mut self, expand: bool) {
        let Some(row) = self.selected() else {
            return;
        };
        if !row.entry.is_dir() || row.expanded == expand {
            return;
        }
        self.toggle_fold();
    }

    // --------------------------------------------------------------------
    // Motions
    // --------------------------------------------------------------------

    /// The last row of the subtree rooted at `index`, which is `index` itself
    /// for a collapsed entry: the extent of the "word" `index` names.
    fn subtree_end(&self, index: usize) -> Option<usize> {
        let depth = self.rows.get(index)?.depth;
        let mut end = index;
        for (i, row) in self.rows.iter().enumerate().skip(index + 1) {
            if row.depth > depth {
                end = i;
            } else {
                break;
            }
        }
        Some(end)
    }

    /// `w`/`W`: the next entry at or above the cursor's depth: the next
    /// word, stepping over the subtree under the cursor rather than into it,
    /// the way Vim's `w` steps over the word it sits on.
    fn word_forward(&mut self) {
        let Some(depth) = self.selected().map(|row| row.depth) else {
            return;
        };
        let at = self.list.cursor();
        let next = (at + 1..self.rows.len()).find(|&i| self.rows[i].depth <= depth);
        self.move_to(next);
    }

    /// `b`/`B`: the previous entry at or above the cursor's depth: the
    /// previous word, stepping back over the whole subtree above rather than
    /// descending into it.
    fn word_back(&mut self) {
        let Some(depth) = self.selected().map(|row| row.depth) else {
            return;
        };
        let previous = (0..self.list.cursor())
            .rev()
            .find(|&i| self.rows[i].depth <= depth);
        self.move_to(previous);
    }

    /// `e`/`E`: the last row of the current entry's subtree, the end of the
    /// word the cursor is on. Already there, or on a leaf, it is the end of
    /// the next word, exactly the way Vim's `e` leaves a word it already sits
    /// at the end of for the next one.
    fn word_end(&mut self) {
        let from = self.list.cursor();
        if let Some(end) = self.subtree_end(from) {
            if from < end {
                self.list.set_cursor(end);
                return;
            }
        }
        self.word_forward();
        let at = self.list.cursor();
        if at != from {
            self.move_to(self.subtree_end(at));
        }
    }

    /// `}`/`{`: the next or previous directory row, the listing's paragraph
    /// motion, since directories sort first and so head every group.
    fn dir_row(&mut self, forward: bool) {
        let at = self.list.cursor();
        let found = match forward {
            true => (at + 1..self.rows.len()).find(|&i| self.rows[i].entry.is_dir()),
            false => (0..at).rev().find(|&i| self.rows[i].entry.is_dir()),
        };
        self.move_to(found);
    }

    /// Interpret one of the shared Vim motions over the listing's rows. A
    /// listing has no columns, so the line motions mean its ends; an entry is
    /// a word, and an expanded directory's subtree is that word's extent; the
    /// paragraphs are the directory-headed groups, which is what the
    /// directories-first sort makes of the rows.
    fn apply_motion(&mut self, motion: CursorMove) {
        if self.list.apply_motion(motion) {
            return;
        }
        match motion {
            CursorMove::WordForward | CursorMove::WordForwardBig => self.word_forward(),
            CursorMove::WordBack | CursorMove::WordBackBig => self.word_back(),
            CursorMove::WordEnd | CursorMove::WordEndBig => self.word_end(),
            CursorMove::ParagraphForward => self.dir_row(true),
            CursorMove::ParagraphBack => self.dir_row(false),
            // The column motions and the rest have no meaning over rows; the
            // keys that reach them here are the ones the listing has not
            // claimed for something of its own.
            _ => {}
        }
    }

    // --------------------------------------------------------------------
    // Search and marks
    // --------------------------------------------------------------------

    /// The names a search reads over, one per row.
    fn names(&self) -> Vec<String> {
        self.rows.iter().map(|row| row.entry.name.clone()).collect()
    }

    /// Remember `query` and move to the first entry whose name holds it.
    fn search_for(&mut self, query: &str) -> EditorOutcome {
        let names = self.names();
        if let Some(notice) = self.list.search_for(&names, query) {
            self.notice = Some(notice);
        }
        EditorOutcome::Consumed
    }

    /// Move to the next entry matching the last search, or report that there is
    /// nothing to move to: a listing that silently stays put looks broken.
    fn search_step(&mut self, forward: bool) -> EditorOutcome {
        let names = self.names();
        if let Some(notice) = self.list.search_step(&names, forward) {
            self.notice = Some(notice);
        }
        EditorOutcome::Consumed
    }

    fn toggle_mark(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        let path = row.entry.path.clone();
        self.marks.toggle(&path);
        self.list.move_by(1);
    }

    fn unmark_selected(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        let path = row.entry.path.clone();
        self.marks.remove(&path);
        self.list.move_by(1);
    }

    fn mark_all(&mut self) {
        let listed: Vec<PathBuf> = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        for path in listed {
            self.marks.insert(&path);
        }
    }

    /// The paths the next operation acts on: every mark still listed, or the
    /// entry under the cursor when nothing is marked.
    fn targets(&self) -> Vec<PathBuf> {
        let listed: Vec<PathBuf> = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        let cursor = self.selected().map(|row| row.entry.path.as_path());
        marks::targets(&self.marks, &listed, cursor)
    }

    // --------------------------------------------------------------------
    // Questions
    // --------------------------------------------------------------------

    /// Ask a question, carrying the tag that says what the answer is for.
    fn ask(&self, tag: &'static str, label: String, initial: String) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial,
            label,
            mode: PromptMode::Text,
            tag,
        })
    }

    /// Ask what name to move to, highlighting what it matches as it is typed.
    fn ask_search(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: "/".to_string(),
            mode: PromptMode::Search { reverse: false },
            tag: ASK_SEARCH,
        })
    }

    // --------------------------------------------------------------------
    // Painting
    // --------------------------------------------------------------------

    /// The row `index` paints as, and the icon beside it.
    fn paint_row(&self, index: usize, cols: usize, now: SystemTime) -> (Vec<Span>, Vec<Icon>) {
        let row = &self.rows[index];
        let edited = self.edited_name(index, row);
        rows::entry_row(
            row,
            edited.as_ref(),
            rows::RowStyle {
                cols,
                gutter_side: self.gutter_side,
                marked: self.marks.contains(&row.entry.path),
                show_details: self.show_details,
                show_sizes: self.show_sizes,
                size: self.dir_size(row),
                git: self.git_marks.get(&row.entry.path),
            },
            now,
        )
    }
}

impl Editor for DirEditor {
    fn next_tick(&self) -> Option<Instant> {
        // A read under way is looked for soon, so it lands as it finishes.
        let read = self.is_reading().then(|| Instant::now() + background::POLL);
        match (self.git_due, read) {
            (Some(git), Some(read)) => Some(git.min(read)),
            (git, read) => git.or(read),
        }
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        Some(OpenTarget::file(self.root.clone()))
    }

    fn set_gutter_side(&mut self, side: GutterSide) {
        self.gutter_side = side;
    }

    fn set_neighbors(&mut self, dirs: Vec<PathBuf>) {
        self.neighbors = dirs;
        self.neighbors.retain(|dir| *dir != self.root);
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if let Some(listing) = self.reading.as_ref().and_then(Background::poll) {
            self.reading = None;
            self.take_listing(listing);
        }
        match self.git_due.filter(|due| *due <= now) {
            Some(_) => {
                self.git_due = None;
                EditorOutcome::Job(JobRequest::Command(vcs::root_request(&self.root)))
            }
            None => EditorOutcome::Consumed,
        }
    }

    fn on_resume(&mut self) -> EditorOutcome {
        // A file edited under the listing has a new size and a new mtime, and
        // a file created or deleted is a row that is not there.
        self.reload();
        EditorOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match reply {
            JobReply::Command(out) => {
                return self.on_git_reply(out.tag, out.succeeded(), &out.stdout);
            }
            // A listing asks for no searches or process lists of its own.
            JobReply::Files(_)
            | JobReply::Processes(_)
            | JobReply::Search(_)
            | JobReply::System(_) => return EditorOutcome::Consumed,
            JobReply::DirSize { bytes, path } => {
                self.sizes.insert(path, bytes);
            }
            JobReply::FileOps(result) => {
                self.notice = Some(match result {
                    Ok(report) => Notice::info(report),
                    Err(report) => Notice::error(format!("failed: {report}")),
                });
                self.marks.clear();
                self.reload();
                return EditorOutcome::Consumed;
            }
        }
        self.request_next_size()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.list.view_mut().release();
        match reply.answer {
            // A search moves the cursor and nothing else, so it must not clear
            // the marks or re-read the listing the way an operation does.
            Some(answer) if reply.tag == ASK_SEARCH => return self.search_for(&answer),
            Some(_) if reply.tag == ASK_APPLY_EDITS => {
                return match self.rename_plan.take() {
                    Some(plan) => self.run_renames(plan),
                    None => EditorOutcome::Consumed,
                };
            }
            // A batch turned down stays on screen to be fixed or applied.
            None if reply.tag == ASK_APPLY_EDITS => {
                self.rename_plan = None;
                self.notice = Some(Notice::info("not renamed; still editing"));
            }
            Some(answer) => return self.apply_answer(reply.tag, &answer),
            None => {
                self.pending_targets = None;
                self.notice = Some(Notice::info("cancelled"));
            }
        }
        EditorOutcome::Consumed
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_SEARCH && !query.is_empty() {
            self.search_for(query)
        } else {
            EditorOutcome::Ignored
        }
    }

    fn title(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root.to_string_lossy().to_string())
    }

    /// Clipboard text goes into the name being edited, and nowhere else.
    fn on_paste(&mut self, text: String) -> EditorOutcome {
        if let Some(edit) = self.edit.as_mut() {
            edit.paste(&text);
        }
        EditorOutcome::Consumed
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        // The view moves and the cursor stays, as in an editor.
        let total = self.rows.len();
        self.list.view_mut().wheel(lines, total);
        EditorOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let now = SystemTime::now();
        let visible = rows.saturating_sub(HEADER_ROWS);
        // Remember the viewport for the half-page motions, which key handling
        // needs between paints.
        self.list.view_mut().set_rows(visible);
        let root = format_full_path(&self.root);
        let mut frame_rows = vec![rows::header_row(
            &root,
            rows::HeaderFlags {
                show_hidden: self.show_hidden,
                show_details: self.show_details,
                show_sizes: self.show_sizes,
            },
        )];
        if self.rows.is_empty() {
            let note = match self.is_reading() {
                true => READING_NOTE,
                false => EMPTY_NOTE,
            };
            frame_rows.push(vec![Span::new(Style::Dim, note)]);
            return Frame::new(frame_rows);
        }
        let reveal = self
            .reveal
            .take()
            .and_then(|path| self.list.position(&path));
        // An entry's height, measured on demand: the wrapping window walks
        // only the rows it may paint, so a listing longer than the pane is not
        // painted in full to measure it. The cursor is taken out for the
        // layout, which reads every other part of the listing.
        let mut list = std::mem::take(&mut self.list);
        let height = |index: usize| {
            row_height(
                &row_text(&self.paint_row(index, cols, now).0),
                cols,
                wrap,
                0,
            )
        };
        let mut placed = list.place(visible, height);
        if let Some(start) = reveal_start(&self.rows, &placed.window, reveal) {
            list.view_mut().set_scroll(start);
            placed = list.place(visible, height);
        }
        self.list = list;
        let window = placed.window;
        let mut icons: Vec<Icon> = Vec::new();
        for index in (window.start..self.rows.len()).take(window.count) {
            let (spans, row_icons) = self.paint_row(index, cols, now);
            let row = frame_rows.len();
            icons.extend(row_icons.into_iter().map(|icon| Icon { row, ..icon }));
            frame_rows.push(spans);
        }
        let content = Frame::new(frame_rows).with_icons(icons);
        match placed.shows_cursor {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    /// The edit mode: Normal when browsing or editing in Normal mode, Insert
    /// while typing a rename, and how much is marked for the next operation.
    fn status(&self) -> EditorStatus {
        let marked = self.marks.len();
        let mode = self
            .edit
            .as_ref()
            .map_or(EditMode::Normal, |edit| edit.modal().mode());
        EditorStatus {
            mode: Some(mode),
            segments: (marked > 0)
                .then(|| format!("{marked} marked"))
                .into_iter()
                .collect(),
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.dispatch_key(key)
    }

    fn cwd(&self) -> Option<PathBuf> {
        // A directory row is itself the answer; any other row means the
        // directory holding it. With the tree folded open the cursor can sit
        // well below the listing's root, which is the whole point of asking
        // the cursor rather than the root.
        let Some(row) = self.selected() else {
            return Some(self.root.clone());
        };
        match row.entry.is_dir() {
            true => Some(row.entry.path.clone()),
            false => row.entry.path.parent().map(PathBuf::from),
        }
    }

    fn root_dir(&self) -> Option<PathBuf> {
        Some(self.root.clone())
    }

    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.root))
    }

    /// Every marked entry's path, one per line; with nothing marked, the
    /// directory the cursor's entry sits in, which is the listing's own unless
    /// the cursor is inside a folder expanded beneath it.
    fn file_reference(&self) -> Option<String> {
        let listed: Vec<PathBuf> = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        let marked = self.marks.in_order(&listed);
        if marked.is_empty() {
            let dir = self
                .selected()
                .and_then(|row| row.entry.path.parent())
                .unwrap_or(&self.root);
            return Some(abbreviate_home(dir));
        }
        let refs: Vec<String> = marked.iter().map(|p| abbreviate_home(p)).collect();
        Some(refs.join("\n"))
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        self.menu_items()
    }

    fn keys(&self) -> Vec<KeyHelp> {
        self.key_help()
    }

    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        let targets = self
            .rows
            .iter()
            .enumerate()
            .map(|(idx, row)| {
                let name = self
                    .edited_name(idx, row)
                    .map(|e| e.text)
                    .unwrap_or_else(|| row.entry.name.clone());
                let label = format!("{:>5}  {}{}", idx + 1, "  ".repeat(row.depth), name);
                JumpTarget::new(idx, label).with_icon(rows::icon_kind(row))
            })
            .collect();
        Some(targets)
    }

    fn jump_cursor(&self) -> Option<usize> {
        Some(self.list.cursor())
    }

    fn jump_to(&mut self, target: usize) {
        self.list.jump_to(target);
    }

    fn jump_cancel(&mut self) {
        self.list.jump_cancel();
    }

    fn jump_confirm(&mut self, target: usize) {
        self.list.jump_confirm(target);
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// Where to scroll so a just-opened directory shows what it opened: the row
/// itself, once the rows nested under it run past the window's bottom, since
/// from there the most of them fit. `None` leaves the window alone, which is
/// the case whenever they already show in full and whenever nothing was
/// opened at all.
fn reveal_start(rows: &[Row], window: &RowWindow, at: Option<usize>) -> Option<usize> {
    let at = at?;
    let head = rows.get(at)?;
    let end = rows
        .iter()
        .enumerate()
        .skip(at + 1)
        .take_while(|(_, row)| row.depth > head.depth)
        .map(|(index, _)| index)
        .last()
        .unwrap_or(at);
    (end >= window.start + window.count).then_some(at)
}

/// Whether `path` is a link that leads to a directory.
fn is_dir_through_link(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|meta| meta.is_dir())
}

/// The final component of `path`, as text.
fn name_of(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| path.to_string_lossy().to_string())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

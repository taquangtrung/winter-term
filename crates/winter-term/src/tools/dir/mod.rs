//! Dir: a keyboard-driven directory listing in a pane.
//!
//! - [`edit`]: the names as editable text (`Ctrl-X Ctrl-Q`): a Vim
//!   Normal/Insert pair over them, applied as renames.
//! - [`entry`]: what a listing is made of.
//! - [`icons`]: the glyph beside an entry's name.
//! - [`listing`]: ordering and filtering.
//! - [`marks`]: which entries an operation acts on.
//! - [`ops`]: creating, moving, copying, and deleting.
//! - [`rows`]: painting a listing.
//! - [`source`]: reading the filesystem.
//! - [`tree`]: expanded directories and row depth.

pub mod edit;
pub mod entry;
pub mod icons;
pub mod listing;
pub mod marks;
pub mod ops;
pub mod rows;
pub mod source;
pub mod tree;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::model::input::CursorMove;
use crate::model::input::{Key, KeyCode};
use crate::model::page::{
    find_match, row_height, row_text, wrap_window, JobReply, JobRequest, OpenTarget, Page,
    PageContent, PageIcon, PageMenuItem, PageOutcome, PageSpan, PageStyle, PageWindow,
    PickQuestion, PromptMode, PromptReply, PromptRequest,
};
use crate::model::vim::nav::{buffer_end, VimKey, VimNav};

use edit::{EditAction, EditMode, EditState};
use listing::SortKey;
use marks::Marks;
use tree::{Folds, Row};

// ========================================================================
// Constants
// ========================================================================

/// Rows of header above the first entry.
const HEADER_ROWS: usize = 1;

/// Shown in place of the listing when a directory has nothing to show.
const EMPTY_NOTE: &str = "  (empty)";

/// Prompt tags, one per question the listing asks.
const ASK_COPY: &str = "copy";
const ASK_DELETE: &str = "delete";
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

/// What the menu opened over a row calls each of the entries it offers. An
/// operation acts on the marked entries when there are any, which is why
/// these name the operation rather than the entry under the cursor.
const LABEL_COPY: &str = "Copy...";
const LABEL_DELETE: &str = "Delete...";
const LABEL_MARK: &str = "Mark";
const LABEL_MOVE: &str = "Move...";
const LABEL_NEW_DIR: &str = "New Directory...";
const LABEL_NEW_FILE: &str = "New File...";
const LABEL_OPEN: &str = "Open";
const LABEL_OPEN_EXTERNAL: &str = "Open With System Handler";
const LABEL_OPEN_IN_EDITOR: &str = "Open in $EDITOR";
const LABEL_RELOAD: &str = "Reload";
const LABEL_RENAME: &str = "Rename...";
const LABEL_UNMARK: &str = "Unmark";

// ========================================================================
// Data Structures
// ========================================================================

/// The first key of a two-key sequence, waiting for its second.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Leader {
    /// `Ctrl-X`, followed by `Ctrl-Q`: toggle editing the names, the chord
    /// Emacs' own `wdired` uses.
    Edit,
    /// `Ctrl-c`, followed by a digit: expand the tree to that depth.
    Depth,
    /// `z`, followed by a fold command.
    Fold,
}

/// A directory listing the keyboard drives: move, fold, descend, and open.
#[derive(Clone, Debug)]
pub struct DirPage {
    /// Directories left behind, most recent last.
    back: Vec<PathBuf>,
    cursor: usize,
    /// The names as editable text while renaming entries in place, in the
    /// manner of Emacs' `wdired`.
    edit: Option<EditState>,
    folds: Folds,
    /// Directories stepped back out of, most recent last.
    forward: Vec<PathBuf>,
    /// What the last operation reported, shown in the header until the next key.
    message: Option<String>,
    marks: Marks,
    /// The first key of a two-key sequence, waiting for its second.
    pending: Option<Leader>,
    root: PathBuf,
    rows: Vec<Row>,
    /// First listed row visible in the pane.
    scroll: usize,
    /// The last name searched for, repeated by the next and previous keys.
    search: String,
    show_details: bool,
    show_hidden: bool,
    /// Whether directory sizes are shown, which each one has to be walked for.
    show_sizes: bool,
    /// Totals already walked, kept across reloads since they rarely change and
    /// re-walking on every keystroke would be the expensive mistake.
    sizes: HashMap<PathBuf, u64>,
    sort: SortKey,
    /// The pane height the last paint saw, in listing rows, for the half-page
    /// motions. Zero until the first paint, when there is nothing to halve.
    viewport: usize,
    /// The shared Vim motion state: the `g` prefix, and the layer the
    /// listing's unclaimed keys fall through to.
    nav: VimNav,
    /// A directory just opened whose children the next paint should bring
    /// into view, if they do not already fit under it. Held until the paint,
    /// since what fits is only known once the pane's height is.
    reveal: Option<usize>,
}

// ========================================================================
// DirPage
// ========================================================================

impl DirPage {
    /// A listing of `root`, collapsed, sorted by name, hiding dotfiles.
    pub fn new(root: PathBuf) -> Self {
        let mut page = Self {
            back: Vec::new(),
            cursor: 0,
            edit: None,
            folds: Folds::new(),
            forward: Vec::new(),
            marks: Marks::new(),
            message: None,
            pending: None,
            root,
            rows: Vec::new(),
            scroll: 0,
            search: String::new(),
            show_details: false,
            show_hidden: false,
            show_sizes: false,
            sizes: HashMap::new(),
            sort: SortKey::default(),
            viewport: 0,
            nav: VimNav::new(),
            reveal: None,
        };
        page.reload();
        page
    }

    /// Re-read the listing, leaving the cursor where it is.
    fn reload(&mut self) {
        self.rows = source::read_rows(&self.root, self.show_hidden, self.sort, &self.folds);
        self.clamp_cursor();
    }

    /// Re-read the listing and keep the cursor on the same entry, so toggling a
    /// view option does not move the user somewhere else in the tree.
    fn reload_keeping_selection(&mut self) {
        let selected = self.selected().map(|row| row.entry.path.clone());
        self.reload();
        if let Some(path) = selected {
            if let Some(index) = self.rows.iter().position(|row| row.entry.path == path) {
                self.cursor = index;
            }
        }
    }

    fn clamp_cursor(&mut self) {
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&Row> {
        self.rows.get(self.cursor)
    }

    /// Enter what the cursor is on: a directory becomes the new root, a file is
    /// handed to the host to open.
    fn enter(&mut self) -> PageOutcome {
        let Some(row) = self.selected() else {
            return PageOutcome::Consumed;
        };
        if !row.entry.is_dir() {
            return PageOutcome::OpenPath(OpenTarget::file(row.entry.path.clone()));
        }
        let target = row.entry.path.clone();
        self.set_root(target);
        PageOutcome::Consumed
    }

    /// Hand the file under the cursor to `$EDITOR`, in a pane of its own, for
    /// the editing the app's own editor deliberately cannot do.
    fn open_external(&self) -> PageOutcome {
        match self.selected() {
            Some(row) if !row.entry.is_dir() => {
                PageOutcome::SpawnEditor(OpenTarget::file(row.entry.path.clone()))
            }
            _ => PageOutcome::Consumed,
        }
    }

    /// Move the listing to the parent directory, keeping the cursor on the
    /// directory just left so stepping up and down again lands where it started.
    fn ascend(&mut self) {
        let Some(parent) = self.root.parent().map(PathBuf::from) else {
            return;
        };
        let left = self.root.clone();
        self.set_root(parent);
        if let Some(index) = self.rows.iter().position(|row| row.entry.path == left) {
            self.cursor = index;
        }
    }

    fn set_root(&mut self, root: PathBuf) {
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
        self.cursor = 0;
        self.scroll = 0;
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

    /// One key for stepping out: collapse an expanded directory, else move to
    /// the directory this row sits in, else leave the root itself.
    fn fold_or_step_out(&mut self) {
        let expanded = self.selected().is_some_and(|row| row.expanded);
        if expanded {
            self.toggle_fold();
            return;
        }
        match self.parent_row(self.cursor) {
            Some(parent) => self.cursor = parent,
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
        self.reload_keeping_selection();
    }

    /// Collapse everything if anything is open, else open one level. One key
    /// that always does the thing the listing is not already showing.
    fn toggle_fold_all(&mut self) {
        let any_open = self.rows.iter().any(|row| row.expanded);
        if any_open {
            self.folds.collapse_all();
            self.reload_keeping_selection();
        } else {
            self.expand_one_level();
        }
    }

    /// Expand every directory down to `depth` levels below the root, reading
    /// only as far as asked: depth 0 collapses everything.
    fn expand_to_depth(&mut self, depth: usize) {
        self.folds.collapse_all();
        for _ in 0..depth {
            self.reload();
            self.expand_one_level();
        }
        self.reload_keeping_selection();
    }

    /// Open the whole subtree under the cursor, bounded by the reader's own
    /// depth cap so a deep tree cannot be read without limit.
    fn expand_subtree(&mut self) {
        let Some(root) = self.selected().map(|row| row.entry.path.clone()) else {
            return;
        };
        for path in source::descendant_dirs(&root, self.show_hidden) {
            self.folds.expand(&path);
        }
        self.folds.expand(&root);
        self.reload_keeping_selection();
    }

    /// Close the whole subtree under the cursor, the directory included.
    fn collapse_subtree(&mut self) {
        let Some(root) = self.selected().map(|row| row.entry.path.clone()) else {
            return;
        };
        self.folds.collapse_under(&root);
        self.reload_keeping_selection();
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

    /// `w`/`W`: the next entry at or above the cursor's depth — the next
    /// word, stepping over the subtree under the cursor rather than into it,
    /// the way Vim's `w` steps over the word it sits on.
    fn word_forward(&mut self) {
        let Some(depth) = self.selected().map(|row| row.depth) else {
            return;
        };
        if let Some(index) = self
            .rows
            .iter()
            .enumerate()
            .skip(self.cursor + 1)
            .find(|(_, row)| row.depth <= depth)
            .map(|(i, _)| i)
        {
            self.cursor = index;
        }
    }

    /// `b`/`B`: the previous entry at or above the cursor's depth — the
    /// previous word, stepping back over the whole subtree above rather than
    /// descending into it.
    fn word_back(&mut self) {
        let Some(depth) = self.selected().map(|row| row.depth) else {
            return;
        };
        if let Some(index) = (0..self.cursor)
            .rev()
            .find(|&i| self.rows[i].depth <= depth)
        {
            self.cursor = index;
        }
    }

    /// `e`/`E`: the last row of the current entry's subtree — the end of the
    /// word the cursor is on. Already there, or on a leaf, it is the end of
    /// the next word, exactly the way Vim's `e` leaves a word it already sits
    /// at the end of for the next one.
    fn word_end(&mut self) {
        if let Some(end) = self.subtree_end(self.cursor) {
            if self.cursor < end {
                self.cursor = end;
                return;
            }
        }
        let from = self.cursor;
        self.word_forward();
        if self.cursor != from {
            if let Some(end) = self.subtree_end(self.cursor) {
                self.cursor = end;
            }
        }
    }

    /// `}`/`{`: the next or previous directory row — the listing's paragraph
    /// motion, since directories sort first and so head every group.
    fn dir_row(&mut self, forward: bool) {
        let range: Vec<usize> = if forward {
            (self.cursor + 1..self.rows.len()).collect()
        } else {
            (0..self.cursor).rev().collect()
        };
        if let Some(index) = range.into_iter().find(|&i| self.rows[i].entry.is_dir()) {
            self.cursor = index;
        }
    }

    /// `Ctrl-d`/`Ctrl-u` and the page motions: `rows` rows down or up,
    /// clamped to the listing.
    fn page_by(&mut self, rows: usize, down: bool) {
        let step = rows.max(1) as isize;
        self.move_by(if down { step } else { -step });
    }

    /// Interpret one of the shared Vim motions over the listing's rows. A
    /// listing has no columns, so the line motions mean its ends; an entry is
    /// a word, and an expanded directory's subtree is that word's extent; the
    /// paragraphs are the directory-headed groups, which is what the
    /// directories-first sort makes of the rows.
    fn apply_motion(&mut self, motion: CursorMove) {
        let last = self.rows.len().saturating_sub(1);
        match motion {
            CursorMove::Down => self.move_by(1),
            CursorMove::Up => self.move_by(-1),
            CursorMove::WordForward | CursorMove::WordForwardBig => self.word_forward(),
            CursorMove::WordBack | CursorMove::WordBackBig => self.word_back(),
            CursorMove::WordEnd | CursorMove::WordEndBig => self.word_end(),
            CursorMove::ParagraphForward => self.dir_row(true),
            CursorMove::ParagraphBack => self.dir_row(false),
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
            // keys that reach them here are the ones the listing has not
            // claimed for something of its own.
            _ => {}
        }
    }

    fn unmark_selected(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        let path = row.entry.path.clone();
        self.marks.remove(&path);
        self.move_by(1);
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
        self.reload_keeping_selection();
        if opening {
            self.reveal = Some(self.cursor);
        }
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
    }

    /// Remember `query` and move to the first entry whose name holds it.
    fn search_for(&mut self, query: &str) {
        self.search = query.to_string();
        self.search_step(true);
    }

    /// Move to the next entry matching the last search, or report that there is
    /// nothing to move to: a listing that silently stays put looks broken.
    fn search_step(&mut self, forward: bool) {
        if self.search.is_empty() {
            self.message = Some("no search".to_string());
            return;
        }
        let names: Vec<&str> = self
            .rows
            .iter()
            .map(|row| row.entry.name.as_str())
            .collect();
        match find_match(&names, &self.search, self.cursor, forward) {
            Some(index) => self.cursor = index,
            None => self.message = Some(format!("not found: {}", self.search)),
        }
    }

    fn toggle_mark(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        let path = row.entry.path.clone();
        self.marks.toggle(&path);
        self.move_by(1);
    }

    fn mark_all(&mut self) {
        let listed: Vec<PathBuf> = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        for path in listed {
            self.marks.insert(&path);
        }
    }

    /// The walked total for a directory row: `None` for a file, or while the
    /// walk is still running.
    fn dir_size(&self, row: &Row) -> Option<u64> {
        if !self.show_sizes || !row.entry.is_dir() {
            return None;
        }
        self.sizes.get(&row.entry.path).copied()
    }

    /// The paths the next operation acts on: every mark still listed, or the
    /// entry under the cursor when nothing is marked.
    fn targets(&self) -> Vec<PathBuf> {
        let listed: Vec<PathBuf> = self.rows.iter().map(|r| r.entry.path.clone()).collect();
        let cursor = self.selected().map(|row| row.entry.path.as_path());
        marks::targets(&self.marks, &listed, cursor)
    }

    /// Ask a question, carrying the tag that says what the answer is for.
    fn ask(&self, tag: &'static str, label: String, initial: String) -> PageOutcome {
        PageOutcome::Prompt(PromptRequest {
            initial,
            label,
            mode: PromptMode::Text,
            tag,
        })
    }

    /// Ask for a yes-or-no answer, for what cannot be undone.
    fn confirm(&self, tag: &'static str, label: String) -> PageOutcome {
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label,
            mode: PromptMode::Confirm,
            tag,
        })
    }

    /// Ask to rename the entry under the cursor, starting from its own name.
    fn ask_rename(&self) -> PageOutcome {
        let Some(row) = self.selected() else {
            return PageOutcome::Consumed;
        };
        self.ask(
            ASK_RENAME,
            "Rename to: ".to_string(),
            row.entry.name.clone(),
        )
    }

    /// Ask to delete the targets, naming how many there are so a marked set is
    /// never deleted on a glance at one filename.
    fn ask_delete(&self) -> PageOutcome {
        let targets = self.targets();
        match targets.len() {
            0 => PageOutcome::Consumed,
            1 => self.confirm(
                ASK_DELETE,
                format!("Delete {}? (y/n) ", name_of(&targets[0])),
            ),
            count => self.confirm(ASK_DELETE, format!("Delete {count} entries? (y/n) ")),
        }
    }

    /// Ask where the targets go, for a copy or a move.
    fn ask_destination(&self, tag: &'static str, verb: &str) -> PageOutcome {
        let targets = self.targets();
        let Some(first) = targets.first() else {
            return PageOutcome::Consumed;
        };
        // One entry is transferred under a new name, which is nothing to
        // list; several go into a directory, which is.
        if targets.len() == 1 {
            return self.ask(
                tag,
                format!("{verb} {} to: ", name_of(first)),
                String::new(),
            );
        }
        let label = format!("{verb} {} entries into", targets.len());
        match PickQuestion::new(tag, &label).over(self.directory_names()) {
            Some(request) => PageOutcome::Pick(request),
            None => self.ask(tag, format!("{label}: "), String::new()),
        }
    }

    /// The directories listed here, by name. Names rather than paths because
    /// that is all a destination may be: a transfer resolves its answer
    /// against this directory and refuses anything that reaches outside it
    /// (see [`ops::resolve_name`]).
    fn directory_names(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row.entry.is_dir())
            .map(|row| name_of(&row.entry.path).to_string())
            .collect()
    }

    /// Carry out the answer to a question, and report what happened.
    fn apply_answer(&mut self, tag: &'static str, answer: &str) -> PageOutcome {
        let result = match tag {
            ASK_COPY => self.copy_targets(answer),
            ASK_DELETE => self.delete_targets(),
            ASK_MKDIR => self.create(answer, true),
            ASK_MODE => self.change_mode(answer),
            ASK_MOVE => self.move_targets(answer),
            ASK_NEW_FILE => self.create(answer, false),
            ASK_RENAME => self.rename_selected(answer),
            _ => Ok(String::new()),
        };
        let created = result.is_ok();
        self.message = Some(match result {
            Ok(report) => report,
            Err(report) => format!("failed: {report}"),
        });
        self.marks.clear();
        self.reload_keeping_selection();
        if tag != ASK_NEW_FILE || !created {
            return PageOutcome::Consumed;
        }
        // A file is made to be written in, so it opens with the cursor in it
        // rather than as one more empty row to find again. The listing lands
        // on it too, for when the editor closes over the top of it.
        let Some(path) = ops::resolve_name(&self.root, answer) else {
            return PageOutcome::Consumed;
        };
        if let Some(index) = self.rows.iter().position(|row| row.entry.path == path) {
            self.cursor = index;
        }
        PageOutcome::OpenPath(OpenTarget::file(path))
    }

    fn create(&mut self, name: &str, directory: bool) -> Result<String, String> {
        let Some(path) = ops::resolve_name(&self.root, name) else {
            return Err(format!("not a name: {name}"));
        };
        let result = if directory {
            ops::create_dir(&path)
        } else {
            ops::create_file(&path)
        };
        result
            .map(|()| format!("created {}", name_of(&path)))
            .map_err(|e| e.to_string())
    }

    fn rename_selected(&mut self, name: &str) -> Result<String, String> {
        let Some(row) = self.selected() else {
            return Err("nothing selected".to_string());
        };
        let from = row.entry.path.clone();
        let Some(to) = ops::resolve_name(&self.root, name) else {
            return Err(format!("not a name: {name}"));
        };
        ops::move_entry(&from, &to)
            .map(|()| format!("renamed to {}", name_of(&to)))
            .map_err(|e| e.to_string())
    }

    fn copy_targets(&mut self, destination: &str) -> Result<String, String> {
        self.transfer(destination, true)
    }

    fn move_targets(&mut self, destination: &str) -> Result<String, String> {
        self.transfer(destination, false)
    }

    /// Copy or move every target to `destination`, which is a name in this
    /// directory for a single target and a directory for several.
    fn transfer(&mut self, destination: &str, copy: bool) -> Result<String, String> {
        let targets = self.targets();
        let single = targets.len() == 1;
        let mut done = 0;
        for from in &targets {
            let to = if single {
                ops::resolve_name(&self.root, destination)
            } else {
                ops::resolve_name(&self.root, destination).map(|dir| dir.join(name_of(from)))
            };
            let Some(to) = to else {
                return Err(format!("not a name: {destination}"));
            };
            let result = if copy {
                ops::copy_entry(from, &to)
            } else {
                ops::move_entry(from, &to)
            };
            // Stop at the first failure with what was already done reported:
            // carrying on would bury the error under later successes.
            result.map_err(|e| format!("{} ({done} done)", e))?;
            done += 1;
        }
        Ok(format!("{} {done}", if copy { "copied" } else { "moved" }))
    }

    fn delete_targets(&mut self) -> Result<String, String> {
        let targets = self.targets();
        let mut done = 0;
        for path in &targets {
            ops::remove_entry(path).map_err(|e| format!("{} ({done} done)", e))?;
            done += 1;
        }
        Ok(format!("deleted {done}"))
    }

    fn change_mode(&mut self, text: &str) -> Result<String, String> {
        let Some(mode) = ops::parse_mode(text) else {
            return Err(format!("not an octal mode: {text}"));
        };
        let targets = self.targets();
        let mut done = 0;
        for path in &targets {
            ops::set_mode(path, mode).map_err(|e| format!("{} ({done} done)", e))?;
            done += 1;
        }
        Ok(format!("set mode on {done}"))
    }

    /// Turn directory sizes on or off. Turning them off stops the walks still
    /// running, which is the whole reason they are cancellable.
    fn toggle_sizes(&mut self) -> PageOutcome {
        self.show_sizes = !self.show_sizes;
        if !self.show_sizes {
            return PageOutcome::CancelJobs;
        }
        self.request_next_size()
    }

    /// Ask for the first directory on screen whose size is not known yet. One
    /// at a time, so a listing of a thousand directories does not start a
    /// thousand walks: each answer asks for the next.
    fn request_next_size(&self) -> PageOutcome {
        if !self.show_sizes {
            return PageOutcome::Consumed;
        }
        match self.next_unsized() {
            Some(path) => PageOutcome::Job(JobRequest::DirSize(path)),
            None => PageOutcome::Consumed,
        }
    }

    fn next_unsized(&self) -> Option<PathBuf> {
        self.rows
            .iter()
            .filter(|row| row.entry.is_dir())
            .map(|row| row.entry.path.clone())
            .find(|path| !self.sizes.contains_key(path))
    }

    /// Resolve the second key of a two-key sequence. An unrecognized follow key
    /// abandons the sequence rather than holding it for the next keystroke.
    fn resolve_pending(&mut self, leader: Leader, key: &Key) -> PageOutcome {
        self.pending = None;
        match (leader, key.code) {
            (Leader::Edit, KeyCode::Char('q')) if key.ctrl => {
                self.enter_edit();
            }
            (Leader::Fold, KeyCode::Char('a')) | (Leader::Fold, KeyCode::Char('A')) => {
                self.toggle_fold_all()
            }
            (Leader::Fold, KeyCode::Char('c')) => {
                self.folds.collapse_all();
                self.reload_keeping_selection();
            }
            (Leader::Fold, KeyCode::Char('u')) | (Leader::Fold, KeyCode::Char('d')) => {
                self.expand_subtree()
            }
            (Leader::Fold, KeyCode::Char('f')) => self.collapse_subtree(),
            (Leader::Fold, KeyCode::Char('t')) => self.toggle_subtree(),
            (Leader::Depth, KeyCode::Char(digit)) if digit.is_ascii_digit() => {
                let depth = digit.to_digit(10).unwrap_or(0) as usize;
                self.expand_to_depth(depth);
            }
            _ => {}
        }
        PageOutcome::Consumed
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
}

/// Where to scroll so a just-opened directory shows what it opened: the row
/// itself, once the rows nested under it run past the window's bottom, since
/// from there the most of them fit. `None` leaves the window alone, which is
/// the case whenever they already show in full and whenever nothing was
/// opened at all.
fn reveal_start(rows: &[Row], window: &PageWindow, at: Option<usize>) -> Option<usize> {
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

impl Page for DirPage {
    fn on_resume(&mut self) -> PageOutcome {
        // A file edited under the listing has a new size and a new mtime, and
        // a file created or deleted is a row that is not there.
        self.reload_keeping_selection();
        PageOutcome::Consumed
    }

    fn on_job(&mut self, reply: JobReply) -> PageOutcome {
        match reply {
            // A listing runs no commands and asks for no searches of its own.
            JobReply::Command(_) | JobReply::Files(_) | JobReply::Search(_) => {
                return PageOutcome::Consumed
            }
            JobReply::DirSize { bytes, path } => {
                self.sizes.insert(path, bytes);
            }
        }
        self.request_next_size()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        match reply.answer {
            // A search moves the cursor and nothing else, so it must not clear
            // the marks or re-read the listing the way an operation does.
            Some(answer) if reply.tag == ASK_SEARCH => self.search_for(&answer),
            Some(answer) => return self.apply_answer(reply.tag, &answer),
            None => self.message = Some("cancelled".to_string()),
        }
        PageOutcome::Consumed
    }

    fn title(&self) -> String {
        self.root
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| self.root.to_string_lossy().to_string())
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
        let now = SystemTime::now();
        // Remember the viewport for the half-page motions, which key handling
        // needs between paints.
        self.viewport = rows.saturating_sub(HEADER_ROWS);
        let mut page_rows = vec![rows::header_row(
            &self.root.to_string_lossy(),
            self.sort,
            rows::HeaderFlags {
                editing: self.edit.as_ref().map(|edit| match edit.mode() {
                    EditMode::Normal => "normal",
                    EditMode::Insert => "insert",
                }),
                show_hidden: self.show_hidden,
                show_details: self.show_details,
                show_sizes: self.show_sizes,
            },
            self.marks.len(),
            self.message.as_deref(),
        )];
        if self.rows.is_empty() {
            page_rows.push(vec![PageSpan::new(PageStyle::Dim, EMPTY_NOTE)]);
            return PageContent::new(page_rows);
        }
        // Taken before the borrow below, which holds the listing for as long
        // as the window is being measured.
        let reveal = self.reveal.take();
        // An entry's spans, built on demand: the wrapping window walks only
        // the rows it may paint, so a listing longer than the pane is not
        // built in full to measure it.
        let entry_spans = |index: usize| {
            let row = &self.rows[index];
            let edited = self.edited_name(index, row);
            rows::entry_row(
                row,
                edited.as_ref(),
                rows::RowStyle {
                    marked: self.marks.contains(&row.entry.path),
                    show_details: self.show_details,
                    show_sizes: self.show_sizes,
                    size: self.dir_size(row),
                },
                now,
            )
            .0
        };
        let visible = rows.saturating_sub(HEADER_ROWS);
        let height = |index: usize| row_height(&row_text(&entry_spans(index)), cols, wrap, 0);
        let mut window = wrap_window(self.scroll, self.cursor, self.rows.len(), visible, height);
        if let Some(start) = reveal_start(&self.rows, &window, reveal) {
            window = wrap_window(start, self.cursor, self.rows.len(), visible, height);
        }
        self.scroll = window.start;
        let mut icons: Vec<PageIcon> = Vec::new();
        for (index, row) in self
            .rows
            .iter()
            .enumerate()
            .skip(window.start)
            .take(window.count)
        {
            let edited = self.edited_name(index, row);
            let (spans, mut icon) = rows::entry_row(
                row,
                edited.as_ref(),
                rows::RowStyle {
                    marked: self.marks.contains(&row.entry.path),
                    show_details: self.show_details,
                    show_sizes: self.show_sizes,
                    size: self.dir_size(row),
                },
                now,
            );
            icon.row = page_rows.len();
            page_rows.push(spans);
            icons.push(icon);
        }
        PageContent::new(page_rows)
            .with_icons(icons)
            .with_cursor_line(HEADER_ROWS + window.cursor)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        if self.edit.is_some() {
            return self.on_edit_key(key);
        }
        if let Some(leader) = self.pending {
            return self.resolve_pending(leader, key);
        }
        if self.nav.in_sequence() {
            return match self.nav.key(key) {
                VimKey::Motion(motion) => {
                    self.apply_motion(motion);
                    PageOutcome::Consumed
                }
                _ => PageOutcome::Consumed,
            };
        }
        if key.alt {
            return self.on_alt_key(key);
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        match key.code {
            KeyCode::Char('z') => {
                self.pending = Some(Leader::Fold);
                PageOutcome::Consumed
            }
            KeyCode::Enter => self.enter(),
            KeyCode::Char('l') | KeyCode::Right => {
                self.expand_selected(true);
                PageOutcome::Consumed
            }
            KeyCode::Char('h') | KeyCode::Left => {
                self.fold_or_step_out();
                PageOutcome::Consumed
            }
            KeyCode::Backspace => {
                self.ascend();
                PageOutcome::Consumed
            }
            KeyCode::Tab if key.shift => {
                self.toggle_fold_all();
                PageOutcome::Consumed
            }
            KeyCode::Tab => {
                self.toggle_fold();
                PageOutcome::Consumed
            }
            KeyCode::Char('/') => self.ask(ASK_SEARCH, "/".to_string(), String::new()),
            KeyCode::Char('n') => {
                self.search_step(true);
                PageOutcome::Consumed
            }
            KeyCode::Char('N') => {
                self.search_step(false);
                PageOutcome::Consumed
            }
            KeyCode::Char('.') => {
                self.show_hidden = !self.show_hidden;
                self.reload_keeping_selection();
                PageOutcome::Consumed
            }
            KeyCode::Char(',') => {
                self.show_details = !self.show_details;
                PageOutcome::Consumed
            }
            KeyCode::Char('s') => {
                self.sort = self.sort.next();
                self.reload_keeping_selection();
                PageOutcome::Consumed
            }
            KeyCode::Char('G') => {
                self.reload_keeping_selection();
                PageOutcome::Consumed
            }
            KeyCode::Char('m') => {
                self.toggle_mark();
                PageOutcome::Consumed
            }
            KeyCode::Char('M') => {
                self.mark_all();
                PageOutcome::Consumed
            }
            KeyCode::Char('u') => {
                self.unmark_selected();
                PageOutcome::Consumed
            }
            KeyCode::Char('U') => {
                self.marks.clear();
                PageOutcome::Consumed
            }
            KeyCode::Char('_') => self.ask(ASK_NEW_FILE, "New file: ".to_string(), String::new()),
            KeyCode::Char('+') => self.ask(ASK_MKDIR, "New directory: ".to_string(), String::new()),
            KeyCode::Char('R') => self.ask_rename(),
            KeyCode::Char('C') => self.ask_destination(ASK_COPY, "Copy"),
            KeyCode::Char('x') => self.ask_delete(),
            KeyCode::Char('*') => self.ask(ASK_MODE, "Mode: ".to_string(), String::new()),
            KeyCode::Char('&') => self
                .selected()
                .map(|row| PageOutcome::OpenExternal(row.entry.path.clone()))
                .unwrap_or(PageOutcome::Consumed),
            // Escape stops the size walks when any are running, since that is
            // the slow thing a listing does; with none running it closes.
            KeyCode::Escape if self.show_sizes => self.toggle_sizes(),
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            // Everything unclaimed falls through to the shared Vim motion
            // layer, whose motions this listing interprets over its rows.
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

    fn context_items(&self) -> Vec<PageMenuItem> {
        let Some(row) = self.selected() else {
            // Below the listing there is no entry to act on, so the only
            // things left are the ones that make a new one.
            return vec![
                PageMenuItem::new(Key::plain(KeyCode::Char('_')), LABEL_NEW_FILE),
                PageMenuItem::new(Key::plain(KeyCode::Char('+')), LABEL_NEW_DIR),
                PageMenuItem::new(Key::plain(KeyCode::Char('G')), LABEL_RELOAD),
            ];
        };
        let is_dir = row.entry.is_dir();
        let marked = self.marks.contains(&row.entry.path);
        let mut items = vec![PageMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN)];
        if !is_dir {
            items.push(PageMenuItem::new(
                Key::with_ctrl(KeyCode::Char('o')),
                LABEL_OPEN_IN_EDITOR,
            ));
            items.push(PageMenuItem::new(
                Key::plain(KeyCode::Char('&')),
                LABEL_OPEN_EXTERNAL,
            ));
        }
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('R')),
            LABEL_RENAME,
        ));
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('C')),
            LABEL_COPY,
        ));
        items.push(PageMenuItem::new(
            Key::with_alt(KeyCode::Char('m')),
            LABEL_MOVE,
        ));
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('x')),
            LABEL_DELETE,
        ));
        items.push(match marked {
            true => PageMenuItem::new(Key::plain(KeyCode::Char('u')), LABEL_UNMARK),
            false => PageMenuItem::new(Key::plain(KeyCode::Char('m')), LABEL_MARK),
        });
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('_')),
            LABEL_NEW_FILE,
        ));
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('+')),
            LABEL_NEW_DIR,
        ));
        items
    }
}

// ========================================================================
// DirPage: modified keys
// ========================================================================

impl DirPage {
    fn on_alt_key(&mut self, key: &Key) -> PageOutcome {
        if let Some(motion) = buffer_end(key) {
            self.apply_motion(motion);
            return PageOutcome::Consumed;
        }
        match key.code {
            KeyCode::Char('n') => {
                if let Some(next) = self.sibling_after(self.cursor) {
                    self.cursor = next;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('p') => {
                if let Some(previous) = self.sibling_before(self.cursor) {
                    self.cursor = previous;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('u') => {
                if let Some(parent) = self.parent_row(self.cursor) {
                    self.cursor = parent;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('d') => {
                if let Some(child) = self.first_child(self.cursor) {
                    self.cursor = child;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('m') => self.ask_destination(ASK_MOVE, "Move"),
            // Shift-Alt pairs: history, and the size walk.
            KeyCode::Char('B') => {
                self.go_back();
                PageOutcome::Consumed
            }
            KeyCode::Char('F') => {
                self.go_forward();
                PageOutcome::Consumed
            }
            KeyCode::Char('S') => self.toggle_sizes(),
            _ => PageOutcome::Ignored,
        }
    }

    fn on_ctrl_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            KeyCode::Char('c') => {
                self.pending = Some(Leader::Depth);
                PageOutcome::Consumed
            }
            KeyCode::Char('x') => {
                self.pending = Some(Leader::Edit);
                PageOutcome::Consumed
            }
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
        }
    }
}

// ========================================================================
// DirPage: editing the names
// ========================================================================

impl DirPage {
    /// Turn every listed name into editable text, the caret parked at the end
    /// of the name under the cursor, where a rename usually begins.
    fn enter_edit(&mut self) {
        if self.rows.is_empty() {
            return;
        }
        self.edit = Some(EditState::new(&self.rows, self.cursor));
    }

    /// Keys while the names are editable text. `Ctrl-X Ctrl-Q` toggles the
    /// session off from either mode; everything else goes to the editor,
    /// whose Vim Normal/Insert pair reports row moves and the session's end
    /// back as an [`EditAction`].
    fn on_edit_key(&mut self, key: &Key) -> PageOutcome {
        if self.pending == Some(Leader::Edit) {
            self.pending = None;
            if key.ctrl && key.code == KeyCode::Char('q') {
                self.discard_edit();
            }
            return PageOutcome::Consumed;
        }
        if key.ctrl && key.code == KeyCode::Char('x') {
            self.pending = Some(Leader::Edit);
            return PageOutcome::Consumed;
        }
        let row = self.cursor;
        let action = match self.edit.as_mut() {
            Some(edit) => edit.on_key(row, key),
            None => return PageOutcome::Ignored,
        };
        match action {
            EditAction::Consumed => {}
            EditAction::Ignored => return PageOutcome::Ignored,
            EditAction::MoveRows(delta) => {
                self.move_by(delta);
                self.clamp_edit_caret();
            }
            EditAction::MoveToRow(row) => {
                self.cursor = row.min(self.rows.len().saturating_sub(1));
                self.clamp_edit_caret();
            }
            EditAction::Apply => return self.apply_edits(),
            EditAction::Leave => self.discard_edit(),
        }
        PageOutcome::Consumed
    }

    /// Keep the edit caret inside the name of the row the cursor moved to.
    fn clamp_edit_caret(&mut self) {
        if let Some(edit) = self.edit.as_mut() {
            edit.move_to_row(self.cursor);
        }
    }

    /// Leave edit mode without renaming anything.
    fn discard_edit(&mut self) {
        let pending = self.pending_edits().len();
        self.edit = None;
        // Names snapping back to the disk's can read as the edits having been
        // applied; saying they were not is one line of insurance.
        self.message = (pending > 0).then(|| format!("discarded {pending}"));
    }

    /// Apply the edited names as renames. Every name is validated before any
    /// rename runs, so one bad name cannot leave the batch half-applied; the
    /// renames then run deepest paths first, so renaming a directory never
    /// orphans the pending rename of an entry inside it.
    fn apply_edits(&mut self) -> PageOutcome {
        let pending = self.pending_edits();
        if pending.is_empty() {
            self.edit = None;
            self.message = Some("no changes".to_string());
            return PageOutcome::Consumed;
        }
        let mut renames: Vec<(usize, PathBuf, PathBuf)> = Vec::new();
        for (index, name) in &pending {
            let from = self.rows[*index].entry.path.clone();
            let Some(to) = from
                .parent()
                .and_then(|dir| ops::resolve_name(dir, name.as_str()))
            else {
                // Stay in edit mode: the name is still on screen, still
                // editable, and now flagged as the problem.
                self.message = Some(format!("failed: not a name: {name}"));
                return PageOutcome::Consumed;
            };
            renames.push((*index, from, to));
        }
        renames.sort_by_key(|(index, _, _)| std::cmp::Reverse(self.rows[*index].depth));
        self.edit = None;
        let mut done = 0;
        let mut failure = None;
        for (_, from, to) in renames {
            // Stop at the first failure with what was already done reported:
            // carrying on would bury the error under later successes.
            if let Err(e) = ops::move_entry(&from, &to) {
                failure = Some(format!("{e} ({done} done)"));
                break;
            }
            done += 1;
        }
        self.message = Some(match failure {
            Some(report) => format!("failed: {report}"),
            None => format!("renamed {done}"),
        });
        self.marks.clear();
        self.reload_keeping_selection();
        PageOutcome::Consumed
    }

    /// The rows whose edited name no longer matches the entry on disk.
    fn pending_edits(&self) -> Vec<(usize, String)> {
        let Some(edit) = self.edit.as_ref() else {
            return Vec::new();
        };
        self.rows
            .iter()
            .enumerate()
            .filter_map(|(index, row)| {
                let name = edit.name(index)?;
                (name != row.entry.name).then(|| (index, name.to_string()))
            })
            .collect()
    }

    /// How `index`'s name is drawn while editing: the edited text, a caret on
    /// the cursor's row, and whether the edit has drifted from the disk.
    fn edited_name(&self, index: usize, row: &Row) -> Option<rows::EditedName> {
        let edit = self.edit.as_ref()?;
        let text = edit.name(index)?.to_string();
        Some(rows::EditedName {
            caret: (index == self.cursor).then(|| edit.col()),
            changed: text != row.entry.name,
            text,
        })
    }
}

// ========================================================================
// Helpers
// ========================================================================

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
mod tests {
    use super::*;
    use std::fs;

    /// A temporary tree that removes itself on drop.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str) -> Self {
            let dir =
                std::env::temp_dir().join(format!("winter-dirpage-{tag}-{}", std::process::id()));
            fs::create_dir_all(&dir).expect("temp dir");
            Self(dir)
        }

        fn path(&self, name: &str) -> PathBuf {
            self.0.join(name)
        }

        fn dir(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::create_dir_all(&path).expect("temp subdir");
            path
        }

        fn touch(&self, name: &str) -> PathBuf {
            let path = self.0.join(name);
            fs::write(&path, "x").expect("temp file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).ok();
        }
    }

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

    fn ctrl(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
        }
    }

    /// Toggle the name editor with its `Ctrl-X Ctrl-Q` chord.
    fn toggle_edit(page: &mut DirPage) {
        page.on_key(&ctrl(KeyCode::Char('x')));
        page.on_key(&ctrl(KeyCode::Char('q')));
    }

    fn selected_name(page: &DirPage) -> String {
        page.selected()
            .map(|row| row.entry.name.clone())
            .unwrap_or_default()
    }

    #[test]
    fn test_every_menu_entry_runs_a_key_the_listing_binds() {
        // The menu offers keys rather than commands of its own, so an entry
        // naming a chord the page does not match is an entry that silently
        // does nothing when it is chosen. Nothing else checks the two sides
        // against each other.
        let tree = TempTree::new("menu_keys");
        tree.touch("a.txt");
        tree.dir("sub");
        let page = DirPage::new(tree.0.clone());

        for item in page.context_items() {
            let mut probe = DirPage::new(tree.0.clone());
            assert_ne!(
                probe.on_key(&item.key),
                PageOutcome::Ignored,
                "the menu offers {:?}, which the listing does not bind",
                item.label
            );
        }
    }

    #[test]
    fn test_the_menu_offers_a_directory_only_what_a_directory_can_do() {
        // Handing a directory to $EDITOR or to the system handler as if it
        // were a file is the mistake a menu built without looking at the row
        // makes.
        let tree = TempTree::new("menu_rows");
        tree.touch("a.txt");
        tree.dir("sub");
        let mut page = DirPage::new(tree.0.clone());

        let labels = |page: &DirPage| -> Vec<String> {
            page.context_items()
                .into_iter()
                .map(|item| item.label)
                .collect()
        };

        while selected_name(&page) != "sub" {
            page.on_key(&press(KeyCode::Char('j')));
        }
        assert!(!labels(&page).contains(&LABEL_OPEN_IN_EDITOR.to_string()));

        while selected_name(&page) != "a.txt" {
            page.on_key(&press(KeyCode::Char('j')));
        }
        assert!(labels(&page).contains(&LABEL_OPEN_IN_EDITOR.to_string()));
    }

    #[test]
    fn test_the_emacs_buffer_ends_reach_the_first_and_last_entry() {
        let tree = TempTree::new("buffer_ends");
        tree.touch("a.txt");
        tree.touch("b.txt");
        tree.touch("c.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&alt(KeyCode::Char('>')));
        assert_eq!(selected_name(&page), "c.txt");
        page.on_key(&alt(KeyCode::Char('<')));
        assert_eq!(selected_name(&page), "a.txt");
    }

    #[test]
    fn test_opening_a_directory_scrolls_its_children_into_a_short_pane() {
        // Unfolding at the bottom of the pane used to leave the children
        // below the fold: the cursor stayed on the directory, which was
        // already visible, so nothing scrolled.
        let tree = TempTree::new("reveal");
        tree.dir("a-first");
        tree.dir("b-second");
        let last = tree.dir("c-last");
        for name in ["a.txt", "b.txt", "c.txt", "d.txt"] {
            fs::write(last.join(name), "x").expect("child");
        }
        let mut page = DirPage::new(tree.0.clone());

        // A pane with room for the three directories and nothing else, with
        // the cursor on the last of them.
        page.on_key(&press(KeyCode::Char('j')));
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(selected_name(&page), "c-last", "sorted after the other two");
        page.content(4, 80, false);
        page.on_key(&press(KeyCode::Tab));
        let painted: Vec<String> = page
            .content(4, 80, false)
            .rows
            .iter()
            .map(row_text)
            .collect();
        assert!(
            painted.iter().any(|row| row.contains("a.txt")),
            "the children the fold opened are on screen, got {painted:?}"
        );
    }

    #[test]
    fn test_cursor_stays_inside_the_listing() {
        let tree = TempTree::new("clamp");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let mut page = DirPage::new(tree.0.clone());

        for _ in 0..5 {
            page.on_key(&press(KeyCode::Char('j')));
        }
        assert_eq!(selected_name(&page), "b.txt");
        for _ in 0..5 {
            page.on_key(&press(KeyCode::Char('k')));
        }
        assert_eq!(selected_name(&page), "a.txt");
    }

    #[test]
    fn test_entering_a_directory_makes_it_the_root() {
        let tree = TempTree::new("descend");
        let nested = tree.dir("nested");
        fs::write(nested.join("inner.txt"), "x").expect("inner file");
        let mut page = DirPage::new(tree.0.clone());

        assert_eq!(page.on_key(&press(KeyCode::Enter)), PageOutcome::Consumed);
        assert_eq!(page.root, nested);
        assert_eq!(selected_name(&page), "inner.txt");
    }

    #[test]
    fn test_entering_a_file_asks_the_host_to_open_it() {
        let tree = TempTree::new("open");
        let file = tree.touch("readme.md");
        let mut page = DirPage::new(tree.0.clone());

        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::file(file))
        );
    }

    #[test]
    fn test_going_up_lands_on_the_directory_just_left() {
        // Stepping up used to reset to the first row, so `l` then `h` walked
        // away from where the user was instead of back to it.
        let tree = TempTree::new("ascend");
        tree.dir("aaa");
        let target = tree.dir("zzz");
        let mut page = DirPage::new(target.clone());

        page.on_key(&press(KeyCode::Char('h')));
        assert_eq!(page.root, tree.0);
        assert_eq!(selected_name(&page), "zzz");
    }

    #[test]
    fn test_going_up_from_the_filesystem_root_is_a_no_op() {
        let mut page = DirPage::new(PathBuf::from("/"));
        page.on_key(&press(KeyCode::Char('h')));
        assert_eq!(page.root, PathBuf::from("/"));
    }

    #[test]
    fn test_showing_dotfiles_keeps_the_cursor_on_the_same_entry() {
        // A reload renumbers every row, so a naive toggle leaves the cursor on
        // whatever entry inherited its index.
        let tree = TempTree::new("dotfiles");
        tree.touch(".hidden");
        tree.touch("visible.txt");
        let mut page = DirPage::new(tree.0.clone());
        assert_eq!(selected_name(&page), "visible.txt");

        page.on_key(&press(KeyCode::Char('.')));
        assert_eq!(page.rows.len(), 2, "the dotfile joined the listing");
        assert_eq!(selected_name(&page), "visible.txt");
    }

    #[test]
    fn test_folding_a_directory_lists_its_children_in_place() {
        let tree = TempTree::new("fold");
        let nested = tree.dir("nested");
        fs::write(nested.join("inner.txt"), "x").expect("inner file");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Tab));
        assert_eq!(page.rows.len(), 2);
        assert_eq!(page.root, tree.0, "expanding in place never moves the root");
        assert_eq!(selected_name(&page), "nested");

        page.on_key(&press(KeyCode::Tab));
        assert_eq!(page.rows.len(), 1);
    }

    #[test]
    fn test_home_and_end_jump_to_the_ends() {
        let tree = TempTree::new("jumps");
        tree.touch("a.txt");
        tree.touch("b.txt");
        tree.touch("c.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::End));
        assert_eq!(selected_name(&page), "c.txt");
        page.on_key(&press(KeyCode::Home));
        assert_eq!(selected_name(&page), "a.txt");
    }

    #[test]
    fn test_a_stray_fold_sequence_is_abandoned_not_left_pending() {
        // A pending `z` that survived an unrelated key would swallow the next
        // keystroke as well.
        let tree = TempTree::new("pending");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('!')));
        assert!(page.pending.is_none());
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(selected_name(&page), "b.txt");
    }

    fn tree_with_nested_children(tag: &str) -> (TempTree, PathBuf) {
        let tree = TempTree::new(tag);
        let nested = tree.dir("nested");
        fs::write(nested.join("child-a.txt"), "x").expect("child a");
        fs::write(nested.join("child-b.txt"), "x").expect("child b");
        tree.touch("zzz-sibling.txt");
        (tree, nested)
    }

    #[test]
    fn test_siblings_stay_inside_the_subtree_the_cursor_is_in() {
        // A naive "next row at this depth" scan walks out of an expanded
        // directory and lands in the parent's next entry instead.
        let (tree, _) = tree_with_nested_children("siblings");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(selected_name(&page), "child-a.txt");

        page.on_key(&alt(KeyCode::Char('n')));
        assert_eq!(selected_name(&page), "child-b.txt");
        page.on_key(&alt(KeyCode::Char('n')));
        assert_eq!(selected_name(&page), "child-b.txt", "no escape upward");
        page.on_key(&alt(KeyCode::Char('p')));
        assert_eq!(selected_name(&page), "child-a.txt");
    }

    #[test]
    fn test_parent_and_first_child_walk_the_tree_vertically() {
        let (tree, _) = tree_with_nested_children("vertical");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));

        page.on_key(&alt(KeyCode::Char('d')));
        assert_eq!(selected_name(&page), "child-a.txt");
        page.on_key(&alt(KeyCode::Char('u')));
        assert_eq!(selected_name(&page), "nested");
        page.on_key(&alt(KeyCode::Char('u')));
        assert_eq!(
            selected_name(&page),
            "nested",
            "the top level has no parent"
        );
    }

    #[test]
    fn test_h_collapses_then_steps_out_then_leaves_the_root() {
        // One key covering three cases: each must be tried in this order, or
        // collapsing a directory would also move the cursor off it.
        let (tree, _) = tree_with_nested_children("stepout");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(selected_name(&page), "child-a.txt");

        page.on_key(&press(KeyCode::Char('h')));
        assert_eq!(
            selected_name(&page),
            "nested",
            "steps out to the parent row"
        );
        page.on_key(&press(KeyCode::Char('h')));
        assert_eq!(selected_name(&page), "nested", "collapses, staying put");
        assert_eq!(page.rows.len(), 2, "the children are gone");

        page.on_key(&press(KeyCode::Char('h')));
        assert_eq!(page.root, tree.0.parent().expect("a parent").to_path_buf());
    }

    #[test]
    fn test_history_walks_back_and_forward_through_visited_directories() {
        let (tree, nested) = tree_with_nested_children("history");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Enter));
        assert_eq!(page.root, nested);

        page.on_key(&alt(KeyCode::Char('B')));
        assert_eq!(page.root, tree.0, "back returns to where it came from");
        page.on_key(&alt(KeyCode::Char('F')));
        assert_eq!(page.root, nested, "forward undoes the step back");
    }

    #[test]
    fn test_history_at_either_end_is_a_no_op() {
        let (tree, _) = tree_with_nested_children("history-ends");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&alt(KeyCode::Char('B')));
        assert_eq!(page.root, tree.0);
        page.on_key(&alt(KeyCode::Char('F')));
        assert_eq!(page.root, tree.0);
    }

    #[test]
    fn test_moving_somewhere_new_drops_the_forward_history() {
        // Otherwise `forward` points at a branch the user has left, and
        // Ctrl-i teleports somewhere unrelated.
        let (tree, nested) = tree_with_nested_children("history-branch");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Enter));
        page.on_key(&alt(KeyCode::Char('B')));
        assert!(!page.forward.is_empty());

        page.on_key(&press(KeyCode::Backspace));
        assert!(page.forward.is_empty());
        page.on_key(&alt(KeyCode::Char('F')));
        assert_ne!(page.root, nested);
    }

    #[test]
    fn test_za_opens_one_level_then_closes_everything() {
        // One key that always does what the listing is not already showing:
        // open when shut, shut when anything is open.
        let tree = TempTree::new("fold-all");
        let nested = tree.dir("nested");
        fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
        let mut page = DirPage::new(tree.0.clone());

        assert_eq!(page.rows.len(), 1);
        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('a')));
        assert_eq!(page.rows.len(), 2, "nested is open, deeper is not");

        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('a')));
        assert_eq!(page.rows.len(), 1, "and closed again");
    }

    #[test]
    fn test_the_depth_keys_open_exactly_that_many_levels() {
        // Reading the whole tree to show two levels is the mistake; each depth
        // must read only as far as it shows.
        let tree = TempTree::new("depth");
        let nested = tree.dir("nested");
        fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
        fs::write(nested.join("deeper").join("leaf.txt"), "x").expect("leaf");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&ctrl(KeyCode::Char('c')));
        page.on_key(&press(KeyCode::Char('1')));
        assert_eq!(page.rows.len(), 2, "nested opens, deeper stays shut");

        page.on_key(&ctrl(KeyCode::Char('c')));
        page.on_key(&press(KeyCode::Char('2')));
        assert_eq!(page.rows.len(), 3, "deeper opens, showing its leaf");

        page.on_key(&ctrl(KeyCode::Char('c')));
        page.on_key(&press(KeyCode::Char('0')));
        assert_eq!(page.rows.len(), 1, "depth zero is everything closed");
    }

    #[test]
    fn test_zu_opens_a_whole_subtree_and_zf_shuts_it() {
        let tree = TempTree::new("subtree");
        let nested = tree.dir("nested");
        fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
        fs::write(nested.join("deeper").join("leaf.txt"), "x").expect("leaf");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('u')));
        assert_eq!(page.rows.len(), 3, "the whole subtree, in one key");

        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('f')));
        assert_eq!(page.rows.len(), 1, "and shut, children included");
    }

    #[test]
    fn test_l_expands_and_is_not_a_toggle() {
        // `l` on an open directory must leave it open: it opens, and `h` is
        // what closes, so holding either never flaps the tree.
        let (tree, _) = tree_with_nested_children("expand-key");
        let mut page = DirPage::new(tree.0.clone());

        for _ in 0..2 {
            page.on_key(&press(KeyCode::Char('l')));
        }
        assert_eq!(page.rows.len(), 4, "still expanded");

        page.on_key(&press(KeyCode::Char('h')));
        assert_eq!(page.rows.len(), 2, "closed, cursor still on it");
        assert_eq!(selected_name(&page), "nested");
    }

    #[test]
    fn test_zc_collapses_everything() {
        let (tree, _) = tree_with_nested_children("zm");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));
        assert_eq!(page.rows.len(), 4);

        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('c')));
        assert_eq!(page.rows.len(), 2);
    }

    #[test]
    fn test_o_hands_the_entry_to_the_system_handler() {
        let tree = TempTree::new("external");
        let file = tree.touch("photo.png");
        let mut page = DirPage::new(tree.0.clone());
        assert_eq!(
            page.on_key(&press(KeyCode::Char('&'))),
            PageOutcome::OpenExternal(file)
        );
    }

    /// Answer whichever way the question was asked: a destination is offered
    /// as a list of the directories in reach, everything else as a prompt,
    /// and both are answered under the same tag.
    fn answer(page: &mut DirPage, outcome: PageOutcome, text: &str) {
        let tag = match outcome {
            PageOutcome::Prompt(request) => request.tag,
            PageOutcome::Pick(request) => request.tag,
            other => panic!("expected a question, got {other:?}"),
        };
        page.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag,
        });
    }

    #[test]
    fn test_a_new_file_opens_for_writing_and_the_listing_lands_on_it() {
        // A file is created to be written in. Leaving it as one more row in
        // the listing means finding it again before anything can go in it.
        let tree = TempTree::new("new-file");
        tree.touch("other.txt");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('_')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        let opened = page.on_prompt(PromptReply {
            answer: Some("notes.md".to_string()),
            tag: request.tag,
        });

        let path = tree.0.join("notes.md");
        assert!(path.exists(), "the file was created");
        assert_eq!(
            opened,
            PageOutcome::OpenPath(OpenTarget::file(path.clone()))
        );
        assert_eq!(
            page.selected().map(|row| row.entry.path.clone()),
            Some(path),
            "and the listing is on it for when the editor closes"
        );
    }

    #[test]
    fn test_a_new_directory_is_not_opened_for_writing() {
        // Handing a directory to the editor would report a read failure at a
        // moment nothing went wrong.
        let tree = TempTree::new("new-dir");
        let mut page = DirPage::new(tree.0.clone());
        let outcome = page.on_key(&press(KeyCode::Char('+')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        assert_eq!(
            page.on_prompt(PromptReply {
                answer: Some("src".to_string()),
                tag: request.tag,
            }),
            PageOutcome::Consumed
        );
        assert!(tree.0.join("src").is_dir());
    }

    #[test]
    fn test_a_name_that_could_not_be_created_opens_nothing() {
        // The failure is reported in the header; opening the file that was
        // never made would report a second, more confusing one.
        let tree = TempTree::new("new-file-clash");
        tree.touch("taken.txt");
        let mut page = DirPage::new(tree.0.clone());
        let outcome = page.on_key(&press(KeyCode::Char('_')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a question, got {outcome:?}");
        };
        assert_eq!(
            page.on_prompt(PromptReply {
                answer: Some("taken.txt".to_string()),
                tag: request.tag,
            }),
            PageOutcome::Consumed
        );
    }

    #[test]
    fn test_marking_moves_on_so_a_run_of_marks_is_one_key_each() {
        let tree = TempTree::new("mark-run");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('m')));
        page.on_key(&press(KeyCode::Char('m')));
        assert_eq!(page.marks.len(), 2);
        assert_eq!(page.targets().len(), 2);
    }

    #[test]
    fn test_an_operation_acts_on_the_marks_not_the_cursor() {
        // The cursor sits on an unmarked entry: deleting must leave it alone.
        let tree = TempTree::new("mark-target");
        tree.touch("marked.txt");
        tree.touch("untouched.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('m')));
        assert_eq!(selected_name(&page), "untouched.txt");
        let outcome = page.on_key(&press(KeyCode::Char('x')));
        answer(&mut page, outcome, "y");

        assert!(!tree.path("marked.txt").exists());
        assert!(tree.path("untouched.txt").exists());
    }

    #[test]
    fn test_a_delete_prompt_says_how_many_it_will_take() {
        let tree = TempTree::new("delete-count");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Char('m')));
        page.on_key(&press(KeyCode::Char('m')));

        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('x'))) else {
            panic!("expected a prompt");
        };
        assert!(request.label.contains('2'), "got {:?}", request.label);
        assert_eq!(request.mode, PromptMode::Confirm);
    }

    #[test]
    fn test_declining_a_delete_keeps_the_files() {
        let tree = TempTree::new("delete-no");
        tree.touch("keep.txt");
        let mut page = DirPage::new(tree.0.clone());

        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('x'))) else {
            panic!("expected a prompt");
        };
        page.on_prompt(PromptReply {
            answer: None,
            tag: request.tag,
        });
        assert!(tree.path("keep.txt").exists());
    }

    #[test]
    fn test_create_rename_and_delete_walk_the_listing_along() {
        let tree = TempTree::new("crud");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('_')));
        answer(&mut page, outcome, "notes.txt");
        assert!(tree.path("notes.txt").exists());
        assert_eq!(selected_name(&page), "notes.txt", "the cursor follows it");

        let outcome = page.on_key(&press(KeyCode::Char('R')));
        answer(&mut page, outcome, "renamed.md");
        assert!(tree.path("renamed.md").exists());
        assert!(!tree.path("notes.txt").exists());

        let outcome = page.on_key(&press(KeyCode::Char('x')));
        answer(&mut page, outcome, "y");
        assert!(!tree.path("renamed.md").exists());
        assert!(page.rows.is_empty());
    }

    #[test]
    fn test_a_new_directory_joins_the_listing() {
        let tree = TempTree::new("mkdir");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('+')));
        answer(&mut page, outcome, "sub");
        assert!(tree.path("sub").is_dir());
    }

    #[test]
    fn test_marked_entries_are_transferred_into_a_directory_chosen_from_a_list() {
        // The same picker the Git view chooses a branch with, over what a
        // destination may be here: a directory of this listing, by name,
        // since a transfer refuses anything reaching outside it.
        let tree = TempTree::new("destinations");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let nested = tree.dir("nested");
        let mut page = DirPage::new(tree.0.clone());
        // The directory sorts first, so mark the two files after it.
        page.on_key(&press(KeyCode::Char('j')));
        page.on_key(&press(KeyCode::Char('m')));
        page.on_key(&press(KeyCode::Char('m')));

        let PageOutcome::Pick(request) = page.on_key(&press(KeyCode::Char('C'))) else {
            panic!("expected a list of directories");
        };
        assert_eq!(request.label, "Copy 2 entries into");
        assert_eq!(request.items, ["nested"], "by name, not by path");

        // The choice lands as the answer to the same question.
        page.on_prompt(PromptReply {
            answer: Some("nested".to_string()),
            tag: request.tag,
        });
        assert!(nested.join("a.txt").exists(), "the copy went there");
    }

    #[test]
    fn test_one_entry_is_still_asked_for_by_name() {
        // A single transfer takes the new name to give it, which is nothing
        // a list could offer.
        let tree = TempTree::new("single");
        tree.touch("a.txt");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('C')));
        assert!(matches!(outcome, PageOutcome::Prompt(_)), "got {outcome:?}");
    }

    #[test]
    fn test_copy_and_move_of_a_single_entry_use_the_answer_as_its_name() {
        let tree = TempTree::new("transfer");
        tree.touch("original.txt");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('C')));
        answer(&mut page, outcome, "copy.txt");
        assert!(tree.path("copy.txt").exists());
        assert!(
            tree.path("original.txt").exists(),
            "a copy leaves the source"
        );

        let outcome = page.on_key(&alt(KeyCode::Char('m')));
        answer(&mut page, outcome, "moved.txt");
        assert!(tree.path("moved.txt").exists());
    }

    #[test]
    fn test_several_marked_entries_transfer_into_a_directory() {
        let tree = TempTree::new("transfer-many");
        tree.touch("one.txt");
        tree.touch("two.txt");
        tree.dir("target");
        let mut page = DirPage::new(tree.0.clone());

        // The directory sorts first, so mark the two files after it.
        page.on_key(&press(KeyCode::Char('j')));
        page.on_key(&press(KeyCode::Char('m')));
        page.on_key(&press(KeyCode::Char('m')));
        assert_eq!(page.marks.len(), 2);

        let outcome = page.on_key(&alt(KeyCode::Char('m')));
        answer(&mut page, outcome, "target");
        assert!(tree.path("target").join("one.txt").exists());
        assert!(tree.path("target").join("two.txt").exists());
    }

    #[test]
    fn test_an_operation_reports_a_failure_instead_of_pretending() {
        // A name that is not a name must be refused visibly, and must not be
        // reported as a success.
        let tree = TempTree::new("bad-name");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('_')));
        answer(&mut page, outcome, "../escape.txt");
        let message = page.message.clone().unwrap_or_default();
        assert!(message.starts_with("failed:"), "got {message:?}");
        assert!(!tree.0.parent().expect("parent").join("escape.txt").exists());
    }

    #[test]
    fn test_an_operation_clears_the_marks_it_consumed() {
        // Marks left behind would silently widen the next operation.
        let tree = TempTree::new("marks-cleared");
        tree.touch("a.txt");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Char('m')));

        let outcome = page.on_key(&press(KeyCode::Char('C')));
        answer(&mut page, outcome, "b.txt");
        assert!(page.marks.is_empty());
    }

    #[test]
    fn test_moving_the_root_clears_the_marks() {
        // A mark that survived into a different listing would be counted by the
        // next operation without ever being visible in it.
        let (tree, nested) = tree_with_nested_children("marks-root");
        let mut page = DirPage::new(tree.0.clone());
        // Marking steps the cursor on, so come back to the directory to enter.
        page.on_key(&press(KeyCode::Char('m')));
        assert_eq!(page.marks.len(), 1);
        page.on_key(&press(KeyCode::Char('k')));

        page.on_key(&press(KeyCode::Enter));
        assert_eq!(page.root, nested);
        assert!(page.marks.is_empty());
    }

    #[test]
    fn test_toggling_sizes_asks_for_one_directory_at_a_time() {
        // Asking for every directory at once starts as many walks as there are
        // rows; each answer is what asks for the next.
        let tree = TempTree::new("sizes");
        let first = tree.dir("aaa");
        let second = tree.dir("bbb");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&alt(KeyCode::Char('S')));
        assert_eq!(
            outcome,
            PageOutcome::Job(JobRequest::DirSize(first.clone()))
        );

        let next = page.on_job(JobReply::DirSize {
            bytes: 10,
            path: first,
        });
        assert_eq!(next, PageOutcome::Job(JobRequest::DirSize(second.clone())));

        let done = page.on_job(JobReply::DirSize {
            bytes: 20,
            path: second,
        });
        assert_eq!(done, PageOutcome::Consumed, "nothing left to walk");
    }

    #[test]
    fn test_turning_sizes_off_cancels_the_walks_still_running() {
        let tree = TempTree::new("sizes-off");
        tree.dir("aaa");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&alt(KeyCode::Char('S')));
        assert_eq!(
            page.on_key(&alt(KeyCode::Char('S'))),
            PageOutcome::CancelJobs
        );
    }

    #[test]
    fn test_an_answer_arriving_after_sizes_were_turned_off_asks_for_nothing() {
        // The walk was cancelled, but one already in flight still reports back;
        // it must not restart the chain.
        let tree = TempTree::new("sizes-late");
        let dir = tree.dir("aaa");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&alt(KeyCode::Char('S')));
        page.on_key(&alt(KeyCode::Char('S')));

        let outcome = page.on_job(JobReply::DirSize {
            bytes: 1,
            path: dir,
        });
        assert_eq!(outcome, PageOutcome::Consumed);
    }

    #[test]
    fn test_a_walked_total_survives_a_reload_but_not_a_new_root() {
        // Re-walking on every keystroke is the expensive mistake; carrying a
        // total into a different directory is the wrong one.
        let (tree, nested) = tree_with_nested_children("sizes-cache");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&alt(KeyCode::Char('S')));
        page.on_job(JobReply::DirSize {
            bytes: 64,
            path: nested.clone(),
        });
        assert_eq!(page.sizes.get(&nested).copied(), Some(64));

        page.on_key(&press(KeyCode::Char('G')));
        assert_eq!(page.sizes.get(&nested).copied(), Some(64));

        page.on_key(&press(KeyCode::Backspace));
        assert!(page.sizes.is_empty(), "a new root re-walks");
    }

    #[test]
    fn test_a_listing_longer_than_the_pane_scrolls_with_the_cursor() {
        // Without a window the rows past the pane's last line are painted and
        // clipped, so the cursor walks off into content nobody can see.
        let tree = TempTree::new("scroll");
        for i in 0..20 {
            tree.touch(&format!("file{i:02}.txt"));
        }
        let mut page = DirPage::new(tree.0.clone());
        let pane_rows = 6;

        let visible = page.content(pane_rows, 80, false).rows.len();
        assert_eq!(visible, pane_rows, "the pane is filled, not overrun");

        for _ in 0..19 {
            page.on_key(&press(KeyCode::Char('j')));
        }
        let content = page.content(pane_rows, 80, false);
        assert_eq!(
            content.cursor_line,
            Some(pane_rows - 1),
            "the cursor rides the last visible row"
        );
        assert_eq!(content.rows.len(), pane_rows);

        page.on_key(&press(KeyCode::Home));
        assert_eq!(
            page.content(pane_rows, 80, false).cursor_line,
            Some(HEADER_ROWS),
            "and returns to the top when the cursor does"
        );
    }

    #[test]
    fn test_an_empty_directory_has_no_cursor_line() {
        // With no rows there is nothing to band, and a cursor line of zero
        // would highlight the header.
        let tree = TempTree::new("empty");
        let mut page = DirPage::new(tree.0.clone());
        assert_eq!(page.content(20, 80, false).cursor_line, None);
    }

    #[test]
    fn test_a_search_moves_the_cursor_to_the_matching_entry() {
        let tree = TempTree::new("search");
        tree.touch("alpha.txt");
        tree.touch("zeta.txt");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('/')));
        answer(&mut page, outcome, "zet");
        assert_eq!(selected_name(&page), "zeta.txt");
    }

    #[test]
    fn test_a_search_leaves_the_marks_and_the_listing_alone() {
        // Answering a search down the same path as an operation would clear the
        // marks, losing what the next operation was aimed at.
        let tree = TempTree::new("search-marks");
        tree.touch("alpha.txt");
        tree.touch("zeta.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('m')));
        let outcome = page.on_key(&press(KeyCode::Char('/')));
        answer(&mut page, outcome, "zeta");
        assert_eq!(page.marks.len(), 1);
    }

    #[test]
    fn test_the_repeat_keys_step_through_the_matches_in_both_directions() {
        let tree = TempTree::new("search-repeat");
        tree.touch("alpha.txt");
        tree.touch("also.txt");
        tree.touch("zeta.txt");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('/')));
        answer(&mut page, outcome, "al");
        assert_eq!(selected_name(&page), "also.txt");

        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(
            selected_name(&page),
            "alpha.txt",
            "and wraps rather than stopping at the last match"
        );

        page.on_key(&press(KeyCode::Char('N')));
        assert_eq!(selected_name(&page), "also.txt");
    }

    #[test]
    fn test_a_search_with_nothing_to_find_reports_it() {
        let tree = TempTree::new("search-miss");
        tree.touch("alpha.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(page.message.as_deref(), Some("no search"));

        let outcome = page.on_key(&press(KeyCode::Char('/')));
        answer(&mut page, outcome, "absent");
        assert_eq!(page.message.as_deref(), Some("not found: absent"));
        assert_eq!(selected_name(&page), "alpha.txt");
    }

    #[test]
    fn test_editing_a_name_and_pressing_enter_renames_the_file() {
        let tree = TempTree::new("edit-apply");
        tree.touch("notes.txt");
        let mut page = DirPage::new(tree.0.clone());

        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));
        for ch in "renamed.md".chars() {
            page.on_key(&press(KeyCode::Char(ch)));
        }
        assert!(page.edit.is_some(), "typing alone renames nothing");

        page.on_key(&press(KeyCode::Enter));
        assert!(page.edit.is_none());
        assert!(tree.path("renamed.md").exists());
        assert!(!tree.path("notes.txt").exists());
        assert_eq!(selected_name(&page), "renamed.md");
    }

    #[test]
    fn test_enter_with_nothing_changed_leaves_edit_mode_quietly() {
        let tree = TempTree::new("edit-none");
        tree.touch("same.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);

        page.on_key(&press(KeyCode::Char('Z')));
        page.on_key(&press(KeyCode::Char('Z')));
        assert!(page.edit.is_none());
        assert_eq!(page.message.as_deref(), Some("no changes"));
        assert!(tree.path("same.txt").exists());
    }

    #[test]
    fn test_escape_throws_the_edits_away_without_touching_the_disk() {
        let tree = TempTree::new("edit-discard");
        tree.touch("keep.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));
        page.on_key(&press(KeyCode::Char('x')));

        // Escape only leaves Insert for Normal; q is what quits the edit.
        page.on_key(&press(KeyCode::Escape));
        assert!(page.edit.is_some(), "Escape returns to Normal mode");
        page.on_key(&press(KeyCode::Char('q')));
        assert!(page.edit.is_none());
        assert_eq!(page.message.as_deref(), Some("discarded 1"));
        assert!(tree.path("keep.txt").exists());
        assert!(!tree.path("x").exists());
        assert_eq!(selected_name(&page), "keep.txt");
    }

    #[test]
    fn test_q_also_leaves_edit_mode_without_applying() {
        // `q` types into a name like any other letter: quitting the edit is
        // Escape's job, because a filename may hold a `q`.
        let tree = TempTree::new("edit-q");
        tree.touch("stay.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));

        for ch in "quick.json".chars() {
            page.on_key(&press(KeyCode::Char(ch)));
        }
        page.on_key(&press(KeyCode::Enter));
        assert!(tree.path("quick.json").exists(), "every letter typed");
    }

    #[test]
    fn test_a_name_emptied_by_editing_is_refused_at_apply() {
        // An empty name is not a name, and the refusal stays in edit mode so
        // the mistake can be fixed rather than started over.
        let tree = TempTree::new("edit-empty");
        tree.touch("gone.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));

        page.on_key(&press(KeyCode::Enter));
        assert!(page.edit.is_some(), "still editing, so it can be fixed");
        let message = page.message.clone().unwrap_or_default();
        assert!(message.starts_with("failed:"), "got {message:?}");
        assert!(tree.path("gone.txt").exists());

        for ch in "back.txt".chars() {
            page.on_key(&press(KeyCode::Char(ch)));
        }
        page.on_key(&press(KeyCode::Enter));
        assert!(page.edit.is_none());
        assert!(tree.path("back.txt").exists());
        assert!(!tree.path("gone.txt").exists());
    }

    #[test]
    fn test_edits_apply_to_children_before_the_directory_that_holds_them() {
        // Renaming the directory first would strand the file's pending path:
        // both renames come from the snapshot, so the deeper one must run
        // first, exactly as wdir sorts by path depth before applying.
        let tree = TempTree::new("edit-order");
        let nested = tree.dir("nested");
        fs::write(nested.join("leaf.txt"), "x").expect("leaf");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));

        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));
        for ch in "outer".chars() {
            page.on_key(&press(KeyCode::Char(ch)));
        }
        page.on_key(&press(KeyCode::Escape));
        page.on_key(&press(KeyCode::Char('j')));
        page.on_key(&press(KeyCode::Char('S')));
        for ch in "sprout.txt".chars() {
            page.on_key(&press(KeyCode::Char(ch)));
        }
        page.on_key(&press(KeyCode::Enter));

        assert!(tree.path("outer").join("sprout.txt").exists());
        assert!(!tree.path("nested").exists());
    }

    #[test]
    fn test_editing_moves_the_caret_between_names() {
        let tree = TempTree::new("edit-caret");
        tree.touch("a.txt");
        tree.touch("bb.txt");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::End));

        toggle_edit(&mut page);
        assert_eq!(
            page.edit.as_ref().expect("editing").col(),
            6,
            "where `A` would start: the append position"
        );

        page.on_key(&press(KeyCode::Char('k')));
        assert_eq!(selected_name(&page), "a.txt");
        assert_eq!(
            page.edit.as_ref().expect("editing").col(),
            5,
            "the column Vim keeps across a shorter line"
        );

        page.on_key(&press(KeyCode::Char('0')));
        assert_eq!(page.edit.as_ref().expect("editing").col(), 0);
        page.on_key(&press(KeyCode::Char('l')));
        assert_eq!(page.edit.as_ref().expect("editing").col(), 1);
        page.on_key(&press(KeyCode::Char('$')));
        assert_eq!(
            page.edit.as_ref().expect("editing").col(),
            4,
            "on the last character"
        );
    }

    #[test]
    fn test_the_edited_row_paints_its_caret() {
        let tree = TempTree::new("edit-paint");
        tree.touch("name.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('0')));

        let content = page.content(10, 80, false);
        let cursor = content.cursor_line.expect("a cursor row");
        let line = crate::model::page::row_text(&content.rows[cursor]);
        assert!(line.contains('│'), "got {line:?}");
    }

    #[test]
    fn test_applying_edits_clears_marks_pointing_at_old_paths() {
        // A mark aimed at a path the rename has just left behind would count
        // toward an operation aimed at nothing.
        let tree = TempTree::new("edit-marks");
        tree.touch("before.txt");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Char('m')));

        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));
        for ch in "after.txt".chars() {
            page.on_key(&press(KeyCode::Char(ch)));
        }
        page.on_key(&press(KeyCode::Enter));
        assert!(page.marks.is_empty());
    }

    #[test]
    fn test_the_toggle_chord_leaves_the_edit_and_discards() {
        // The same chord that opens the session closes it, from either mode,
        // keeping the disk as it is.
        let tree = TempTree::new("edit-toggle-off");
        tree.touch("keep.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('S')));
        page.on_key(&press(KeyCode::Char('x')));
        assert!(page.edit.is_some(), "fixture: an unsaved edit");

        toggle_edit(&mut page);
        assert!(page.edit.is_none());
        assert_eq!(page.message.as_deref(), Some("discarded 1"));
        assert!(tree.path("keep.txt").exists());
        assert!(!tree.path("x").exists());
    }

    #[test]
    fn test_a_stray_follow_key_abandons_the_chord_instead_of_leaking() {
        // `Ctrl-X` followed by anything but `Ctrl-Q` must not leave the
        // leader pending, swallowing the next keystroke.
        let tree = TempTree::new("edit-stray-chord");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&ctrl(KeyCode::Char('x')));
        page.on_key(&press(KeyCode::Char('!')));
        assert!(page.pending.is_none());
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(selected_name(&page), "b.txt");
    }

    #[test]
    fn test_escape_leaves_insert_for_normal_and_never_discards() {
        // A vim pair: Escape steps out of Insert; a second Escape is inert in
        // Normal, so mashing it cannot throw the edits away.
        let tree = TempTree::new("edit-escape-pair");
        tree.touch("base.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        page.on_key(&press(KeyCode::Char('A')));
        page.on_key(&press(KeyCode::Char('2')));
        assert_eq!(
            page.edit.as_ref().expect("editing").mode(),
            edit::EditMode::Insert,
            "fixture: typing keeps Insert"
        );

        page.on_key(&press(KeyCode::Escape));
        page.on_key(&press(KeyCode::Escape));
        assert_eq!(
            page.edit.as_ref().expect("editing").mode(),
            edit::EditMode::Normal,
            "the first Escape steps out"
        );
        page.on_key(&press(KeyCode::Escape));
        assert!(page.edit.is_some(), "the second is inert");
        assert_eq!(
            page.edit.as_ref().and_then(|e| e.name(0)),
            Some("base.txt2")
        );
    }

    #[test]
    fn test_the_header_names_the_mode_the_editor_is_in() {
        let tree = TempTree::new("edit-header-mode");
        tree.touch("name.txt");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        let header = crate::model::page::row_text(&page.content(10, 80, false).rows[0]);
        assert!(header.contains("editing:normal"), "got {header:?}");

        page.on_key(&press(KeyCode::Char('A')));
        let header = crate::model::page::row_text(&page.content(10, 80, false).rows[0]);
        assert!(header.contains("editing:insert"), "got {header:?}");
    }

    /// A listing whose tree gives the word motions something to step over:
    /// an expanded directory with two children, another directory, two files.
    fn wordy_tree(tag: &str) -> (TempTree, PathBuf) {
        let tree = TempTree::new(tag);
        let nested = tree.dir("nested");
        fs::write(nested.join("child-a.txt"), "x").expect("child a");
        fs::write(nested.join("child-b.txt"), "x").expect("child b");
        tree.dir("other");
        tree.touch("file1.txt");
        tree.touch("file2.txt");
        (tree, nested)
    }

    #[test]
    fn test_w_steps_over_the_subtree_under_the_cursor() {
        // The expanded subtree is the word the cursor sits on: `w` lands past
        // it, on the next entry at its own depth, not on its first child.
        let (tree, _) = wordy_tree("word-w");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));
        assert_eq!(page.rows.len(), 6, "fixture: nested open with two children");

        page.on_key(&press(KeyCode::Char('w')));
        assert_eq!(
            selected_name(&page),
            "other",
            "the children were stepped over"
        );

        page.on_key(&press(KeyCode::Char('w')));
        assert_eq!(
            selected_name(&page),
            "file1.txt",
            "a leaf steps to the next entry"
        );
    }

    #[test]
    fn test_b_steps_back_over_the_subtree_above_the_cursor() {
        let (tree, _) = wordy_tree("word-b");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));
        page.on_key(&press(KeyCode::End));

        page.on_key(&press(KeyCode::Char('b')));
        assert_eq!(selected_name(&page), "file1.txt");
        page.on_key(&press(KeyCode::Char('b')));
        assert_eq!(selected_name(&page), "other");
        page.on_key(&press(KeyCode::Char('b')));
        assert_eq!(
            selected_name(&page),
            "nested",
            "the whole subtree above was stepped over, not descended into"
        );
    }

    #[test]
    fn test_e_lands_at_the_end_of_the_word() {
        let (tree, _) = wordy_tree("word-e");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));

        // On the expanded directory: its subtree's last row is the word's end.
        page.on_key(&press(KeyCode::Char('e')));
        assert_eq!(selected_name(&page), "child-b.txt");

        // Already at a leaf's end: the next word's end, which for a collapsed
        // entry is the entry itself.
        page.on_key(&press(KeyCode::Char('e')));
        assert_eq!(selected_name(&page), "other");
    }

    #[test]
    fn test_the_braces_jump_between_directories() {
        // Directories sort first and so head every group: they are the
        // paragraphs of a listing.
        let (tree, _) = wordy_tree("word-braces");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));

        page.on_key(&press(KeyCode::Char('}')));
        assert_eq!(selected_name(&page), "other");
        page.on_key(&press(KeyCode::Char('}')));
        assert_eq!(
            selected_name(&page),
            "other",
            "no directory after the last one"
        );

        page.on_key(&press(KeyCode::Char('{')));
        assert_eq!(selected_name(&page), "nested");
        page.on_key(&press(KeyCode::Char('{')));
        assert_eq!(
            selected_name(&page),
            "nested",
            "no directory before the first"
        );
    }

    #[test]
    fn test_dollar_and_gg_reach_the_ends() {
        let (tree, _) = wordy_tree("word-ends");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));

        page.on_key(&press(KeyCode::Char('$')));
        assert_eq!(selected_name(&page), "file2.txt");
        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('g')));
        assert_eq!(selected_name(&page), "nested");
    }

    #[test]
    fn test_the_half_page_motions_move_half_the_pane() {
        let (tree, _) = wordy_tree("word-halfpage");
        let mut page = DirPage::new(tree.0.clone());
        for i in 0..20 {
            tree.touch(&format!("pad{i:02}.txt"));
        }
        page.reload();
        page.on_key(&press(KeyCode::Tab));
        page.on_key(&press(KeyCode::Home));

        // The pane holds 10 rows, one of them the header: half of nine is four.
        page.content(10, 80, false);
        page.on_key(&ctrl(KeyCode::Char('d')));
        assert_eq!(page.cursor, 4, "half a viewport down");
        page.on_key(&ctrl(KeyCode::Char('u')));
        assert_eq!(page.cursor, 0, "and back up, clamped at the top");
    }

    #[test]
    fn test_editing_an_empty_listing_is_a_no_op() {
        let tree = TempTree::new("edit-empty-listing");
        let mut page = DirPage::new(tree.0.clone());
        toggle_edit(&mut page);
        assert!(page.edit.is_none());
    }
}

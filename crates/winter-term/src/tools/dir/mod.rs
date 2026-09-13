//! Dir: a keyboard-driven directory listing in a pane.
//!
//! - [`entry`]: what a listing is made of.
//! - [`icons`]: the glyph beside an entry's name.
//! - [`listing`]: ordering and filtering.
//! - [`marks`]: which entries an operation acts on.
//! - [`ops`]: creating, moving, copying, and deleting.
//! - [`rows`]: painting a listing.
//! - [`source`]: reading the filesystem.
//! - [`tree`]: expanded directories and row depth.

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

use crate::model::input::{Key, KeyCode};
use crate::model::page::{
    scroll_to_cursor, JobReply, JobRequest, Page, PageContent, PageOutcome, PageSpan, PageStyle,
    PromptMode, PromptReply, PromptRequest,
};

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

/// Directories remembered in each direction of the visit history. Matches the
/// jumplist's own depth, for the same reason: enough to walk back through a
/// session's wandering, bounded so it cannot grow without limit.
const MAX_HISTORY: usize = 100;

// ========================================================================
// Data Structures
// ========================================================================

/// A directory listing the keyboard drives: move, fold, descend, and open.
#[derive(Clone, Debug)]
pub struct DirPage {
    /// Directories left behind, most recent last.
    back: Vec<PathBuf>,
    cursor: usize,
    folds: Folds,
    /// Directories stepped back out of, most recent last.
    forward: Vec<PathBuf>,
    /// What the last operation reported, shown in the header until the next key.
    message: Option<String>,
    marks: Marks,
    /// The first key of a two-key sequence, waiting for its second.
    pending: Option<char>,
    root: PathBuf,
    rows: Vec<Row>,
    /// First listed row visible in the pane.
    scroll: usize,
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
// DirPage
// ========================================================================

impl DirPage {
    /// A listing of `root`, collapsed, sorted by name, hiding dotfiles.
    pub fn new(root: PathBuf) -> Self {
        let mut page = Self {
            back: Vec::new(),
            cursor: 0,
            folds: Folds::new(),
            forward: Vec::new(),
            marks: Marks::new(),
            message: None,
            pending: None,
            root,
            rows: Vec::new(),
            scroll: 0,
            show_details: false,
            show_hidden: false,
            show_sizes: false,
            sizes: HashMap::new(),
            sort: SortKey::default(),
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
            return PageOutcome::OpenPath(row.entry.path.clone());
        }
        let target = row.entry.path.clone();
        self.set_root(target);
        PageOutcome::Consumed
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
            self.folds.toggle(&path);
        }
        self.reload_keeping_selection();
    }

    fn toggle_fold(&mut self) {
        let Some(row) = self.selected() else {
            return;
        };
        if !row.entry.is_dir() {
            return;
        }
        let path = row.entry.path.clone();
        self.folds.toggle(&path);
        self.reload_keeping_selection();
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
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
        let label = if targets.len() == 1 {
            format!("{verb} {} to: ", name_of(first))
        } else {
            format!("{verb} {} entries into: ", targets.len())
        };
        self.ask(tag, label, String::new())
    }

    /// Carry out the answer to a question, and report what happened.
    fn apply_answer(&mut self, tag: &'static str, answer: &str) {
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
        self.message = Some(match result {
            Ok(report) => report,
            Err(report) => format!("failed: {report}"),
        });
        self.marks.clear();
        self.reload_keeping_selection();
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
    fn resolve_pending(&mut self, leader: char, code: KeyCode) -> PageOutcome {
        self.pending = None;
        match (leader, code) {
            ('g', KeyCode::Char('g')) => self.cursor = 0,
            ('z', KeyCode::Char('o')) => self.expand_selected(true),
            ('z', KeyCode::Char('c')) => self.expand_selected(false),
            ('z', KeyCode::Char('a')) => self.toggle_fold(),
            ('z', KeyCode::Char('R')) => self.expand_one_level(),
            ('z', KeyCode::Char('M')) => {
                self.folds.collapse_all();
                self.reload_keeping_selection();
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

impl Page for DirPage {
    fn on_job(&mut self, reply: JobReply) -> PageOutcome {
        match reply {
            JobReply::DirSize { bytes, path } => {
                self.sizes.insert(path, bytes);
            }
        }
        self.request_next_size()
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        match reply.answer {
            Some(answer) => self.apply_answer(reply.tag, &answer),
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

    fn content(&mut self, rows: usize) -> PageContent {
        let now = SystemTime::now();
        let mut page_rows = vec![rows::header_row(
            &self.root.to_string_lossy(),
            self.sort,
            self.show_hidden,
            self.show_details,
            self.marks.len(),
            self.message.as_deref(),
            self.show_sizes,
        )];
        if self.rows.is_empty() {
            page_rows.push(vec![PageSpan::new(PageStyle::Dim, EMPTY_NOTE)]);
            return PageContent::new(page_rows);
        }
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.scroll = scroll_to_cursor(self.scroll, self.cursor, self.rows.len(), visible);
        page_rows.extend(self.rows.iter().skip(self.scroll).take(visible).map(|row| {
            rows::entry_row(
                row,
                rows::RowStyle {
                    marked: self.marks.contains(&row.entry.path),
                    show_details: self.show_details,
                    show_sizes: self.show_sizes,
                    size: self.dir_size(row),
                },
                now,
            )
        }));
        PageContent::new(page_rows).with_cursor_line(HEADER_ROWS + self.cursor - self.scroll)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        if let Some(leader) = self.pending {
            return self.resolve_pending(leader, key.code);
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_by(1);
                PageOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_by(-1);
                PageOutcome::Consumed
            }
            KeyCode::Char('g') | KeyCode::Char('z') => {
                self.pending = match key.code {
                    KeyCode::Char(c) => Some(c),
                    _ => None,
                };
                PageOutcome::Consumed
            }
            KeyCode::Char('G') => {
                self.cursor = self.rows.len().saturating_sub(1);
                PageOutcome::Consumed
            }
            KeyCode::Char('l') | KeyCode::Enter | KeyCode::Right => self.enter(),
            KeyCode::Char('h') | KeyCode::Left => {
                self.fold_or_step_out();
                PageOutcome::Consumed
            }
            KeyCode::Char('-') => {
                self.ascend();
                PageOutcome::Consumed
            }
            KeyCode::Char('^') => {
                if let Some(parent) = self.parent_row(self.cursor) {
                    self.cursor = parent;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char(']') => {
                if let Some(next) = self.sibling_after(self.cursor) {
                    self.cursor = next;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('[') => {
                if let Some(previous) = self.sibling_before(self.cursor) {
                    self.cursor = previous;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('}') => {
                if let Some(child) = self.first_child(self.cursor) {
                    self.cursor = child;
                }
                PageOutcome::Consumed
            }
            KeyCode::Char('o') if key.ctrl => {
                self.go_back();
                PageOutcome::Consumed
            }
            KeyCode::Char('i') if key.ctrl => {
                self.go_forward();
                PageOutcome::Consumed
            }
            KeyCode::Char('o') => self
                .selected()
                .map(|row| PageOutcome::OpenExternal(row.entry.path.clone()))
                .unwrap_or(PageOutcome::Consumed),
            KeyCode::Tab => {
                self.toggle_fold();
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
            KeyCode::Char('r') => {
                self.reload_keeping_selection();
                PageOutcome::Consumed
            }
            KeyCode::Char('S') => self.toggle_sizes(),
            KeyCode::Char('m') => {
                self.toggle_mark();
                PageOutcome::Consumed
            }
            KeyCode::Char('*') => {
                self.mark_all();
                PageOutcome::Consumed
            }
            KeyCode::Char('U') => {
                self.marks.clear();
                PageOutcome::Consumed
            }
            KeyCode::Char('a') => self.ask(ASK_NEW_FILE, "New file: ".to_string(), String::new()),
            KeyCode::Char('A') => self.ask(ASK_MKDIR, "New directory: ".to_string(), String::new()),
            KeyCode::Char('R') => self.ask_rename(),
            KeyCode::Char('C') => self.ask_destination(ASK_COPY, "Copy"),
            KeyCode::Char('M') => self.ask_destination(ASK_MOVE, "Move"),
            KeyCode::Char('D') => self.ask_delete(),
            KeyCode::Char('x') => self.ask(ASK_MODE, "Mode: ".to_string(), String::new()),
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            // Every other key belongs to the host.
            _ => PageOutcome::Ignored,
        }
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

    fn ctrl(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: true,
            shift: false,
        }
    }

    fn selected_name(page: &DirPage) -> String {
        page.selected()
            .map(|row| row.entry.name.clone())
            .unwrap_or_default()
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
            PageOutcome::OpenPath(file)
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
    fn test_gg_and_shift_g_jump_to_the_ends() {
        let tree = TempTree::new("jumps");
        tree.touch("a.txt");
        tree.touch("b.txt");
        tree.touch("c.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('G')));
        assert_eq!(selected_name(&page), "c.txt");
        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('g')));
        assert_eq!(selected_name(&page), "a.txt");
    }

    #[test]
    fn test_a_stray_g_sequence_is_abandoned_not_left_pending() {
        // A pending `g` that survived an unrelated key would swallow the next
        // keystroke as well.
        let tree = TempTree::new("pending");
        tree.touch("a.txt");
        tree.touch("b.txt");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('x')));
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

        page.on_key(&press(KeyCode::Char(']')));
        assert_eq!(selected_name(&page), "child-b.txt");
        page.on_key(&press(KeyCode::Char(']')));
        assert_eq!(selected_name(&page), "child-b.txt", "no escape upward");
        page.on_key(&press(KeyCode::Char('[')));
        assert_eq!(selected_name(&page), "child-a.txt");
    }

    #[test]
    fn test_parent_and_first_child_walk_the_tree_vertically() {
        let (tree, _) = tree_with_nested_children("vertical");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));

        page.on_key(&press(KeyCode::Char('}')));
        assert_eq!(selected_name(&page), "child-a.txt");
        page.on_key(&press(KeyCode::Char('^')));
        assert_eq!(selected_name(&page), "nested");
        page.on_key(&press(KeyCode::Char('^')));
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

        page.on_key(&ctrl(KeyCode::Char('o')));
        assert_eq!(page.root, tree.0, "back returns to where it came from");
        page.on_key(&ctrl(KeyCode::Char('i')));
        assert_eq!(page.root, nested, "forward undoes the step back");
    }

    #[test]
    fn test_history_at_either_end_is_a_no_op() {
        let (tree, _) = tree_with_nested_children("history-ends");
        let mut page = DirPage::new(tree.0.clone());

        page.on_key(&ctrl(KeyCode::Char('o')));
        assert_eq!(page.root, tree.0);
        page.on_key(&ctrl(KeyCode::Char('i')));
        assert_eq!(page.root, tree.0);
    }

    #[test]
    fn test_moving_somewhere_new_drops_the_forward_history() {
        // Otherwise `forward` points at a branch the user has left, and
        // Ctrl-i teleports somewhere unrelated.
        let (tree, nested) = tree_with_nested_children("history-branch");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Enter));
        page.on_key(&ctrl(KeyCode::Char('o')));
        assert!(!page.forward.is_empty());

        page.on_key(&press(KeyCode::Char('-')));
        assert!(page.forward.is_empty());
        page.on_key(&ctrl(KeyCode::Char('i')));
        assert_ne!(page.root, nested);
    }

    #[test]
    fn test_zr_expands_one_level_at_a_time() {
        // Expanding the whole tree at once can read an unbounded number of
        // directories; each press must reach exactly one level further.
        let tree = TempTree::new("zr");
        let nested = tree.dir("nested");
        fs::create_dir_all(nested.join("deeper")).expect("deeper dir");
        fs::write(nested.join("deeper").join("leaf.txt"), "x").expect("leaf");
        let mut page = DirPage::new(tree.0.clone());

        assert_eq!(page.rows.len(), 1);
        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('R')));
        assert_eq!(page.rows.len(), 2, "nested is open, deeper is not");
        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('R')));
        assert_eq!(page.rows.len(), 3, "one level further");
    }

    #[test]
    fn test_zo_and_zc_are_not_toggles() {
        // `zo` on an open directory must leave it open, not close it.
        let (tree, _) = tree_with_nested_children("zoc");
        let mut page = DirPage::new(tree.0.clone());

        for _ in 0..2 {
            page.on_key(&press(KeyCode::Char('z')));
            page.on_key(&press(KeyCode::Char('o')));
        }
        assert_eq!(page.rows.len(), 4, "still expanded");

        for _ in 0..2 {
            page.on_key(&press(KeyCode::Char('z')));
            page.on_key(&press(KeyCode::Char('c')));
        }
        assert_eq!(page.rows.len(), 2, "still collapsed");
    }

    #[test]
    fn test_zm_collapses_everything() {
        let (tree, _) = tree_with_nested_children("zm");
        let mut page = DirPage::new(tree.0.clone());
        page.on_key(&press(KeyCode::Tab));
        assert_eq!(page.rows.len(), 4);

        page.on_key(&press(KeyCode::Char('z')));
        page.on_key(&press(KeyCode::Char('M')));
        assert_eq!(page.rows.len(), 2);
    }

    #[test]
    fn test_o_hands_the_entry_to_the_system_handler() {
        let tree = TempTree::new("external");
        let file = tree.touch("photo.png");
        let mut page = DirPage::new(tree.0.clone());
        assert_eq!(
            page.on_key(&press(KeyCode::Char('o'))),
            PageOutcome::OpenExternal(file)
        );
    }

    fn answer(page: &mut DirPage, outcome: PageOutcome, text: &str) {
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        page.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        });
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
        let outcome = page.on_key(&press(KeyCode::Char('D')));
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

        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('D'))) else {
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

        let PageOutcome::Prompt(request) = page.on_key(&press(KeyCode::Char('D'))) else {
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

        let outcome = page.on_key(&press(KeyCode::Char('a')));
        answer(&mut page, outcome, "notes.txt");
        assert!(tree.path("notes.txt").exists());
        assert_eq!(selected_name(&page), "notes.txt", "the cursor follows it");

        let outcome = page.on_key(&press(KeyCode::Char('R')));
        answer(&mut page, outcome, "renamed.md");
        assert!(tree.path("renamed.md").exists());
        assert!(!tree.path("notes.txt").exists());

        let outcome = page.on_key(&press(KeyCode::Char('D')));
        answer(&mut page, outcome, "y");
        assert!(!tree.path("renamed.md").exists());
        assert!(page.rows.is_empty());
    }

    #[test]
    fn test_a_new_directory_joins_the_listing() {
        let tree = TempTree::new("mkdir");
        let mut page = DirPage::new(tree.0.clone());

        let outcome = page.on_key(&press(KeyCode::Char('A')));
        answer(&mut page, outcome, "sub");
        assert!(tree.path("sub").is_dir());
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

        let outcome = page.on_key(&press(KeyCode::Char('M')));
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

        let outcome = page.on_key(&press(KeyCode::Char('M')));
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

        let outcome = page.on_key(&press(KeyCode::Char('a')));
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

        let outcome = page.on_key(&press(KeyCode::Char('S')));
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

        page.on_key(&press(KeyCode::Char('S')));
        assert_eq!(
            page.on_key(&press(KeyCode::Char('S'))),
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
        page.on_key(&press(KeyCode::Char('S')));
        page.on_key(&press(KeyCode::Char('S')));

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
        page.on_key(&press(KeyCode::Char('S')));
        page.on_job(JobReply::DirSize {
            bytes: 64,
            path: nested.clone(),
        });
        assert_eq!(page.sizes.get(&nested).copied(), Some(64));

        page.on_key(&press(KeyCode::Char('r')));
        assert_eq!(page.sizes.get(&nested).copied(), Some(64));

        page.on_key(&press(KeyCode::Char('-')));
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

        let visible = page.content(pane_rows).rows.len();
        assert_eq!(visible, pane_rows, "the pane is filled, not overrun");

        for _ in 0..19 {
            page.on_key(&press(KeyCode::Char('j')));
        }
        let content = page.content(pane_rows);
        assert_eq!(
            content.cursor_line,
            Some(pane_rows - 1),
            "the cursor rides the last visible row"
        );
        assert_eq!(content.rows.len(), pane_rows);

        page.on_key(&press(KeyCode::Char('g')));
        page.on_key(&press(KeyCode::Char('g')));
        assert_eq!(
            page.content(pane_rows).cursor_line,
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
        assert_eq!(page.content(20).cursor_line, None);
    }
}

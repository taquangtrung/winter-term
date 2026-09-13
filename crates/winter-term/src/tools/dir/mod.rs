//! Dir: a keyboard-driven directory listing in a pane.
//!
//! - [`entry`]: what a listing is made of.
//! - [`icons`]: the glyph beside an entry's name.
//! - [`listing`]: ordering and filtering.
//! - [`rows`]: painting a listing.
//! - [`source`]: reading the filesystem.
//! - [`tree`]: expanded directories and row depth.

pub mod entry;
pub mod icons;
pub mod listing;
pub mod rows;
pub mod source;
pub mod tree;

use std::path::PathBuf;
use std::time::SystemTime;

use crate::model::input::{Key, KeyCode};
use crate::model::page::{scroll_to_cursor, Page, PageContent, PageOutcome, PageSpan, PageStyle};

use listing::SortKey;
use tree::{Folds, Row};

// ========================================================================
// Constants
// ========================================================================

/// Rows of header above the first entry.
const HEADER_ROWS: usize = 1;

/// Shown in place of the listing when a directory has nothing to show.
const EMPTY_NOTE: &str = "  (empty)";

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
    /// The first key of a two-key sequence, waiting for its second.
    pending: Option<char>,
    root: PathBuf,
    rows: Vec<Row>,
    /// First listed row visible in the pane.
    scroll: usize,
    show_details: bool,
    show_hidden: bool,
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
            pending: None,
            root,
            rows: Vec::new(),
            scroll: 0,
            show_details: false,
            show_hidden: false,
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
        )];
        if self.rows.is_empty() {
            page_rows.push(vec![PageSpan::new(PageStyle::Dim, EMPTY_NOTE)]);
            return PageContent::new(page_rows);
        }
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.scroll = scroll_to_cursor(self.scroll, self.cursor, self.rows.len(), visible);
        page_rows.extend(
            self.rows
                .iter()
                .skip(self.scroll)
                .take(visible)
                .map(|row| rows::entry_row(row, self.show_details, now)),
        );
        PageContent::new(page_rows).with_cursor_line(HEADER_ROWS + self.cursor - self.scroll)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
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
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            // Every other key belongs to the host.
            _ => PageOutcome::Ignored,
        }
    }
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

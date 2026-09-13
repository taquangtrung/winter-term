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

// ========================================================================
// Data Structures
// ========================================================================

/// A directory listing the keyboard drives: move, fold, descend, and open.
#[derive(Clone, Debug)]
pub struct DirPage {
    cursor: usize,
    folds: Folds,
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
            cursor: 0,
            folds: Folds::new(),
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
        self.folds.retain_under(&root);
        self.root = root;
        self.cursor = 0;
        self.reload();
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

    /// Resolve the second key of a `g` sequence.
    fn resolve_pending(&mut self, code: KeyCode) -> PageOutcome {
        self.pending = None;
        if code == KeyCode::Char('g') {
            self.cursor = 0;
        }
        PageOutcome::Consumed
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
        if self.pending.is_some() {
            return self.resolve_pending(key.code);
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
            KeyCode::Char('g') => {
                self.pending = Some('g');
                PageOutcome::Consumed
            }
            KeyCode::Char('G') => {
                self.cursor = self.rows.len().saturating_sub(1);
                PageOutcome::Consumed
            }
            KeyCode::Char('l') | KeyCode::Enter | KeyCode::Right => self.enter(),
            KeyCode::Char('h') | KeyCode::Char('-') | KeyCode::Left => {
                self.ascend();
                PageOutcome::Consumed
            }
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
            KeyCode::Char('z') => {
                self.folds.collapse_all();
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

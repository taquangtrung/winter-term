//! Tool pages in panes: opening one in a split, closing it, and offering it
//! keys before the modal keymap sees them.

use std::path::PathBuf;

use crate::model::input::Key;
use crate::model::layout::{Direction, PaneId};
use crate::model::mode::Mode;
use crate::model::page::{Page, PageOutcome};
use crate::tools::dir::DirPage;
use crate::tools::keys::KeysPage;

use super::App;
use super::SPLIT_RATIO;

// ========================================================================
// App: tool pages
// ========================================================================

impl App {
    /// Open the keys page beside the focused pane.
    pub(crate) fn open_keys_page(&mut self) {
        let page = KeysPage::new(&self.window_keymap);
        self.open_page_in_split(Box::new(page));
    }

    /// Open a directory listing beside the focused pane, rooted at the focused
    /// pane's working directory.
    pub(crate) fn open_dir_page(&mut self) {
        let root = self
            .focused_cwd()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        self.open_page_in_split(Box::new(DirPage::new(root)));
    }

    /// Open `page` in a fresh split beside the focused pane, and focus it. The
    /// new pane spawns no process: the page is the whole content.
    pub(crate) fn open_page_in_split(&mut self, page: Box<dyn Page>) {
        let new_id = self.alloc_pane_id();
        self.tab_mut()
            .split(Direction::Vertical, SPLIT_RATIO, new_id);
        self.tab_mut().balance();
        self.pane_titles.insert(new_id, page.title());
        self.pages.insert(new_id, page);
        self.modes.insert(new_id, Mode::Page);
        if self.renderer.is_some() {
            self.resize_all_panes();
        }
        self.dirty = true;
    }

    /// Close every open page. A page is not a process and is not restored, so
    /// it leaves the layout before a session snapshot is taken.
    pub(crate) fn close_all_pages(&mut self) {
        let open: Vec<PaneId> = self.pages.keys().copied().collect();
        for pane_id in open {
            self.close_pane_in_any_tab(pane_id);
        }
    }

    /// Offer `key` to the page in `pane_id`. Returns whether the key was spent,
    /// so a key the page declines still reaches the ordinary keymap.
    pub(crate) fn offer_key_to_page(&mut self, pane_id: PaneId, key: &Key) -> bool {
        let Some(page) = self.pages.get_mut(&pane_id) else {
            return false;
        };
        let outcome = page.on_key(key);
        // A listing that descended into a subdirectory is a different page
        // than the one that opened, so the tab's label follows it.
        let title = page.title();
        self.pane_titles.insert(pane_id, title);
        match outcome {
            PageOutcome::Close => {
                self.close_pane_in_any_tab(pane_id);
                self.dirty = true;
                true
            }
            PageOutcome::Consumed => {
                self.dirty = true;
                true
            }
            PageOutcome::Ignored => false,
            PageOutcome::OpenPath(path) => {
                self.open_file_in_new_tab(path, None);
                true
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::KeyCode;
    use crate::model::page::{PageContent, PageSpan};

    /// A page that counts the keys it claims, so a test can tell a key the
    /// page spent from one it handed back.
    #[derive(Default)]
    struct CountingPage {
        claimed: usize,
    }

    impl Page for CountingPage {
        fn title(&self) -> String {
            "Counting".to_string()
        }

        fn content(&mut self, _rows: usize) -> PageContent {
            PageContent::new(vec![vec![PageSpan::plain("counting")]])
        }

        fn on_key(&mut self, key: &Key) -> PageOutcome {
            match key.code {
                KeyCode::Char('x') => {
                    self.claimed += 1;
                    PageOutcome::Consumed
                }
                KeyCode::Char('q') => PageOutcome::Close,
                _ => PageOutcome::Ignored,
            }
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

    fn app_with_open_page() -> (App, PaneId) {
        let mut app = App::new();
        app.open_page_in_split(Box::new(CountingPage::default()));
        let pane = app.tab().focused();
        (app, pane)
    }

    #[test]
    fn test_opening_a_page_splits_and_focuses_it_without_a_process() {
        let (app, pane) = app_with_open_page();
        assert_eq!(app.tab().panes().len(), 2);
        assert!(app.pages.contains_key(&pane), "the split holds the page");
        assert!(
            !app.panes.contains_key(&pane),
            "a page pane must never also hold a terminal"
        );
        assert_eq!(app.modes.get(&pane).copied(), Some(Mode::Page));
    }

    #[test]
    fn test_declined_key_falls_through_to_the_host() {
        let (mut app, pane) = app_with_open_page();
        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('x'))));
        assert!(!app.offer_key_to_page(pane, &press(KeyCode::Char('z'))));
    }

    #[test]
    fn test_page_closing_itself_drops_the_pane_and_its_state() {
        let (mut app, pane) = app_with_open_page();
        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('q'))));
        assert!(app.pages.is_empty(), "the page is gone");
        assert!(!app.modes.contains_key(&pane), "and so is its mode");
        assert_eq!(app.tab().panes().len(), 1, "the split collapsed");
    }

    #[test]
    fn test_a_terminal_pane_is_never_offered_a_page_key() {
        // The two maps are disjoint, so offering a terminal pane's id must not
        // find a page to hand the key to.
        let (mut app, page_pane) = app_with_open_page();
        let terminal = app
            .tab()
            .panes()
            .into_iter()
            .find(|id| *id != page_pane)
            .expect("the pane the page split off from");
        assert!(!app.offer_key_to_page(terminal, &press(KeyCode::Char('x'))));
    }

    #[test]
    fn test_pages_leave_the_layout_before_a_session_snapshot() {
        // A page is not a process: left in the saved layout it would come back
        // as a leaf with nothing behind it.
        let (mut app, _) = app_with_open_page();
        app.close_all_pages();
        assert!(app.pages.is_empty());
        assert_eq!(app.tab().panes().len(), 1);
    }

    #[test]
    fn test_resize_leaves_a_page_pane_alone() {
        // Resizing walks every laid-out pane; a page has no PTY to signal and
        // no grid to reflow, so the pass must skip it rather than panic.
        let (mut app, pane) = app_with_open_page();
        app.resize_all_panes();
        assert!(app.pages.contains_key(&pane));
    }
}

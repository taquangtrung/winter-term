//! Tool pages over panes: opening one in place, closing it, and offering it
//! keys before the modal keymap sees them.

use std::path::PathBuf;

use crate::model::input::Key;
use crate::model::layout::PaneId;
use crate::model::mode::Mode;
use crate::model::page::{Page, PageOutcome};
use crate::tools::dir::DirPage;
use crate::tools::keys::KeysPage;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Tool name recorded for the directory listing, so its own chord toggles it.
const DIR_TOOL: &str = "dir";

/// Tool name recorded for the keys page.
const KEYS_TOOL: &str = "keys";

/// The glyph each tool shows in the status bar, in place of a mode icon. One
/// line per tool, from the Font Awesome range every Nerd Font carries.
const TOOL_ICONS: [(&str, char); 2] = [(DIR_TOOL, '\u{f07b}'), (KEYS_TOOL, '\u{f11c}')];

// ========================================================================
// Data Structures
// ========================================================================

/// A page covering one pane: the page itself, which tool opened it, and the
/// mode the pane was in, so closing the page puts the pane back as it was.
pub(crate) struct PageSlot {
    pub(crate) page: Box<dyn Page>,
    prior_mode: Mode,
    tool: &'static str,
}

// ========================================================================
// PageSlot
// ========================================================================

impl PageSlot {
    /// How the status bar names what owns the keyboard: the tool's glyph, when
    /// it has one, then the page's own title.
    pub(crate) fn status_label(&self) -> String {
        let title = self.page.title();
        match TOOL_ICONS.iter().find(|(tool, _)| *tool == self.tool) {
            Some((_, icon)) => format!("{icon} {title}"),
            None => title,
        }
    }
}

// ========================================================================
// App: tool pages
// ========================================================================

impl App {
    /// Show the keys page over the focused pane, or close it if it is already
    /// the page showing there.
    pub(crate) fn open_keys_page(&mut self) {
        if self.close_page_if_showing(KEYS_TOOL) {
            return;
        }
        let page = KeysPage::new(&self.window_keymap);
        self.show_page(KEYS_TOOL, Box::new(page));
    }

    /// Show a directory listing over the focused pane, rooted at that pane's
    /// working directory, or close it if a listing is already showing there.
    pub(crate) fn open_dir_page(&mut self) {
        if self.close_page_if_showing(DIR_TOOL) {
            return;
        }
        let root = self
            .focused_cwd()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"));
        self.show_page(DIR_TOOL, Box::new(DirPage::new(root)));
    }

    /// Cover the focused pane with `page`. The pane keeps its process, which
    /// goes on running underneath, and gets it back when the page closes.
    pub(crate) fn show_page(&mut self, tool: &'static str, page: Box<dyn Page>) {
        let pane_id = self.tab().focused();
        let prior_mode = self.modes.get(&pane_id).copied().unwrap_or_default();
        self.pages.insert(
            pane_id,
            PageSlot {
                page,
                prior_mode,
                tool,
            },
        );
        self.modes.insert(pane_id, Mode::Page);
        // Rich blocks are anchored to the terminal grid the page now covers,
        // so their tiles would otherwise float on top of the listing.
        self.webview_mgr.hide_all();
        self.last_tile_layout = None;
        self.dirty = true;
    }

    /// Uncover `pane_id`, restoring the mode it was in before the page opened.
    pub(crate) fn close_page(&mut self, pane_id: PaneId) {
        let Some(slot) = self.pages.remove(&pane_id) else {
            return;
        };
        self.modes.insert(pane_id, slot.prior_mode);
        self.last_tile_layout = None;
        self.dirty = true;
    }

    /// Close the focused pane's page when `tool` is what opened it, so a tool's
    /// own chord toggles rather than reopening it. Returns whether it closed.
    fn close_page_if_showing(&mut self, tool: &'static str) -> bool {
        let pane_id = self.tab().focused();
        if self
            .pages
            .get(&pane_id)
            .is_some_and(|slot| slot.tool == tool)
        {
            self.close_page(pane_id);
            return true;
        }
        false
    }

    /// Offer `key` to the page covering `pane_id`. Returns whether the key was
    /// spent, so a key the page declines still reaches the ordinary keymap.
    pub(crate) fn offer_key_to_page(&mut self, pane_id: PaneId, key: &Key) -> bool {
        let Some(slot) = self.pages.get_mut(&pane_id) else {
            return false;
        };
        match slot.page.on_key(key) {
            PageOutcome::Close => {
                self.close_page(pane_id);
                true
            }
            PageOutcome::Consumed => {
                self.dirty = true;
                true
            }
            PageOutcome::Ignored => false,
            PageOutcome::OpenExternal(path) => {
                match ::open::that(&path) {
                    Ok(()) => self.set_notice(format!("opened {}", path.display())),
                    Err(e) => self.set_error(format!("could not open {}: {e}", path.display())),
                }
                true
            }
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
        app.show_page("counting", Box::new(CountingPage::default()));
        let pane = app.tab().focused();
        (app, pane)
    }

    #[test]
    fn test_a_page_covers_the_focused_pane_without_splitting_it() {
        let (app, pane) = app_with_open_page();
        assert_eq!(app.tab().panes().len(), 1, "no new pane is created");
        assert!(app.pages.contains_key(&pane));
        assert_eq!(app.modes.get(&pane).copied(), Some(Mode::Page));
    }

    #[test]
    fn test_closing_a_page_gives_the_pane_back_as_it_was() {
        // The terminal underneath kept running, so the pane has to return to
        // the mode it was in rather than being closed or left in Page mode.
        let mut app = App::new();
        let pane = app.tab().focused();
        app.modes.insert(pane, Mode::Normal);
        app.show_page("counting", Box::new(CountingPage::default()));

        app.close_page(pane);
        assert!(app.pages.is_empty());
        assert_eq!(app.tab().panes().len(), 1);
        assert_eq!(app.modes.get(&pane).copied(), Some(Mode::Normal));
    }

    #[test]
    fn test_a_tool_chord_pressed_twice_closes_its_own_page() {
        let mut app = App::new();
        let pane = app.tab().focused();
        app.open_keys_page();
        assert!(app.pages.contains_key(&pane));
        app.open_keys_page();
        assert!(app.pages.is_empty(), "the same tool toggles off");
    }

    #[test]
    fn test_another_tool_replaces_the_page_instead_of_toggling() {
        let mut app = App::new();
        let pane = app.tab().focused();
        app.open_keys_page();
        app.open_dir_page();
        assert!(app.pages.contains_key(&pane), "the pane still shows a page");
        assert_eq!(app.pages.len(), 1);
    }

    #[test]
    fn test_declined_key_falls_through_to_the_host() {
        let (mut app, pane) = app_with_open_page();
        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('x'))));
        assert!(!app.offer_key_to_page(pane, &press(KeyCode::Char('z'))));
    }

    #[test]
    fn test_a_page_closing_itself_uncovers_the_pane() {
        let (mut app, pane) = app_with_open_page();
        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('q'))));
        assert!(app.pages.is_empty(), "the page is gone");
        assert_eq!(app.tab().panes().len(), 1, "the pane is not");
    }

    #[test]
    fn test_a_pane_with_no_page_is_never_offered_a_page_key() {
        // Keys for an uncovered pane must reach its terminal, so the lookup has
        // to miss rather than find some other pane's page.
        let (mut app, covered) = app_with_open_page();
        let other = PaneId(covered.0 + 1);
        assert!(!app.offer_key_to_page(other, &press(KeyCode::Char('x'))));
    }

    #[test]
    fn test_resize_leaves_a_covered_pane_showing_its_page() {
        // Resizing walks every laid-out pane and reflows the grid underneath;
        // the page covering it must survive the pass.
        let (mut app, pane) = app_with_open_page();
        app.resize_all_panes();
        assert!(app.pages.contains_key(&pane));
    }
}

//! Tool pages over panes: opening one in place, closing it, and offering it
//! keys before the modal keymap sees them.

use std::path::PathBuf;

use crate::model::input::{Key, KeyCode};
use crate::model::layout::PaneId;
use crate::model::mode::Mode;
use crate::model::page::{Page, PageOutcome, PromptMode, PromptReply, PromptRequest};
use crate::tools::dir::DirPage;
use crate::tools::git::GitPage;
use crate::tools::grep::GrepPage;
use crate::tools::keys::KeysPage;
use winter_render::Grid;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Tool name recorded for the directory listing, so its own chord toggles it.
const DIR_TOOL: &str = "dir";

/// Tool name recorded for the git view.
const GIT_TOOL: &str = "git";

/// Tool name recorded for the text search.
const GREP_TOOL: &str = "grep";

/// Tool name recorded for the keys page.
const KEYS_TOOL: &str = "keys";

/// Drawn after a prompt's typed text, so the line reads as an input.
const CARET: char = '\u{2502}';

/// The glyph each tool shows in the status bar, in place of a mode icon. One
/// line per tool, from the Font Awesome range every Nerd Font carries.
const TOOL_ICONS: [(&str, char); 4] = [
    (DIR_TOOL, '\u{f07b}'),
    (GIT_TOOL, '\u{f1d3}'),
    (GREP_TOOL, '\u{f002}'),
    (KEYS_TOOL, '\u{f11c}'),
];

// ========================================================================
// Data Structures
// ========================================================================

/// A question a page asked, and the answer being typed for it.
pub(crate) struct ActivePrompt {
    input: String,
    pane: PaneId,
    request: PromptRequest,
}

/// A page covering one pane: the page itself, which tool opened it, and the
/// mode the pane was in, so closing the page puts the pane back as it was.
pub(crate) struct PageSlot {
    pub(crate) page: Box<dyn Page>,
    /// The row the page last banded as its cursor line, so a text cursor
    /// starts where the user was already looking.
    pub(crate) cursor_line: Option<usize>,
    /// The grid the page was last painted into, retained so that selecting
    /// over the pane reads what is on screen.
    ///
    /// A page draws over a pane whose terminal is still running underneath.
    /// Without this, a selection resolved against `panes[..].grid()` names the
    /// shell output hidden behind the page, and copying a listing silently
    /// yields whatever scrolled past before the tool opened.
    pub(crate) painted: Option<Grid>,
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

    /// How the tab bar names the pane while the page covers it: the tool's
    /// name rather than the status bar's glyph, so the tab reads in any font
    /// and names the tool even where the page's title alone would not — a dir
    /// page titled "winter-term" says a directory, not that `dir` is the one
    /// listing it. A title that merely restates the tool's name ("Keys") is
    /// dropped rather than doubled up.
    pub(crate) fn tab_label(&self) -> String {
        let title = self.page.title();
        if title.eq_ignore_ascii_case(self.tool) {
            title
        } else {
            format!("{}: {}", self.tool, title)
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
        self.show_page(DIR_TOOL, Box::new(DirPage::new(self.page_start_dir())));
    }

    /// Show the working tree's state over the focused pane, for the repository
    /// containing that pane's working directory.
    pub(crate) fn open_git_page(&mut self) {
        if self.close_page_if_showing(GIT_TOOL) {
            return;
        }
        let page = GitPage::new(self.page_start_dir());
        // The view knows nothing until git answers, so its first request goes
        // out with it rather than waiting for a keystroke.
        let first = page.initial_request();
        self.show_page(GIT_TOOL, Box::new(page));
        let pane_id = self.tab().focused();
        self.jobs.spawn(pane_id, first);
    }

    /// Search the pane's working directory for text, or close the search if one
    /// is already showing there.
    pub(crate) fn open_grep_page(&mut self) {
        if self.close_page_if_showing(GREP_TOOL) {
            return;
        }
        self.show_page(GREP_TOOL, Box::new(GrepPage::new(self.page_start_dir())));
    }

    /// Where a tool opens: the focused pane's working directory, falling back
    /// to this process's own.
    fn page_start_dir(&self) -> PathBuf {
        self.focused_cwd()
            .map(PathBuf::from)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("/"))
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
                cursor_line: None,
                painted: None,
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
        // A question, a text cursor, and any work in flight all belong to the
        // page that opened them: a cursor outliving its page would go on
        // swallowing keys for rows that are no longer painted.
        if self.page_prompt.as_ref().is_some_and(|p| p.pane == pane_id) {
            self.page_prompt = None;
        }
        if self.page_cursor.as_ref().is_some_and(|c| c.pane == pane_id) {
            self.stop_page_cursor();
        }
        self.jobs.cancel_for(pane_id);
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
        // Wrap belongs to the pane, not the tool: every page gets the toggle,
        // ahead of its own keys, so no tool has to claim it itself.
        if key.alt && !key.ctrl && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('Z')) {
            self.page_wrap = !self.page_wrap;
            self.dirty = true;
            return true;
        }
        let Some(slot) = self.pages.get_mut(&pane_id) else {
            return false;
        };
        let outcome = slot.page.on_key(key);
        self.act_on_page_outcome(pane_id, outcome)
    }

    /// Route one key into the open prompt: `Enter` answers, `Esc` cancels, and
    /// a confirm prompt resolves on the first key it sees.
    pub(crate) fn handle_prompt_key(&mut self, key: &Key) {
        let Some(prompt) = self.page_prompt.as_mut() else {
            return;
        };
        let confirm = prompt.request.mode == PromptMode::Confirm;
        let answer = match key.code {
            KeyCode::Escape => Some(None),
            KeyCode::Enter if !confirm => Some(Some(prompt.input.clone())),
            KeyCode::Char(c) if confirm => Some((c == 'y' || c == 'Y').then(|| c.to_string())),
            KeyCode::Backspace if !confirm => {
                prompt.input.pop();
                None
            }
            KeyCode::Char(c) if !key.ctrl && !key.alt => {
                prompt.input.push(c);
                None
            }
            _ => None,
        };
        self.dirty = true;
        let Some(answer) = answer else {
            return;
        };
        let Some(prompt) = self.page_prompt.take() else {
            return;
        };
        let pane_id = prompt.pane;
        let reply = PromptReply {
            answer,
            tag: prompt.request.tag,
        };
        let Some(slot) = self.pages.get_mut(&pane_id) else {
            return;
        };
        let outcome = slot.page.on_prompt(reply);
        self.act_on_page_outcome(pane_id, outcome);
    }

    /// What the open prompt shows: its question, then what has been typed.
    pub(crate) fn prompt_display(&self) -> Option<String> {
        let prompt = self.page_prompt.as_ref()?;
        match prompt.request.mode {
            PromptMode::Confirm => Some(prompt.request.label.clone()),
            PromptMode::Text => Some(format!("{}{}{CARET}", prompt.request.label, prompt.input)),
        }
    }

    /// Carry out what a page asked for. Returns whether the input that produced
    /// it was spent.
    pub(super) fn act_on_page_outcome(&mut self, pane_id: PaneId, outcome: PageOutcome) -> bool {
        match outcome {
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
            PageOutcome::OpenPath(target) => {
                self.open_file_in_new_tab(target.path, target.line);
                true
            }
            PageOutcome::CancelJobs => {
                self.jobs.cancel_for(pane_id);
                self.dirty = true;
                true
            }
            PageOutcome::Job(request) => {
                self.jobs.spawn(pane_id, request);
                true
            }
            PageOutcome::Spawn(request) => {
                self.spawn_in_tab(request);
                true
            }
            PageOutcome::Yank(value) => {
                let copied = self
                    .clipboard()
                    .and_then(|clipboard| clipboard.set_text(&value).ok())
                    .is_some();
                if copied {
                    self.set_notice(format!("copied {value}"));
                } else {
                    self.set_error("clipboard unavailable");
                }
                true
            }
            PageOutcome::Prompt(request) => {
                self.page_prompt = Some(ActivePrompt {
                    input: request.initial.clone(),
                    pane: pane_id,
                    request,
                });
                self.dirty = true;
                true
            }
            PageOutcome::RunAction(action) => {
                // The pane goes back before the command runs, so a command that
                // opens a page of its own has an uncovered pane to open in.
                self.close_page(pane_id);
                self.run_command(&action, pane_id);
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
    use std::cell::RefCell;
    use std::rc::Rc;

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

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PageContent {
            PageContent::new(vec![vec![PageSpan::plain("counting")]])
        }

        fn on_key(&mut self, key: &Key) -> PageOutcome {
            match key.code {
                KeyCode::Char('x') => {
                    self.claimed += 1;
                    PageOutcome::Consumed
                }
                KeyCode::Char('r') => PageOutcome::RunAction("new_tab".to_string()),
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
    fn test_a_page_running_a_command_uncovers_the_pane_first() {
        // A command that opens a page of its own needs a pane with none on it,
        // so the running and the uncovering cannot be the other way around.
        let (mut app, pane) = app_with_open_page();
        let tabs_before = app.tabs.all.len();

        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('r'))));
        assert!(app.pages.is_empty(), "the page handed the pane back");
        assert_eq!(app.tabs.all.len(), tabs_before + 1, "and the command ran");
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
    fn test_alt_z_toggles_wrapping_without_reaching_the_page() {
        // Wrapping belongs to the pane rather than any one tool, so the host
        // spends the toggle itself: the key is claimed rather than declined,
        // which would otherwise hand it to the ordinary keymap.
        let (mut app, pane) = app_with_open_page();
        let mut key = press(KeyCode::Char('z'));
        key.alt = true;
        assert!(app.offer_key_to_page(pane, &key), "the toggle is spent");
        assert!(app.page_wrap);
        assert!(app.offer_key_to_page(pane, &key));
        assert!(!app.page_wrap);
    }

    /// A page that asks one question, sharing the answer it hears with the
    /// test that opened it.
    struct AskingPage {
        heard: Rc<RefCell<Vec<Option<String>>>>,
    }

    impl Page for AskingPage {
        fn title(&self) -> String {
            "Asking".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PageContent {
            PageContent::new(vec![vec![PageSpan::plain("asking")]])
        }

        fn on_key(&mut self, _key: &Key) -> PageOutcome {
            PageOutcome::Prompt(PromptRequest {
                initial: "seed".to_string(),
                label: "Name: ".to_string(),
                mode: PromptMode::Text,
                tag: "ask",
            })
        }

        fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
            self.heard.borrow_mut().push(reply.answer);
            PageOutcome::Consumed
        }
    }

    fn app_asking() -> (App, PaneId, Rc<RefCell<Vec<Option<String>>>>) {
        let mut app = App::new();
        let heard: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
        app.show_page(
            "asking",
            Box::new(AskingPage {
                heard: Rc::clone(&heard),
            }),
        );
        let pane = app.tab().focused();
        app.offer_key_to_page(pane, &press(KeyCode::Char('x')));
        (app, pane, heard)
    }

    #[test]
    fn test_a_prompt_starts_holding_its_initial_text() {
        let (app, _, _) = app_asking();
        assert_eq!(app.prompt_display().as_deref(), Some("Name: seed\u{2502}"));
    }

    #[test]
    fn test_typing_edits_the_prompt_and_enter_delivers_the_answer() {
        let (mut app, pane, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Backspace));
        app.handle_prompt_key(&press(KeyCode::Char('!')));
        assert_eq!(app.prompt_display().as_deref(), Some("Name: see!\u{2502}"));

        app.handle_prompt_key(&press(KeyCode::Enter));
        assert!(app.page_prompt.is_none(), "the prompt closes on Enter");
        assert!(app.pages.contains_key(&pane), "and the page stays open");
        assert_eq!(heard.borrow().as_slice(), [Some("see!".to_string())]);
    }

    #[test]
    fn test_escape_cancels_a_prompt_and_says_so() {
        // The page has to hear the cancellation, not just stop hearing: an
        // operation waiting on an answer would otherwise stay half-started.
        let (mut app, _, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Escape));
        assert!(app.page_prompt.is_none());
        assert_eq!(heard.borrow().as_slice(), [None]);
    }

    #[test]
    fn test_closing_the_page_takes_its_question_with_it() {
        // A prompt left behind would deliver its answer to a page that is gone,
        // or worse, to whatever page opened next in that pane.
        let (mut app, pane, heard) = app_asking();
        assert!(app.page_prompt.is_some());
        app.close_page(pane);
        assert!(app.page_prompt.is_none());
        assert!(heard.borrow().is_empty(), "and answers nothing");
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

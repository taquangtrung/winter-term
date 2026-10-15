//! Page tabs: opening one beside the focused tab, closing and reopening it,
//! and offering it keys before the window keymap sees them.

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::control::EditWaiter;
use crate::model::input::{self, Key, KeyCode};
use crate::model::layout::{Direction, LayoutTree, Rect, TabId};
use crate::model::page::{
    OpenTarget, Page, PageOutcome, PagePoint, PromptMode, PromptReply, PromptRequest,
};
use crate::model::palette::{Palette, PaletteMode};
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditOutcome;
use crate::model::textedit::recall::Recall;
use crate::pages::dir::DirPage;
use crate::pages::editor::{EditorPage, DIRTY_MARK};
use crate::pages::git::GitPage;
use crate::pages::grep::GrepPage;
use crate::pages::keys::KeysPage;
use crate::pages::pdf::PdfPage;
use crate::pages::proc::ProcPage;
use crate::pages::reltime;
use crate::pages::sys::SysPage;
use crate::session::PageSession;
use winter_render::Grid;
#[cfg(test)]
use winter_render::InputView;

use super::{App, SPLIT_RATIO};

// ========================================================================
// Constants
// ========================================================================

/// How many closed things are kept for reopening. The point of the stash is
/// the page just closed by accident, not a history of every page ever closed,
/// and each entry holds a whole page: an editor's buffers, a listing's rows.
const MAX_CLOSED: usize = 8;

/// The ceiling the stash is trimmed to even when everything in it is holding
/// unwritten edits. Past this much unsaved work nothing further can be kept
/// without letting a session grow without bound, and the oldest goes however
/// dirty it is.
const MAX_CLOSED_UNSAVED: usize = 32;

/// The command the first-close hint names, looked up in the keymap in force
/// rather than written out, so the hint cannot name a key that is not bound.
const REOPEN_COMMAND: &str = "reopen_page";

/// What a row of the tab list is: a page that is open somewhere, or one that
/// was closed. The rest of the row's action says which one.
const OPEN_ROW: &str = "open:";
const CLOSED_ROW: &str = "closed:";

/// Which question the host asked when a close would take unwritten edits with
/// it. The host answers this one itself: no page asked it.
const ASK_CLOSE_UNSAVED: &str = "close-unsaved";

/// How a running program is named when the system does not say.
const RUNNING_PROGRAM: &str = "a program";

/// How a row of the tab list says where an open page is: the tab showing
/// in its pane, or one waiting on that pane's strip.
const SHOWING_HINT: &str = "showing";
const OPEN_HINT: &str = "open";

/// How the list of closed things names a whole pane, and terminal tabs
/// closed out of a pane that stayed.
const CLOSED_PANE_NOUN: &str = "pane";
const CLOSED_TERMINAL_NOUN: &str = "terminal";

/// Said when there is nothing to put back, rather than the key doing nothing
/// at all.
const NOTHING_CLOSED: &str = "nothing has been closed to reopen";

/// Said when the row picked stood for a page the stash has since dropped,
/// which is the one way a list drawn a moment ago can be out of date.
const GONE_FROM_LIST: &str = "that tab is no longer there to reopen";

#[cfg(test)]
const HINT_CONFIRM: &str = "y to confirm, any other key to cancel";
#[cfg(test)]
const HINT_TEXT: &str = "Enter to accept, Esc to cancel";

// ========================================================================
// Data Structures
// ========================================================================

/// Which kind of page a tab holds: a peer of the terminal, opened by its own
/// chord and toggled by it again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PageKind {
    Dir,
    Editor,
    Git,
    Grep,
    Keys,
    Pdf,
    Proc,
    Sys,
}

/// A question a page asked, and the answer being typed for it.
pub(crate) struct ActivePrompt {
    /// What has been typed, edited with the same keys as any other text.
    pub(crate) field: TextField,
    /// How far the arrow keys have walked back through earlier answers.
    pub(crate) recall: Recall,
    /// The tab that asked.
    pub(crate) tab: TabId,
    pub(crate) request: PromptRequest,
}

/// The page question the palette is standing in for while it shows a list of
/// that page's own choices.
pub(crate) struct ActivePick {
    /// What the list is of, shown where the palette shows its prompt.
    pub(crate) label: String,
    /// The page that asked.
    pub(crate) tab: TabId,
    /// The question being answered, carried back with the choice.
    pub(crate) tag: &'static str,
}

/// What the host is waiting to close once its question is answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingClose {
    /// Every pane but the one holding this tab.
    OtherPanes(TabId),
    /// Every tab but this one, in its pane.
    OtherTabs(TabId),
    /// The pane holding this tab, and every tab in it.
    Terminal(TabId),
    /// This one tab.
    Tab(TabId),
    /// The whole window.
    Quit,
}

/// Whether what is being closed is worth keeping for reopening.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Closing {
    /// The reader may want this back: a page closed with `q`, a pane merged
    /// away, a tab closed. These are the closes that turn out to be
    /// accidents.
    Keep,
    /// Nothing to keep: a terminal whose shell exited on its own. Keeping
    /// these would bury the closes a reader actually wants back.
    Forget,
}

/// Something closed that can be put back, oldest first in `App::closed`.
pub(crate) enum Closed {
    /// One page.
    Page(ClosedPage),
    /// Terminal tabs, or a whole pane: where they sat, what they were
    /// running in, and the page tabs that went with them.
    Terminal(ClosedPane),
}

/// Terminal tabs that were closed, as much of them as can come back: one
/// terminal tab, or a whole pane with every tab it held.
///
/// The shells themselves cannot come back: closing a terminal dropped its
/// pseudo-terminal and the child with it. What is kept is where they sat, so
/// a merged-away split comes back rather than a fresh one wherever the reader
/// now is, the directory each shell was in, so its replacement starts there,
/// and the page tabs that went with them, which are kept whole beside them.
pub(crate) struct ClosedPane {
    closed_at: i64,
    /// Whether the whole pane went, rather than tabs inside one that stayed.
    whole_pane: bool,
    id: u64,
    /// The window's split tree as it was, the snapshot the tabs are put back
    /// into when the rest of the layout has not moved on since.
    layout: LayoutTree,
    /// The page tabs that went with them, by the id they are kept under in
    /// the same stash.
    pages: Vec<u64>,
    /// The terminal tabs, in strip order.
    terminals: Vec<ClosedTerminal>,
}

/// One closed terminal tab: which tab it was, and where its shell was.
#[derive(Clone, Debug)]
pub(crate) struct ClosedTerminal {
    /// The shell's working directory, for the shell that takes its place.
    pub(crate) cwd: Option<String>,
    pub(crate) tab: TabId,
}

/// A page that was closed, kept whole rather than described.
///
/// Reopening one is the page coming back, not a new page of its kind: the
/// buffers an editor had open and the edits never written to them, where a
/// listing's cursor was and what it had folded, the page a document was
/// turned to. Rebuilding from a description would lose exactly what the
/// reader lost by closing it.
pub(crate) struct ClosedPage {
    /// When it was closed, in seconds since the epoch, for the age the list
    /// of them shows: the order alone does not say whether the page above was
    /// closed a moment ago or an hour ago.
    closed_at: i64,
    /// Which closed page this is, so a row of the list means the same page
    /// however the list has changed since it was drawn.
    id: u64,
    page: Box<dyn Page>,
    /// The tab it was, so a layout snapshot naming it still finds it.
    tab: TabId,
    kind: PageKind,
}

// ========================================================================
// PageKind
// ========================================================================

impl PageKind {
    /// Every kind, for reading a stored name back.
    const ALL: [PageKind; 8] = [
        PageKind::Dir,
        PageKind::Editor,
        PageKind::Git,
        PageKind::Grep,
        PageKind::Keys,
        PageKind::Pdf,
        PageKind::Proc,
        PageKind::Sys,
    ];

    /// The name a session file records and the icon tables are keyed by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            PageKind::Dir => "dir",
            PageKind::Editor => "editor",
            PageKind::Git => "git",
            PageKind::Grep => "grep",
            PageKind::Keys => "keys",
            PageKind::Pdf => "pdf",
            PageKind::Proc => "proc",
            PageKind::Sys => "sys",
        }
    }

    /// The kind a stored name stands for, or `None` for one no longer known.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// How the title bar and the window title name the kind.
    pub(crate) fn title(self) -> &'static str {
        match self {
            PageKind::Dir => "Dir",
            PageKind::Editor => "Editor",
            PageKind::Git => "Git",
            PageKind::Grep => "Grep",
            PageKind::Keys => "Keys",
            PageKind::Pdf => "PDF",
            PageKind::Proc => "Processes",
            PageKind::Sys => "System",
        }
    }

    /// The glyph the status bar draws on its right edge for the kind, from
    /// the Font Awesome range every Nerd Font carries.
    fn status_icon(self) -> char {
        match self {
            PageKind::Dir => '\u{f07b}',
            PageKind::Editor => '\u{f044}',
            PageKind::Git => '\u{f1d3}',
            PageKind::Grep => '\u{f002}',
            PageKind::Keys => '\u{f11c}',
            PageKind::Pdf => '\u{f1c1}',
            PageKind::Proc => '\u{f080}',
            PageKind::Sys => '\u{f2db}',
        }
    }
}

// ========================================================================
// Closed
// ========================================================================

impl Closed {
    /// When it was closed, for the age its row shows.
    fn closed_at(&self) -> i64 {
        match self {
            Closed::Page(page) => page.closed_at,
            Closed::Terminal(pane) => pane.closed_at,
        }
    }

    /// Which closed thing this is, so a row of the list stands for it rather
    /// than for a position in a list that moves.
    fn id(&self) -> u64 {
        match self {
            Closed::Page(page) => page.id,
            Closed::Terminal(pane) => pane.id,
        }
    }

    /// Whether putting this back would recover work that is nowhere else. A
    /// pane answers for itself only: the pages it was holding are kept beside
    /// it and answer for themselves.
    fn is_dirty(&self) -> bool {
        match self {
            Closed::Page(page) => page.page.is_dirty(),
            Closed::Terminal(_) => false,
        }
    }
}

// ========================================================================
// ClosedPane
// ========================================================================

impl ClosedPane {
    /// Whether the whole pane went, rather than tabs inside one that stayed.
    pub(super) fn is_whole_pane(&self) -> bool {
        self.whole_pane
    }

    /// The window's split tree as it was.
    pub(super) fn layout(&self) -> LayoutTree {
        self.layout.clone()
    }

    /// The page tabs that went with them.
    pub(super) fn page_ids(&self) -> Vec<u64> {
        self.pages.clone()
    }

    /// The terminal tabs, in strip order.
    pub(super) fn terminals(&self) -> Vec<ClosedTerminal> {
        self.terminals.clone()
    }
}

// ========================================================================
// ClosedPage
// ========================================================================

impl ClosedPage {
    /// How the tab list names this one.
    fn label(&self) -> String {
        tab_row_label(self.kind, self.page.as_ref())
    }
}

/// A page tab's contents: the page itself, which kind it is, and what the
/// host keeps of its last frame.
pub(crate) struct PageSlot {
    pub(crate) page: Box<dyn Page>,
    /// The row the page last banded as its cursor line, so a text cursor
    /// starts where the user was already looking.
    pub(crate) cursor_line: Option<usize>,
    /// The grid the page was last painted into, retained so that selecting
    /// over the tab reads what is on screen: a page has no terminal grid of
    /// its own to resolve a selection against.
    pub(crate) painted: Option<Grid>,
    kind: PageKind,
}

// ========================================================================
// PageSlot
// ========================================================================

impl PageSlot {
    /// A page of `kind` that has not been painted yet.
    fn new(kind: PageKind, page: Box<dyn Page>) -> Self {
        Self {
            page,
            cursor_line: None,
            painted: None,
            kind,
        }
    }

    /// Which kind of page this is.
    pub(crate) fn kind(&self) -> PageKind {
        self.kind
    }

    /// The packed icon a pane strip draws before the tab's title. An editor
    /// shows the type icon of the file it has open; every other kind its own.
    pub(crate) fn tab_icon(&self) -> String {
        match self.kind {
            PageKind::Editor => crate::icons::file_icon(&self.page.title()),
            PageKind::Dir
            | PageKind::Git
            | PageKind::Grep
            | PageKind::Keys
            | PageKind::Pdf
            | PageKind::Proc
            | PageKind::Sys => crate::icons::page_icon(self.kind.name()),
        }
    }

    /// The glyph the status bar draws on its right edge for this page.
    pub(crate) fn status_icon(&self) -> char {
        self.kind.status_icon()
    }

    /// How a pane strip names the tab. A page whose icon already names it (an
    /// editor's file-type icon, `dir`'s folder) shows just the page's title; a
    /// page on the default file icon keeps its kind in front, so the tab still
    /// says what is listing what.
    pub(crate) fn tab_label(&self) -> String {
        let title = self.page.title();
        match self.kind == PageKind::Editor || crate::icons::has_page_icon(self.kind.name()) {
            true => title,
            false => page_label(self.kind, &title),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Now, in seconds since the epoch, which is what an age is measured from. A
/// clock that cannot answer reads as the epoch rather than failing a list of
/// closed pages over a wrong timestamp.
fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since| since.as_secs() as i64)
        .unwrap_or_default()
}

/// How a dialog names what is holding unwritten edits: the one thing, or the
/// first of them and how many others there are.
fn named(unsaved: &[String]) -> String {
    match unsaved.split_first() {
        Some((first, [])) => first.clone(),
        Some((first, rest)) => format!("{first} and {} more", rest.len()),
        None => String::new(),
    }
}

/// How a row of the tab list names one page: its kind, what it is showing, a
/// mark when it is holding edits that are on no disk, and the directory it is
/// looking at. The directory is what tells two files of the same name apart,
/// and what makes either findable by the tree it sits in.
fn tab_row_label(kind: PageKind, page: &dyn Page) -> String {
    let mut label = page_label(kind, &page.title());
    if page.is_dirty() {
        label.push_str(DIRTY_MARK);
    }
    if let Some(dir) = page.cwd() {
        label.push_str("  ");
        label.push_str(&dir.display().to_string());
    }
    label
}

/// How a page is named away from the page itself: the kind it was opened as
/// and what it is showing. A title that merely restates the kind's name
/// ("Keys") is left to stand on its own rather than doubled up.
fn page_label(kind: PageKind, title: &str) -> String {
    let name = kind.name();
    match title.eq_ignore_ascii_case(name) {
        true => title.to_string(),
        false => format!("{name}: {title}"),
    }
}

/// The dialog for one question: what is asked, the line it is answered on for
/// a question that takes one, and how to answer it. A question answered by a
/// single key has no line to type on, so it is given none.
/// Which way a key walks a prompt's earlier answers: `Some(true)` back to an
/// older one (Up, `Ctrl-P`), `Some(false)` on to a newer (Down, `Ctrl-N`).
fn recall_step(key: &Key) -> Option<bool> {
    match (key.ctrl, key.code) {
        (false, KeyCode::Up) | (true, KeyCode::Char('p')) => Some(true),
        (false, KeyCode::Down) | (true, KeyCode::Char('n')) => Some(false),
        _ => None,
    }
}

#[cfg(test)]
pub(crate) fn input_dialog(label: &str, input: &str, caret: usize, mode: PromptMode) -> InputView {
    InputView {
        caret,
        hint: match mode {
            PromptMode::Confirm => HINT_CONFIRM.to_string(),
            PromptMode::Text => HINT_TEXT.to_string(),
        },
        input: match mode {
            PromptMode::Confirm => None,
            PromptMode::Text => Some(input.to_string()),
        },
        // Labels are written to sit before an answer on one line, so they end
        // in a separator that reads as a dangling colon over one.
        label: label.trim_end().trim_end_matches(':').to_string(),
    }
}

// ========================================================================
// App: pages
// ========================================================================

impl App {
    /// Open the keys page in a tab beside the focused one, or step away from
    /// it when it is already the tab showing (see [`Self::toggle_page`]).
    pub(crate) fn open_keys_page(&mut self) {
        if self.toggle_page(PageKind::Keys) {
            return;
        }
        let page = KeysPage::new(&self.window_keymap);
        self.show_page(PageKind::Keys, Box::new(page));
    }

    /// Open a directory listing in a tab beside the focused one, rooted where
    /// that tab is looking, or go to the listing the pane already has.
    pub(crate) fn open_dir_page(&mut self) {
        if self.toggle_page(PageKind::Dir) {
            return;
        }
        self.show_page(
            PageKind::Dir,
            Box::new(DirPage::new(self.focused_start_dir())),
        );
    }

    /// List `dir` in a new tab beside the focused one, as a directory opened
    /// from the terminal's text is.
    pub(crate) fn open_dir_page_at(&mut self, dir: PathBuf) {
        self.show_page(PageKind::Dir, Box::new(DirPage::new(dir)));
    }

    /// Open the working tree's state in a tab beside the focused one, for the
    /// repository containing where that tab is looking. If a view for this
    /// repository is already open, it is reused or toggled.
    pub(crate) fn open_git_page(&mut self) {
        let start = self.focused_start_dir();
        let matches_repo = |page: &dyn Page| {
            page.root_dir().is_some_and(|root| {
                start == root || start.starts_with(&root) || root.starts_with(&start)
            })
        };
        if self.toggle_page_matching(PageKind::Git, &matches_repo) {
            return;
        }
        // Where the view looks is decided by the pane it was asked from, before
        // focus moves to the one it opens in.
        self.prefer_other_pane();
        if self.show_open_page_matching(PageKind::Git, &matches_repo) {
            return;
        }
        let page = GitPage::new(start);
        // The view knows nothing until git answers, so its first request goes
        // out with it rather than waiting for a keystroke.
        let first = page.initial_request();
        let tab_id = self.show_page_beside(PageKind::Git, Box::new(page));
        self.jobs.spawn(tab_id, first);
    }

    /// Focus the open git page for `repo` if one exists, otherwise open a new
    /// git page for `repo` and focus it.
    pub(crate) fn focus_or_open_git_page(&mut self, repo: PathBuf) {
        let matches_repo = |page: &dyn Page| {
            page.root_dir().is_some_and(|root| {
                repo == root || repo.starts_with(&root) || root.starts_with(&repo)
            })
        };
        let open_tab = self.layout().members().into_iter().find(|&member| {
            self.open_tabs
                .page(&member)
                .is_some_and(|slot| slot.kind == PageKind::Git && matches_repo(&*slot.page))
        });
        if let Some(tab) = open_tab {
            self.show_tab(tab);
        } else {
            let page = GitPage::new(repo);
            let first = page.initial_request();
            let tab_id = self.show_page_beside(PageKind::Git, Box::new(page));
            self.jobs.spawn(tab_id, first);
        }
    }

    /// Move focus to another pane when the window has more than one, so what
    /// opens next opens there: pages that are read beside the work, not over it.
    /// The pane beside this one in the same row is preferred, then the next in
    /// tree order. With a single pane [`Self::show_page_beside`] splits one off
    /// instead.
    pub(crate) fn prefer_other_pane(&mut self) {
        if self.layout().shown_tabs().len() < 2 {
            return;
        }
        let viewport = self.viewport_rect();
        let viewport = Rect::new(viewport.x, viewport.y, viewport.width, viewport.height);
        if !self.layout_mut().focus_side_by_side(viewport) {
            self.layout_mut().focus_next();
        }
    }

    /// Show `page` beside the work: as a tab of the pane focus was moved to by
    /// [`Self::prefer_other_pane`], or, where there is only one pane, in a new
    /// one split off to its right.
    fn show_page_beside(&mut self, kind: PageKind, page: Box<dyn Page>) -> TabId {
        match self.layout().shown_tabs().len() > 1 {
            true => self.show_page(kind, page),
            false => self.show_page_split(kind, page),
        }
    }

    /// Split the focused pane vertically and show `page` in the new half, which
    /// takes focus.
    fn show_page_split(&mut self, kind: PageKind, page: Box<dyn Page>) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, tab_id);
        self.layout_mut().balance();
        self.open_tabs
            .insert_page(tab_id, PageSlot::new(kind, page));
        self.after_tab_change();
        tab_id
    }

    /// Show the `kind` tab matching `predicate` in the focused pane when it
    /// already has one, whether or not it is the one showing. Returns whether
    /// there was one.
    fn show_open_page_matching(
        &mut self,
        kind: PageKind,
        predicate: &dyn Fn(&dyn Page) -> bool,
    ) -> bool {
        let focused = self.layout().focused();
        let open = self
            .layout()
            .recent_in_pane(focused)
            .into_iter()
            .find(|tab| {
                self.open_tabs
                    .page(tab)
                    .is_some_and(|slot| slot.kind == kind && predicate(&*slot.page))
            });
        match open {
            Some(tab) => {
                self.show_tab(tab);
                true
            }
            None => false,
        }
    }

    /// Search where the focused tab is looking for text, in a tab of its own.
    pub(crate) fn open_grep_page(&mut self) {
        if self.toggle_page(PageKind::Grep) {
            return;
        }
        self.show_page(
            PageKind::Grep,
            Box::new(GrepPage::new(self.focused_start_dir())),
        );
    }

    /// Show the machine's processes in a tab of their own. The monitor asks
    /// for its own first snapshot on the next tick.
    pub(crate) fn open_proc_page(&mut self) {
        if self.toggle_page(PageKind::Proc) {
            return;
        }
        self.show_page(PageKind::Proc, Box::new(ProcPage::new()));
    }

    /// Show the machine's CPU, memory, disks, and GPUs in a tab of their own.
    pub(crate) fn open_sys_page(&mut self) {
        if self.toggle_page(PageKind::Sys) {
            return;
        }
        self.show_page(PageKind::Sys, Box::new(SysPage::new()));
    }

    /// Refresh each page that keeps itself current, and say when the earliest
    /// of them next wants to be asked.
    pub(crate) fn tick_pages(&mut self, now: Instant) -> Option<Instant> {
        let due: Vec<TabId> = self
            .open_tabs
            .pages()
            .filter(|(_, slot)| slot.page.next_tick().is_some_and(|at| at <= now))
            .map(|(pane, _)| *pane)
            .collect();
        for tab_id in due {
            let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
                continue;
            };
            let outcome = slot.page.on_tick(now);
            self.act_on_page_outcome(tab_id, outcome);
        }
        self.open_tabs
            .pages()
            .map(|(_, slot)| slot)
            .filter_map(|slot| slot.page.next_tick())
            .min()
    }

    /// Open `target`'s file in a tab beside the focused one, in whichever
    /// page can show it.
    ///
    /// A PDF is not text and the editor refuses it as binary, so it goes to
    /// the viewer instead. Everything else is the editor's.
    pub(crate) fn open_path_page(&mut self, target: OpenTarget) {
        if PdfPage::handles(&target.path) {
            self.show_page(PageKind::Pdf, Box::new(PdfPage::new(target.path)));
            return;
        }
        self.open_editor_page(target);
    }

    /// Open `target`'s file as editable text in a tab beside the focused one,
    /// so closing it returns to the tab the file was opened from.
    ///
    /// A file the editor will not open (binary, not UTF-8, too large to
    /// repaint) says so and stays closed; `Ctrl-O` hands those to `$EDITOR`.
    pub(crate) fn open_editor_page(&mut self, target: OpenTarget) {
        // A file opened from inside the editor joins it as another buffer of
        // its own, rather than opening a second editor beside it.
        let tab_id = self.layout().focused();
        if let Some(slot) = self.open_tabs.page_mut(&tab_id) {
            if slot.kind == PageKind::Editor && slot.page.open_file(target.clone()) {
                // Through the outcome, so a file the editor could not open
                // is reported now rather than at the next key.
                self.act_on_page_outcome(tab_id, PageOutcome::Consumed);
                return;
            }
        }
        match EditorPage::new(target.path, target.line) {
            Ok(page) => {
                self.show_page(PageKind::Editor, Box::new(page));
            }
            Err(e) => self.set_error(format!("{e}")),
        }
    }

    /// Open `path` in an editor tab of its own for a program blocked on it
    /// (`git commit` waiting on its message), answering `waiter` when the tab
    /// goes. Never joins an editor already up, since closing that one is not
    /// the end of this edit.
    pub(crate) fn open_waited_editor(&mut self, path: PathBuf, waiter: EditWaiter) {
        let start = self.focused_start_dir();
        self.prefer_other_pane();
        match EditorPage::new(path, None) {
            Ok(page) => {
                let with_changes = page.edits_commit();
                let editor =
                    self.show_page_beside(PageKind::Editor, Box::new(page.with_waiter(waiter)));
                if with_changes {
                    self.show_staged_beside(editor, start);
                }
            }
            Err(e) => self.set_error(format!("{e}")),
        }
    }

    /// Add what is staged in the repository containing `start` as a tab in
    /// `editor`'s pane, so the change a commit message describes is one tab
    /// away without a pane of its own. The editor stays the one showing, and
    /// the tab goes when the editor does.
    fn show_staged_beside(&mut self, editor: TabId, start: PathBuf) {
        let page = GitPage::staged_diff(start);
        let first = page.initial_request();
        let diff = self.show_page(PageKind::Git, Box::new(page));
        self.layout_mut().focus(editor);
        self.commit_companions.insert(editor, diff);
        self.jobs.spawn(diff, first);
        self.after_tab_change();
    }

    /// Open the file browser where the focused terminal points, else over its
    /// working directory (see [`Self::files_palette_at_point`]): the palette,
    /// listing a directory a row at a time. The chord that opened it closes
    /// it again, as a page's own chord does.
    pub(crate) fn open_file_browser(&mut self) {
        let showing = self
            .palette
            .as_ref()
            .is_some_and(|palette| palette.mode == PaletteMode::Files);
        self.palette = match showing {
            true => None,
            false => Some(self.files_palette_at_point()),
        };
        self.dirty = true;
    }

    /// The page tabs as a session saves them: each that can say where it was.
    pub(crate) fn page_sessions(&self) -> Vec<PageSession> {
        self.open_tabs
            .pages()
            .filter_map(|(id, slot)| {
                let at = slot.page.snapshot()?;
                Some(PageSession {
                    id: id.0 as usize,
                    kind: slot.kind.name().to_string(),
                    line: at.line,
                    path: at.path.to_string_lossy().to_string(),
                })
            })
            .collect()
    }

    /// Open a saved page tab again as tab `id`, outside the layout, which the
    /// restore rebuilds from the session. `false` for a page that cannot be
    /// opened there any more (a file since deleted, a kind no longer known).
    pub(crate) fn restore_page(&mut self, id: TabId, saved: &PageSession) -> bool {
        let path = PathBuf::from(&saved.path);
        // A view that knows nothing until git answers has to ask, as it does
        // when opened; restored without the request it would wait for ever.
        let mut first = None;
        let Some(kind) = PageKind::from_name(&saved.kind) else {
            return false;
        };
        let page: Box<dyn Page> = match kind {
            PageKind::Dir if path.is_dir() => Box::new(DirPage::new(path)),
            PageKind::Git if path.is_dir() => {
                let page = GitPage::new(path);
                first = Some(page.initial_request());
                Box::new(page)
            }
            PageKind::Grep if path.is_dir() => Box::new(GrepPage::new(path)),
            PageKind::Editor => match EditorPage::new(path, saved.line) {
                Ok(page) => Box::new(page),
                Err(_) => return false,
            },
            PageKind::Dir
            | PageKind::Git
            | PageKind::Grep
            | PageKind::Keys
            | PageKind::Pdf
            | PageKind::Proc
            | PageKind::Sys => return false,
        };
        self.open_tabs.insert_page(id, PageSlot::new(kind, page));
        if let Some(first) = first {
            self.jobs.spawn(id, first);
        }
        true
    }

    /// Open `page` in a new tab beside the focused one and show it. Returns
    /// the tab it went into.
    pub(crate) fn show_page(&mut self, kind: PageKind, page: Box<dyn Page>) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.insert_page_tab(tab_id, kind, page);
        tab_id
    }

    /// Put `page` into the layout as tab `tab_id`, beside the focused tab.
    fn insert_page_tab(&mut self, tab_id: TabId, kind: PageKind, page: Box<dyn Page>) {
        self.open_tabs
            .insert_page(tab_id, PageSlot::new(kind, page));
        self.layout_mut().add_tab(tab_id);
        self.after_tab_change();
    }

    /// Close the page tab `tab_id`, keeping its page for reopening.
    pub(crate) fn close_page(&mut self, tab_id: TabId) {
        self.close_tab_now(tab_id, Closing::Keep);
    }

    /// Ask before closing a pane whose page tabs hold edits on no disk, or
    /// whose terminal has a program running in it.
    ///
    /// Returns whether the question was asked, which is the caller's cue to
    /// wait for the answer rather than close now. A shell sitting at its
    /// prompt has nothing to lose, but a program in the way (an agent, an
    /// editor, a build) is ended with the pane.
    pub(crate) fn ask_before_closing_pane(&mut self, tab_id: TabId) -> bool {
        let tabs = self.layout().tabs_in_pane(tab_id);
        self.ask_before_closing(&tabs, "pane", PendingClose::Terminal(tab_id))
    }

    /// The same, for every pane but the one holding `keep`.
    pub(crate) fn ask_before_closing_others(&mut self, keep: TabId) -> bool {
        let kept = self.layout().tabs_in_pane(keep);
        let others: Vec<TabId> = self
            .layout()
            .members()
            .into_iter()
            .filter(|pane| !kept.contains(pane))
            .collect();
        self.ask_before_closing(&others, "other panes", PendingClose::OtherPanes(keep))
    }

    /// The same, for every tab in `keep`'s pane but `keep`.
    pub(crate) fn ask_before_closing_other_tabs(&mut self, keep: TabId) -> bool {
        let others: Vec<TabId> = self
            .layout()
            .tabs_in_pane(keep)
            .into_iter()
            .filter(|tab| *tab != keep)
            .collect();
        self.ask_before_closing(&others, "other tabs", PendingClose::OtherTabs(keep))
    }

    /// The same, for one tab.
    pub(crate) fn ask_before_closing_tab(&mut self, tab_id: TabId) -> bool {
        self.ask_before_closing(&[tab_id], "tab", PendingClose::Tab(tab_id))
    }

    /// The same, for the whole window.
    pub(crate) fn ask_before_quitting(&mut self) -> bool {
        let everything = self.layout().members();
        self.ask_before_closing(&everything, "window", PendingClose::Quit)
    }

    /// Put the question, if there is anything to ask about.
    fn ask_before_closing(&mut self, panes: &[TabId], what: &str, pending: PendingClose) -> bool {
        let unsaved = self.unsaved_in(panes);
        let running = self.running_in(panes);
        if unsaved.is_empty() && running.is_empty() {
            return false;
        }
        let mut reasons = Vec::new();
        if !unsaved.is_empty() {
            reasons.push(format!("unsaved edits in {}", named(&unsaved)));
        }
        if !running.is_empty() {
            reasons.push(format!("{} still running", named(&running)));
        }
        // The pane the dialog belongs to, which is only where it is drawn:
        // the question is the host's and the host answers it.
        let asked_of = panes
            .first()
            .copied()
            .unwrap_or_else(|| self.layout().focused());
        self.pending_close = Some(pending);
        self.page_prompt = Some(ActivePrompt {
            field: TextField::line(""),
            recall: Recall::default(),
            tab: asked_of,
            request: PromptRequest {
                initial: String::new(),
                label: format!("{}. Close the {what}? (y/n) ", reasons.join("; ")),
                mode: PromptMode::Confirm,
                tag: ASK_CLOSE_UNSAVED,
            },
        });
        self.dirty = true;
        true
    }

    /// The program running in each terminal among `panes` that has one beyond
    /// its shell, named as the system names it.
    fn running_in(&self, panes: &[TabId]) -> Vec<String> {
        panes
            .iter()
            .filter_map(|pane| self.open_tabs.terminal(pane))
            .filter(|terminal| terminal.has_foreground_process())
            .map(|terminal| {
                terminal
                    .foreground_process_name()
                    .unwrap_or_else(|| RUNNING_PROGRAM.to_string())
            })
            .collect()
    }

    /// What every page tab among `panes` is holding that is on no disk,
    /// named as the pages name themselves.
    fn unsaved_in(&self, panes: &[TabId]) -> Vec<String> {
        panes
            .iter()
            .filter_map(|pane| self.open_tabs.page(pane))
            .filter(|slot| slot.page.is_dirty())
            .map(|slot| slot.page.title())
            .collect()
    }

    /// Close what the question was about, or nothing when the answer was not
    /// yes. The pages go to the stash either way once the close goes ahead,
    /// so a yes here is still not the last word on those edits.
    fn answer_close_prompt(&mut self, pending: PendingClose, answer: Option<String>) {
        if answer.is_none() {
            return;
        }
        match pending {
            PendingClose::OtherPanes(keep) => self.close_other_panes_now(keep),
            PendingClose::OtherTabs(keep) => self.close_other_tabs_now(keep),
            PendingClose::Terminal(pane) => self.close_pane_now(pane),
            PendingClose::Tab(pane) => self.close_tab_now(pane, Closing::Keep),
            PendingClose::Quit => self.exit_requested = true,
        }
        // Closing the last tab quits, and the answer just given was the one
        // that question needed.
        if self.exit_requested {
            self.quit_confirmed = true;
        }
    }

    /// Keep a closed page for reopening, and say once a session that closing
    /// one is not final.
    ///
    /// An older entry naming the same thing is dropped: a page opened and
    /// closed over and over would otherwise fill the list with rows that are
    /// all the same page as far as a reader can tell. One holding unwritten
    /// edits is never the one dropped, however old it is.
    pub(crate) fn stash_closed_page(&mut self, tab: TabId, mut slot: PageSlot) -> u64 {
        slot.page.on_closed();
        let kind = slot.kind;
        let closed = ClosedPage {
            closed_at: now_seconds(),
            id: self.take_closed_id(),
            page: slot.page,
            tab,
            kind,
        };
        let label = closed.label();
        self.closed.retain(|other| match other {
            Closed::Page(page) => page.page.is_dirty() || page.label() != label,
            Closed::Terminal(_) => true,
        });
        let id = closed.id;
        self.closed.push(Closed::Page(closed));
        self.trim_closed();
        self.hint_at_reopening(kind);
        id
    }

    /// Keep closed terminal tabs: where they sat, what they were running in,
    /// and which page tabs went with them. `whole_pane` says the whole pane went.
    pub(crate) fn stash_closed_pane(
        &mut self,
        terminals: &[TabId],
        layout: LayoutTree,
        pages: Vec<u64>,
        whole_pane: bool,
    ) {
        let terminals = terminals
            .iter()
            .map(|&tab| ClosedTerminal {
                cwd: self.open_tabs.terminal(&tab).and_then(|p| p.cwd()),
                tab,
            })
            .collect();
        let id = self.take_closed_id();
        self.closed.push(Closed::Terminal(ClosedPane {
            closed_at: now_seconds(),
            whole_pane,
            id,
            layout,
            pages,
            terminals,
        }));
        self.trim_closed();
    }

    /// The id the next closed thing goes in under.
    fn take_closed_id(&mut self) -> u64 {
        self.next_closed_id += 1;
        self.next_closed_id
    }

    /// Drop what the stash no longer has room for: the oldest entry a reader
    /// could lose nothing by dropping, and only past a hard ceiling the
    /// oldest entry of all, however much it is holding.
    fn trim_closed(&mut self) {
        while self.closed.len() > MAX_CLOSED {
            let Some(at) = self.closed.iter().position(|closed| !closed.is_dirty()) else {
                break;
            };
            self.closed.remove(at);
        }
        while self.closed.len() > MAX_CLOSED_UNSAVED {
            self.closed.remove(0);
        }
    }

    /// Say, once a session, that what was just closed can be brought back,
    /// naming the key that does it. A stash nobody knows about is no way back
    /// at all, and a key named where it is not bound would be worse.
    fn hint_at_reopening(&mut self, kind: PageKind) {
        if self.reopen_hint_shown {
            return;
        }
        let chord = self.window_keymap.chord_hint(REOPEN_COMMAND);
        if chord.is_empty() {
            return;
        }
        self.reopen_hint_shown = true;
        self.set_notice(format!("{} closed. {chord} puts it back", kind.name()));
    }

    /// Put the last closed thing back: a page tab beside the focused one, or
    /// terminal tabs where they sat when the layout still has room for them.
    pub(crate) fn reopen_last_closed(&mut self) {
        let Some(closed) = self.closed.pop() else {
            self.set_notice(NOTHING_CLOSED);
            return;
        };
        self.restore_closed(closed);
    }

    /// Put back the closed thing a row of the list stands for.
    ///
    /// By id rather than by position: the list is drawn from a stash that a
    /// pane dying behind it can still push onto, and a row that moved must
    /// not come back as whatever took its place.
    pub(crate) fn reopen_closed_by_id(&mut self, id: u64) {
        let Some(at) = self.closed.iter().position(|closed| closed.id() == id) else {
            self.set_notice(GONE_FROM_LIST);
            return;
        };
        let closed = self.closed.remove(at);
        self.restore_closed(closed);
    }

    /// Act on a row of the tab list: go to a page that is open somewhere, or
    /// put back what was closed.
    pub(crate) fn choose_tab_row(&mut self, action: &str) {
        if let Some(tab) = action
            .strip_prefix(OPEN_ROW)
            .and_then(|id| id.parse::<u64>().ok())
        {
            self.switch_to_tab(TabId(tab));
            return;
        }
        if let Some(id) = action
            .strip_prefix(CLOSED_ROW)
            .and_then(|id| id.parse::<u64>().ok())
        {
            self.reopen_closed_by_id(id);
        }
    }

    /// Show every page tab as a list to pick from: the ones open somewhere
    /// first, then what was closed recently, newest first.
    pub(crate) fn open_tab_list(&mut self) {
        let rows = self.tab_rows();
        if rows.is_empty() {
            self.set_notice(NOTHING_CLOSED);
            return;
        }
        self.palette = Some(Palette::open_tab_list(rows));
        self.dirty = true;
    }

    /// Every row of the tab list, as `(action, label, hint)`.
    fn tab_rows(&self) -> Vec<(String, String, String)> {
        let mut rows: Vec<(String, String, String)> = Vec::new();
        let shown = self.layout().shown_tabs();
        for tab in self.layout().members() {
            let Some(slot) = self.open_tabs.page(&tab) else {
                continue;
            };
            let hint = match shown.contains(&tab) {
                true => SHOWING_HINT,
                false => OPEN_HINT,
            };
            rows.push((
                format!("{OPEN_ROW}{}", tab.0),
                tab_row_label(slot.kind, slot.page.as_ref()),
                hint.to_string(),
            ));
        }

        let now = now_seconds();
        for closed in self.closed.iter().rev() {
            let age = format!("closed {} ago", reltime::ago(closed.closed_at(), now));
            let label = match closed {
                Closed::Page(page) => page.label(),
                Closed::Terminal(pane) => self.closed_pane_label(pane),
            };
            rows.push((format!("{CLOSED_ROW}{}", closed.id()), label, age));
        }
        rows
    }

    /// How the list names closed terminals: where their shells were, and the
    /// pages that went with them, so a row reads as more than "a pane".
    fn closed_pane_label(&self, pane: &ClosedPane) -> String {
        let what = match pane.whole_pane {
            true => CLOSED_PANE_NOUN,
            false => CLOSED_TERMINAL_NOUN,
        };
        let cwds: Vec<&str> = pane
            .terminals
            .iter()
            .filter_map(|terminal| terminal.cwd.as_deref())
            .collect();
        let named = match cwds.is_empty() {
            true => what.to_string(),
            false => format!("{what}: {}", cwds.join(", ")),
        };
        let pages: Vec<&str> = pane
            .pages
            .iter()
            .filter_map(|id| self.closed.iter().find(|closed| closed.id() == *id))
            .filter_map(|closed| match closed {
                Closed::Page(page) => Some(page.kind.name()),
                Closed::Terminal(_) => None,
            })
            .collect();
        match pages.is_empty() {
            true => named,
            false => format!("{named}  ({})", pages.join(", ")),
        }
    }

    /// Put back whatever was closed, page or pane.
    fn restore_closed(&mut self, closed: Closed) {
        match closed {
            Closed::Page(page) => self.restore_closed_page(page),
            Closed::Terminal(pane) => self.restore_closed_pane(pane),
        }
    }

    /// The pages a restored pane was holding, taken out of the stash so that
    /// nothing is put back twice. Any that has since been reopened on its own
    /// is simply not there, and the pane comes back without it.
    pub(super) fn take_closed_pages(&mut self, ids: &[u64]) -> Vec<ClosedPage> {
        let mut taken = Vec::new();
        for id in ids {
            let found = self.closed.iter().position(|closed| match closed {
                Closed::Page(page) => page.id == *id,
                Closed::Terminal(_) => false,
            });
            if let Some(at) = found {
                if let Closed::Page(page) = self.closed.remove(at) {
                    taken.push(page);
                }
            }
        }
        taken
    }

    /// Put a closed page back as a tab beside the focused one, re-reading
    /// what it was showing on the way back. It keeps the tab id it had when
    /// that id is free, so a layout snapshot naming it still finds it.
    pub(super) fn restore_closed_page(&mut self, mut closed: ClosedPage) {
        // What it was showing may have changed while it was gone.
        let outcome = closed.page.on_resume();
        let tab_id = match self.layout().contains(closed.tab) {
            true => self.alloc_tab_id(),
            false => closed.tab,
        };
        self.insert_page_tab(tab_id, closed.kind, closed.page);
        self.act_on_page_outcome(tab_id, outcome);
    }

    /// Step between a page and where the reader came from with the page's own
    /// chord: on the page's tab, go back to the tab shown before it; with the
    /// page open elsewhere in the focused pane, go to it. Returns whether the
    /// chord was spent that way, else the caller opens the page.
    fn toggle_page(&mut self, kind: PageKind) -> bool {
        self.toggle_page_matching(kind, &|_| true)
    }

    /// Step between a `kind` page matching `predicate` and where the reader
    /// came from: on that page's tab, go back to the tab shown before it; with
    /// it open elsewhere in the focused pane, go to it. Returns whether the
    /// chord was spent.
    fn toggle_page_matching(
        &mut self,
        kind: PageKind,
        predicate: &dyn Fn(&dyn Page) -> bool,
    ) -> bool {
        let focused = self.layout().focused();
        let matches = |app: &App, tab: &TabId| {
            app.open_tabs
                .page(tab)
                .is_some_and(|slot| slot.kind == kind && predicate(&*slot.page))
        };
        if matches(self, &focused) {
            if let Some(&back) = self.layout().recent_in_pane(focused).get(1) {
                self.show_tab(back);
            }
            return true;
        }
        let open = self
            .layout()
            .recent_in_pane(focused)
            .into_iter()
            .find(|tab| matches(self, tab));
        match open {
            Some(tab) => {
                self.show_tab(tab);
                true
            }
            None => false,
        }
    }

    /// Offer `key` to the page covering `tab_id`. Returns whether the key was
    /// spent, so a key the page declines still reaches the ordinary keymap.
    pub(crate) fn offer_key_to_page(&mut self, tab_id: TabId, key: &Key) -> bool {
        // Wrap belongs to the pane, not the page: every page gets the toggle,
        // ahead of its own keys, so no page has to claim it itself.
        if key.alt && !key.ctrl && matches!(key.code, KeyCode::Char('z') | KeyCode::Char('Z')) {
            self.page_wrap = !self.page_wrap;
            self.dirty = true;
            return true;
        }
        let neighbors: Vec<PathBuf> = self
            .layout()
            .shown_tabs()
            .into_iter()
            .filter(|pane| *pane != tab_id)
            .filter_map(|pane| {
                self.open_tabs
                    .page(&pane)
                    .and_then(|slot| slot.page.root_dir())
            })
            .collect();
        let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
            return false;
        };
        slot.page.set_neighbors(neighbors);
        let outcome = slot.page.on_key(&key.chorded());
        self.act_on_page_outcome(tab_id, outcome)
    }

    /// Route a key a page surface handed back, because the WebView holding
    /// the keyboard is the only thing that saw it.
    ///
    /// The page is offered it first, exactly as a key typed into any other
    /// page is, and what the page declines is resolved against the window
    /// keymap, so the chords that split, zoom, and switch panes and tabs keep
    /// working from inside a document.
    ///
    /// The terminal's own encoding settings are left at their defaults: a
    /// pane showing a page never forwards a key to its PTY, so nothing here
    /// can reach the encoder they belong to.
    pub(crate) fn route_surface_key(&mut self, tab_id: TabId, key: Key) {
        if self.offer_key_to_page(tab_id, &key) {
            self.dirty = true;
            return;
        }
        let mode = self.open_tabs.mode(&tab_id).unwrap_or_default();
        let action = input::resolve_with(
            mode,
            &key,
            &mut self.pending,
            &self.window_keymap,
            0,
            None,
            false,
        );
        self.handle_action(action, tab_id);
        self.dirty = true;
    }

    /// Offer a pointer press or drag to the page covering `tab_id`, as the
    /// row and column of the pane it landed on. Returns whether the page took
    /// it, so a click it declines still reaches the pane underneath.
    pub(crate) fn offer_mouse_to_page(&mut self, tab_id: TabId, at: PagePoint) -> bool {
        let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.page.on_mouse(at);
        self.act_on_page_outcome(tab_id, outcome)
    }

    /// Offer a turn of the wheel to the page covering `tab_id`.
    pub(crate) fn offer_scroll_to_page(&mut self, tab_id: TabId, lines: isize) -> bool {
        let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.page.on_scroll(lines);
        self.act_on_page_outcome(tab_id, outcome)
    }

    /// Route one key into the open prompt: `Enter` answers, `Esc` cancels, and
    /// a confirm prompt resolves on the first key it sees.
    pub(crate) fn handle_prompt_key(&mut self, key: &Key) {
        let key = &key.typed();
        let Some(prompt) = self.page_prompt.as_mut() else {
            return;
        };
        let answer = match (prompt.request.mode, key.code) {
            (_, KeyCode::Escape) => Some(None),
            (PromptMode::Confirm, KeyCode::Char(c)) => {
                Some((c == 'y' || c == 'Y').then(|| c.to_string()))
            }
            (PromptMode::Confirm, _) => None,
            // Up and Down walk back through what this question was answered
            // with before, as a shell's history does.
            (PromptMode::Text, _) if recall_step(key).is_some() => {
                let answers = self.prompt_answers.of(prompt.request.tag);
                let typed = prompt.field.text().to_string();
                let recalled = match recall_step(key) {
                    Some(true) => prompt.recall.older(answers, &typed),
                    _ => prompt.recall.newer(answers),
                };
                if let Some(text) = recalled {
                    prompt.field.set_text(&text);
                    self.prompt_debounce = Some((
                        std::time::Instant::now()
                            + std::time::Duration::from_millis(super::SEARCH_DEBOUNCE_MILLIS),
                        prompt.tab,
                    ));
                }
                None
            }
            // The answer is a line of text like any other, so it takes the
            // same Emacs keys a file does while typing.
            (PromptMode::Text, _) => match prompt.field.on_key(key, 1) {
                EditOutcome::Submit => {
                    self.prompt_debounce = None;
                    let answer = prompt.field.text().to_string();
                    self.prompt_answers.record(prompt.request.tag, &answer);
                    Some(Some(answer))
                }
                EditOutcome::Cancel => {
                    self.prompt_debounce = None;
                    Some(None)
                }
                outcome => {
                    if outcome != EditOutcome::Ignored {
                        self.prompt_debounce = Some((
                            std::time::Instant::now()
                                + std::time::Duration::from_millis(super::SEARCH_DEBOUNCE_MILLIS),
                            prompt.tab,
                        ));
                    }
                    None
                }
            },
        };
        self.dirty = true;
        if answer.is_none() {
            if let Some(prompt) = self.page_prompt.as_ref() {
                if prompt.request.tag == "search" || prompt.request.tag == "search-back" {
                    let tab = prompt.tab;
                    self.update_search_preview(tab);
                }
            }
        }
        let Some(answer) = answer else {
            return;
        };
        self.search.match_index = 0;
        self.search.match_total = 0;
        self.search.current = None;
        self.prompt_debounce = None;
        let Some(prompt) = self.page_prompt.take() else {
            return;
        };
        // A question the host asked is the host's to act on: no page asked
        // it, and no page is waiting to hear the answer.
        if let Some(pending) = self.pending_close.take() {
            self.answer_close_prompt(pending, answer);
            return;
        }
        let tab_id = prompt.tab;
        let reply = PromptReply {
            answer,
            tag: prompt.request.tag,
        };
        let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
            return;
        };
        let outcome = slot.page.on_prompt(reply);
        self.act_on_page_outcome(tab_id, outcome);
    }

    /// Hand a page the choice made from the list it asked for, as the answer
    /// to the question it asked. A palette closed without choosing answers
    /// with nothing, the way an escaped prompt does.
    pub(crate) fn answer_page_pick(&mut self, choice: Option<String>) {
        let Some(pick) = self.page_pick.take() else {
            return;
        };
        let Some(slot) = self.open_tabs.page_mut(&pick.tab) else {
            return;
        };
        let outcome = slot.page.on_prompt(PromptReply {
            answer: choice,
            tag: pick.tag,
        });
        self.act_on_page_outcome(pick.tab, outcome);
    }

    /// What the open prompt shows: its question, then what has been typed.
    #[cfg(test)]
    pub(crate) fn input_view(&self) -> Option<InputView> {
        let prompt = self.page_prompt.as_ref()?;
        Some(input_dialog(
            &prompt.request.label,
            prompt.field.text(),
            prompt.field.col(),
            prompt.request.mode,
        ))
    }

    /// Carry out what a page asked for. Returns whether the input that produced
    /// it was spent.
    pub(super) fn act_on_page_outcome(&mut self, tab_id: TabId, outcome: PageOutcome) -> bool {
        let notice = self
            .open_tabs
            .page_mut(&tab_id)
            .and_then(|slot| slot.page.take_notice());
        if let Some(notice) = notice {
            self.show_notice(notice.text, notice.kind);
        }
        match outcome {
            PageOutcome::Close => {
                self.close_page(tab_id);
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
                self.open_path_page(target);
                true
            }
            PageOutcome::SpawnEditor(target) => {
                self.open_file_in_new_tab(target.path, target.line);
                true
            }
            PageOutcome::CancelJobs => {
                self.jobs.cancel_for(tab_id);
                self.dirty = true;
                true
            }
            PageOutcome::Job(request) => {
                self.jobs.spawn(tab_id, request);
                true
            }
            PageOutcome::Spawn(request) => {
                self.spawn_in_tab(request);
                true
            }
            PageOutcome::Paste => {
                let text = self.clipboard_text().unwrap_or_default();
                let Some(slot) = self.open_tabs.page_mut(&tab_id) else {
                    return true;
                };
                let outcome = slot.page.on_paste(text);
                self.act_on_page_outcome(tab_id, outcome)
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
                    field: TextField::line(&request.initial),
                    recall: Recall::default(),
                    tab: tab_id,
                    request,
                });
                self.dirty = true;
                true
            }
            PageOutcome::Pick(request) => {
                // The palette is the list picker Winter already has: what a
                // page adds is where the choice goes when it is made.
                self.page_pick = Some(ActivePick {
                    label: request.label,
                    tab: tab_id,
                    tag: request.tag,
                });
                self.palette = Some(Palette::open_pick(request.items));
                self.dirty = true;
                true
            }
            PageOutcome::RunAction(action) => {
                // The page is done once it has named the command, and the
                // command runs against the tab shown in its place.
                self.close_page(tab_id);
                let focused = self.layout().focused();
                self.run_command(&action, focused);
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
    use crate::app::SPLIT_RATIO;
    use crate::model::input::KeyCode;
    use crate::model::layout::Direction;
    use crate::model::mode::Mode;
    use crate::model::page::{PageContent, PageNotice, PageSpan};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;

    /// A page that counts the keys it claims, so a test can tell a key the
    /// page spent from one it handed back.
    #[derive(Default)]
    struct CountingPage {
        claimed: usize,
    }

    impl Page for CountingPage {
        /// The count is in the title so a test can tell one instance from
        /// another: a page that comes back holding what it held is not the
        /// same thing as a fresh page of its kind.
        fn title(&self) -> String {
            format!("Counting {}", self.claimed)
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
                // The chord the Git page commits on, which the window also
                // binds to copy.
                // Matched on the lowercase letter alone: the window hands over
                // the capital, and the page must not have to know.
                KeyCode::Char('c') if key.ctrl && key.shift => {
                    self.claimed += 1;
                    PageOutcome::Consumed
                }
                KeyCode::Char('r') => PageOutcome::RunAction("new_tab".to_string()),
                KeyCode::Char('q') => PageOutcome::Close,
                _ => PageOutcome::Ignored,
            }
        }
    }

    /// A page with one report queued for the status bar.
    struct ReportingPage(Option<PageNotice>);

    impl Page for ReportingPage {
        fn title(&self) -> String {
            "Reporting".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PageContent {
            PageContent::new(vec![vec![PageSpan::plain("reporting")]])
        }

        fn on_key(&mut self, _key: &Key) -> PageOutcome {
            PageOutcome::Consumed
        }

        fn take_notice(&mut self) -> Option<PageNotice> {
            self.0.take()
        }
    }

    /// A page holding edits that are on no disk, which is the one thing
    /// closing a pane cannot give back on its own.
    struct UnsavedPage(&'static str);

    impl Page for UnsavedPage {
        fn title(&self) -> String {
            self.0.to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PageContent {
            PageContent::new(vec![vec![PageSpan::plain("unsaved")]])
        }

        fn on_key(&mut self, _key: &Key) -> PageOutcome {
            PageOutcome::Ignored
        }

        fn is_dirty(&self) -> bool {
            true
        }
    }

    /// A page that reports `dir` as the place it is looking at.
    struct LocatedPage(PathBuf);

    impl Page for LocatedPage {
        fn title(&self) -> String {
            "Located".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PageContent {
            PageContent::new(vec![vec![PageSpan::plain("located")]])
        }

        fn on_key(&mut self, _key: &Key) -> PageOutcome {
            PageOutcome::Ignored
        }

        fn cwd(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    #[test]
    fn test_a_page_opens_where_the_page_is_looking_not_where_the_shell_is() {
        // Opening the file browser while reading a file used to start at the
        // shell's directory, which for a file opened from elsewhere is not
        // even the same tree. The page tab being looked at answers first.
        let mut app = App::new();
        let looking_at = PathBuf::from("/tmp/winter-test-somewhere-else");
        app.show_page(PageKind::Dir, Box::new(LocatedPage(looking_at.clone())));
        assert_eq!(app.focused_start_dir(), looking_at);
    }

    #[test]
    fn test_a_page_tab_with_no_place_of_its_own_starts_where_its_shell_is() {
        // A page that says nothing about where it is looking defers to the
        // terminal it was opened from, so a tab opened from it does not land
        // in the process's own directory.
        let mut app = App::new();
        let from_shell = app.focused_start_dir();
        app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        assert_eq!(app.focused_start_dir(), from_shell);
    }

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn app_with_open_page() -> (App, TabId) {
        let mut app = App::new();
        let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        (app, tab)
    }

    #[test]
    fn test_a_page_opens_as_a_tab_beside_the_terminal_without_splitting() {
        let mut app = App::new();
        let terminal = app.layout().focused();
        let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        assert_ne!(tab, terminal, "a tab of its own");
        assert_eq!(
            app.layout().shown_tabs(),
            vec![tab],
            "showing in the same pane"
        );
        assert_eq!(app.layout().members(), vec![terminal, tab]);
        assert!(app.open_tabs.is_page(&tab));
    }

    #[test]
    fn test_opening_a_file_opens_an_editor_tab_and_a_refused_one_opens_nothing() {
        // The host side of every `Enter` on a file: the editor has to end up
        // in a tab, and a file it will not open has to leave the pane showing
        // whatever it was showing rather than an empty editor.
        let dir = std::env::temp_dir().join(format!("winter-open-editor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let text = dir.join("a.txt");
        std::fs::write(&text, "one\ntwo\n").expect("temp file");
        let binary = dir.join("a.bin");
        std::fs::write(&binary, [0x00, 0x01]).expect("temp file");

        let mut app = App::new();
        let terminal = app.layout().focused();
        app.open_editor_page(OpenTarget::at_line(text, 2));
        let editor = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&editor).map(|slot| slot.kind),
            Some(PageKind::Editor)
        );

        app.close_page(editor);
        assert_eq!(app.layout().focused(), terminal);
        app.open_editor_page(OpenTarget::file(binary));
        assert!(app.open_tabs.pages().next().is_none(), "nothing was opened");
        assert_eq!(app.layout().focused(), terminal);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_closing_a_tab_opened_from_another_goes_back_to_it() {
        // Opening a file from a listing and closing it again has to land back
        // on the listing, not on whichever tab sits next to it on the strip.
        let mut app = App::new();
        let listing = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        app.layout_mut().focus(TabId(0));
        let other = app.show_page(PageKind::Proc, Box::new(CountingPage::default()));
        app.show_tab(listing);
        let file = app.show_page(PageKind::Sys, Box::new(CountingPage::default()));
        assert_eq!(
            app.layout().members(),
            vec![TabId(0), other, listing, file],
            "the file opens right beside the listing"
        );

        app.close_page(file);
        assert_eq!(app.layout().focused(), listing);
        assert!(
            app.open_tabs.is_page(&listing),
            "and the listing is as it was"
        );
    }

    #[test]
    fn test_closing_a_page_leaves_the_terminal_as_it_was() {
        // The terminal tab kept running beside the page, so it has to come
        // back in the mode it was in.
        let mut app = App::new();
        let terminal = app.layout().focused();
        let shell = crate::terminal::session::Terminal::with_command(
            20,
            5,
            portable_pty::CommandBuilder::new("true"),
            1000,
        )
        .expect("test terminal spawn");
        app.open_tabs.insert_terminal(terminal, shell);
        app.open_tabs.set_mode(&terminal, Mode::Normal);
        let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));

        app.close_page(tab);
        assert!(app.open_tabs.pages().next().is_none());
        assert_eq!(app.layout().members(), vec![terminal]);
        assert_eq!(app.open_tabs.mode(&terminal), Some(Mode::Normal));
        assert!(!app.open_tabs.is_page(&tab), "the page's tab is forgotten");
    }

    /// Put `claimed` keys into the page covering `pane`, which is what makes
    /// one open page tell itself apart from another.
    fn tally(app: &mut App, tab: TabId, claimed: usize) {
        for _ in 0..claimed {
            if let Some(slot) = app.open_tabs.page_mut(&tab) {
                slot.page.on_key(&press(KeyCode::Char('x')));
            }
        }
    }

    /// What the focused pane's page calls itself, or nothing when no page
    /// covers it.
    fn showing(app: &App) -> Option<String> {
        app.open_tabs
            .page(&app.layout().focused())
            .map(|slot| slot.page.title())
    }

    #[test]
    fn test_a_closed_page_comes_back_the_page_it_was() {
        // The whole point of reopening: what was lost was the state, not the
        // kind of page. A rebuilt page of the same kind would come back at
        // zero, which is the bug this catches.
        let (mut app, pane) = app_with_open_page();
        tally(&mut app, pane, 3);
        app.close_page(pane);
        assert!(app.open_tabs.pages().next().is_none(), "closed first");

        app.reopen_last_closed();
        assert_eq!(app.layout().focused(), pane, "under the tab id it had");
        assert_eq!(
            app.open_tabs.page(&pane).map(|slot| slot.kind),
            Some(PageKind::Grep),
            "the page it was opened as"
        );
        assert_eq!(showing(&app).as_deref(), Some("Counting 3"));
        assert!(app.open_tabs.is_page(&pane));
        assert!(app.closed.is_empty(), "and is spent by reopening");
    }

    #[test]
    fn test_a_closed_pane_comes_back_with_the_pages_it_was_holding() {
        // Merging a split away used to drop the pane and its pages both. One
        // reopen puts the split back, in its place, with every tab it held.
        let mut app = App::new();
        let terminal = app.layout().focused();
        let listing = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        tally(&mut app, listing, 1);
        let file = app.show_page(PageKind::Sys, Box::new(CountingPage::default()));
        tally(&mut app, file, 2);

        let sibling = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);
        app.close_pane(file);
        assert!(app.open_tabs.pages().next().is_none());
        assert_eq!(
            app.layout().members(),
            vec![sibling],
            "the split was merged away"
        );

        app.reopen_last_closed();
        assert_eq!(app.layout().shown_tabs().len(), 2, "the split is back");
        assert_eq!(
            app.layout().members(),
            vec![terminal, listing, file, sibling],
            "in the place it was, with its tabs in their order"
        );
        assert_eq!(
            app.open_tabs.page(&file).unwrap().page.title(),
            "Counting 2"
        );
        assert_eq!(
            app.open_tabs.page(&listing).unwrap().page.title(),
            "Counting 1"
        );
        assert!(app.closed.is_empty(), "and nothing left over to reopen");
    }

    #[test]
    fn test_a_page_chord_steps_between_the_page_and_the_tab_before_it() {
        // The chord that opened a page takes the reader back to where they
        // were, and then to the page again, which keeps what it was showing
        // rather than starting over.
        let mut app = App::new();
        let terminal = app.layout().focused();
        app.open_keys_page();
        let keys = app.layout().focused();
        assert_ne!(keys, terminal);
        app.open_keys_page();
        assert_eq!(
            app.layout().focused(),
            terminal,
            "back where it was opened from"
        );
        app.open_keys_page();
        assert_eq!(app.layout().focused(), keys, "and to the same tab again");
        assert_eq!(app.open_tabs.pages().count(), 1, "never a second one");
        assert!(app.closed.is_empty(), "nothing was closed");
    }

    #[test]
    fn test_closing_the_same_page_twice_leaves_one_row_for_it() {
        // Opening and closing a listing over and over would otherwise fill
        // the list with rows a reader cannot tell apart, and push the page
        // they actually lost off the end of it.
        let mut app = App::new();
        for _ in 0..3 {
            let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
            app.close_page(tab);
        }
        assert_eq!(app.closed.len(), 1);
    }

    #[test]
    fn test_a_page_holding_unsaved_edits_is_never_the_one_dropped() {
        // The cap exists so the stash stays a way back from an accident. A
        // page holding the only copy of an edit is the one entry dropping
        // which would make it one.
        let mut app = App::new();
        let tab = app.show_page(PageKind::Editor, Box::new(UnsavedPage("main.rs")));
        app.close_page(tab);
        for claimed in 1..=MAX_CLOSED + 2 {
            let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
            tally(&mut app, tab, claimed);
            app.close_page(tab);
        }
        assert!(
            app.closed.iter().any(|closed| closed.is_dirty()),
            "the unsaved one outlived the cap"
        );
        assert_eq!(app.closed.len(), MAX_CLOSED, "with the cap still held");
    }

    #[test]
    fn test_closing_a_pane_over_unsaved_edits_asks_first() {
        // Closing a pane never went near the page's own close, so an editor
        // holding unwritten edits was merged away without a word.
        let mut app = App::new();
        let tab = app.show_page(PageKind::Editor, Box::new(UnsavedPage("main.rs")));
        let sibling = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);

        app.close_pane(tab);
        assert_eq!(app.layout().shown_tabs().len(), 2, "nothing closed yet");
        let asked = app.input_view().expect("the question is up");
        assert!(
            asked.label.contains("main.rs"),
            "naming it: {}",
            asked.label
        );

        // Anything but yes leaves the pane exactly as it was.
        app.handle_prompt_key(&press(KeyCode::Char('n')));
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert!(app.open_tabs.is_page(&tab), "the editor is still there");

        app.close_pane(tab);
        app.handle_prompt_key(&press(KeyCode::Char('y')));
        assert_eq!(
            app.layout().shown_tabs(),
            vec![sibling],
            "and yes closes it"
        );
        assert!(
            app.closed.iter().any(|closed| closed.is_dirty()),
            "with the edits kept for reopening even so"
        );
    }

    #[test]
    fn test_the_first_close_of_a_session_says_how_to_undo_it() {
        // A way back nobody is told about is a way back nobody takes.
        let mut app = App::new();
        let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        app.close_page(tab);
        let said = app
            .notice
            .as_ref()
            .map(|(text, _, _)| text.clone())
            .expect("the first close says so");
        assert!(said.contains("Ctrl-Shift-u"), "naming the key: {said}");

        app.notice = None;
        let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        app.close_page(tab);
        assert!(app.notice.is_none(), "and only says it once");
    }

    #[test]
    fn test_the_tab_list_reaches_a_page_that_is_open_elsewhere() {
        // Half the point of one list: a page already up in another pane is
        // gone to rather than opened a second time.
        let mut app = App::new();
        let here = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        let sibling = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);
        app.layout_mut().focus(sibling);

        app.open_tab_list();
        let palette = app.palette.clone().expect("the list opens");
        let row = palette
            .entries
            .iter()
            .find(|entry| entry.label.starts_with("grep"))
            .expect("the open page is listed");
        assert_eq!(row.action, format!("open:{}", here.0));
        app.choose_tab_row(&row.action);
        assert_eq!(app.layout().focused(), here, "focus moved to it");
        assert!(app.closed.is_empty(), "and nothing was reopened");
    }

    #[test]
    fn test_the_stash_holds_the_last_few_closes_and_no_more() {
        // Each entry is a whole page, so the stash is a way back from an
        // accident rather than a log of every page ever closed.
        let mut app = App::new();
        for claimed in 0..MAX_CLOSED + 2 {
            let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
            tally(&mut app, tab, claimed);
            app.close_page(tab);
        }
        assert_eq!(app.closed.len(), MAX_CLOSED);

        app.reopen_last_closed();
        assert_eq!(
            showing(&app).as_deref(),
            Some(&format!("Counting {}", MAX_CLOSED + 1)[..]),
            "the last one closed is the first one back"
        );
        for _ in 0..MAX_CLOSED {
            app.reopen_last_closed();
        }
        assert_eq!(
            showing(&app).as_deref(),
            Some("Counting 2"),
            "and the two oldest were dropped rather than kept"
        );
    }

    #[test]
    fn test_the_list_of_closed_tabs_reads_newest_first() {
        // The order is the whole point of a recency list, and a row has to say
        // which page and what it was showing: "counting" twice over would be
        // two rows a reader cannot choose between.
        let mut app = App::new();
        for claimed in 1..=2 {
            let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
            tally(&mut app, tab, claimed);
            app.close_page(tab);
        }

        app.open_tab_list();
        let palette = app.palette.as_ref().expect("the list opens");
        assert_eq!(palette.mode, PaletteMode::TabList);
        let labels: Vec<&str> = palette
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["grep: Counting 2", "grep: Counting 1"]);
        assert!(
            palette
                .entries
                .iter()
                .all(|entry| entry.shortcut.ends_with("ago")),
            "every row says how long ago it was closed: {:?}",
            palette.entries
        );
    }

    #[test]
    fn test_picking_a_row_reopens_that_page_and_leaves_the_rest_closed() {
        // Reopening the newest whatever was picked would make the list a
        // decoration over `Ctrl-Shift-u`.
        let mut app = App::new();
        for claimed in 1..=3 {
            let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
            tally(&mut app, tab, claimed);
            app.close_page(tab);
        }
        let terminal = app.layout().focused();

        app.open_tab_list();
        let mut palette = app.palette.clone().expect("the list opens");
        // The oldest row, which is the last one in a newest-first list.
        palette.selected = palette.filtered.len() - 1;
        app.confirm_palette_selection(&palette, terminal);

        assert_eq!(showing(&app).as_deref(), Some("Counting 1"));
        assert_eq!(app.closed.len(), 2, "the other two are still closed");
    }

    #[test]
    fn test_a_row_standing_for_a_dropped_page_says_so() {
        // The list is drawn from a stash a pane dying behind it can still push
        // onto, which for a full stash drops the oldest. That row must not
        // reopen whichever page took its place.
        let mut app = App::new();
        let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
        app.close_page(tab);
        let gone = app.closed[0].id();
        for claimed in 0..MAX_CLOSED {
            // Each told apart from the rest, so none replaces another's row.
            let tab = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));
            tally(&mut app, tab, claimed + 1);
            app.close_page(tab);
        }

        app.reopen_closed_by_id(gone);
        assert!(
            app.open_tabs.pages().next().is_none(),
            "nothing was reopened in its place"
        );
        assert_eq!(
            app.notice.as_ref().map(|(text, _, _)| text.as_str()),
            Some(GONE_FROM_LIST)
        );
    }

    #[test]
    fn test_the_list_does_not_open_with_nothing_in_it() {
        // An empty panel over an empty stash says nothing about why.
        let mut app = App::new();
        app.open_tab_list();
        assert!(app.palette.is_none());
        assert_eq!(
            app.notice.as_ref().map(|(text, _, _)| text.as_str()),
            Some(NOTHING_CLOSED)
        );
    }

    #[test]
    fn test_reopening_with_nothing_closed_says_so() {
        // A key that does nothing and says nothing reads as a broken binding.
        let mut app = App::new();
        app.reopen_last_closed();
        assert!(app.open_tabs.pages().next().is_none());
        assert_eq!(
            app.notice.as_ref().map(|(text, _, _)| text.as_str()),
            Some(NOTHING_CLOSED)
        );
    }

    #[test]
    fn test_a_page_report_reaches_the_status_bar_whatever_the_outcome() {
        // A page reports beside another outcome, such as the job that re-reads
        // it, so the report cannot ride on a `Notice` outcome alone.
        let mut app = App::new();
        let page = ReportingPage(Some(PageNotice::info("pushed")));
        let tab = app.show_page(PageKind::Git, Box::new(page));
        app.act_on_page_outcome(tab, PageOutcome::Consumed);
        assert_eq!(
            app.notice.as_ref().map(|(text, _, _)| text.as_str()),
            Some("pushed")
        );
    }

    /// A directory holding `names`, removed again when the test ends.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str, names: &[&str]) -> Self {
            let dir = std::env::temp_dir()
                .join(format!("winter-browse-page-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            for name in names {
                match name.ends_with('/') {
                    true => std::fs::create_dir_all(dir.join(name.trim_end_matches('/'))),
                    false => std::fs::write(dir.join(name), "hello\n"),
                }
                .expect("temp entry");
            }
            Self(dir)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    /// One key into the palette, the way the window loop delivers it.
    fn browse_key(app: &mut App, named: winit::keyboard::NamedKey) {
        use winit::keyboard::{Key, PhysicalKey};

        let tab = app.layout().focused();
        let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        let mut palette = app.palette.take().expect("the browser is up");
        app.handle_palette_input(&mut palette, &Key::Named(named), &phys, tab);
        if palette.active {
            app.palette = Some(palette);
        }
    }

    /// Put the selection on the row labelled `label`.
    fn browse_to(app: &mut App, label: &str) {
        let palette = app.palette.as_mut().expect("the browser is up");
        let at = palette
            .filtered
            .iter()
            .position(|&i| palette.entries[i].label == label)
            .unwrap_or_else(|| panic!("no row labelled {label}"));
        palette.selected = at;
    }

    #[test]
    fn test_browsing_into_a_directory_keeps_the_palette_up_rooted_there() {
        use winit::keyboard::NamedKey;

        // The whole point of the browser is that a directory is a step rather
        // than a choice: closing on one would make it a one-shot picker.
        let tree = TempTree::new("step", &["src/", "README.md"]);
        std::fs::write(tree.0.join("src").join("lib.rs"), "fn main() {}\n").expect("nested file");
        let mut app = App::new();
        app.palette = Some(crate::model::palette::Palette::open_files(tree.0.clone()));

        browse_to(&mut app, "src/");
        browse_key(&mut app, NamedKey::Enter);

        let palette = app.palette.as_ref().expect("still browsing");
        assert_eq!(palette.dir.as_ref(), Some(&tree.0.join("src")));
        assert!(
            palette.entries.iter().any(|entry| entry.label == "lib.rs"),
            "showing what is in there"
        );

        // Backspace rubs out typed text and never leaves the directory, even
        // with nothing typed.
        browse_key(&mut app, NamedKey::Backspace);
        let palette = app.palette.as_ref().expect("still browsing");
        assert_eq!(palette.dir.as_ref(), Some(&tree.0.join("src")));

        // `Ctrl-H` is the way back out.
        app.modifiers = winit::keyboard::ModifiersState::CONTROL.into();
        let tab = app.layout().focused();
        let phys = winit::keyboard::PhysicalKey::Unidentified(
            winit::keyboard::NativeKeyCode::Unidentified,
        );
        let mut palette = app.palette.take().expect("the browser is up");
        app.handle_palette_input(
            &mut palette,
            &winit::keyboard::Key::Character("h".into()),
            &phys,
            tab,
        );
        app.palette = Some(palette);
        let palette = app.palette.as_ref().expect("still browsing");
        assert_eq!(palette.dir.as_ref(), Some(&tree.0));
    }

    #[test]
    fn test_a_commit_message_opens_with_the_staged_changes_as_a_tab_and_takes_them_along() {
        use crate::control::EditWaiter;
        use std::os::unix::net::UnixStream;

        let tree = TempTree::new("commit-beside", &["COMMIT_EDITMSG"]);
        let mut app = App::new();
        let before = app.layout().members().len();
        let (stream, _client) = UnixStream::pair().expect("socket pair");

        app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));

        let editor = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&editor).map(|slot| slot.kind),
            Some(PageKind::Editor)
        );
        let diff = *app.commit_companions.get(&editor).expect("a tab beside it");
        assert_eq!(
            app.open_tabs.page(&diff).map(|slot| slot.kind),
            Some(PageKind::Git)
        );
        assert_eq!(
            app.layout().members().len(),
            before + 2,
            "editor tab and diff tab"
        );
        assert!(
            app.layout().tabs_in_pane(editor).contains(&diff),
            "the diff is a tab of the editor's pane, not a pane of its own"
        );
        assert_eq!(
            app.layout().recent_in_pane(editor).first(),
            Some(&editor),
            "and the editor is the one showing"
        );

        app.close_tab_now(editor, Closing::Forget);
        assert!(
            !app.layout().contains(diff),
            "the diff goes with the editor"
        );
        assert!(app.commit_companions.is_empty());
    }

    #[test]
    fn test_closing_commit_editor_focuses_git_view_of_repo() {
        use crate::control::EditWaiter;
        use std::os::unix::net::UnixStream;

        let tree = TempTree::new("commit-focus", &["COMMIT_EDITMSG"]);
        let mut app = App::new();
        let git_page = GitPage::new(tree.0.clone());
        let git_tab = app.show_page(PageKind::Git, Box::new(git_page));

        let (stream, _client) = UnixStream::pair().expect("socket pair");
        app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));
        let editor = app.layout().focused();
        assert_ne!(editor, git_tab);

        app.close_page(editor);
        assert_eq!(
            app.layout().focused(),
            git_tab,
            "closing the commit editor focuses on the repo's git view"
        );
    }

    /// A terminal tab over `cat`, with a full-screen program on it when
    /// `busy`, standing in for an agent or an editor that owns the pane.
    fn terminal_tab(app: &mut App, busy: bool) -> TabId {
        let id = app.layout().focused();
        let mut terminal = crate::terminal::session::Terminal::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        if busy {
            terminal.feed_program_output(b"\x1b[?1049h");
        }
        app.open_tabs.insert_terminal(id, terminal);
        id
    }

    #[test]
    fn test_closing_a_tab_with_a_program_running_asks_first_and_an_idle_one_does_not() {
        let mut idle = App::new();
        let tab = terminal_tab(&mut idle, false);
        assert!(
            !idle.ask_before_closing_tab(tab),
            "a shell at its prompt just closes"
        );
        assert!(idle.page_prompt.is_none());

        let mut busy = App::new();
        let tab = terminal_tab(&mut busy, true);
        assert!(busy.ask_before_closing_tab(tab));
        let label = busy
            .page_prompt
            .as_ref()
            .expect("asked")
            .request
            .label
            .clone();
        assert!(label.contains("still running"), "label: {label}");
        assert!(label.contains("Close the tab?"), "label: {label}");
        assert_eq!(busy.pending_close, Some(PendingClose::Tab(tab)));
    }

    #[test]
    fn test_quitting_with_a_program_running_asks_and_a_yes_goes_through_without_asking_again() {
        let mut app = App::new();
        terminal_tab(&mut app, true);

        assert!(app.ask_before_quitting());
        assert_eq!(app.pending_close, Some(PendingClose::Quit));
        assert!(!app.exit_requested, "nothing has been closed yet");

        app.handle_prompt_key(&Key::plain(KeyCode::Char('y')));

        assert!(app.exit_requested);
        assert!(
            app.quit_confirmed,
            "the quit that follows is not asked about again"
        );
    }

    #[test]
    fn test_quitting_declined_keeps_the_window() {
        let mut app = App::new();
        terminal_tab(&mut app, true);
        app.ask_before_quitting();

        app.handle_prompt_key(&Key::plain(KeyCode::Char('n')));

        assert!(!app.exit_requested);
        assert!(!app.quit_confirmed);
        assert!(app.page_prompt.is_none());
    }

    #[test]
    fn test_quitting_an_idle_window_asks_nothing() {
        let mut app = App::new();
        terminal_tab(&mut app, false);
        assert!(!app.ask_before_quitting());
    }

    /// An app with two panes, focus on the first, and the second's id.
    fn two_panes() -> (App, TabId, TabId) {
        let mut app = App::new();
        let first = app.layout().focused();
        let second = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, second);
        app.layout_mut().focus(first);
        (app, first, second)
    }

    #[test]
    fn test_the_git_view_prefers_the_pane_beside_over_the_next_one_in_tree_order() {
        // a over b, with c across the right: tree order from a would be b.
        let mut app = App::new();
        let a = app.layout().focused();
        let c = app.alloc_tab_id();
        app.layout_mut().split(Direction::Vertical, SPLIT_RATIO, c);
        app.layout_mut().focus(a);
        let b = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Horizontal, SPLIT_RATIO, b);
        app.layout_mut().focus(a);

        app.open_git_page();

        let git = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&git).map(|slot| slot.kind),
            Some(PageKind::Git)
        );
        assert!(
            app.layout().tabs_in_pane(c).contains(&git),
            "in the pane beside"
        );
        assert!(!app.layout().tabs_in_pane(b).contains(&git));
    }

    #[test]
    fn test_the_git_view_opens_in_the_other_pane_and_is_found_there_again() {
        let (mut app, first, second) = two_panes();

        app.open_git_page();
        let git = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&git).map(|slot| slot.kind),
            Some(PageKind::Git)
        );
        assert!(
            app.layout().tabs_in_pane(second).contains(&git),
            "beside the work"
        );
        assert!(!app.layout().tabs_in_pane(first).contains(&git));

        app.layout_mut().focus(first);
        app.open_git_page();
        assert_eq!(
            app.layout().focused(),
            git,
            "the open view, not a second one"
        );
        let views = app
            .open_tabs
            .pages()
            .map(|(_, slot)| slot)
            .filter(|slot| slot.kind == PageKind::Git)
            .count();
        assert_eq!(views, 1);
    }

    #[test]
    fn test_the_git_view_opens_new_view_for_different_project() {
        let (mut app, first, second) = two_panes();

        // Project A in first pane
        let term_a = crate::terminal::session::Terminal::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("term a");
        app.open_tabs.insert_terminal(first, term_a);
        app.open_tabs
            .terminal_mut(&first)
            .unwrap()
            .feed_program_output(b"\x1b]7;file://localhost/path/to/project_a\x1b\\");

        // Open git page for project A
        app.open_git_page();
        let git_a = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&git_a).map(|slot| slot.kind),
            Some(PageKind::Git)
        );
        assert_eq!(
            app.open_tabs.page(&git_a).unwrap().page.root_dir(),
            Some(PathBuf::from("/path/to/project_a"))
        );

        // Project B in second pane
        app.layout_mut().focus(second);
        let term_b = crate::terminal::session::Terminal::with_command(
            20,
            4,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("term b");
        app.open_tabs.insert_terminal(second, term_b);
        app.open_tabs
            .terminal_mut(&second)
            .unwrap()
            .feed_program_output(b"\x1b]7;file://localhost/path/to/project_b\x1b\\");

        // Open git page for project B: should NOT reuse project A's view
        app.open_git_page();
        let git_b = app.layout().focused();
        assert_ne!(git_a, git_b, "opens a new git view for project B");
        assert_eq!(
            app.open_tabs.page(&git_b).unwrap().page.root_dir(),
            Some(PathBuf::from("/path/to/project_b"))
        );

        let views = app
            .open_tabs
            .pages()
            .map(|(_, slot)| slot)
            .filter(|slot| slot.kind == PageKind::Git)
            .count();
        assert_eq!(views, 2, "two distinct git views open for two projects");
    }

    #[test]
    fn test_the_git_view_splits_a_pane_off_when_there_is_just_one() {
        let mut app = App::new();
        let only = app.layout().focused();

        app.open_git_page();

        let git = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&git).map(|slot| slot.kind),
            Some(PageKind::Git)
        );
        assert_eq!(app.layout().shown_tabs().len(), 2, "the work and the view");
        assert!(!app.layout().tabs_in_pane(only).contains(&git));
    }

    #[test]
    fn test_the_commit_editor_splits_a_pane_off_when_there_is_just_one() {
        use crate::control::EditWaiter;
        use std::os::unix::net::UnixStream;

        let tree = TempTree::new("commit-split", &["MERGE_MSG"]);
        let mut app = App::new();
        let only = app.layout().focused();
        let (stream, _client) = UnixStream::pair().expect("socket pair");

        app.open_waited_editor(tree.0.join("MERGE_MSG"), EditWaiter::new(stream));

        let editor = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&editor).map(|slot| slot.kind),
            Some(PageKind::Editor)
        );
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert!(!app.layout().tabs_in_pane(only).contains(&editor));
    }

    #[test]
    fn test_the_commit_editor_opens_in_the_other_pane() {
        use crate::control::EditWaiter;
        use std::os::unix::net::UnixStream;

        let tree = TempTree::new("commit-other", &["COMMIT_EDITMSG"]);
        let (mut app, first, second) = two_panes();
        let (stream, _client) = UnixStream::pair().expect("socket pair");

        app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));

        let editor = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&editor).map(|slot| slot.kind),
            Some(PageKind::Editor)
        );
        assert!(app.layout().tabs_in_pane(second).contains(&editor));
        assert!(!app.layout().tabs_in_pane(first).contains(&editor));
    }

    #[test]
    fn test_closing_the_commit_editor_answers_git_even_though_the_page_is_kept_for_reopening() {
        // Regression: a closed page is kept so it can be reopened, and the
        // editor kept its waiter with it, so git was never told the edit was
        // over. The commit hung for as long as the window lived, and the Git
        // tab that started it spun the whole time.
        use crate::control::EditWaiter;
        use std::io::Read;
        use std::os::unix::net::UnixStream;
        use std::time::Duration;

        let tree = TempTree::new("commit-answer", &["COMMIT_EDITMSG"]);
        let mut app = App::new();
        let (stream, mut client) = UnixStream::pair().expect("socket pair");
        client
            .set_read_timeout(Some(Duration::from_millis(300)))
            .expect("timeout");
        app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));
        let editor = app.layout().focused();

        app.close_page(editor);

        let mut answer = [0u8; 1];
        assert_eq!(
            client.read(&mut answer).expect("git was answered"),
            1,
            "the waiting helper hears that the edit is over"
        );
    }

    #[test]
    fn test_a_merge_message_gets_no_staged_pane() {
        use crate::control::EditWaiter;
        use std::os::unix::net::UnixStream;

        let tree = TempTree::new("merge-alone", &["MERGE_MSG"]);
        let mut app = App::new();
        let (stream, _client) = UnixStream::pair().expect("socket pair");

        app.open_waited_editor(tree.0.join("MERGE_MSG"), EditWaiter::new(stream));

        assert!(app.commit_companions.is_empty());
    }

    #[test]
    fn test_each_file_opened_from_a_listing_gets_a_tab_of_its_own() {
        let tree = TempTree::new("two-files", &["one.txt", "two.txt"]);
        let mut app = App::new();
        let listing = app.show_page(PageKind::Grep, Box::new(CountingPage::default()));

        app.open_editor_page(OpenTarget::file(tree.0.join("one.txt")));
        let one = app.layout().focused();
        app.show_tab(listing);
        app.open_editor_page(OpenTarget::file(tree.0.join("two.txt")));
        let two = app.layout().focused();

        assert_ne!(one, two, "two editor tabs");
        assert_eq!(
            app.open_tabs.page(&one).map(|slot| slot.kind),
            Some(PageKind::Editor)
        );
        assert_eq!(
            app.open_tabs.page(&two).map(|slot| slot.kind),
            Some(PageKind::Editor)
        );
    }

    #[test]
    fn test_a_file_opened_from_inside_the_editor_joins_it() {
        // The editor keeps buffers of its own; opening a file from in there
        // adds one rather than a second editor tab.
        let tree = TempTree::new("join", &["one.txt", "two.txt"]);
        let mut app = App::new();
        app.open_editor_page(OpenTarget::file(tree.0.join("one.txt")));
        app.open_editor_page(OpenTarget::file(tree.0.join("two.txt")));
        assert_eq!(app.open_tabs.pages().count(), 1, "one editor, not two");
    }

    #[test]
    fn test_the_browser_chord_pressed_twice_puts_it_away() {
        let mut app = App::new();
        app.open_file_browser();
        assert!(app.palette.is_some());
        app.open_file_browser();
        assert!(app.palette.is_none(), "the same chord toggles it off");
    }

    #[test]
    fn test_browsing_onto_a_file_opens_it_in_an_editor_tab() {
        use winit::keyboard::NamedKey;

        let tree = TempTree::new("open", &["README.md"]);
        let mut app = App::new();
        app.palette = Some(crate::model::palette::Palette::open_files(tree.0.clone()));

        browse_to(&mut app, "README.md");
        browse_key(&mut app, NamedKey::Enter);

        assert!(app.palette.is_none(), "the browser is done");
        let tab = app.layout().focused();
        assert_eq!(
            app.open_tabs.page(&tab).map(|slot| slot.kind),
            Some(PageKind::Editor),
            "and the file is open in a tab of the pane"
        );
    }

    #[test]
    fn test_another_page_opens_beside_the_first_instead_of_toggling() {
        let mut app = App::new();
        app.open_keys_page();
        app.open_dir_page();
        assert_eq!(
            app.open_tabs.pages().count(),
            2,
            "each page has a tab of its own"
        );
        assert_eq!(app.layout().members().len(), 3);
    }

    #[test]
    fn test_ctrl_shift_c_with_nothing_selected_reaches_the_page() {
        // Copy is a global chord, checked ahead of every page. With nothing
        // selected it copies nothing, so a page binding the same chord (the
        // Git page commits on it) has to be the one that gets it.
        let (mut app, pane) = app_with_open_page();
        app.window_keymap = crate::model::input::WindowKeymap::default();
        app.modifiers = winit::event::Modifiers::from(
            winit::keyboard::ModifiersState::CONTROL | winit::keyboard::ModifiersState::SHIFT,
        );
        app.handle_key(
            winit::event::ElementState::Pressed,
            winit::keyboard::Key::Character("C".into()),
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyC),
        );
        assert_eq!(
            app.open_tabs.page(&pane).unwrap().page.title(),
            "Counting 1"
        );
    }

    #[test]
    fn test_declined_key_falls_through_to_the_host() {
        let (mut app, pane) = app_with_open_page();
        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('x'))));
        assert!(!app.offer_key_to_page(pane, &press(KeyCode::Char('z'))));
    }

    #[test]
    fn test_a_page_closing_itself_closes_its_tab() {
        let (mut app, pane) = app_with_open_page();
        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('q'))));
        assert!(app.open_tabs.pages().next().is_none(), "the page is gone");
        assert_eq!(app.layout().members().len(), 1, "the terminal tab is not");
    }

    #[test]
    fn test_a_page_running_a_command_closes_first() {
        // The page is done once it has named the command, so the command
        // runs against the tab shown in its place.
        let (mut app, pane) = app_with_open_page();
        let tabs_before = app.layout().members().len();

        assert!(app.offer_key_to_page(pane, &press(KeyCode::Char('r'))));
        assert!(app.open_tabs.pages().next().is_none(), "the page closed");
        assert_eq!(
            app.layout().members().len(),
            tabs_before,
            "and the command opened a tab in its place"
        );
    }

    #[test]
    fn test_a_pane_with_no_page_is_never_offered_a_page_key() {
        // Keys for an uncovered pane must reach its terminal, so the lookup has
        // to miss rather than find some other pane's page.
        let (mut app, covered) = app_with_open_page();
        let other = TabId(covered.0 + 1);
        assert!(!app.offer_key_to_page(other, &press(KeyCode::Char('x'))));
    }

    #[test]
    fn test_alt_z_toggles_wrapping_without_reaching_the_page() {
        // Wrapping belongs to the pane rather than any one page, so the host
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

    fn app_asking() -> (App, TabId, Rc<RefCell<Vec<Option<String>>>>) {
        let mut app = App::new();
        let heard: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
        let tab = app.show_page(
            PageKind::Grep,
            Box::new(AskingPage {
                heard: Rc::clone(&heard),
            }),
        );
        app.offer_key_to_page(tab, &press(KeyCode::Char('x')));
        (app, tab, heard)
    }

    #[test]
    fn test_a_prompt_starts_holding_its_initial_text() {
        let (app, _, _) = app_asking();
        let view = app.input_view().expect("a dialog");
        assert_eq!(view.input.as_deref(), Some("seed"));
        assert_eq!(view.label, "Name", "the label's separator is the layout's");
    }

    #[test]
    fn test_a_question_answered_by_one_key_offers_no_line_to_type_on() {
        // A caret on a line that cannot be typed into invites typing into it.
        let text = input_dialog("Name: ", "seed", 4, PromptMode::Text);
        assert_eq!(text.input.as_deref(), Some("seed"));

        let confirm = input_dialog("Delete 3 entries?", "", 0, PromptMode::Confirm);
        assert_eq!(confirm.input, None);
        assert_eq!(confirm.label, "Delete 3 entries?");
        assert!(
            confirm.hint.contains('y'),
            "a dialog taking one key says which: {}",
            confirm.hint
        );
    }

    #[test]
    fn test_typing_edits_the_prompt_and_enter_delivers_the_answer() {
        let (mut app, pane, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Backspace));
        app.handle_prompt_key(&press(KeyCode::Char('!')));
        assert_eq!(
            app.input_view().and_then(|view| view.input).as_deref(),
            Some("see!")
        );

        app.handle_prompt_key(&press(KeyCode::Enter));
        assert!(app.page_prompt.is_none(), "the prompt closes on Enter");
        assert!(app.open_tabs.is_page(&pane), "and the page stays open");
        assert_eq!(heard.borrow().as_slice(), [Some("see!".to_string())]);
    }

    #[test]
    fn test_a_prompt_answer_takes_the_emacs_editing_keys() {
        let (mut app, _pane, heard) = app_asking();
        let ctrl = |c| Key {
            ctrl: true,
            ..press(KeyCode::Char(c))
        };
        app.handle_prompt_key(&ctrl('a'));
        app.handle_prompt_key(&press(KeyCode::Char('>')));
        assert_eq!(
            app.input_view().map(|view| view.caret),
            Some(1),
            "the caret is mid-line"
        );
        app.handle_prompt_key(&ctrl('k'));
        app.handle_prompt_key(&ctrl('y'));
        app.handle_prompt_key(&press(KeyCode::Enter));
        assert_eq!(heard.borrow().as_slice(), [Some(">seed".to_string())]);
    }

    #[test]
    fn test_a_saved_page_tab_opens_again_where_it_was() {
        let dir = std::env::temp_dir().join(format!("winter-restore-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("notes.md");
        std::fs::write(&file, "one\ntwo\nthree\n").expect("write");
        let mut app = App::new();
        app.show_page(PageKind::Dir, Box::new(DirPage::new(dir.clone())));
        app.open_path_page(OpenTarget::at_line(file.clone(), 3));
        let saved = app.page_sessions();
        assert_eq!(saved.len(), 2, "both pages say where they were: {saved:?}");

        let mut fresh = App::new();
        for page in &saved {
            assert!(
                fresh.restore_page(TabId(100 + page.id as u64), page),
                "{page:?}"
            );
        }
        let editor = saved
            .iter()
            .find(|page| page.kind == PageKind::Editor.name())
            .expect("the editor");
        assert_eq!(editor.line, Some(3), "at the line it was on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_restored_git_tab_asks_git_for_its_repository() {
        let dir = std::env::temp_dir().join(format!("winter-restore-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let page = PageSession {
            id: 7,
            kind: PageKind::Git.name().to_string(),
            line: None,
            path: dir.to_string_lossy().to_string(),
        };
        let mut app = App::new();
        assert!(!app.jobs.is_busy());
        assert!(app.restore_page(TabId(7), &page));
        assert!(
            app.jobs.is_busy(),
            "its first read is under way, not awaited"
        );

        let dir_page = PageSession {
            kind: PageKind::Dir.name().to_string(),
            ..page
        };
        let mut app = App::new();
        assert!(app.restore_page(TabId(8), &dir_page));
        assert!(!app.jobs.is_busy(), "a listing reads on its own");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_up_brings_back_the_last_answer_to_the_same_question() {
        let (mut app, pane, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Char('!')));
        app.handle_prompt_key(&press(KeyCode::Enter));
        let asked = || crate::model::page::PromptRequest {
            initial: String::new(),
            label: "Name: ".to_string(),
            mode: PromptMode::Text,
            tag: "ask",
        };
        app.act_on_page_outcome(pane, PageOutcome::Prompt(asked()));
        app.handle_prompt_key(&press(KeyCode::Up));
        assert_eq!(
            app.input_view().and_then(|view| view.input).as_deref(),
            Some("seed!")
        );
        app.handle_prompt_key(&press(KeyCode::Down));
        assert_eq!(
            app.input_view().and_then(|view| view.input).as_deref(),
            Some("")
        );
        app.handle_prompt_key(&press(KeyCode::Up));
        app.handle_prompt_key(&press(KeyCode::Enter));
        assert_eq!(heard.borrow().len(), 2);
    }

    #[test]
    fn test_the_space_bar_types_a_space_into_a_prompt() {
        // Regression: it has a key code of its own, which the prompt did not
        // match, so a search or a name could not hold a space.
        let (mut app, _pane, _heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Space));
        app.handle_prompt_key(&press(KeyCode::Char('x')));
        assert_eq!(
            app.input_view().and_then(|view| view.input).as_deref(),
            Some("seed x")
        );
    }

    #[test]
    fn test_a_list_answers_the_page_the_way_a_prompt_does() {
        // The palette stands in for the prompt when the answer is one of a
        // known set: what is chosen comes back under the same tag.
        let (mut app, pane, heard) = app_asking();
        app.page_prompt = None;
        app.act_on_page_outcome(
            pane,
            PageOutcome::Pick(crate::model::page::PickRequest {
                items: vec!["main".to_string(), "feature".to_string()],
                label: "Checkout".to_string(),
                tag: "ask",
            }),
        );
        assert!(app.palette.is_some(), "the list is showing");
        assert_eq!(
            app.page_pick.as_ref().map(|pick| pick.label.as_str()),
            Some("Checkout"),
            "and it says what it is a list of"
        );

        app.answer_page_pick(Some("feature".to_string()));
        assert!(app.page_pick.is_none(), "the question is answered");
        assert_eq!(heard.borrow().as_slice(), [Some("feature".to_string())]);
    }

    #[test]
    fn test_a_list_closed_without_choosing_says_so() {
        let (mut app, pane, heard) = app_asking();
        app.page_prompt = None;
        app.act_on_page_outcome(
            pane,
            PageOutcome::Pick(crate::model::page::PickRequest {
                items: vec!["main".to_string()],
                label: "Checkout".to_string(),
                tag: "ask",
            }),
        );
        app.answer_page_pick(None);
        assert_eq!(heard.borrow().as_slice(), [None]);
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
    fn test_resize_leaves_a_page_tab_showing_its_page() {
        // Resizing walks every laid-out pane and reflows the grids of the
        // terminals; a page tab, which has none, must survive the pass.
        let (mut app, pane) = app_with_open_page();
        app.resize_all_panes();
        assert!(app.open_tabs.is_page(&pane));
    }
}

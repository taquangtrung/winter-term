//! Tool pages over panes: opening one in place, closing it, and offering it
//! keys before the modal keymap sees them.

use std::time::SystemTime;

use crate::model::input::{self, Key, KeyCode};
use crate::model::layout::{LayoutTree, PaneId};
use crate::model::mode::Mode;
use crate::model::page::{
    OpenTarget, Page, PageOutcome, PagePoint, PromptMode, PromptReply, PromptRequest,
};
use crate::model::palette::{Palette, PaletteMode};
use crate::tools::dir::DirPage;
use crate::tools::editor::{EditorPage, DIRTY_MARK};
use crate::tools::git::GitPage;
use crate::tools::grep::GrepPage;
use crate::tools::keys::KeysPage;
use crate::tools::pdf::PdfPage;
use crate::tools::reltime;
use winter_render::Grid;
use winter_render::InputView;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Tool name recorded for the directory listing, so its own chord toggles it.
const DIR_TOOL: &str = "dir";

/// Tool name recorded for the editor.
const EDITOR_TOOL: &str = "editor";

/// Tool name recorded for the git view.
const GIT_TOOL: &str = "git";

/// Tool name recorded for the text search.
const GREP_TOOL: &str = "grep";

/// Tool name recorded for the keys page.
const KEYS_TOOL: &str = "keys";

/// Tool name recorded for the PDF viewer.
const PDF_TOOL: &str = "pdf";

/// How many closed things are kept for reopening. The point of the stash is
/// the tool just closed by accident, not a history of every tool ever closed,
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

/// What a row of the tool list is: a tool that is open somewhere, or one that
/// was closed. The rest of the row's action says which one.
const OPEN_ROW: &str = "open:";
const CLOSED_ROW: &str = "closed:";

/// Which question the host asked when a close would take unwritten edits with
/// it. The host answers this one itself: no page asked it.
const ASK_CLOSE_UNSAVED: &str = "close-unsaved";

/// Said when there is nothing to put back, rather than the key doing nothing
/// at all.
const NOTHING_CLOSED: &str = "no tool has been closed to reopen";

/// Said when the row picked stood for a page the stash has since dropped,
/// which is the one way a list drawn a moment ago can be out of date.
const GONE_FROM_LIST: &str = "that tool is no longer there to reopen";

/// How each kind of question is answered, said under the input: a dialog
/// taking one key has to name it, and a line has to say what ends it.
const HINT_CONFIRM: &str = "y to confirm, any other key to cancel";
const HINT_TEXT: &str = "Enter to accept, Esc to cancel";

/// The glyph each tool shows in the status bar, in place of a mode icon. One
/// line per tool, from the Font Awesome range every Nerd Font carries.
const TOOL_ICONS: [(&str, char); 6] = [
    (DIR_TOOL, '\u{f07b}'),
    (EDITOR_TOOL, '\u{f044}'),
    (GIT_TOOL, '\u{f1d3}'),
    (GREP_TOOL, '\u{f002}'),
    (KEYS_TOOL, '\u{f11c}'),
    (PDF_TOOL, '\u{f1c1}'),
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

/// The page question the palette is standing in for while it shows a list of
/// that page's own choices.
pub(crate) struct ActivePick {
    /// What the list is of, shown where the palette shows its prompt.
    pub(crate) label: String,
    /// The page that asked.
    pub(crate) pane: PaneId,
    /// The question being answered, carried back with the choice.
    pub(crate) tag: &'static str,
}

/// What the host is waiting to close once its question is answered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum PendingClose {
    /// Every pane of the focused tab but this one.
    OtherPanes(PaneId),
    /// This pane.
    Pane(PaneId),
    /// This tab, and every pane in it.
    Tab(usize),
}

/// Whether what is being closed is worth keeping for reopening.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Closing {
    /// The reader may want this back: a tool closed with `q`, a pane merged
    /// away, a tab closed. These are the closes that turn out to be
    /// accidents.
    Keep,
    /// Nothing to keep: a tool toggled off by the chord that toggles it back
    /// on, or a pane whose shell exited on its own. Keeping these would bury
    /// the closes a reader actually wants back.
    Forget,
}

/// Something closed that can be put back, oldest first in `App::closed`.
pub(crate) enum Closed {
    /// One tool page.
    Page(ClosedPage),
    /// A whole pane: where it sat, what it was running in, and the pages it
    /// was holding.
    Pane(ClosedPane),
}

/// A pane that was closed, as much of it as can come back.
///
/// The shell itself cannot: closing the pane dropped its pseudo-terminal and
/// the child with it. What is kept is where the pane sat, so the split comes
/// back rather than a fresh one wherever the reader now is, the directory the
/// shell was in, so its replacement starts there, and the pages it was
/// holding, which are kept whole beside it.
pub(crate) struct ClosedPane {
    closed_at: i64,
    /// The shell's working directory, for the shell that takes its place.
    cwd: Option<String>,
    id: u64,
    /// The whole tab's split tree as it was, the snapshot the pane is put
    /// back into when the rest of the tab has not moved on since.
    layout: LayoutTree,
    /// Which pane of that tree this was.
    pane: PaneId,
    /// The pages it was holding, innermost first, by the id they are kept
    /// under in the same stash.
    pages: Vec<u64>,
    /// Which tab it was in, to find the tab again while it is still there.
    tab: usize,
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
    /// of them shows: the order alone does not say whether the tool above was
    /// closed a moment ago or an hour ago.
    closed_at: i64,
    /// Which closed page this is, so a row of the list means the same page
    /// however the list has changed since it was drawn.
    id: u64,
    page: Box<dyn Page>,
    tool: &'static str,
}

// ========================================================================
// Closed
// ========================================================================

impl Closed {
    /// When it was closed, for the age its row shows.
    fn closed_at(&self) -> i64 {
        match self {
            Closed::Page(page) => page.closed_at,
            Closed::Pane(pane) => pane.closed_at,
        }
    }

    /// Which closed thing this is, so a row of the list stands for it rather
    /// than for a position in a list that moves.
    fn id(&self) -> u64 {
        match self {
            Closed::Page(page) => page.id,
            Closed::Pane(pane) => pane.id,
        }
    }

    /// Whether putting this back would recover work that is nowhere else. A
    /// pane answers for itself only: the pages it was holding are kept beside
    /// it and answer for themselves.
    fn is_dirty(&self) -> bool {
        match self {
            Closed::Page(page) => page.page.is_dirty(),
            Closed::Pane(_) => false,
        }
    }
}

// ========================================================================
// ClosedPane
// ========================================================================

impl ClosedPane {
    /// The shell's directory, for the shell that takes its place.
    pub(super) fn cwd(&self) -> Option<String> {
        self.cwd.clone()
    }

    /// The tab's split tree as it was.
    pub(super) fn layout(&self) -> LayoutTree {
        self.layout.clone()
    }

    /// Every pane the snapshot names, which is what says whether the tab it
    /// came from still agrees with it.
    pub(super) fn layout_panes(&self) -> Vec<PaneId> {
        let mut panes = Vec::new();
        collect_layout_panes(&self.layout, &mut panes);
        panes
    }

    /// Which pane of that tree this was.
    pub(super) fn pane(&self) -> PaneId {
        self.pane
    }

    /// The pages it was holding, innermost first.
    pub(super) fn page_ids(&self) -> Vec<u64> {
        self.pages.clone()
    }

    /// Which tab it was in.
    pub(super) fn tab(&self) -> usize {
        self.tab
    }
}

// ========================================================================
// ClosedPage
// ========================================================================

impl ClosedPage {
    /// How the list of tools names this one.
    fn label(&self) -> String {
        tool_row_label(self.tool, self.page.as_ref())
    }
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
        page_label(self.tool, &self.page.title())
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Now, in seconds since the epoch, which is what an age is measured from. A
/// clock that cannot answer reads as the epoch rather than failing a list of
/// closed tools over a wrong timestamp.
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

/// Every pane a layout snapshot names, in tree order.
fn collect_layout_panes(tree: &LayoutTree, into: &mut Vec<PaneId>) {
    match tree {
        LayoutTree::Pane(pane) => into.push(*pane),
        LayoutTree::Split { first, second, .. } => {
            collect_layout_panes(first, into);
            collect_layout_panes(second, into);
        }
    }
}

/// How a row of the tool list names one page: the tool, what it is showing, a
/// mark when it is holding edits that are on no disk, and the directory it is
/// looking at. The directory is what tells two files of the same name apart,
/// and what makes either findable by the tree it sits in.
fn tool_row_label(tool: &str, page: &dyn Page) -> String {
    let mut label = page_label(tool, &page.title());
    if page.is_dirty() {
        label.push_str(DIRTY_MARK);
    }
    if let Some(dir) = page.cwd() {
        label.push_str("  ");
        label.push_str(&dir.display().to_string());
    }
    label
}

/// How a page is named away from the page itself: the tool it was opened as
/// and what it is showing. A title that merely restates the tool's name
/// ("Keys") is left to stand on its own rather than doubled up.
fn page_label(tool: &str, title: &str) -> String {
    match title.eq_ignore_ascii_case(tool) {
        true => title.to_string(),
        false => format!("{tool}: {title}"),
    }
}

/// The dialog for one question: what is asked, the line it is answered on for
/// a question that takes one, and how to answer it. A question answered by a
/// single key has no line to type on, so it is given none.
pub(crate) fn input_dialog(label: &str, input: &str, mode: PromptMode) -> InputView {
    InputView {
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
        self.show_page(DIR_TOOL, Box::new(DirPage::new(self.focused_start_dir())));
    }

    /// Show the working tree's state over the focused pane, for the repository
    /// containing that pane's working directory.
    pub(crate) fn open_git_page(&mut self) {
        if self.close_page_if_showing(GIT_TOOL) {
            return;
        }
        let page = GitPage::new(self.focused_start_dir());
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
        self.show_page(GREP_TOOL, Box::new(GrepPage::new(self.focused_start_dir())));
    }

    /// Show `target`'s file over whatever the focused pane is showing, in
    /// whichever tool can show it.
    ///
    /// A PDF is not text and the editor refuses it as binary, so it goes to
    /// the viewer instead. Everything else is the editor's.
    pub(crate) fn open_path_page(&mut self, target: OpenTarget) {
        if PdfPage::handles(&target.path) {
            self.stack_page(PDF_TOOL, Box::new(PdfPage::new(target.path)));
            return;
        }
        self.open_editor_page(target);
    }

    /// Show `target`'s file as editable text over whatever the focused pane is
    /// showing, so closing it returns to the tool the file was opened from.
    ///
    /// A file the editor will not open (binary, not UTF-8, too large to
    /// repaint) says so and stays closed; `Ctrl-O` hands those to `$EDITOR`.
    pub(crate) fn open_editor_page(&mut self, target: OpenTarget) {
        // An editor already covering the pane takes the file as another
        // buffer of its own, rather than being covered by a second editor.
        let pane_id = self.tab().focused();
        if let Some(slot) = self.pages.get_mut(&pane_id) {
            if slot.tool == EDITOR_TOOL && slot.page.open_file(target.clone()) {
                self.dirty = true;
                return;
            }
        }
        match EditorPage::new(target.path, target.line) {
            Ok(page) => self.stack_page(EDITOR_TOOL, Box::new(page)),
            Err(e) => self.set_error(format!("{e}")),
        }
    }

    /// Open the file browser over the focused pane's working directory: the
    /// palette, listing a directory a row at a time. The chord that opened it
    /// closes it again, as a tool's own chord does.
    pub(crate) fn open_file_browser(&mut self) {
        let showing = self
            .palette
            .as_ref()
            .is_some_and(|palette| palette.mode == PaletteMode::Files);
        self.palette = match showing {
            true => None,
            false => Some(Palette::open_files(self.focused_start_dir())),
        };
        self.dirty = true;
    }

    /// Cover the focused pane with `page`, keeping any page already there
    /// underneath it: closing the new one puts the old one back, rather than
    /// dropping the listing a file was opened from.
    pub(crate) fn stack_page(&mut self, tool: &'static str, page: Box<dyn Page>) {
        let pane_id = self.tab().focused();
        if let Some(slot) = self.pages.remove(&pane_id) {
            // Work the covered page asked for would come back to whichever
            // page is on top, which is not the one that asked for it.
            self.jobs.cancel_for(pane_id);
            // A pane holds one surface, and it belongs to the page on top.
            // The covered page builds itself a fresh one when it comes back.
            self.webview_mgr.remove_surface(pane_id);
            self.covered.entry(pane_id).or_default().push(slot);
        }
        self.show_page(tool, page);
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
        self.uncover_page(pane_id, Closing::Keep);
    }

    /// Uncover `pane_id`, keeping the page for reopening or not.
    fn uncover_page(&mut self, pane_id: PaneId, closing: Closing) {
        let Some(slot) = self.pages.remove(&pane_id) else {
            return;
        };
        let prior_mode = slot.prior_mode;
        match closing {
            Closing::Keep => {
                self.stash_closed_page(slot);
            }
            Closing::Forget => drop(slot),
        }
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
        self.webview_mgr.remove_surface(pane_id);
        self.last_tile_layout = None;
        self.dirty = true;
        // A page opened over another one uncovers it rather than the terminal,
        // and the page coming back re-reads whatever it was showing: the file
        // just edited is the likeliest thing to have changed underneath it.
        if let Some(mut covered) = self.covered.get_mut(&pane_id).and_then(Vec::pop) {
            let outcome = covered.page.on_resume();
            self.pages.insert(pane_id, covered);
            self.modes.insert(pane_id, Mode::Page);
            self.act_on_page_outcome(pane_id, outcome);
            return;
        }
        self.modes.insert(pane_id, prior_mode);
    }

    /// Ask before closing a pane that is holding edits on no disk.
    ///
    /// Returns whether the question was asked, which is the caller's cue to
    /// wait for the answer rather than close now. A pane's own tools are the
    /// only thing it can lose: the shell underneath is a process, and closing
    /// a pane has always ended one.
    pub(crate) fn ask_before_closing_pane(&mut self, pane_id: PaneId) -> bool {
        self.ask_before_closing(&[pane_id], "pane", PendingClose::Pane(pane_id))
    }

    /// The same, for every pane of the focused tab but one.
    pub(crate) fn ask_before_closing_others(&mut self, keep: PaneId) -> bool {
        let others: Vec<PaneId> = self
            .tab()
            .panes()
            .into_iter()
            .filter(|pane| *pane != keep)
            .collect();
        self.ask_before_closing(&others, "other panes", PendingClose::OtherPanes(keep))
    }

    /// The same, for a whole tab.
    pub(crate) fn ask_before_closing_tab(&mut self, index: usize) -> bool {
        let Some(tab) = self.tabs.all.get(index) else {
            return false;
        };
        let panes = tab.panes();
        self.ask_before_closing(&panes, "tab", PendingClose::Tab(index))
    }

    /// Put the question, if there is anything to ask about.
    fn ask_before_closing(&mut self, panes: &[PaneId], what: &str, pending: PendingClose) -> bool {
        let unsaved = self.unsaved_in(panes);
        if unsaved.is_empty() {
            return false;
        }
        // The pane the dialog belongs to, which is only where it is drawn:
        // the question is the host's and the host answers it.
        let asked_of = panes
            .first()
            .copied()
            .unwrap_or_else(|| self.tab().focused());
        self.pending_close = Some(pending);
        self.page_prompt = Some(ActivePrompt {
            input: String::new(),
            pane: asked_of,
            request: PromptRequest {
                initial: String::new(),
                label: format!(
                    "unsaved edits in {}. Close the {what}? (y/n) ",
                    named(&unsaved)
                ),
                mode: PromptMode::Confirm,
                tag: ASK_CLOSE_UNSAVED,
            },
        });
        self.dirty = true;
        true
    }

    /// What every page over `panes` is holding that is on no disk, named as
    /// the pages name themselves.
    fn unsaved_in(&self, panes: &[PaneId]) -> Vec<String> {
        let mut unsaved = Vec::new();
        for pane in panes {
            let covered = self.covered.get(pane).into_iter().flatten();
            let showing = self.pages.get(pane).into_iter();
            for slot in covered.chain(showing) {
                if slot.page.is_dirty() {
                    unsaved.push(slot.page.title());
                }
            }
        }
        unsaved
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
            PendingClose::Pane(pane) => self.close_pane_now(pane),
            PendingClose::Tab(index) => self.close_tab_now(index),
        }
    }

    /// Keep a closed page for reopening, and say once a session that closing
    /// one is not final.
    ///
    /// An older entry naming the same thing is dropped: a tool opened and
    /// closed over and over would otherwise fill the list with rows that are
    /// all the same page as far as a reader can tell. One holding unwritten
    /// edits is never the one dropped, however old it is.
    pub(crate) fn stash_closed_page(&mut self, slot: PageSlot) -> u64 {
        let closed = ClosedPage {
            closed_at: now_seconds(),
            id: self.take_closed_id(),
            page: slot.page,
            tool: slot.tool,
        };
        let label = closed.label();
        self.closed.retain(|other| match other {
            Closed::Page(page) => page.page.is_dirty() || page.label() != label,
            Closed::Pane(_) => true,
        });
        let id = closed.id;
        self.closed.push(Closed::Page(closed));
        self.trim_closed();
        self.hint_at_reopening(slot.tool);
        id
    }

    /// Keep a closed pane: where it sat, what it was running in, and which
    /// pages went with it.
    pub(crate) fn stash_closed_pane(
        &mut self,
        pane_id: PaneId,
        tab: usize,
        layout: LayoutTree,
        pages: Vec<u64>,
    ) {
        let cwd = self.panes.get(&pane_id).and_then(|pane| pane.cwd());
        let id = self.take_closed_id();
        self.closed.push(Closed::Pane(ClosedPane {
            closed_at: now_seconds(),
            cwd,
            id,
            layout,
            pane: pane_id,
            pages,
            tab,
        }));
        self.trim_closed();
    }

    /// Keep every page `pane_id` was holding, for a pane or a tab being
    /// closed out from under them, and say which ids they went in under.
    ///
    /// The stack goes in bottom first, so reopening walks back up it the way
    /// closing each page would have: the file that was on top comes back
    /// before the listing it was opened from.
    pub(crate) fn stash_pane_pages(&mut self, pane_id: PaneId, closing: Closing) -> Vec<u64> {
        let covered = self.covered.remove(&pane_id).unwrap_or_default();
        let showing = self.pages.remove(&pane_id);
        if closing == Closing::Forget {
            return Vec::new();
        }
        let mut ids: Vec<u64> = covered
            .into_iter()
            .map(|slot| self.stash_closed_page(slot))
            .collect();
        ids.extend(showing.map(|slot| self.stash_closed_page(slot)));
        ids
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
    fn hint_at_reopening(&mut self, tool: &str) {
        if self.reopen_hint_shown {
            return;
        }
        let chord = self.window_keymap.chord_hint(REOPEN_COMMAND);
        if chord.is_empty() {
            return;
        }
        self.reopen_hint_shown = true;
        self.set_notice(format!("{tool} closed. {chord} puts it back"));
    }

    /// Put the last closed page back over the focused pane.
    ///
    /// It goes back wherever the reader is now rather than into the pane it
    /// was closed in, which for a pane or a tab that was closed with it no
    /// longer exists. It covers what is showing there rather than replacing
    /// it, so reopening cannot cost a second page.
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

    /// Act on a row of the tool list: go to a tool that is open somewhere, or
    /// put back one that was closed.
    pub(crate) fn choose_tool_row(&mut self, action: &str) {
        if let Some(pane) = action
            .strip_prefix(OPEN_ROW)
            .and_then(|id| id.parse::<u64>().ok())
        {
            self.switch_to_pane(PaneId(pane));
            return;
        }
        if let Some(id) = action
            .strip_prefix(CLOSED_ROW)
            .and_then(|id| id.parse::<u64>().ok())
        {
            self.reopen_closed_by_id(id);
        }
    }

    /// Show every tool as a list to pick from: the ones open somewhere first,
    /// then the ones closed recently, newest first.
    ///
    /// The same fuzzy panel every other list in Winter is picked from, so the
    /// tool wanted is typed for rather than counted back to, and one list for
    /// both means not having to know which of the two a tool is in.
    pub(crate) fn open_tool_palette(&mut self) {
        let rows = self.tool_rows();
        if rows.is_empty() {
            self.set_notice(NOTHING_CLOSED);
            return;
        }
        self.palette = Some(Palette::open_tools(rows));
        self.dirty = true;
    }

    /// Every row of the tool list, as `(action, label, hint)`.
    fn tool_rows(&self) -> Vec<(String, String, String)> {
        let mut rows: Vec<(String, String, String)> = Vec::new();
        for (index, tab) in self.tabs.all.iter().enumerate() {
            for pane in tab.panes() {
                let Some(slot) = self.pages.get(&pane) else {
                    continue;
                };
                rows.push((
                    format!("{OPEN_ROW}{}", pane.0),
                    tool_row_label(slot.tool, slot.page.as_ref()),
                    self.tab_title(index),
                ));
            }
        }

        let now = now_seconds();
        for closed in self.closed.iter().rev() {
            let age = format!("closed {} ago", reltime::ago(closed.closed_at(), now));
            let label = match closed {
                Closed::Page(page) => page.label(),
                Closed::Pane(pane) => self.closed_pane_label(pane),
            };
            rows.push((format!("{CLOSED_ROW}{}", closed.id()), label, age));
        }
        rows
    }

    /// How the list names a closed pane: where its shell was, and the tools
    /// that went with it, so it reads as more than "a pane".
    fn closed_pane_label(&self, pane: &ClosedPane) -> String {
        let named = match &pane.cwd {
            Some(cwd) => format!("pane: {cwd}"),
            None => "pane".to_string(),
        };
        let tools: Vec<&str> = pane
            .pages
            .iter()
            .filter_map(|id| self.closed.iter().find(|closed| closed.id() == *id))
            .filter_map(|closed| match closed {
                Closed::Page(page) => Some(page.tool),
                Closed::Pane(_) => None,
            })
            .collect();
        match tools.is_empty() {
            true => named,
            false => format!("{named}  ({})", tools.join(", ")),
        }
    }

    /// Put back whatever was closed, page or pane.
    fn restore_closed(&mut self, closed: Closed) {
        match closed {
            Closed::Page(page) => self.restore_closed_page(page),
            Closed::Pane(pane) => self.restore_closed_pane(pane),
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
                Closed::Pane(_) => false,
            });
            if let Some(at) = found {
                if let Closed::Page(page) = self.closed.remove(at) {
                    taken.push(page);
                }
            }
        }
        taken
    }

    /// Cover `pane_id` with a page that was closed, innermost first, so a
    /// pane that was holding a file over a listing gets both back in order.
    pub(super) fn restore_pages_into(&mut self, pane_id: PaneId, pages: Vec<ClosedPage>) {
        for closed in pages {
            self.tab_mut().focus(pane_id);
            self.restore_closed_page(closed);
        }
    }

    /// Cover the focused pane with a page that was closed, re-reading what it
    /// was showing on the way back.
    pub(super) fn restore_closed_page(&mut self, mut closed: ClosedPage) {
        // What it was showing may have changed while it was gone, which is
        // the same reason a covered page re-reads the world when it comes
        // back up.
        let outcome = closed.page.on_resume();
        self.stack_page(closed.tool, closed.page);
        let pane_id = self.tab().focused();
        self.act_on_page_outcome(pane_id, outcome);
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
            // Toggled off, not closed: the chord that did it opens it again,
            // so there is nothing here worth keeping for reopening.
            self.uncover_page(pane_id, Closing::Forget);
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

    /// Route a key a page surface handed back, because the WebView holding
    /// the keyboard is the only thing that saw it.
    ///
    /// The page is offered it first, exactly as a key typed into any other
    /// tool is, and what the page declines is resolved against the window
    /// keymap, so the chords that split, zoom, and switch panes and tabs keep
    /// working from inside a document.
    ///
    /// The terminal's own encoding settings are left at their defaults: a
    /// pane showing a page never forwards a key to its PTY, so nothing here
    /// can reach the encoder they belong to.
    pub(crate) fn route_surface_key(&mut self, pane_id: PaneId, key: Key) {
        if self.offer_key_to_page(pane_id, &key) {
            self.dirty = true;
            return;
        }
        let mode = self.modes.get(&pane_id).copied().unwrap_or_default();
        let action = input::resolve_with(
            mode,
            &key,
            &mut self.pending,
            &self.window_keymap,
            0,
            None,
            false,
        );
        self.handle_action(action, pane_id);
        self.dirty = true;
    }

    /// Offer a pointer press or drag to the page covering `pane_id`, as the
    /// row and column of the pane it landed on. Returns whether the page took
    /// it, so a click it declines still reaches the pane underneath.
    pub(crate) fn offer_mouse_to_page(&mut self, pane_id: PaneId, at: PagePoint) -> bool {
        let Some(slot) = self.pages.get_mut(&pane_id) else {
            return false;
        };
        let outcome = slot.page.on_mouse(at);
        self.act_on_page_outcome(pane_id, outcome)
    }

    /// Offer a turn of the wheel to the page covering `pane_id`.
    pub(crate) fn offer_scroll_to_page(&mut self, pane_id: PaneId, lines: isize) -> bool {
        let Some(slot) = self.pages.get_mut(&pane_id) else {
            return false;
        };
        let outcome = slot.page.on_scroll(lines);
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
        // A question the host asked is the host's to act on: no page asked
        // it, and no page is waiting to hear the answer.
        if let Some(pending) = self.pending_close.take() {
            self.answer_close_prompt(pending, answer);
            return;
        }
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

    /// Hand a page the choice made from the list it asked for, as the answer
    /// to the question it asked. A palette closed without choosing answers
    /// with nothing, the way an escaped prompt does.
    pub(crate) fn answer_page_pick(&mut self, choice: Option<String>) {
        let Some(pick) = self.page_pick.take() else {
            return;
        };
        let Some(slot) = self.pages.get_mut(&pick.pane) else {
            return;
        };
        let outcome = slot.page.on_prompt(PromptReply {
            answer: choice,
            tag: pick.tag,
        });
        self.act_on_page_outcome(pick.pane, outcome);
    }

    /// What the open prompt shows: its question, then what has been typed.
    pub(crate) fn input_view(&self) -> Option<InputView> {
        let prompt = self.page_prompt.as_ref()?;
        Some(input_dialog(
            &prompt.request.label,
            &prompt.input,
            prompt.request.mode,
        ))
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
                self.open_path_page(target);
                true
            }
            PageOutcome::SpawnEditor(target) => {
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
            PageOutcome::Paste => {
                let text = self.clipboard_text().unwrap_or_default();
                let Some(slot) = self.pages.get_mut(&pane_id) else {
                    return true;
                };
                let outcome = slot.page.on_paste(text);
                self.act_on_page_outcome(pane_id, outcome)
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
            PageOutcome::Pick(request) => {
                // The palette is the list picker Winter already has: what a
                // page adds is where the choice goes when it is made.
                self.page_pick = Some(ActivePick {
                    label: request.label,
                    pane: pane_id,
                    tag: request.tag,
                });
                self.palette = Some(Palette::open_pick(request.items));
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
    use crate::app::SPLIT_RATIO;
    use crate::model::input::KeyCode;
    use crate::model::layout::Direction;
    use crate::model::page::{PageContent, PageSpan};
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
                KeyCode::Char('r') => PageOutcome::RunAction("new_tab".to_string()),
                KeyCode::Char('q') => PageOutcome::Close,
                _ => PageOutcome::Ignored,
            }
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
    fn test_a_tool_opens_where_the_page_is_looking_not_where_the_shell_is() {
        // Opening the file browser while reading a file used to start at the
        // shell's directory, which for a file opened from elsewhere is not
        // even the same tree. The page covering the pane answers first.
        let mut app = App::new();
        let looking_at = PathBuf::from("/tmp/winter-test-somewhere-else");
        app.show_page("located", Box::new(LocatedPage(looking_at.clone())));
        assert_eq!(app.focused_start_dir(), looking_at);
    }

    #[test]
    fn test_a_pane_with_no_page_still_starts_where_its_shell_is() {
        // The fallback has to stay intact: a bare terminal pane has no page
        // to ask, and the shell's own directory is the right answer there.
        let mut app = App::new();
        app.show_page("counting", Box::new(CountingPage::default()));
        let with_page = app.focused_start_dir();
        app.close_page_if_showing("counting");
        assert!(app.pages.is_empty(), "the page is gone");
        // A page reporting nowhere leaves the resolution exactly as it was.
        assert_eq!(app.focused_start_dir(), with_page);
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
    fn test_opening_a_file_stacks_the_editor_and_a_refused_one_opens_nothing() {
        // The host side of every `Enter` on a file: the editor has to end up
        // covering the pane, and a file it will not open has to leave the pane
        // showing whatever it was showing rather than an empty editor.
        let dir = std::env::temp_dir().join(format!("winter-open-editor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let text = dir.join("a.txt");
        std::fs::write(&text, "one\ntwo\n").expect("temp file");
        let binary = dir.join("a.bin");
        std::fs::write(&binary, [0x00, 0x01]).expect("temp file");

        let mut app = App::new();
        let pane = app.tab().focused();
        app.open_editor_page(OpenTarget::at_line(text, 2));
        assert_eq!(app.pages.get(&pane).map(|slot| slot.tool), Some("editor"));

        app.close_page(pane);
        app.open_editor_page(OpenTarget::file(binary));
        assert!(app.pages.is_empty(), "nothing was opened");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_page_opened_over_another_uncovers_it_rather_than_the_terminal() {
        // Opening a file from a listing and closing it again has to land back
        // on the listing: dropping it would throw away where the user was in
        // a tree they may have spent a while walking into.
        let mut app = App::new();
        let pane = app.tab().focused();
        app.modes.insert(pane, Mode::Normal);
        app.show_page("counting", Box::new(CountingPage::default()));
        app.stack_page("stacked", Box::new(CountingPage::default()));

        assert_eq!(
            app.pages.get(&pane).map(|slot| slot.tool),
            Some("stacked"),
            "the new page is the one showing"
        );

        app.close_page(pane);
        assert_eq!(
            app.pages.get(&pane).map(|slot| slot.tool),
            Some("counting"),
            "and closing it puts the covered one back"
        );
        assert_eq!(app.modes.get(&pane).copied(), Some(Mode::Page));

        app.close_page(pane);
        assert!(app.pages.is_empty(), "the last one uncovers the terminal");
        assert_eq!(app.modes.get(&pane).copied(), Some(Mode::Normal));
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

    /// Put `claimed` keys into the page covering `pane`, which is what makes
    /// one open page tell itself apart from another.
    fn tally(app: &mut App, pane: PaneId, claimed: usize) {
        for _ in 0..claimed {
            if let Some(slot) = app.pages.get_mut(&pane) {
                slot.page.on_key(&press(KeyCode::Char('x')));
            }
        }
    }

    /// What the focused pane's page calls itself, or nothing when no page
    /// covers it.
    fn showing(app: &App) -> Option<String> {
        app.pages
            .get(&app.tab().focused())
            .map(|slot| slot.page.title())
    }

    #[test]
    fn test_a_closed_page_comes_back_the_page_it_was() {
        // The whole point of reopening: what was lost was the state, not the
        // kind of tool. A rebuilt page of the same kind would come back at
        // zero, which is the bug this catches.
        let (mut app, pane) = app_with_open_page();
        tally(&mut app, pane, 3);
        app.close_page(pane);
        assert!(app.pages.is_empty(), "closed first");

        app.reopen_last_closed();
        assert_eq!(
            app.pages.get(&pane).map(|slot| slot.tool),
            Some("counting"),
            "the tool it was opened as"
        );
        assert_eq!(showing(&app).as_deref(), Some("Counting 3"));
        assert_eq!(app.modes.get(&pane).copied(), Some(Mode::Page));
        assert!(app.closed.is_empty(), "and is spent by reopening");
    }

    #[test]
    fn test_a_closed_pane_comes_back_with_the_tools_it_was_holding() {
        // Merging a split away used to drop the pane and its pages both. One
        // reopen puts the split back and every page it was holding into it,
        // stacked the way they were: the file over the listing it was opened
        // from, not the listing over the file.
        let mut app = App::new();
        let pane = app.tab().focused();
        app.show_page("counting", Box::new(CountingPage::default()));
        tally(&mut app, pane, 1);
        app.stack_page("stacked", Box::new(CountingPage::default()));
        tally(&mut app, pane, 2);

        // A tab's last pane closes the tab instead, so the split has to be
        // there for this to be the pane-closing path at all.
        let sibling = app.alloc_pane_id();
        app.tab_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);
        app.close_pane(pane);
        assert!(app.pages.is_empty() && app.covered.is_empty());
        assert_eq!(
            app.tab().panes(),
            vec![sibling],
            "the split was merged away"
        );

        app.reopen_last_closed();
        assert_eq!(app.tab().panes().len(), 2, "the split is back");
        let back = app.tab().focused();
        assert_eq!(back, pane, "in the place it was, not as a fresh split");
        assert_eq!(showing(&app).as_deref(), Some("Counting 2"), "the top page");
        assert_eq!(
            app.covered.get(&back).map(|under| under.len()),
            Some(1),
            "with the one it was covering still under it"
        );
        assert!(app.closed.is_empty(), "and nothing left over to reopen");
    }

    #[test]
    fn test_a_tool_toggled_off_by_its_own_chord_is_not_kept() {
        // The chord that closed it opens it again, so keeping these would
        // only bury the closes a reader cannot undo any other way.
        let mut app = App::new();
        app.open_dir_page();
        app.open_dir_page();
        assert!(app.pages.is_empty(), "the second press closed it");
        assert!(app.closed.is_empty(), "and left nothing in the stash");
    }

    #[test]
    fn test_closing_the_same_tool_twice_leaves_one_row_for_it() {
        // Opening and closing a listing over and over would otherwise fill
        // the list with rows a reader cannot tell apart, and push the tool
        // they actually lost off the end of it.
        let mut app = App::new();
        let pane = app.tab().focused();
        for _ in 0..3 {
            app.show_page("counting", Box::new(CountingPage::default()));
            app.close_page(pane);
        }
        assert_eq!(app.closed.len(), 1);
    }

    #[test]
    fn test_a_page_holding_unsaved_edits_is_never_the_one_dropped() {
        // The cap exists so the stash stays a way back from an accident. A
        // page holding the only copy of an edit is the one entry dropping
        // which would make it one.
        let mut app = App::new();
        let pane = app.tab().focused();
        app.show_page("editor", Box::new(UnsavedPage("main.rs")));
        app.close_page(pane);
        for claimed in 1..=MAX_CLOSED + 2 {
            app.show_page("counting", Box::new(CountingPage::default()));
            tally(&mut app, pane, claimed);
            app.close_page(pane);
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
        let pane = app.tab().focused();
        app.show_page("editor", Box::new(UnsavedPage("main.rs")));
        let sibling = app.alloc_pane_id();
        app.tab_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);

        app.close_pane(pane);
        assert_eq!(app.tab().panes().len(), 2, "nothing closed yet");
        let asked = app.input_view().expect("the question is up");
        assert!(
            asked.label.contains("main.rs"),
            "naming it: {}",
            asked.label
        );

        // Anything but yes leaves the pane exactly as it was.
        app.handle_prompt_key(&press(KeyCode::Char('n')));
        assert_eq!(app.tab().panes().len(), 2);
        assert!(app.pages.contains_key(&pane), "the editor is still there");

        app.close_pane(pane);
        app.handle_prompt_key(&press(KeyCode::Char('y')));
        assert_eq!(app.tab().panes(), vec![sibling], "and yes closes it");
        assert!(
            app.closed.iter().any(|closed| closed.is_dirty()),
            "with the edits kept for reopening even so"
        );
    }

    #[test]
    fn test_the_first_close_of_a_session_says_how_to_undo_it() {
        // A way back nobody is told about is a way back nobody takes.
        let mut app = App::new();
        let pane = app.tab().focused();
        app.show_page("counting", Box::new(CountingPage::default()));
        app.close_page(pane);
        let said = app
            .notice
            .as_ref()
            .map(|(text, _, _)| text.clone())
            .expect("the first close says so");
        assert!(said.contains("Ctrl-Shift-u"), "naming the key: {said}");

        app.notice = None;
        app.show_page("counting", Box::new(CountingPage::default()));
        app.close_page(pane);
        assert!(app.notice.is_none(), "and only says it once");
    }

    #[test]
    fn test_the_tool_list_reaches_a_tool_that_is_open_elsewhere() {
        // Half the point of one list: a tool already up in another pane is
        // gone to rather than opened a second time.
        let mut app = App::new();
        let here = app.tab().focused();
        app.show_page("counting", Box::new(CountingPage::default()));
        let sibling = app.alloc_pane_id();
        app.tab_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);
        app.tab_mut().focus(sibling);

        app.open_tool_palette();
        let palette = app.palette.clone().expect("the list opens");
        let row = palette
            .entries
            .iter()
            .find(|entry| entry.label.starts_with("counting"))
            .expect("the open tool is listed");
        assert_eq!(row.action, format!("open:{}", here.0));
        app.choose_tool_row(&row.action);
        assert_eq!(app.tab().focused(), here, "focus moved to it");
        assert!(app.closed.is_empty(), "and nothing was reopened");
    }

    #[test]
    fn test_the_stash_holds_the_last_few_closes_and_no_more() {
        // Each entry is a whole page, so the stash is a way back from an
        // accident rather than a log of every tool ever closed.
        let mut app = App::new();
        let pane = app.tab().focused();
        for claimed in 0..MAX_CLOSED + 2 {
            app.show_page("counting", Box::new(CountingPage::default()));
            tally(&mut app, pane, claimed);
            app.close_page(pane);
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
    fn test_the_list_of_closed_tools_reads_newest_first() {
        // The order is the whole point of a recency list, and a row has to say
        // which tool and what it was showing: "counting" twice over would be
        // two rows a reader cannot choose between.
        let mut app = App::new();
        let pane = app.tab().focused();
        for claimed in 1..=2 {
            app.show_page("counting", Box::new(CountingPage::default()));
            tally(&mut app, pane, claimed);
            app.close_page(pane);
        }

        app.open_tool_palette();
        let palette = app.palette.as_ref().expect("the list opens");
        assert_eq!(palette.mode, PaletteMode::Tools);
        let labels: Vec<&str> = palette
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["counting: Counting 2", "counting: Counting 1"]);
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
        let pane = app.tab().focused();
        for claimed in 1..=3 {
            app.show_page("counting", Box::new(CountingPage::default()));
            tally(&mut app, pane, claimed);
            app.close_page(pane);
        }

        app.open_tool_palette();
        let mut palette = app.palette.clone().expect("the list opens");
        // The oldest row, which is the last one in a newest-first list.
        palette.selected = palette.filtered.len() - 1;
        app.confirm_palette_selection(&palette, pane);

        assert_eq!(showing(&app).as_deref(), Some("Counting 1"));
        assert_eq!(app.closed.len(), 2, "the other two are still closed");
    }

    #[test]
    fn test_a_row_standing_for_a_dropped_page_says_so() {
        // The list is drawn from a stash a pane dying behind it can still push
        // onto, which for a full stash drops the oldest. That row must not
        // reopen whichever page took its place.
        let mut app = App::new();
        let pane = app.tab().focused();
        app.show_page("counting", Box::new(CountingPage::default()));
        app.close_page(pane);
        let gone = app.closed[0].id();
        for _ in 0..MAX_CLOSED {
            app.show_page("counting", Box::new(CountingPage::default()));
            app.close_page(pane);
        }

        app.reopen_closed_by_id(gone);
        assert!(app.pages.is_empty(), "nothing was reopened in its place");
        assert_eq!(
            app.notice.as_ref().map(|(text, _, _)| text.as_str()),
            Some(GONE_FROM_LIST)
        );
    }

    #[test]
    fn test_the_list_does_not_open_with_nothing_in_it() {
        // An empty panel over an empty stash says nothing about why.
        let mut app = App::new();
        app.open_tool_palette();
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
        assert!(app.pages.is_empty());
        assert_eq!(
            app.notice.as_ref().map(|(text, _, _)| text.as_str()),
            Some(NOTHING_CLOSED)
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

        let pane = app.tab().focused();
        let phys = PhysicalKey::Unidentified(winit::keyboard::NativeKeyCode::Unidentified);
        let mut palette = app.palette.take().expect("the browser is up");
        app.handle_palette_input(&mut palette, &Key::Named(named), &phys, pane);
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

        // And back out again, on the key that has nothing else to do.
        browse_key(&mut app, NamedKey::Backspace);
        let palette = app.palette.as_ref().expect("still browsing");
        assert_eq!(palette.dir.as_ref(), Some(&tree.0));
    }

    #[test]
    fn test_a_second_file_joins_the_editor_rather_than_covering_it() {
        // Two files opened from a listing used to be two editors stacked on
        // one another, with no way back to the first but closing the second.
        let tree = TempTree::new("two-files", &["one.txt", "two.txt"]);
        let mut app = App::new();
        let pane = app.tab().focused();

        app.open_editor_page(OpenTarget::file(tree.0.join("one.txt")));
        app.open_editor_page(OpenTarget::file(tree.0.join("two.txt")));

        assert_eq!(
            app.pages.get(&pane).map(|slot| slot.tool),
            Some(EDITOR_TOOL)
        );
        assert!(
            app.covered.get(&pane).is_none_or(Vec::is_empty),
            "one editor, not two"
        );
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
    fn test_browsing_onto_a_file_opens_it_in_the_editor_over_the_pane() {
        use winit::keyboard::NamedKey;

        let tree = TempTree::new("open", &["README.md"]);
        let mut app = App::new();
        let pane = app.tab().focused();
        app.palette = Some(crate::model::palette::Palette::open_files(tree.0.clone()));

        browse_to(&mut app, "README.md");
        browse_key(&mut app, NamedKey::Enter);

        assert!(app.palette.is_none(), "the browser is done");
        assert_eq!(
            app.pages.get(&pane).map(|slot| slot.tool),
            Some(EDITOR_TOOL),
            "and the file is open over the pane"
        );
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
        let view = app.input_view().expect("a dialog");
        assert_eq!(view.input.as_deref(), Some("seed"));
        assert_eq!(view.label, "Name", "the label's separator is the layout's");
    }

    #[test]
    fn test_a_question_answered_by_one_key_offers_no_line_to_type_on() {
        // A caret on a line that cannot be typed into invites typing into it.
        let text = input_dialog("Name: ", "seed", PromptMode::Text);
        assert_eq!(text.input.as_deref(), Some("seed"));

        let confirm = input_dialog("Delete 3 entries?", "", PromptMode::Confirm);
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
        assert!(app.pages.contains_key(&pane), "and the page stays open");
        assert_eq!(heard.borrow().as_slice(), [Some("see!".to_string())]);
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
    fn test_resize_leaves_a_covered_pane_showing_its_page() {
        // Resizing walks every laid-out pane and reflows the grid underneath;
        // the page covering it must survive the pass.
        let (mut app, pane) = app_with_open_page();
        app.resize_all_panes();
        assert!(app.pages.contains_key(&pane));
    }
}

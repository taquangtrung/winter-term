//! Tool tabs: opening one beside the focused tab, closing and reopening it,
//! and offering it keys before the window keymap sees them.

use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use crate::control::EditWaiter;
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::layout::{Direction, LayoutTree, Rect, TabId};
use crate::model::paint::{IconKind, Point};
use crate::model::palette::{builtin_commands, Palette, PaletteEntry, PaletteMode};
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditOutcome;
use crate::model::textedit::recall::Recall;
use crate::model::tool::{
    OpenTarget, PromptMode, PromptReply, PromptRequest, SearchReport, Tool, ToolOutcome,
};
use crate::session::ToolSession;
use crate::tools::dir::DirTool;
use crate::tools::editor::{EditorTool, DIRTY_MARK};
use crate::tools::git::GitTool;
use crate::tools::grep::GrepTool;
use crate::tools::image::ImageTool;
use crate::tools::keys::KeysTool;
use crate::model::button::ButtonGroup;
use crate::tools::pdf::PdfTool;
use crate::tools::proc::ProcTool;
use crate::tools::reltime;
use crate::tools::sys::SysTool;
use winter_render::{ButtonView, Grid, InputView};

use super::jobs::STOP_TASKS_KEY;
use super::overlay::{ClosePrompt, Overlay, PaletteOverlay};
use super::{App, SPLIT_RATIO};

// ========================================================================
// Constants
// ========================================================================

/// How many closed things are kept for reopening. The point of the stash is
/// the tool just closed by accident, not a history of every tool ever closed,
/// and each entry holds a whole tool: an editor's buffers, a listing's rows.
const MAX_CLOSED: usize = 8;

/// The ceiling the stash is trimmed to even when everything in it is holding
/// unwritten edits. Past this much unsaved work nothing further can be kept
/// without letting a session grow without bound, and the oldest goes however
/// dirty it is.
const MAX_CLOSED_UNSAVED: usize = 32;

/// The command the first-close hint names, looked up in the keymap in force
/// rather than written out, so the hint cannot name a key that is not bound.
const REOPEN_COMMAND: &str = "reopen_page";

/// What a row of the tab list is: a tool that is open somewhere, or one that
/// was closed. The rest of the row's action says which one.
const OPEN_ROW: &str = "open:";
const CLOSED_ROW: &str = "closed:";

/// How a running program is named when the system does not say.
const RUNNING_PROGRAM: &str = "a program";

/// How a row of the tab list says where an open tool is: the tab showing
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

/// Said when the row picked stood for a tool the stash has since dropped,
/// which is the one way a list drawn a moment ago can be out of date.
const GONE_FROM_LIST: &str = "that tab is no longer there to reopen";

/// The answers to a question taken by one key, as the key and the button it
/// is drawn as: the one asked for first.
const CONFIRM_KEYS: [(&str, &str); 2] = [("y", "Yes"), ("any key", "No")];

/// How a question taken by one key says so in its own words, which its
/// buttons now say instead.
const YES_NO_SUFFIX: &str = "(y/n)";
/// What the question naming a theme being saved asks.
const THEME_NAME_LABEL: &str = "New theme name";

/// Heads the list of a tab's keys, before what the tab is.
const KEYS_TITLE: &str = "Keys";

/// What a terminal's key list is called: its keys are the window's.
const WINDOW_KEYS: &str = "Window";

/// The keys that answer a question typed on a line, and what each does.
const TEXT_KEYS: [(&str, &str); 2] = [("Enter", "accept"), ("Esc", "cancel")];

/// What each character of a secret answer is drawn as, so the dialog shows
/// how much has been typed and never what.
const SECRET_MASK: char = '•';

// ========================================================================
// Data Structures
// ========================================================================

/// Which kind of tool a tab holds: a peer of the terminal, opened by its own
/// chord.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ToolKind {
    Dir,
    Editor,
    Git,
    Grep,
    Image,
    Keys,
    Notifications,
    Pdf,
    Proc,
    Sys,
}

/// A question a tool asked, and the answer being typed for it.
pub(crate) struct ActivePrompt {
    /// Optional button group for interactive confirmation dialogs.
    pub(crate) buttons: Option<ButtonGroup<bool>>,
    /// What has been typed, edited with the same keys as any other text.
    pub(crate) field: TextField,
    /// How far the arrow keys have walked back through earlier answers.
    pub(crate) recall: Recall,
    /// The tab that asked.
    pub(crate) tab: TabId,
    pub(crate) request: PromptRequest,
}

/// The tool question the palette is standing in for while it shows a list of
/// that tool's own choices.
pub(crate) struct ActivePick {
    /// What the list is of, shown where the palette shows its prompt.
    pub(crate) label: String,
    /// The tool that asked.
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
    /// The window, replaced by the binary now on disk.
    Reload,
}

/// Whether what is being closed is worth keeping for reopening.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Closing {
    /// The reader may want this back: a tool closed with `q`, a pane merged
    /// away, a tab closed. These are the closes that turn out to be
    /// accidents.
    Keep,
    /// Nothing to keep: a terminal whose shell exited on its own. Keeping
    /// these would bury the closes a reader actually wants back.
    Forget,
}

/// Something closed that can be put back, oldest first in `App::closed`.
pub(crate) enum Closed {
    /// One tool.
    Tool(ClosedTool),
    /// Terminal tabs, or a whole pane: where they sat, what they were
    /// running in, and the tool tabs that went with them.
    Terminal(ClosedPane),
}

/// Terminal tabs that were closed, as much of them as can come back: one
/// terminal tab, or a whole pane with every tab it held.
///
/// The shells themselves cannot come back: closing a terminal dropped its
/// pseudo-terminal and the child with it. What is kept is where they sat, so
/// a merged-away split comes back rather than a fresh one wherever the reader
/// now is, the directory each shell was in, so its replacement starts there,
/// and the tool tabs that went with them, which are kept whole beside them.
pub(crate) struct ClosedPane {
    closed_at: i64,
    /// Whether the whole pane went, rather than tabs inside one that stayed.
    whole_pane: bool,
    id: u64,
    /// The window's split tree as it was, the snapshot the tabs are put back
    /// into when the rest of the layout has not moved on since.
    layout: LayoutTree,
    /// The tool tabs that went with them, by the id they are kept under in
    /// the same stash.
    tools: Vec<u64>,
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

/// A tool that was closed, kept whole rather than described.
///
/// Reopening one is the tool coming back, not a new tool of its kind: the
/// buffers an editor had open and the edits never written to them, where a
/// listing's cursor was and what it had folded, the tool a document was
/// turned to. Rebuilding from a description would lose exactly what the
/// reader lost by closing it.
pub(crate) struct ClosedTool {
    /// When it was closed, in seconds since the epoch, for the age the list
    /// of them shows: the order alone does not say whether the tool above was
    /// closed a moment ago or an hour ago.
    closed_at: i64,
    /// Which closed tool this is, so a row of the list means the same tool
    /// however the list has changed since it was drawn.
    id: u64,
    tool: Box<dyn Tool>,
    /// The tab it was, so a layout snapshot naming it still finds it.
    tab: TabId,
    kind: ToolKind,
}

// ========================================================================
// PendingClose
// ========================================================================

impl PendingClose {
    /// The tab the close is about, whose own closing makes the question moot.
    /// `None` for the whole window, which no single tab stands for.
    pub(crate) fn subject(self) -> Option<TabId> {
        match self {
            Self::OtherPanes(tab) | Self::OtherTabs(tab) | Self::Terminal(tab) | Self::Tab(tab) => {
                Some(tab)
            }
            Self::Quit | Self::Reload => None,
        }
    }
}

// ========================================================================
// ActivePrompt
// ========================================================================

impl ActivePrompt {
    /// Create an active prompt for `request` on `tab`.
    pub(crate) fn new(tab: TabId, request: PromptRequest) -> Self {
        let buttons = match request.mode {
            PromptMode::Confirm => Some(ButtonGroup::confirm()),
            _ => None,
        };
        Self {
            buttons,
            field: TextField::line(&request.initial),
            recall: Recall::default(),
            tab,
            request,
        }
    }

    /// What a search question has had typed into it so far, and whether it
    /// runs backwards. `None` for any other question.
    pub(crate) fn typed_search(&self) -> Option<(&str, bool)> {
        match self.request.mode {
            PromptMode::Search { reverse } => Some((self.field.text(), reverse)),
            PromptMode::Confirm | PromptMode::Secret | PromptMode::Text => None,
        }
    }
}

// ========================================================================
// ToolKind
// ========================================================================

impl ToolKind {
    /// Every kind, for reading a stored name back.
    const ALL: [ToolKind; 10] = [
        ToolKind::Dir,
        ToolKind::Editor,
        ToolKind::Git,
        ToolKind::Grep,
        ToolKind::Image,
        ToolKind::Keys,
        ToolKind::Notifications,
        ToolKind::Pdf,
        ToolKind::Proc,
        ToolKind::Sys,
    ];

    /// The name a session file records and the icon tables are keyed by.
    pub(crate) fn name(self) -> &'static str {
        match self {
            ToolKind::Dir => "dir",
            ToolKind::Editor => "editor",
            ToolKind::Git => "git",
            ToolKind::Grep => "grep",
            ToolKind::Image => "image",
            ToolKind::Keys => "keys",
            ToolKind::Notifications => "notifications",
            ToolKind::Pdf => "pdf",
            ToolKind::Proc => "proc",
            ToolKind::Sys => "sys",
        }
    }

    /// The kind a stored name stands for, or `None` for one no longer known.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|kind| kind.name() == name)
    }

    /// How the title bar and the window title name the kind.
    pub(crate) fn title(self) -> &'static str {
        match self {
            ToolKind::Dir => "Dir",
            ToolKind::Editor => "Editor",
            ToolKind::Git => "Git",
            ToolKind::Grep => "Grep",
            ToolKind::Image => "Image",
            ToolKind::Keys => "Keys",
            ToolKind::Notifications => "Notifications",
            ToolKind::Pdf => "PDF",
            ToolKind::Proc => "Processes",
            ToolKind::Sys => "System",
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
            Closed::Tool(tool) => tool.closed_at,
            Closed::Terminal(pane) => pane.closed_at,
        }
    }

    /// Which closed thing this is, so a row of the list stands for it rather
    /// than for a position in a list that moves.
    fn id(&self) -> u64 {
        match self {
            Closed::Tool(tool) => tool.id,
            Closed::Terminal(pane) => pane.id,
        }
    }

    /// Whether putting this back would recover work that is nowhere else. A
    /// pane answers for itself only: the tools it was holding are kept beside
    /// it and answer for themselves.
    fn is_dirty(&self) -> bool {
        match self {
            Closed::Tool(tool) => tool.tool.is_dirty(),
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

    /// The tool tabs that went with them.
    pub(super) fn tool_ids(&self) -> Vec<u64> {
        self.tools.clone()
    }

    /// The terminal tabs, in strip order.
    pub(super) fn terminals(&self) -> Vec<ClosedTerminal> {
        self.terminals.clone()
    }
}

// ========================================================================
// ClosedTool
// ========================================================================

impl ClosedTool {
    /// How the tab list names this one.
    fn label(&self) -> String {
        tab_row_label(self.kind, self.tool.as_ref())
    }
}

/// A tool tab's contents: the tool itself, which kind it is, and what the
/// host keeps of its last frame.
pub(crate) struct ToolSlot {
    pub(crate) tool: Box<dyn Tool>,
    /// The row the tool last banded as its cursor line, so a text cursor
    /// starts where the user was already looking.
    pub(crate) cursor_line: Option<usize>,
    /// The grid the tool was last painted into, retained so that selecting
    /// over the tab reads what is on screen: a tool has no terminal grid of
    /// its own to resolve a selection against.
    pub(crate) painted: Option<Grid>,
    kind: ToolKind,
    /// The earliest tick the tool has asked for since its last one. Kept
    /// rather than asked for anew, since a tool reckons from its own clock: a
    /// tick wanted "at once" or "a moment from now", asked for again on every
    /// pass, would stay just ahead of the loop for ever.
    tick_at: Option<Instant>,
}

// ========================================================================
// ToolSlot
// ========================================================================

impl ToolSlot {
    /// A tool of `kind` that has not been painted yet.
    fn new(kind: ToolKind, tool: Box<dyn Tool>) -> Self {
        Self {
            tool,
            cursor_line: None,
            painted: None,
            kind,
            tick_at: None,
        }
    }

    /// Which kind of tool this is.
    pub(crate) fn kind(&self) -> ToolKind {
        self.kind
    }

    /// The icon a pane strip draws before the tab's title.
    pub(crate) fn tab_icon(&self) -> IconKind {
        tab_icon(self.kind, self.tool.as_ref())
    }

    /// How a pane strip names the tab: just the tool's title, since the icon
    /// before it already says which kind of tool it is (an editor's file
    /// type, `dir`'s folder).
    pub(crate) fn tab_label(&self) -> String {
        self.tool.title()
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

/// The icon a tool's tab draws, open or closed: what the tool says it is
/// showing (an editor's file), else its kind's own.
fn tab_icon(kind: ToolKind, tool: &dyn Tool) -> IconKind {
    tool.icon().unwrap_or(IconKind::Tool { kind: kind.name() })
}

/// How a row of the tab list names one tool: its kind, what it is showing, a
/// mark when it is holding edits that are on no disk, and the directory it is
/// looking at. The directory is what tells two files of the same name apart,
/// and what makes either findable by the tree it sits in.
fn tab_row_label(kind: ToolKind, tool: &dyn Tool) -> String {
    let mut label = tool_label(kind, &tool.title());
    if tool.is_dirty() {
        label.push_str(DIRTY_MARK);
    }
    if let Some(dir) = tool.cwd() {
        label.push_str("  ");
        label.push_str(&dir.display().to_string());
    }
    label
}

/// How a tool is named away from the tool itself: the kind it was opened as
/// and what it is showing. A title that merely restates the kind's name
/// ("Keys") is left to stand on its own rather than doubled up.
fn tool_label(kind: ToolKind, title: &str) -> String {
    let name = kind.name();
    let named = title
        .get(..name.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(name));
    match named {
        true => title.to_string(),
        false => format!("{name}: {title}"),
    }
}

/// The dialog for one question: what is asked, the line it is answered on for
/// a question that takes one, and how to answer it. A question answered by a
/// single key has no line to type on, so it is given none.
/// Which way a key walks a prompt's earlier answers: `Some(true)` back to an
/// older one (Up, `C-p`), `Some(false)` on to a newer (Down, `C-n`).
fn recall_step(key: &Key) -> Option<bool> {
    match (key.ctrl, key.glyph()) {
        (false, KeyCode::Up) | (true, KeyCode::Char('p')) => Some(true),
        (false, KeyCode::Down) | (true, KeyCode::Char('n')) => Some(false),
        _ => None,
    }
}

pub(crate) fn input_dialog(
    label: &str,
    input: &str,
    caret: usize,
    mode: PromptMode,
    buttons: Vec<ButtonView>,
) -> InputView {
    InputView {
        buttons,
        caret,
        caret_visible: true,
        input: match mode {
            PromptMode::Confirm => None,
            PromptMode::Secret => Some(input.chars().map(|_| SECRET_MASK).collect()),
            PromptMode::Search { .. } | PromptMode::Text => Some(input.to_string()),
        },
        keys: match mode {
            PromptMode::Confirm => &CONFIRM_KEYS,
            PromptMode::Search { .. } | PromptMode::Secret | PromptMode::Text => &TEXT_KEYS,
        }
        .iter()
        .map(|(key, does)| (key.to_string(), does.to_string()))
        .collect(),
        // Labels are written to sit before an answer on one line, so they end
        // in a separator that reads as a dangling colon over one, or in a
        // `(y/n)` the buttons say instead.
        label: label
            .trim_end()
            .trim_end_matches(YES_NO_SUFFIX)
            .trim_end()
            .trim_end_matches(':')
            .to_string(),
    }
}

// ========================================================================
// App: tools
// ========================================================================

impl App {
    /// Open the keys tool in a tab beside the focused one, or go to the one the
    /// pane already has.
    pub(crate) fn open_keys_tool(&mut self) {
        if self.show_open_tool(ToolKind::Keys) {
            return;
        }
        let tool = KeysTool::new(&self.keymap);
        self.show_tool(ToolKind::Keys, Box::new(tool));
    }

    /// Open a directory listing in a tab beside the focused one, rooted where
    /// that tab is looking, or go to the pane's listing already rooted there.
    /// A listing rooted anywhere else is another place, so it is left alone.
    pub(crate) fn open_dir_tool(&mut self) {
        let focused = self.layout().focused();
        // From inside a listing the chord stays put, as every tool's does,
        // rather than opening the folder under its cursor.
        if self
            .open_tabs
            .tool(&focused)
            .is_some_and(|slot| slot.kind == ToolKind::Dir)
        {
            return;
        }
        let dir = self.focused_start_dir();
        let rooted_here = |tool: &dyn Tool| tool.root_dir().as_ref() == Some(&dir);
        if self
            .show_open_tool_matching(ToolKind::Dir, &rooted_here)
            .is_some()
        {
            return;
        }
        self.record_recent_dir(&dir);
        self.show_tool(ToolKind::Dir, Box::new(DirTool::new(dir)));
    }

    /// List `dir` in a new tab beside the focused one, as a directory opened
    /// from the terminal's text is.
    pub(crate) fn open_dir_tool_at(&mut self, dir: PathBuf) {
        self.record_recent_dir(&dir);
        self.show_tool(ToolKind::Dir, Box::new(DirTool::new(dir)));
    }

    /// Open the working tree's state in a tab beside the focused one, for the
    /// repository containing where that tab is looking. If a view for this
    /// repository is already open, it is reused.
    pub(crate) fn open_git_tool(&mut self) {
        let start = self.focused_start_dir();
        let matches_repo = |tool: &dyn Tool| {
            tool.root_dir().is_some_and(|root| {
                start == root || start.starts_with(&root) || root.starts_with(&start)
            })
        };
        // A view already open is read again: asking for the status is asking
        // for what it is now, which the view's watcher may not have seen (a
        // change under a directory it does not watch, or one made while it
        // was showing output).
        if let Some(tab) = self.show_open_tool_matching(ToolKind::Git, &matches_repo) {
            self.resume_tool(tab);
            return;
        }
        // Where the view looks is decided by the pane it was asked from, before
        // focus moves to the one it opens in.
        self.prefer_other_pane();
        if let Some(tab) = self.show_open_tool_matching(ToolKind::Git, &matches_repo) {
            self.resume_tool(tab);
            return;
        }
        let tool = GitTool::new(start);
        // The view knows nothing until git answers, so its first request goes
        // out with it rather than waiting for a keystroke.
        let first = tool.initial_request();
        let tab_id = self.show_tool_beside(ToolKind::Git, Box::new(tool));
        self.jobs.spawn(tab_id, first);
    }

    /// Focus the open git tool for `repo` if one exists, reading it again,
    /// otherwise open a new git tool for `repo` and focus it.
    pub(crate) fn focus_or_open_git_tool(&mut self, repo: PathBuf) {
        let matches_repo = |tool: &dyn Tool| {
            tool.root_dir().is_some_and(|root| {
                repo == root || repo.starts_with(&root) || root.starts_with(&repo)
            })
        };
        let open_tab = self.layout().members().into_iter().find(|&member| {
            self.open_tabs
                .tool(&member)
                .is_some_and(|slot| slot.kind == ToolKind::Git && matches_repo(&*slot.tool))
        });
        if let Some(tab) = open_tab {
            self.show_tab(tab);
            self.resume_tool(tab);
        } else {
            let tool = GitTool::new(repo);
            let first = tool.initial_request();
            let tab_id = self.show_tool_beside(ToolKind::Git, Box::new(tool));
            self.jobs.spawn(tab_id, first);
        }
    }

    /// Move focus to another pane when the window has more than one, so what
    /// opens next opens there: tools that are read beside the work, not over it.
    /// The pane beside this one in the same row is preferred, then the next in
    /// tree order. With a single pane [`Self::show_tool_beside`] splits one off
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

    /// Show `tool` beside the work: as a tab of the pane focus was moved to by
    /// [`Self::prefer_other_pane`], or, where there is only one pane, in a new
    /// one split off to its right.
    fn show_tool_beside(&mut self, kind: ToolKind, tool: Box<dyn Tool>) -> TabId {
        match self.layout().shown_tabs().len() > 1 {
            true => self.show_tool(kind, tool),
            false => self.show_tool_split(kind, tool),
        }
    }

    /// Split the focused pane vertically and show `tool` in the new half, which
    /// takes focus.
    fn show_tool_split(&mut self, kind: ToolKind, tool: Box<dyn Tool>) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, tab_id);
        self.layout_mut().balance();
        self.open_tabs
            .insert_tool(tab_id, ToolSlot::new(kind, tool));
        self.after_tab_change();
        tab_id
    }

    /// Show the `kind` tab matching `predicate` in the focused pane when it
    /// already has one. One that is already showing stays showing: the tool's
    /// chord never steps away from it. Returns the tab, where there was one.
    fn show_open_tool_matching(
        &mut self,
        kind: ToolKind,
        predicate: &dyn Fn(&dyn Tool) -> bool,
    ) -> Option<TabId> {
        let focused = self.layout().focused();
        let open = self
            .layout()
            .recent_in_pane(focused)
            .into_iter()
            .find(|tab| {
                self.open_tabs
                    .tool(tab)
                    .is_some_and(|slot| slot.kind == kind && predicate(&*slot.tool))
            });
        if let Some(tab) = open.filter(|tab| *tab != focused) {
            self.show_tab(tab);
        }
        open
    }

    /// Have the tool in `tab` read again what it shows, as it does when a tab
    /// that covered it closes.
    fn resume_tool(&mut self, tab: TabId) {
        let Some(slot) = self.open_tabs.tool_mut(&tab) else {
            return;
        };
        let outcome = slot.tool.on_resume();
        self.act_on_tool_outcome(tab, outcome);
    }

    /// Search where the focused tab is looking for text, in a tab of its own.
    pub(crate) fn open_grep_tool(&mut self) {
        if self.show_open_tool(ToolKind::Grep) {
            return;
        }
        self.show_tool(
            ToolKind::Grep,
            Box::new(GrepTool::new(self.focused_start_dir())),
        );
    }

    /// Show the machine's processes in a tab of their own. The monitor asks
    /// for its own first snapshot on the next tick.
    pub(crate) fn open_proc_tool(&mut self) {
        if self.show_open_tool(ToolKind::Proc) {
            return;
        }
        self.show_tool(ToolKind::Proc, Box::new(ProcTool::new()));
    }

    /// Show the machine's CPU, memory, disks, and GPUs in a tab of their own.
    pub(crate) fn open_sys_tool(&mut self) {
        if self.show_open_tool(ToolKind::Sys) {
            return;
        }
        self.show_tool(ToolKind::Sys, Box::new(SysTool::new()));
    }

    /// Refresh each tool whose tick came due by `now`, then note when each
    /// next wants one, and say when the earliest does. A tool that asks for a
    /// later tick than it did keeps the earlier one, and may be ticked before
    /// it has anything to do.
    pub(crate) fn tick_tools(&mut self, now: Instant) -> Option<Instant> {
        let due: Vec<TabId> = self
            .open_tabs
            .tools()
            .filter(|(_, slot)| slot.tick_at.is_some_and(|at| at <= now))
            .map(|(tab, _)| *tab)
            .collect();
        for tab_id in due {
            let Some(slot) = self.open_tabs.tool_mut(&tab_id) else {
                continue;
            };
            slot.tick_at = None;
            let outcome = slot.tool.on_tick(now);
            self.act_on_tool_outcome(tab_id, outcome);
        }
        self.open_tabs
            .tools_mut()
            .filter_map(|(_, slot)| {
                let wanted = slot.tool.next_tick();
                slot.tick_at = match (slot.tick_at, wanted) {
                    (Some(kept), Some(asked)) => Some(kept.min(asked)),
                    (_, asked) => asked,
                };
                slot.tick_at
            })
            .min()
    }

    /// Open `target`'s file in a tab beside the focused one, in whichever
    /// tool can show it.
    ///
    /// A PDF or a picture is not text and the editor refuses it as binary,
    /// so it goes to its viewer instead. Everything else is the editor's,
    /// an SVG included: it is markup as much as a picture, and "Open With"
    /// shows it drawn.
    pub(crate) fn open_path_tool(&mut self, target: OpenTarget) {
        self.record_recent_file(&target.path);
        // The viewers read from the local disk alone.
        let local = crate::remote::parse(&target.path).is_none();
        if local && PdfTool::handles(&target.path) {
            self.show_tool(ToolKind::Pdf, Box::new(PdfTool::new(target.path)));
            return;
        }
        if local && ImageTool::handles(&target.path) && !ImageTool::is_markup(&target.path) {
            self.show_tool(ToolKind::Image, Box::new(ImageTool::new(target.path)));
            return;
        }
        self.open_editor_tool(target);
    }

    /// Open `target`'s file as editable text in a tab beside the focused one,
    /// so closing it returns to the tab the file was opened from.
    ///
    /// A file the editor will not open (binary, not UTF-8, too large to
    /// repaint) says so and stays closed; `C-o` hands those to `$EDITOR`.
    pub(crate) fn open_editor_tool(&mut self, target: OpenTarget) {
        // A file opened from inside the editor joins it as another buffer of
        // its own, rather than opening a second editor beside it.
        let tab_id = self.layout().focused();
        if let Some(slot) = self.open_tabs.tool_mut(&tab_id) {
            if slot.kind == ToolKind::Editor && slot.tool.open_file(target.clone()) {
                // Through the outcome, so a file the editor could not open
                // is reported now rather than at the next key, and a file
                // only root can read asks for the password straight away.
                let outcome = slot.tool.on_resume();
                self.act_on_tool_outcome(tab_id, outcome);
                return;
            }
        }
        match EditorTool::new(target.path, target.line) {
            Ok(mut tool) => {
                let outcome = tool.on_resume();
                let tab_id = self.show_tool(ToolKind::Editor, Box::new(tool));
                self.act_on_tool_outcome(tab_id, outcome);
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
        match EditorTool::new(path, None) {
            Ok(tool) => {
                let with_changes = tool.edits_commit();
                let editor =
                    self.show_tool_beside(ToolKind::Editor, Box::new(tool.with_waiter(waiter)));
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
        let tool = GitTool::staged_diff(start);
        let first = tool.initial_request();
        let diff = self.show_tool(ToolKind::Git, Box::new(tool));
        self.set_focus(editor);
        self.commit_companions.insert(editor, diff);
        self.jobs.spawn(diff, first);
        self.after_tab_change();
    }

    /// Open the file browser where the focused terminal points, else over its
    /// working directory (see [`Self::files_palette_at_point`]): the palette,
    /// listing a directory a row at a time. The chord that opened it closes
    /// it again.
    pub(crate) fn open_file_browser(&mut self) {
        let showing = self
            .palette()
            .is_some_and(|palette| palette.mode == PaletteMode::Files);
        match showing {
            true => self.dismiss_overlay(),
            false => {
                let browser = self.files_palette_at_point();
                self.open_palette(browser);
            }
        }
    }

    /// The tool tabs as a session saves them: each that can say where it was.
    pub(crate) fn tool_sessions(&self) -> Vec<ToolSession> {
        self.open_tabs
            .tools()
            .filter_map(|(id, slot)| {
                let at = slot.tool.snapshot()?;
                Some(ToolSession {
                    id: id.0 as usize,
                    kind: slot.kind.name().to_string(),
                    line: at.line,
                    path: at.path.to_string_lossy().to_string(),
                })
            })
            .collect()
    }

    /// Open a saved tool tab again as tab `id`, outside the layout, which the
    /// restore rebuilds from the session. `false` for a tool that cannot be
    /// opened there any more (a file since deleted, a kind no longer known).
    pub(crate) fn restore_tool(&mut self, id: TabId, saved: &ToolSession) -> bool {
        let path = PathBuf::from(&saved.path);
        // A view that knows nothing until git answers has to ask, as it does
        // when opened; restored without the request it would wait for ever.
        let mut first = None;
        let Some(kind) = ToolKind::from_name(&saved.kind) else {
            return false;
        };
        let tool: Box<dyn Tool> = match kind {
            ToolKind::Dir if path.is_dir() => Box::new(DirTool::new(path)),
            ToolKind::Git if path.is_dir() => {
                let tool = GitTool::new(path);
                first = Some(tool.initial_request());
                Box::new(tool)
            }
            ToolKind::Grep if path.is_dir() => Box::new(GrepTool::new(path)),
            ToolKind::Editor => match EditorTool::restored(path, saved.line) {
                Ok(tool) => Box::new(tool),
                Err(_) => return false,
            },
            ToolKind::Pdf if path.is_file() => Box::new(PdfTool::at_page(path, saved.line)),
            ToolKind::Image if path.is_file() => Box::new(ImageTool::new(path)),
            ToolKind::Dir
            | ToolKind::Git
            | ToolKind::Grep
            | ToolKind::Image
            | ToolKind::Keys
            | ToolKind::Notifications
            | ToolKind::Pdf
            | ToolKind::Proc
            | ToolKind::Sys => return false,
        };
        self.open_tabs.insert_tool(id, ToolSlot::new(kind, tool));
        if let Some(first) = first {
            self.jobs.spawn(id, first);
        }
        true
    }

    /// Open `tool` in a new tab beside the focused one and show it. Returns
    /// the tab it went into.
    pub(crate) fn show_tool(&mut self, kind: ToolKind, tool: Box<dyn Tool>) -> TabId {
        let tab_id = self.alloc_tab_id();
        self.insert_tool_tab(tab_id, kind, tool);
        tab_id
    }

    /// Put `tool` into the layout as tab `tab_id`, beside the focused tab.
    fn insert_tool_tab(&mut self, tab_id: TabId, kind: ToolKind, tool: Box<dyn Tool>) {
        self.open_tabs
            .insert_tool(tab_id, ToolSlot::new(kind, tool));
        self.layout_mut().add_tab(tab_id);
        self.after_tab_change();
    }

    /// Close the tool tab `tab_id`, keeping its tool for reopening.
    pub(crate) fn close_tool(&mut self, tab_id: TabId) {
        self.close_tab_now(tab_id, Closing::Keep);
    }

    /// Ask before closing a pane whose tool tabs hold edits on no disk, or
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

    /// Ask before replacing the window with the binary on disk, when that
    /// would lose something: edits on no disk, which a reload does not carry
    /// over, and tasks still running, which it cuts off. Where the reload
    /// cannot hand the programs in the panes over to the new binary, those
    /// are asked about too, since they end with it.
    pub(crate) fn ask_before_reloading(&mut self) -> bool {
        let everything = self.layout().members();
        let mut lost = self.unsaved_reasons(&everything);
        lost.extend(self.jobs_reason());
        if !cfg!(unix) {
            lost.extend(self.running_reason(&everything));
        }
        self.ask_close(lost, "window and reload", PendingClose::Reload)
    }

    /// Put the question, if there is anything to ask about.
    fn ask_before_closing(&mut self, panes: &[TabId], what: &str, pending: PendingClose) -> bool {
        let mut lost = self.unsaved_reasons(panes);
        lost.extend(self.running_reason(panes));
        self.ask_close(lost, what, pending)
    }

    /// Ask whether to close `what`, naming what would be `lost`; nothing is
    /// asked when nothing would be. Returns whether the question was put.
    fn ask_close(&mut self, lost: Vec<String>, what: &str, pending: PendingClose) -> bool {
        if lost.is_empty() {
            return false;
        }
        self.open_overlay(Overlay::Close(ClosePrompt {
            buttons: ButtonGroup::confirm(),
            label: format!("{}. Close the {what}? (y/n) ", lost.join("; ")),
            pending,
        }));
        true
    }

    /// What closing `panes` would lose in unwritten edits, as a reason.
    fn unsaved_reasons(&self, panes: &[TabId]) -> Vec<String> {
        let unsaved = self.unsaved_in(panes);
        match unsaved.is_empty() {
            true => Vec::new(),
            false => vec![format!("unsaved edits in {}", named(&unsaved))],
        }
    }

    /// What closing `panes` would end of the programs running in them.
    fn running_reason(&self, panes: &[TabId]) -> Option<String> {
        let running = self.running_in(panes);
        (!running.is_empty()).then(|| format!("{} still running", named(&running)))
    }

    /// The tasks running for any tool, which a reload cuts off.
    fn jobs_reason(&self) -> Option<String> {
        let labels = self.jobs.labels();
        (!labels.is_empty()).then(|| format!("{} still running", named(&labels)))
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

    /// What every tool tab among `panes` is holding that is on no disk,
    /// named as the tools name themselves.
    fn unsaved_in(&self, panes: &[TabId]) -> Vec<String> {
        panes
            .iter()
            .filter_map(|pane| self.open_tabs.tool(pane))
            .filter(|slot| slot.tool.is_dirty())
            .map(|slot| slot.tool.title())
            .collect()
    }

    /// Close what the question was about, or nothing when the answer was not
    /// yes. The tools go to the stash either way once the close goes ahead,
    /// so a yes here is still not the last word on those edits.
    fn answer_close_prompt(&mut self, pending: PendingClose, yes: bool) {
        if !yes {
            return;
        }
        match pending {
            PendingClose::OtherPanes(keep) => self.close_other_panes_now(keep),
            PendingClose::OtherTabs(keep) => self.close_other_tabs_now(keep),
            PendingClose::Terminal(pane) => self.close_pane_now(pane),
            PendingClose::Tab(pane) => self.close_tab_now(pane, Closing::Keep),
            PendingClose::Quit => self.exit_requested = true,
            PendingClose::Reload => {
                self.reload_confirmed = true;
                self.pending_reload = true;
            }
        }
        // Closing the last tab quits, and the answer just given was the one
        // that question needed.
        if self.exit_requested {
            self.quit_confirmed = true;
        }
    }

    /// Keep a closed tool for reopening, and say once a session that closing
    /// one is not final.
    ///
    /// An older entry naming the same thing is dropped: a tool opened and
    /// closed over and over would otherwise fill the list with rows that are
    /// all the same tool as far as a reader can tell. One holding unwritten
    /// edits is never the one dropped, however old it is.
    pub(crate) fn stash_closed_tool(&mut self, tab: TabId, mut slot: ToolSlot) -> u64 {
        slot.tool.on_closed();
        let kind = slot.kind;
        let closed = ClosedTool {
            closed_at: now_seconds(),
            id: self.take_closed_id(),
            tool: slot.tool,
            tab,
            kind,
        };
        let label = closed.label();
        self.closed.retain(|other| match other {
            Closed::Tool(tool) => tool.tool.is_dirty() || tool.label() != label,
            Closed::Terminal(_) => true,
        });
        let id = closed.id;
        self.closed.push(Closed::Tool(closed));
        self.trim_closed();
        self.hint_at_reopening(kind);
        id
    }

    /// Keep closed terminal tabs: where they sat, what they were running in,
    /// and which tool tabs went with them. `whole_pane` says the whole pane went.
    pub(crate) fn stash_closed_pane(
        &mut self,
        terminals: &[TabId],
        layout: LayoutTree,
        tools: Vec<u64>,
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
            tools,
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
    fn hint_at_reopening(&mut self, kind: ToolKind) {
        if self.reopen_hint_shown {
            return;
        }
        let chord = self.keymap.chord_hint(REOPEN_COMMAND);
        if chord.is_empty() {
            return;
        }
        self.reopen_hint_shown = true;
        self.set_notice(format!("{} closed. {chord} puts it back", kind.name()));
    }

    /// Put the last closed thing back: a tool tab beside the focused one, or
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

    /// Act on a row of the tab list: go to a tool that is open somewhere, or
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

    /// Show every tool tab as a list to pick from: the ones open somewhere
    /// first, then what was closed recently, newest first.
    pub(crate) fn open_tab_list(&mut self) {
        let rows = self.tab_rows();
        if rows.is_empty() {
            self.set_notice(NOTHING_CLOSED);
            return;
        }
        self.open_palette(Palette::open_tab_list(rows));
        self.dirty = true;
    }

    /// List the keys of the focused tab: a tool's own, read from the tables
    /// it dispatches by, else every window command bound to a key.
    pub(crate) fn open_tab_keys(&mut self, focused: TabId) {
        let (title, rows) = match self.open_tabs.tool(&focused) {
            Some(slot) => {
                let rows = slot
                    .tool
                    .keys()
                    .into_iter()
                    .map(|help| PaletteEntry::new(String::new(), help.label, help.keys))
                    .collect();
                (format!("{KEYS_TITLE}: {}", slot.tool.title()), rows)
            }
            None => {
                let rows = builtin_commands(&self.keymap)
                    .into_iter()
                    .filter(|entry| !entry.shortcut.is_empty())
                    .collect();
                (format!("{KEYS_TITLE}: {WINDOW_KEYS}"), rows)
            }
        };
        self.open_palette(Palette::open_tab_keys(rows, title));
        self.dirty = true;
    }

    /// Every row of the tab list, each with the icon its tab draws.
    fn tab_rows(&self) -> Vec<PaletteEntry> {
        let mut rows: Vec<PaletteEntry> = Vec::new();
        let shown = self.layout().shown_tabs();
        for tab in self.layout().members() {
            let Some(slot) = self.open_tabs.tool(&tab) else {
                continue;
            };
            let hint = match shown.contains(&tab) {
                true => SHOWING_HINT,
                false => OPEN_HINT,
            };
            rows.push(
                PaletteEntry::new(
                    format!("{OPEN_ROW}{}", tab.0),
                    tab_row_label(slot.kind, slot.tool.as_ref()),
                    hint,
                )
                .with_icon(slot.tab_icon()),
            );
        }

        let now = now_seconds();
        for closed in self.closed.iter().rev() {
            let age = format!("closed {} ago", reltime::ago(closed.closed_at(), now));
            let (label, icon) = match closed {
                Closed::Tool(tool) => (tool.label(), tab_icon(tool.kind, tool.tool.as_ref())),
                Closed::Terminal(pane) => (self.closed_pane_label(pane), IconKind::Terminal),
            };
            let id = format!("{CLOSED_ROW}{}", closed.id());
            rows.push(PaletteEntry::new(id, label, age).with_icon(icon));
        }
        rows
    }

    /// How the list names closed terminals: where their shells were, and the
    /// tools that went with them, so a row reads as more than "a pane".
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
        let tools: Vec<&str> = pane
            .tools
            .iter()
            .filter_map(|id| self.closed.iter().find(|closed| closed.id() == *id))
            .filter_map(|closed| match closed {
                Closed::Tool(tool) => Some(tool.kind.name()),
                Closed::Terminal(_) => None,
            })
            .collect();
        match tools.is_empty() {
            true => named,
            false => format!("{named}  ({})", tools.join(", ")),
        }
    }

    /// Put back whatever was closed, tool or pane.
    fn restore_closed(&mut self, closed: Closed) {
        match closed {
            Closed::Tool(tool) => self.restore_closed_tool(tool),
            Closed::Terminal(pane) => self.restore_closed_pane(pane),
        }
    }

    /// The tools a restored pane was holding, taken out of the stash so that
    /// nothing is put back twice. Any that has since been reopened on its own
    /// is simply not there, and the pane comes back without it.
    pub(super) fn take_closed_tools(&mut self, ids: &[u64]) -> Vec<ClosedTool> {
        let mut taken = Vec::new();
        for id in ids {
            let found = self.closed.iter().position(|closed| match closed {
                Closed::Tool(tool) => tool.id == *id,
                Closed::Terminal(_) => false,
            });
            if let Some(at) = found {
                if let Closed::Tool(tool) = self.closed.remove(at) {
                    taken.push(tool);
                }
            }
        }
        taken
    }

    /// Put a closed tool back as a tab beside the focused one, re-reading
    /// what it was showing on the way back. It keeps the tab id it had when
    /// that id is free, so a layout snapshot naming it still finds it.
    pub(super) fn restore_closed_tool(&mut self, mut closed: ClosedTool) {
        // What it was showing may have changed while it was gone.
        let outcome = closed.tool.on_resume();
        let tab_id = match self.layout().contains(closed.tab) {
            true => self.alloc_tab_id(),
            false => closed.tab,
        };
        self.insert_tool_tab(tab_id, closed.kind, closed.tool);
        self.act_on_tool_outcome(tab_id, outcome);
    }

    /// Show the focused pane's `kind` tab when it already has one. Returns
    /// whether there was one, else the caller opens the tool.
    pub(super) fn show_open_tool(&mut self, kind: ToolKind) -> bool {
        self.show_open_tool_matching(kind, &|_| true).is_some()
    }

    /// Offer `key` to the tool covering `tab_id`. Returns whether the key was
    /// spent, so a key the tool declines still reaches the ordinary keymap.
    pub(crate) fn offer_key_to_tool(&mut self, tab_id: TabId, key: &Key) -> bool {
        // The tool's running tasks stop on the stop key, ahead of any use the
        // tool makes of it (the PDF tool's `C-g`), but only while there is
        // something to stop.
        if *key == STOP_TASKS_KEY && self.jobs.is_busy_for(tab_id) {
            self.stop_tasks(tab_id);
            return true;
        }
        // Wrap belongs to the pane, not the tool: every tool gets the toggle,
        // ahead of its own keys, so no tool has to claim it itself.
        if key.mods() == Mods::ALT && key.glyph() == KeyCode::Char('z') {
            self.tool_wrap = !self.tool_wrap;
            self.dirty = true;
            return true;
        }
        // The tool binds the plain key; `S-RET` is a different key, and `s-x`
        // is none a tool binds. Both are the window's to match.
        if key.is_shifted_plain_key() || key.sup {
            return false;
        }
        let neighbors: Vec<PathBuf> = self
            .layout()
            .shown_tabs()
            .into_iter()
            .filter(|pane| *pane != tab_id)
            .filter_map(|pane| {
                self.open_tabs
                    .tool(&pane)
                    .and_then(|slot| slot.tool.root_dir())
            })
            .collect();
        let Some(slot) = self.open_tabs.tool_mut(&tab_id) else {
            return false;
        };
        slot.tool.set_neighbors(neighbors);
        let outcome = slot.tool.on_key(&key.canonical());
        self.act_on_tool_outcome(tab_id, outcome)
    }

    /// Offer a pointer press or drag to the tool covering `tab_id`, as the
    /// row and column of the pane it landed on. Returns whether the tool took
    /// it, so a click it declines still reaches the pane underneath.
    pub(crate) fn offer_mouse_to_tool(&mut self, tab_id: TabId, at: Point) -> bool {
        let Some(slot) = self.open_tabs.tool_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.tool.on_mouse(at);
        self.act_on_tool_outcome(tab_id, outcome)
    }

    /// Offer a turn of the wheel to the tool covering `tab_id`.
    pub(crate) fn offer_scroll_to_tool(&mut self, tab_id: TabId, lines: isize) -> bool {
        let Some(slot) = self.open_tabs.tool_mut(&tab_id) else {
            return false;
        };
        let outcome = slot.tool.on_scroll(lines);
        self.act_on_tool_outcome(tab_id, outcome)
    }

    /// Route one key into the host's close question: `Tab` cycles between buttons,
    /// `Enter` confirms the focused button, direct shortcuts (`y`, `n`, `Esc`) activate immediately.
    pub(crate) fn handle_close_prompt_key(&mut self, key: &Key) {
        let Some(Overlay::Close(prompt)) = &mut self.overlay else {
            return;
        };
        if let Some(yes) = prompt.buttons.on_key(key) {
            let Some(Overlay::Close(prompt)) =
                self.overlay.take_if(|o| matches!(o, Overlay::Close(_)))
            else {
                return;
            };
            self.dirty = true;
            self.answer_close_prompt(prompt.pending, yes);
        } else {
            self.dirty = true;
        }
    }

    /// Route one key into the open prompt: `Enter` answers, `Esc` cancels, and
    /// a confirm prompt resolves on the first key it sees.
    /// Hand what has been typed into tab `tab`'s question to the tool once the
    /// typing pauses, so a search follows the typing without running on
    /// every key.
    pub(crate) fn debounce_prompt_input(&mut self, tab: TabId) {
        self.prompt_debounce = Some((
            std::time::Instant::now()
                + std::time::Duration::from_millis(super::SEARCH_DEBOUNCE_MILLIS),
            tab,
        ));
    }

    pub(crate) fn handle_prompt_key(&mut self, key: &Key) {
        let key = &key.typed();
        let mut debounce_from = None;
        let answer = {
            // The field rather than the accessor, so the answers kept beside
            // it can be read while the prompt is held.
            let Some(Overlay::ToolPrompt(prompt)) = &mut self.overlay else {
                return;
            };
            match (prompt.request.mode, key.glyph()) {
                (PromptMode::Confirm, _) => {
                    if let Some(buttons) = &mut prompt.buttons {
                        if let Some(yes) = buttons.on_key(key) {
                            Some(yes.then(|| "y".to_string()))
                        } else {
                            None
                        }
                    } else {
                        match key.glyph() {
                            KeyCode::Escape => Some(None),
                            KeyCode::Char(c) if c == 'y' || c == 'Y' => Some(Some("y".to_string())),
                            KeyCode::Char(_) => Some(None),
                            _ => None,
                        }
                    }
                }
                (_, KeyCode::Escape) => Some(None),
                // A password is edited like any answer, but is neither
                // recorded for recall nor handed to the tool while typed.
                (PromptMode::Secret, _) => match prompt.field.on_key(key, 1) {
                    EditOutcome::Submit => Some(Some(prompt.field.text().to_string())),
                    EditOutcome::Cancel => Some(None),
                    _ => None,
                },
                // Up and Down walk back through what this question was
                // answered with before, as a shell's history does.
                (PromptMode::Search { .. } | PromptMode::Text, _) if recall_step(key).is_some() => {
                    let answers = self.prompt_answers.of(prompt.request.tag);
                    let typed = prompt.field.text().to_string();
                    let recalled = match recall_step(key) {
                        Some(true) => prompt.recall.older(answers, &typed),
                        _ => prompt.recall.newer(answers),
                    };
                    if let Some(text) = recalled {
                        prompt.field.set_text(&text);
                        debounce_from = Some(prompt.tab);
                    }
                    None
                }
                // The answer is a line of text like any other, so it takes
                // the same Emacs keys a file does while typing.
                (PromptMode::Search { .. } | PromptMode::Text, _) => {
                    match prompt.field.on_key(key, 1) {
                        EditOutcome::Submit => {
                            let answer = prompt.field.text().to_string();
                            self.prompt_answers.record(prompt.request.tag, &answer);
                            Some(Some(answer))
                        }
                        EditOutcome::Cancel => Some(None),
                        outcome => {
                            if outcome != EditOutcome::Ignored {
                                debounce_from = Some(prompt.tab);
                            }
                            None
                        }
                    }
                }
            }
        };
        self.dirty = true;
        if let Some(tab) = debounce_from {
            self.debounce_prompt_input(tab);
        }
        let Some(answer) = answer else {
            return;
        };
        self.search.match_index = 0;
        self.search.match_total = 0;
        self.search.current = None;
        self.prompt_debounce = None;
        let Some(Overlay::ToolPrompt(prompt)) = self
            .overlay
            .take_if(|o| matches!(o, Overlay::ToolPrompt(_)))
        else {
            return;
        };
        let tab_id = prompt.tab;
        let reply = PromptReply {
            answer,
            tag: prompt.request.tag,
        };
        let Some(slot) = self.open_tabs.tool_mut(&tab_id) else {
            return;
        };
        let outcome = slot.tool.on_prompt(reply);
        self.act_on_tool_outcome(tab_id, outcome);
    }

    /// Hand a tool the choice made from the list it asked for, as the answer
    /// to the question it asked, closing the list. The window loop answers
    /// through [`Self::answer_pick`] from the palette it has taken out. A list closed without
    /// choosing answers with nothing, the way an escaped prompt does.
    #[cfg(test)]
    pub(crate) fn answer_tool_pick(&mut self, choice: Option<String>) {
        let Some(mut overlay) = self.take_palette_overlay() else {
            return;
        };
        match overlay.pick.take() {
            Some(pick) => self.answer_pick(pick, choice),
            // Not a pick: the palette was the window's own, and stays.
            None => self.restore_palette_overlay(overlay),
        }
    }

    /// Hand the tool that asked `pick` the `choice` made for it.
    pub(crate) fn answer_pick(&mut self, pick: ActivePick, choice: Option<String>) {
        let Some(slot) = self.open_tabs.tool_mut(&pick.tab) else {
            return;
        };
        let outcome = slot.tool.on_prompt(PromptReply {
            answer: choice,
            tag: pick.tag,
        });
        self.act_on_tool_outcome(pick.tab, outcome);
    }

    /// The dialog for the question that is open, if one is: a tool's, the
    /// host's own before a close, or the name of a theme being saved. A
    /// question is asked in the middle of the window, where the eye already
    /// is, and never on the status bar.
    pub(crate) fn input_view(&self) -> Option<InputView> {
        // A search is typed in the find popup instead (see
        // [`App::find_target`]).
        if let Some(prompt) = self.tool_prompt().filter(|prompt| prompt.typed_search().is_none()) {
            let buttons = prompt
                .buttons
                .as_ref()
                .map(|b| b.to_view())
                .unwrap_or_default();
            return Some(input_dialog(
                &prompt.request.label,
                prompt.field.text(),
                prompt.field.col(),
                prompt.request.mode,
                buttons,
            ));
        }
        if let Some(prompt) = self.close_prompt() {
            return Some(input_dialog(
                &prompt.label,
                "",
                0,
                PromptMode::Confirm,
                prompt.buttons.to_view(),
            ));
        }
        if let Some(input) = self.password_input() {
            return Some(input_dialog(
                super::remote::PASSWORD_LABEL,
                input.field.text(),
                input.field.col(),
                PromptMode::Secret,
                Vec::new(),
            ));
        }
        if let Some(field) = self.zai_key_input() {
            return Some(input_dialog(
                super::quota::ZAI_KEY_LABEL,
                field.text(),
                field.col(),
                PromptMode::Secret,
                Vec::new(),
            ));
        }
        let field = self.theme_name_input()?;
        Some(input_dialog(
            THEME_NAME_LABEL,
            field.text(),
            field.col(),
            PromptMode::Text,
            Vec::new(),
        ))
    }

    /// Take on the search a tool just ran as the window's own, the way a
    /// terminal's `/` is: its matches stay highlighted and counted once the
    /// question is answered, `Esc` puts them away, and the query is the one
    /// `n` and `N` repeat in a terminal too.
    fn adopt_tool_search(&mut self, report: SearchReport) {
        self.search.match_index = report.index;
        self.search.match_total = report.total;
        self.search.current = None;
        self.search.origin = None;
        self.dirty = true;
        // While the question is open, what is typed into it is what is
        // highlighted: a search only becomes the window's once answered.
        if self.tool_prompt().is_some() {
            return;
        }
        let was_hidden = self.search.query.is_none();
        self.search.last = Some(report.query.clone());
        self.search.query = Some(report.query);
        self.search.reverse = report.reverse;
        // Mirrors `start_search`: a live search forces a hidden status bar on.
        if was_hidden && !self.config.status_bar.enabled {
            self.resize_all_panes();
        }
    }

    /// Put `text` on the clipboard for a tool and say `notice`, or say the
    /// clipboard could not be reached.
    fn copy_for_tool(&mut self, text: &str, notice: String) {
        let copied = self
            .clipboard()
            .and_then(|clipboard| clipboard.set_text(text).ok())
            .is_some();
        if copied {
            self.set_notice(notice);
        } else {
            self.set_error("clipboard unavailable");
        }
    }

    /// Carry out what a tool asked for. Returns whether the input that produced
    /// it was spent.
    pub(super) fn act_on_tool_outcome(&mut self, tab_id: TabId, outcome: ToolOutcome) -> bool {
        let notice = self.open_tabs.tool_mut(&tab_id).and_then(|slot| {
            let source = slot.kind().title();
            slot.tool
                .take_notice()
                .map(|notice| notice.from_source(source))
        });
        if let Some(notice) = notice {
            self.notify(notice);
        }
        let search = self
            .open_tabs
            .tool_mut(&tab_id)
            .and_then(|slot| slot.tool.take_search());
        if let Some(search) = search {
            self.adopt_tool_search(search);
        }
        match outcome {
            ToolOutcome::Close => {
                self.close_tool(tab_id);
                true
            }
            ToolOutcome::Consumed => {
                self.dirty = true;
                true
            }
            ToolOutcome::Ignored => false,
            ToolOutcome::OpenExternal(path) => {
                match ::open::that_detached(&path) {
                    Ok(()) => self.set_notice(format!("opened {}", path.display())),
                    Err(e) => self.set_error(format!("could not open {}: {e}", path.display())),
                }
                true
            }
            ToolOutcome::OpenPath(target) => {
                self.open_path_tool(target);
                true
            }
            ToolOutcome::SpawnEditor(target) => {
                self.open_file_in_new_tab(target.path, target.line);
                true
            }
            ToolOutcome::CancelJobs => {
                self.jobs.cancel_for(tab_id);
                self.dirty = true;
                true
            }
            ToolOutcome::Job(request) => {
                self.jobs.spawn(tab_id, request);
                true
            }
            ToolOutcome::Spawn(request) => {
                self.spawn_in_tab(request);
                true
            }
            ToolOutcome::Paste => {
                let text = self.clipboard_text().unwrap_or_default();
                let Some(slot) = self.open_tabs.tool_mut(&tab_id) else {
                    return true;
                };
                let outcome = slot.tool.on_paste(text);
                self.act_on_tool_outcome(tab_id, outcome)
            }
            ToolOutcome::Yank(value) => {
                let notice = format!("copied {value}");
                self.copy_for_tool(&value, notice);
                true
            }
            ToolOutcome::Copy { notice, text } => {
                self.copy_for_tool(&text, notice);
                true
            }
            ToolOutcome::Prompt(request) => {
                // A new search counts its own matches, not the last one's.
                if matches!(request.mode, PromptMode::Search { .. }) {
                    self.search.match_index = 0;
                    self.search.match_total = 0;
                }
                self.open_overlay(Overlay::ToolPrompt(ActivePrompt::new(tab_id, request)));
                true
            }
            ToolOutcome::Pick(request) => {
                // The palette is the list picker Winter already has: what a
                // tool adds is where the choice goes when it is made.
                self.open_overlay(Overlay::Palette(PaletteOverlay {
                    palette: Palette::open_pick(request.items, request.icons),
                    pick: Some(ActivePick {
                        label: request.label,
                        tab: tab_id,
                        tag: request.tag,
                    }),
                }));
                true
            }
            ToolOutcome::RunAction(action) => {
                // The tool is done once it has named the command, and the
                // command runs against the tab shown in its place.
                self.close_tool(tab_id);
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
    use crate::model::paint::{Frame, Span};
    use crate::model::tool::{CommandRequest, JobRequest, Notice};
    use std::cell::RefCell;
    use std::path::PathBuf;
    use std::rc::Rc;
    use std::time::Duration;

    /// Far longer than a tool's tick takes to come due.
    const TICK_WAIT: Duration = Duration::from_millis(500);

    /// The longest a test loop sleeps between two passes.
    const TICK_STEP: Duration = Duration::from_millis(5);

    /// A tool that counts the keys it claims, so a test can tell a key the
    /// tool spent from one it handed back.
    #[derive(Default)]
    struct CountingTool {
        claimed: usize,
    }

    impl Tool for CountingTool {
        /// The count is in the title so a test can tell one instance from
        /// another: a tool that comes back holding what it held is not the
        /// same thing as a fresh tool of its kind.
        fn title(&self) -> String {
            format!("Counting {}", self.claimed)
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("counting")]])
        }

        fn on_key(&mut self, key: &Key) -> ToolOutcome {
            match key.glyph() {
                KeyCode::Char('x') => {
                    self.claimed += 1;
                    ToolOutcome::Consumed
                }
                // The chord the Git tool commits on, which the window also
                // binds to copy.
                // Matched on the lowercase letter alone: the window hands over
                // the capital, and the tool must not have to know.
                KeyCode::Char('c') if key.ctrl && key.shift => {
                    self.claimed += 1;
                    ToolOutcome::Consumed
                }
                KeyCode::Char('r') => ToolOutcome::RunAction("new_tab".to_string()),
                KeyCode::Char('q') => ToolOutcome::Close,
                _ => ToolOutcome::Ignored,
            }
        }
    }

    /// A tool that asks to be ticked `after` from whenever it is asked, as a
    /// tool polling a background read does, and counts the ticks it gets.
    struct PollingTool {
        after: Duration,
        ticks: Rc<RefCell<usize>>,
    }

    impl Tool for PollingTool {
        fn title(&self) -> String {
            "Polling".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("polling")]])
        }

        fn on_key(&mut self, _key: &Key) -> ToolOutcome {
            ToolOutcome::Ignored
        }

        fn next_tick(&self) -> Option<Instant> {
            Some(Instant::now() + self.after)
        }

        fn on_tick(&mut self, _now: Instant) -> ToolOutcome {
            *self.ticks.borrow_mut() += 1;
            ToolOutcome::Consumed
        }
    }

    /// A tool with one report queued for the status bar.
    struct ReportingTool(Option<Notice>);

    impl Tool for ReportingTool {
        fn title(&self) -> String {
            "Reporting".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("reporting")]])
        }

        fn on_key(&mut self, _key: &Key) -> ToolOutcome {
            ToolOutcome::Consumed
        }

        fn take_notice(&mut self) -> Option<Notice> {
            self.0.take()
        }
    }

    /// A tool holding edits that are on no disk, which is the one thing
    /// closing a pane cannot give back on its own.
    struct UnsavedTool(&'static str);

    impl Tool for UnsavedTool {
        fn title(&self) -> String {
            self.0.to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("unsaved")]])
        }

        fn on_key(&mut self, _key: &Key) -> ToolOutcome {
            ToolOutcome::Ignored
        }

        fn is_dirty(&self) -> bool {
            true
        }
    }

    /// A tool that reports `dir` as the place it is looking at.
    struct LocatedTool(PathBuf);

    impl Tool for LocatedTool {
        fn title(&self) -> String {
            "Located".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("located")]])
        }

        fn on_key(&mut self, _key: &Key) -> ToolOutcome {
            ToolOutcome::Ignored
        }

        fn cwd(&self) -> Option<PathBuf> {
            Some(self.0.clone())
        }
    }

    #[test]
    fn test_a_tool_opens_where_the_tool_is_looking_not_where_the_shell_is() {
        // Opening the file browser while reading a file used to start at the
        // shell's directory, which for a file opened from elsewhere is not
        // even the same tree. The tool tab being looked at answers first.
        let mut app = App::new();
        let looking_at = PathBuf::from("/tmp/winter-test-somewhere-else");
        app.show_tool(ToolKind::Dir, Box::new(LocatedTool(looking_at.clone())));
        assert_eq!(app.focused_start_dir(), looking_at);
    }

    #[test]
    fn test_a_tool_tab_with_no_place_of_its_own_starts_where_its_shell_is() {
        // A tool that says nothing about where it is looking defers to the
        // terminal it was opened from, so a tab opened from it does not land
        // in the process's own directory.
        let mut app = App::new();
        let from_shell = app.focused_start_dir();
        app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        assert_eq!(app.focused_start_dir(), from_shell);
    }

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    fn app_with_open_tool() -> (App, TabId) {
        let mut app = App::new();
        let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        (app, tab)
    }

    #[test]
    fn test_the_stop_key_stops_a_busy_tool_and_is_the_tool_s_own_when_idle() {
        let (mut app, tab) = app_with_open_tool();
        // Idle: nothing to stop, so the key is offered to the tool, which
        // declines it.
        assert!(!app.offer_key_to_tool(tab, &STOP_TASKS_KEY));
        app.jobs.spawn(
            tab,
            JobRequest::Command(CommandRequest {
                args: vec!["30".to_string()],
                cwd: PathBuf::from("."),
                env: Vec::new(),
                program: "sleep".to_string(),
                stdin: None,
                tag: "test",
            }),
        );
        assert!(app.offer_key_to_tool(tab, &STOP_TASKS_KEY));
        let notice = app.notice_text();
        assert_eq!(notice, Some("stopping sleep 30"));
        assert!(
            app.job_progress_line(tab).is_none(),
            "too new to mention yet"
        );
    }

    #[test]
    fn test_a_tool_opens_as_a_tab_beside_the_terminal_without_splitting() {
        let mut app = App::new();
        let terminal = app.layout().focused();
        let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        assert_ne!(tab, terminal, "a tab of its own");
        assert_eq!(
            app.layout().shown_tabs(),
            vec![tab],
            "showing in the same pane"
        );
        assert_eq!(app.layout().members(), vec![terminal, tab]);
        assert!(app.open_tabs.is_tool(&tab));
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
        app.open_editor_tool(OpenTarget::at_line(text, 2));
        let editor = app.layout().focused();
        assert_eq!(
            app.open_tabs.tool(&editor).map(|slot| slot.kind),
            Some(ToolKind::Editor)
        );

        app.close_tool(editor);
        assert_eq!(app.layout().focused(), terminal);
        app.open_editor_tool(OpenTarget::file(binary));
        assert!(app.open_tabs.tools().next().is_none(), "nothing was opened");
        assert_eq!(app.layout().focused(), terminal);

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_closing_a_tab_opened_from_another_goes_back_to_it() {
        // Opening a file from a listing and closing it again has to land back
        // on the listing, not on whichever tab sits next to it on the strip.
        let mut app = App::new();
        let listing = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        app.layout_mut().focus(TabId(0));
        let other = app.show_tool(ToolKind::Proc, Box::new(CountingTool::default()));
        app.show_tab(listing);
        let file = app.show_tool(ToolKind::Sys, Box::new(CountingTool::default()));
        assert_eq!(
            app.layout().members(),
            vec![TabId(0), other, listing, file],
            "the file opens right beside the listing"
        );

        app.close_tool(file);
        assert_eq!(app.layout().focused(), listing);
        assert!(
            app.open_tabs.is_tool(&listing),
            "and the listing is as it was"
        );
    }

    #[test]
    fn test_closing_a_tool_leaves_the_terminal_as_it_was() {
        // The terminal tab kept running beside the tool, so it has to come
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
        let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));

        app.close_tool(tab);
        assert!(app.open_tabs.tools().next().is_none());
        assert_eq!(app.layout().members(), vec![terminal]);
        assert_eq!(app.open_tabs.mode(&terminal), Some(Mode::Normal));
        assert!(!app.open_tabs.is_tool(&tab), "the page's tab is forgotten");
    }

    /// Put `claimed` keys into the tool covering `pane`, which is what makes
    /// one open tool tell itself apart from another.
    fn tally(app: &mut App, tab: TabId, claimed: usize) {
        for _ in 0..claimed {
            if let Some(slot) = app.open_tabs.tool_mut(&tab) {
                slot.tool.on_key(&press(KeyCode::Char('x')));
            }
        }
    }

    /// What the focused pane's tool calls itself, or nothing when no tool
    /// covers it.
    fn showing(app: &App) -> Option<String> {
        app.open_tabs
            .tool(&app.layout().focused())
            .map(|slot| slot.tool.title())
    }

    #[test]
    fn test_a_closed_tool_comes_back_the_tool_it_was() {
        // The whole point of reopening: what was lost was the state, not the
        // kind of tool. A rebuilt tool of the same kind would come back at
        // zero, which is the bug this catches.
        let (mut app, pane) = app_with_open_tool();
        tally(&mut app, pane, 3);
        app.close_tool(pane);
        assert!(app.open_tabs.tools().next().is_none(), "closed first");

        app.reopen_last_closed();
        assert_eq!(app.layout().focused(), pane, "under the tab id it had");
        assert_eq!(
            app.open_tabs.tool(&pane).map(|slot| slot.kind),
            Some(ToolKind::Grep),
            "the page it was opened as"
        );
        assert_eq!(showing(&app).as_deref(), Some("Counting 3"));
        assert!(app.open_tabs.is_tool(&pane));
        assert!(app.closed.is_empty(), "and is spent by reopening");
    }

    #[test]
    fn test_a_closed_pane_comes_back_with_the_tools_it_was_holding() {
        // Merging a split away used to drop the pane and its tools both. One
        // reopen puts the split back, in its place, with every tab it held.
        let mut app = App::new();
        let terminal = app.layout().focused();
        let listing = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        tally(&mut app, listing, 1);
        let file = app.show_tool(ToolKind::Sys, Box::new(CountingTool::default()));
        tally(&mut app, file, 2);

        let sibling = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);
        app.close_pane(file);
        assert!(app.open_tabs.tools().next().is_none());
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
            app.open_tabs.tool(&file).unwrap().tool.title(),
            "Counting 2"
        );
        assert_eq!(
            app.open_tabs.tool(&listing).unwrap().tool.title(),
            "Counting 1"
        );
        assert!(app.closed.is_empty(), "and nothing left over to reopen");
    }

    #[test]
    fn test_a_tool_chord_stays_on_the_tool_and_returns_to_it_from_elsewhere() {
        // The chord that opened a tool does nothing while the tool is in
        // front, and from another tab goes to the same tool again, which keeps
        // what it was showing rather than starting over.
        let mut app = App::new();
        let terminal = app.layout().focused();
        app.open_keys_tool();
        let keys = app.layout().focused();
        assert_ne!(keys, terminal);
        app.open_keys_tool();
        assert_eq!(app.layout().focused(), keys, "stays on the tool");
        app.show_tab(terminal);
        app.open_keys_tool();
        assert_eq!(app.layout().focused(), keys, "and to the same tab again");
        assert_eq!(app.open_tabs.tools().count(), 1, "never a second one");
        assert!(app.closed.is_empty(), "nothing was closed");
    }

    #[test]
    fn test_closing_the_same_tool_twice_leaves_one_row_for_it() {
        // Opening and closing a listing over and over would otherwise fill
        // the list with rows a reader cannot tell apart, and push the tool
        // they actually lost off the end of it.
        let mut app = App::new();
        for _ in 0..3 {
            let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
            app.close_tool(tab);
        }
        assert_eq!(app.closed.len(), 1);
    }

    #[test]
    fn test_a_tool_holding_unsaved_edits_is_never_the_one_dropped() {
        // The cap exists so the stash stays a way back from an accident. A
        // tool holding the only copy of an edit is the one entry dropping
        // which would make it one.
        let mut app = App::new();
        let tab = app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        app.close_tool(tab);
        for claimed in 1..=MAX_CLOSED + 2 {
            let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
            tally(&mut app, tab, claimed);
            app.close_tool(tab);
        }
        assert!(
            app.closed.iter().any(|closed| closed.is_dirty()),
            "the unsaved one outlived the cap"
        );
        assert_eq!(app.closed.len(), MAX_CLOSED, "with the cap still held");
    }

    #[test]
    fn test_closing_a_pane_over_unsaved_edits_asks_first() {
        // Closing a pane never went near the tool's own close, so an editor
        // holding unwritten edits was merged away without a word.
        let mut app = App::new();
        let tab = app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        let sibling = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);

        app.close_pane(tab);
        assert_eq!(app.layout().shown_tabs().len(), 2, "nothing closed yet");
        let asked = app.close_prompt().expect("the question is up");
        assert!(
            asked.label.contains("main.rs"),
            "naming it: {}",
            asked.label
        );

        // Anything but yes leaves the pane exactly as it was.
        app.handle_close_prompt_key(&press(KeyCode::Char('n')));
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert!(app.open_tabs.is_tool(&tab), "the editor is still there");

        app.close_pane(tab);
        app.handle_close_prompt_key(&press(KeyCode::Char('y')));
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
        let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        app.close_tool(tab);
        let said = app
            .notice_text()
            .map(str::to_string)
            .expect("the first close says so");
        assert!(said.contains("C-S-u"), "naming the key: {said}");

        app.notifier.toasts.dismiss_all();
        let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        app.close_tool(tab);
        assert!(app.notice_text().is_none(), "and only says it once");
    }

    #[test]
    fn test_the_tab_list_reaches_a_tool_that_is_open_elsewhere() {
        // Half the point of one list: a tool already up in another pane is
        // gone to rather than opened a second time.
        let mut app = App::new();
        let here = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        let sibling = app.alloc_tab_id();
        app.layout_mut()
            .split(Direction::Vertical, SPLIT_RATIO, sibling);
        app.layout_mut().focus(sibling);

        app.open_tab_list();
        let palette = app.palette().cloned().expect("the list opens");
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
    fn test_f1_h_lists_the_focused_tool_s_own_keys() {
        let mut app = App::new();
        assert_eq!(app.keymap.chord_hint("tab_keys"), "<f1> h");
        let tab = app.show_tool(ToolKind::Sys, Box::new(SysTool::new()));
        app.open_tab_keys(tab);
        let palette = app.palette().cloned().expect("the list opens");
        assert_eq!(palette.mode, PaletteMode::TabKeys);
        assert_eq!(palette.title.as_deref(), Some("Keys: System"));
        let refresh = palette
            .entries
            .iter()
            .find(|entry| entry.label == "Refresh Now")
            .expect("the tool's own key is listed");
        assert_eq!(refresh.shortcut, "r / C-r");
    }

    #[test]
    fn test_every_tool_kind_but_the_editor_has_artwork_of_its_own() {
        // The icon table is keyed by the kind's name as a string, so a kind
        // renamed or added without a row there falls back to the generic file
        // icon in silence. The editor draws its file's icon instead.
        for kind in ToolKind::ALL {
            assert_eq!(
                crate::icons::name_for(&IconKind::Tool { kind: kind.name() })
                    != crate::icons::name_for(&IconKind::Tool { kind: "" }),
                kind != ToolKind::Editor,
                "{kind:?}"
            );
        }
    }

    #[test]
    fn test_every_tab_list_row_draws_the_icon_its_tab_would() {
        // A closed tool has no slot left to ask, so its row's icon comes from
        // what was stashed: the kind it was, not whatever is open now.
        let mut app = App::new();
        app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        let closed = app.show_tool(ToolKind::Git, Box::new(CountingTool::default()));
        app.close_tool(closed);

        app.open_tab_list();
        let palette = app.palette().expect("the list opens");
        let icons: Vec<Option<&IconKind>> = palette
            .entries
            .iter()
            .map(|entry| entry.icon.as_ref())
            .collect();
        assert_eq!(
            icons,
            [
                Some(&IconKind::Tool { kind: "grep" }),
                Some(&IconKind::Tool { kind: "git" }),
            ]
        );
    }

    #[test]
    fn test_the_stash_holds_the_last_few_closes_and_no_more() {
        // Each entry is a whole tool, so the stash is a way back from an
        // accident rather than a log of every tool ever closed.
        let mut app = App::new();
        for claimed in 0..MAX_CLOSED + 2 {
            let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
            tally(&mut app, tab, claimed);
            app.close_tool(tab);
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
        // which tool and what it was showing: "counting" twice over would be
        // two rows a reader cannot choose between.
        let mut app = App::new();
        for claimed in 1..=2 {
            let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
            tally(&mut app, tab, claimed);
            app.close_tool(tab);
        }

        app.open_tab_list();
        let palette = app.palette().expect("the list opens");
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
    fn test_picking_a_row_reopens_that_tool_and_leaves_the_rest_closed() {
        // Reopening the newest whatever was picked would make the list a
        // decoration over `C-S-u`.
        let mut app = App::new();
        for claimed in 1..=3 {
            let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
            tally(&mut app, tab, claimed);
            app.close_tool(tab);
        }
        let terminal = app.layout().focused();

        app.open_tab_list();
        let mut palette = app.palette().cloned().expect("the list opens");
        // The oldest row, which is the last one in a newest-first list.
        palette.selected = palette.filtered.len() - 1;
        app.confirm_palette_selection(&palette, None, terminal);

        assert_eq!(showing(&app).as_deref(), Some("Counting 1"));
        assert_eq!(app.closed.len(), 2, "the other two are still closed");
    }

    #[test]
    fn test_a_row_standing_for_a_dropped_tool_says_so() {
        // The list is drawn from a stash a pane dying behind it can still push
        // onto, which for a full stash drops the oldest. That row must not
        // reopen whichever tool took its place.
        let mut app = App::new();
        let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
        app.close_tool(tab);
        let gone = app.closed[0].id();
        for claimed in 0..MAX_CLOSED {
            // Each told apart from the rest, so none replaces another's row.
            let tab = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));
            tally(&mut app, tab, claimed + 1);
            app.close_tool(tab);
        }

        app.reopen_closed_by_id(gone);
        assert!(
            app.open_tabs.tools().next().is_none(),
            "nothing was reopened in its place"
        );
        assert_eq!(app.notice_text(), Some(GONE_FROM_LIST));
    }

    #[test]
    fn test_the_list_does_not_open_with_nothing_in_it() {
        // An empty panel over an empty stash says nothing about why.
        let mut app = App::new();
        app.open_tab_list();
        assert!(app.palette().is_none());
        assert_eq!(app.notice_text(), Some(NOTHING_CLOSED));
    }

    #[test]
    fn test_reopening_with_nothing_closed_says_so() {
        // A key that does nothing and says nothing reads as a broken binding.
        let mut app = App::new();
        app.reopen_last_closed();
        assert!(app.open_tabs.tools().next().is_none());
        assert_eq!(app.notice_text(), Some(NOTHING_CLOSED));
    }

    #[test]
    fn test_a_tool_report_reaches_the_status_bar_whatever_the_outcome() {
        // A tool reports beside another outcome, such as the job that re-reads
        // it, so the report cannot ride on a `Notice` outcome alone.
        let mut app = App::new();
        let tool = ReportingTool(Some(Notice::info("pushed")));
        let tab = app.show_tool(ToolKind::Git, Box::new(tool));
        app.act_on_tool_outcome(tab, ToolOutcome::Consumed);
        assert_eq!(app.notice_text(), Some("pushed"));
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
        app.press_palette_key(&Key::Named(named), &phys, tab);
    }

    /// Put the selection on the row labelled `label`.
    fn browse_to(app: &mut App, label: &str) {
        let palette = app.palette_mut().expect("the browser is up");
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
        app.open_palette(crate::model::palette::Palette::open_files(tree.0.clone()));

        browse_to(&mut app, "src/");
        browse_key(&mut app, NamedKey::Enter);

        let palette = app.palette().expect("still browsing");
        assert_eq!(palette.dir.as_ref(), Some(&tree.0.join("src")));
        assert!(
            palette.entries.iter().any(|entry| entry.label == "lib.rs"),
            "showing what is in there"
        );

        // Backspace rubs out typed text and never leaves the directory, even
        // with nothing typed.
        browse_key(&mut app, NamedKey::Backspace);
        let palette = app.palette().expect("still browsing");
        assert_eq!(palette.dir.as_ref(), Some(&tree.0.join("src")));

        // `C-h` is the way back out.
        app.modifiers = winit::keyboard::ModifiersState::CONTROL.into();
        let tab = app.layout().focused();
        let phys = winit::keyboard::PhysicalKey::Unidentified(
            winit::keyboard::NativeKeyCode::Unidentified,
        );
        app.press_palette_key(&winit::keyboard::Key::Character("h".into()), &phys, tab);
        let palette = app.palette().expect("still browsing");
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
            app.open_tabs.tool(&editor).map(|slot| slot.kind),
            Some(ToolKind::Editor)
        );
        let diff = *app.commit_companions.get(&editor).expect("a tab beside it");
        assert_eq!(
            app.open_tabs.tool(&diff).map(|slot| slot.kind),
            Some(ToolKind::Git)
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
        let git_tool = GitTool::new(tree.0.clone());
        let git_tab = app.show_tool(ToolKind::Git, Box::new(git_tool));

        let (stream, _client) = UnixStream::pair().expect("socket pair");
        app.open_waited_editor(tree.0.join("COMMIT_EDITMSG"), EditWaiter::new(stream));
        let editor = app.layout().focused();
        assert_ne!(editor, git_tab);

        app.jobs.cancel_for(git_tab);
        app.close_tool(editor);
        assert_eq!(
            app.layout().focused(),
            git_tab,
            "closing the commit editor focuses on the repo's git view"
        );
        assert!(
            app.jobs.is_busy_for(git_tab),
            "and reads it again, the commit having changed it"
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
        assert!(idle.tool_prompt().is_none());

        let mut busy = App::new();
        let tab = terminal_tab(&mut busy, true);
        assert!(busy.ask_before_closing_tab(tab));
        let label = busy.close_prompt().expect("asked").label.clone();
        assert!(label.contains("still running"), "label: {label}");
        assert!(label.contains("Close the tab?"), "label: {label}");
        assert_eq!(
            busy.close_prompt().map(|prompt| prompt.pending),
            Some(PendingClose::Tab(tab))
        );
    }

    #[test]
    fn test_quitting_with_a_program_running_asks_and_a_yes_goes_through_without_asking_again() {
        let mut app = App::new();
        terminal_tab(&mut app, true);

        assert!(app.ask_before_quitting());
        assert_eq!(
            app.close_prompt().map(|prompt| prompt.pending),
            Some(PendingClose::Quit)
        );
        assert!(!app.exit_requested, "nothing has been closed yet");

        app.handle_close_prompt_key(&Key::plain(KeyCode::Char('y')));

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

        app.handle_close_prompt_key(&Key::plain(KeyCode::Char('n')));

        assert!(!app.exit_requested);
        assert!(!app.quit_confirmed);
        assert!(app.close_prompt().is_none());
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

        app.open_git_tool();

        let git = app.layout().focused();
        assert_eq!(
            app.open_tabs.tool(&git).map(|slot| slot.kind),
            Some(ToolKind::Git)
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

        app.open_git_tool();
        let git = app.layout().focused();
        assert_eq!(
            app.open_tabs.tool(&git).map(|slot| slot.kind),
            Some(ToolKind::Git)
        );
        assert!(
            app.layout().tabs_in_pane(second).contains(&git),
            "beside the work"
        );
        assert!(!app.layout().tabs_in_pane(first).contains(&git));

        app.layout_mut().focus(first);
        app.open_git_tool();
        assert_eq!(
            app.layout().focused(),
            git,
            "the open view, not a second one"
        );
        let views = app
            .open_tabs
            .tools()
            .map(|(_, slot)| slot)
            .filter(|slot| slot.kind == ToolKind::Git)
            .count();
        assert_eq!(views, 1);
    }

    #[test]
    fn test_asking_for_the_status_again_reads_an_open_view_again() {
        // The view's watcher misses some changes, so asking for the status
        // is the way to see what it is now, from the view or from beside it.
        let (mut app, first, _) = two_panes();
        app.open_git_tool();
        let git = app.layout().focused();

        app.jobs.cancel_for(git);
        app.open_git_tool();
        assert_eq!(app.layout().focused(), git);
        assert!(app.jobs.is_busy_for(git), "asked from the view itself");

        app.jobs.cancel_for(git);
        app.layout_mut().focus(first);
        app.open_git_tool();
        assert_eq!(app.layout().focused(), git);
        assert!(app.jobs.is_busy_for(git), "asked from the pane beside it");
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

        // Open git tool for project A
        app.open_git_tool();
        let git_a = app.layout().focused();
        assert_eq!(
            app.open_tabs.tool(&git_a).map(|slot| slot.kind),
            Some(ToolKind::Git)
        );
        assert_eq!(
            app.open_tabs.tool(&git_a).unwrap().tool.root_dir(),
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

        // Open git tool for project B: should NOT reuse project A's view
        app.open_git_tool();
        let git_b = app.layout().focused();
        assert_ne!(git_a, git_b, "opens a new git view for project B");
        assert_eq!(
            app.open_tabs.tool(&git_b).unwrap().tool.root_dir(),
            Some(PathBuf::from("/path/to/project_b"))
        );

        let views = app
            .open_tabs
            .tools()
            .map(|(_, slot)| slot)
            .filter(|slot| slot.kind == ToolKind::Git)
            .count();
        assert_eq!(views, 2, "two distinct git views open for two projects");
    }

    #[test]
    fn test_the_git_view_splits_a_pane_off_when_there_is_just_one() {
        let mut app = App::new();
        let only = app.layout().focused();

        app.open_git_tool();

        let git = app.layout().focused();
        assert_eq!(
            app.open_tabs.tool(&git).map(|slot| slot.kind),
            Some(ToolKind::Git)
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
            app.open_tabs.tool(&editor).map(|slot| slot.kind),
            Some(ToolKind::Editor)
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
            app.open_tabs.tool(&editor).map(|slot| slot.kind),
            Some(ToolKind::Editor)
        );
        assert!(app.layout().tabs_in_pane(second).contains(&editor));
        assert!(!app.layout().tabs_in_pane(first).contains(&editor));
    }

    #[test]
    fn test_closing_the_commit_editor_answers_git_even_though_the_tool_is_kept_for_reopening() {
        // Regression: a closed tool is kept so it can be reopened, and the
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

        app.close_tool(editor);

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
        let listing = app.show_tool(ToolKind::Grep, Box::new(CountingTool::default()));

        app.open_editor_tool(OpenTarget::file(tree.0.join("one.txt")));
        let one = app.layout().focused();
        app.show_tab(listing);
        app.open_editor_tool(OpenTarget::file(tree.0.join("two.txt")));
        let two = app.layout().focused();

        assert_ne!(one, two, "two editor tabs");
        assert_eq!(
            app.open_tabs.tool(&one).map(|slot| slot.kind),
            Some(ToolKind::Editor)
        );
        assert_eq!(
            app.open_tabs.tool(&two).map(|slot| slot.kind),
            Some(ToolKind::Editor)
        );
    }

    #[test]
    fn test_a_file_opened_from_inside_the_editor_joins_it() {
        // The editor keeps buffers of its own; opening a file from in there
        // adds one rather than a second editor tab.
        let tree = TempTree::new("join", &["one.txt", "two.txt"]);
        let mut app = App::new();
        app.open_editor_tool(OpenTarget::file(tree.0.join("one.txt")));
        app.open_editor_tool(OpenTarget::file(tree.0.join("two.txt")));
        assert_eq!(app.open_tabs.tools().count(), 1, "one editor, not two");
    }

    #[test]
    fn test_the_browser_chord_pressed_twice_puts_it_away() {
        let mut app = App::new();
        app.open_file_browser();
        assert!(app.palette().is_some());
        app.open_file_browser();
        assert!(app.palette().is_none(), "the same chord toggles it off");
    }

    #[test]
    fn test_browsing_onto_a_file_opens_it_in_an_editor_tab() {
        use winit::keyboard::NamedKey;

        let tree = TempTree::new("open", &["README.md"]);
        let mut app = App::new();
        app.open_palette(crate::model::palette::Palette::open_files(tree.0.clone()));

        browse_to(&mut app, "README.md");
        browse_key(&mut app, NamedKey::Enter);

        assert!(app.palette().is_none(), "the browser is done");
        let tab = app.layout().focused();
        assert_eq!(
            app.open_tabs.tool(&tab).map(|slot| slot.kind),
            Some(ToolKind::Editor),
            "and the file is open in a tab of the pane"
        );
    }

    #[test]
    fn test_another_tool_opens_beside_the_first_instead_of_toggling() {
        let mut app = App::new();
        app.open_keys_tool();
        app.open_dir_tool();
        assert_eq!(
            app.open_tabs.tools().count(),
            2,
            "each page has a tab of its own"
        );
        assert_eq!(app.layout().members().len(), 3);
    }

    #[test]
    fn test_a_listing_rooted_elsewhere_does_not_stand_in_for_a_new_one() {
        let mut app = App::new();
        let work = app.layout().focused();
        let start = app.focused_start_dir();
        app.open_dir_tool_at(PathBuf::from("/"));
        app.show_tab(work);

        app.open_dir_tool();
        let opened = app.layout().focused();
        app.show_tab(work);
        app.open_dir_tool();

        assert_eq!(
            app.open_tabs
                .tool(&opened)
                .and_then(|slot| slot.tool.root_dir()),
            Some(start),
            "the new listing is rooted where the work tab is looking"
        );
        assert_eq!(app.layout().focused(), opened, "asked again, it is reused");
        assert_eq!(app.open_tabs.tools().count(), 2);
    }

    #[test]
    fn test_ctrl_shift_c_with_nothing_selected_reaches_the_tool() {
        // Copy is a global chord, checked ahead of every tool. With nothing
        // selected it copies nothing, so a tool binding the same chord (the
        // Git tool commits on it) has to be the one that gets it.
        let (mut app, pane) = app_with_open_tool();
        app.keymap = crate::model::input::Keymap::default();
        app.modifiers = winit::event::Modifiers::from(
            winit::keyboard::ModifiersState::CONTROL | winit::keyboard::ModifiersState::SHIFT,
        );
        app.handle_key(
            winit::event::ElementState::Pressed,
            winit::keyboard::Key::Character("C".into()),
            winit::keyboard::PhysicalKey::Code(winit::keyboard::KeyCode::KeyC),
        );
        assert_eq!(
            app.open_tabs.tool(&pane).unwrap().tool.title(),
            "Counting 1"
        );
    }

    #[test]
    fn test_a_super_chord_and_a_shifted_enter_are_not_the_plain_keys_to_a_tool() {
        let (mut app, pane) = app_with_open_tool();
        let super_x = Key {
            sup: true,
            ..press(KeyCode::Char('x'))
        };
        assert!(!app.offer_key_to_tool(pane, &super_x), "s-x is not x");
        let shifted_enter = Key {
            shift: true,
            ..press(KeyCode::Enter)
        };
        assert!(!app.offer_key_to_tool(pane, &shifted_enter));
        assert!(app.offer_key_to_tool(pane, &press(KeyCode::Char('x'))));
    }

    #[test]
    fn test_declined_key_falls_through_to_the_host() {
        let (mut app, pane) = app_with_open_tool();
        assert!(app.offer_key_to_tool(pane, &press(KeyCode::Char('x'))));
        assert!(!app.offer_key_to_tool(pane, &press(KeyCode::Char('z'))));
    }

    #[test]
    fn test_a_tool_closing_itself_closes_its_tab() {
        let (mut app, pane) = app_with_open_tool();
        assert!(app.offer_key_to_tool(pane, &press(KeyCode::Char('q'))));
        assert!(app.open_tabs.tools().next().is_none(), "the page is gone");
        assert_eq!(app.layout().members().len(), 1, "the terminal tab is not");
    }

    #[test]
    fn test_a_tool_running_a_command_closes_first() {
        // The tool is done once it has named the command, so the command
        // runs against the tab shown in its place.
        let (mut app, pane) = app_with_open_tool();
        let tabs_before = app.layout().members().len();

        assert!(app.offer_key_to_tool(pane, &press(KeyCode::Char('r'))));
        assert!(app.open_tabs.tools().next().is_none(), "the page closed");
        assert_eq!(
            app.layout().members().len(),
            tabs_before,
            "and the command opened a tab in its place"
        );
    }

    #[test]
    fn test_a_pane_with_no_tool_is_never_offered_a_tool_key() {
        // Keys for an uncovered pane must reach its terminal, so the lookup has
        // to miss rather than find some other pane's tool.
        let (mut app, covered) = app_with_open_tool();
        let other = TabId(covered.0 + 1);
        assert!(!app.offer_key_to_tool(other, &press(KeyCode::Char('x'))));
    }

    #[test]
    fn test_alt_z_toggles_wrapping_without_reaching_the_tool() {
        // Wrapping belongs to the pane rather than any one tool, so the host
        // spends the toggle itself: the key is claimed rather than declined,
        // which would otherwise hand it to the ordinary keymap.
        let (mut app, pane) = app_with_open_tool();
        let mut key = press(KeyCode::Char('z'));
        key.alt = true;
        assert!(app.offer_key_to_tool(pane, &key), "the toggle is spent");
        assert!(app.tool_wrap);
        assert!(app.offer_key_to_tool(pane, &key));
        assert!(!app.tool_wrap);
    }

    /// A tool that asks one question, sharing the answer it hears with the
    /// test that opened it.
    struct AskingTool {
        heard: Rc<RefCell<Vec<Option<String>>>>,
    }

    impl Tool for AskingTool {
        fn title(&self) -> String {
            "Asking".to_string()
        }

        fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
            Frame::new(vec![vec![Span::plain("asking")]])
        }

        fn on_key(&mut self, _key: &Key) -> ToolOutcome {
            ToolOutcome::Prompt(PromptRequest {
                initial: "seed".to_string(),
                label: "Name: ".to_string(),
                mode: PromptMode::Text,
                tag: "ask",
            })
        }

        fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
            self.heard.borrow_mut().push(reply.answer);
            ToolOutcome::Consumed
        }
    }

    fn app_asking() -> (App, TabId, Rc<RefCell<Vec<Option<String>>>>) {
        let mut app = App::new();
        let heard: Rc<RefCell<Vec<Option<String>>>> = Rc::default();
        let tab = app.show_tool(
            ToolKind::Grep,
            Box::new(AskingTool {
                heard: Rc::clone(&heard),
            }),
        );
        app.offer_key_to_tool(tab, &press(KeyCode::Char('x')));
        (app, tab, heard)
    }

    /// Type `text` into the open question and answer it with Enter.
    fn answer(app: &mut App, text: &str) {
        for c in text.chars() {
            app.handle_prompt_key(&press(KeyCode::Char(c)));
        }
        app.handle_prompt_key(&press(KeyCode::Enter));
    }

    #[test]
    fn test_a_close_question_goes_with_the_tab_it_was_about() {
        // Regression: the question went when its tab did, but what it would
        // close stayed pending, and the next tool question answered was
        // taken as the answer to it.
        let (mut app, asking, heard) = app_asking();
        app.overlay = None;
        let unsaved = app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        assert!(app.ask_before_closing_tab(unsaved));
        app.close_tab_now(unsaved, Closing::Keep);
        assert!(app.close_prompt().is_none(), "nothing left to ask about");

        app.offer_key_to_tool(asking, &press(KeyCode::Char('x')));
        answer(&mut app, "y");
        assert_eq!(heard.borrow().as_slice(), [Some("seedy".to_string())]);
        assert!(!app.exit_requested);
    }

    #[test]
    fn test_a_tool_question_cannot_hide_the_hosts_close_question() {
        let (mut app, asking, heard) = app_asking();
        app.overlay = None;
        let unsaved = app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        assert!(app.ask_before_closing_tab(unsaved));

        app.offer_key_to_tool(asking, &press(KeyCode::Char('x')));
        assert!(app.close_prompt().is_some(), "the host's question stays up");
        assert_eq!(
            heard.borrow().as_slice(),
            [None],
            "and the tool hears that its own went unanswered"
        );
    }

    #[test]
    fn test_the_hosts_question_tells_the_tool_question_it_replaces() {
        // Regression: the close question took the prompt's place without a
        // word to the tool, which went on waiting for an answer.
        let (mut app, _, heard) = app_asking();
        let unsaved = app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        assert!(app.ask_before_closing_tab(unsaved));
        assert_eq!(heard.borrow().as_slice(), [None]);
        assert!(app.tool_prompt().is_none());
    }

    #[test]
    fn test_a_reload_over_unsaved_edits_asks_first_and_a_yes_carries_it_out() {
        // Regression: a reload, from the palette or another process, dropped
        // edits on no disk without asking.
        let mut app = App::new();
        assert!(
            !app.ask_before_reloading(),
            "nothing to lose, nothing asked"
        );
        app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        assert!(app.ask_before_reloading());
        let label = app.close_prompt().expect("asked").label.clone();
        assert!(label.contains("main.rs"), "label: {label}");

        app.handle_close_prompt_key(&press(KeyCode::Char('y')));
        assert!(app.pending_reload, "the reload goes ahead");
        assert!(app.reload_confirmed, "without asking again");
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
        let text = input_dialog("Name: ", "seed", 4, PromptMode::Text, Vec::new());
        assert_eq!(text.input.as_deref(), Some("seed"));

        let confirm = input_dialog(
            "Delete 3 entries? (y/n) ",
            "",
            0,
            PromptMode::Confirm,
            ButtonGroup::confirm().to_view(),
        );
        assert_eq!(confirm.input, None);
        assert_eq!(confirm.label, "Delete 3 entries?", "the buttons say y/n");
        assert_eq!(confirm.buttons.len(), 2, "confirmation has buttons");
        assert!(confirm.buttons[0].focused, "first button is focused");
        assert_eq!(
            confirm.keys.first().map(|(key, _)| key.as_str()),
            Some("y"),
            "a dialog taking one key says which"
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
        assert!(app.tool_prompt().is_none(), "the prompt closes on Enter");
        assert!(app.open_tabs.is_tool(&pane), "and the page stays open");
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
    fn test_confirmation_prompt_tab_switches_buttons_and_enter_submits_focused() {
        let (mut app, pane, heard) = app_asking();
        app.act_on_tool_outcome(
            pane,
            ToolOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: "Delete file?".to_string(),
                mode: PromptMode::Confirm,
                tag: "delete",
            }),
        );
        heard.borrow_mut().clear();
        let view = app.input_view().expect("confirmation dialog");
        assert_eq!(view.buttons.len(), 2);
        assert!(view.buttons[0].focused, "Yes starts focused");

        // Tab moves focus to No
        app.handle_prompt_key(&press(KeyCode::Tab));
        let view = app.input_view().expect("still open");
        assert!(!view.buttons[0].focused);
        assert!(view.buttons[1].focused, "No is now focused");

        // Enter submits the focused button (No -> None)
        app.handle_prompt_key(&press(KeyCode::Enter));
        assert!(app.input_view().is_none(), "dialog closed");
        assert_eq!(heard.borrow().as_slice(), [None], "declined via focused No button");
    }

    #[test]
    fn test_close_prompt_tab_switches_buttons_and_enter_submits_focused() {
        let mut app = App::new();
        let tab = app.show_tool(ToolKind::Editor, Box::new(UnsavedTool("main.rs")));
        app.close_pane(tab);
        assert!(app.close_prompt().is_some());
        let view = app.input_view().expect("dialog open");
        assert!(view.buttons[0].focused, "Yes starts focused");

        // Tab moves to No
        app.handle_close_prompt_key(&press(KeyCode::Tab));
        let view = app.input_view().expect("dialog stays open");
        assert!(view.buttons[1].focused, "No is now focused");

        // Enter on No cancels the close
        app.handle_close_prompt_key(&press(KeyCode::Enter));
        assert!(app.close_prompt().is_none(), "dialog closed");
        assert_eq!(app.layout().shown_tabs().len(), 1, "pane was not closed");
    }

    #[test]
    fn test_a_saved_tool_tab_opens_again_where_it_was() {
        let dir = std::env::temp_dir().join(format!("winter-restore-pages-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("notes.md");
        std::fs::write(&file, "one\ntwo\nthree\n").expect("write");
        let mut app = App::new();
        app.show_tool(ToolKind::Dir, Box::new(DirTool::new(dir.clone())));
        app.open_path_tool(OpenTarget::at_line(file.clone(), 3));
        let saved = app.tool_sessions();
        assert_eq!(saved.len(), 2, "both pages say where they were: {saved:?}");

        let mut fresh = App::new();
        for tool in &saved {
            assert!(
                fresh.restore_tool(TabId(100 + tool.id as u64), tool),
                "{tool:?}"
            );
        }
        let editor = saved
            .iter()
            .find(|tool| tool.kind == ToolKind::Editor.name())
            .expect("the editor");
        assert_eq!(editor.line, Some(3), "at the line it was on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_restored_git_tab_asks_git_for_its_repository() {
        let dir = std::env::temp_dir().join(format!("winter-restore-git-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let tool = ToolSession {
            id: 7,
            kind: ToolKind::Git.name().to_string(),
            line: None,
            path: dir.to_string_lossy().to_string(),
        };
        let mut app = App::new();
        assert!(app.jobs.busy_tabs().is_empty());
        assert!(app.restore_tool(TabId(7), &tool));
        assert!(
            !app.jobs.busy_tabs().is_empty(),
            "its first read is under way, not awaited"
        );

        let dir_tool = ToolSession {
            kind: ToolKind::Dir.name().to_string(),
            ..tool
        };
        let mut app = App::new();
        assert!(app.restore_tool(TabId(8), &dir_tool));
        assert!(
            app.jobs.busy_tabs().is_empty(),
            "a listing reads on its own"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_up_brings_back_the_last_answer_to_the_same_question() {
        let (mut app, pane, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Char('!')));
        app.handle_prompt_key(&press(KeyCode::Enter));
        let asked = || crate::model::tool::PromptRequest {
            initial: String::new(),
            label: "Name: ".to_string(),
            mode: PromptMode::Text,
            tag: "ask",
        };
        app.act_on_tool_outcome(pane, ToolOutcome::Prompt(asked()));
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
    fn test_a_secret_prompt_shows_dots_and_is_never_recalled() {
        let (mut app, pane, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Escape));
        let secret = || crate::model::tool::PromptRequest {
            initial: String::new(),
            label: "Password: ".to_string(),
            mode: PromptMode::Secret,
            tag: "ask",
        };
        app.act_on_tool_outcome(pane, ToolOutcome::Prompt(secret()));
        for c in "pw".chars() {
            app.handle_prompt_key(&press(KeyCode::Char(c)));
        }
        assert_eq!(
            app.input_view().and_then(|view| view.input).as_deref(),
            Some("••"),
            "how much is typed shows, never what"
        );
        app.handle_prompt_key(&press(KeyCode::Enter));
        assert_eq!(heard.borrow().last(), Some(&Some("pw".to_string())));

        // The same question asked in the open finds nothing to bring back.
        let text = crate::model::tool::PromptRequest {
            mode: PromptMode::Text,
            ..secret()
        };
        app.act_on_tool_outcome(pane, ToolOutcome::Prompt(text));
        app.handle_prompt_key(&press(KeyCode::Up));
        assert_eq!(
            app.input_view().and_then(|view| view.input).as_deref(),
            Some(""),
            "a password is not kept to be brought back"
        );
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
    fn test_a_list_answers_the_tool_the_way_a_prompt_does() {
        // The palette stands in for the prompt when the answer is one of a
        // known set: what is chosen comes back under the same tag.
        let (mut app, pane, heard) = app_asking();
        app.overlay = None;
        app.act_on_tool_outcome(
            pane,
            ToolOutcome::Pick(crate::model::tool::PickRequest {
                icons: Vec::new(),
                items: vec!["main".to_string(), "feature".to_string()],
                label: "Checkout".to_string(),
                tag: "ask",
            }),
        );
        assert!(app.palette().is_some(), "the list is showing");
        assert_eq!(
            app.tool_pick().map(|pick| pick.label.as_str()),
            Some("Checkout"),
            "and it says what it is a list of"
        );

        app.answer_tool_pick(Some("feature".to_string()));
        assert!(app.tool_pick().is_none(), "the question is answered");
        assert_eq!(heard.borrow().as_slice(), [Some("feature".to_string())]);
    }

    #[test]
    fn test_a_list_closed_without_choosing_says_so() {
        let (mut app, pane, heard) = app_asking();
        app.overlay = None;
        app.act_on_tool_outcome(
            pane,
            ToolOutcome::Pick(crate::model::tool::PickRequest {
                icons: Vec::new(),
                items: vec!["main".to_string()],
                label: "Checkout".to_string(),
                tag: "ask",
            }),
        );
        app.answer_tool_pick(None);
        assert_eq!(heard.borrow().as_slice(), [None]);
    }

    #[test]
    fn test_escape_cancels_a_prompt_and_says_so() {
        // The tool has to hear the cancellation, not just stop hearing: an
        // operation waiting on an answer would otherwise stay half-started.
        let (mut app, _, heard) = app_asking();
        app.handle_prompt_key(&press(KeyCode::Escape));
        assert!(app.tool_prompt().is_none());
        assert_eq!(heard.borrow().as_slice(), [None]);
    }

    #[test]
    fn test_closing_the_tool_takes_its_question_with_it() {
        // A prompt left behind would deliver its answer to a tool that is gone,
        // or worse, to whatever tool opened next in that pane.
        let (mut app, pane, heard) = app_asking();
        assert!(app.tool_prompt().is_some());
        app.close_tool(pane);
        assert!(app.tool_prompt().is_none());
        assert!(heard.borrow().is_empty(), "and answers nothing");
    }

    #[test]
    fn test_resize_leaves_a_tool_tab_showing_its_tool() {
        // Resizing walks every laid-out pane and reflows the grids of the
        // terminals; a tool tab, which has none, must survive the pass.
        let (mut app, pane) = app_with_open_tool();
        app.resize_all_panes();
        assert!(app.open_tabs.is_tool(&pane));
    }

    #[test]
    fn test_a_tool_is_ticked_once_the_deadline_it_asked_for_passes() {
        // A tool says when it next wants a tick in its own time, a moment
        // after the loop took its own: "at once" and "in a little while" must
        // still come due, rather than staying just ahead of the loop for ever.
        for after in [Duration::ZERO, Duration::from_millis(10)] {
            let ticks = Rc::new(RefCell::new(0));
            let mut app = App::new();
            let tool = PollingTool {
                after,
                ticks: Rc::clone(&ticks),
            };
            app.show_tool(ToolKind::Proc, Box::new(tool));
            let end = Instant::now() + TICK_WAIT;
            while *ticks.borrow() == 0 && Instant::now() < end {
                let wake = app.tick_tools(Instant::now());
                let pause =
                    wake.map_or(TICK_STEP, |at| at.saturating_duration_since(Instant::now()));
                std::thread::sleep(pause.min(TICK_STEP));
            }
            assert!(
                *ticks.borrow() > 0,
                "a tool asking for a tick {after:?} out is ticked"
            );
        }
    }
}

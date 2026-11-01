//! Editors: the tabs Winter paints itself instead of running a terminal in
//! them. An editor takes its own keys and hands the host a [`Frame`] of styled
//! rows each paint; the rows themselves are [`super::paint`]'s.
//!
//! The [`Editor`] trait and what it answers with live here; what goes back
//! and forth through it is split by what it is about, and re-exported here so
//! every editor names it from one place:
//!
//! - `commands`: the names of the commands editors take.
//! - `compare`: comparing and merging texts.
//! - `jobs`: the work an editor asks the host to run, and its replies.
//! - `popup`: completion, info cards, and toolbars drawn over the rows.
//! - `prompt`: the questions an editor asks.
//! - [`refresh`]: the schedule of an editor that keeps itself current.
//! - `search`: searching the rows, and the jumps that land somewhere.

mod commands;
mod compare;
mod jobs;
mod menu;
mod popup;
mod prompt;
pub mod refresh;
mod search;

use std::any::Any;
use std::ops::RangeInclusive;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub use super::notify::Notice;

use super::guard::Guarded;

use super::input::table::KeyHelp;
use super::input::{Key, MacroStep};
use super::paint::{Caret, ColumnName, Frame, GutterLane, IconKind, Point, Row};
use super::picture::{Picture, Tile};
use super::textedit::modal::EditMode;

pub use commands::*;
pub use compare::{CompareRequest, DiffMerge, DiffRequest, DiffText, FileShape, GitViewRequest};
pub use jobs::{
    BuildStart, CommandOutput, CommandRequest, FileOps, JobReply, JobRequest, SpawnRequest,
};
pub use menu::{menu_groups, menu_of, present, EditorMenuItem, MenuRun};
pub use popup::{
    CompletionItem, CompletionPopup, DropList, InfoCard, Popups, TokenInspectData, Toolbar,
};
pub use prompt::{
    PickPlacement, PickQuestion, PickRequest, PromptMode, PromptReply, PromptRequest,
};
pub use search::{
    contains_match, find_match, match_spans, row_search_report, text_regex, JumpStep, JumpTarget,
    SearchHit, SearchOptions, SearchPattern, SearchReport, SearchRequest, SearchResult,
};

// ========================================================================
// Data Structures
// ========================================================================

/// A command an editor runs by name, which its block in `keybindings.kdl` can
/// bind to a chord or a sequence.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EditorCommand {
    /// What the hint and the Keys editor call it.
    pub label: &'static str,
    /// The name a binding gives it, and [`Page::run_command`] is handed.
    pub name: &'static str,
}

/// What an editor waiting on the second key of a command offers: what the
/// sequence is called, and the keys that complete it with what each does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EditorHint {
    /// The continuations, as `(key, what it does)`.
    pub items: Vec<(String, String)>,
    /// Whether this is a menu to choose from, as Git's popups are, shown at
    /// once, rather than the keys a prefix can go on with, shown only once
    /// typing has stalled on it as every other key hint is.
    pub menu: bool,
    /// What the sequence is called, shown as the hint's heading.
    pub title: String,
}

/// What an editor puts in the window's status bar while it has the keyboard:
/// where the keys stand, rather than what the editor shows. What the editor
/// shows stays in its own rows.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EditorStatus {
    /// The editor's own mode, drawn where a terminal's is. `None` for an editor
    /// with no mode of its own, which the bar marks as an editor.
    pub mode: Option<EditMode>,
    /// Short facts about where the cursor is and what is part-typed, drawn
    /// at the bar's right edge in order: `12:5`, `340 lines`, `"a`.
    pub segments: Vec<String>,
}

/// A file's text as the editor holding it has it, for a view following it
/// (a Markdown preview): a number that moves whenever the text does, where
/// the cursor is, and the lines, left out where the asker already has them.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SharedText {
    /// The cursor's line, from zero.
    pub cursor: usize,
    /// The text, or `None` where it has not changed since the revision the
    /// asker named.
    pub lines: Option<Vec<String>>,
    /// The text's revision.
    pub revision: usize,
}

/// What the host should do with a key the editor was offered.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorOutcome {
    /// Close the editor, and the pane holding it.
    Close,
    /// Show the differences between two files, in a view beside this one.
    Compare(CompareRequest),
    /// Put `rows` on the clipboard as rich text in the theme's colors, for a
    /// document or a slide, with `text` for what takes only plain text.
    CopyStyled {
        /// The lines, as they are painted.
        rows: Vec<super::paint::Row>,
        /// The same lines as plain text.
        text: String,
    },
    /// The editor acted on the key.
    Consumed,
    /// Read a git view in a tab of its own, or go to the tab already
    /// showing it.
    GitView(GitViewRequest),
    /// The editor has no binding for it, so resolve it as an ordinary key.
    Ignored,
    /// The editor asks the host to open this path with the system handler.
    OpenExternal(PathBuf),
    /// The editor asks the host to stop the work it started.
    CancelJobs,
    /// The editor asks the host to do slow work for it.
    Job(JobRequest),
    /// The editor holds a place for what is chosen to open there, and it
    /// is chosen: the host opens it in the editor's tab, in its directory.
    Launch(Launch),
    /// The editor asks the host to read an answer from the user.
    Prompt(PromptRequest),
    /// The editor asks the host to have the user choose from a list, rather
    /// than type. The answer comes back as a [`PromptReply`] under the same
    /// tag, so a question asked either way is answered in one place.
    Pick(PickRequest),
    /// The editor asks the host to run one of its own commands, named the way the
    /// command palette names it, and is done: it closes first, as the
    /// command list does once a command is picked.
    RunAction(String),
    /// The editor asks the host to run one of its own commands for it, and
    /// stays open: a file's editor opening its preview or building its
    /// document.
    RunActionHere(String),
    /// The editor asks the host to record or replay a keyboard macro.
    Macro(MacroStep),
    /// The editor asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The editor asks the host to run this command line as a task, in a
    /// Compile tab of its own: what a code lens's Run asks for.
    RunTask(BuildStart),
    /// The editor asks the host to type this into a terminal beside it and
    /// run it: code sent to the REPL or shell there, as Emacs sends a region
    /// to its inferior process.
    SendToTerminal(String),
    /// The editor asks the host to copy this to the clipboard.
    Yank(String),
    /// The editor asks the host to put what a delete took on the clipboard,
    /// saying nothing.
    Cut(String),
    /// The editor asks the host to copy text to the clipboard and say
    /// something of its own, for text too long to echo back the way
    /// [`Self::Yank`] does.
    Copy {
        /// What to say once it is copied.
        notice: String,
        /// What to copy.
        text: String,
    },
    /// The editor asks the host to open this file for editing, in the editor
    /// the app carries; a directory is listed in Dir.
    OpenPath(OpenTarget),
    /// The editor is starting a build: the host saves the files being
    /// edited first, and takes it as the newest build, the one rerun and
    /// next-error mean. The editor starts the build on its next tick.
    Build(BuildStart),
    /// The editor asks the host to open this file beside the editor rather
    /// than over it, so what it was opened from stays in sight.
    OpenBeside(OpenTarget),
    /// The editor wrote this file to disk, and acted on the key that asked
    /// it to: the host may follow a save up, rebuilding the document it
    /// belongs to.
    Wrote(PathBuf),
    /// The editor asks the host to open this file in `$EDITOR`, in a pane of
    /// its own, for the editing the app's own editor deliberately cannot do.
    SpawnEditor(OpenTarget),
    /// Compare or merge texts in the diff tool, in a tab of its own.
    Diff(DiffRequest),
    /// The editor asks the host for what is on the clipboard, which comes back
    /// through [`Page::on_paste`].
    Paste,
    /// The editor asks the host to show the PDF at `path`, in the tab
    /// already showing it or a new one, with `mark` brought into view.
    ShowInPdf {
        /// Whether in the other pane rather than this one.
        beside: bool,
        /// Where on which page, or `None` to open the PDF as it is.
        mark: Option<PdfMark>,
        /// The PDF.
        path: PathBuf,
    },
    /// The editor asks the host to open this file in the hex editor.
    OpenHex(PathBuf),
}

/// What a pane's launcher opens in its place.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Launch {
    /// A directory listing.
    Dir,
    /// The file picker, over what is under the directory.
    Files,
    /// The working tree's state.
    Git,
    /// A shell.
    Terminal,
}

/// A box on a page of a PDF, for the viewer to bring into view and mark:
/// where SyncTeX says a line of LaTeX was set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PdfMark {
    /// How tall, in points.
    pub height: f32,
    /// The left edge, in points from the page's.
    pub left: f32,
    /// The page, from zero.
    pub page: usize,
    /// The top edge, in points down from the page's.
    pub top: f32,
    /// How wide, in points; zero for a spot with no width of its own.
    pub width: f32,
}

/// A place in a file an [`EditorRequest::Edit`] names.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct EditPoint {
    /// The column, in UTF-16 code units, as a language server counts it:
    /// only the editor that comes to hold the file can count it in
    /// characters, from the line's text.
    pub column: u32,
    /// The line, from zero.
    pub line: u32,
}

/// One edit of an [`EditorRequest::Edit`]: the text from `start` to `end`,
/// not included, replaced by `new_text`.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct FileEdit {
    /// Where the text replaced ends, not included.
    pub end: EditPoint,
    /// What goes in.
    pub new_text: String,
    /// Where the text replaced starts.
    pub start: EditPoint,
}

/// What an editor asks of the window that only the window can do, since it
/// reaches past the editor's own file.
#[derive(Clone, Debug, PartialEq)]
pub enum EditorRequest {
    /// Make `edits` to the file at `path`, in the buffer holding it, opened
    /// buried where no buffer does.
    Edit {
        /// The edits, in the order they came.
        edits: Vec<FileEdit>,
        /// The file they are to.
        path: PathBuf,
    },
    /// Show the file `target` names, at its place: a jump into another file.
    Open(OpenTarget),
    /// Bind command `command`, of block `block` of the user's keybindings, to
    /// `chords`, replacing what they bound it to; with no `chords`, take
    /// their binding of it away, so the built-in one is in force again.
    Rebind {
        /// The block it is bound in: `window` for the window's own commands,
        /// an editor's block for that editor's.
        block: String,
        /// The chords, each one or more keys as `keybindings.kdl` spells them.
        chords: Vec<String>,
        /// The command's name.
        command: String,
    },
    /// Stop the window's workspace setting `key`, so the user's value in
    /// `settings.kdl` is in force again.
    ResetSetting(String),
    /// Put a setting in force at once, and keep it where its scope says.
    Setting(SettingEdit),
}

/// A setting changed on the settings page.
#[derive(Clone, Debug, PartialEq)]
pub struct SettingEdit {
    /// The setting's key, as the page names its row.
    pub key: String,
    /// Where it is kept: `settings.kdl`, or the window's workspace.
    pub scope: crate::settings_page::Scope,
    /// Its new value, as `settings.kdl` writes it.
    pub value: String,
}

/// The window's look, for an editor that draws a page of its own in it: the
/// theme's colors, how many screen pixels a logical one is, and how rows of
/// highlighted code read as HTML in those colors.
pub struct Appearance<'a> {
    /// Screen pixels to a logical pixel.
    pub scale: f32,
    /// `rows` as a `<pre>` of spans in the theme's colors.
    pub styled_html: &'a dyn Fn(&[Row]) -> String,
    /// The theme in effect.
    pub theme: &'a winter_render::Theme,
}

/// A file an editor wants opened, and where in it to land.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenTarget {
    /// The column to put the cursor on, counting from one, when the line
    /// is known and so is the column.
    pub column: Option<usize>,
    /// The line to put the cursor on, when the editor knows one.
    pub line: Option<usize>,
    /// The file to open.
    pub path: PathBuf,
}

/// Where an editor's view sits in its content, for drawing its scrollbar
/// and navigating with the mouse.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScrollPosition {
    /// How many units (lines, rows, or pixels) the view has scrolled from the top.
    pub offset: usize,
    /// Total number of units in the content.
    pub total: usize,
    /// How many units are visible in the pane.
    pub visible: usize,
}

impl ScrollPosition {
    /// Create a new scroll position.
    pub fn new(offset: usize, visible: usize, total: usize) -> Self {
        Self {
            offset,
            total,
            visible,
        }
    }

    /// Convert to `(scroll_offset, scrollback_len)` for rendering on a grid with `grid_rows`.
    pub fn to_scroll_params(self, grid_rows: usize) -> (usize, usize) {
        if self.total <= self.visible || self.visible == 0 || grid_rows == 0 {
            return (0, 0);
        }
        let total_virtual =
            ((self.total as f64 / self.visible as f64) * grid_rows as f64).round() as usize;
        let total_virtual = total_virtual.max(grid_rows);
        let scrollback_len = total_virtual.saturating_sub(grid_rows);
        if scrollback_len == 0 {
            return (0, 0);
        }
        let top_virtual =
            ((self.offset as f64 / self.total as f64) * total_virtual as f64).round() as usize;
        let scroll_offset = scrollback_len.saturating_sub(top_virtual);
        (scroll_offset, scrollback_len)
    }
}

// ========================================================================
// Traits
// ========================================================================

/// What a tab Winter draws itself holds: a page paints itself as rows of
/// styled text and gets first refusal on keys while its tab is focused.
///
/// A page shows Winter's own state (its settings, its keys, its notices) and
/// holds nothing of its own to keep: closing its tab lets it go, and opening
/// it again builds it anew. An [`Editor`] is a page whose contents are a
/// buffer, to share among tabs, bury, and bring back.
///
/// The trait holds what any page may do. What only one kind does (the text
/// editor's language servers, Git's status view) is that kind's own method,
/// reached through `with_kind`.
pub trait Page: AsAny {
    /// A short label for the pane title.
    fn title(&self) -> String;

    /// This page as the editor it is, where its contents are a buffer:
    /// `None` for a page holding none.
    fn as_editor(&self) -> Option<&dyn Editor> {
        None
    }

    /// The same, to change.
    fn as_editor_mut(&mut self) -> Option<&mut dyn Editor> {
        None
    }

    /// What the tab's icon depicts, when the editor shows something with a type
    /// of its own: the file an editor has open. `None` draws the icon of the
    /// editor's kind.
    fn icon(&self) -> Option<IconKind> {
        None
    }

    /// Whether the editor's cursor picks whole rows, as a listing's does: a
    /// view of it keeps its column at the start of the line, outside typing,
    /// so stepping between rows lands each at its name.
    fn picks_rows(&self) -> bool {
        false
    }

    /// Hear which directories the other panes on screen are looking at, just
    /// before a key arrives, for an editor that offers them as a destination
    /// (a listing copying into the listing beside it).
    fn set_neighbors(&mut self, _dirs: Vec<PathBuf>) {}

    /// The rows to draw this frame, given how many the pane can show below any
    /// rows it pins, and how wide its content is (the pane less its gutter,
    /// which the editor fills through [`Frame::gutter`] and never paints
    /// itself): an editor longer than its pane
    /// returns the window it wants visible, so scrolling stays where the
    /// cursor is, and when `wrap` is set a row wider than `cols` wraps onto
    /// the next screen row — folding at its last word boundary — rather than
    /// clipping at the pane's edge.
    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame;

    /// The lanes of the editor's gutter, left to right: none, and so no
    /// gutter, for one with nothing to show beside its rows.
    fn gutter_lanes(&self) -> Vec<GutterLane> {
        Vec::new()
    }

    /// Offer a key to the editor.
    fn on_key(&mut self, key: &Key) -> EditorOutcome;

    /// Run one of the editor's own commands by the name its `keybindings.kdl`
    /// block binds it under, or `None` for a name it does not know.
    fn run_command(&mut self, _name: &str) -> Option<EditorOutcome> {
        None
    }

    /// Offer a press or a drag of the pointer to the editor. Editors that take no
    /// pointer never implement it, and the click falls through to the host.
    fn on_mouse(&mut self, _at: Point) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Offer a double-click to the editor, after the press that ends it was
    /// offered through [`Self::on_mouse`]. By default nothing more happens.
    fn on_double_click(&mut self, _at: Point) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Whether the editor glides its own scrolling, by the pixel: the host
    /// then hands it a wheel's whole turn at once, rather than feeding it the
    /// rows a frame at a time.
    fn scrolls_smoothly(&self) -> bool {
        false
    }

    /// Offer a touchpad's scroll to the editor in the pixels it measures,
    /// positive upwards, for an editor that follows it exactly. By default
    /// it is declined, and the host offers it as rows.
    fn on_scroll_pixels(&mut self, _pixels: f32) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Keep `rows` screen rows above and below the cursor's as the view
    /// follows it, where the rows allow: the window's `scroll-margin`, told
    /// before each paint. Editors with no cursor to follow never implement it.
    fn set_scroll_margin(&mut self, _rows: usize) {}

    /// Offer a turn of the wheel to the editor, in rows, positive upwards.
    /// Editors that do not scroll themselves never implement it.
    fn on_scroll(&mut self, _lines: isize) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Where the editor's view sits in its content, for drawing the pane's
    /// scrollbar and navigating with the mouse. `None` for an editor with
    /// no scrollable content.
    fn scroll_position(&self) -> Option<ScrollPosition> {
        None
    }

    /// Scroll to the given row or unit offset (e.g. from clicking or dragging
    /// the scrollbar).
    fn scroll_to(&mut self, _offset: usize) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Offer a press of the mouse's back (`back`) or forward side button,
    /// to step through the places the editor has been. Editors that keep no
    /// such places never implement it.
    fn on_side_button(&mut self, _back: bool) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Hear the pointer rest on the cell `at` with no button held, or leave
    /// the editor's pane (`None`). Editors that show nothing on hover never
    /// implement it.
    fn on_hover(&mut self, _at: Option<Point>) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Hand back what the clipboard holds, for an editor that asked for it.
    fn on_paste(&mut self, _text: String) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// What the editor is waiting for, when it is part-way through a command
    /// that takes more than one key: the sequence's name and what the next
    /// key may be. The host draws it the way it draws its own key hints, so a
    /// multi-key command looks the same wherever it is being typed. Editors
    /// with no multi-key commands never implement it.
    fn hint(&self) -> Option<EditorHint> {
        None
    }

    /// A transient popup window being shown: action commands and parameter switches.
    /// Drawn on a content-fitted transient window rather than a full-size HUD.
    fn transient(&self) -> Option<winter_render::TransientView> {
        None
    }

    /// Select row `row` of the transient being shown, counting its switches
    /// then its commands, as the pointer moving over it does: the row `RET`
    /// then runs. Editors with no transient never implement it.
    fn select_transient_row(&mut self, _row: usize) {}

    /// Every key the editor answers in the state it is in, read from the same
    /// key tables its dispatch looks keys up in, for the host's help list.
    fn keys(&self) -> Vec<KeyHelp> {
        Vec::new()
    }

    /// Hand back the answer to a prompt the editor asked for. Editors that never
    /// ask never implement it.
    fn on_prompt(&mut self, _reply: PromptReply) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// Hand back intermediate, debounced changes to what has been typed into an
    /// open prompt, allowing editors to perform iterative search or live filtering.
    fn on_prompt_input(&mut self, _query: &str, _tag: &'static str) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Hand back the result of work the editor asked for.
    fn on_job(&mut self, _reply: JobReply) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// When the editor next wants [`Page::on_tick`], for an editor that keeps
    /// itself current without being asked. The host wakes for the earliest
    /// deadline of any editor showing. Editors whose content changes only on a key
    /// never implement it.
    fn next_tick(&self) -> Option<Instant> {
        None
    }

    /// The earliest deadline [`Page::next_tick`] gave that the editor has
    /// since called off, as a timer is cancelled: the host, which keeps the
    /// earliest deadline it was given, drops that one rather than waking
    /// for nothing. `None` where nothing was called off since last asked.
    fn take_cancelled_tick(&mut self) -> Option<Instant> {
        None
    }

    /// The deadline from [`Page::next_tick`] has passed: refresh whatever the
    /// editor shows, usually by asking for a job.
    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// What an extension's view asked of Winter while taking keys, each
    /// with the extension, for the window to do on its own host. Asked as
    /// [`Page::take_requests`] is.
    fn take_extension_calls(&mut self) -> Vec<(&'static str, winter_ext_api::Reply)> {
        Vec::new()
    }

    /// What the editor asks of the window besides its outcome since it was
    /// last asked: a place in another file to go to, edits to make to
    /// another file. Asked after every call, as [`Page::take_notice`] is,
    /// so an editor answering a server can ask whatever it was doing.
    fn take_requests(&mut self) -> Vec<EditorRequest> {
        Vec::new()
    }

    /// What the editor has to report since it was last asked, for the status
    /// bar: the one way an editor reports. The host asks after every call it
    /// makes into the editor, so a report made alongside some other outcome,
    /// such as a job, still shows, and an editor never draws a report itself.
    fn take_notice(&mut self) -> Option<Notice> {
        None
    }

    /// The search the editor has run or repeated since it was last asked,
    /// taken like [`Page::take_notice`]. The host highlights its matches
    /// over what the editor paints, so an editor never draws a match itself.
    fn take_search(&mut self) -> Option<SearchReport> {
        None
    }

    /// What the status bar shows for the editor while it is focused: its mode
    /// and where its cursor is. An editor with neither shows nothing there.
    fn status(&self) -> EditorStatus {
        EditorStatus::default()
    }

    /// Where the editor's own caret is on its cursor line, for an editor that puts
    /// one somewhere other than the first thing it painted. An editor whose
    /// cursor is a whole row reports none.
    fn caret(&self) -> Option<Caret> {
        None
    }

    /// A picture for the host to draw over the whole pane, for an editor whose
    /// content is an image rather than rows. `None`, the default, for a
    /// editor that paints rows.
    fn picture(&self) -> Option<Picture> {
        None
    }

    /// Hear the size of the picture the host decoded and of the pane it is
    /// drawn in, in pixels, each frame it is drawn: what an editor zooming and
    /// scrolling it reckons from.
    fn set_picture_geometry(&mut self, _natural: (f32, f32), _pane: (f32, f32)) {}

    /// Pixels the editor drew itself, each placed in the pane, for an editor
    /// whose content is drawn rather than painted as rows: the pages of a
    /// document. Empty, the default, for an editor that paints rows.
    fn tiles(&self) -> Vec<Tile> {
        Vec::new()
    }

    /// How many of the pane's top rows are the editor's painted rows and not
    /// its drawing: what [`Page::tiles`] places there is cut away, so a page
    /// scrolled up under a toolbar never covers it. None, the default.
    fn header_rows(&self) -> usize {
        0
    }

    /// Take the window's look each frame it is drawn, ahead of
    /// [`Page::set_pane_size`], for an editor drawing a page of its own in
    /// the theme's colors.
    fn set_appearance(&mut self, _appearance: &Appearance) {}

    /// Hear the pane's size in pixels each frame it is drawn, for an editor
    /// that lays out its own drawing.
    fn set_pane_size(&mut self, _pane: (f32, f32)) {}

    /// Whether the editor is running work of its own that the host's jobs
    /// do not see, a build, which its tab shows as a spinner.
    fn is_busy(&self) -> bool {
        false
    }

    /// A key pressed while a `v` or `V` selection is up over `rows`, the
    /// screen rows it covers: the editor's to act on all of them at once
    /// (Git stages them), or `None` to leave the key to the selection.
    fn on_selected_rows_key(
        &mut self,
        _key: &Key,
        _rows: RangeInclusive<usize>,
    ) -> Option<EditorOutcome> {
        None
    }

    /// Data describing the character and TextMate syntax scope stack under the
    /// cursor, when token inspection is active.
    fn token_inspect(&mut self) -> Option<TokenInspectData> {
        None
    }

    /// Data describing the character and TextMate syntax scope stack under `(row, col)`,
    /// when token inspection is active. Defaults to delegating to [`token_inspect`](Self::token_inspect).
    fn token_inspect_at(&mut self, _row: usize, _col: usize) -> Option<TokenInspectData> {
        self.token_inspect()
    }

    /// The editor is showing again after another one was closed over the top of
    /// it. What it was showing may have changed while it was covered, so a
    /// editor that reads the world re-reads it here. Editors showing something
    /// that cannot go stale never implement it.
    fn on_resume(&mut self) -> EditorOutcome {
        EditorOutcome::Consumed
    }

    /// What to offer in a menu opened over the editor's rows, for the row the
    /// cursor is on: the host puts the cursor under the pointer before
    /// asking, so an editor answers for what was clicked.
    ///
    /// An empty list, the default, means the editor offers no menu and the
    /// click does nothing. Editors whose rows all mean the same thing never
    /// implement it.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        Vec::new()
    }

    /// Hear a right-click at `at`, before the menu is built: by default the
    /// same as a click, so the menu acts on the row under the pointer. A
    /// editor whose click would lose what the menu should act on (a selection
    /// a "Copy" entry would copy) answers otherwise.
    fn on_context_mouse(&mut self, at: Point) -> EditorOutcome {
        self.on_mouse(at)
    }

    /// What the editor has open over its rows: a card, a completion popup,
    /// a drop-down list. None of them for most editors.
    fn popups(&self) -> Popups {
        Popups::default()
    }

    /// Where the editor is, outermost first (a directory's path, one crumb to
    /// a folder): the breadcrumb row of the band the window draws over the
    /// editor's rows, off its grid, each crumb a press away (see
    /// [`Page::on_crumb`]). Empty for an editor with no breadcrumbs, which
    /// gets no row.
    fn crumbs(&self) -> Vec<String> {
        Vec::new()
    }

    /// A press on crumb `index` of [`Page::crumbs`].
    fn on_crumb(&mut self, _index: usize) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// The names of the editor's columns, drawn on the breadcrumb row each
    /// over the content columns it names, and kept in place as the rows
    /// scroll. Empty for an editor with no columns to name.
    fn column_header(&self) -> Vec<ColumnName> {
        Vec::new()
    }

    /// The bar the editor wants drawn over its top rows in the window's own
    /// font. `None` for an editor without one.
    fn toolbar(&self) -> Option<Toolbar> {
        None
    }

    /// A press on button `button` of the editor's toolbar.
    fn on_toolbar(&mut self, _button: u16) -> EditorOutcome {
        EditorOutcome::Ignored
    }

    /// Tooltip text for button `button` of the editor's toolbar.
    fn toolbar_tooltip(&self, _button: u16) -> Option<String> {
        None
    }

    /// What the editor is showing, as an outline: its headings, sections,
    /// and definitions, each naming an index a [`JumpStep`] takes, so the
    /// outline is gone through as the jump list is. `None` for an editor
    /// with no outline to give.
    fn outline_targets(&mut self) -> Option<Vec<JumpTarget>> {
        None
    }

    /// Jump targets in the current view, each naming an internal index (a
    /// line, a row) a [`JumpStep`] takes.
    fn jump_targets(&self) -> Option<Vec<JumpTarget>> {
        None
    }

    /// Current cursor row or position index corresponding to target indices
    /// in [`jump_targets`](Page::jump_targets), used to preselect the nearest target when opening jump.
    fn jump_cursor(&self) -> Option<usize> {
        None
    }

    /// Take one step through the jump list or the outline: show a target,
    /// settle on it, or go back to where the jump started.
    fn on_jump(&mut self, _step: JumpStep) {}

    /// Where an editor or shell opened from this editor starts: the directory it
    /// is looking at, following the cursor rather than its root. Editors tied
    /// to nowhere on disk never implement it.
    fn cwd(&self) -> Option<PathBuf> {
        None
    }
}

/// A page whose contents are a buffer: a file's text, a directory, a
/// repository, a build. The buffer is shared by every tab showing it, buried
/// when the last one closes, and brought back with the session, so what an
/// editor adds to a page is what that takes: its views, its file, and what
/// it holds that could be lost.
pub trait Editor: Page {
    /// The editor's buffer is being killed: tell whatever is attached to the
    /// file (a language server, a program blocked on the edit) that it is
    /// over, ahead of the drop.
    fn on_closed(&mut self) {}

    /// A copy of the state of the view loaded now (cursor, scroll, selection,
    /// pane geometry), for the window to keep while another view of the same
    /// buffer is loaded. Small by design: what can be shared (the text, its
    /// caches) stays in the editor. `None` for a kind whose views share one.
    fn save_view(&self) -> Option<Box<dyn Any>> {
        None
    }

    /// Load a view's state that [`Editor::save_view`] gave earlier.
    fn restore_view(&mut self, _state: Box<dyn Any>) {}

    /// Whether any tab shows the editor. A buried one stops what it refreshes
    /// on a timer (a process list, a file watch), and refreshes once when a
    /// tab shows it again; work running (a build, a search) goes on.
    fn set_shown(&mut self, _shown: bool) {}

    /// Drop what can be built again, as the editor's buffer is buried: what
    /// a buried buffer keeps should be its state, not its pictures.
    fn shed_caches(&mut self) {}

    /// Whether a program is blocked on this edit, such as `git commit` waiting
    /// on its message. Closing such an editor's tab kills its buffer rather
    /// than burying it, since a buried one would keep the program waiting.
    fn is_waited_on(&self) -> bool {
        false
    }

    /// Whether closing this editor's last tab buries its buffer to come back
    /// to, rather than letting it go. An editor holding nothing worth coming
    /// back to (a launcher, a commit message whose commit has run) says no.
    fn is_kept_when_closed(&self) -> bool {
        !self.is_waited_on()
    }

    /// The repository whose git view the window goes back to once this
    /// editor's tab closes: a commit message's, its work done there. `None`
    /// for every other editor.
    fn returns_to_git(&self) -> Option<PathBuf> {
        None
    }

    /// Where the title bar says this editor is: one fixed location, written
    /// with `~` for home, that no cursor movement, line, or editor changes.
    /// `None` for an editor tied to nowhere on disk, which the bar then names by
    /// its kind alone.
    fn address(&self) -> Option<String> {
        None
    }

    /// Offer a file to the editor, for an editor that can show more than one.
    /// `false`, the default, means the host opens it however it would have.
    fn open_file(&mut self, _target: OpenTarget) -> bool {
        false
    }

    /// Where the editor is looking, for opening it there again after a restart:
    /// the directory an editor is rooted in or the file an editor has open, and
    /// the line its cursor is on. `None` for an editor that cannot be reopened
    /// from a place alone.
    fn snapshot(&self) -> Option<OpenTarget> {
        None
    }

    /// Whether the editor has the file at `path` open, shown or not: an editor
    /// holds every buffer it has open, a viewer the one file it shows.
    fn holds_file(&self, path: &Path) -> bool {
        self.snapshot().is_some_and(|at| at.path == path)
    }

    /// The base directory this editor is browsing or rooted in. Unlike `cwd()`,
    /// this does not track cursor movements across entries inside the editor.
    fn root_dir(&self) -> Option<PathBuf> {
        self.cwd()
    }

    /// A file or directory reference for the current view, formatted for copying.
    /// Editors with a more specific location (like an active selection or cursor line)
    /// override this.
    fn file_reference(&self) -> Option<String> {
        self.cwd().map(|p| crate::path::abbreviate_home(&p))
    }

    /// Whether work already under way still wants ticks while the editor's
    /// buffer is buried: a build to see through, edits a language server has
    /// still to hear of. A buried buffer is ticked for nothing else; what it
    /// looks at on a timer waits until a pane shows it again.
    fn has_work_in_flight(&self) -> bool {
        self.is_busy()
    }

    /// Whether the editor is holding work that would be lost with it.
    ///
    /// The host asks before closing a pane or a tab over one, keeps it out of
    /// the reopen stash's way when that fills up, and marks it in the list of
    /// what was closed. Editors holding nothing a reader could lose never
    /// implement it.
    fn is_dirty(&self) -> bool {
        false
    }

    /// The file the editor is showing and its text as edited, for an
    /// extension to read. Editors holding no file's text never implement it.
    fn document(&self) -> Option<(PathBuf, String)> {
        None
    }

    /// A number that moves whenever the text of [`Editor::document`] does,
    /// so an extension can be told of an edit. Editors holding no file's
    /// text never implement it.
    fn text_revision(&self) -> Option<usize> {
        None
    }

    /// The file the editor is showing, without its text: what an
    /// extension is told was opened or closed.
    fn document_path(&self) -> Option<PathBuf> {
        self.document().map(|(path, _)| path)
    }

    /// The keyboard has left the editor, for another pane or another
    /// window: write what it holds, as far as that can be done without
    /// asking anything, and say which files were written. Editors holding
    /// nothing to write never implement it.
    fn focus_lost(&mut self) -> Vec<PathBuf> {
        Vec::new()
    }
}

/// A page seen as [`Any`], for [`dyn Page::downcast_ref`]: given to every
/// page by the blanket impl below, so none writes it.
pub trait AsAny {
    /// This, as [`Any`].
    fn as_any(&self) -> &dyn Any;
    /// The same, to change.
    fn as_any_mut(&mut self) -> &mut dyn Any;
}

impl<T: Any> AsAny for T {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }
}

impl dyn Page {
    /// Run `run` on this page as the kind `T`, if it is one: under the
    /// guard the editor runs in, so a panic in `run` stops the editor as one
    /// in any other call does. `None` for a page of another kind, or one
    /// stopped.
    ///
    /// The only way into a kind's own methods: there is no reference to one
    /// to hold, so no call can slip past the guard.
    pub fn with_kind<T: Page + 'static, R>(&self, run: impl FnOnce(&T) -> R) -> Option<R> {
        let any = (*self).as_any();
        if let Some(guarded) = any.downcast_ref::<Guarded<Box<dyn Editor>>>() {
            return guarded.with_inner(|page| page.as_any().downcast_ref::<T>().map(run));
        }
        if let Some(guarded) = any.downcast_ref::<Guarded<Box<dyn Page>>>() {
            return guarded.with_inner(|page| page.as_any().downcast_ref::<T>().map(run));
        }
        any.downcast_ref::<T>().map(run)
    }

    /// The same, to change.
    pub fn with_kind_mut<T: Page + 'static, R>(
        &mut self,
        run: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let any = self.as_any_mut();
        if any.is::<Guarded<Box<dyn Editor>>>() {
            return any
                .downcast_mut::<Guarded<Box<dyn Editor>>>()?
                .with_inner_mut(|page| page.as_any_mut().downcast_mut::<T>().map(run));
        }
        if any.is::<Guarded<Box<dyn Page>>>() {
            return any
                .downcast_mut::<Guarded<Box<dyn Page>>>()?
                .with_inner_mut(|page| page.as_any_mut().downcast_mut::<T>().map(run));
        }
        any.downcast_mut::<T>().map(run)
    }

    /// This page as the kind `T`, seen through the guard, for a test to
    /// look inside. Outside tests a kind is reached only through
    /// [`Self::with_kind`].
    #[cfg(any(test, feature = "test-support"))]
    pub fn downcast_ref<T: Page + 'static>(&self) -> Option<&T> {
        let any = (*self).as_any();
        if let Some(guarded) = any.downcast_ref::<Guarded<Box<dyn Editor>>>() {
            return guarded.inner().downcast_ref();
        }
        match any.downcast_ref::<Guarded<Box<dyn Page>>>() {
            Some(guarded) => guarded.inner().downcast_ref(),
            None => any.downcast_ref(),
        }
    }

    /// The same, to change.
    #[cfg(any(test, feature = "test-support"))]
    pub fn downcast_mut<T: Page + 'static>(&mut self) -> Option<&mut T> {
        let any = self.as_any_mut();
        if any.is::<Guarded<Box<dyn Editor>>>() {
            return any
                .downcast_mut::<Guarded<Box<dyn Editor>>>()?
                .inner_mut()
                .downcast_mut();
        }
        if any.is::<Guarded<Box<dyn Page>>>() {
            return any
                .downcast_mut::<Guarded<Box<dyn Page>>>()?
                .inner_mut()
                .downcast_mut();
        }
        any.downcast_mut()
    }
}

impl dyn Editor {
    /// [`with_kind`](Page::with_kind) for an editor: the same, through the
    /// page it is.
    pub fn with_kind<T: Page + 'static, R>(&self, run: impl FnOnce(&T) -> R) -> Option<R> {
        let page: &dyn Page = self;
        page.with_kind(run)
    }

    /// The same, to change.
    pub fn with_kind_mut<T: Page + 'static, R>(
        &mut self,
        run: impl FnOnce(&mut T) -> R,
    ) -> Option<R> {
        let page: &mut dyn Page = self;
        page.with_kind_mut(run)
    }

    /// [`downcast_ref`](Page::downcast_ref) for an editor, for a test.
    #[cfg(any(test, feature = "test-support"))]
    pub fn downcast_ref<T: Page + 'static>(&self) -> Option<&T> {
        let page: &dyn Page = self;
        page.downcast_ref()
    }

    /// The same, to change.
    #[cfg(any(test, feature = "test-support"))]
    pub fn downcast_mut<T: Page + 'static>(&mut self) -> Option<&mut T> {
        let page: &mut dyn Page = self;
        page.downcast_mut()
    }
}

// ========================================================================
// EditorOutcome
// ========================================================================

impl EditorOutcome {
    /// Copy `text`, saying how many characters went rather than echoing
    /// what may be pages of it.
    pub fn copied(text: String) -> Self {
        let count = text.chars().count();
        let noun = match count {
            1 => "character",
            _ => "characters",
        };
        Self::Copy {
            notice: format!("Copied {count} {noun}"),
            text,
        }
    }
}

// ========================================================================
// OpenTarget
// ========================================================================

impl OpenTarget {
    /// Open `path`, leaving it to the editor where to start.
    pub fn file(path: PathBuf) -> Self {
        Self {
            column: None,
            line: None,
            path,
        }
    }

    /// Open `path` with the cursor on `line`, counting from one.
    pub fn at_line(path: PathBuf, line: usize) -> Self {
        Self {
            column: None,
            line: Some(line),
            path,
        }
    }

    /// Open `path` with the cursor on `line` and, where known, `column`,
    /// both counting from one.
    pub fn at(path: PathBuf, line: usize, column: Option<usize>) -> Self {
        Self {
            column,
            line: Some(line),
            path,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_yes_or_no_question_starts_with_a_capital_and_ends_with_its_keys() {
        // Built by hand in seven editors, two of which capitalized by byte
        // slicing, which panics on a question starting with `é`.
        let asked = PromptRequest::confirm("t", "kill 42 (vim)?");
        assert_eq!(asked.label, "Kill 42 (vim)? (y/n) ");
        assert_eq!(asked.mode, PromptMode::Confirm);
        assert_eq!(
            PromptRequest::confirm("t", "éteindre?").label,
            "Éteindre? (y/n) "
        );
    }

    #[test]
    fn test_a_search_looks_past_the_cursor_and_wraps() {
        let labels = ["main.rs", "lib.rs", "mod.rs"];
        assert_eq!(find_match(&labels, "mod", 0, true), Some(2));
        assert_eq!(find_match(&labels, "main", 2, true), Some(0));
        assert_eq!(find_match(&labels, "mod", 0, false), Some(2));
        assert_eq!(find_match(&labels, "lib", 0, false), Some(1));
    }

    #[test]
    fn test_a_search_ignores_case_until_the_query_has_a_capital() {
        assert_eq!(find_match(&["Cargo.toml"], "cargo", 0, true), Some(0));
        assert_eq!(find_match(&["cargo.toml"], "Cargo", 0, true), None);
        assert_eq!(find_match(&["x", "Cargo.toml"], "Cargo", 0, true), Some(1));
    }

    #[test]
    fn test_match_spans_lists_every_occurrence_in_columns_of_the_text() {
        assert_eq!(match_spans("aXa xa", "xa"), vec![(1, 3), (4, 6)]);
        assert_eq!(match_spans("aaa", "aa"), vec![(0, 2), (1, 3)]);
        // `İ` lowers to two characters; folding it whole would shift every
        // column after it one to the right of the text it highlights.
        assert_eq!(match_spans("İ mod", "mod"), vec![(2, 5)]);
    }

    #[test]
    fn test_a_row_search_report_counts_matching_rows_and_the_cursors_place() {
        let labels = ["mod.rs", "lib.rs", "mod_two.rs"];
        let report = row_search_report(&labels, "mod", 2, false);
        assert_eq!((report.index, report.total), (2, 2));
        assert_eq!(row_search_report(&labels, "mod", 1, false).index, 0);
    }

    #[test]
    fn test_the_row_under_the_cursor_matches_last() {
        // Otherwise a query the cursor already sits on never moves anywhere,
        // and repeating it looks like the search broke.
        let labels = ["mod.rs", "modal.rs"];
        assert_eq!(find_match(&labels, "mod", 0, true), Some(1));
        assert_eq!(find_match(&["mod.rs", "lib.rs"], "mod", 0, true), Some(0));
    }

    #[test]
    fn test_nothing_is_found_without_a_query_or_without_rows() {
        // An empty listing would divide by its own length.
        let empty: [&str; 0] = [];
        assert_eq!(find_match(&empty, "mod", 0, true), None);
        assert_eq!(find_match(&["mod.rs"], "", 0, true), None);
        assert_eq!(find_match(&["mod.rs"], "absent", 0, true), None);
    }

    #[test]
    fn test_scroll_position_to_scroll_params() {
        // Content fits on screen -> no scrollbar.
        let fits = ScrollPosition::new(0, 25, 20);
        assert_eq!(fits.to_scroll_params(25), (0, 0));

        let exact = ScrollPosition::new(0, 25, 25);
        assert_eq!(exact.to_scroll_params(25), (0, 0));

        // Long content at the very top.
        let top = ScrollPosition::new(0, 25, 100);
        let (offset, len) = top.to_scroll_params(25);
        assert_eq!(len, 75);
        assert_eq!(offset, 75); // top_virtual = len - offset = 0

        // Long content at the bottom.
        let bottom = ScrollPosition::new(75, 25, 100);
        let (offset, len) = bottom.to_scroll_params(25);
        assert_eq!(len, 75);
        assert_eq!(offset, 0); // top_virtual = len - offset = 75

        // Scaled / pixel-based units.
        let pixels = ScrollPosition::new(300, 600, 1800);
        let (offset, len) = pixels.to_scroll_params(30);
        // total_virtual = (1800 / 600) * 30 = 90
        // scrollback_len = 90 - 30 = 60
        // top_virtual = (300 / 1800) * 90 = 15
        // scroll_offset = 60 - 15 = 45
        assert_eq!(len, 60);
        assert_eq!(offset, 45);
    }
}

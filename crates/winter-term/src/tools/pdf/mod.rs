//! PDF: one document, drawn by a real web engine over the pane's pixels.
//!
//! The page itself paints only its header row, in the terminal's own font and
//! theme. Everything below that is a [`PageSurface`] the host fills with a
//! WebView running the vendored pdf.js build, and the keys bound here reach it
//! as scripts rather than as motions over rows.

mod assets;

use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::model::input::{Key, KeyCode};
use crate::model::page::{
    Page, PageContent, PageHint, PageOutcome, PageRow, PageSpan, PageStyle, PageSurface,
    PromptMode, PromptReply, PromptRequest,
};

// ========================================================================
// Constants
// ========================================================================

/// Rows the page paints above its surface: the title, and a blank row so the
/// document does not butt against it.
const HEADER_ROWS: usize = 2;

/// What the header calls each mode, in the place the other tools put a state
/// word. Normal mode says nothing: it is the one you are in unless told
/// otherwise.
const LABEL_CURSOR: &str = "CURSOR";
const LABEL_VISUAL: &str = "VISUAL";
const LABEL_VISUAL_LINE: &str = "V-LINE";

/// Hint shown beside the title.
const HINT: &str = "i cursor, / search, j/k scroll, Ctrl-d/u half, gg/G ends, {n} repeats, q close";

/// Which question a prompt's answer belongs to: what to look for down the
/// document, and what to look for back up it.
const ASK_SEARCH: &str = "search";
const ASK_SEARCH_BACK: &str = "search-back";

/// Left margin the title starts at, matching the other tools.
const LEFT_PAD: usize = 2;

/// Fraction of the viewport `Ctrl-d` and `Ctrl-u` move, the half-screen step
/// vim's own are named for.
const HALF_SCREEN: f32 = 0.5;

/// Fraction of the viewport `Ctrl-f`, `Ctrl-b`, and the page keys move. Short
/// of a whole screen on purpose: the lines that were at the edge stay on it,
/// so a page read straight through never skips a band of text.
const FULL_SCREEN: f32 = 0.9;

/// The largest count a motion will accept, so a key held on the number row
/// cannot ask the surface for a scroll it has to compute its way out of.
const MAX_COUNT: usize = 99_999;

/// Titles of the two prefixes, for the key hints the host draws while one is
/// part-way through.
const TITLE_GOTO: &str = "g";
const TITLE_PLACE: &str = "z";

// ========================================================================
// Data Structures
// ========================================================================

/// A page showing one PDF, driven by keys and drawn by its surface.
#[derive(Clone, Debug)]
pub struct PdfPage {
    /// What the surface last said went wrong, shown beside the title until the
    /// next key.
    message: Option<String>,
    /// Which line the cursor is on, once there is one, for the header.
    caret_line: usize,
    /// How the document is being read: with a cursor in the text, or with the
    /// keys moving the window over it.
    mode: Mode,
    /// The command being typed: a count, an operator, and any prefix waiting
    /// for the key that completes it.
    pending: Pending,
    /// How far through the document the surface last reported being, for the
    /// position `Ctrl-g` shows.
    percent: usize,
    /// What the cursor has selected, which the header names so a selection
    /// left open is never a surprise.
    visual: Visual,
    /// The page the surface is showing, once it has said which.
    page: usize,
    /// How many pages the document has, once the surface has counted them.
    pages: usize,
    path: PathBuf,
    /// Script waiting to run in the surface, taken by the host each frame.
    script: Option<String>,
    /// What `/` or `?` last looked for. The surface does the searching and
    /// remembers which way it went; this is what tells `n` that there is a
    /// search to repeat at all.
    search: Option<String>,
}

/// What a PDF surface posts back: where it scrolled to, or why it could not
/// show the document. Every field is optional because the surface reports
/// whichever it has, and an engine that fails early has only the error.
#[derive(Deserialize)]
struct SurfaceReport {
    /// Set when the reader asked where they are, rather than the surface
    /// saying so in passing after a scroll.
    announce: Option<bool>,
    /// Where the cursor is, when there is one.
    caret: Option<CaretReport>,
    error: Option<String>,
    /// What the surface says it is in, which is the truth: asking for a
    /// cursor in a document with no text leaves it without one.
    mode: Option<String>,
    page: Option<usize>,
    pages: Option<usize>,
    percent: Option<usize>,
    /// Text the surface copied, for the host to put on the clipboard.
    yank: Option<String>,
}

/// Where the surface says its cursor is.
#[derive(Deserialize)]
struct CaretReport {
    line: usize,
    page: usize,
}

/// The command being typed, which in vim's grammar is a count, then an
/// optional operator, then a key that may itself be the first of a pair.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct Pending {
    /// The digits typed so far, `None` until one is. A motion with no count
    /// means once, which is not the same as a count of zero.
    count: Option<usize>,
    /// An operator waiting for the motion that says what it acts on.
    operator: Operator,
    prefix: Prefix,
}

/// A key that is the first of a pair, waiting for the one that says what it
/// meant.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Prefix {
    /// Nothing is pending; the next key starts a command.
    #[default]
    None,
    /// `f`, `F`, `t`, or `T`, waiting for the character to find.
    Find(FindKind),
    /// `g`, which `g` completes as the first page.
    Goto,
    /// `z`, which `t`, `z`, or `b` completes by putting the page at an edge
    /// of the viewport.
    Place,
}

/// How the document is being read.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Mode {
    /// No cursor: the keys move the window over the document, the way a
    /// pager's do.
    #[default]
    Normal,
    /// A cursor sits in the text and the keys move it, the way vim's do.
    Cursor,
}

/// What the cursor has selected, when it has anything.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Visual {
    /// Up to the character the cursor is on.
    Char,
    /// Whole lines, however far into them the ends sit.
    Line,
    /// Nothing is selected.
    #[default]
    None,
}

/// What is being done with what a motion passes over.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Operator {
    /// No operator: the motion moves the cursor.
    #[default]
    Move,
    /// `y`: the motion says what to copy.
    Yank,
}

/// Which way `f`, `F`, `t`, and `T` look, and whether they stop on the
/// character or just short of it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum FindKind {
    /// `F`: back to the character.
    Backward,
    /// `T`: back to just after it.
    BackwardTill,
    /// `f`: on to the character.
    Forward,
    /// `t`: on to just before it.
    ForwardTill,
}

/// A motion the cursor makes, named as `caret.mjs` names it. The two sides
/// only ever meet in this string, so it is spelled once here rather than at
/// every call.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Motion {
    DocEnd,
    DocStart,
    Down,
    Left,
    LineEnd,
    LineFirstNonBlank,
    LineStart,
    /// The first line of the page a count names.
    Page,
    ParagraphBackward,
    ParagraphForward,
    Right,
    SentenceBackward,
    SentenceForward,
    Up,
    ViewBottom,
    ViewMiddle,
    ViewTop,
    WordBackward,
    WordEnd,
    WordForward,
}

/// A cursor motion, repeated `count` times, with `operator` saying what is
/// being done to what it passes over.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaretMotion {
    count: usize,
    motion: Motion,
    operator: Operator,
}

/// A find on the cursor's own line, under an operator or not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaretFind {
    ch: char,
    count: usize,
    kind: FindKind,
    operator: Operator,
}

/// A search down the document or back up it, under an operator or not.
#[derive(Clone, Debug, Eq, PartialEq)]
struct CaretSearch {
    count: usize,
    /// Whether the search runs down the document, which is what tells `/`
    /// from `?` and which way `n` then goes.
    forward: bool,
    operator: Operator,
    pattern: String,
}

/// `n` or `N`: the last search again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaretSearchStep {
    count: usize,
    operator: Operator,
    /// Whether to go the other way, which is what tells `N` from `n`.
    reverse: bool,
}

/// `;` or `,`: the last find again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CaretRepeat {
    count: usize,
    operator: Operator,
    /// Whether to look the other way, which is what tells `,` from `;`.
    reverse: bool,
}

/// Where `zt`, `zz`, and `zb` put the page the cursor is on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PagePlace {
    Bottom,
    Center,
    Top,
}

/// What a resolved key asks the surface to do. One variant per command the
/// viewer has, so the key handling decides *what* and this decides *how to
/// say it*: the two cannot drift into disagreeing about a script's name.
#[derive(Clone, Debug, PartialEq)]
enum Command {
    /// Put a cursor in the text.
    CaretEnter,
    /// Find a character on the cursor's own line.
    CaretFind(CaretFind),
    /// Take the cursor back out of the text.
    CaretLeave,
    /// Move the cursor, or act on what the motion passes over.
    CaretMotion(CaretMotion),
    /// Put the cursor's own line at an edge of the pane, moving the window
    /// and not the cursor.
    CaretPlaceLine(PagePlace),
    /// Repeat the last find, the same way or the opposite one.
    CaretRepeatFind(CaretRepeat),
    /// Look for text, down the document or back up it.
    CaretSearch(CaretSearch),
    /// Repeat the last search, the same way or the opposite one.
    CaretSearchStep(CaretSearchStep),
    /// Put the cursor on the other end of the selection.
    CaretSwapEnds,
    /// Start, change, or drop a selection.
    CaretVisual(Visual),
    /// Copy whole lines from the cursor's own.
    CaretYankLines(usize),
    /// Copy what is selected.
    CaretYankSelection,
    /// Fit the page's width to the pane, which also ends a zoom.
    FitWidth,
    /// Jump to an absolute page, clamped by the surface.
    GoToPage(usize),
    /// Step this many pages, backwards when negative.
    GoToPageBy(isize),
    /// Jump to the last page.
    GoToLastPage,
    /// Put the current page at an edge of the viewport, scrolling nothing
    /// else.
    Place(PagePlace),
    /// Report where in the document the reader is.
    ReportPosition,
    /// Scroll sideways by this many steps, left when negative.
    ScrollColumns(isize),
    /// Scroll by this many lines, up when negative.
    ScrollLines(isize),
    /// Scroll by this fraction of the viewport, up when negative.
    ScrollScreens(f32),
    /// Zoom in when positive, out when negative.
    Zoom(isize),
}

// ========================================================================
// PdfPage
// ========================================================================

impl PdfPage {
    /// A page showing `path`, before its surface has opened it.
    pub fn new(path: PathBuf) -> Self {
        Self {
            message: None,
            page: 0,
            pages: 0,
            caret_line: 0,
            mode: Mode::default(),
            path,
            pending: Pending::default(),
            percent: 0,
            script: None,
            search: None,
            visual: Visual::None,
        }
    }

    /// Whether `path` is a file this page should be the one to open.
    pub fn handles(path: &Path) -> bool {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    }

    /// Queue `js` to run in the surface on the host's next frame.
    fn run(&mut self, js: &str) -> PageOutcome {
        self.script = Some(js.to_string());
        PageOutcome::Consumed
    }

    /// Carry out a resolved command, clearing whatever was being typed: a
    /// count belongs to the motion it preceded and to nothing after it.
    fn act(&mut self, command: Command) -> PageOutcome {
        self.pending = Pending::default();
        let js = script(command);
        self.run(&js)
    }

    /// The count typed before the pending motion, or one when none was: a
    /// motion with no count runs once.
    fn count(&self) -> usize {
        self.pending.count.unwrap_or(1)
    }

    /// The count as a signed repeat in `direction`, for a motion that has two
    /// of them.
    fn steps(&self, direction: isize) -> isize {
        (self.count().min(MAX_COUNT) as isize).saturating_mul(direction)
    }

    /// Take `digit` as part of the count being typed, or say it is not one.
    ///
    /// A leading `0` is not a count in vim, it is a motion, so it only counts
    /// once digits are already being collected.
    fn take_count_digit(&mut self, digit: char) -> bool {
        let Some(value) = digit.to_digit(10).map(|value| value as usize) else {
            return false;
        };
        if value == 0 && self.pending.count.is_none() {
            return false;
        }
        let so_far = self.pending.count.unwrap_or(0);
        self.pending.count = Some(
            so_far
                .saturating_mul(10)
                .saturating_add(value)
                .min(MAX_COUNT),
        );
        true
    }

    /// The name of the file, for the title. A path with no file name at all
    /// falls back to the whole path rather than showing nothing.
    fn file_name(&self) -> String {
        match self.path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => self.path.display().to_string(),
        }
    }

    /// Where the reader is, once the surface has said. Blank until then: a
    /// confident "page 0 of 0" would be a lie about a document still loading.
    fn position(&self) -> String {
        if self.pages == 0 {
            return String::new();
        }
        format!("page {} / {}", self.page.max(1), self.pages)
    }

    fn title_row(&self) -> PageRow {
        let mut spans = vec![PageSpan::new(
            PageStyle::Header,
            format!("{}{}  ", pad(LEFT_PAD), self.file_name()),
        )];
        let position = self.position();
        if !position.is_empty() {
            spans.push(PageSpan::new(PageStyle::Accent, format!("{position}  ")));
        }
        let state = self.mode_label();
        if !state.is_empty() {
            spans.push(PageSpan::new(PageStyle::Header, format!("{state}  ")));
        }
        spans.push(PageSpan::new(PageStyle::Dim, HINT));
        // What has been typed so far, the way vim shows a pending count in
        // the corner: without it a count typed by accident is invisible until
        // the next motion jumps much too far.
        let typed = self.typed();
        if !typed.is_empty() {
            spans.push(PageSpan::new(PageStyle::Accent, format!("  {typed}")));
        }
        if let Some(message) = &self.message {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  {message}")));
        }
        spans
    }

    /// What to call the mode in the header: nothing in Normal mode, and what
    /// is selected when something is.
    fn mode_label(&self) -> String {
        match (self.mode, self.visual) {
            (Mode::Normal, _) => String::new(),
            (Mode::Cursor, Visual::None) => format!("{LABEL_CURSOR} {}", self.caret_line),
            (Mode::Cursor, Visual::Char) => format!("{LABEL_VISUAL} {}", self.caret_line),
            (Mode::Cursor, Visual::Line) => format!("{LABEL_VISUAL_LINE} {}", self.caret_line),
        }
    }

    /// The keys typed so far towards a command that is not finished.
    fn typed(&self) -> String {
        let count = match self.pending.count {
            Some(count) => count.to_string(),
            None => String::new(),
        };
        let operator = match self.pending.operator {
            Operator::Move => "",
            Operator::Yank => "y",
        };
        let prefix = match self.pending.prefix {
            Prefix::Find(kind) => kind.as_js(),
            Prefix::Goto => TITLE_GOTO,
            Prefix::None => "",
            Prefix::Place => TITLE_PLACE,
        };
        format!("{count}{operator}{prefix}")
    }

    /// Act on `key` while `g` is pending.
    ///
    /// `gg` is the first page, or the page a count names, which is the one
    /// place vim reads a count as an absolute rather than a repeat. Anything
    /// else abandons the prefix, as vim does.
    fn on_goto_key(&mut self, key: &Key) -> PageOutcome {
        let count = self.pending.count;
        if key.code != KeyCode::Char('g') {
            self.pending = Pending::default();
            return PageOutcome::Consumed;
        }
        match self.mode {
            // With a cursor in the text, `gg` is where the text starts, not
            // where the window goes.
            Mode::Cursor => {
                let motion = match count {
                    Some(_) => Motion::Page,
                    None => Motion::DocStart,
                };
                self.caret_motion(motion)
            }
            Mode::Normal => self.act(Command::GoToPage(count.unwrap_or(1))),
        }
    }

    /// Act on `key` while `z` is pending: `zt`, `zz`, and `zb` put the page
    /// the reader is on at the top, middle, or bottom of the pane.
    fn on_place_key(&mut self, key: &Key) -> PageOutcome {
        let place = match key.code {
            KeyCode::Char('b') => Some(PagePlace::Bottom),
            KeyCode::Char('t') => Some(PagePlace::Top),
            KeyCode::Char('z') => Some(PagePlace::Center),
            _ => None,
        };
        let Some(place) = place else {
            self.pending = Pending::default();
            return PageOutcome::Consumed;
        };
        match self.mode {
            // With a cursor, the window moves so the cursor's own line lands
            // at the edge, which is what vim's own z-chords do.
            Mode::Cursor => self.act(Command::CaretPlaceLine(place)),
            Mode::Normal => self.act(Command::Place(place)),
        }
    }

    /// Act on the character typed after `f`, `F`, `t`, or `T`.
    fn on_find_key(&mut self, kind: FindKind, key: &Key) -> PageOutcome {
        let pending = self.pending;
        self.pending = Pending::default();
        let KeyCode::Char(ch) = key.code else {
            // Anything but a character abandons the find, as vim does.
            return PageOutcome::Consumed;
        };
        self.act(Command::CaretFind(CaretFind {
            ch,
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            kind,
            operator: pending.operator,
        }))
    }

    /// Run `motion` with whatever count and operator were typed before it.
    fn caret_motion(&mut self, motion: Motion) -> PageOutcome {
        let pending = self.pending;
        self.act(Command::CaretMotion(CaretMotion {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            motion,
            operator: pending.operator,
        }))
    }

    /// Act on a key held with Ctrl, which is where vim keeps the scrolls that
    /// move by a screen or a line without moving a cursor.
    fn on_ctrl_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            KeyCode::Char('b') => self.act(Command::ScrollScreens(-FULL_SCREEN)),
            KeyCode::Char('d') => self.act(Command::ScrollScreens(HALF_SCREEN)),
            KeyCode::Char('e') => {
                let lines = self.steps(1);
                self.act(Command::ScrollLines(lines))
            }
            KeyCode::Char('f') => self.act(Command::ScrollScreens(FULL_SCREEN)),
            KeyCode::Char('g') => self.act(Command::ReportPosition),
            KeyCode::Char('u') => self.act(Command::ScrollScreens(-HALF_SCREEN)),
            KeyCode::Char('y') => {
                let lines = self.steps(-1);
                self.act(Command::ScrollLines(lines))
            }
            _ => PageOutcome::Ignored,
        }
    }

    /// Act on a key with a cursor in the text and nothing pending.
    ///
    /// These are vim's motions, and they mean here what they mean there. The
    /// three that cannot are the ones with no character to step over: `h` and
    /// `l` still move the cursor, but `zt`, `zz`, and `zb` move the window,
    /// and the Ctrl scrolls leave the cursor where it is.
    fn on_cursor_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            KeyCode::Char('h') | KeyCode::Left => self.caret_motion(Motion::Left),
            KeyCode::Char('l') | KeyCode::Right => self.caret_motion(Motion::Right),
            KeyCode::Char('j') | KeyCode::Down => self.caret_motion(Motion::Down),
            KeyCode::Char('k') | KeyCode::Up => self.caret_motion(Motion::Up),
            KeyCode::Char('w') => self.caret_motion(Motion::WordForward),
            KeyCode::Char('b') => self.caret_motion(Motion::WordBackward),
            KeyCode::Char('e') => self.caret_motion(Motion::WordEnd),
            KeyCode::Char('0') | KeyCode::Home => self.caret_motion(Motion::LineStart),
            KeyCode::Char('^') => self.caret_motion(Motion::LineFirstNonBlank),
            KeyCode::Char('$') | KeyCode::End => self.caret_motion(Motion::LineEnd),
            KeyCode::Char('{') => self.caret_motion(Motion::ParagraphBackward),
            KeyCode::Char('}') => self.caret_motion(Motion::ParagraphForward),
            KeyCode::Char('(') => self.caret_motion(Motion::SentenceBackward),
            KeyCode::Char(')') => self.caret_motion(Motion::SentenceForward),
            // Uppercase, as vim has them: the line nearest an edge of what is
            // on screen, without scrolling to get there.
            KeyCode::Char('H') => self.caret_motion(Motion::ViewTop),
            KeyCode::Char('M') => self.caret_motion(Motion::ViewMiddle),
            KeyCode::Char('L') => self.caret_motion(Motion::ViewBottom),
            KeyCode::Char('G') => match self.pending.count {
                Some(_) => self.caret_motion(Motion::Page),
                None => self.caret_motion(Motion::DocEnd),
            },
            KeyCode::Char('f') => self.wait_for_find(FindKind::Forward),
            KeyCode::Char('F') => self.wait_for_find(FindKind::Backward),
            KeyCode::Char('t') => self.wait_for_find(FindKind::ForwardTill),
            KeyCode::Char('T') => self.wait_for_find(FindKind::BackwardTill),
            KeyCode::Char(';') => self.repeat_find(false),
            KeyCode::Char(',') => self.repeat_find(true),
            KeyCode::Char('v') => self.set_visual(Visual::Char),
            KeyCode::Char('V') => self.set_visual(Visual::Line),
            KeyCode::Char('o') => self.act(Command::CaretSwapEnds),
            // `y` on its own is the operator; in visual mode it takes the
            // selection there and then, and `yy` takes whole lines.
            KeyCode::Char('y') => self.on_yank_key(),
            KeyCode::Char('Y') => {
                let count = self.pending.count.unwrap_or(1).min(MAX_COUNT);
                self.act(Command::CaretYankLines(count))
            }
            KeyCode::Escape => self.on_cursor_escape(),
            KeyCode::Char('q') => PageOutcome::Close,
            // Everything left over is the window's, so a document can still
            // be paged through with a cursor in it.
            _ => self.on_window_key(key),
        }
    }

    /// Wait for the character `f`, `F`, `t`, or `T` is looking for, keeping
    /// the count and operator already typed.
    fn wait_for_find(&mut self, kind: FindKind) -> PageOutcome {
        self.pending.prefix = Prefix::Find(kind);
        PageOutcome::Consumed
    }

    /// `/` and `?`: ask the host for what to look for, keeping whatever count
    /// and operator were typed until the answer comes back.
    fn ask_search(&self, forward: bool) -> PageOutcome {
        PageOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: match forward {
                true => "/".to_string(),
                false => "?".to_string(),
            },
            mode: PromptMode::Text,
            tag: match forward {
                true => ASK_SEARCH,
                false => ASK_SEARCH_BACK,
            },
        })
    }

    /// Look for what the prompt came back with. An answer of nothing is the
    /// prompt cancelled, which leaves the last search alone.
    fn start_search(&mut self, pattern: String, forward: bool) -> PageOutcome {
        if pattern.is_empty() {
            self.pending = Pending::default();
            return PageOutcome::Consumed;
        }
        let pending = self.pending;
        self.search = Some(pattern.clone());
        self.act(Command::CaretSearch(CaretSearch {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            forward,
            operator: pending.operator,
            pattern,
        }))
    }

    /// `n` and `N`, which need a search to repeat and say so when there has
    /// not been one.
    fn search_step(&mut self, reverse: bool) -> PageOutcome {
        if self.search.is_none() {
            self.pending = Pending::default();
            self.message = Some("nothing has been searched for".to_string());
            return PageOutcome::Consumed;
        }
        let pending = self.pending;
        self.act(Command::CaretSearchStep(CaretSearchStep {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            operator: pending.operator,
            reverse,
        }))
    }

    /// `;` and `,`, which repeat the last find rather than asking for a new
    /// character.
    fn repeat_find(&mut self, reverse: bool) -> PageOutcome {
        let pending = self.pending;
        self.act(Command::CaretRepeatFind(CaretRepeat {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            operator: pending.operator,
            reverse,
        }))
    }

    /// `y`: the selection when there is one, `yy` when it follows itself, and
    /// otherwise the operator waiting for a motion.
    fn on_yank_key(&mut self) -> PageOutcome {
        if self.visual != Visual::None {
            self.visual = Visual::None;
            return self.act(Command::CaretYankSelection);
        }
        if self.pending.operator == Operator::Yank {
            let count = self.pending.count.unwrap_or(1).min(MAX_COUNT);
            return self.act(Command::CaretYankLines(count));
        }
        self.pending.operator = Operator::Yank;
        PageOutcome::Consumed
    }

    /// Escape with a cursor in the text: give up whatever is half-typed
    /// first, then the selection, then the cursor itself. One key, one step
    /// back, the way vim's own Escape works.
    fn on_cursor_escape(&mut self) -> PageOutcome {
        if self.pending != Pending::default() {
            self.pending = Pending::default();
            return PageOutcome::Consumed;
        }
        if self.visual != Visual::None {
            return self.set_visual(Visual::None);
        }
        self.mode = Mode::Normal;
        self.act(Command::CaretLeave)
    }

    /// Start a selection, swap which kind it is, or drop it: pressing `v` or
    /// `V` again ends the selection it started, the way vim does.
    fn set_visual(&mut self, kind: Visual) -> PageOutcome {
        self.visual = match self.visual == kind {
            true => Visual::None,
            false => kind,
        };
        self.act(Command::CaretVisual(self.visual))
    }

    /// Act on a key with no cursor in the text.
    fn on_normal_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            // Vim's own key for putting a cursor where the text is.
            KeyCode::Char('i') => {
                self.mode = Mode::Cursor;
                self.act(Command::CaretEnter)
            }
            _ => self.on_window_key(key),
        }
    }

    /// Act on a key that moves the window rather than a cursor. Reached from
    /// Normal mode directly, and from cursor mode for the keys it leaves
    /// alone, so a document can still be paged through with a cursor in it.
    fn on_window_key(&mut self, key: &Key) -> PageOutcome {
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                let lines = self.steps(1);
                self.act(Command::ScrollLines(lines))
            }
            KeyCode::Char('k') | KeyCode::Up => {
                let lines = self.steps(-1);
                self.act(Command::ScrollLines(lines))
            }
            // Sideways, for a page zoomed in past the pane's width. Vim has
            // no vertical-only rule here and neither does the reader.
            KeyCode::Char('h') | KeyCode::Char('H') | KeyCode::Left => {
                let steps = self.steps(-1);
                self.act(Command::ScrollColumns(steps))
            }
            KeyCode::Char('l') | KeyCode::Char('L') | KeyCode::Right => {
                let steps = self.steps(1);
                self.act(Command::ScrollColumns(steps))
            }
            // The surface relays every key home rather than acting on it, so
            // the scrolling a web engine would have done itself is the page's
            // to do: without these, the keys a reader reaches for first would
            // do nothing at all.
            KeyCode::Char(' ') | KeyCode::Space | KeyCode::PageDown => {
                self.act(Command::ScrollScreens(FULL_SCREEN))
            }
            KeyCode::Char('b') | KeyCode::PageUp => self.act(Command::ScrollScreens(-FULL_SCREEN)),
            KeyCode::Char('G') | KeyCode::End => match self.pending.count {
                Some(page) => self.act(Command::GoToPage(page)),
                None => self.act(Command::GoToLastPage),
            },
            KeyCode::Home => self.act(Command::GoToPage(1)),
            KeyCode::Char(']') => {
                let pages = self.steps(1);
                self.act(Command::GoToPageBy(pages))
            }
            KeyCode::Char('[') => {
                let pages = self.steps(-1);
                self.act(Command::GoToPageBy(pages))
            }
            // `+` on its own, and `=` only while Shift is held: bare `=` is
            // the fit, and a guard covering both alternatives would send a
            // plain `+` to it.
            KeyCode::Char('+') => self.act(Command::Zoom(1)),
            KeyCode::Char('=') if key.shift => self.act(Command::Zoom(1)),
            KeyCode::Char('-') => self.act(Command::Zoom(-1)),
            KeyCode::Char('=') => self.act(Command::FitWidth),
            KeyCode::Char('q') => PageOutcome::Close,
            KeyCode::Escape => {
                // Whatever was half-typed, abandoned, the way Escape does
                // everywhere else. Consumed even when nothing was pending, so
                // Escape in a document never falls through to the pane.
                self.pending = Pending::default();
                PageOutcome::Consumed
            }
            _ => PageOutcome::Ignored,
        }
    }
}

impl Page for PdfPage {
    fn title(&self) -> String {
        self.file_name()
    }

    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> PageContent {
        // Only the header. Everything below it belongs to the surface, which
        // the host draws over these rows, so painting anything there would
        // just be hidden.
        PageContent::new(vec![self.title_row(), PageRow::new()])
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        // A prefix owns the key after it, before anything else looks at it:
        // the `t` of `zt` is not the text tool, and the `g` of `gg` is not a
        // fresh prefix.
        match self.pending.prefix {
            Prefix::Find(kind) => return self.on_find_key(kind, key),
            Prefix::Goto => return self.on_goto_key(key),
            Prefix::None => {}
            Prefix::Place => return self.on_place_key(key),
        }
        if key.ctrl {
            return self.on_ctrl_key(key);
        }
        if key.alt {
            return PageOutcome::Ignored;
        }
        // A count is typed ahead of the motion it multiplies, so the digits
        // are taken before any key is read as a command.
        if let KeyCode::Char(digit) = key.code {
            if self.take_count_digit(digit) {
                return PageOutcome::Consumed;
            }
        }
        match key.code {
            KeyCode::Char('g') => {
                self.pending.prefix = Prefix::Goto;
                PageOutcome::Consumed
            }
            KeyCode::Char('z') => {
                self.pending.prefix = Prefix::Place;
                PageOutcome::Consumed
            }
            // Searching is Winter's one convention that holds whichever mode
            // the document is being read in, so these are answered before the
            // mode says what a key means. A search finds text, and text is
            // where the cursor lives, so the surface puts one there.
            KeyCode::Char('/') => self.ask_search(true),
            KeyCode::Char('?') => self.ask_search(false),
            KeyCode::Char('n') => self.search_step(false),
            KeyCode::Char('N') => self.search_step(true),
            _ => match self.mode {
                Mode::Cursor => self.on_cursor_key(key),
                Mode::Normal => self.on_normal_key(key),
            },
        }
    }

    fn hint(&self) -> Option<PageHint> {
        // The same card the terminal's own prefixes use, so a half-typed
        // command in a document looks like a half-typed command anywhere.
        let items = match self.pending.prefix {
            Prefix::Find(_) => vec![(
                "{char}".to_string(),
                "the character on this line to move to".to_string(),
            )],
            Prefix::Goto => vec![(
                "g".to_string(),
                "first page, or the page counted to".to_string(),
            )],
            Prefix::None => return None,
            Prefix::Place => vec![
                ("t".to_string(), "this page to the top".to_string()),
                ("z".to_string(), "this page to the middle".to_string()),
                ("b".to_string(), "this page to the bottom".to_string()),
            ],
        };
        Some(PageHint {
            items,
            title: self.typed(),
        })
    }

    fn on_scroll(&mut self, lines: isize) -> PageOutcome {
        // The wheel reports rows the way the grid counts them, positive
        // upwards, which is the opposite of how far the document moves.
        self.act(Command::ScrollLines(-lines))
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        match (reply.tag, reply.answer) {
            (ASK_SEARCH, Some(pattern)) => self.start_search(pattern, true),
            (ASK_SEARCH_BACK, Some(pattern)) => self.start_search(pattern, false),
            _ => {
                // A prompt escaped out of takes the half-typed command with
                // it, the way Escape does in the document itself.
                self.pending = Pending::default();
                PageOutcome::Consumed
            }
        }
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn surface(&self) -> Option<PageSurface> {
        Some(PageSurface {
            assets: assets::asset,
            document: self.path.clone(),
            entry: assets::VIEWER_PATH.to_string(),
            top_row: HEADER_ROWS,
        })
    }

    fn take_surface_script(&mut self) -> Option<String> {
        self.script.take()
    }

    fn on_surface_message(&mut self, message: String) -> PageOutcome {
        let Ok(report) = serde_json::from_str::<SurfaceReport>(&message) else {
            return PageOutcome::Consumed;
        };
        if let Some(error) = report.error {
            self.message = Some(error);
        }
        if let Some(page) = report.page {
            self.page = page;
        }
        if let Some(pages) = report.pages {
            self.pages = pages;
        }
        if let Some(percent) = report.percent {
            self.percent = percent;
        }
        if let Some(caret) = report.caret {
            self.caret_line = caret.line;
            self.page = caret.page;
        }
        // The surface has the last word on whether there is a cursor: asking
        // for one in a document with no text at all leaves it without.
        if let Some(mode) = report.mode.as_deref() {
            self.mode = match mode {
                "normal" => Mode::Normal,
                _ => Mode::Cursor,
            };
            self.visual = match mode {
                "char" => Visual::Char,
                "line" => Visual::Line,
                _ => Visual::None,
            };
        }
        // A yank is the one thing a surface asks the host to do rather than
        // report: only the host can reach the clipboard.
        if let Some(text) = report.yank {
            return PageOutcome::Yank(text);
        }
        // Vim's own `Ctrl-G` answers with the file, where you are in it, and
        // how far through: the surface knows the last two, this knows the
        // first.
        if report.announce.unwrap_or(false) {
            self.message = Some(format!(
                "{}, page {} of {}, {}%",
                self.file_name(),
                self.page.max(1),
                self.pages,
                self.percent
            ));
        }
        PageOutcome::Consumed
    }
}

// ========================================================================
// Free functions
// ========================================================================

impl PagePlace {
    /// The name `viewer.mjs` and `caret.mjs` both know this by.
    fn as_js(self) -> &'static str {
        match self {
            PagePlace::Bottom => "bottom",
            PagePlace::Center => "center",
            PagePlace::Top => "top",
        }
    }
}

/// A character as a JavaScript string literal, so a quote or a backslash
/// typed after `f` cannot end the script early or escape the next thing in
/// it.
fn escape_char(ch: char) -> String {
    escape_text(&ch.to_string())
}

/// Text as a JavaScript string literal, for the same reason: what a reader
/// searched for goes into a script, and a quote in it would otherwise close
/// the one it sits in.
fn escape_text(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_else(|_| "\"\"".to_string())
}

/// `width` spaces, for indenting a row.
fn pad(width: usize) -> String {
    " ".repeat(width)
}

impl FindKind {
    /// The name `caret.mjs` knows this by, which is vim's own key for it.
    fn as_js(self) -> &'static str {
        match self {
            FindKind::Backward => "F",
            FindKind::BackwardTill => "T",
            FindKind::Forward => "f",
            FindKind::ForwardTill => "t",
        }
    }
}

impl Motion {
    /// The name `caret.mjs` knows this by.
    fn as_js(self) -> &'static str {
        match self {
            Motion::DocEnd => "docEnd",
            Motion::DocStart => "docStart",
            Motion::Down => "down",
            Motion::Left => "left",
            Motion::LineEnd => "lineEnd",
            Motion::LineFirstNonBlank => "lineFirstNonBlank",
            Motion::LineStart => "lineStart",
            Motion::Page => "page",
            Motion::ParagraphBackward => "paragraphBackward",
            Motion::ParagraphForward => "paragraphForward",
            Motion::Right => "right",
            Motion::SentenceBackward => "sentenceBackward",
            Motion::SentenceForward => "sentenceForward",
            Motion::Up => "up",
            Motion::ViewBottom => "viewBottom",
            Motion::ViewMiddle => "viewMiddle",
            Motion::ViewTop => "viewTop",
            Motion::WordBackward => "wordBackward",
            Motion::WordEnd => "wordEnd",
            Motion::WordForward => "wordForward",
        }
    }
}

impl Operator {
    /// The name `caret.mjs` knows this by.
    fn as_js(self) -> &'static str {
        match self {
            Operator::Move => "move",
            Operator::Yank => "yank",
        }
    }
}

impl Visual {
    /// The name `caret.mjs` knows this by.
    fn as_js(self) -> &'static str {
        match self {
            Visual::Char => "char",
            Visual::Line => "line",
            Visual::None => "none",
        }
    }
}

/// The script that carries out `command` in the surface.
///
/// Every one names a function `viewer.mjs` puts on `window.winterPdf`. A
/// rename on either side leaves a key doing nothing at all, with nothing said
/// anywhere, which is what `test_every_command_names_a_primitive_the_viewer_defines`
/// exists to catch.
fn script(command: Command) -> String {
    match command {
        Command::CaretEnter => "window.winterPdf.caretEnter();".to_string(),
        Command::CaretFind(find) => format!(
            "window.winterPdf.caretFind('{}','{}',{},{});",
            find.operator.as_js(),
            find.kind.as_js(),
            escape_char(find.ch),
            find.count
        ),
        Command::CaretLeave => "window.winterPdf.caretLeave();".to_string(),
        Command::CaretMotion(motion) => format!(
            "window.winterPdf.caretMotion('{}','{}',{});",
            motion.operator.as_js(),
            motion.motion.as_js(),
            motion.count
        ),
        Command::CaretPlaceLine(place) => {
            format!("window.winterPdf.caretPlaceLine('{}');", place.as_js())
        }
        Command::CaretRepeatFind(repeat) => format!(
            "window.winterPdf.caretRepeatFind('{}',{},{});",
            repeat.operator.as_js(),
            repeat.reverse,
            repeat.count
        ),
        Command::CaretSearch(search) => format!(
            "window.winterPdf.caretSearch('{}',{},{},{});",
            search.operator.as_js(),
            escape_text(&search.pattern),
            search.forward,
            search.count
        ),
        Command::CaretSearchStep(step) => format!(
            "window.winterPdf.caretSearchStep('{}',{},{});",
            step.operator.as_js(),
            step.reverse,
            step.count
        ),
        Command::CaretSwapEnds => "window.winterPdf.caretSwapEnds();".to_string(),
        Command::CaretVisual(visual) => {
            format!("window.winterPdf.caretVisual('{}');", visual.as_js())
        }
        Command::CaretYankLines(count) => {
            format!("window.winterPdf.caretYankLines({count});")
        }
        Command::CaretYankSelection => "window.winterPdf.caretYankSelection();".to_string(),
        Command::FitWidth => "window.winterPdf.fitWidth();".to_string(),
        Command::GoToLastPage => "window.winterPdf.goToLastPage();".to_string(),
        Command::GoToPage(number) => format!("window.winterPdf.goToPage({number});"),
        Command::GoToPageBy(delta) => format!("window.winterPdf.goToPageBy({delta});"),
        Command::Place(place) => format!("window.winterPdf.placePage('{}');", place.as_js()),
        Command::ReportPosition => "window.winterPdf.reportPosition();".to_string(),
        Command::ScrollColumns(steps) => format!("window.winterPdf.scrollColumns({steps});"),
        Command::ScrollLines(lines) => format!("window.winterPdf.scrollLines({lines});"),
        Command::ScrollScreens(fraction) => format!("window.winterPdf.scrollScreens({fraction});"),
        Command::Zoom(direction) => format!("window.winterPdf.zoom({direction});"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn page() -> PdfPage {
        PdfPage::new(PathBuf::from("/tmp/paper.pdf"))
    }

    fn press(code: KeyCode) -> Key {
        Key::plain(code)
    }

    /// Type `keys` as plain presses and hand back the last script the page
    /// queued, which is the one thing the surface would actually run.
    fn typed(pdf: &mut PdfPage, keys: &str) -> Option<String> {
        let mut last = None;
        for key in keys.chars() {
            pdf.on_key(&press(KeyCode::Char(key)));
            if let Some(script) = pdf.take_surface_script() {
                last = Some(script);
            }
        }
        last
    }

    #[test]
    fn test_only_pdf_paths_are_claimed() {
        // The host routes on this, so a miss here sends a PDF to the editor,
        // which refuses it as binary, and sends a `.pdfx` to a viewer that
        // cannot read it.
        assert!(PdfPage::handles(Path::new("a/paper.pdf")));
        assert!(PdfPage::handles(Path::new("a/PAPER.PDF")));
        assert!(!PdfPage::handles(Path::new("a/paper.pdf.gz")));
        assert!(!PdfPage::handles(Path::new("a/paper")));
    }

    #[test]
    fn test_a_count_multiplies_the_motion_it_precedes() {
        // The whole point of the grammar: `3j` is three lines, not one, and
        // not a jump to line three.
        let mut pdf = page();
        assert_eq!(
            typed(&mut pdf, "3j").as_deref(),
            Some("window.winterPdf.scrollLines(3);")
        );
        assert_eq!(
            typed(&mut pdf, "12k").as_deref(),
            Some("window.winterPdf.scrollLines(-12);")
        );
    }

    #[test]
    fn test_a_count_is_spent_by_the_motion_that_used_it() {
        // A count that outlived its motion would silently multiply the next
        // one, which is the bug that makes a viewer feel possessed.
        let mut pdf = page();
        typed(&mut pdf, "5j");
        assert_eq!(
            typed(&mut pdf, "j").as_deref(),
            Some("window.winterPdf.scrollLines(1);"),
            "the next motion runs once"
        );
    }

    #[test]
    fn test_a_leading_zero_is_a_key_and_not_a_count() {
        // Vim's rule: `0` only counts once digits are already being
        // collected, so `10j` is ten lines while a bare `0` is not a count.
        let mut pdf = page();
        pdf.on_key(&press(KeyCode::Char('0')));
        assert_eq!(pdf.pending.count, None);
        assert_eq!(
            typed(&mut pdf, "10j").as_deref(),
            Some("window.winterPdf.scrollLines(10);")
        );
    }

    #[test]
    fn test_g_reads_its_count_as_a_page_and_not_a_repeat() {
        // `gg` and `G` are the one place a count names an absolute rather
        // than multiplying: `5G` is page five, not five pages on.
        let mut pdf = page();
        assert_eq!(
            typed(&mut pdf, "gg").as_deref(),
            Some("window.winterPdf.goToPage(1);")
        );
        assert_eq!(
            typed(&mut pdf, "7gg").as_deref(),
            Some("window.winterPdf.goToPage(7);")
        );
        assert_eq!(
            typed(&mut pdf, "5G").as_deref(),
            Some("window.winterPdf.goToPage(5);")
        );
        assert_eq!(
            typed(&mut pdf, "G").as_deref(),
            Some("window.winterPdf.goToLastPage();"),
            "a bare G is the end of the document"
        );
    }

    #[test]
    fn test_a_prefix_owns_the_key_after_it() {
        // Without this, the `t` of `zt` would be read as a command of its
        // own and the `g` of `gg` as a fresh prefix.
        let mut pdf = page();
        assert_eq!(
            typed(&mut pdf, "zt").as_deref(),
            Some("window.winterPdf.placePage('top');")
        );
        assert_eq!(
            typed(&mut pdf, "zz").as_deref(),
            Some("window.winterPdf.placePage('center');")
        );
        assert_eq!(
            typed(&mut pdf, "zb").as_deref(),
            Some("window.winterPdf.placePage('bottom');")
        );
    }

    #[test]
    fn test_an_unfinished_prefix_is_abandoned_rather_than_left_pending() {
        // A `z` followed by anything else must not leave the next key being
        // read as its completion.
        let mut pdf = page();
        pdf.on_key(&press(KeyCode::Char('z')));
        pdf.on_key(&press(KeyCode::Char('x')));
        assert_eq!(pdf.pending, Pending::default());
        assert_eq!(
            typed(&mut pdf, "j").as_deref(),
            Some("window.winterPdf.scrollLines(1);")
        );
    }

    #[test]
    fn test_escape_abandons_a_half_typed_command() {
        let mut pdf = page();
        typed(&mut pdf, "12");
        pdf.on_key(&press(KeyCode::Escape));
        assert_eq!(pdf.pending, Pending::default());
        assert_eq!(
            typed(&mut pdf, "j").as_deref(),
            Some("window.winterPdf.scrollLines(1);")
        );
    }

    #[test]
    fn test_the_half_and_full_screen_scrolls_differ() {
        // Ctrl-d and Ctrl-f both scroll down; a viewer where they move the
        // same distance has quietly lost one of them.
        let mut pdf = page();
        pdf.on_key(&Key::with_ctrl(KeyCode::Char('d')));
        let half = pdf.take_surface_script().expect("Ctrl-d scrolls");
        pdf.on_key(&Key::with_ctrl(KeyCode::Char('f')));
        let full = pdf.take_surface_script().expect("Ctrl-f scrolls");
        assert_ne!(half, full);
        assert!(half.contains("0.5"), "{half}");
        assert!(full.contains("0.9"), "{full}");
    }

    #[test]
    fn test_the_keys_a_web_engine_would_have_handled_itself_are_bound() {
        // The surface relays every key home instead of acting on it, so the
        // scrolling the engine used to do for free is now the page's. An
        // unbound one here is a reader pressing Space and nothing happening.
        for code in [
            KeyCode::Space,
            KeyCode::PageDown,
            KeyCode::PageUp,
            KeyCode::Home,
            KeyCode::End,
            KeyCode::Down,
            KeyCode::Up,
            KeyCode::Left,
            KeyCode::Right,
        ] {
            let mut pdf = page();
            assert_eq!(
                pdf.on_key(&press(code)),
                PageOutcome::Consumed,
                "{code:?} must move the document"
            );
            assert!(
                pdf.take_surface_script().is_some(),
                "{code:?} reaches the surface"
            );
        }
    }

    #[test]
    fn test_every_command_names_a_primitive_the_viewer_defines() {
        // The two sides only ever meet in a string, so a renamed primitive
        // is a key that stops working with nothing said in any log.
        const VIEWER: &str = include_str!("viewer.mjs");
        for command in [
            Command::CaretEnter,
            Command::CaretFind(CaretFind {
                ch: 'x',
                count: 1,
                kind: FindKind::Forward,
                operator: Operator::Move,
            }),
            Command::CaretLeave,
            Command::CaretMotion(CaretMotion {
                count: 1,
                motion: Motion::WordForward,
                operator: Operator::Move,
            }),
            Command::CaretPlaceLine(PagePlace::Top),
            Command::CaretRepeatFind(CaretRepeat {
                count: 1,
                operator: Operator::Move,
                reverse: false,
            }),
            Command::CaretSearch(CaretSearch {
                count: 1,
                forward: true,
                operator: Operator::Move,
                pattern: "text".to_string(),
            }),
            Command::CaretSearchStep(CaretSearchStep {
                count: 1,
                operator: Operator::Move,
                reverse: false,
            }),
            Command::CaretSwapEnds,
            Command::CaretVisual(Visual::Char),
            Command::CaretYankLines(1),
            Command::CaretYankSelection,
            Command::FitWidth,
            Command::GoToLastPage,
            Command::GoToPage(1),
            Command::GoToPageBy(1),
            Command::Place(PagePlace::Bottom),
            Command::Place(PagePlace::Center),
            Command::Place(PagePlace::Top),
            Command::ReportPosition,
            Command::ScrollColumns(1),
            Command::ScrollLines(1),
            Command::ScrollScreens(1.0),
            Command::Zoom(1),
        ] {
            let js = script(command.clone());
            let name = js
                .trim_start_matches("window.winterPdf.")
                .split('(')
                .next()
                .expect("a script always has a name");
            assert!(
                VIEWER.contains(&format!("{name}(")) || VIEWER.contains(&format!("{name}:")),
                "{command:?} calls {name}, which viewer.mjs does not define"
            );
        }
    }

    #[test]
    fn test_every_caret_command_the_viewer_forwards_is_exported() {
        // `viewer.mjs` hands each of these straight on to the cursor module.
        // A name it forwards that the module does not export is a failure at
        // import time, which takes the whole surface down blank with the
        // document never drawn at all.
        const CARET: &str = include_str!("caret.mjs");
        const VIEWER: &str = include_str!("viewer.mjs");
        let mut checked = 0;
        for (at, _) in VIEWER.match_indices("caret.") {
            // The import names the file rather than the module object.
            if VIEWER[..at].ends_with('/') {
                continue;
            }
            let rest = &VIEWER[at + "caret.".len()..];
            let end = rest
                .find(|c: char| !c.is_ascii_alphanumeric())
                .unwrap_or(rest.len());
            let name = &rest[..end];
            assert!(
                CARET.contains(&format!("export function {name}("))
                    || CARET.contains(&format!("export async function {name}(")),
                "viewer.mjs forwards caret.{name}, which caret.mjs does not export"
            );
            checked += 1;
        }
        assert!(checked > 0, "the forwarding is being read at all");
    }

    #[test]
    fn test_every_motion_names_a_case_the_caret_module_handles() {
        // A motion the caret module has no case for falls through its switch
        // and returns null, so the key does nothing at all and says nothing.
        const CARET: &str = include_str!("caret.mjs");
        for motion in [
            Motion::DocEnd,
            Motion::DocStart,
            Motion::Down,
            Motion::Left,
            Motion::LineEnd,
            Motion::LineFirstNonBlank,
            Motion::LineStart,
            Motion::Page,
            Motion::ParagraphBackward,
            Motion::ParagraphForward,
            Motion::Right,
            Motion::SentenceBackward,
            Motion::SentenceForward,
            Motion::Up,
            Motion::ViewBottom,
            Motion::ViewMiddle,
            Motion::ViewTop,
            Motion::WordBackward,
            Motion::WordEnd,
            Motion::WordForward,
        ] {
            let name = motion.as_js();
            assert!(
                CARET.contains(&format!("'{name}'")),
                "{motion:?} is sent as {name}, which caret.mjs does not name"
            );
        }
        for kind in [
            FindKind::Backward,
            FindKind::BackwardTill,
            FindKind::Forward,
            FindKind::ForwardTill,
        ] {
            let name = kind.as_js();
            assert!(
                CARET.contains(&format!("'{name}'")),
                "{kind:?} is sent as {name}, which caret.mjs does not name"
            );
        }
    }

    #[test]
    fn test_i_puts_a_cursor_in_the_text_and_escape_takes_it_out() {
        let mut pdf = page();
        assert_eq!(pdf.mode, Mode::Normal);
        assert_eq!(
            typed(&mut pdf, "i").as_deref(),
            Some("window.winterPdf.caretEnter();")
        );
        assert_eq!(pdf.mode, Mode::Cursor, "the next key is read as a motion");

        pdf.on_key(&press(KeyCode::Escape));
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.caretLeave();")
        );
        assert_eq!(pdf.mode, Mode::Normal);
    }

    #[test]
    fn test_the_same_key_means_different_things_in_the_two_modes() {
        // `b` is a screen back with no cursor and a word back with one, which
        // is exactly what a mode is for.
        let mut pdf = page();
        assert_eq!(
            typed(&mut pdf, "b").as_deref(),
            Some("window.winterPdf.scrollScreens(-0.9);")
        );
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "b").as_deref(),
            Some("window.winterPdf.caretMotion('move','wordBackward',1);")
        );
    }

    #[test]
    fn test_a_count_reaches_a_cursor_motion() {
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "3w").as_deref(),
            Some("window.winterPdf.caretMotion('move','wordForward',3);")
        );
    }

    #[test]
    fn test_yank_composes_with_the_motion_that_follows_it() {
        // The operator is the half of vim's grammar that a viewer without it
        // cannot express at all: `yw` has to mean the words `w` would cross.
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "yw").as_deref(),
            Some("window.winterPdf.caretMotion('yank','wordForward',1);")
        );
        assert_eq!(
            typed(&mut pdf, "3ye").as_deref(),
            Some("window.winterPdf.caretMotion('yank','wordEnd',3);")
        );
        assert_eq!(
            typed(&mut pdf, "y$").as_deref(),
            Some("window.winterPdf.caretMotion('yank','lineEnd',1);")
        );
    }

    #[test]
    fn test_yank_doubled_takes_whole_lines() {
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "yy").as_deref(),
            Some("window.winterPdf.caretYankLines(1);")
        );
        assert_eq!(
            typed(&mut pdf, "4yy").as_deref(),
            Some("window.winterPdf.caretYankLines(4);")
        );
        assert_eq!(
            typed(&mut pdf, "Y").as_deref(),
            Some("window.winterPdf.caretYankLines(1);"),
            "Y acts at once rather than waiting for a motion"
        );
    }

    #[test]
    fn test_find_waits_for_its_character_and_composes_with_yank() {
        let mut pdf = page();
        typed(&mut pdf, "i");
        pdf.on_key(&press(KeyCode::Char('f')));
        assert!(
            pdf.take_surface_script().is_none(),
            "f alone is not a motion yet"
        );
        pdf.on_key(&press(KeyCode::Char('x')));
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.caretFind('move','f',\"x\",1);")
        );

        assert_eq!(
            typed(&mut pdf, "2ytq").as_deref(),
            Some("window.winterPdf.caretFind('yank','t',\"q\",2);")
        );
    }

    #[test]
    fn test_a_quote_typed_after_find_cannot_break_out_of_the_script() {
        // The character goes into a script as a literal, so a quote or a
        // backslash would otherwise end the string and run what followed.
        let mut pdf = page();
        typed(&mut pdf, "i");
        pdf.on_key(&press(KeyCode::Char('f')));
        pdf.on_key(&press(KeyCode::Char('\'')));
        let js = pdf.take_surface_script().expect("a find runs");
        assert!(js.contains("\"'\""), "{js}");

        pdf.on_key(&press(KeyCode::Char('f')));
        pdf.on_key(&press(KeyCode::Char('\\')));
        let js = pdf.take_surface_script().expect("a find runs");
        assert!(js.contains("\"\\\\\""), "{js}");
    }

    #[test]
    fn test_a_search_asks_the_host_for_what_to_look_for() {
        // `/` is Winter's one convention that holds in every tool, and the
        // question belongs to the terminal's own prompt rather than to any
        // chrome the surface could draw for itself.
        let mut pdf = page();
        let asked = pdf.on_key(&press(KeyCode::Char('/')));
        let PageOutcome::Prompt(request) = asked else {
            panic!("`/` asks what to look for, got {asked:?}");
        };
        assert_eq!(request.label, "/");
        assert_eq!(request.tag, ASK_SEARCH);
        assert_eq!(
            pdf.on_prompt(PromptReply {
                answer: Some("figure".to_string()),
                tag: ASK_SEARCH,
            }),
            PageOutcome::Consumed
        );
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.caretSearch('move',\"figure\",true,1);")
        );
    }

    #[test]
    fn test_a_search_backwards_and_a_cancelled_one_are_told_apart() {
        let mut pdf = page();
        let asked = pdf.on_key(&press(KeyCode::Char('?')));
        let PageOutcome::Prompt(request) = asked else {
            panic!("`?` asks what to look for, got {asked:?}");
        };
        assert_eq!(request.tag, ASK_SEARCH_BACK);
        pdf.on_prompt(PromptReply {
            answer: Some("figure".to_string()),
            tag: ASK_SEARCH_BACK,
        });
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.caretSearch('move',\"figure\",false,1);")
        );
        // A prompt escaped out of leaves the document exactly as it was.
        pdf.on_key(&press(KeyCode::Char('/')));
        pdf.on_prompt(PromptReply {
            answer: None,
            tag: ASK_SEARCH,
        });
        assert_eq!(pdf.take_surface_script(), None);
    }

    #[test]
    fn test_a_pattern_carrying_a_quote_cannot_break_out_of_the_script() {
        // Everything typed at the prompt ends up inside a script literal, so
        // a quote in it would otherwise close the string it sits in.
        let mut pdf = page();
        pdf.on_key(&press(KeyCode::Char('/')));
        pdf.on_prompt(PromptReply {
            answer: Some("a\"b\\".to_string()),
            tag: ASK_SEARCH,
        });
        let js = pdf
            .take_surface_script()
            .expect("a search reaches the surface");
        assert!(
            js.contains(r#"caretSearch('move',"a\"b\\",true,1);"#),
            "the pattern stays one literal: {js}"
        );
    }

    #[test]
    fn test_n_needs_a_search_to_repeat_and_then_goes_either_way() {
        let mut pdf = page();
        pdf.on_key(&press(KeyCode::Char('n')));
        assert_eq!(pdf.take_surface_script(), None, "nothing to repeat yet");
        assert!(pdf.message.is_some(), "and the header says so");

        pdf.on_key(&press(KeyCode::Char('/')));
        pdf.on_prompt(PromptReply {
            answer: Some("figure".to_string()),
            tag: ASK_SEARCH,
        });
        pdf.take_surface_script();
        assert_eq!(
            typed(&mut pdf, "n").as_deref(),
            Some("window.winterPdf.caretSearchStep('move',false,1);")
        );
        assert_eq!(
            typed(&mut pdf, "3N").as_deref(),
            Some("window.winterPdf.caretSearchStep('move',true,3);"),
            "`N` goes the other way, and a count still multiplies it"
        );
    }

    #[test]
    fn test_a_count_and_an_operator_survive_the_question() {
        // The prompt takes the keys while it is up, so the count and the
        // operator typed before `/` have to still be there when the answer
        // comes back: `y2/text` is a yank of what two matches cover.
        let mut pdf = page();
        typed(&mut pdf, "i");
        typed(&mut pdf, "y2");
        pdf.on_key(&press(KeyCode::Char('/')));
        pdf.on_prompt(PromptReply {
            answer: Some("text".to_string()),
            tag: ASK_SEARCH,
        });
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.caretSearch('yank',\"text\",true,2);")
        );
        assert_eq!(pdf.pending, Pending::default(), "and are spent by it");
    }

    #[test]
    fn test_semicolon_and_comma_repeat_the_last_find_either_way() {
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, ";").as_deref(),
            Some("window.winterPdf.caretRepeatFind('move',false,1);")
        );
        assert_eq!(
            typed(&mut pdf, "2,").as_deref(),
            Some("window.winterPdf.caretRepeatFind('move',true,2);")
        );
    }

    #[test]
    fn test_visual_toggles_and_yank_takes_what_is_selected() {
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "v").as_deref(),
            Some("window.winterPdf.caretVisual('char');")
        );
        assert_eq!(pdf.visual, Visual::Char);
        assert_eq!(
            typed(&mut pdf, "V").as_deref(),
            Some("window.winterPdf.caretVisual('line');"),
            "V switches which kind rather than dropping it"
        );
        assert_eq!(
            typed(&mut pdf, "y").as_deref(),
            Some("window.winterPdf.caretYankSelection();")
        );
        assert_eq!(pdf.visual, Visual::None, "a yank ends the selection");
    }

    #[test]
    fn test_pressing_the_same_visual_key_again_drops_the_selection() {
        let mut pdf = page();
        typed(&mut pdf, "i");
        typed(&mut pdf, "v");
        assert_eq!(
            typed(&mut pdf, "v").as_deref(),
            Some("window.winterPdf.caretVisual('none');")
        );
        assert_eq!(pdf.visual, Visual::None);
    }

    #[test]
    fn test_escape_steps_back_one_thing_at_a_time() {
        // Vim's own rule: a half-typed command, then the selection, then the
        // cursor. An Escape that did all three at once would lose a
        // selection to a stray keystroke.
        let mut pdf = page();
        typed(&mut pdf, "iv");
        typed(&mut pdf, "12");
        pdf.on_key(&press(KeyCode::Escape));
        assert_eq!(pdf.pending, Pending::default(), "the count goes first");
        assert_eq!(pdf.visual, Visual::Char, "the selection survives it");

        pdf.on_key(&press(KeyCode::Escape));
        assert_eq!(pdf.visual, Visual::None);
        assert_eq!(pdf.mode, Mode::Cursor, "the cursor survives it");

        pdf.on_key(&press(KeyCode::Escape));
        assert_eq!(pdf.mode, Mode::Normal);
    }

    #[test]
    fn test_the_prefixes_mean_the_cursor_once_there_is_one() {
        // `gg` is the top of the window with no cursor and the start of the
        // text with one; `zt` moves the window either way, but around a
        // different thing.
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "gg").as_deref(),
            Some("window.winterPdf.caretMotion('move','docStart',1);")
        );
        assert_eq!(
            typed(&mut pdf, "6gg").as_deref(),
            Some("window.winterPdf.caretMotion('move','page',6);")
        );
        assert_eq!(
            typed(&mut pdf, "G").as_deref(),
            Some("window.winterPdf.caretMotion('move','docEnd',1);")
        );
        assert_eq!(
            typed(&mut pdf, "zt").as_deref(),
            Some("window.winterPdf.caretPlaceLine('top');")
        );
    }

    #[test]
    fn test_the_window_keys_still_work_with_a_cursor_in_the_text() {
        // A cursor must not cost the reader the ability to page through the
        // document it is sitting in.
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(
            typed(&mut pdf, "]").as_deref(),
            Some("window.winterPdf.goToPageBy(1);")
        );
        pdf.on_key(&press(KeyCode::Space));
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.scrollScreens(0.9);")
        );
        pdf.on_key(&Key::with_ctrl(KeyCode::Char('d')));
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.scrollScreens(0.5);")
        );
    }

    #[test]
    fn test_a_yank_reaches_the_clipboard_rather_than_the_header() {
        // Only the host can reach the clipboard, so the surface asks for it
        // and the outcome has to carry the text out.
        let mut pdf = page();
        let outcome = pdf.on_surface_message(r#"{"yank":"some text"}"#.to_string());
        assert_eq!(outcome, PageOutcome::Yank("some text".to_string()));
    }

    #[test]
    fn test_the_surface_has_the_last_word_on_whether_there_is_a_cursor() {
        // Asking for a cursor in a document with no text at all leaves it
        // without one, and the host has to follow rather than keep reading
        // keys as motions nothing will act on.
        let mut pdf = page();
        typed(&mut pdf, "i");
        assert_eq!(pdf.mode, Mode::Cursor);
        pdf.on_surface_message(r#"{"mode":"normal","caret":null}"#.to_string());
        assert_eq!(pdf.mode, Mode::Normal);
    }

    #[test]
    fn test_zoom_in_and_fit_width_are_told_apart() {
        let mut pdf = page();
        pdf.on_key(&press(KeyCode::Char('+')));
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.zoom(1);")
        );

        pdf.on_key(&press(KeyCode::Char('=')));
        assert_eq!(
            pdf.take_surface_script().as_deref(),
            Some("window.winterPdf.fitWidth();")
        );
    }

    #[test]
    fn test_a_bound_key_queues_its_script_once() {
        let mut pdf = page();
        assert_eq!(
            pdf.on_key(&press(KeyCode::Char('j'))),
            PageOutcome::Consumed
        );
        assert!(pdf.take_surface_script().is_some());
        assert!(
            pdf.take_surface_script().is_none(),
            "a script must run once, not on every frame after"
        );
    }

    #[test]
    fn test_an_unbound_key_falls_through_to_the_host() {
        // A page that consumed everything would swallow the chords that
        // switch panes and tabs while a PDF is open.
        let mut pdf = page();
        assert_eq!(pdf.on_key(&press(KeyCode::Char('Z'))), PageOutcome::Ignored);
        assert_eq!(
            pdf.on_key(&Key::with_alt(KeyCode::Char('h'))),
            PageOutcome::Ignored,
            "the pane chords have to survive a document"
        );
        assert!(pdf.take_surface_script().is_none());
    }

    #[test]
    fn test_a_pending_prefix_offers_its_own_key_hints() {
        // The host draws these the way it draws the terminal's, so a
        // half-typed command in a document says what finishes it.
        let mut pdf = page();
        assert!(pdf.hint().is_none(), "nothing is pending yet");
        pdf.on_key(&press(KeyCode::Char('z')));
        let hint = pdf.hint().expect("z is waiting for its second key");
        let keys: Vec<String> = hint.items.into_iter().map(|(key, _)| key).collect();
        assert_eq!(keys, vec!["t", "z", "b"]);
    }

    #[test]
    fn test_the_title_stays_quiet_until_the_surface_has_counted_the_pages() {
        let mut pdf = page();
        assert_eq!(pdf.position(), "", "nothing is known yet");

        pdf.on_surface_message(r#"{"page":3,"pages":18}"#.to_string());
        assert_eq!(pdf.position(), "page 3 / 18");
    }

    #[test]
    fn test_asking_where_you_are_names_the_file_the_page_knows() {
        // The surface counts the pages and the percentage; only the host
        // knows which file is open, so neither can answer on its own.
        let mut pdf = page();
        pdf.on_surface_message(r#"{"announce":true,"page":3,"pages":18,"percent":42}"#.to_string());
        assert_eq!(pdf.message.as_deref(), Some("paper.pdf, page 3 of 18, 42%"));
    }

    #[test]
    fn test_a_surface_error_is_shown_and_malformed_output_is_not() {
        let mut pdf = page();
        pdf.on_surface_message(r#"{"error":"broken xref"}"#.to_string());
        assert_eq!(pdf.message.as_deref(), Some("broken xref"));

        pdf.on_surface_message("not json at all".to_string());
        assert_eq!(
            pdf.message.as_deref(),
            Some("broken xref"),
            "output the page cannot parse must leave what it knows alone"
        );
    }
}

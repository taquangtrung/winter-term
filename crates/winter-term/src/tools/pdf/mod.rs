//! PDF: one document, drawn by Winter itself.
//!
//! The pages are drawn on a thread of their own and placed over the pane as
//! pictures; their text is rebuilt from the glyphs, for a vim cursor to move
//! through, search, select, and copy.

mod caret;
mod document;
mod layout;
mod outline;
mod outline_list;
mod text;
mod toolbar;
mod worker;

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::{format_key, Key, KeyCode};
use crate::model::paint::{Frame, IconKind, Point, Row};
use crate::model::path::abbreviate_home;
use crate::model::picture::{Content, Pixels, Tile};
use crate::model::textedit::modal::EditMode;
use crate::model::textedit::search::Search;
use crate::model::tool::{
    menu_groups, Notice, OpenTarget, PromptMode, PromptReply, PromptRequest, SearchReport, Tool,
    ToolHint, ToolMenuItem, ToolOutcome, ToolStatus,
};
use crate::tools::background::POLL;

use caret::{Caret, Effect, FindKind, Motion, Operator, Pos, Visual};
use document::Document;
use layout::Column;
use outline::Target;
use outline_list::{OutlineList, RowHit};
use text::union;
use toolbar::{Bar, Button};
use worker::{Reply, Request, Worker};

// ========================================================================
// Constants
// ========================================================================

/// Pane pixels one `j`/`k` scrolls, and one `h`/`l` sideways.
const LINE_PX: f32 = 48.0;
const COLUMN_PX: f32 = 48.0;

/// How much one zoom step scales by.
const ZOOM_STEP: f32 = 1.2;

/// The smallest and largest scale a zoom reaches, in pixels a point.
const MIN_SCALE: f32 = 0.1;
const MAX_SCALE: f32 = 16.0;

/// The longest side a page is drawn at. Zoomed past it, the page is
/// stretched rather than drawn larger, which keeps one zoom from asking for
/// a texture no GPU holds or the memory to fill it.
const MAX_DRAWN_SIDE: f32 = 4096.0;

/// Pages either side of the view drawn ahead, so scrolling onto one finds it
/// drawn.
const LOOKAHEAD: usize = 1;

/// Where down the pane the page being read is reckoned from: a third of the
/// way, where the eye reads, not the top edge a page is half scrolled off.
const READING_LINE: f32 = 1.0 / 3.0;

/// The colors drawn over the pages: white for a page not drawn yet (black
/// while the pages are inverted), and over a drawn one the cursor's bar,
/// what is selected, and what a search found, in the blue every view marks
/// its current match with (the theme's `search_current_bg`), light enough
/// to read the page's own text through.
const BLANK_COLOR: [u8; 4] = [255, 255, 255, 255];
const BLANK_INVERTED_COLOR: [u8; 4] = [0, 0, 0, 255];
const CARET_COLOR: [u8; 4] = [29, 78, 216, 255];
const SELECTION_COLOR: [u8; 4] = [51, 136, 255, 90];
const FOUND_COLOR: [u8; 4] = [39, 91, 173, 110];

/// How wide the cursor's bar is drawn, and the least it is drawn tall, in
/// pane pixels: a bar on the left of its character, visible over a
/// footnote's type.
const CARET_WIDTH: f32 = 2.0;
const MIN_CARET_HEIGHT: f32 = 8.0;

/// Hands each drawn page, and each overlay, a key of its own.
static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

/// Which question a prompt's answer belongs to: what to look for down the
/// document, and what to look for back up it.
const ASK_SEARCH: &str = "search";
const ASK_SEARCH_BACK: &str = "search-back";

/// The toolbar's questions: which page to go to, and what to zoom to.
const ASK_PAGE: &str = "page";
const ASK_ZOOM: &str = "zoom";

/// The menu's entries, a group at a time, by the tables' keys and named
/// shorter than the help list names them: finding a place, the zoom, and
/// the colors. Copying the selection leads them while there is one.
const MENU_COPY: &str = "Copy";
const MENU: [&[(Act, &str)]; 3] = [
    &[
        (Act::Search(true), "Find"),
        (Act::AskPage, "Go to Page"),
        (Act::Outline, "Outline"),
    ],
    &[
        (Act::Zoom(1), "Zoom In"),
        (Act::Zoom(-1), "Zoom Out"),
        (Act::FitWidth, "Fit Width"),
    ],
    &[(Act::Invert, "Invert Colors")],
];

/// The rows the toolbar takes at the pane's top.
const TOOLBAR_ROWS: usize = 1;

/// The most of the pane's rows the open outline takes under the toolbar.
const OUTLINE_SHARE: f32 = 0.6;

/// How a zoom typed at the prompt is told apart from a percentage: the
/// words, and their first letters, that fit the pages instead.
const FIT_WIDTH_WORDS: &[&str] = &["w", "width"];
const FIT_HEIGHT_WORDS: &[&str] = &["h", "height"];
const FIT_PAGE_WORDS: &[&str] = &["p", "page"];

/// What the zoom percentage is a percentage of: one pixel a point.
const PERCENT: f32 = 100.0;

/// Fraction of the viewport `C-d` and `C-u` move, the half-screen step
/// vim's own are named for.
const HALF_SCREEN: f32 = 0.5;

/// Fraction of the viewport `C-f`, `C-b`, and the page keys move. Short
/// of a whole screen on purpose: the lines that were at the edge stay on it,
/// so a page read straight through never skips a band of text.
const FULL_SCREEN: f32 = 0.9;

/// The largest count a motion will accept, so a key held on the number row
/// cannot ask the surface for a scroll it has to compute its way out of.
const MAX_COUNT: usize = 99_999;

/// The keys that mean the same whichever way the document is being read.
/// Searching is Winter's one convention that holds whichever mode the
/// document is read in, so these are answered before the mode says what a
/// key means. A search finds text, and text is where the cursor lives, so
/// the surface puts one there.
const COMMON_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::First,
        keys: &["g g"],
        label: "First page, or the page counted to",
    },
    Binding {
        action: Act::Place(PagePlace::Top),
        keys: &["z t"],
        label: "This page to the top",
    },
    Binding {
        action: Act::Place(PagePlace::Center),
        keys: &["z z"],
        label: "This page to the middle",
    },
    Binding {
        action: Act::Place(PagePlace::Bottom),
        keys: &["z b"],
        label: "This page to the bottom",
    },
    Binding {
        action: Act::Search(true),
        keys: &["/"],
        label: "Search down the document",
    },
    Binding {
        action: Act::Search(false),
        keys: &["?"],
        label: "Search back up the document",
    },
    Binding {
        action: Act::SearchStep(false),
        keys: &["n"],
        label: "Next match",
    },
    Binding {
        action: Act::SearchStep(true),
        keys: &["N"],
        label: "Previous match",
    },
    Binding {
        action: Act::Screens(-FULL_SCREEN),
        keys: &["C-b"],
        label: "A screen up",
    },
    Binding {
        action: Act::Screens(FULL_SCREEN),
        keys: &["C-f"],
        label: "A screen down",
    },
    Binding {
        action: Act::Screens(HALF_SCREEN),
        keys: &["C-d"],
        label: "Half a screen down",
    },
    Binding {
        action: Act::Screens(-HALF_SCREEN),
        keys: &["C-u"],
        label: "Half a screen up",
    },
    Binding {
        action: Act::Lines(1),
        keys: &["C-e"],
        label: "A line down",
    },
    Binding {
        action: Act::Lines(-1),
        keys: &["C-y"],
        label: "A line up",
    },
    Binding {
        action: Act::Position,
        keys: &["C-g"],
        label: "Say where in the document you are",
    },
];

/// The keys with no cursor in the text, ahead of [`WINDOW_KEYS`].
const NORMAL_KEYS: &[Binding<Act>] = &[Binding {
    action: Act::Cursor,
    keys: &["i"],
    label: "Put a cursor in the text",
}];

/// The keys that move the window rather than a cursor: reached with no
/// cursor, and with one for the keys [`CURSOR_KEYS`] leaves alone, so a
/// document can still be paged through with a cursor in it.
const WINDOW_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Lines(1),
        keys: &["j", "<down>"],
        label: "Scroll down",
    },
    Binding {
        action: Act::Lines(-1),
        keys: &["k", "<up>"],
        label: "Scroll up",
    },
    // Sideways, for a page zoomed in past the pane's width. Vim has no
    // vertical-only rule here and neither does the reader.
    Binding {
        action: Act::Columns(-1),
        keys: &["h", "H", "<left>"],
        label: "Scroll left",
    },
    Binding {
        action: Act::Columns(1),
        keys: &["l", "L", "<right>"],
        label: "Scroll right",
    },
    // The surface relays every key home rather than acting on it, so the
    // scrolling a web engine would have done itself is the page's to do:
    // without these, the keys a reader reaches for first would do nothing.
    Binding {
        action: Act::Screens(FULL_SCREEN),
        keys: &["SPC", "<next>"],
        label: "A screen down",
    },
    Binding {
        action: Act::Screens(-FULL_SCREEN),
        keys: &["b", "<prior>"],
        label: "A screen up",
    },
    Binding {
        action: Act::Last,
        keys: &["G", "<end>"],
        label: "Last page, or the page counted to",
    },
    Binding {
        action: Act::Home,
        keys: &["<home>"],
        label: "First page",
    },
    Binding {
        action: Act::Pages(1),
        keys: &["]"],
        label: "Next page",
    },
    Binding {
        action: Act::Pages(-1),
        keys: &["["],
        label: "Previous page",
    },
    Binding {
        action: Act::Zoom(1),
        keys: &["+", "S-="],
        label: "Zoom in",
    },
    Binding {
        action: Act::Zoom(-1),
        keys: &["-"],
        label: "Zoom out",
    },
    Binding {
        action: Act::FitWidth,
        keys: &["="],
        label: "Fit the width",
    },
    Binding {
        action: Act::AskPage,
        keys: &[":"],
        label: "Go to a page",
    },
    Binding {
        action: Act::Outline,
        keys: &["O"],
        label: "Open or close the outline",
    },
    Binding {
        action: Act::Invert,
        keys: &["I"],
        label: "Invert the pages' colors",
    },
    // Whatever was half-typed, abandoned, the way Escape does everywhere
    // else. Consumed even when nothing was pending, so Escape in a document
    // never falls through to the pane.
    Binding {
        action: Act::Abandon,
        keys: &["ESC"],
        label: "Abandon what is half-typed",
    },
];

/// The keys with a cursor in the text: vim's motions, meaning here what they
/// mean there. The three that cannot are the ones with no character to step
/// over: `h` and `l` still move the cursor, but `zt`, `zz`, and `zb` move the
/// window, and the Ctrl scrolls leave the cursor where it is.
const CURSOR_KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Caret(Motion::Left),
        keys: &["h", "<left>"],
        label: "Cursor left",
    },
    Binding {
        action: Act::Caret(Motion::Right),
        keys: &["l", "<right>"],
        label: "Cursor right",
    },
    Binding {
        action: Act::Caret(Motion::Down),
        keys: &["j", "<down>"],
        label: "Cursor down",
    },
    Binding {
        action: Act::Caret(Motion::Up),
        keys: &["k", "<up>"],
        label: "Cursor up",
    },
    Binding {
        action: Act::Caret(Motion::WordForward),
        keys: &["w"],
        label: "Next word",
    },
    Binding {
        action: Act::Caret(Motion::WordBackward),
        keys: &["b"],
        label: "Previous word",
    },
    Binding {
        action: Act::Caret(Motion::WordEnd),
        keys: &["e"],
        label: "End of the word",
    },
    Binding {
        action: Act::Caret(Motion::LineStart),
        keys: &["0", "<home>"],
        label: "Start of the line",
    },
    Binding {
        action: Act::Caret(Motion::LineFirstNonBlank),
        keys: &["^"],
        label: "First non-blank of the line",
    },
    Binding {
        action: Act::Caret(Motion::LineEnd),
        keys: &["$", "<end>"],
        label: "End of the line",
    },
    Binding {
        action: Act::Caret(Motion::ParagraphBackward),
        keys: &["{"],
        label: "Previous paragraph",
    },
    Binding {
        action: Act::Caret(Motion::ParagraphForward),
        keys: &["}"],
        label: "Next paragraph",
    },
    Binding {
        action: Act::Caret(Motion::SentenceBackward),
        keys: &["("],
        label: "Previous sentence",
    },
    Binding {
        action: Act::Caret(Motion::SentenceForward),
        keys: &[")"],
        label: "Next sentence",
    },
    // Uppercase, as vim has them: the line nearest an edge of what is on
    // screen, without scrolling to get there.
    Binding {
        action: Act::Caret(Motion::ViewTop),
        keys: &["H"],
        label: "Line nearest the top of the pane",
    },
    Binding {
        action: Act::Caret(Motion::ViewMiddle),
        keys: &["M"],
        label: "Line nearest the middle of the pane",
    },
    Binding {
        action: Act::Caret(Motion::ViewBottom),
        keys: &["L"],
        label: "Line nearest the bottom of the pane",
    },
    Binding {
        action: Act::CaretEnd,
        keys: &["G"],
        label: "End of the document, or the page counted to",
    },
    Binding {
        action: Act::Find(FindKind::Forward),
        keys: &["f"],
        label: "Find a character forward",
    },
    Binding {
        action: Act::Find(FindKind::Backward),
        keys: &["F"],
        label: "Find a character backward",
    },
    Binding {
        action: Act::Find(FindKind::ForwardTill),
        keys: &["t"],
        label: "Till a character forward",
    },
    Binding {
        action: Act::Find(FindKind::BackwardTill),
        keys: &["T"],
        label: "Till a character backward",
    },
    Binding {
        action: Act::RepeatFind(false),
        keys: &[";"],
        label: "Repeat the find",
    },
    Binding {
        action: Act::RepeatFind(true),
        keys: &[","],
        label: "Repeat the find the other way",
    },
    Binding {
        action: Act::Visual(Visual::Char),
        keys: &["v"],
        label: "Select characters",
    },
    Binding {
        action: Act::Visual(Visual::Line),
        keys: &["V"],
        label: "Select lines",
    },
    Binding {
        action: Act::SwapEnds,
        keys: &["o"],
        label: "Swap the selection's ends",
    },
    // `y` on its own is the operator; in visual mode it takes the selection
    // there and then, and `yy` takes whole lines.
    Binding {
        action: Act::Yank,
        keys: &["y"],
        label: "Copy: the selection, `yy` lines, or with a motion",
    },
    Binding {
        action: Act::YankLines,
        keys: &["Y"],
        label: "Copy lines",
    },
    Binding {
        action: Act::Leave,
        keys: &["ESC"],
        label: "Step back: what is half-typed, the selection, the cursor",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the viewer's own does, before the count and mode typed with
/// it say how.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    /// Escape with no cursor: drop what is half-typed.
    Abandon,
    /// Ask which page to go to.
    AskPage,
    /// A cursor motion.
    Caret(Motion),
    /// `G` with a cursor: the end, or the page counted to.
    CaretEnd,
    /// Scroll sideways this many steps per count.
    Columns(isize),
    /// Put a cursor in the text.
    Cursor,
    /// Wait for the character to find.
    Find(FindKind),
    /// `gg`: the first page, or with a cursor where the text starts.
    First,
    /// Fit the page to the pane's width.
    FitWidth,
    /// The first page.
    Home,
    /// Invert the pages' colors, or put them back.
    Invert,
    /// `G` with no cursor: the last page, or the page counted to.
    Last,
    /// Escape with a cursor: one step back.
    Leave,
    /// Scroll this many lines per count.
    Lines(isize),
    /// Open or close the outline.
    Outline,
    /// Step this many pages per count.
    Pages(isize),
    /// Put the page, or the cursor's line, at an edge.
    Place(PagePlace),
    /// Say where in the document the reader is.
    Position,
    /// Repeat the last find, the other way when set.
    RepeatFind(bool),
    /// Scroll by this fraction of a screen.
    Screens(f32),
    /// Ask what to look for, down the document when set.
    Search(bool),
    /// Repeat the search, the other way when set.
    SearchStep(bool),
    /// Swap the selection's ends.
    SwapEnds,
    /// Start, switch, or drop a selection.
    Visual(Visual),
    /// The `y` operator.
    Yank,
    /// Copy whole lines.
    YankLines,
    /// Zoom by this many steps.
    Zoom(isize),
}

/// A tab showing one PDF.
pub struct PdfTool {
    /// The toolbar as last painted, which a click on it is told apart by.
    bar: Bar,
    /// The cursor in the text, and what it has selected.
    caret: Caret,
    /// A cell's size in pixels, which a pointer's cell is turned back into
    /// pixels by.
    cell: (f32, f32),
    /// The document, once the thread has opened it.
    document: Option<Document>,
    /// Where a drag with the pointer started.
    drag_origin: Option<Pos>,
    /// Pages that could not be drawn, not asked for again.
    failed: HashSet<usize>,
    /// Numbers each layout, so a page drawn for one left behind is not taken
    /// for the current one.
    generation: u64,
    /// The pane's rows and columns as last painted.
    grid: (usize, usize),
    /// Whether the pages are drawn with their colors inverted, light text on
    /// dark.
    inverted: bool,
    notice: Option<Notice>,
    /// The outline, while it is open under the toolbar.
    outline: Option<OutlineList>,
    /// The rows the open outline took as last painted.
    outline_rows: usize,
    /// The pixels drawn over the pages.
    overlays: Overlays,
    /// Each page drawn and kept, at whatever scale it was drawn.
    pages: HashMap<usize, DrawnPage>,
    /// The size in pixels of the pane below the toolbar, as last drawn,
    /// which is what the pages are laid out in.
    pane: (f32, f32),
    path: PathBuf,
    /// The command being typed: a count, an operator, and any prefix waiting
    /// for the key that completes it.
    pending: Pending,
    /// Pages asked for at the current layout and not drawn yet.
    requested: HashSet<usize>,
    /// The page to turn to as soon as the document has opened: a tab
    /// coming back after a restart, where it was left.
    resume_page: Option<usize>,
    /// How far the view is scrolled, in pixels from the column's top-left.
    scroll: (f32, f32),
    /// What `/` or `?` last looked for, for the host to show.
    search: Option<Search>,
    /// Where the cursor was when the search prompt opened, and what had
    /// been searched for before: each letter typed searches from there
    /// again, and a prompt escaped out of puts both back.
    search_origin: Option<(Option<Pos>, Option<Search>)>,
    /// Whether a search ran since the host last asked.
    searched: bool,
    worker: Option<Worker>,
    zoom: Zoom,
}

/// A page as drawn: its pixels, and the scale they were drawn at.
struct DrawnPage {
    /// Whether it was drawn with its colors inverted.
    inverted: bool,
    key: u64,
    pixels: Arc<Pixels>,
    scale: f32,
}

/// The single-color pixels stretched over the pages.
struct Overlays {
    blank: Overlay,
    blank_inverted: Overlay,
    caret: Overlay,
    found: Overlay,
    selection: Overlay,
}

/// One color, as a pixel stretched to whatever size it is drawn.
struct Overlay {
    key: u64,
    pixels: Arc<Pixels>,
}

/// How the pages are scaled.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zoom {
    /// The tallest page fills the pane's height.
    FitHeight,
    /// The largest page fits the pane both ways.
    FitPage,
    /// The widest page fills the pane's width.
    FitWidth,
    /// This many pixels a point.
    Scale(f32),
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
    /// The first key of a sequence in [`COMMON_KEYS`] (`g` of `gg`, `z` of
    /// `zt`), waiting for the key that completes it.
    Keys(Key),
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
    /// Invert the pages' colors, or put them back.
    Invert,
    /// Open the outline, or close it.
    ToggleOutline,
    /// Show the pages at this zoom.
    SetZoom(Zoom),
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
// Overlay
// ========================================================================

impl Overlay {
    /// A pixel of `color`, under a key of its own.
    fn new(color: [u8; 4]) -> Self {
        Self {
            key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
            pixels: Arc::new(Pixels {
                height: 1,
                rgba: color.to_vec(),
                width: 1,
            }),
        }
    }

    /// The color drawn over `rect`, `(left, top, width, height)` in the pane.
    fn tile(&self, rect: (f32, f32, f32, f32)) -> Tile {
        Tile {
            content: Content::Pixels(Arc::clone(&self.pixels)),
            height: rect.3,
            key: self.key,
            width: rect.2,
            x: rect.0,
            y: rect.1,
        }
    }
}

// ========================================================================
// PdfTool
// ========================================================================

impl PdfTool {
    /// A tab showing `path`, opened on a thread of its own.
    pub fn new(path: PathBuf) -> Self {
        Self::opening(Some(Worker::open(path.clone())), path)
    }

    /// The tool showing `path`, turned to `page` as soon as the document has
    /// opened: a tab coming back after a restart, where it was left.
    pub fn at_page(path: PathBuf, page: Option<usize>) -> Self {
        Self {
            resume_page: page.filter(|page| *page > 1),
            ..Self::new(path)
        }
    }

    /// Whether `path` is a file this viewer should be the one to open.
    pub fn handles(path: &Path) -> bool {
        path.extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"))
    }

    fn opening(worker: Option<Worker>, path: PathBuf) -> Self {
        Self {
            bar: Bar::default(),
            caret: Caret::default(),
            cell: (0.0, 0.0),
            document: None,
            drag_origin: None,
            failed: HashSet::new(),
            generation: 0,
            grid: (0, 0),
            inverted: false,
            notice: None,
            outline: None,
            outline_rows: 0,
            overlays: Overlays {
                blank: Overlay::new(BLANK_COLOR),
                blank_inverted: Overlay::new(BLANK_INVERTED_COLOR),
                caret: Overlay::new(CARET_COLOR),
                found: Overlay::new(FOUND_COLOR),
                selection: Overlay::new(SELECTION_COLOR),
            },
            pages: HashMap::new(),
            pane: (0.0, 0.0),
            path,
            pending: Pending::default(),
            requested: HashSet::new(),
            resume_page: None,
            scroll: (0.0, 0.0),
            search: None,
            search_origin: None,
            searched: false,
            worker,
            zoom: Zoom::FitWidth,
        }
    }

    // --------------------------------------------------------------------
    // The grammar
    // --------------------------------------------------------------------

    /// Carry out a resolved command, clearing whatever was being typed: a
    /// count belongs to the motion it preceded and to nothing after it.
    fn act(&mut self, command: Command) -> ToolOutcome {
        self.pending = Pending::default();
        self.execute(command)
    }

    /// The count typed before the pending motion, or one when none was.
    fn count(&self) -> usize {
        self.pending.count.unwrap_or(1).min(MAX_COUNT)
    }

    /// The count as a signed repeat in `direction`.
    fn steps(&self, direction: isize) -> isize {
        (self.count() as isize).saturating_mul(direction)
    }

    /// Take `digit` as part of the count being typed, or say it is not one:
    /// a leading `0` is a motion in vim, not a count.
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

    /// How the document is being read: with a cursor in it or without.
    fn mode(&self) -> Mode {
        match self.caret.pos {
            Some(_) => Mode::Cursor,
            None => Mode::Normal,
        }
    }

    /// The mode the status bar shows: none while the view only scrolls.
    fn edit_mode(&self) -> Option<EditMode> {
        match (self.mode(), self.caret.visual) {
            (Mode::Normal, _) => None,
            (Mode::Cursor, Visual::None) => Some(EditMode::Normal),
            (Mode::Cursor, Visual::Char | Visual::Line) => Some(EditMode::Visual),
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
            Prefix::Find(kind) => find_key(kind).to_string(),
            Prefix::Keys(first) => format_key(&first),
            Prefix::None => String::new(),
        };
        format!("{count}{operator}{prefix}")
    }

    /// Act on `key` after the first key of a sequence: the sequence it
    /// completes, or the prefix abandoned, as vim does.
    fn on_sequence_key(&mut self, first: Key, key: &Key) -> ToolOutcome {
        match table::lookup(COMMON_KEYS, &[first, *key]) {
            TableMatch::Exact(act) => self.on_act(act),
            TableMatch::Miss | TableMatch::Prefix => {
                self.pending = Pending::default();
                ToolOutcome::Consumed
            }
        }
    }

    /// Act on the character typed after `f`, `F`, `t`, or `T`.
    fn on_find_key(&mut self, kind: FindKind, key: &Key) -> ToolOutcome {
        let pending = self.pending;
        self.pending = Pending::default();
        let KeyCode::Char(ch) = key.glyph() else {
            // Anything but a character abandons the find, as vim does.
            return ToolOutcome::Consumed;
        };
        self.act(Command::CaretFind(CaretFind {
            ch,
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            kind,
            operator: pending.operator,
        }))
    }

    /// Run `motion` with whatever count and operator were typed before it.
    fn caret_motion(&mut self, motion: Motion) -> ToolOutcome {
        let pending = self.pending;
        self.act(Command::CaretMotion(CaretMotion {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            motion,
            operator: pending.operator,
        }))
    }

    /// `/` and `?`: ask the host for what to look for, keeping the count and
    /// operator typed until the answer comes back.
    fn ask_search(&mut self, forward: bool) -> ToolOutcome {
        self.search_origin = Some((self.caret.pos, self.search.clone()));
        ToolOutcome::Prompt(PromptRequest {
            initial: String::new(),
            label: match forward {
                true => "/".to_string(),
                false => "?".to_string(),
            },
            mode: PromptMode::Search { reverse: !forward },
            tag: match forward {
                true => ASK_SEARCH,
                false => ASK_SEARCH_BACK,
            },
        })
    }

    /// Put the cursor back where it was when the search prompt opened.
    fn back_to_search_origin(&mut self) {
        if let Some((pos, _)) = &self.search_origin {
            self.caret.pos = *pos;
        }
        self.caret.found = None;
    }

    /// Look for what the prompt came back with, from where it was opened.
    fn start_search(&mut self, pattern: String, forward: bool) -> ToolOutcome {
        self.back_to_search_origin();
        if pattern.is_empty() {
            self.pending = Pending::default();
            return ToolOutcome::Consumed;
        }
        let pending = self.pending;
        self.search = Some(Search::new(pattern.clone(), forward));
        self.searched = true;
        self.act(Command::CaretSearch(CaretSearch {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            forward,
            operator: pending.operator,
            pattern,
        }))
    }

    /// `n` and `N`, which need a search to repeat and say so when there has
    /// not been one.
    fn search_step(&mut self, reverse: bool) -> ToolOutcome {
        if self.search.is_none() {
            self.pending = Pending::default();
            self.notice = Some(Notice::error("nothing has been searched for"));
            return ToolOutcome::Consumed;
        }
        self.searched = true;
        let pending = self.pending;
        self.act(Command::CaretSearchStep(CaretSearchStep {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            operator: pending.operator,
            reverse,
        }))
    }

    /// `;` and `,`, which repeat the last find.
    fn repeat_find(&mut self, reverse: bool) -> ToolOutcome {
        let pending = self.pending;
        self.act(Command::CaretRepeatFind(CaretRepeat {
            count: pending.count.unwrap_or(1).min(MAX_COUNT),
            operator: pending.operator,
            reverse,
        }))
    }

    /// `y`: the selection when there is one, `yy` when it follows itself,
    /// and otherwise the operator waiting for a motion.
    fn on_yank_key(&mut self) -> ToolOutcome {
        if self.caret.visual != Visual::None {
            return self.act(Command::CaretYankSelection);
        }
        if self.pending.operator == Operator::Yank {
            let count = self.count();
            return self.act(Command::CaretYankLines(count));
        }
        self.pending.operator = Operator::Yank;
        ToolOutcome::Consumed
    }

    /// Escape with a cursor in the text: give up what is half-typed first,
    /// then the selection, then the cursor itself.
    fn on_cursor_escape(&mut self) -> ToolOutcome {
        if self.pending != Pending::default() {
            self.pending = Pending::default();
            return ToolOutcome::Consumed;
        }
        if self.caret.visual != Visual::None {
            return self.act(Command::CaretVisual(Visual::None));
        }
        self.act(Command::CaretLeave)
    }

    /// Start a selection, swap which kind it is, or drop it: the same key
    /// again ends the selection it started.
    fn set_visual(&mut self, kind: Visual) -> ToolOutcome {
        let visual = match self.caret.visual == kind {
            true => Visual::None,
            false => kind,
        };
        self.act(Command::CaretVisual(visual))
    }

    /// Do what a key of the viewer's own asks, with the count and the mode
    /// saying how.
    fn on_act(&mut self, act: Act) -> ToolOutcome {
        let count = self.pending.count;
        match act {
            Act::Abandon => {
                self.pending = Pending::default();
                ToolOutcome::Consumed
            }
            Act::AskPage => self.ask_page(),
            Act::Caret(motion) => self.caret_motion(motion),
            Act::CaretEnd => match count {
                Some(_) => self.caret_motion(Motion::Page),
                None => self.caret_motion(Motion::DocEnd),
            },
            Act::Columns(direction) => {
                let steps = self.steps(direction);
                self.act(Command::ScrollColumns(steps))
            }
            Act::Cursor => self.act(Command::CaretEnter),
            Act::Find(kind) => {
                self.pending.prefix = Prefix::Find(kind);
                ToolOutcome::Consumed
            }
            // `gg` is the first page, or the page a count names: the one
            // place vim reads a count as an absolute. With a cursor, it is
            // where the text starts.
            Act::First => match (self.mode(), count) {
                (Mode::Cursor, Some(_)) => self.caret_motion(Motion::Page),
                (Mode::Cursor, None) => self.caret_motion(Motion::DocStart),
                (Mode::Normal, _) => self.act(Command::GoToPage(count.unwrap_or(1))),
            },
            Act::FitWidth => self.act(Command::FitWidth),
            Act::Home => self.act(Command::GoToPage(1)),
            Act::Invert => self.act(Command::Invert),
            Act::Last => match count {
                Some(page) => self.act(Command::GoToPage(page)),
                None => self.act(Command::GoToLastPage),
            },
            Act::Leave => self.on_cursor_escape(),
            Act::Lines(direction) => {
                let lines = self.steps(direction);
                self.act(Command::ScrollLines(lines))
            }
            Act::Outline => self.act(Command::ToggleOutline),
            Act::Pages(direction) => {
                let pages = self.steps(direction);
                self.act(Command::GoToPageBy(pages))
            }
            Act::Place(place) => match self.mode() {
                Mode::Cursor => self.act(Command::CaretPlaceLine(place)),
                Mode::Normal => self.act(Command::Place(place)),
            },
            Act::Position => self.act(Command::ReportPosition),
            Act::RepeatFind(reverse) => self.repeat_find(reverse),
            Act::Screens(fraction) => self.act(Command::ScrollScreens(fraction)),
            Act::Search(forward) => self.ask_search(forward),
            Act::SearchStep(reverse) => self.search_step(reverse),
            Act::SwapEnds => self.act(Command::CaretSwapEnds),
            Act::Visual(kind) => self.set_visual(kind),
            Act::Yank => self.on_yank_key(),
            Act::YankLines => {
                let count = self.count();
                self.act(Command::CaretYankLines(count))
            }
            Act::Zoom(steps) => self.act(Command::Zoom(steps)),
        }
    }

    /// The tables a key is looked up in, in order, for the mode the
    /// document is being read in.
    fn mode_tables(&self) -> [&'static [Binding<Act>]; 2] {
        match self.mode() {
            Mode::Cursor => [CURSOR_KEYS, WINDOW_KEYS],
            Mode::Normal => [NORMAL_KEYS, WINDOW_KEYS],
        }
    }

    // --------------------------------------------------------------------
    // Carrying commands out
    // --------------------------------------------------------------------

    /// Carry out `command` on the document and the view.
    fn execute(&mut self, command: Command) -> ToolOutcome {
        match command {
            Command::CaretEnter => {
                let at = self.screen_line(0.0);
                self.with_caret(|caret, doc| caret.enter(doc, at))
            }
            Command::CaretFind(find) => self.with_caret(|caret, doc| {
                caret.find(doc, find.kind, find.ch, find.count, find.operator)
            }),
            Command::CaretLeave => {
                self.caret.leave();
                ToolOutcome::Consumed
            }
            Command::CaretMotion(motion) => {
                let screen = match motion.motion {
                    Motion::ViewTop => self.screen_line(0.0),
                    Motion::ViewMiddle => self.screen_line(self.pane.1 / 2.0),
                    Motion::ViewBottom => self.screen_line(self.pane.1),
                    _ => None,
                };
                self.with_caret(|caret, doc| {
                    caret.motion(doc, motion.motion, motion.count, motion.operator, screen)
                })
            }
            Command::CaretPlaceLine(place) => {
                if let Some(rect) = self.caret_line_rect() {
                    let offset = place_offset(place, rect.3, self.pane.1, 0.0);
                    self.scroll_to((self.scroll.0, self.scroll.1 + rect.1 - offset));
                }
                ToolOutcome::Consumed
            }
            Command::CaretRepeatFind(repeat) => self.with_caret(|caret, doc| {
                caret.repeat_find(doc, repeat.reverse, repeat.count, repeat.operator)
            }),
            Command::CaretSearch(search) => {
                let from = self.screen_line(0.0);
                self.with_caret(|caret, doc| {
                    caret.search(
                        doc,
                        &search.pattern,
                        search.forward,
                        search.count,
                        search.operator,
                        from,
                    )
                })
            }
            Command::CaretSearchStep(step) => {
                let from = self.screen_line(0.0);
                self.with_caret(|caret, doc| {
                    caret.search_again(doc, step.reverse, step.count, step.operator, from)
                })
            }
            Command::CaretSwapEnds => self.with_caret(|caret, _| caret.swap_ends()),
            Command::CaretVisual(visual) => {
                self.caret.set_visual(visual);
                ToolOutcome::Consumed
            }
            Command::CaretYankLines(count) => {
                self.with_caret(|caret, doc| caret.yank_lines(doc, count))
            }
            Command::CaretYankSelection => self.with_caret(|caret, doc| caret.yank_selection(doc)),
            Command::FitWidth => {
                self.set_zoom(Zoom::FitWidth);
                ToolOutcome::Consumed
            }
            Command::Invert => {
                // Drawn again, inverted or not: what is shown meanwhile is
                // the drawing it replaces, as at a zoom.
                self.inverted = !self.inverted;
                self.generation += 1;
                self.requested.clear();
                self.request_visible();
                ToolOutcome::Consumed
            }
            Command::SetZoom(zoom) => {
                self.set_zoom(zoom);
                ToolOutcome::Consumed
            }
            Command::ToggleOutline => {
                self.outline = match self.outline.take() {
                    Some(_) => None,
                    None => self.document.as_ref().and_then(|doc| {
                        let entries = doc.outline();
                        (!entries.is_empty())
                            .then(|| OutlineList::open(entries, self.current_section()))
                    }),
                };
                ToolOutcome::Consumed
            }
            Command::GoToLastPage => {
                self.go_to_page(self.page_count().saturating_sub(1));
                ToolOutcome::Consumed
            }
            Command::GoToPage(number) => {
                self.go_to_page(number.saturating_sub(1));
                ToolOutcome::Consumed
            }
            Command::GoToPageBy(delta) => {
                let page = self.current_page().saturating_add_signed(delta);
                self.go_to_page(page);
                ToolOutcome::Consumed
            }
            Command::Place(place) => {
                if let Some(column) = self.column() {
                    let rect = column.pages[self.current_page()];
                    let offset = place_offset(place, rect.height, self.pane.1, layout::GAP);
                    self.scroll_to((self.scroll.0, rect.y - offset));
                }
                ToolOutcome::Consumed
            }
            Command::ReportPosition => {
                self.notice = Some(Notice::info(self.position()));
                ToolOutcome::Consumed
            }
            Command::ScrollColumns(steps) => {
                let x = self.scroll.0 + steps as f32 * COLUMN_PX;
                self.scroll_to((x, self.scroll.1));
                ToolOutcome::Consumed
            }
            Command::ScrollLines(lines) => {
                let y = self.scroll.1 + lines as f32 * LINE_PX;
                self.scroll_to((self.scroll.0, y));
                ToolOutcome::Consumed
            }
            Command::ScrollScreens(fraction) => {
                let y = self.scroll.1 + fraction * self.pane.1;
                self.scroll_to((self.scroll.0, y));
                ToolOutcome::Consumed
            }
            Command::Zoom(steps) => {
                let steps = i32::try_from(steps).unwrap_or(i32::MAX);
                let scale = self.scale() * ZOOM_STEP.powi(steps);
                self.set_zoom(Zoom::Scale(scale.clamp(MIN_SCALE, MAX_SCALE)));
                ToolOutcome::Consumed
            }
        }
    }

    /// Run `act` on the cursor and the document, once there is one, and
    /// follow up what it did.
    fn with_caret(&mut self, act: impl FnOnce(&mut Caret, &mut Document) -> Effect) -> ToolOutcome {
        let Some(doc) = self.document.as_mut() else {
            return ToolOutcome::Consumed;
        };
        match act(&mut self.caret, doc) {
            Effect::Moved => {
                self.scroll_caret_into_view();
                ToolOutcome::Consumed
            }
            Effect::Nothing => ToolOutcome::Consumed,
            Effect::Refused(why) => {
                self.notice = Some(Notice::error(why));
                ToolOutcome::Consumed
            }
            Effect::Yanked(text) => copy(text),
        }
    }

    // --------------------------------------------------------------------
    // Layout
    // --------------------------------------------------------------------

    /// How many pages the document has, once it has opened.
    fn page_count(&self) -> usize {
        self.document.as_ref().map_or(0, Document::page_count)
    }

    /// Pixels a point the pages are shown at.
    fn scale(&self) -> f32 {
        match (self.zoom, &self.document) {
            (Zoom::Scale(scale), _) => scale,
            (Zoom::FitWidth, Some(doc)) => layout::fit_width(doc.sizes(), self.pane.0),
            (Zoom::FitHeight, Some(doc)) => layout::fit_height(doc.sizes(), self.pane.1),
            (Zoom::FitPage, Some(doc)) => layout::fit_width(doc.sizes(), self.pane.0)
                .min(layout::fit_height(doc.sizes(), self.pane.1)),
            (Zoom::FitWidth | Zoom::FitHeight | Zoom::FitPage, None) => 1.0,
        }
    }

    /// The pages laid out at the scale they are shown at.
    fn column(&self) -> Option<Column> {
        let doc = self.document.as_ref()?;
        Some(Column::new(doc.sizes(), self.scale(), self.pane.0))
    }

    /// The page being read: the one under the reading line.
    fn current_page(&self) -> usize {
        self.column().map_or(0, |column| {
            column.page_at(self.scroll.1 + self.pane.1 * READING_LINE)
        })
    }

    /// Scroll to `scroll`, kept where no gap opens past the column's edges.
    fn scroll_to(&mut self, scroll: (f32, f32)) {
        let Some(column) = self.column() else {
            return;
        };
        let (max_x, max_y) = column.max_scroll(self.pane);
        self.scroll = (scroll.0.clamp(0.0, max_x), scroll.1.clamp(0.0, max_y));
        self.request_visible();
    }

    /// Scroll so page `page`, counted from zero, starts at the top.
    fn go_to_page(&mut self, page: usize) {
        let Some(column) = self.column() else {
            return;
        };
        let Some(rect) = column
            .pages
            .get(page.min(column.pages.len().saturating_sub(1)))
        else {
            return;
        };
        self.scroll_to((self.scroll.0, rect.y - layout::GAP));
    }

    /// Show the pages at `zoom`, keeping the point at the reading line on the
    /// same spot of the same page.
    fn set_zoom(&mut self, zoom: Zoom) {
        let Some(before) = self.column() else {
            self.zoom = zoom;
            return;
        };
        let line = self.pane.1 * READING_LINE;
        let page = before.page_at(self.scroll.1 + line);
        let rect = before.pages[page];
        let fraction = (self.scroll.1 + line - rect.y) / rect.height;
        let across = (self.scroll.0 + self.pane.0 / 2.0) / before.width;
        self.zoom = zoom;
        self.generation += 1;
        self.requested.clear();
        let Some(after) = self.column() else {
            return;
        };
        let rect = after.pages[page];
        self.scroll_to((
            across * after.width - self.pane.0 / 2.0,
            rect.y + fraction * rect.height - line,
        ));
    }

    /// Where `bounds`, `(left, top, width, height)` in page `page`'s points,
    /// fall in the pane.
    fn on_screen(
        &self,
        column: &Column,
        page: usize,
        bounds: (f32, f32, f32, f32),
    ) -> (f32, f32, f32, f32) {
        let rect = column.pages[page];
        let scale = self.scale();
        (
            rect.x - self.scroll.0 + bounds.0 * scale,
            rect.y - self.scroll.1 + bounds.1 * scale,
            bounds.2 * scale,
            bounds.3 * scale,
        )
    }

    /// Where the cursor's line is in the pane.
    fn caret_line_rect(&mut self) -> Option<(f32, f32, f32, f32)> {
        let pos = self.caret.pos?;
        let column = self.column()?;
        let bounds = self
            .document
            .as_mut()?
            .text(pos.page)?
            .lines
            .get(pos.line)?
            .bounds()?;
        Some(self.on_screen(&column, pos.page, bounds))
    }

    /// Scroll as little as brings the cursor's character into view: not at
    /// all while it already is.
    fn scroll_caret_into_view(&mut self) {
        let (Some(pos), Some(column)) = (self.caret.pos, self.column()) else {
            return;
        };
        let Some(bounds) = self.document.as_mut().and_then(|doc| {
            let line = doc.text(pos.page)?.lines.get(pos.line)?;
            let index = pos.char.min(line.boxes.len().checked_sub(1)?);
            Some(line.boxes[index].bounds())
        }) else {
            return;
        };
        let (x, y, width, height) = self.on_screen(&column, pos.page, bounds);
        let (mut left, mut top) = self.scroll;
        if y < 0.0 {
            top += y;
        } else if y + height > self.pane.1 {
            top += y + height - self.pane.1;
        }
        if x < 0.0 {
            left += x;
        } else if x + width > self.pane.0 {
            left += x + width - self.pane.0;
        }
        self.scroll_to((left, top));
    }

    /// The start of the line in view nearest `want` pixels down the pane:
    /// where `H`, `M`, `L`, and a cursor put in by `i` land.
    fn screen_line(&mut self, want: f32) -> Option<Pos> {
        let column = self.column()?;
        let scale = self.scale();
        let (_, top) = self.scroll;
        let shown = column.visible(top, self.pane.1);
        let doc = self.document.as_mut()?;
        let mut best: Option<(f32, Pos)> = None;
        for page in shown {
            let page_top = column.pages[page].y - top;
            let Some(text) = doc.text(page) else {
                continue;
            };
            for (index, line) in text.lines.iter().enumerate() {
                let Some((_, line_top, _, height)) = line.bounds() else {
                    continue;
                };
                let middle = page_top + (line_top + height / 2.0) * scale;
                let distance = (middle - want).abs();
                if best.is_none_or(|(nearest, _)| distance < nearest) {
                    best = Some((distance, Pos::new(page, index, 0)));
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// The character nearest `(x, y)` in the pane: on the line the point is
    /// least far outside of, across and down at once, so a point in a
    /// column's margin lands in that column.
    fn char_at(&mut self, x: f32, y: f32) -> Option<Pos> {
        let column = self.column()?;
        let scale = self.scale();
        let page = column.page_at(self.scroll.1 + y);
        let rect = column.pages[page];
        let at = (
            (x + self.scroll.0 - rect.x) / scale,
            (y + self.scroll.1 - rect.y) / scale,
        );
        let text = self.document.as_mut()?.text(page)?;
        let mut best: Option<(f32, usize)> = None;
        for (index, line) in text.lines.iter().enumerate() {
            let Some((left, top, width, height)) = line.bounds() else {
                continue;
            };
            let distance = outside(at.0, left, left + width) + outside(at.1, top, top + height);
            if best.is_none_or(|(nearest, _)| distance < nearest) {
                best = Some((distance, index));
            }
        }
        let (_, line) = best?;
        let boxes = &text.lines[line].boxes;
        let char = boxes
            .iter()
            .position(|cell| at.0 < cell.x + cell.width)
            .unwrap_or(boxes.len().saturating_sub(1));
        Some(Pos::new(page, line, char))
    }

    // --------------------------------------------------------------------
    // Drawing
    // --------------------------------------------------------------------

    /// The scale a page of `size` points is drawn at: the one it is shown
    /// at, short of [`MAX_DRAWN_SIDE`].
    fn drawn_scale(&self, size: (f32, f32)) -> f32 {
        self.scale().min(MAX_DRAWN_SIDE / size.0.max(size.1))
    }

    /// Ask for every page in view, and those just past it, that is not drawn
    /// at the scale it is shown at, and let go of those far from it.
    fn request_visible(&mut self) {
        let (Some(column), Some(doc), Some(worker)) =
            (self.column(), self.document.as_ref(), self.worker.as_ref())
        else {
            return;
        };
        if self.pane.1 <= 0.0 {
            return;
        }
        let shown = column.visible(self.scroll.1, self.pane.1);
        let wanted =
            shown.start.saturating_sub(LOOKAHEAD)..(shown.end + LOOKAHEAD).min(doc.page_count());
        for page in wanted.clone() {
            let scale = self.drawn_scale(doc.sizes()[page]);
            let drawn = self
                .pages
                .get(&page)
                .is_some_and(|drawn| drawn.scale == scale && drawn.inverted == self.inverted);
            if drawn || self.requested.contains(&page) || self.failed.contains(&page) {
                continue;
            }
            worker.request(Request {
                generation: self.generation,
                invert: self.inverted,
                page,
                scale,
            });
            self.requested.insert(page);
        }
        self.pages.retain(|page, _| wanted.contains(page));
    }

    /// Take in what the drawing thread has said.
    fn take_replies(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        for reply in worker.poll() {
            match reply {
                Reply::Opened(Ok(opened)) => {
                    self.take_document(Document::new(opened.pdf, opened.sizes, opened.outline));
                }
                Reply::Opened(Err(why)) => {
                    self.notice = Some(Notice::error(format!("{}: {why}", self.file_name())));
                    self.worker = None;
                    return;
                }
                Reply::Page(drawn) => {
                    let page = drawn.request.page;
                    let current = drawn.request.generation == self.generation;
                    if current {
                        self.requested.remove(&page);
                    }
                    // A page drawn for a layout since left still beats a
                    // blank one until the current drawing arrives.
                    if current || !self.pages.contains_key(&page) {
                        self.pages.insert(
                            page,
                            DrawnPage {
                                inverted: drawn.request.invert,
                                key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
                                pixels: drawn.pixels,
                                scale: drawn.request.scale,
                            },
                        );
                    }
                }
                Reply::Text(page, text) => {
                    if let Some(doc) = self.document.as_mut() {
                        doc.insert_text(page, text);
                    }
                }
                Reply::Failed(page) => {
                    self.requested.remove(&page);
                    self.failed.insert(page);
                    self.notice = Some(Notice::error(format!(
                        "{}: page {} could not be drawn",
                        self.file_name(),
                        page + 1
                    )));
                }
            }
        }
    }

    /// Show `document`, at the page a restored tab was left on.
    fn take_document(&mut self, document: Document) {
        self.document = Some(document);
        if let Some(page) = self.resume_page.take() {
            self.go_to_page(page - 1);
        }
        self.request_visible();
    }

    /// Read the text of the pages in view while there is a cursor or a
    /// match to draw over them, which is drawn from the text.
    fn read_visible_text(&mut self) {
        if self.caret.pos.is_none() {
            return;
        }
        let Some(column) = self.column() else {
            return;
        };
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        for page in column.visible(self.scroll.1, self.pane.1) {
            doc.text(page);
        }
    }

    /// What is drawn over the pages in view: what a search found, what is
    /// selected, and the cursor's bar.
    fn overlay_tiles(&self, column: &Column) -> Vec<Tile> {
        let Some(doc) = self.document.as_ref() else {
            return Vec::new();
        };
        let shown = column.visible(self.scroll.1, self.pane.1);
        let line_span = |page: usize, line: usize, from: usize, to: usize| {
            let line = doc.read(page)?.lines.get(line)?;
            let last = line.boxes.len().checked_sub(1)?;
            let first = line.boxes[from.min(last)].bounds();
            let end = line.boxes[to.min(last)].bounds();
            Some(self.on_screen(column, page, union(first, end)))
        };
        let mut tiles = Vec::new();
        if let Some(found) = self
            .caret
            .found
            .filter(|found| shown.contains(&found.at.page))
        {
            let at = found.at;
            let end = at.char + found.length.saturating_sub(1);
            if let Some(rect) = line_span(at.page, at.line, at.char, end) {
                tiles.push(self.overlays.found.tile(rect));
            }
        }
        if let Some((start, end, linewise)) = self.caret.selection() {
            for page in start.page.max(shown.start)..=end.page.min(shown.end.saturating_sub(1)) {
                let Some(text) = doc.read(page) else {
                    continue;
                };
                let first = if page == start.page { start.line } else { 0 };
                let last = match page == end.page {
                    true => end.line,
                    false => text.lines.len().saturating_sub(1),
                };
                for line in first..=last.min(text.lines.len().saturating_sub(1)) {
                    let opens = page == start.page && line == start.line;
                    let closes = page == end.page && line == end.line;
                    let from = if linewise || !opens { 0 } else { start.char };
                    let to = if linewise || !closes {
                        usize::MAX
                    } else {
                        end.char
                    };
                    if let Some(rect) = line_span(page, line, from, to) {
                        tiles.push(self.overlays.selection.tile(rect));
                    }
                }
            }
        }
        if let Some(pos) = self.caret.pos.filter(|pos| shown.contains(&pos.page)) {
            if let Some((x, y, _, height)) = line_span(pos.page, pos.line, pos.char, pos.char) {
                // On whole pixels: a two-pixel bar at a fractional offset is
                // drawn as a soft three-pixel smear.
                let rect = (
                    x.round(),
                    y.round(),
                    CARET_WIDTH,
                    height.max(MIN_CARET_HEIGHT).round(),
                );
                tiles.push(self.overlays.caret.tile(rect));
            }
        }
        tiles
    }

    // --------------------------------------------------------------------
    // Saying where it is
    // --------------------------------------------------------------------

    /// The name of the file, for the title.
    fn file_name(&self) -> String {
        match self.path.file_name() {
            Some(name) => name.to_string_lossy().to_string(),
            None => self.path.display().to_string(),
        }
    }

    /// `page 3 / 18`, once the document has opened: blank before, since a
    /// confident "page 0 of 0" would be a lie about one still loading.
    fn page_label(&self) -> String {
        match self.page_count() {
            0 => String::new(),
            pages => format!("page {} / {pages}", self.current_page() + 1),
        }
    }

    /// How far through the document the view has scrolled.
    fn percent(&self) -> u32 {
        let Some(column) = self.column() else {
            return 0;
        };
        let (_, max_y) = column.max_scroll(self.pane);
        match max_y > 0.0 {
            true => (self.scroll.1 / max_y * 100.0).round() as u32,
            false => 100,
        }
    }

    /// The file, the page, and how far through, as `C-g` says them.
    fn position(&self) -> String {
        format!(
            "{}, page {} of {}, {}%",
            self.file_name(),
            self.current_page() + 1,
            self.page_count(),
            self.percent()
        )
    }

    // --------------------------------------------------------------------
    // The toolbar and the outline
    // --------------------------------------------------------------------

    /// How tall the toolbar is in pixels.
    fn toolbar_height(&self) -> f32 {
        TOOLBAR_ROWS as f32 * self.cell.1
    }

    /// What the toolbar shows.
    fn bar_state(&self) -> toolbar::State<'_> {
        let entries = self.document.as_ref().map_or(&[][..], Document::outline);
        toolbar::State {
            has_outline: !entries.is_empty(),
            inverted: self.inverted,
            outline_open: self.outline.is_some(),
            page: (self.page_count() > 0).then(|| (self.current_page() + 1, self.page_count())),
            section: self
                .current_section()
                .map(|index| entries[index].title.as_str()),
            zoom: (self.scale() * PERCENT).round() as u32,
        }
    }

    /// The spot at the reading line: its page, and how far down it.
    fn reading_spot(&self) -> Option<Target> {
        let column = self.column()?;
        let line = self.scroll.1 + self.pane.1 * READING_LINE;
        let page = column.page_at(line);
        Some(Target {
            page,
            y: (line - column.pages[page].y) / self.scale(),
        })
    }

    /// The outline entry the reading spot is in.
    fn current_section(&self) -> Option<usize> {
        let entries = self.document.as_ref()?.outline();
        outline_list::current(entries, self.reading_spot()?)
    }

    /// Scroll so outline entry `index`'s spot is at the top, and close the
    /// outline. An entry pointing nowhere leaves it open.
    fn jump_to_section(&mut self, index: usize) -> ToolOutcome {
        let target = self
            .document
            .as_ref()
            .and_then(|doc| doc.outline().get(index))
            .and_then(|entry| entry.target);
        let (Some(target), Some(column)) = (target, self.column()) else {
            return ToolOutcome::Consumed;
        };
        if let Some(rect) = column.pages.get(target.page) {
            let y = rect.y + target.y * self.scale() - layout::GAP;
            self.scroll_to((self.scroll.0, y));
        }
        self.outline = None;
        ToolOutcome::Consumed
    }

    /// Ask which page to go to, the one being read filled in.
    fn ask_page(&mut self) -> ToolOutcome {
        self.pending = Pending::default();
        if self.page_count() == 0 {
            return ToolOutcome::Consumed;
        }
        ToolOutcome::Prompt(PromptRequest {
            initial: (self.current_page() + 1).to_string(),
            label: format!("Page (1-{}): ", self.page_count()),
            mode: PromptMode::Text,
            tag: ASK_PAGE,
        })
    }

    /// Ask what to zoom to, the zoom in force filled in.
    fn ask_zoom(&mut self) -> ToolOutcome {
        self.pending = Pending::default();
        ToolOutcome::Prompt(PromptRequest {
            initial: format!("{}", (self.scale() * PERCENT).round()),
            label: "Zoom (%, or w width, h height, p page): ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_ZOOM,
        })
    }

    /// Do what toolbar button `button` does.
    fn press(&mut self, button: Button) -> ToolOutcome {
        match button {
            Button::Invert => self.act(Command::Invert),
            Button::NextPage => self.act(Command::GoToPageBy(1)),
            Button::Outline => self.act(Command::ToggleOutline),
            Button::Page => self.ask_page(),
            Button::PreviousPage => self.act(Command::GoToPageBy(-1)),
            Button::Zoom => self.ask_zoom(),
            Button::ZoomIn => self.act(Command::Zoom(1)),
            Button::ZoomOut => self.act(Command::Zoom(-1)),
        }
    }

    /// A key while the outline is open: it moves the outline's cursor, and
    /// the keys it has no use for close it.
    fn on_outline_key(&mut self, key: &Key) -> ToolOutcome {
        let Some(entries) = self.document.as_ref().map(|doc| doc.outline().to_vec()) else {
            return ToolOutcome::Ignored;
        };
        let Some(list) = self.outline.as_mut() else {
            return ToolOutcome::Ignored;
        };
        match (key.code, key.ctrl) {
            (KeyCode::Down, _) | (KeyCode::Char('j'), false) | (KeyCode::Char('n'), true) => {
                list.step(&entries, 1);
            }
            (KeyCode::Up, _) | (KeyCode::Char('k'), false) | (KeyCode::Char('p'), true) => {
                list.step(&entries, -1);
            }
            (KeyCode::Right, _) | (KeyCode::Char('l'), false) => list.unfold(&entries),
            (KeyCode::Left, _) | (KeyCode::Char('h'), false) => list.fold(&entries),
            (KeyCode::Tab, _) => list.toggle(list.cursor),
            (KeyCode::Enter, _) => {
                let index = list.cursor;
                return self.jump_to_section(index);
            }
            (KeyCode::Escape, _) | (KeyCode::Char('q' | 'O'), false) | (KeyCode::Char('g'), true) => {
                self.outline = None;
            }
            _ => {}
        }
        ToolOutcome::Consumed
    }

    /// A click on the toolbar or the open outline under it, at `at`:
    /// `None` for one on the pages.
    fn on_chrome_mouse(&mut self, at: Point) -> Option<ToolOutcome> {
        if at.row < TOOLBAR_ROWS {
            // A drag across the bar, or one carried onto it from the pages,
            // presses nothing.
            if at.drag {
                return Some(ToolOutcome::Consumed);
            }
            return Some(match self.bar.button_at(at.col) {
                Some(button) => self.press(button),
                None => ToolOutcome::Consumed,
            });
        }
        self.outline.as_ref()?;
        let row = at.row - TOOLBAR_ROWS;
        if row >= self.outline_rows {
            // A click on the pages closes the outline and is spent on that.
            if !at.drag {
                self.outline = None;
            }
            return Some(ToolOutcome::Consumed);
        }
        if at.drag {
            return Some(ToolOutcome::Consumed);
        }
        let entries = self.document.as_ref()?.outline();
        let hit = self.outline.as_ref()?.hit(entries, row, at.col);
        Some(match hit {
            Some(RowHit::Fold(index)) => {
                if let Some(list) = self.outline.as_mut() {
                    list.toggle(index);
                }
                ToolOutcome::Consumed
            }
            Some(RowHit::Entry(index)) => self.jump_to_section(index),
            None => ToolOutcome::Consumed,
        })
    }

    /// The open outline's rows, as many as fit in `rows` of the pane below
    /// the toolbar, and which of them is the cursor's.
    fn outline_rows(&mut self, rows: usize, cols: usize) -> Option<(Vec<Row>, usize)> {
        let entries = self.document.as_ref()?.outline().to_vec();
        let list = self.outline.as_mut()?;
        let room = ((rows as f32 * OUTLINE_SHARE) as usize).max(1);
        let height = list.shown(&entries).len().min(room);
        Some(list.rows(&entries, height, cols))
    }

    /// What a zoom typed at the prompt asks for: a fit by its word or first
    /// letter, or a percentage, `%` or not. `None` for anything else.
    fn parse_zoom(answer: &str) -> Option<Zoom> {
        let answer = answer.trim().to_lowercase();
        let is = |words: &[&str]| words.contains(&answer.as_str());
        if is(FIT_WIDTH_WORDS) {
            return Some(Zoom::FitWidth);
        }
        if is(FIT_HEIGHT_WORDS) {
            return Some(Zoom::FitHeight);
        }
        if is(FIT_PAGE_WORDS) {
            return Some(Zoom::FitPage);
        }
        let percent: f32 = answer.trim_end_matches('%').trim().parse().ok()?;
        (percent.is_finite() && percent > 0.0)
            .then(|| Zoom::Scale((percent / PERCENT).clamp(MIN_SCALE, MAX_SCALE)))
    }
}

impl Tool for PdfTool {
    fn context_items(&self) -> Vec<ToolMenuItem> {
        let has_outline = self
            .document
            .as_ref()
            .is_some_and(|doc| !doc.outline().is_empty());
        let entry = |act: Act, label: &str| {
            [COMMON_KEYS, CURSOR_KEYS, WINDOW_KEYS]
                .into_iter()
                .find_map(|keys| table::menu_entry(keys, act))
                .map(|(key, _)| ToolMenuItem::new(key, label))
        };
        let copy = match self.caret.selection() {
            Some(_) => entry(Act::Yank, MENU_COPY).into_iter().collect(),
            None => Vec::new(),
        };
        let rest = MENU.iter().map(|group| {
            group
                .iter()
                .filter(|(act, _)| *act != Act::Outline || has_outline)
                .filter_map(|&(act, label)| entry(act, label))
                .collect()
        });
        menu_groups(std::iter::once(copy).chain(rest))
    }

    /// A right-click moves nothing and presses no toolbar button: the
    /// selection stays for the menu's Copy, and the menu's entries act on
    /// the document rather than on a spot in it.
    fn on_context_mouse(&mut self, _at: Point) -> ToolOutcome {
        ToolOutcome::Consumed
    }

    fn title(&self) -> String {
        self.file_name()
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::File {
            name: self.file_name(),
        })
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        // The toolbar, and the outline under it while it is open; the pages
        // the host draws cover every row below.
        self.grid = (rows, cols);
        self.bar = Bar::new(&self.bar_state(), cols);
        let mut painted = vec![self.bar.row.clone()];
        let below = rows.saturating_sub(TOOLBAR_ROWS);
        let outline = self.outline_rows(below, cols);
        self.outline_rows = outline.as_ref().map_or(0, |(list, _)| list.len());
        match outline {
            Some((list, cursor)) => {
                painted.extend(list);
                Frame::new(painted).with_cursor_line(TOOLBAR_ROWS + cursor)
            }
            None => Frame::new(painted),
        }
    }

    fn header_rows(&self) -> usize {
        TOOLBAR_ROWS + self.outline_rows
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    /// What was looked for, how many times the document holds it, and
    /// which of those the cursor is on.
    fn take_search(&mut self) -> Option<SearchReport> {
        if !std::mem::take(&mut self.searched) {
            return None;
        }
        let search = self.search.as_ref()?;
        let at = self.caret.found.map(|found| found.at);
        let (index, total) = match self.document.as_mut() {
            Some(doc) => caret::count_matches(doc, &search.pattern, at),
            None => (0, 0),
        };
        Some(SearchReport {
            index,
            query: search.pattern.clone(),
            reverse: !search.forward,
            total,
        })
    }

    /// The page, the cursor's line once there is a cursor, and what has been
    /// typed towards a command, the way vim shows a pending count.
    fn status(&self) -> ToolStatus {
        let mut segments = Vec::new();
        let page = self.page_label();
        if !page.is_empty() {
            segments.push(page);
        }
        if let Some(pos) = self.caret.pos {
            segments.push(format!("line {}", pos.line + 1));
        }
        let typed = self.typed();
        if !typed.is_empty() {
            segments.push(typed);
        }
        ToolStatus {
            mode: self.edit_mode(),
            segments,
        }
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        // The space bar may come as the character it types.
        let key = &match key.code {
            KeyCode::Char(' ') => Key {
                code: KeyCode::Space,
                ..*key
            },
            _ => *key,
        };
        if self.outline.is_some() {
            return self.on_outline_key(key);
        }
        // A prefix owns the key after it: the `t` of `zt` is not the till
        // find, and the `g` of `gg` is not a fresh prefix.
        match self.pending.prefix {
            Prefix::Find(kind) => return self.on_find_key(kind, key),
            Prefix::Keys(first) => return self.on_sequence_key(first, key),
            Prefix::None => {}
        }
        if key.alt {
            return ToolOutcome::Ignored;
        }
        // A count is typed ahead of the motion it multiplies.
        if let (false, KeyCode::Char(digit)) = (key.ctrl, key.glyph()) {
            if self.take_count_digit(digit) {
                return ToolOutcome::Consumed;
            }
        }
        match table::lookup(COMMON_KEYS, std::slice::from_ref(key)) {
            TableMatch::Exact(act) => return self.on_act(act),
            TableMatch::Prefix => {
                self.pending.prefix = Prefix::Keys(*key);
                return ToolOutcome::Consumed;
            }
            TableMatch::Miss => {}
        }
        let act = self
            .mode_tables()
            .into_iter()
            .find_map(|keys| table::find(keys, key));
        match act {
            Some(act) => self.on_act(act),
            None => ToolOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(COMMON_KEYS);
        for keys in self.mode_tables() {
            rows.extend(table::help(keys));
        }
        rows
    }

    fn hint(&self) -> Option<ToolHint> {
        let items = match self.pending.prefix {
            Prefix::Find(_) => vec![(
                "{char}".to_string(),
                "the character on this line to move to".to_string(),
            )],
            Prefix::Keys(first) => table::continuations(COMMON_KEYS, &[first]),
            Prefix::None => return None,
        };
        Some(ToolHint {
            items,
            title: self.typed(),
        })
    }

    fn on_mouse(&mut self, at: Point) -> ToolOutcome {
        let (rows, cols) = self.grid;
        if rows == 0 || cols == 0 {
            return ToolOutcome::Ignored;
        }
        if let Some(outcome) = self.on_chrome_mouse(at) {
            return outcome;
        }
        let x = (at.col as f32 + 0.5) * self.cell.0;
        let y = (at.row as f32 + 0.5) * self.cell.1 - self.toolbar_height();
        let Some(pos) = self.char_at(x, y) else {
            return ToolOutcome::Consumed;
        };
        match at.drag {
            true => self.caret.point(pos, self.drag_origin),
            false => {
                self.drag_origin = Some(pos);
                self.caret.point(pos, None);
            }
        }
        // What a drag selects is copied as it grows, the way a terminal's
        // selection is: the host hears of no release to wait for.
        if !at.drag {
            return ToolOutcome::Consumed;
        }
        let selected = self
            .document
            .as_mut()
            .and_then(|doc| self.caret.selected_text(doc));
        match selected {
            Some(text) if !text.is_empty() => copy(text),
            _ => ToolOutcome::Consumed,
        }
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        // The wheel reports rows the way the grid counts them, positive
        // upwards, which is the opposite of how far the pages move.
        self.execute(Command::ScrollLines(-lines))
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        match (reply.tag, reply.answer) {
            (ASK_PAGE, answer) => {
                match answer.and_then(|page| page.trim().parse::<usize>().ok()) {
                    Some(page) => self.act(Command::GoToPage(page)),
                    None => ToolOutcome::Consumed,
                }
            }
            (ASK_ZOOM, answer) => match answer.as_deref().and_then(Self::parse_zoom) {
                Some(zoom) => self.act(Command::SetZoom(zoom)),
                None => ToolOutcome::Consumed,
            },
            (ASK_SEARCH, Some(pattern)) => self.start_search(pattern, true),
            (ASK_SEARCH_BACK, Some(pattern)) => self.start_search(pattern, false),
            _ => {
                // A prompt escaped out of leaves the cursor and the last
                // search as they were, and takes the half-typed command
                // with it.
                self.back_to_search_origin();
                if let Some((_, search)) = self.search_origin.take() {
                    self.search = search;
                }
                self.pending = Pending::default();
                ToolOutcome::Consumed
            }
        }
    }

    /// Each letter typed at the prompt searches again from where it opened,
    /// so the match follows the typing. Not under an operator: `y/` copies
    /// once, when the answer is in.
    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> ToolOutcome {
        let forward = tag == ASK_SEARCH;
        if !(forward || tag == ASK_SEARCH_BACK) || self.pending.operator != Operator::Move {
            return ToolOutcome::Consumed;
        }
        self.back_to_search_origin();
        if query.is_empty() {
            return ToolOutcome::Consumed;
        }
        let from = self.screen_line(0.0);
        if let Some(doc) = self.document.as_mut() {
            let found = self
                .caret
                .search(doc, query, forward, 1, Operator::Move, from);
            if found == Effect::Moved {
                self.scroll_caret_into_view();
            }
        }
        // Shown in the bar as it is typed, with how many there are.
        self.search = Some(Search::new(query.to_string(), forward));
        self.searched = true;
        ToolOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        let waiting = self.document.is_none() || !self.requested.is_empty();
        (self.worker.is_some() && waiting).then(|| Instant::now() + POLL)
    }

    fn on_tick(&mut self, _now: Instant) -> ToolOutcome {
        self.take_replies();
        ToolOutcome::Consumed
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.path))
    }

    fn file_reference(&self) -> Option<String> {
        let base = abbreviate_home(&self.path);
        match self.page_count() {
            0 => Some(base),
            _ => Some(format!("{base}:{}", self.current_page() + 1)),
        }
    }

    /// The document, at the page being read, so a restart brings the tab
    /// back where it was.
    fn snapshot(&self) -> Option<OpenTarget> {
        let page = match self.page_count() {
            0 => self.resume_page.unwrap_or(1),
            _ => self.current_page() + 1,
        };
        Some(OpenTarget::at_line(self.path.clone(), page))
    }

    fn tiles(&self) -> Vec<Tile> {
        let Some(column) = self.column() else {
            return Vec::new();
        };
        let mut tiles: Vec<Tile> = column
            .visible(self.scroll.1, self.pane.1)
            .map(|page| {
                let rect = column.pages[page];
                let blank = match self.inverted {
                    true => &self.overlays.blank_inverted,
                    false => &self.overlays.blank,
                };
                let (key, pixels) = match self.pages.get(&page) {
                    Some(drawn) => (drawn.key, Arc::clone(&drawn.pixels)),
                    None => (blank.key, Arc::clone(&blank.pixels)),
                };
                Tile {
                    content: Content::Pixels(pixels),
                    height: rect.height,
                    key,
                    width: rect.width,
                    x: rect.x - self.scroll.0,
                    y: rect.y - self.scroll.1,
                }
            })
            .collect();
        tiles.extend(self.overlay_tiles(&column));
        // Laid out in the pane below the toolbar, and placed in the whole.
        let top = self.toolbar_height();
        for tile in &mut tiles {
            tile.y += top;
        }
        tiles
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        // The pages are laid out below the toolbar, measured in the cells the
        // last paint counted.
        let (rows, cols) = self.grid;
        if rows > 0 && cols > 0 {
            self.cell = (pane.0 / cols as f32, pane.1 / rows as f32);
        }
        let pane = (pane.0, (pane.1 - self.toolbar_height()).max(0.0));
        if pane != self.pane {
            // A resize keeps the reader on the same spot of the same page, as
            // a zoom does: fitted to the width, it is one.
            let zoom = self.zoom;
            let resized = self.pane != (0.0, 0.0);
            self.pane = pane;
            match resized {
                true => self.set_zoom(zoom),
                false => self.request_visible(),
            }
        }
        self.read_visible_text();
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Copy `text` to the clipboard, saying how many characters it holds rather
/// than echoing what may be pages of it.
fn copy(text: String) -> ToolOutcome {
    let count = text.chars().count();
    let noun = match count {
        1 => "character",
        _ => "characters",
    };
    ToolOutcome::Copy {
        notice: format!("Copied {count} {noun}"),
        text,
    }
}

/// What vim's own key for `kind` is, as the status bar shows it pending.
fn find_key(kind: FindKind) -> &'static str {
    match kind {
        FindKind::Backward => "F",
        FindKind::BackwardTill => "T",
        FindKind::Forward => "f",
        FindKind::ForwardTill => "t",
    }
}

/// How far below the pane's top something `height` tall stands when placed
/// at `place` in a pane `room` tall, `margin` off the edge it is put at.
fn place_offset(place: PagePlace, height: f32, room: f32, margin: f32) -> f32 {
    match place {
        PagePlace::Top => margin,
        PagePlace::Center => (room - height) / 2.0,
        PagePlace::Bottom => room - height - margin,
    }
}

/// How far `value` lies outside `[low, high]`, or nothing inside it.
fn outside(value: f32, low: f32, high: f32) -> f32 {
    if value < low {
        return low - value;
    }
    (value - high).max(0.0)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests;

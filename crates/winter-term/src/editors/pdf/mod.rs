//! PDF: one document, drawn by Winter itself.
//!
//! The pages are drawn on a thread of their own and placed over the pane as
//! pictures; their text is rebuilt from the glyphs, for a vim cursor to move
//! through, search, select, and copy.
//!
//! The viewer's state and its types are here; what it does is split by
//! topic: [`keys`], [`view`], [`navigate`], [`pages`], [`marks`],
//! [`chrome`], and the window's side of it in [`editor`].

mod caret;
mod chrome;
mod document;
mod editor;
mod keys;
mod layout;
mod links;
mod marks;
mod navigate;
mod outline;
mod outline_list;
mod pages;
mod references;
mod sentences;
mod spelling;
mod text;
mod toolbar;
mod view;
mod worker;

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::model::editor::{DropList, EditorOutcome, Notice, OpenTarget, PdfMark};
use crate::model::input::Key;
use crate::model::picture::{Content, Pixels, Tile};
use crate::model::textedit::search::Search;

use caret::{Caret, FindKind, Motion, Operator, Pos, Visual};
use document::Document;
use layout::Crop;
use links::Link;
use outline::Target;
use outline_list::OutlineList;
use sentences::Run;
use crate::editors::synctex::{Context, Source};
use worker::Worker;

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

/// The narrowest a SyncTeX mark is drawn, in points: a box SyncTeX gives no
/// width still shows where it is.
const MIN_MARK_WIDTH: f32 = 3.0;

/// The keys a link's label is made of, the home row first, as magic-vscode's
/// are.
const HINT_KEYS: &[char] = &[
    'a', 's', 'd', 'f', 'g', 'h', 'j', 'k', 'l', 'q', 'w', 'e', 'r', 't', 'y', 'u', 'i', 'o', 'p',
];

/// How much of a link's target its preview shows, in points: from a little
/// above the spot, so a heading it sits just under is not cut, down a
/// paragraph or two.
const PREVIEW_ABOVE: f32 = 12.0;
const PREVIEW_HEIGHT: f32 = 160.0;

/// The widest a preview is drawn, as a share of the pane's width.
const PREVIEW_WIDTH_SHARE: f32 = 0.9;

/// Pane pixels between a link and its preview, and the preview's frame.
const PREVIEW_GAP: f32 = 6.0;
const PREVIEW_FRAME: f32 = 2.0;

/// How long a scroll by keys or the wheel takes to glide to where it
/// lands, and how often the glide is drawn on its way.
const GLIDE: Duration = Duration::from_millis(120);
const GLIDE_FRAME: Duration = Duration::from_millis(16);

/// The thumbnails' strip down the pane's left: its share of the pane's
/// width, kept between a narrowest and a widest, in pixels; how thick the
/// frame round the page being read is; and how many thumbnails past either
/// end of the strip are drawn ahead.
const THUMB_SHARE: f32 = 0.2;
const THUMB_MIN_WIDTH: f32 = 96.0;
const THUMB_MAX_WIDTH: f32 = 200.0;
const THUMB_FRAME: f32 = 2.0;
const THUMB_LOOKAHEAD: usize = 2;

/// How far past the text's box a page cropped to its text keeps, in
/// points, so the ink at its edges is not cut.
const CROP_MARGIN: f32 = 6.0;

/// How long the pointer rests on a link before where it leads is shown, as
/// magic-vscode waits: long enough that a pointer passing over is let be.
const HOVER_DELAY: Duration = Duration::from_millis(100);

/// How often the file is looked at for a rebuild.
const WATCH_INTERVAL: Duration = Duration::from_millis(500);

/// How many times a rebuilt file that will not open is read again before
/// the viewer gives up on it until it changes again: a build may still be
/// writing it.
const RELOAD_ATTEMPTS: u32 = 3;

/// Pages either side of the view drawn ahead, so scrolling onto one finds it
/// drawn.
const LOOKAHEAD: usize = 1;

/// How long a drawn page no view wants is kept. Two tabs showing the document
/// each want their own pages in turn, frame by frame; kept this long, neither
/// lets go of the other's.
const KEEP_UNWANTED: Duration = Duration::from_secs(1);

/// Where down the pane the page being read is reckoned from: a third of the
/// way, where the eye reads, not the top edge a page is half scrolled off.
const READING_LINE: f32 = 1.0 / 3.0;

/// How near, in points down the same page, a place may be to the last one
/// kept for `C-o` and be taken for it rather than kept as another.
const SAME_PLACE: f32 = 10.0;

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

/// The links' outlines, as magic-vscode draws them: a frame around a link
/// to a spot in the document, a line under a URL, and how thick either is
/// drawn, in pane pixels.
const LINK_COLOR: [u8; 4] = [0, 105, 218, 150];
const URL_COLOR: [u8; 4] = [40, 150, 255, 230];
const LINK_LINE: f32 = 1.0;

/// The line under a word spelled wrong: the orange a warning is, and how
/// thick, in pane pixels.
const SPELLING_COLOR: [u8; 4] = [214, 120, 0, 230];
const SPELLING_LINE: f32 = 2.0;

/// The tints the sentences are told apart by, one after another, as
/// magic-vscode tints them: four muted hues, no yellow, which a search's
/// match is.
const SENTENCE_COLORS: [[u8; 4]; 4] = [
    [0, 150, 136, 71],
    [63, 110, 200, 66],
    [196, 84, 138, 66],
    [122, 156, 58, 77],
];

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
const MENU: [&[(Act, &str)]; 4] = [
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
    &[(Act::Invert, "Invert Colors"), (Act::ShowLinks, "Show Links")],
    &[(Act::SyncBack, "SyncTeX Backward")],
];

/// The rows the toolbar takes at the pane's top.
const TOOLBAR_ROWS: usize = 1;

/// The most of the pane's rows the open outline takes under the toolbar.
const OUTLINE_SHARE: f32 = 0.6;

/// The cell column the outline's card starts at, under the outline button.
const OUTLINE_COL: usize = 1;

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

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the viewer's own does, before the count and mode typed with
/// it say how.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Act {
    /// Escape with no cursor: drop what is half-typed.
    Abandon,
    /// Take the word spelled wrong last stepped to as right.
    AddWord,
    /// Ask which page to go to.
    AskPage,
    /// A cursor motion.
    Caret(Motion),
    /// `G` with a cursor: the end, or the page counted to.
    CaretEnd,
    /// Center the pages across the pane.
    Center,
    /// Scroll sideways this many steps per count.
    Columns(isize),
    /// Count the words selected, or the document's.
    Count,
    /// Crop the pages to their text, or show them whole again.
    Crop,
    /// Put a cursor in the text.
    Cursor,
    /// Wait for the character to find.
    Find(FindKind),
    /// `gg`: the first page, or with a cursor where the text starts.
    First,
    /// Fit the text's width to the pane's.
    FitContent,
    /// Fit the whole page in the pane.
    FitPage,
    /// Fit the page to the pane's width.
    FitWidth,
    /// Follow the link under the cursor.
    FollowAtCaret,
    /// Label the links in view, or the lines, for what a label is chosen
    /// for.
    Hints(HintUse),
    /// The first page.
    Home,
    /// Invert the pages' colors, or put them back.
    Invert,
    /// `G` with no cursor: the last page, or the page counted to.
    Last,
    /// Go back along the jumps made when set, forward again otherwise.
    Jump(bool),
    /// Escape with a cursor: one step back.
    Leave,
    /// Scroll this many lines per count.
    Lines(isize),
    /// The next word spelled wrong when set, the previous otherwise.
    Misspelling(bool),
    /// Open or close the outline.
    Outline,
    /// Step this many pages per count.
    Pages(isize),
    /// Put the page, or the cursor's line, at an edge.
    Place(PagePlace),
    /// Preview where the link under the cursor leads.
    PreviewAtCaret,
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
    /// Tint the sentences each their own color, or stop.
    Sentences,
    /// Underline the words spelled wrong, or stop.
    Spelling,
    /// Outline every link in view, or go back to only the one under the
    /// pointer.
    ShowLinks,
    /// Swap the selection's ends.
    SwapEnds,
    /// Go back to the LaTeX source of the cursor's spot.
    SyncBack,
    /// Show the pages small down the left, or hide them.
    Thumbnails,
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
pub struct PdfEditor {
    /// The cursor in the text, and what it has selected.
    caret: Caret,
    /// A cell's size in pixels, which a pointer's cell is turned back into
    /// pixels by.
    cell: (f32, f32),
    /// A change to the file seen at the last look, read once it has held
    /// still for another.
    changed: Option<Stamp>,
    /// Whether the pages are cropped to their text.
    cropped: bool,
    /// The file as last read, which a rebuild is told apart from.
    disk: Option<Stamp>,
    /// The document, once the thread has opened it.
    document: Option<Document>,
    /// Where a drag with the pointer started.
    drag_origin: Option<Pos>,
    /// Pages that could not be drawn, not asked for again.
    failed: HashSet<usize>,
    /// Numbers each layout, so a page drawn for one left behind is not taken
    /// for the current one.
    generation: u64,
    /// A scroll gliding to where it lands, while it is on its way.
    glide: Option<Glide>,
    /// The pane's rows and columns as last painted.
    grid: (usize, usize),
    /// The links in view, while their labels are up.
    hints: Option<Hints>,
    /// The link the pointer rests on, while it does.
    hover: Option<Hover>,
    /// Whether the pages are drawn with their colors inverted, light text on
    /// dark.
    inverted: bool,
    /// The places jumps left (a link, a section, a page asked for, either
    /// end), latest last, for `C-o`, and the ones gone back from, for `C-i`.
    jumps_ahead: Vec<Place>,
    jumps_back: Vec<Place>,
    /// Whether every link in view is outlined, not only the one under the
    /// pointer.
    links_shown: bool,
    /// Where a SyncTeX forward search landed, marked until the next key or
    /// click.
    mark: Option<PdfMark>,
    /// The character a right-click opened the menu over, for the one key
    /// the menu then sends.
    menu_spot: Option<Pos>,
    notice: Option<Notice>,
    /// The outline, while it is open under the toolbar.
    outline: Option<OutlineList>,
    /// The open outline as last painted: a drop-down list over the pages.
    outline_drop: Option<DropList>,
    /// The pane's whole size, as the host last told it, the toolbar and
    /// the thumbnails' strip in it.
    outer: (f32, f32),
    /// The pixels drawn over the pages.
    overlays: Overlays,
    /// Whether no tab shows the viewer, its buffer buried: the file is not
    /// watched until one does.
    hidden: bool,
    /// Each page drawn and kept, at whatever scale it was drawn.
    pages: HashMap<usize, DrawnPage>,
    /// When each page was last wanted by a view, which keeps it drawn.
    wanted_at: HashMap<usize, Instant>,
    /// The size in pixels of the pane below the toolbar, as last drawn,
    /// which is what the pages are laid out in.
    pane: (f32, f32),
    path: PathBuf,
    /// The command being typed: a count, an operator, and any prefix waiting
    /// for the key that completes it.
    pending: Pending,
    /// A link's target, shown under it before it is followed.
    preview: Option<Preview>,
    /// The rebuilt file being read, the document on screen kept until it
    /// has opened.
    reload: Option<Worker>,
    /// How many reads of the rebuilt file have failed in a row.
    reload_failures: u32,
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
    /// A SyncTeX backward search on its way.
    /// Each page's sentences, while they are tinted: `None` while they
    /// are not, a page missing until it has been in view.
    sentences: Option<HashMap<usize, Vec<Vec<Run>>>>,
    /// The words spelled wrong, while they are checked.
    spelling: Option<Spelling>,
    sync: Option<BackSync>,
    /// A SyncTeX mark to show once the document has opened and the pane
    /// has a size.
    /// The box the text of the document's first pages falls in, in
    /// points, once a crop or a fit to it has asked for it.
    text_box: Option<Crop>,
    /// The pages drawn small down the pane's left, while they are shown.
    thumbnails: Option<Thumbnails>,
    waiting_mark: Option<PdfMark>,
    worker: Option<Worker>,
    zoom: Zoom,
}

/// How far a SyncTeX backward search has got, carrying the text around the
/// spot it started from to narrow SyncTeX's line down with.
enum BackSync {
    /// `synctex edit` is running.
    Locating(Context),
    /// The source SyncTeX named is being read.
    Reading(Context, Source),
    /// The files that source includes near its line are being read.
    Including(Context, Source),
}

/// A page as drawn: its pixels, and the scale they were drawn at.
struct DrawnPage {
    /// Its pixels cut to the crop they were cut to, while the pages are
    /// cropped to their text.
    cropped: Option<(Crop, u64, Arc<Pixels>)>,
    /// Whether it was drawn with its colors inverted.
    inverted: bool,
    key: u64,
    pixels: Arc<Pixels>,
    scale: f32,
    /// Whether it was drawn from the file before a rebuild: shown until the
    /// page is drawn anew, rather than a blank one.
    stale: bool,
}

/// A scroll gliding from where it was to where it lands, eased so it
/// slows into place.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Glide {
    from: (f32, f32),
    started: Instant,
    to: (f32, f32),
}

/// The links in view while their labels are up, and what typing one does.
struct Hints {
    /// What a label is chosen for.
    purpose: HintUse,
    links: Vec<Hinted>,
    /// What has been typed of a label so far.
    typed: String,
}

/// What the labels are put up for: to follow a link, to preview where one
/// leads, or to go to a line's LaTeX source.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum HintUse {
    Follow,
    Preview,
    Sync,
}

/// A box on a page, as `(left, top, width, height)` in points down from
/// its top-left.
type Bounds = (f32, f32, f32, f32);

/// What a label stands for: a link, or the start of a line of text.
enum HintTo {
    Line(Pos),
    Link(Link),
}

/// A link in view, with the label it is chosen by.
struct Hinted {
    /// Where on its page the label is put.
    bounds: Bounds,
    /// What the label is drawn under, for the host to draw it once.
    key: u64,
    label: String,
    /// The page it is on.
    page: usize,
    to: HintTo,
}

/// The link the pointer rests on, and when where it leads is due to show.
struct Hover {
    /// When the preview is due; `None` once it is shown.
    due: Option<Instant>,
    link: Link,
    /// The page the link is on.
    page: usize,
}

/// The pages drawn small down the pane's left, the one being read framed.
#[derive(Default)]
struct Thumbnails {
    /// Each page drawn small, once drawn.
    drawn: HashMap<usize, DrawnThumb>,
    /// The pages asked to be drawn small, with the layout each was asked
    /// for: one asked for a layout since left is asked for again.
    requested: HashMap<usize, u64>,
}

/// The words spelled wrong, found a page at a time.
#[derive(Default)]
struct Spelling {
    /// The one last stepped to, by its page and its place among the page's.
    at: Option<(usize, usize)>,
    /// Each page's words spelled wrong, once it has been checked.
    found: HashMap<usize, Vec<Run>>,
}

/// A page drawn small.
struct DrawnThumb {
    /// Whether it was drawn with its colors inverted.
    inverted: bool,
    key: u64,
    pixels: Arc<Pixels>,
}

/// A link's target, shown under the link before it is followed.
struct Preview {
    /// The target page's pixels around the spot, once the page is drawn,
    /// under a key of their own.
    crop: Option<(u64, Arc<Pixels>)>,
    /// Whether the pointer resting on the link opened it, to close when the
    /// pointer leaves the link.
    hovered: bool,
    link: Link,
    /// The page the link is on.
    page: usize,
    target: Target,
}

/// What one tab's view of the document keeps while another tab on it is the
/// one acted on: where it is scrolled to, at what zoom, in what pane, and the
/// cursor in the text.
#[derive(Clone, Debug)]
struct PdfView {
    caret: Caret,
    cell: (f32, f32),
    grid: (usize, usize),
    jumps_ahead: Vec<Place>,
    jumps_back: Vec<Place>,
    outer: (f32, f32),
    pane: (f32, f32),
    scroll: (f32, f32),
    zoom: Zoom,
}

/// A place the reader was: a page, and how far down it in points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Place {
    offset: f32,
    page: usize,
}

/// When the file was last written, and how long it was then: enough to tell
/// a rebuild from the file as it was read.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Stamp {
    len: u64,
    modified: Option<SystemTime>,
}

/// The single-color pixels stretched over the pages.
struct Overlays {
    blank: Overlay,
    blank_inverted: Overlay,
    caret: Overlay,
    found: Overlay,
    link: Overlay,
    selection: Overlay,
    sentences: [Overlay; SENTENCE_COLORS.len()],
    spelling: Overlay,
    url: Overlay,
}

/// One color, as a pixel stretched to whatever size it is drawn.
struct Overlay {
    key: u64,
    pixels: Arc<Pixels>,
}

/// How the pages are scaled.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Zoom {
    /// The text's box fills the pane's width, its margins off the sides.
    FitContent,
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
    /// The first key of a sequence in `COMMON_KEYS` (`g` of `gg`, `z` of
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
    /// Center the pages across the pane.
    CenterPages,
    /// Fit the text's width to the pane, its margins off the sides.
    FitContent,
    /// Fit the page's width to the pane, which also ends a zoom.
    FitWidth,
    /// Crop the pages to their text, or show them whole again.
    ToggleCrop,
    /// Invert the pages' colors, or put them back.
    Invert,
    /// Open the outline, or close it.
    ToggleOutline,
    /// Say how many words are selected, or are in the document.
    ReportCount,
    /// Outline every link in view, or only the one under the pointer.
    ToggleLinks,
    /// Tint the sentences each their own color, or stop.
    ToggleSentences,
    /// Show the pages small down the left, or hide them.
    ToggleThumbnails,
    /// Underline the words spelled wrong, or stop.
    ToggleSpelling,
    /// Go to the next word spelled wrong when set, the previous otherwise.
    StepMisspelling(bool),
    /// Take the word spelled wrong last stepped to as right.
    AddWord,
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
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// A tab showing `path`, opened on a thread of its own.
    pub fn new(path: PathBuf) -> Self {
        Self::opening(Some(Worker::open(path.clone())), path)
    }

    /// The editor showing `path`, turned to `page` as soon as the document has
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
            caret: Caret::default(),
            cell: (0.0, 0.0),
            changed: None,
            cropped: false,
            disk: stamp(&path),
            document: None,
            drag_origin: None,
            failed: HashSet::new(),
            generation: 0,
            glide: None,
            grid: (0, 0),
            hints: None,
            hover: None,
            inverted: false,
            jumps_ahead: Vec::new(),
            jumps_back: Vec::new(),
            links_shown: false,
            mark: None,
            menu_spot: None,
            notice: None,
            outline: None,
            outline_drop: None,
            outer: (0.0, 0.0),
            overlays: Overlays {
                blank: Overlay::new(BLANK_COLOR),
                blank_inverted: Overlay::new(BLANK_INVERTED_COLOR),
                caret: Overlay::new(CARET_COLOR),
                found: Overlay::new(FOUND_COLOR),
                link: Overlay::new(LINK_COLOR),
                selection: Overlay::new(SELECTION_COLOR),
                sentences: SENTENCE_COLORS.map(Overlay::new),
                spelling: Overlay::new(SPELLING_COLOR),
                url: Overlay::new(URL_COLOR),
            },
            hidden: false,
            pages: HashMap::new(),
            wanted_at: HashMap::new(),
            pane: (0.0, 0.0),
            path,
            pending: Pending::default(),
            preview: None,
            reload: None,
            reload_failures: 0,
            requested: HashSet::new(),
            resume_page: None,
            scroll: (0.0, 0.0),
            search: None,
            search_origin: None,
            searched: false,
            sentences: None,
            spelling: None,
            sync: None,
            text_box: None,
            thumbnails: None,
            waiting_mark: None,
            worker,
            zoom: Zoom::FitWidth,
        }
    }

    // --------------------------------------------------------------------
    // The grammar
    // --------------------------------------------------------------------

}

// ========================================================================
// Free functions
// ========================================================================

/// Copy `text` to the clipboard, saying how many characters it holds rather
/// The first `count` labels made of [`HINT_KEYS`], all the same length:
/// the shortest that gives every link one of its own.
fn hint_labels(count: usize) -> Vec<String> {
    let base = HINT_KEYS.len();
    let mut length = 1;
    while base.pow(length) < count {
        length += 1;
    }
    (0..count)
        .map(|mut index| {
            let mut label = vec![HINT_KEYS[0]; length as usize];
            for slot in label.iter_mut().rev() {
                *slot = HINT_KEYS[index % base];
                index /= base;
            }
            label.into_iter().collect()
        })
        .collect()
}

/// The four sides of `rect`, `(left, top, width, height)` in the pane,
/// drawn `thickness` thick in `overlay`'s color, inside it.
fn frame(overlay: &Overlay, rect: (f32, f32, f32, f32), thickness: f32) -> [Tile; 4] {
    let (x, y, width, height) = rect;
    [
        (x, y, width, thickness),
        (x, y + height - thickness, width, thickness),
        (x, y, thickness, height),
        (x + width - thickness, y, thickness, height),
    ]
    .map(|side| overlay.tile(side))
}

/// The part of `pixels` in `rect`, `(left, top, width, height)` in its
/// pixels, cut short at its edges.
fn crop_rect(pixels: &Pixels, rect: (u32, u32, u32, u32)) -> Pixels {
    let (left, top) = (rect.0.min(pixels.width), rect.1.min(pixels.height));
    let width = rect.2.min(pixels.width - left);
    let height = rect.3.min(pixels.height - top);
    let row = pixels.width as usize * 4;
    let rgba = (top..top + height)
        .flat_map(|y| {
            let start = y as usize * row + left as usize * 4;
            pixels.rgba[start..start + width as usize * 4].iter().copied()
        })
        .collect();
    Pixels {
        height,
        rgba,
        width,
    }
}

/// When `path` was last written and how long it is, or `None` where it
/// cannot be looked at.
fn stamp(path: &Path) -> Option<Stamp> {
    let meta = fs::metadata(path).ok()?;
    Some(Stamp {
        len: meta.len(),
        modified: meta.modified().ok(),
    })
}

/// Open `source` in the editor, at its line, beside the PDF viewer so the
/// document stays in sight.
fn open_source(source: Source) -> EditorOutcome {
    EditorOutcome::OpenBeside(OpenTarget::at_line(source.path, source.line))
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

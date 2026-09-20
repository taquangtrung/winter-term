//! Tool pages: pane content Winter paints itself, as styled text rows instead
//! of terminal output.

use std::path::PathBuf;

use super::input::Key;

// ========================================================================
// Data Structures
// ========================================================================

/// What a page wants drawn, top to bottom.
#[derive(Clone, Debug, Default)]
pub struct PageContent {
    /// The row to band as the cursor line, counted from the first page row.
    pub cursor_line: Option<usize>,
    /// Icons to draw over the rows, one per row that has one.
    pub icons: Vec<PageIcon>,
    /// Every line of the page.
    pub rows: Vec<PageRow>,
    /// For each row, the column its wrapped continuation starts at: a diff
    /// line reserves its marker's room, so the text it spills onto the next
    /// screen row lines up under the text it started with, not under the
    /// marker. Rows past the list, and pages that set nothing, wrap from
    /// column zero.
    pub wrap_indents: Vec<usize>,
}

/// An icon a page wants drawn beside one of its rows.
///
/// A page names *what* the icon is and where it goes, never how it is drawn:
/// the host resolves that against the `icons` setting, painting a glyph into
/// the row, rasterizing the bundled artwork over it, or neither. The row's
/// text always leaves [`PageIcon::WIDTH`] columns clear at `col`, so every
/// column downstream sits in the same place whichever way the icon is drawn.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageIcon {
    /// Column of the icon's left edge.
    pub col: usize,
    /// The glyph to draw when the icon style is a font, in place of artwork.
    pub glyph: char,
    /// What the icon depicts.
    pub kind: PageIconKind,
    /// Row within the page's returned rows.
    pub row: usize,
}

impl PageIcon {
    /// Columns an icon occupies: the artwork is square and a cell is roughly
    /// half as wide as it is tall, so one cell would squash it.
    pub const WIDTH: usize = 2;
}

/// What a [`PageIcon`] depicts, in terms the icon set can resolve.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageIconKind {
    /// A directory, whose artwork differs by whether it is expanded.
    Dir {
        /// Whether the directory's children are listed beneath it.
        expanded: bool,
        /// The directory's own name, without a path.
        name: String,
    },
    /// A file, named so the icon set can match it by name or extension.
    File {
        /// The file's name, without a path.
        name: String,
    },
    /// A Git working-tree status, named as the bundled set spells it.
    Git {
        /// The status stem, e.g. `git-modified`.
        status: String,
    },
}

/// One rendered line: styled runs laid out left to right from column zero.
pub type PageRow = Vec<PageSpan>;

/// The rows of a page to paint in a pane, once wrapping is accounted for:
/// where the window starts, how many rows fill the pane, and which screen
/// row the cursor paints on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PageWindow {
    /// How many rows to paint; the pane clips whatever wraps past its bottom.
    pub count: usize,
    /// The screen row the cursor's row paints on, counted from the first
    /// painted row, in screen rows once wrapping is on.
    pub cursor: usize,
    /// The first of the page's own rows to paint.
    pub start: usize,
}

/// A run of text sharing one style.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageSpan {
    /// How the run is colored.
    pub style: PageStyle,
    /// The run's text.
    pub text: String,
}

/// Slow work a page is asking the host to do off the event-loop thread.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobRequest {
    /// Run a program and capture what it wrote.
    Command(CommandRequest),
    /// Total the bytes under this directory.
    DirSize(PathBuf),
    /// Read these files, skipping the ones that are not there. For the small
    /// state files a program keeps beside its data, where "not there" is an
    /// answer rather than a failure.
    ReadFiles(Vec<PathBuf>),
    /// Find the lines under a directory that hold some text.
    Search(SearchRequest),
}

/// A program to run on a page's behalf.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// The program.
    pub program: String,
    /// What to write to the program's standard input, for a command that reads
    /// its payload rather than taking it as an argument.
    pub stdin: Option<String>,
    /// Which request this is, so the page knows what finished.
    pub tag: &'static str,
}

/// What a page waiting on the second key of a command offers: what the
/// sequence is called, and the keys that complete it with what each does.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PageHint {
    /// The continuations, as `(key, what it does)`.
    pub items: Vec<(String, String)>,
    /// What the sequence is called, shown as the hint's heading.
    pub title: String,
}

/// A choice a page asks the host to collect from a list it already knows: the
/// branches to check out, the tags to delete, the remotes to prune.
///
/// What the host shows is a filter over `items`, not a text field: a name that
/// is not in the list is not an answer, which is the difference between this
/// and a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickRequest {
    /// The choices, in the order they should read.
    pub items: Vec<String>,
    /// What the list is of, shown where a prompt shows its question.
    pub label: String,
    /// Which question this answers, so the page knows what came back.
    pub tag: &'static str,
}

/// A question a page answers from a list rather than from typing, held while
/// the list itself is gathered.
///
/// The gathering differs per tool — a git branch list is a command away, a
/// directory's own rows are already in hand — but what happens to the answer
/// does not: it arrives under [`PickQuestion::tag`] as the reply to the same
/// question a [`PromptRequest`] would have asked. A list that comes back
/// empty is no list at all, and every constructor here says so with `None`,
/// which is the caller's cue to fall back to asking for it typed.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PickQuestion {
    /// What the list is of, shown as the picker's heading.
    pub label: String,
    /// The question the choice answers.
    pub tag: &'static str,
}

impl PickQuestion {
    /// A question to be answered from a list.
    pub fn new(tag: &'static str, label: impl Into<String>) -> Self {
        Self {
            label: label.into(),
            tag,
        }
    }

    /// The list a page already holds, in the order it should read.
    pub fn over(&self, items: Vec<String>) -> Option<PickRequest> {
        (!items.is_empty()).then(|| PickRequest {
            items,
            label: self.label.clone(),
            tag: self.tag,
        })
    }

    /// The list a program wrote, one candidate per line: blank lines are
    /// dropped, each line is trimmed, and `keep` has the last word on what
    /// belongs (a remote's own `HEAD`, say, which stands for a name already
    /// listed).
    pub fn over_lines(&self, text: &str, keep: impl Fn(&str) -> bool) -> Option<PickRequest> {
        self.over(
            text.lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && keep(line))
                .map(str::to_string)
                .collect(),
        )
    }
}

/// A text search over a directory tree, run without a pattern engine: the
/// query is literal text, matched whatever the case.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchRequest {
    /// The text every reported line holds.
    pub query: String,
    /// Where the walk starts.
    pub root: PathBuf,
}

/// The answer to a [`JobRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum JobReply {
    /// What a program wrote, and how it exited.
    Command(CommandOutput),
    /// What a directory holds, with unreadable parts skipped rather than
    /// reported as an error: a total is worth more than a failure here.
    DirSize {
        /// Bytes under the directory.
        bytes: u64,
        /// The directory that was totalled.
        path: PathBuf,
    },
    /// What the files that could be read hold, keyed by the path asked for.
    /// A file that is missing or unreadable is simply absent.
    Files(Vec<(PathBuf, String)>),
    /// The lines a search found.
    Search(SearchResult),
}

/// What running a program produced. A program that could not be started at all
/// reports no code and its reason on `stderr`, so one path covers both.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandOutput {
    /// Exit status, absent when the program never ran.
    pub code: Option<i32>,
    /// What it wrote to standard error.
    pub stderr: String,
    /// What it wrote to standard output.
    pub stdout: String,
    /// The tag of the request this answers.
    pub tag: &'static str,
}

impl CommandOutput {
    /// Whether the program ran and reported success.
    pub fn succeeded(&self) -> bool {
        self.code == Some(0)
    }

    /// The first line of the failure, for a one-line report.
    pub fn failure(&self) -> String {
        let text = if self.stderr.trim().is_empty() {
            self.stdout.trim()
        } else {
            self.stderr.trim()
        };
        text.lines().next().unwrap_or("failed").to_string()
    }
}

/// What a search found. A walk that hit its cap reports what it had rather
/// than failing, since a first page of matches is still worth showing.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SearchResult {
    /// The matching lines, in the order the walk found them.
    pub hits: Vec<SearchHit>,
    /// The query this answers, so a page that has moved on to another one can
    /// tell that this is not about the query it is showing.
    pub query: String,
    /// Whether the cap stopped the walk with matches left unreported.
    pub truncated: bool,
}

/// One matching line.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SearchHit {
    /// Which line of the file it is, counting from one.
    pub line: usize,
    /// The file holding it.
    pub path: PathBuf,
    /// The line itself, with trailing whitespace dropped.
    pub text: String,
}

/// A command a page wants run in a real terminal rather than captured.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpawnRequest {
    /// Arguments after the program name.
    pub args: Vec<String>,
    /// Directory to run in.
    pub cwd: PathBuf,
    /// The program.
    pub program: String,
}

/// What a page is asking the host to read from the user.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptRequest {
    /// Text the input starts out holding, for an answer that is usually an edit
    /// of what is already there.
    pub initial: String,
    /// What is being asked, shown before the input.
    pub label: String,
    /// Whether the answer is typed or a single yes-or-no key.
    pub mode: PromptMode,
    /// Which question this is, so the page knows what the answer belongs to.
    pub tag: &'static str,
}

/// The answer to a [`PromptRequest`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PromptReply {
    /// What the user typed, or `None` when the prompt was cancelled.
    pub answer: Option<String>,
    /// The tag of the question being answered.
    pub tag: &'static str,
}

/// How a prompt collects its answer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PromptMode {
    /// One key: `y` confirms, anything else cancels. For a question whose wrong
    /// answer cannot be undone.
    Confirm,
    /// A line of text, ending at `Enter`.
    #[default]
    Text,
}

/// A semantic style, resolved against the active theme so a page never names a
/// color of its own.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PageStyle {
    /// An added diff line: arriving, faintly.
    Added,
    /// The words an edit added: arriving, emphatically.
    AddedEdit,
    /// Emphasized, in the theme's accent color.
    Accent,
    /// A file a change added, where a view names what happened to it.
    ChangeAdded,
    /// A file a change left conflicted or unmerged.
    ChangeConflict,
    /// A file a change deleted.
    ChangeDeleted,
    /// A file a change edited in place.
    ChangeModified,
    /// A file a change moved or renamed.
    ChangeRenamed,
    /// De-emphasized: metadata, hints, and inactive detail.
    Dim,
    /// A title or a column header.
    Header,
    /// The heading over paths a merge left contested.
    HeadingConflict,
    /// The heading over changes that are staged.
    HeadingStaged,
    /// The heading over changes that are not staged.
    HeadingUnstaged,
    /// A heading over rows that are not a kind of change, and the labels of a
    /// header block's own lines, which read as headings of a sort.
    HeadingPlain,
    /// The heading over paths nothing tracks.
    HeadingUntracked,
    /// A hunk's header line: the band a hunk sits under.
    Hunk,
    /// Selected by the user for an operation to act on.
    Marked,
    /// Ordinary text.
    #[default]
    Normal,
    /// A removed diff line: leaving, faintly.
    Removed,
    /// The words an edit removed: leaving, emphatically.
    RemovedEdit,
    /// The checked-out branch, where a ref is drawn beside a commit.
    RefHead,
    /// A local branch other than the checked-out one, beside a commit.
    RefLocal,
    /// A branch on a remote, beside a commit.
    RefRemote,
    /// A tag, beside a commit.
    RefTag,
    /// Work one side of a branch pair has and the other does not: the marker
    /// on a commit the upstream has not seen, and the counts saying how far
    /// the two have drifted apart.
    Unpushed,
    /// A file's row in a diff: the band its changes sit under, bright while
    /// they show beneath it.
    Section,
    /// A file's row in a diff whose changes are folded away: the band, receded.
    SectionFolded,
    /// Source being read: a comment, standing back from the code it explains.
    SyntaxComment,
    /// Source being read: a word of the language rather than of the program.
    SyntaxKeyword,
    /// Source being read: a literal number.
    SyntaxNumber,
    /// Source being read: a literal string.
    SyntaxString,
    /// Source being read: the name of a type.
    SyntaxType,
}

/// What the host should do with a key the page was offered.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PageOutcome {
    /// Close the page, and the pane holding it.
    Close,
    /// The page acted on the key.
    Consumed,
    /// The page has no binding for it, so resolve it as an ordinary key.
    Ignored,
    /// The page asks the host to open this path with the system handler.
    OpenExternal(PathBuf),
    /// The page asks the host to stop the work it started.
    CancelJobs,
    /// The page asks the host to do slow work for it.
    Job(JobRequest),
    /// The page asks the host to read an answer from the user.
    Prompt(PromptRequest),
    /// The page asks the host to have the user choose from a list, rather
    /// than type. The answer comes back as a [`PromptReply`] under the same
    /// tag, so a question asked either way is answered in one place.
    Pick(PickRequest),
    /// The page asks the host to run one of its own commands, named the way the
    /// command palette names it.
    RunAction(String),
    /// The page asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The page asks the host to copy this to the clipboard.
    Yank(String),
    /// The page asks the host to open this file for editing, in the editor
    /// the app carries.
    OpenPath(OpenTarget),
    /// The page asks the host to open this file in `$EDITOR`, in a pane of
    /// its own, for the editing the app's own editor deliberately cannot do.
    SpawnEditor(OpenTarget),
    /// The page asks the host for what is on the clipboard, which comes back
    /// through [`Page::on_paste`].
    Paste,
}

/// Where a pointer is over a page: the row and column of the pane's own
/// content, counted from the first row the page painted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PagePoint {
    /// The column the pointer is over.
    pub col: usize,
    /// Whether the pointer is being dragged with its button held, rather than
    /// pressed where it now is.
    pub drag: bool,
    /// The row the pointer is over.
    pub row: usize,
}

/// Where a page's own caret sits within the row it bands as its cursor line,
/// and whether what it is doing there is typing. A page that reports one is
/// drawn with the caret on that cell, rather than on the row's first non-blank
/// character, which for a page with a gutter is in the gutter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PageCaret {
    /// The column, counted in painted cells from the row's start.
    pub col: usize,
    /// Whether the next key typed goes into the text, which the caret takes
    /// its shape from the way the terminal's own does.
    pub insert: bool,
}

/// One entry a page offers in the menu opened over its own rows.
///
/// An entry names a key the page already binds, rather than a command of its
/// own. A menu built that way cannot offer something the keyboard cannot do,
/// cannot drift from what the keys mean, and costs a page one list rather
/// than a second dispatch path beside `on_key`.
pub struct PageMenuItem {
    /// The key choosing this entry stands for.
    pub key: Key,
    /// What the host shows for it.
    pub label: String,
}

/// A WebView a page owns, covering the pane below whatever rows the page
/// paints itself.
///
/// A page returns one when its content is something Winter cannot draw as
/// styled text: a rendered PDF page, for instance. The host gives it a real
/// web engine over the pane's pixels, and goes on painting the page's own
/// rows above it, so the header and the key hints stay in the terminal's font
/// and theme while the document is drawn by the engine.
pub struct PageSurface {
    /// Resolves a path under the surface's own asset root to the bytes to
    /// serve for it. A plain `fn` rather than a closure so the host can hand
    /// it to a protocol handler that outlives this call.
    pub assets: fn(&str) -> Option<SurfaceAsset>,
    /// A file the surface is allowed to read, served to it as the document.
    /// Nothing else on disk is reachable from inside the surface.
    pub document: PathBuf,
    /// The document to load, as a path under the surface's asset root.
    pub entry: String,
    /// The first pane row the surface covers. Rows above it are painted as
    /// ordinary page rows, which is how a page keeps a native header over a
    /// surface it does not draw.
    pub top_row: usize,
}

/// One asset a [`PageSurface`] serves to its own WebView.
pub struct SurfaceAsset {
    /// The file's contents.
    pub bytes: Vec<u8>,
    /// The `Content-Type` to serve it under. A web engine refuses to run a
    /// module script sent as anything but a JavaScript type.
    pub mime: &'static str,
}

/// A file a page wants opened, and where in it to land.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenTarget {
    /// The line to put the cursor on, when the page knows one.
    pub line: Option<usize>,
    /// The file to open.
    pub path: PathBuf,
}

// ========================================================================
// Functions
// ========================================================================

/// The slice of `total` rows to show so that `cursor` stays visible, given a
/// pane that can draw `rows` of them below `header` pinned rows. `scroll` is
/// the current first visible row, and is moved the least amount that reveals
/// the cursor, so paging down a long listing does not jump the view around.
pub fn scroll_to_cursor(scroll: usize, cursor: usize, total: usize, visible: usize) -> usize {
    if visible == 0 {
        return 0;
    }
    let last_start = total.saturating_sub(visible);
    let mut start = scroll.min(last_start);
    if cursor < start {
        start = cursor;
    } else if cursor >= start + visible {
        start = cursor + 1 - visible;
    }
    start.min(last_start)
}

/// The index of the next label holding `query`, ignoring case, looking one past
/// `from` and wrapping once so the row under the cursor is tried last. Matching
/// is literal text, which keeps a page's search free of a pattern engine.
pub fn find_match(
    labels: &[impl AsRef<str>],
    query: &str,
    from: usize,
    forward: bool,
) -> Option<usize> {
    let total = labels.len();
    if query.is_empty() || total == 0 {
        return None;
    }
    let needle = query.to_lowercase();
    (1..=total)
        .map(|step| {
            if forward {
                (from + step) % total
            } else {
                (from + total - step) % total
            }
        })
        .find(|&index| labels[index].as_ref().to_lowercase().contains(&needle))
}

/// What a row puts on screen, with its styles dropped: the text a search reads.
pub fn row_text(row: &PageRow) -> String {
    row.iter().map(|span| span.text.as_str()).collect()
}

/// The column a wrapped row's continuation starts at, clamped so at least one
/// column of text remains however narrow the pane or wide the indent.
pub fn wrap_start(indent: usize, cols: usize) -> usize {
    indent.min(cols.saturating_sub(1))
}

/// The half-open char ranges a row's text paints as, one per screen row,
/// wrapped to a pane `cols` wide. A fold lands before the last word that
/// starts inside the line, and only when the part of it overflowing the
/// margin would fit beside the continuation's indent — the same choice the
/// terminal grid's own word wrap makes — so words move down whole, a diff
/// marker keeps its line, and only a word with no room at all breaks
/// mid-word. Continuations after the first start at the row's wrap indent
/// (clamped by [`wrap_start`], so every line holds at least one column and
/// wrapping always terminates). With `wrap` off, the whole text is one line
/// whatever the pane's width.
pub fn wrapped_lines(
    chars: &[char],
    cols: usize,
    wrap: bool,
    indent: usize,
) -> Vec<(usize, usize)> {
    if !wrap || cols == 0 || chars.len() <= cols {
        return vec![(0, chars.len())];
    }
    let cont = wrap_start(indent, cols);
    let mut lines = Vec::new();
    let mut pos = 0;
    let mut room = cols;
    while pos < chars.len() {
        let end = if chars.len() - pos <= room {
            chars.len()
        } else {
            // The line must fold. The fold lands before the last word that
            // starts inside the line, and only when the part overflowing the
            // margin fits beside the continuation's indent — the same choice
            // the grid's own word wrap makes, so a diff marker keeps its
            // place and a word too long for the indent stays split at the
            // margin. Leading whitespace is never a break. With no word
            // boundary in reach, the line breaks exactly at the margin, so
            // wrapping always terminates.
            let limit = pos + room;
            let mut end = limit;
            for w in (pos + 1..=limit).rev() {
                if !chars[w - 1].is_whitespace() || chars[w].is_whitespace() {
                    continue;
                }
                if limit - w <= cols - cont {
                    end = w;
                    break;
                }
            }
            end
        };
        lines.push((pos, end));
        if end == chars.len() {
            break;
        }
        pos = end;
        while pos < chars.len() && chars[pos].is_whitespace() {
            pos += 1;
        }
        room = cols - cont;
    }
    lines
}

/// How many screen rows a painted row takes in a pane `cols` wide: one,
/// unless it wraps, in which case one per line it spills onto — the first
/// filling the pane, the rest starting at the row's wrap indent and so
/// holding less. Counts the lines [`wrapped_lines`] paints, so the height a
/// page measures its rows by is the height they paint at.
pub fn row_height(text: &str, cols: usize, wrap: bool, indent: usize) -> usize {
    if !wrap || cols == 0 || text.chars().count() <= cols {
        return 1;
    }
    let chars: Vec<char> = text.chars().collect();
    wrapped_lines(&chars, cols, wrap, indent).len()
}

/// The window of a page's rows to paint in a pane, so the cursor stays
/// visible: the same cursor-following window [`scroll_to_cursor`] gives, but
/// measured in screen rows, since a row wider than the pane wraps onto
/// several of them and leaves room for fewer of the page's own rows. Each
/// row's height is read from `height`, which decides how a row wider than the
/// pane is counted, and only for the rows the walk visits — so a page whose
/// rows are built as they are painted is not asked to build them all. With
/// every row one screen row this is the plain window.
pub fn wrap_window(
    scroll: usize,
    cursor: usize,
    total: usize,
    screen_rows: usize,
    height: impl Fn(usize) -> usize,
) -> PageWindow {
    // The logical clamp first: it lands jumps (a long way to the bottom, back
    // to the top) with the cursor inside a window of plain rows, without
    // measuring every row to get there.
    let mut start = scroll_to_cursor(scroll, cursor, total, screen_rows);
    // Enough rows from `start` to fill the pane, counting wrapped heights.
    let fill_from = |start: usize| {
        let mut filled = 0;
        let mut count = 0;
        for index in start..total {
            filled += height(index);
            count += 1;
            if filled >= screen_rows {
                break;
            }
        }
        count
    };
    let mut count = fill_from(start);
    // Wrapping shrinks the window below the cursor the logical clamp put
    // inside it, so when it did, the cursor's own row goes to the top — the
    // one place it is certainly on screen, even if its own tail wraps past
    // the pane's bottom.
    if !(start..start + count).contains(&cursor) {
        start = cursor.min(total.saturating_sub(1));
        count = fill_from(start);
    }
    // The cursor's screen row, counted from the first painted row.
    let mut cursor_row = 0;
    for index in start..cursor.min(start + count) {
        cursor_row += height(index);
    }
    PageWindow {
        count,
        cursor: cursor_row,
        start,
    }
}

// ========================================================================
// Traits
// ========================================================================

/// Non-terminal pane content. A page paints itself as rows of styled text and
/// gets first refusal on keys while its pane is focused.
pub trait Page {
    /// A short label for the pane title.
    fn title(&self) -> String;

    /// The rows to draw this frame, given how many the pane can show below any
    /// rows it pins, and how wide the pane is: a page longer than its pane
    /// returns the window it wants visible, so scrolling stays where the
    /// cursor is, and when `wrap` is set a row wider than `cols` wraps onto
    /// the next screen row — folding at its last word boundary — rather than
    /// clipping at the pane's edge.
    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent;

    /// Offer a key to the page.
    fn on_key(&mut self, key: &Key) -> PageOutcome;

    /// Offer a press or a drag of the pointer to the page. Pages that take no
    /// pointer never implement it, and the click falls through to the host.
    fn on_mouse(&mut self, _at: PagePoint) -> PageOutcome {
        PageOutcome::Ignored
    }

    /// Offer a turn of the wheel to the page, in rows, positive upwards.
    /// Pages that do not scroll themselves never implement it.
    fn on_scroll(&mut self, _lines: isize) -> PageOutcome {
        PageOutcome::Ignored
    }

    /// Hand back what the clipboard holds, for a page that asked for it.
    fn on_paste(&mut self, _text: String) -> PageOutcome {
        PageOutcome::Consumed
    }

    /// Offer a file to the page, for a page that can show more than one.
    /// `false`, the default, means the host opens it however it would have.
    fn open_file(&mut self, _target: OpenTarget) -> bool {
        false
    }

    /// What the page is waiting for, when it is part-way through a command
    /// that takes more than one key: the sequence's name and what the next
    /// key may be. The host draws it the way it draws its own key hints, so a
    /// multi-key command looks the same wherever it is being typed. Pages
    /// with no multi-key commands never implement it.
    fn hint(&self) -> Option<PageHint> {
        None
    }

    /// Hand back the answer to a prompt the page asked for. Pages that never
    /// ask never implement it.
    fn on_prompt(&mut self, _reply: PromptReply) -> PageOutcome {
        PageOutcome::Consumed
    }

    /// Hand back the result of work the page asked for.
    fn on_job(&mut self, _reply: JobReply) -> PageOutcome {
        PageOutcome::Consumed
    }

    /// Where the page's own caret is on its cursor line, for a page that puts
    /// one somewhere other than the first thing it painted. A page whose
    /// cursor is a whole row reports none.
    fn caret(&self) -> Option<PageCaret> {
        None
    }

    /// Where a tool or shell opened from this page starts: the directory it
    /// is looking at, following the cursor rather than its root. Pages tied
    /// to nowhere on disk never implement it.
    fn cwd(&self) -> Option<PathBuf> {
        None
    }

    /// The page is showing again after another one was closed over the top of
    /// it. What it was showing may have changed while it was covered, so a
    /// page that reads the world re-reads it here. Pages showing something
    /// that cannot go stale never implement it.
    fn on_resume(&mut self) -> PageOutcome {
        PageOutcome::Consumed
    }

    /// The WebView this page owns, if its content is something Winter cannot
    /// paint as text. Pages that draw themselves in rows never implement it.
    fn surface(&self) -> Option<PageSurface> {
        None
    }

    /// Script the page wants run inside its surface, taken and cleared: this
    /// is how a key the page bound reaches the document the engine is
    /// drawing. Pages with no surface never implement it.
    fn take_surface_script(&mut self) -> Option<String> {
        None
    }

    /// Hand back what the page's surface posted out of the engine, which is
    /// how a surface reports state (the page it scrolled to, how many there
    /// are) that only it knows.
    fn on_surface_message(&mut self, _message: String) -> PageOutcome {
        PageOutcome::Consumed
    }

    /// What to offer in a menu opened over the page's rows, for the row the
    /// cursor is on: the host puts the cursor under the pointer before
    /// asking, so a page answers for what was clicked.
    ///
    /// An empty list, the default, means the page offers no menu and the
    /// click does nothing. Pages whose rows all mean the same thing never
    /// implement it, and neither does a page drawn by a [`PageSurface`]: the
    /// child WebView takes the click before the window sees it, and the menu
    /// is painted by the GPU underneath a native view that is always on top
    /// of it.
    fn context_items(&self) -> Vec<PageMenuItem> {
        Vec::new()
    }
}

// ========================================================================
// PageContent
// ========================================================================

impl PageContent {
    /// A page of `rows` with no cursor line.
    pub fn new(rows: Vec<PageRow>) -> Self {
        Self {
            cursor_line: None,
            icons: Vec::new(),
            rows,
            wrap_indents: Vec::new(),
        }
    }

    /// Attach the icons the host should draw over these rows.
    pub fn with_icons(mut self, icons: Vec<PageIcon>) -> Self {
        self.icons = icons;
        self
    }

    /// Attach the column each row's wrapped continuation starts at, one per
    /// row.
    pub fn with_wrap_indents(mut self, wrap_indents: Vec<usize>) -> Self {
        self.wrap_indents = wrap_indents;
        self
    }

    /// Band `row` as the cursor line, counted from the first returned row.
    pub fn with_cursor_line(mut self, row: usize) -> Self {
        self.cursor_line = Some(row);
        self
    }
}

// ========================================================================
// OpenTarget
// ========================================================================

impl OpenTarget {
    /// Open `path`, leaving it to the editor where to start.
    pub fn file(path: PathBuf) -> Self {
        Self { line: None, path }
    }

    /// Open `path` with the cursor on `line`, counting from one.
    pub fn at_line(path: PathBuf, line: usize) -> Self {
        Self {
            line: Some(line),
            path,
        }
    }
}

// ========================================================================
// PageSpan
// ========================================================================

impl PageMenuItem {
    /// An entry called `label`, run by `key`.
    pub fn new(key: Key, label: impl Into<String>) -> Self {
        Self {
            key,
            label: label.into(),
        }
    }
}

impl PageSpan {
    /// A run drawn in `style`.
    pub fn new(style: PageStyle, text: impl Into<String>) -> Self {
        Self {
            style,
            text: text.into(),
        }
    }

    /// A run drawn in the page's ordinary text style.
    pub fn plain(text: impl Into<String>) -> Self {
        Self::new(PageStyle::Normal, text)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_cursor_already_in_view_does_not_move_the_window() {
        assert_eq!(scroll_to_cursor(10, 15, 100, 20), 10);
    }

    #[test]
    fn test_the_window_follows_the_cursor_by_the_least_it_can() {
        // Stepping one row past the bottom must scroll one row, not a page.
        assert_eq!(scroll_to_cursor(0, 20, 100, 20), 1);
        assert_eq!(scroll_to_cursor(10, 9, 100, 20), 9);
    }

    #[test]
    fn test_the_window_never_scrolls_past_the_last_row() {
        // Otherwise a listing that shrank under a scrolled view (a reload after
        // deleting files) would paint a screen of blank rows.
        assert_eq!(scroll_to_cursor(90, 5, 10, 20), 0);
        assert_eq!(scroll_to_cursor(0, 99, 100, 20), 80);
    }

    #[test]
    fn test_a_pane_with_no_room_shows_the_first_row() {
        assert_eq!(scroll_to_cursor(5, 9, 100, 0), 0);
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
    fn test_a_search_ignores_case_on_both_sides() {
        assert_eq!(find_match(&["Cargo.toml"], "cargo", 0, true), Some(0));
        assert_eq!(find_match(&["cargo.toml"], "CARGO", 0, true), Some(0));
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
    fn test_without_wrapping_the_window_is_one_row_per_row() {
        // Every row one screen row: the window is the plain cursor-following
        // one, and the count is the pane's rows.
        let window = wrap_window(10, 15, 100, 20, |_| 1);
        assert_eq!(
            window,
            PageWindow {
                count: 20,
                cursor: 5,
                start: 10
            }
        );
    }

    #[test]
    fn test_an_indented_wrap_holds_less_per_continuation_line() {
        // A 17-cell row of one unbreakable word in a 10-cell pane: without an
        // indent it spills seven cells onto a second line; with the
        // continuation starting five cells in, that line holds only five, so
        // the row takes three.
        assert_eq!(row_height(&"x".repeat(17), 10, true, 0), 2);
        assert_eq!(row_height(&"x".repeat(17), 10, true, 5), 3);
    }

    #[test]
    fn test_a_wrap_indent_never_leaves_no_room_for_text() {
        // An indent as wide as the pane is clamped to leave one column of
        // text, so wrapping still terminates.
        assert_eq!(wrap_start(10, 10), 9);
        assert_eq!(wrap_start(4, 10), 4);
        assert_eq!(row_height(&"x".repeat(12), 10, true, 10), 3);
    }

    #[test]
    fn test_a_row_folds_at_its_last_space_that_fits() {
        // "aa bb ccc" in eight cells: the fold lands before "ccc", the last
        // word that fits to start a line, so the first line holds "aa bb" and
        // carries "ccc" down whole rather than splitting it across the margin.
        let chars: Vec<char> = "aa bb ccc".chars().collect();
        assert_eq!(wrapped_lines(&chars, 8, true, 0), vec![(0, 6), (6, 9)]);
        assert_eq!(row_height("aa bb ccc", 8, true, 0), 2);
    }

    #[test]
    fn test_a_word_too_long_for_the_indent_stays_at_the_margin() {
        // The grid's own word wrap carries a word down only when the part
        // overflowing the margin fits beside the continuation's indent:
        // "-oldoldoldold" beside a five-cell indent does not, so the fold
        // falls back to the margin and the marker keeps its line.
        let chars: Vec<char> = "    -oldoldoldold".chars().collect();
        assert_eq!(
            wrapped_lines(&chars, 10, true, 5),
            vec![(0, 10), (10, 15), (15, 17)]
        );
    }

    #[test]
    fn test_a_word_longer_than_a_line_breaks_at_the_margin() {
        // No space in reach: the word breaks mid-word exactly at the margin,
        // the way the terminal grid's own wrap falls back to.
        let chars: Vec<char> = "aaaaaa".chars().collect();
        assert_eq!(wrapped_lines(&chars, 4, true, 0), vec![(0, 4), (4, 6)]);
    }

    #[test]
    fn test_the_fold_lands_before_the_word_not_on_the_spaces() {
        // A fold never starts a continuation on whitespace: the spaces end
        // the line they fold at, and a row ending in spaces does not spill
        // them onto a line of their own.
        let chars: Vec<char> = "a  b".chars().collect();
        assert_eq!(wrapped_lines(&chars, 1, true, 0), vec![(0, 1), (3, 4)]);
        let trailing: Vec<char> = "word ".chars().collect();
        assert_eq!(wrapped_lines(&trailing, 4, true, 0), vec![(0, 4)]);
    }

    #[test]
    fn test_wrapping_off_keeps_one_line_however_wide() {
        let chars: Vec<char> = "aaa bbb ccc".chars().collect();
        assert_eq!(wrapped_lines(&chars, 4, false, 0), vec![(0, 11)]);
        assert_eq!(row_height("aaa bbb ccc", 4, false, 0), 1);
    }

    #[test]
    fn test_a_wrapped_row_pushes_the_cursor_down_a_screen_row() {
        // A row five panes wide takes five screen rows, so the row under it
        // paints on screen row five, not one.
        let heights = [5, 1, 1, 1];
        let window = wrap_window(0, 1, heights.len(), 10, |index| heights[index]);
        assert_eq!(window.cursor, 5);
        // Four rows heights 5+1+1+1 fill eight of ten screen rows.
        assert_eq!(window.count, 4);
    }

    #[test]
    fn test_a_cursor_squeezed_out_by_wrapping_goes_to_the_top() {
        // Rows tall enough that the filled window ends before the cursor:
        // the cursor's own row becomes the first painted one, where it is
        // certainly on screen.
        let heights = [4, 4, 4, 4, 4];
        let window = wrap_window(0, 4, heights.len(), 10, |index| heights[index]);
        assert_eq!(window.start, 4);
        assert_eq!(window.cursor, 0);
    }

    #[test]
    fn test_wrapping_fits_fewer_rows_than_the_pane_is_tall() {
        // Ten screen rows of two-row-tall rows hold five of the page's rows.
        let heights = [2; 30];
        let window = wrap_window(0, 0, heights.len(), 10, |index| heights[index]);
        assert_eq!(window.count, 5);
    }

    #[test]
    fn test_an_empty_page_windows_to_nothing() {
        let window = wrap_window(3, 0, 0, 10, |_| 1);
        assert_eq!(
            window,
            PageWindow {
                count: 0,
                cursor: 0,
                start: 0
            }
        );
    }
}

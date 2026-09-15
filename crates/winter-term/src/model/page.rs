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
    /// Emphasized, in the theme's accent color.
    Accent,
    /// De-emphasized: metadata, hints, and inactive detail.
    Dim,
    /// A title or a column header.
    Header,
    /// Selected by the user for an operation to act on.
    Marked,
    /// Ordinary text.
    #[default]
    Normal,
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
    /// The page asks the host to run one of its own commands, named the way the
    /// command palette names it.
    RunAction(String),
    /// The page asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The page asks the host to copy this to the clipboard.
    Yank(String),
    /// The page asks the host to open this file for editing.
    OpenPath(OpenTarget),
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

// ========================================================================
// Traits
// ========================================================================

/// Non-terminal pane content. A page paints itself as rows of styled text and
/// gets first refusal on keys while its pane is focused.
pub trait Page {
    /// A short label for the pane title.
    fn title(&self) -> String;

    /// The rows to draw this frame, given how many the pane can show. A page
    /// longer than its pane returns the window it wants visible, so scrolling
    /// stays where the cursor is.
    fn content(&mut self, rows: usize) -> PageContent;

    /// Offer a key to the page.
    fn on_key(&mut self, key: &Key) -> PageOutcome;

    /// Hand back the answer to a prompt the page asked for. Pages that never
    /// ask never implement it.
    fn on_prompt(&mut self, _reply: PromptReply) -> PageOutcome {
        PageOutcome::Consumed
    }

    /// Hand back the result of work the page asked for.
    fn on_job(&mut self, _reply: JobReply) -> PageOutcome {
        PageOutcome::Consumed
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
        }
    }

    /// Attach the icons the host should draw over these rows.
    pub fn with_icons(mut self, icons: Vec<PageIcon>) -> Self {
        self.icons = icons;
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
}

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
    /// Every line of the page.
    pub rows: Vec<PageRow>,
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
    /// The page asks the host to run a command in a pane of its own, for work
    /// that needs a terminal: an editor, or anything that prompts.
    Spawn(SpawnRequest),
    /// The page asks the host to copy this to the clipboard.
    Yank(String),
    /// The page asks the host to open this path for editing.
    OpenPath(PathBuf),
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
            rows,
        }
    }

    /// Band `row` as the cursor line, counted from the first returned row.
    pub fn with_cursor_line(mut self, row: usize) -> Self {
        self.cursor_line = Some(row);
        self
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
}

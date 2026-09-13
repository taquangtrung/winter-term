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
    /// The page asks the host to read an answer from the user.
    Prompt(PromptRequest),
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

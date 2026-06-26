//! Notifications: everything the window has said this session, newest first,
//! in a tab of its own. The window hands the history over whenever it
//! changes, so the list stays current while the tab is up.

use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::Key;
use crate::model::notify::{Notification, Severity};
use crate::model::paint::{row_height, row_text, Frame, Point, Row, Span, Style};
use crate::model::tool::{
    menu_groups, Notice, PromptMode, PromptReply, PromptRequest, SearchReport, Tool, ToolMenuItem,
    ToolOutcome, ToolStatus,
};
use crate::model::vim::nav::{VimKey, VimNav};
use crate::tools::list::ListView;
use crate::tools::reltime;

// ========================================================================
// Constants
// ========================================================================

/// The tool's title.
const TITLE: &str = "Notifications";

/// Rows of header above the first notification.
const HEADER_ROWS: usize = 1;

/// The one question the tool asks.
const ASK_SEARCH: &str = "notifications-search";

/// The window command that empties the history, which the window keeps.
const CLEAR_ACTION: &str = "clear_notifications";

/// What the list says with nothing in it.
const EMPTY: &str = "No notifications";

/// The width the age and the severity are padded to, so the messages line
/// up.
const AGE_WIDTH: usize = 9;
const SEVERITY_WIDTH: usize = 8;

/// The tool's keys, ahead of the motions every list shares.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Copy,
        keys: &["y", "RET"],
        label: "Copy the Message",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Find",
    },
    Binding {
        action: Act::SearchStep(true),
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Act::SearchStep(false),
        keys: &["N"],
        label: "Previous Match",
    },
    Binding {
        action: Act::Clear,
        keys: &["C"],
        label: "Clear the History",
    },
    Binding {
        action: Act::Close,
        keys: &["q"],
        label: "Close",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the tool's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Clear,
    Close,
    Copy,
    Search,
    /// The search again, forward when set.
    SearchStep(bool),
}

/// The window's notification history, as a list to read and copy from.
pub struct NotificationsTool {
    entries: Vec<Notification>,
    list: ListView<usize>,
    nav: VimNav,
    notice: Option<Notice>,
    /// When the history was last handed over, in seconds since the Unix
    /// epoch, which each age is reckoned from.
    now: i64,
}

// ========================================================================
// NotificationsTool
// ========================================================================

impl NotificationsTool {
    /// The tool over `entries`, newest first, their ages reckoned at `now`.
    pub fn new(entries: Vec<Notification>, now: i64) -> Self {
        let mut tool = Self {
            entries: Vec::new(),
            list: ListView::new(),
            nav: VimNav::new(),
            notice: None,
            now,
        };
        tool.set_entries(entries, now);
        tool
    }

    /// Show `entries` instead, keeping the cursor on the notification it
    /// was on where that is still listed.
    pub fn set_entries(&mut self, entries: Vec<Notification>, now: i64) {
        // A notification is known by its position from the oldest, which a
        // newer one arriving at the top does not change.
        let keys = (0..entries.len()).rev().collect();
        self.entries = entries;
        self.now = now;
        self.list.set_keys(keys);
    }

    /// The notification under the cursor.
    fn selected(&self) -> Option<&Notification> {
        let from_oldest = *self.list.selected()?;
        self.entries
            .len()
            .checked_sub(from_oldest + 1)
            .and_then(|index| self.entries.get(index))
    }

    /// The row a notification is listed as: how long ago, how much it
    /// matters, then what was said, by whom, and how many times.
    fn row(&self, entry: &Notification) -> Row {
        let age = format!("{:<AGE_WIDTH$}", reltime::ago(entry.at, self.now));
        let severity = format!("{:<SEVERITY_WIDTH$}", entry.severity.name());
        let severity_style = match entry.severity {
            Severity::Error => Style::ChangeConflict,
            Severity::Warning => Style::ChangeModified,
            _ => Style::Dim,
        };
        vec![
            Span::new(Style::Dim, age),
            Span::new(severity_style, severity),
            Span::plain(entry.message()),
        ]
    }

    /// The header: the title and how many notifications there are.
    fn header_row(&self) -> Row {
        vec![
            Span::new(Style::HeaderAccent, TITLE),
            Span::new(Style::Dim, format!("  {}", self.entries.len())),
        ]
    }

    /// Each row's text, for the search to read.
    fn search_labels(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| row_text(&self.row(entry)))
            .collect()
    }

    /// Move to the next notification holding `query`.
    fn search_for(&mut self, query: &str) -> ToolOutcome {
        let labels = self.search_labels();
        self.notice = self.list.search_for(&labels, query.trim());
        ToolOutcome::Consumed
    }

    /// Do what a key of the tool's own asks.
    fn run(&mut self, act: Act) -> ToolOutcome {
        match act {
            Act::Clear => ToolOutcome::RunAction(CLEAR_ACTION.to_string()),
            Act::Close => ToolOutcome::Close,
            Act::Copy => match self.selected() {
                Some(entry) => ToolOutcome::Yank(entry.text.clone()),
                None => ToolOutcome::Consumed,
            },
            Act::Search => ToolOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: "/".to_string(),
                mode: PromptMode::Search { reverse: false },
                tag: ASK_SEARCH,
            }),
            Act::SearchStep(forward) => {
                let labels = self.search_labels();
                self.notice = self.list.search_step(&labels, forward);
                ToolOutcome::Consumed
            }
        }
    }
}

impl Tool for NotificationsTool {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let mut frame_rows = vec![self.header_row()];
        if self.entries.is_empty() {
            frame_rows.push(vec![Span::new(Style::Dim, EMPTY)]);
            return Frame::new(frame_rows);
        }
        let painted: Vec<Row> = self.entries.iter().map(|entry| self.row(entry)).collect();
        let texts: Vec<String> = painted.iter().map(row_text).collect();
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.list.view_mut().set_rows(visible);
        let placed = self
            .list
            .place(visible, |index| row_height(&texts[index], cols, wrap, 0));
        let window = placed.window;
        frame_rows.extend(painted.into_iter().skip(window.start).take(window.count));
        let content = Frame::new(frame_rows);
        match placed.shows_cursor {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn status(&self) -> ToolStatus {
        ToolStatus {
            mode: None,
            segments: vec![format!("{} notifications", self.entries.len())],
        }
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence() {
            if let Some(act) = table::find(KEYS, key) {
                return self.run(act);
            }
        }
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.list.apply_motion(motion);
                ToolOutcome::Consumed
            }
            VimKey::Pending => ToolOutcome::Consumed,
            VimKey::Unhandled => ToolOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        let total = self.entries.len();
        self.list.view_mut().wheel(lines, total);
        ToolOutcome::Consumed
    }

    /// A press on a notification puts the cursor on it.
    fn on_mouse(&mut self, at: Point) -> ToolOutcome {
        if at.drag || at.row < HEADER_ROWS {
            return ToolOutcome::Ignored;
        }
        self.list.view_mut().release();
        let at = self.list.view().scroll() + at.row - HEADER_ROWS;
        if at < self.entries.len() {
            self.list.set_cursor(at);
        }
        ToolOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        match (reply.tag, reply.answer) {
            (ASK_SEARCH, Some(query)) => self.search_for(&query),
            _ => ToolOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> ToolOutcome {
        if tag == ASK_SEARCH && !query.trim().is_empty() {
            return self.search_for(query);
        }
        ToolOutcome::Consumed
    }

    fn set_notifications(&mut self, entries: &[Notification], now: i64) {
        self.set_entries(entries.to_vec(), now);
    }

    /// Copy the message under the pointer, or clear the history, or close.
    fn context_items(&self) -> Vec<ToolMenuItem> {
        let entry = |act| {
            table::menu_entry(KEYS, act).map(|(key, label)| ToolMenuItem::new(key, label))
        };
        let copy = match self.selected() {
            Some(_) => entry(Act::Copy).into_iter().collect(),
            None => Vec::new(),
        };
        menu_groups([
            copy,
            entry(Act::Search).into_iter().collect(),
            entry(Act::Clear).into_iter().collect(),
            entry(Act::Close).into_iter().collect(),
        ])
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::notify::NotificationLog;

    /// A history of `notices`, oldest first, as the log lists it.
    fn history(notices: &[Notice]) -> Vec<Notification> {
        let mut log = NotificationLog::default();
        for (at, notice) in notices.iter().enumerate() {
            log.record(notice, at as i64);
        }
        log.iter().cloned().collect()
    }

    fn texts(frame: &Frame) -> Vec<String> {
        frame.rows.iter().map(row_text).collect()
    }

    #[test]
    fn test_the_newest_is_listed_first_and_copied_from_the_cursor() {
        let entries = history(&[Notice::info("first"), Notice::error("second")]);
        let mut tool = NotificationsTool::new(entries, 10);
        let frame = tool.content(10, 60, false);
        let rows = texts(&frame);
        assert!(rows[1].contains("error") && rows[1].contains("second"), "{rows:?}");
        assert!(rows[2].contains("first"), "{rows:?}");
        assert_eq!(
            tool.on_key(&Key::plain(crate::model::input::KeyCode::Char('y'))),
            ToolOutcome::Yank("second".to_string())
        );
    }

    #[test]
    fn test_a_new_notification_keeps_the_cursor_on_the_one_it_was_on() {
        let mut tool = NotificationsTool::new(history(&[Notice::info("a"), Notice::info("b")]), 5);
        tool.list.set_cursor(1);
        assert_eq!(tool.selected().map(|entry| entry.text.as_str()), Some("a"));
        let newer = history(&[Notice::info("a"), Notice::info("b"), Notice::info("c")]);
        tool.set_notifications(&newer, 6);
        assert_eq!(tool.selected().map(|entry| entry.text.as_str()), Some("a"));
    }

    #[test]
    fn test_an_empty_history_says_so() {
        let mut tool = NotificationsTool::new(Vec::new(), 0);
        assert!(texts(&tool.content(5, 40, false)).contains(&EMPTY.to_string()));
    }

    #[test]
    fn test_clearing_asks_the_window_which_keeps_the_history() {
        let mut tool = NotificationsTool::new(history(&[Notice::info("a")]), 0);
        assert_eq!(
            tool.on_key(&Key::plain(crate::model::input::KeyCode::Char('C'))),
            ToolOutcome::RunAction(CLEAR_ACTION.to_string())
        );
    }
}

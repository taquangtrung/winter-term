//! Notifications: everything the window has said this session, newest first,
//! in a tab of its own. The window hands the history over whenever it
//! changes, so the list stays current while the tab is up.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::editors::list::ListView;
use crate::editors::reltime;
use crate::model::editor::{
    menu_groups, Editor, EditorMenuItem, EditorOutcome, EditorStatus, Notice, PromptMode,
    PromptReply, PromptRequest, SearchReport,
};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::Key;
use crate::model::notify::{Notification, Severity};
use crate::model::paint::{row_height, row_text, Frame, Point, Row, Span, Style};
use crate::model::vim::nav::{VimKey, VimNav};

// ========================================================================
// Constants
// ========================================================================

/// The editor's title.
const TITLE: &str = "Notifications";

/// The one question the editor asks.
const ASK_SEARCH: &str = "notifications-search";

/// The window command that empties the history, which the window keeps.
const CLEAR_ACTION: &str = "clear_notifications";

/// What the list says with nothing in it.
const EMPTY: &str = "No notifications";

/// The codicons a notification leads with, by how much it matters: the
/// same ones its toast card shows.
const ERROR_ICON: &str = "\u{ea87}";
const WARNING_ICON: &str = "\u{ea6c}";
const INFO_ICON: &str = "\u{ea74}";

/// The least space kept between a message and its age.
const AGE_GAP: usize = 2;

/// What ends a message cut short to leave room for its age.
const ELLIPSIS: char = '\u{2026}';

/// The editor's keys, ahead of the motions every list shares.
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

/// What a key of the editor's own does.
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
pub struct NotificationsEditor {
    entries: Vec<Notification>,
    list: ListView<usize>,
    nav: VimNav,
    notice: Option<Notice>,
    /// When the history was last handed over, in seconds since the Unix
    /// epoch, which each age is reckoned from.
    now: i64,
}

// ========================================================================
// NotificationsEditor
// ========================================================================

impl NotificationsEditor {
    /// The editor over `entries`, newest first, their ages reckoned at `now`.
    pub fn new(entries: Vec<Notification>, now: i64) -> Self {
        let mut editor = Self {
            entries: Vec::new(),
            list: ListView::new(),
            nav: VimNav::new(),
            notice: None,
            now,
        };
        editor.set_entries(entries, now);
        editor
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

    /// The row a notification is listed as in `cols` columns: an icon for
    /// how much it matters, then what was said, by whom, and how many times,
    /// at the left, and how long ago at the right edge. A message too long
    /// for the row is cut short, or with `wrap` folds onto the rows below
    /// with its age after it.
    fn row(&self, entry: &Notification, cols: usize, wrap: bool) -> Row {
        let age = reltime::ago(entry.at, self.now);
        let message = entry.message();
        let (icon, style) = match entry.severity {
            Severity::Error => (ERROR_ICON, Style::ChangeConflict),
            Severity::Warning => (WARNING_ICON, Style::ChangeModified),
            _ => (INFO_ICON, Style::Dim),
        };
        let icon = format!("{icon} ");
        let room = cols.saturating_sub(icon.width() + age.width() + AGE_GAP);
        if wrap && message.width() > room {
            let gap = " ".repeat(AGE_GAP);
            return vec![
                Span::new(style, icon),
                Span::plain(message),
                Span::new(Style::Dim, format!("{gap}{age}")),
            ];
        }
        let message = fit(&message, room);
        let pad = cols.saturating_sub(icon.width() + message.width() + age.width());
        vec![
            Span::new(style, icon),
            Span::plain(message),
            Span::plain(" ".repeat(pad.max(AGE_GAP))),
            Span::new(Style::Dim, age),
        ]
    }

    /// Each row's text, for the search to read: the whole message, however
    /// the row cuts it, then its age.
    fn search_labels(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| format!("{}  {}", entry.message(), reltime::ago(entry.at, self.now)))
            .collect()
    }

    /// Move to the next notification holding `query`.
    fn search_for(&mut self, query: &str) -> EditorOutcome {
        let labels = self.search_labels();
        self.notice = self.list.search_for(&labels, query.trim());
        EditorOutcome::Consumed
    }

    /// Do what a key of the editor's own asks.
    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::Clear => EditorOutcome::RunAction(CLEAR_ACTION.to_string()),
            Act::Close => EditorOutcome::Close,
            Act::Copy => match self.selected() {
                Some(entry) => EditorOutcome::Yank(entry.text.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::Search => EditorOutcome::Prompt(PromptRequest {
                initial: String::new(),
                label: "/".to_string(),
                mode: PromptMode::Search { reverse: false },
                tag: ASK_SEARCH,
            }),
            Act::SearchStep(forward) => {
                let labels = self.search_labels();
                self.notice = self.list.search_step(&labels, forward);
                EditorOutcome::Consumed
            }
        }
    }
}

impl Editor for NotificationsEditor {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        if self.entries.is_empty() {
            return Frame::new(vec![vec![Span::new(Style::Dim, EMPTY)]]);
        }
        let painted: Vec<Row> = self
            .entries
            .iter()
            .map(|entry| self.row(entry, cols, wrap))
            .collect();
        let texts: Vec<String> = painted.iter().map(row_text).collect();
        self.list.view_mut().set_rows(rows);
        let placed = self
            .list
            .place(rows, |index| row_height(&texts[index], cols, wrap, 0));
        let window = placed.window;
        let shown = painted.into_iter().skip(window.start).take(window.count);
        let content = Frame::new(shown.collect());
        match placed.shows_cursor {
            true => content.with_cursor_line(window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: None,
            segments: vec![format!("{} notifications", self.entries.len())],
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence() {
            if let Some(act) = table::find(KEYS, key) {
                return self.run(act);
            }
        }
        match self.nav.key(key) {
            VimKey::Motion(motion) => {
                self.list.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        let total = self.entries.len();
        self.list.view_mut().wheel(lines, total);
        EditorOutcome::Consumed
    }

    /// A press on a notification puts the cursor on it.
    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        self.list.view_mut().release();
        let at = self.list.view().scroll() + at.row;
        if at < self.entries.len() {
            self.list.set_cursor(at);
        }
        EditorOutcome::Consumed
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        match (reply.tag, reply.answer) {
            (ASK_SEARCH, Some(query)) => self.search_for(&query),
            _ => EditorOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_SEARCH && !query.trim().is_empty() {
            return self.search_for(query);
        }
        EditorOutcome::Consumed
    }

    fn set_notifications(&mut self, entries: &[Notification], now: i64) {
        self.set_entries(entries.to_vec(), now);
    }

    /// Copy the message under the pointer, or clear the history, or close.
    fn context_items(&self) -> Vec<EditorMenuItem> {
        let entry =
            |act| table::menu_entry(KEYS, act).map(|(key, label)| EditorMenuItem::new(key, label));
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
// Functions
// ========================================================================

/// `text`, cut short with an ellipsis to fit in `room` columns.
fn fit(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_string();
    }
    let mut cut = String::new();
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w + 1 > room {
            break;
        }
        cut.push(c);
        used += w;
    }
    if room > 0 {
        cut.push(ELLIPSIS);
    }
    cut
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
        let mut editor = NotificationsEditor::new(entries, 10);
        let frame = editor.content(10, 60, false);
        let rows = texts(&frame);
        assert!(
            rows[0].starts_with(&format!("{ERROR_ICON} second")),
            "{rows:?}"
        );
        assert!(
            rows[1].starts_with(&format!("{INFO_ICON} first")),
            "{rows:?}"
        );
        assert_eq!(
            frame.rows[0][0].style,
            Style::ChangeConflict,
            "an error's icon is red"
        );
        assert_eq!(
            editor.on_key(&Key::plain(crate::model::input::KeyCode::Char('y'))),
            EditorOutcome::Yank("second".to_string())
        );
    }

    #[test]
    fn test_a_new_notification_keeps_the_cursor_on_the_one_it_was_on() {
        let mut editor =
            NotificationsEditor::new(history(&[Notice::info("a"), Notice::info("b")]), 5);
        editor.list.set_cursor(1);
        assert_eq!(
            editor.selected().map(|entry| entry.text.as_str()),
            Some("a")
        );
        let newer = history(&[Notice::info("a"), Notice::info("b"), Notice::info("c")]);
        editor.set_notifications(&newer, 6);
        assert_eq!(
            editor.selected().map(|entry| entry.text.as_str()),
            Some("a")
        );
    }

    #[test]
    fn test_the_message_sits_at_the_left_and_its_age_at_the_right_edge() {
        let entries = history(&[Notice::info("x".repeat(80))]);
        let mut editor = NotificationsEditor::new(entries, 0);
        let age = reltime::ago(0, 0);
        let rows = texts(&editor.content(5, 40, false));
        assert_eq!(rows[0].width(), 40, "{rows:?}");
        assert!(rows[0].ends_with(&age), "{rows:?}");
        assert!(
            rows[0].contains(ELLIPSIS),
            "cut short for the age: {rows:?}"
        );

        let short = history(&[Notice::info("hi")]);
        let mut editor = NotificationsEditor::new(short, 0);
        let rows = texts(&editor.content(5, 40, false));
        assert!(rows[0].starts_with(&format!("{INFO_ICON} hi ")), "{rows:?}");
        assert!(rows[0].ends_with(&age), "{rows:?}");
        assert_eq!(rows[0].width(), 40, "{rows:?}");
    }

    #[test]
    fn test_an_empty_history_says_so() {
        let mut editor = NotificationsEditor::new(Vec::new(), 0);
        assert!(texts(&editor.content(5, 40, false)).contains(&EMPTY.to_string()));
    }

    #[test]
    fn test_clearing_asks_the_window_which_keeps_the_history() {
        let mut editor = NotificationsEditor::new(history(&[Notice::info("a")]), 0);
        assert_eq!(
            editor.on_key(&Key::plain(crate::model::input::KeyCode::Char('C'))),
            EditorOutcome::RunAction(CLEAR_ACTION.to_string())
        );
    }
}

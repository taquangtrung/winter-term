//! Messages: everything the window has said this session, in an Emacs-style
//! text stream buffer (`*Messages*`), chronological (oldest to newest).
//! Each entry is formatted with timestamp, source or severity badge, and body,
//! with unread markers tracked. The window hands the history over whenever
//! it changes, so the buffer stays current while the tab is up.

use std::collections::HashSet;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::editors::list::{ListPlace, ListView};
use crate::model::editor::{
    menu_groups, Editor, EditorMenuItem, EditorOutcome, EditorStatus, Notice,
    PromptReply, PromptRequest, SearchReport,
};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::Key;
use crate::model::notify::{Notification, Severity};
use crate::model::paint::{Frame, Point, Row, Span, Style};
use crate::model::vim::nav::{VimKey, VimNav};

// ========================================================================
// Constants
// ========================================================================

/// The editor's title, matching Emacs' `*Messages*` buffer convention.
const TITLE: &str = "*Messages*";

/// The one question the editor asks.
const ASK_SEARCH: &str = "notifications-search";

/// The window command that empties the history, which the window keeps.
const CLEAR_ACTION: &str = "clear_notifications";

/// What the page says with nothing to show, and under it, why.
const EMPTY: &str = "You're all caught up";
const EMPTY_DETAIL: &str = "Nothing has been said this session";

/// What marks a notification not yet looked at, and the room it takes
/// beside one that has been.
const UNREAD_MARK: &str = "\u{25cf} ";
const READ_MARK: &str = "  ";

/// Seconds in a day, for calculating local timestamp of the day.
const DAY_SECONDS: i64 = 86_400;

/// The editor's keys, ahead of the motions every list shares.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Clear,
        keys: &["C-M-l", "C"],
        label: "Clear All",
    },
    Binding {
        action: Act::Copy,
        keys: &["y"],
        label: "Copy Message",
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
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Clear,
    Copy,
    Search,
    /// The search again, forward when set.
    SearchStep(bool),
}

/// The window's message history as an Emacs-style stream buffer.
pub struct NotificationsEditor {
    /// What each screen row painted last shows: the entry index at that place.
    cards_at: Vec<Option<usize>>,
    entries: Vec<Notification>,
    /// The shown messages, by their place from the oldest (0 = oldest).
    list: ListView<usize>,
    nav: VimNav,
    notice: Option<Notice>,
    /// When the history was last handed over, in seconds since the Unix epoch.
    now: i64,
    /// The notifications not looked at when they were handed over, by
    /// their place from the oldest.
    unread: HashSet<usize>,
    /// How far ahead of UTC the local clock is, in seconds.
    utc_offset: i64,
}

// ========================================================================
// NotificationsEditor
// ========================================================================

impl NotificationsEditor {
    /// The editor over `entries`, chronological (oldest to newest).
    pub fn new(entries: Vec<Notification>, now: i64) -> Self {
        let mut editor = Self {
            cards_at: Vec::new(),
            entries: Vec::new(),
            list: ListView::new(),
            nav: VimNav::new(),
            notice: None,
            now,
            unread: HashSet::new(),
            utc_offset: local_utc_offset(),
        };
        editor.set_entries(entries, now);
        // Default scroll to the bottom of the stream buffer (newest message).
        let total = editor.entries.len();
        if total > 0 {
            editor.list.set_cursor(total.saturating_sub(1));
            editor.list.view_mut().set_scroll(total.saturating_sub(1));
        }
        editor
    }

    /// Show `entries` instead, keeping the cursor where it was or following
    /// to the bottom if it was already at the bottom.
    pub fn set_entries(&mut self, entries: Vec<Notification>, now: i64) {
        let was_at_bottom = self.list.cursor() + 1 >= self.entries.len();
        if entries.len() < self.entries.len() {
            self.unread.clear();
        }
        let total = entries.len();
        // The host passes entries newest-first; we store them in chronological
        // order (oldest to newest) for a stream buffer.
        let mut chronological = entries;
        chronological.reverse();

        self.unread.extend(
            chronological
                .iter()
                .enumerate()
                .filter(|(_, entry)| entry.is_unread())
                .map(|(index, _)| index),
        );
        self.entries = chronological;
        self.now = now;

        let keys: Vec<usize> = (0..total).collect();
        self.list.set_keys(keys);

        if was_at_bottom && total > 0 {
            self.list.set_cursor(total.saturating_sub(1));
            self.list.view_mut().set_scroll(total.saturating_sub(1));
        }
    }

    /// The notification known as `index` (0 = oldest).
    fn entry(&self, index: usize) -> Option<&Notification> {
        self.entries.get(index)
    }

    /// The notification under the cursor.
    fn selected(&self) -> Option<&Notification> {
        self.entry(*self.list.selected()?)
    }

    /// Format a single notification entry into styled rows:
    /// ` [unread] [HH:MM:SS] [Source] Message (xN)`
    fn message_rows(&self, index: usize, cols: usize) -> Vec<Row> {
        let Some(entry) = self.entry(index) else {
            return Vec::new();
        };

        let seconds_of_day = (entry.at + self.utc_offset).rem_euclid(DAY_SECONDS);
        let hours = seconds_of_day / 3600;
        let mins = (seconds_of_day % 3600) / 60;
        let secs = seconds_of_day % 60;
        let time_str = format!("[{hours:02}:{mins:02}:{secs:02}]");

        let mark = match self.unread.contains(&index) {
            true => Span::new(Style::Accent, UNREAD_MARK),
            false => Span::plain(READ_MARK),
        };

        let (tag_text, tag_style) = match (&entry.source, entry.severity) {
            (Some(source), Severity::Error) => (format!("[{source}]"), Style::ChangeConflict),
            (Some(source), Severity::Warning) => (format!("[{source}]"), Style::ChangeModified),
            (Some(source), Severity::Info) => (format!("[{source}]"), Style::Dim),
            (None, Severity::Error) => ("[Error]".to_string(), Style::ChangeConflict),
            (None, Severity::Warning) => ("[Warning]".to_string(), Style::ChangeModified),
            (None, Severity::Info) => ("[Info]".to_string(), Style::Dim),
        };

        let repeats_str = match entry.repeats {
            1 => String::new(),
            n => format!(" (\u{d7}{n})"),
        };

        let prefix_width = 1 + 2 + time_str.width() + 1 + tag_text.width() + 1;
        let body_room = cols.saturating_sub(prefix_width + repeats_str.width()).max(10);
        let lines: Vec<&str> = entry.text.lines().collect();
        let indent = " ".repeat(prefix_width);

        let mut rows = Vec::new();
        let first = lines.first().copied().unwrap_or_default();
        let first_pieces = wrap(first, body_room);

        // First row includes the time badge and tag.
        let mut row0 = vec![
            Span::plain(" "),
            mark,
            Span::new(Style::Dim, time_str),
            Span::plain(" "),
            Span::new(tag_style, tag_text),
            Span::plain(" "),
        ];

        if let Some(piece) = first_pieces.first() {
            row0.push(Span::plain(piece.clone()));
            if first_pieces.len() == 1 && lines.len() == 1 && !repeats_str.is_empty() {
                row0.push(Span::new(Style::Dim, repeats_str.clone()));
            }
        }
        rows.push(row0);

        // Continuation pieces of the first line.
        for piece in first_pieces.iter().skip(1) {
            rows.push(vec![Span::plain(format!("{indent}{piece}"))]);
        }

        // Subsequent lines from multi-line messages.
        for line in lines.iter().skip(1) {
            for piece in wrap(line, body_room) {
                rows.push(vec![Span::plain(format!("{indent}{piece}"))]);
            }
        }

        // If repeats weren't added to row 0, append to the final row.
        if (!repeats_str.is_empty()) && (first_pieces.len() > 1 || lines.len() > 1) {
            if let Some(last_row) = rows.last_mut() {
                last_row.push(Span::new(Style::Dim, repeats_str));
            }
        }

        rows
    }

    /// What the buffer shows when there are no messages.
    fn empty(&self, rows: usize, cols: usize) -> Vec<Row> {
        let centered = |text: &str, style: Style| {
            let pad = cols.saturating_sub(text.width()) / 2;
            vec![Span::plain(" ".repeat(pad)), Span::new(style, text.to_string())]
        };
        let mut painted = vec![Row::new(); rows / 3];
        painted.push(centered(EMPTY, Style::Header));
        painted.push(centered(EMPTY_DETAIL, Style::Dim));
        painted
    }

    /// Each message's text for search indexing.
    fn search_labels(&self) -> Vec<String> {
        self.entries.iter().map(Notification::message).collect()
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
            Act::Copy => match self.selected() {
                Some(entry) => EditorOutcome::Yank(entry.text.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::Search => EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH)),
            Act::SearchStep(forward) => {
                let labels = self.search_labels();
                self.notice = self.list.search_step(&labels, forward);
                EditorOutcome::Consumed
            }
        }
    }
}

// ========================================================================
// NotificationsEditor: what only the window asks of it
// ========================================================================

impl NotificationsEditor {
    /// Hear the window's notification history, newest first, each time it
    /// changes, with the time it was handed over in seconds since the Unix
    /// epoch: for the editor that lists it. Every other editor ignores it.
    pub(crate) fn set_notifications(&mut self, entries: &[Notification], now: i64) {
        self.set_entries(entries.to_vec(), now);
    }
}

impl Editor for NotificationsEditor {
    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.list.saved_place()))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(place) = state.downcast::<ListPlace<usize>>() {
            self.list.restore_place(*place);
        }
    }

    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        let mut painted = Vec::new();
        self.cards_at = Vec::new();

        if self.entries.is_empty() {
            painted.extend(self.empty(rows, cols));
            return Frame::new(painted);
        }

        let message_blocks: Vec<Vec<Row>> = (0..self.entries.len())
            .map(|index| self.message_rows(index, cols))
            .collect();

        self.list.view_mut().set_rows(rows);
        let placed = self.list.place(rows, |index| message_blocks[index].len());
        let window = placed.window;

        // A card is its own rows, so the view starting partway down one
        // drops them here rather than leaving them to the host.
        for (index, block) in message_blocks.iter().enumerate().skip(window.start).take(window.count) {
            let rows = block.iter().skip(window.skip_of(index));
            self.cards_at.extend(std::iter::repeat_n(Some(index), rows.len()));
            painted.extend(rows.cloned());
        }

        placed.band(Frame::new(painted), 0)
    }

    fn header_rows(&self) -> usize {
        0
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn take_search(&mut self) -> Option<SearchReport> {
        self.list.take_search()
    }

    fn status(&self) -> EditorStatus {
        let total = self.entries.len();
        let unread = self.unread.len();
        let said = match unread {
            0 => format!("{total} messages"),
            n => format!("{total} messages ({n} unread)"),
        };
        EditorStatus {
            mode: None,
            segments: vec![said],
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence() {
            if let Some(act) = table::find(KEYS, key) {
                return self.run(act);
            }
        }
        match self.nav.row_key(key) {
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

    fn set_scroll_margin(&mut self, rows: usize) {
        self.list.view_mut().set_margin(rows);
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        self.list.view_mut().wheel(lines);
        EditorOutcome::Consumed
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        self.list.view_mut().release();
        let Some(index) = self.cards_at.get(at.row).copied().flatten() else {
            return EditorOutcome::Consumed;
        };
        self.list.set_cursor(index);
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

    fn context_items(&self) -> Vec<EditorMenuItem> {
        let entry =
            |act| table::menu_entry(KEYS, act).map(|(key, label)| EditorMenuItem::new(key, label));
        let on_entry = match self.selected() {
            Some(_) => [Act::Copy].into_iter().filter_map(entry).collect(),
            None => Vec::new(),
        };
        menu_groups([
            on_entry,
            entry(Act::Search).into_iter().collect(),
            entry(Act::Clear).into_iter().collect(),
        ])
    }
}

// ========================================================================
// Functions
// ========================================================================

/// How far ahead of UTC the local clock is now, in seconds.
fn local_utc_offset() -> i64 {
    i64::from(chrono::Local::now().offset().local_minus_utc())
}

/// `text` broken into pieces of at most `width` columns, at a space where
/// one falls near the end, else anywhere.
fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut pieces = Vec::new();
    let mut rest = text;
    while rest.width() > width {
        let mut used = 0;
        let mut cut = 0;
        let mut space = None;
        for (at, c) in rest.char_indices() {
            let w = c.width().unwrap_or(0);
            if used + w > width {
                break;
            }
            used += w;
            cut = at + c.len_utf8();
            if c == ' ' {
                space = Some(at);
            }
        }
        let ends_word = rest[cut..].starts_with(' ');
        let end = match ends_word {
            true => cut,
            false => space.filter(|at| *at > 0).unwrap_or(cut.max(1)),
        };
        pieces.push(rest[..end].trim_end().to_string());
        rest = rest[end..].trim_start();
    }
    pieces.push(rest.to_string());
    pieces
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::KeyCode;
    use crate::model::notify::NotificationLog;
    use crate::model::paint::row_text;

    fn history(notices: &[Notice]) -> Vec<Notification> {
        let mut log = NotificationLog::default();
        for (at, notice) in notices.iter().enumerate() {
            log.record(notice, at as i64);
        }
        log.iter().cloned().collect()
    }

    fn editor_over(notices: &[Notice], now: i64) -> NotificationsEditor {
        let mut editor = NotificationsEditor::new(history(notices), now);
        editor.utc_offset = 0;
        editor
    }

    fn texts(frame: &Frame) -> Vec<String> {
        frame.rows.iter().map(row_text).collect()
    }

    fn press(code: KeyCode) -> Key {
        Key::plain(code)
    }

    #[test]
    fn test_message_stream_buffer_starts_on_row_0_chronological() {
        let mut editor = editor_over(
            &[
                Notice::info("first message"),
                Notice::error("cannot find value").from_source("cargo build"),
            ],
            10,
        );
        let frame = editor.content(20, 80, false);
        let rows = texts(&frame);

        // Row 0 is the first message immediately: no pinned header or divider rule.
        assert!(rows[0].contains("first message"), "first notice on row 0: {rows:?}");
        assert!(rows[1].contains("cargo build"), "second notice on row 1: {rows:?}");
        assert!(rows[1].contains("cannot find value"), "second notice body: {rows:?}");

        // Yank copies the selected message.
        assert_eq!(
            editor.on_key(&press(KeyCode::Char('y'))),
            EditorOutcome::Yank("cannot find value".to_string())
        );

        // 'q' does not close the editor.
        assert_ne!(
            editor.on_key(&press(KeyCode::Char('q'))),
            EditorOutcome::Close
        );
    }

    #[test]
    fn test_clear_with_c_and_ctrl_alt_l() {
        let mut editor = editor_over(&[Notice::info("message")], 1);

        // 'C' clears.
        assert_eq!(
            editor.on_key(&press(KeyCode::Char('C'))),
            EditorOutcome::RunAction(CLEAR_ACTION.to_string())
        );

        // 'C-M-l' clears.
        let chord_c_m_l = Key {
            alt: true,
            code: KeyCode::Char('l'),
            ctrl: true,
            shift: false,
            sup: false,
        };
        assert_eq!(
            editor.on_key(&chord_c_m_l),
            EditorOutcome::RunAction(CLEAR_ACTION.to_string())
        );
    }

    #[test]
    fn test_unread_notices_marked_with_accent_dot() {
        let mut editor = editor_over(&[Notice::info("done"), Notice::error("failed")], 5);
        let rows = texts(&editor.content(20, 70, false));

        // Info notice is never unread; error notice is marked unread.
        assert!(!rows[0].contains(UNREAD_MARK.trim()), "info not unread: {rows:?}");
        assert!(rows[1].contains(UNREAD_MARK.trim()), "error marked unread: {rows:?}");
    }

    #[test]
    fn test_empty_buffer_shows_all_caught_up() {
        let mut editor = editor_over(&[], 0);
        let rows = texts(&editor.content(12, 40, false));
        assert!(rows.iter().any(|row| row.contains(EMPTY)), "{rows:?}");
        assert!(rows.iter().any(|row| row.contains(EMPTY_DETAIL)), "{rows:?}");
    }

    #[test]
    fn test_wrapping_long_lines_at_spaces() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
    }
}

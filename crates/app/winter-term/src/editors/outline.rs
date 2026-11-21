//! The Outline page: the headings and symbols of the file being worked on,
//! the main-area tab that last had the keyboard, one to a row. A sidebar
//! tool: the window hands it the file's outline as it changes, and the entry
//! the file's cursor is in is selected while the page is not in use. `RET`
//! or a click takes that file there.

use unicode_width::UnicodeWidthStr;

use winter_model::editor::{EditorOutcome, EditorStatus, JumpTarget, Page, ScrollPosition};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::list::ListView;
use winter_model::paint::{Frame, Point, Row, Span, Style};
use winter_model::vim::nav::{VimKey, VimNav};

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Outline";

/// What the page says with nothing to list, and under it, why.
const EMPTY: &str = "No outline";
const EMPTY_DETAIL: &str = "Nothing to list in this tab";

/// The page's keys, ahead of the motions every list shares.
const KEYS: KeyTable<Act> = KeyTable {
    block: "outline",
    bindings: &[Binding {
        action: Act::Go,
        id: "go",
        keys: &["RET"],
        label: "Go to Entry",
    }],
    configurable: true,
    title: "Outline",
};

/// Every key table of this surface, for the Keys editor to list.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the page's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    /// Take the file to the entry selected.
    Go,
}

/// The outline of the file being worked on.
pub struct OutlinePage {
    /// The entry chosen to go to, by the file's own index for it, until the
    /// window takes it.
    chosen: Option<usize>,
    /// The file's headings and symbols, in its order.
    entries: Vec<JumpTarget>,
    /// The entries shown, by their place in the file's order.
    list: ListView<usize>,
    nav: VimNav,
    /// Which entry each row painted last shows.
    rows_at: Vec<Option<usize>>,
    /// The name of the file outlined, `None` with no file.
    source: Option<String>,
}

// ========================================================================
// OutlinePage
// ========================================================================

impl OutlinePage {
    /// The page, empty until the window hands it an outline.
    pub fn new() -> Self {
        Self {
            chosen: None,
            entries: Vec::new(),
            list: ListView::new(),
            nav: VimNav::new(),
            rows_at: Vec::new(),
            source: None,
        }
    }

    /// Show `entries`, the outline of the file named `source`, selecting
    /// `at`, the entry the file's cursor is in, when one is given; the
    /// selection stays where it was otherwise.
    pub(crate) fn set_outline(
        &mut self,
        source: Option<String>,
        entries: Vec<JumpTarget>,
        at: Option<usize>,
    ) {
        let same_source = source == self.source;
        self.list.set_keys((0..entries.len()).collect());
        self.entries = entries;
        self.source = source;
        match at {
            Some(at) => self.list.set_cursor(at),
            None if !same_source => self.list.set_cursor(0),
            None => {}
        }
    }

    /// The entry chosen to go to, by the file's own index for it, taken so
    /// it is gone to once.
    pub(crate) fn take_chosen(&mut self) -> Option<usize> {
        self.chosen.take()
    }

    /// Choose the entry under the cursor to go to.
    fn choose(&mut self) {
        self.chosen = self
            .list
            .selected()
            .and_then(|index| self.entries.get(*index))
            .map(|entry| entry.target);
    }

    /// What the page shows with nothing to list.
    fn empty(rows: usize, cols: usize) -> Vec<Row> {
        let centered = |text: &str, style: Style| {
            let pad = cols.saturating_sub(text.width()) / 2;
            vec![
                Span::plain(" ".repeat(pad)),
                Span::new(style, text.to_string()),
            ]
        };
        let mut painted = vec![Row::new(); rows / 3];
        painted.push(centered(EMPTY, Style::Header));
        painted.push(centered(EMPTY_DETAIL, Style::Dim));
        painted
    }
}

impl Default for OutlinePage {
    fn default() -> Self {
        Self::new()
    }
}

impl Page for OutlinePage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.rows_at = Vec::new();
        if self.entries.is_empty() {
            return Frame::new(Self::empty(rows, cols));
        }
        self.list.view_mut().set_rows(rows);
        let placed = self.list.place(rows, |_| 1);
        let window = placed.window;
        let painted: Vec<Row> = self
            .entries
            .iter()
            .enumerate()
            .skip(window.start)
            .take(window.count)
            .map(|(index, entry)| {
                self.rows_at.push(Some(index));
                vec![Span::plain(" "), Span::plain(entry.label.clone())]
            })
            .collect();
        placed.band(Frame::new(painted), 0)
    }

    fn header_rows(&self) -> usize {
        0
    }

    fn status(&self) -> EditorStatus {
        let said = match &self.source {
            Some(source) => format!("{source}: {} entries", self.entries.len()),
            None => format!("{} entries", self.entries.len()),
        };
        EditorStatus {
            mode: None,
            segments: vec![said],
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence()
            && let Some(Act::Go) = table::find(&KEYS, key)
        {
            self.choose();
            return EditorOutcome::Consumed;
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
        let mut rows = table::help(&KEYS);
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

    fn scroll_position(&self) -> Option<ScrollPosition> {
        self.list.scroll_position()
    }

    fn scroll_to(&mut self, offset: usize) -> EditorOutcome {
        self.list.scroll_to(offset);
        EditorOutcome::Consumed
    }

    /// A click on an entry goes to it.
    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        self.list.view_mut().release();
        let Some(index) = self.rows_at.get(at.row).copied().flatten() else {
            return EditorOutcome::Consumed;
        };
        self.list.set_cursor(index);
        self.choose();
        EditorOutcome::Consumed
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::input::KeyCode;

    use super::*;

    /// An entry named `label` at the file's index `target`.
    fn entry(label: &str, target: usize) -> JumpTarget {
        JumpTarget {
            icon: None,
            label: label.to_string(),
            target,
        }
    }

    #[test]
    fn test_return_chooses_the_selected_entry_once() {
        let mut page = OutlinePage::new();
        page.set_outline(
            Some("notes.md".to_string()),
            vec![entry("Intro", 0), entry("Usage", 12), entry("Notes", 40)],
            Some(1),
        );
        page.on_key(&Key::plain(KeyCode::Char('j')));
        page.on_key(&Key::plain(KeyCode::Enter));
        assert_eq!(page.take_chosen(), Some(40), "the entry below the file's");
        assert_eq!(page.take_chosen(), None, "gone to once");
    }

    #[test]
    fn test_a_new_file_starts_at_the_top_and_the_same_file_keeps_its_place() {
        let mut page = OutlinePage::new();
        let entries = vec![entry("A", 0), entry("B", 5), entry("C", 9)];
        page.set_outline(Some("a.rs".to_string()), entries.clone(), Some(2));
        page.set_outline(Some("a.rs".to_string()), entries.clone(), None);
        page.on_key(&Key::plain(KeyCode::Enter));
        assert_eq!(page.take_chosen(), Some(9), "kept");
        page.set_outline(Some("b.rs".to_string()), entries, None);
        page.on_key(&Key::plain(KeyCode::Enter));
        assert_eq!(page.take_chosen(), Some(0), "a new file from its top");
    }

    #[test]
    fn test_nothing_to_list_says_so() {
        let mut page = OutlinePage::new();
        let frame = page.content(9, 40, false);
        let text: String = frame
            .rows
            .iter()
            .flat_map(|row| row.iter().map(|span| span.text.clone()))
            .collect();
        assert!(text.contains(EMPTY), "{text}");
    }
}

//! A picker's rows saved as a buffer of their own: the lines it showed, to
//! read, search, and copy, and a path among them to open. Named after the
//! picker and what its rows were filtered by (`Shell History: cargo`).

use winter_model::editor::{
    menu_groups, present, Editor, EditorMenuItem, EditorOutcome, EditorStatus, Notice, OpenTarget,
    Page, PromptReply, PromptRequest, ScrollPosition, SearchReport,
};
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::input::Key;
use winter_model::list::ListView;
use winter_model::paint::{Frame, Point, Row, Span};
use winter_model::path::expand_home;
use winter_model::vim::nav::{VimKey, VimNav};

// ========================================================================
// Constants
// ========================================================================

/// The one question the editor asks.
const ASK_SEARCH: &str = "list-search";

/// What a line's path is cut from the rest of it by: the gap the picker put
/// between a row's label and its description.
const FIELD_GAP: &str = "  ";

/// What `Enter` says on a line naming no place on this machine.
const NO_PATH: &str = "no path on this line";

/// The editor's keys, ahead of the motions every list shares.
const KEYS: KeyTable<Act> = KeyTable {
    block: "list",
    bindings: &[
        Binding {
            action: Act::Open,
            id: "open",
            keys: &["RET"],
            label: "Open the Path on the Line",
        },
        Binding {
            action: Act::Copy,
            id: "copy",
            keys: &["y"],
            label: "Copy the Line",
        },
        Binding {
            action: Act::CopyAll,
            id: "copy_all",
            keys: &["Y"],
            label: "Copy Every Line",
        },
        Binding {
            action: Act::Search,
            id: "search",
            keys: &["/"],
            label: "Find",
        },
        Binding {
            action: Act::SearchStep(true),
            id: "next_match",
            keys: &["n"],
            label: "Next Match",
        },
        Binding {
            action: Act::SearchStep(false),
            id: "previous_match",
            keys: &["N"],
            label: "Previous Match",
        },
    ],
    configurable: true,
    title: "Saved List",
};

/// Every key table of this surface, for the Keys editor to list.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Copy,
    CopyAll,
    Open,
    Search,
    /// The search again, forward when set.
    SearchStep(bool),
}

/// A picker's rows, saved.
pub struct ListEditor {
    lines: Vec<String>,
    /// The lines, by their place.
    list: ListView<usize>,
    nav: VimNav,
    notice: Option<Notice>,
    /// The first screen row the last frame painted, for a click to find its
    /// line by.
    painted_from: usize,
    /// What the buffer is called (see [`buffer_title`]).
    title: String,
}

// ========================================================================
// ListEditor
// ========================================================================

impl ListEditor {
    /// The lines `lines` of the picker called `name`, its rows filtered by
    /// `query`.
    pub fn new(name: &str, query: &str, lines: Vec<String>) -> Self {
        let mut list = ListView::new();
        list.set_keys((0..lines.len()).collect());
        Self {
            lines,
            list,
            nav: VimNav::new(),
            notice: None,
            painted_from: 0,
            title: buffer_title(name, query),
        }
    }

    /// The line under the cursor.
    fn selected(&self) -> Option<&String> {
        self.lines.get(*self.list.selected()?)
    }

    /// Move to the next line holding `query`.
    fn search_for(&mut self, query: &str) -> EditorOutcome {
        self.notice = self.list.search_for(&self.lines, query.trim());
        EditorOutcome::Consumed
    }

    /// Open the place the line under the cursor names, where it names one
    /// on this machine.
    fn open(&mut self) -> EditorOutcome {
        let path = self.selected().map(|line| {
            let field = line.split(FIELD_GAP).next().unwrap_or(line);
            expand_home(field.trim())
        });
        match path.filter(|path| path.is_absolute() && path.exists()) {
            Some(path) => EditorOutcome::OpenPath(OpenTarget::file(path)),
            None => {
                self.notice = Some(Notice::refusal(NO_PATH));
                EditorOutcome::Consumed
            }
        }
    }

    /// Do what a key of the editor's own asks.
    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::Copy => match self.selected() {
                Some(line) => EditorOutcome::Yank(line.clone()),
                None => EditorOutcome::Consumed,
            },
            Act::CopyAll => EditorOutcome::Yank(self.lines.join("\n")),
            Act::Open => self.open(),
            Act::Search => EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH)),
            Act::SearchStep(forward) => {
                self.notice = self.list.search_step(&self.lines, forward);
                EditorOutcome::Consumed
            }
        }
    }
}

impl Page for ListEditor {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn content(&mut self, rows: usize, _cols: usize, _wrap: bool) -> Frame {
        self.list.view_mut().set_rows(rows);
        let placed = self.list.place(rows, |_| 1);
        let window = placed.window;
        self.painted_from = window.start;
        let painted: Vec<Row> = self
            .lines
            .iter()
            .skip(window.start)
            .take(window.count)
            .map(|line| vec![Span::plain(line.clone())])
            .collect();
        placed.band(Frame::new(painted), 0)
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
            segments: vec![format!("{} lines", self.lines.len())],
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if !self.nav.in_sequence() {
            if let Some(act) = table::find(&KEYS, key) {
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

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        if at.drag {
            return EditorOutcome::Ignored;
        }
        self.list.view_mut().release();
        let line = self.painted_from + at.row;
        if line < self.lines.len() {
            self.list.set_cursor(line);
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

    fn context_items(&self) -> Vec<EditorMenuItem> {
        let entry = |act| EditorMenuItem::bound(&KEYS, act);
        let on_line = self.selected().is_some();
        menu_groups([
            present([
                entry(Act::Open).filter(|_| on_line),
                entry(Act::Copy).filter(|_| on_line),
                entry(Act::CopyAll),
            ]),
            present([entry(Act::Search)]),
        ])
    }
}

impl Editor for ListEditor {}

// ========================================================================
// Functions
// ========================================================================

/// What a list saved from the picker called `name` is called: the picker,
/// and what its rows were filtered by where they were (`Recent: notes`).
fn buffer_title(name: &str, query: &str) -> String {
    match query.trim() {
        "" => name.to_string(),
        query => format!("{name}: {query}"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use winter_model::input::KeyCode;

    fn press(code: KeyCode) -> Key {
        Key::plain(code)
    }

    #[test]
    fn test_a_saved_list_is_named_by_its_picker_and_its_filter() {
        assert_eq!(
            ListEditor::new("Recent", "notes", Vec::new()).title(),
            "Recent: notes"
        );
        assert_eq!(
            ListEditor::new("Recent", "  ", Vec::new()).title(),
            "Recent"
        );
    }

    #[test]
    fn test_enter_opens_the_path_a_line_starts_with_and_refuses_a_line_naming_none() {
        let dir = std::env::temp_dir();
        let lines = vec![
            format!("{}{FIELD_GAP}directory", dir.display()),
            "cargo build".to_string(),
        ];
        let mut editor = ListEditor::new("Recent", "", lines);
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::OpenPath(OpenTarget::file(dir))
        );
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::Consumed
        );
        assert_eq!(editor.take_notice(), Some(Notice::refusal(NO_PATH)));
    }
}

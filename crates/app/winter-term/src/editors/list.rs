//! A picker's rows saved as a buffer of their own: the lines it showed, to
//! read, search, and copy, and a path among them to open, at its line where
//! it names one (`~/src/a.rs:12`). Named after the picker and what its rows
//! were filtered by (`Shell History: cargo`).

use std::path::Path;

use winter_model::editor::{
    Editor, EditorMenuItem, EditorOutcome, EditorStatus, Notice, OpenTarget, Page, PromptReply,
    PromptRequest, ScrollPosition, SearchReport, menu_groups, present,
};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed};
use winter_model::list::{ListView, Viewport};
use winter_model::paint::{Frame, Point, Row, Span, row_height};
use winter_model::path::{abbreviate_home, expand_home};
use winter_model::vim::nav::{VimKey, VimNav};

use super::notice::ListNotice;
use crate::session::SavedListSession;

// ========================================================================
// Constants
// ========================================================================

/// The one question the editor asks.
const ASK_SEARCH: &str = "list-search";

/// What a line's path is cut from the rest of it by: the gap the picker put
/// between a row's label and its description.
const FIELD_GAP: &str = "  ";

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
    notice: Option<ListNotice>,
    /// Each line the last frame painted and the screen rows it took there,
    /// top to bottom, for a click to find its line by.
    painted: Vec<(usize, usize)>,
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
            painted: Vec::new(),
            title: buffer_title(name, query),
        }
    }

    /// The list as a session keeps it: what it is called, and its lines.
    pub fn saved(&self) -> SavedListSession {
        SavedListSession {
            lines: self.lines.clone(),
            title: self.title.clone(),
        }
    }

    /// The list a session kept as `saved`.
    pub fn from_saved(saved: &SavedListSession) -> Self {
        Self::new(&saved.title, "", saved.lines.clone())
    }

    /// The line under the cursor.
    fn selected(&self) -> Option<&String> {
        self.lines.get(*self.list.selected()?)
    }

    /// Move to the next line holding `query`.
    fn search_for(&mut self, query: &str) -> EditorOutcome {
        self.notice = self
            .list
            .search_for(&self.lines, query.trim())
            .map(ListNotice::Search);
        EditorOutcome::Consumed
    }

    /// Open the place the line under the cursor names, where it names one
    /// on this machine.
    fn open(&mut self) -> EditorOutcome {
        match self.selected().and_then(|line| place_of(line)) {
            Some(target) => EditorOutcome::OpenPath(target),
            None => {
                self.notice = Some(ListNotice::NoPath);
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
                self.notice = self
                    .list
                    .search_step(&self.lines, forward)
                    .map(ListNotice::Search);
                EditorOutcome::Consumed
            }
        }
    }
}

impl Page for ListEditor {
    fn title(&self) -> String {
        self.title.clone()
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        self.list.view_mut().set_rows(rows);
        let lines = &self.lines;
        let height = |index: usize| row_height(&lines[index], cols, wrap, 0);
        let placed = self.list.place(rows, height);
        let window = placed.window;
        let shown = window.start..window.start + window.count;
        self.painted = shown
            .clone()
            .map(|index| (index, height(index) - window.skip_of(index)))
            .collect();
        let painted: Vec<Row> = self.lines[shown]
            .iter()
            .map(|line| vec![Span::plain(line.clone())])
            .collect();
        placed.apply(Frame::new(painted), 0)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take().map(ListNotice::into_notice)
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
        if !self.nav.in_sequence()
            && let Some(act) = table::find(&KEYS, key)
        {
            return self.run(act);
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

    fn visit_viewports(&mut self, visit: &mut dyn FnMut(&mut Viewport)) {
        visit(self.list.view_mut());
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
        let mut top = 0;
        for &(line, height) in &self.painted {
            if at.row < top + height {
                self.list.set_cursor(line);
                break;
            }
            top += height;
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

/// A line naming the place `line` of `path`, then `text` after it: what a
/// search's line found is saved as, for `Enter` to open there.
pub(crate) fn place_line(path: &Path, line: usize, text: &str) -> String {
    format!("{}:{line}{FIELD_GAP}{}", abbreviate_home(path), text.trim())
}

/// The place `line` starts with, before the gap to the rest of it: a path
/// on this machine, at the line a `:` and a number after it name.
pub(crate) fn place_of(line: &str) -> Option<OpenTarget> {
    let field = line.split(FIELD_GAP).next().unwrap_or(line).trim();
    let exists = |path: &Path| path.is_absolute() && path.exists();
    let whole = expand_home(field);
    if exists(&whole) {
        return Some(OpenTarget::file(whole));
    }
    let (path, number) = field.rsplit_once(':')?;
    let path = expand_home(path);
    let number = number.parse().ok()?;
    exists(&path).then(|| OpenTarget::at_line(path, number))
}

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
        assert_eq!(editor.notice.take(), Some(ListNotice::NoPath));
    }

    #[test]
    fn test_j_and_k_step_through_a_wrapped_line_by_screen_rows() {
        let lines = vec!["w".repeat(70), "after".to_string(), "last".to_string()];
        let mut editor = ListEditor::new("Recent", "", lines);
        winter_model::list::test_support::assert_steps_screen_rows(&mut editor, 10, 30);
    }

    #[test]
    fn test_a_click_on_a_wrapped_line_s_second_screen_row_picks_that_line() {
        let lines = vec!["w".repeat(70), "after".to_string()];
        let mut editor = ListEditor::new("Recent", "", lines);
        editor.content(10, 30, true);
        let click = |row| Point {
            col: 0,
            drag: false,
            row,
        };
        editor.on_mouse(click(2));
        assert_eq!(editor.list.cursor(), 0, "the first line takes three screen rows");
        editor.on_mouse(click(3));
        assert_eq!(editor.list.cursor(), 1);
    }

    #[test]
    fn test_enter_opens_a_saved_search_line_at_its_line() {
        let file = std::env::temp_dir().join(format!("winter-list-{}.rs", std::process::id()));
        std::fs::write(&file, "a\nb\n").expect("file");
        let lines = vec![place_line(&file, 2, "  b  ")];
        assert!(lines[0].ends_with(&format!(":2{FIELD_GAP}b")), "{lines:?}");
        let mut editor = ListEditor::new("Search", "b", lines);
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::OpenPath(OpenTarget::at_line(file.clone(), 2))
        );
        let _ = std::fs::remove_file(&file);
    }
}

//! The Todo page: the comments across a project that hold one of the
//! `todo-keywords`, under the file each is in. Each shows its keyword as a
//! badge and the words after it, the comment's marks dropped; the footer
//! shows the selected one's place and its line as written. The tabs at the
//! heading's right narrow the list to one keyword.
//!
//! Misty draws the page as a view ([`PageView`]); its keys are Winter's, in
//! the `todo` block of `keybindings.kdl`, moving the page's own cursor.
//! The comments are found by the window's search job, again when the page
//! comes back into sight or is asked to (`r`).

use std::path::{Path, PathBuf};
use std::time::Instant;

use misty::view::{Item, Press, Space, badge, column, item, page, text};
use misty::{AccessAction, Element, EventCx, Frame as MistyFrame, HostedView, Input, Rect};
use misty::{Theme, ThemeToken};

use winter_model::editor::{
    EditorOutcome, EditorStatus, JobReply, JobRequest, Notice, OpenTarget, Page, PageView,
    SearchHit, SearchRequest, SearchResult,
};
use winter_model::input::Key;
use winter_model::input::table::{self, Binding, KeyHelp, KeyTable, Listed, TableMatch};
use winter_model::paint::{Frame, Row};

// ========================================================================
// Constants
// ========================================================================

/// What the page and its tab are called.
const TITLE: &str = "Todo";

/// What the first of the heading's tabs, every keyword at once, is called.
const ALL: &str = "All";

/// What the page says while it looks, and with nothing found.
const SEARCHING: &str = "Looking for comments…";
const NOTHING_FOUND: &str = "Nothing to do";
const NOTHING_FOUND_DETAIL: &str = "No comment here holds one of the todo-keywords";

/// What an item says when its comment has no words past its keyword.
const NO_WORDS: &str = "(no words)";

/// What a comment may open or close with around its words, dropped from
/// what an item says: the marks of the languages' comments and the
/// punctuation a keyword is written with (`TODO:`, `TODO -`).
const LEADING_MARKS: &[char] = &[':', '-', ' ', '\t'];
const TRAILING_MARKS: &[&str] = &["*/", "-->", "#}", "%}", "--}}", "*)"];

/// The clicks in a row that open it.
const DOUBLE_CLICK: u8 = 2;

/// The page's keys, block `todo`.
const KEYS: KeyTable<Act> = KeyTable {
    block: "todo",
    bindings: &[
        Binding {
            action: Act::Down,
            id: "next_row",
            keys: &["j", "<down>"],
            label: "Next Comment",
        },
        Binding {
            action: Act::Up,
            id: "previous_row",
            keys: &["k", "<up>"],
            label: "Previous Comment",
        },
        Binding {
            action: Act::NextFile,
            id: "next_file",
            keys: &["J", "M-n"],
            label: "First Comment of the Next File",
        },
        Binding {
            action: Act::PreviousFile,
            id: "previous_file",
            keys: &["K", "M-p"],
            label: "First Comment of the Previous File",
        },
        Binding {
            action: Act::First,
            id: "first_row",
            keys: &["g g", "<home>"],
            label: "First Comment",
        },
        Binding {
            action: Act::Last,
            id: "last_row",
            keys: &["G", "<end>"],
            label: "Last Comment",
        },
        Binding {
            action: Act::Open,
            id: "open",
            keys: &["RET"],
            label: "Open the Comment's Line",
        },
        Binding {
            action: Act::OpenBeside,
            id: "open_beside",
            keys: &["o"],
            label: "Open the Comment's Line Beside",
        },
        Binding {
            action: Act::NextKeyword,
            id: "next_keyword",
            keys: &["]"],
            label: "Only the Next Keyword",
        },
        Binding {
            action: Act::PreviousKeyword,
            id: "previous_keyword",
            keys: &["["],
            label: "Only the Previous Keyword",
        },
        Binding {
            action: Act::Refresh,
            id: "refresh",
            keys: &["r", "C-r", "M-S-r"],
            label: "Look Again",
        },
    ],
    configurable: true,
    title: "Todo",
};

/// The page's key tables, for the Keys page and `keybindings.kdl`.
pub(crate) const KEY_TABLES: &[&dyn Listed] = &[&KEYS];

// ========================================================================
// Data Structures
// ========================================================================

/// What a Todo key does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Down,
    First,
    Last,
    NextFile,
    NextKeyword,
    Open,
    OpenBeside,
    PreviousFile,
    PreviousKeyword,
    Refresh,
    Up,
}

/// The Todo page.
pub(crate) struct TodoPage {
    /// The page's view and what it shows.
    view: PageView<Todos>,
}

/// What the page shows and where its cursor is: the state its view is
/// built from and its keys and clicks change.
struct Todos {
    /// The comments found, a file at a time, in line order.
    found: Vec<Todo>,
    /// The words searched for, in the order the heading's tabs list them.
    keywords: Vec<String>,
    /// Whether a search is to be asked for on the next tick.
    look: bool,
    /// What to tell the user, until the window takes it.
    notice: Option<Notice>,
    /// What a click asked of the window, until the window takes it.
    outcome: Option<EditorOutcome>,
    /// The project searched.
    root: PathBuf,
    /// The rows of the list: the comments the tab chosen keeps, each file's
    /// name above its first.
    rows: Vec<Line>,
    /// The heading's tab chosen: `0` for every keyword, else one past the
    /// keyword's place in `keywords`.
    scope: usize,
    /// Whether a search is out.
    searching: bool,
    /// The row the cursor is on, a comment's whenever there is one.
    selected: usize,
    /// The keys of a sequence typed so far (`g` of `g g`).
    typed: Vec<Key>,
}

/// One comment found.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct Todo {
    /// Which keyword it holds, as `keywords` writes it.
    pub(crate) keyword: usize,
    /// Which line of its file it is on, counting from one.
    pub(crate) line: usize,
    /// The file it is in.
    pub(crate) path: PathBuf,
    /// Its line as written.
    source: String,
    /// The words after its keyword, its marks dropped.
    pub(crate) words: String,
}

/// One row of the list.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Line {
    /// The name of the file the comments under it are in, by the first.
    File(usize),
    /// A comment, by its place in `found`.
    Todo(usize),
}

// ========================================================================
// TodoPage
// ========================================================================

impl TodoPage {
    /// The page over the project at `root`, looking for `keywords`.
    pub(crate) fn new(root: PathBuf, keywords: &[String]) -> Self {
        Self {
            view: PageView::new(Todos {
                found: Vec::new(),
                keywords: keywords.to_vec(),
                look: true,
                notice: None,
                outcome: None,
                root,
                rows: Vec::new(),
                scope: 0,
                searching: false,
                selected: 0,
                typed: Vec::new(),
            }),
        }
    }
}

impl Page for TodoPage {
    fn title(&self) -> String {
        TITLE.to_string()
    }

    /// Misty draws the page; the grid under it is empty.
    fn content(&mut self, _rows: usize, _cols: usize, _wrap: bool) -> Frame {
        Frame::new(vec![Row::new()])
    }

    fn is_view(&self) -> bool {
        true
    }

    fn draw_view(&mut self, frame: &mut MistyFrame, area: Rect, theme: &Theme) {
        self.view.draw_view(frame, area, theme);
    }

    /// The view's clicks and tabs, and what they asked of the window; its
    /// keys go on to [`Page::on_key`].
    fn view_input(&mut self, input: &Input, cx: &mut EventCx<'_>) -> Option<EditorOutcome> {
        let used = self.view.view_input(input, cx);
        let asked = self.view.state_mut().outcome.take();
        asked.or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_access_action(&mut self, action: AccessAction) -> Option<EditorOutcome> {
        let used = self.view.access_action(action);
        let asked = self.view.state_mut().outcome.take();
        asked.or(used.then_some(EditorOutcome::Consumed))
    }

    fn view_about_to_wait(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.view.view_about_to_wait(cx)
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.view.state_mut().on_key(key)
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        if let JobReply::Search(found) = reply {
            self.view.state_mut().take_found(found);
        }
        EditorOutcome::Consumed
    }

    fn next_tick(&self) -> Option<Instant> {
        self.view.state().look.then(Instant::now)
    }

    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        self.view.state_mut().search()
    }

    /// What was found may have changed while the page was covered.
    fn on_resume(&mut self) -> EditorOutcome {
        self.view.state_mut().search()
    }

    fn keys(&self) -> Vec<KeyHelp> {
        table::help(&KEYS)
    }

    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: None,
            segments: vec![self.view.state().count()],
        }
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.view.state_mut().notice.take()
    }

    fn crumb_path(&self) -> Option<PathBuf> {
        Some(self.view.state().root.clone())
    }

    fn cwd(&self) -> Option<PathBuf> {
        Some(self.view.state().root.clone())
    }
}

// ========================================================================
// Todos
// ========================================================================

impl Todos {
    /// Ask the window to look for the comments, unless it is looking now.
    fn search(&mut self) -> EditorOutcome {
        self.look = false;
        if self.searching || self.keywords.is_empty() {
            return EditorOutcome::Consumed;
        }
        self.searching = true;
        EditorOutcome::Job(JobRequest::Search(SearchRequest {
            root: self.root.clone(),
            words: self.keywords.clone(),
        }))
    }

    /// Take what a search found, keeping the cursor on the comment it was
    /// on where that is still there.
    fn take_found(&mut self, found: SearchResult) {
        self.searching = false;
        let was = self.selected_todo().cloned();
        self.found = found
            .hits
            .iter()
            .filter_map(|hit| todo_of(hit, &self.keywords))
            .collect();
        self.rebuild();
        if let Some(was) = was {
            let again = self.rows.iter().position(|line| {
                matches!(line, Line::Todo(at)
                    if self.found[*at].path == was.path && self.found[*at].source == was.source)
            });
            if let Some(again) = again {
                self.selected = again;
            }
        }
    }

    /// The rows the chosen tab keeps, the cursor kept on a comment.
    fn rebuild(&mut self) {
        self.rows = rows(&self.found, self.scope);
        self.selected = self.selected.min(self.rows.len().saturating_sub(1));
        self.settle(1);
    }

    /// Put the cursor on a comment, stepping off a file's name the way
    /// `step` goes (`1` down, `-1` up), and back the other way at an end.
    fn settle(&mut self, step: isize) {
        for step in [step, -step] {
            let mut at = self.selected;
            loop {
                if matches!(self.rows.get(at), Some(Line::Todo(_))) {
                    self.selected = at;
                    return;
                }
                match at.checked_add_signed(step) {
                    Some(next) if next < self.rows.len() => at = next,
                    _ => break,
                }
            }
        }
    }

    /// A key, a sequence of the page's own (`g g`) waited out.
    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.typed.push(*key);
        match table::lookup(&KEYS, &self.typed) {
            TableMatch::Exact(act) => {
                self.typed.clear();
                self.run(act)
            }
            TableMatch::Prefix => EditorOutcome::Consumed,
            TableMatch::Miss => {
                let ended = self.typed.len() > 1;
                self.typed.clear();
                match ended {
                    true => EditorOutcome::Consumed,
                    false => EditorOutcome::Ignored,
                }
            }
        }
    }

    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::Down => self.move_by(1),
            Act::Up => self.move_by(-1),
            Act::First => {
                self.selected = 0;
                self.settle(1);
            }
            Act::Last => {
                self.selected = self.rows.len().saturating_sub(1);
                self.settle(-1);
            }
            Act::NextFile => self.step_file(1),
            Act::PreviousFile => self.step_file(-1),
            Act::NextKeyword => self.choose((self.scope + 1).min(self.keywords.len())),
            Act::PreviousKeyword => self.choose(self.scope.saturating_sub(1)),
            Act::Open => return self.open(false),
            Act::OpenBeside => return self.open(true),
            Act::Refresh => return self.search(),
        }
        EditorOutcome::Consumed
    }

    /// Move the cursor `delta` comments, over the files' names.
    fn move_by(&mut self, delta: isize) {
        let step = delta.signum();
        for _ in 0..delta.unsigned_abs() {
            let mut at = self.selected;
            loop {
                match at.checked_add_signed(step) {
                    Some(next) if next < self.rows.len() => at = next,
                    _ => return,
                }
                if matches!(self.rows[at], Line::Todo(_)) {
                    self.selected = at;
                    break;
                }
            }
        }
    }

    /// Put the cursor on the first comment of the file after the one it is
    /// in, or before it (`step` `-1`); the first of its own file when it is
    /// past that going back.
    fn step_file(&mut self, step: isize) {
        let starts: Vec<usize> = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, line)| matches!(line, Line::File(_)))
            .map(|(at, _)| at + 1)
            .collect();
        let target = match step > 0 {
            true => starts.iter().find(|&&start| start > self.selected),
            false => starts.iter().rev().find(|&&start| start < self.selected),
        };
        if let Some(&target) = target {
            self.selected = target;
        }
    }

    /// Show only the keyword of tab `scope`, or all of them at `0`.
    fn choose(&mut self, scope: usize) {
        if scope == self.scope {
            return;
        }
        self.scope = scope;
        self.selected = 0;
        self.rebuild();
    }

    /// Open the selected comment's line, in this pane or `beside` it.
    fn open(&self, beside: bool) -> EditorOutcome {
        let Some(todo) = self.selected_todo() else {
            return EditorOutcome::Consumed;
        };
        let target = OpenTarget::at_line(todo.path.clone(), todo.line);
        match beside {
            true => EditorOutcome::SpawnEditor(target),
            false => EditorOutcome::OpenPath(target),
        }
    }

    /// Select the row at `at`, and open it on a double click.
    fn click(&mut self, at: usize, press: Press) {
        self.selected = at;
        if press.clicks >= DOUBLE_CLICK {
            self.outcome = Some(self.open(false));
        }
    }

    fn selected_todo(&self) -> Option<&Todo> {
        match self.rows.get(self.selected)? {
            Line::Todo(at) => self.found.get(*at),
            Line::File(_) => None,
        }
    }

    /// How many comments are listed, and in how many files.
    fn count(&self) -> String {
        let todos = self
            .rows
            .iter()
            .filter(|line| matches!(line, Line::Todo(_)))
            .count();
        let files = self.rows.len() - todos;
        match (todos, files) {
            (1, _) => "1 comment".to_string(),
            (todos, 1) => format!("{todos} comments in 1 file"),
            (todos, files) => format!("{todos} comments in {files} files"),
        }
    }

    /// The heading's tabs: every keyword, then each one with how many
    /// comments hold it.
    fn scopes(&self) -> Vec<String> {
        let mut names = vec![format!("{ALL} {}", self.found.len())];
        names.extend(self.keywords.iter().enumerate().map(|(at, keyword)| {
            let held = self.found.iter().filter(|todo| todo.keyword == at).count();
            format!("{keyword} {held}")
        }));
        names
    }

    /// The row at `at`: a file's name, or a comment's words with its
    /// keyword's badge and its line number.
    fn row_view(&self, at: usize) -> Element<'_, Self> {
        match self.rows[at] {
            Line::File(first) => text(relative(&self.root, &self.found[first].path))
                .muted()
                .into(),
            Line::Todo(todo) => {
                let row: Element<'_, Self> = self.todo_view(at, &self.found[todo]).into();
                // The page scrolls the selected comment into view.
                match at == self.selected {
                    true => row.keep_in_view(),
                    false => row,
                }
            }
        }
    }

    fn todo_view<'a>(&'a self, at: usize, todo: &'a Todo) -> Item<'a, Self> {
        let keyword = &self.keywords[todo.keyword];
        item(todo.words.as_str())
            .selected(at == self.selected)
            .badge(badge(keyword.as_str(), tone(keyword)))
            .trailing(text(todo.line.to_string()).muted())
            .tooltip(format!("Line {} of {}", todo.line, todo.path.display()))
            .on_pointer_press(move |todos: &mut Self, press: Press| todos.click(at, press))
    }

    /// The selected comment's place and its line as written.
    fn footer(&self) -> Option<Element<'_, Self>> {
        let todo = self.selected_todo()?;
        let place = format!("{}:{}", relative(&self.root, &todo.path), todo.line);
        Some(
            column([text(place).muted().into(), text(todo.source.trim()).into()])
                .gap(Space::Cells(0.25)),
        )
    }
}

impl HostedView for Todos {
    /// The heading with a tab for each keyword, every comment, the page
    /// scrolling them, and the selected one's line at the foot.
    fn view(&self) -> Element<'_, Self> {
        let body: Element<'_, Self> = match (self.rows.is_empty(), self.searching) {
            (true, true) => text(SEARCHING).muted().proportional().into(),
            (true, false) => column([
                text(NOTHING_FOUND).proportional().into(),
                text(NOTHING_FOUND_DETAIL).muted().proportional().into(),
            ]),
            (false, _) => column(
                (0..self.rows.len())
                    .map(|at| self.row_view(at))
                    .collect::<Vec<_>>(),
            )
            .gap(Space::Cells(0.25)),
        };
        let mut shown = page(TITLE)
            .scopes(self.scopes(), self.scope, |todos: &mut Self, scope| {
                todos.choose(scope)
            })
            .scrolling(body);
        if let Some(footer) = self.footer() {
            shown = shown.footer(footer);
        }
        shown.into()
    }
}

// ========================================================================
// Functions
// ========================================================================

/// The comment `hit` is, by the first of `keywords` it holds as a whole
/// word, or `None` for a line holding none.
pub(crate) fn todo_of(hit: &SearchHit, keywords: &[String]) -> Option<Todo> {
    let (keyword, end) = keywords
        .iter()
        .enumerate()
        .filter_map(|(at, keyword)| word_at(&hit.text, keyword).map(|start| (at, start)))
        .min_by_key(|(_, start)| *start)
        .map(|(at, start)| (at, start + keywords[at].len()))?;
    Some(Todo {
        keyword,
        line: hit.line,
        path: hit.path.clone(),
        source: hit.text.clone(),
        words: words_after(&hit.text[end..]),
    })
}

/// Where `word` first stands in `text` whole, in its own case.
fn word_at(text: &str, word: &str) -> Option<usize> {
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(word).map(|(at, _)| at).find(|&at| {
        let before = text[..at].chars().next_back();
        let after = text[at + word.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

/// What a comment says past its keyword: its punctuation and the marks
/// that close a comment dropped.
fn words_after(rest: &str) -> String {
    let mut words = rest.trim_start_matches(LEADING_MARKS).trim_end();
    for mark in TRAILING_MARKS {
        words = words.strip_suffix(mark).unwrap_or(words).trim_end();
    }
    match words.is_empty() {
        true => NO_WORDS.to_string(),
        false => words.to_string(),
    }
}

/// The rows of `found` that tab `scope` keeps, each file's name above its
/// first comment.
fn rows(found: &[Todo], scope: usize) -> Vec<Line> {
    let mut rows = Vec::new();
    let mut file: Option<&Path> = None;
    let kept = found
        .iter()
        .enumerate()
        .filter(|(_, todo)| scope == 0 || todo.keyword == scope - 1);
    for (at, todo) in kept {
        if file != Some(todo.path.as_path()) {
            rows.push(Line::File(at));
            file = Some(todo.path.as_path());
        }
        rows.push(Line::Todo(at));
    }
    rows
}

/// The color a keyword's badge is in: what says something is broken in the
/// danger color, a shortcut taken in the warning one, a note muted, and
/// the rest in the accent.
pub(crate) fn tone(keyword: &str) -> ThemeToken {
    match keyword {
        "BUG" | "FIXME" => ThemeToken::Danger,
        "HACK" | "XXX" => ThemeToken::Warning,
        "NOTE" => ThemeToken::Muted,
        _ => ThemeToken::Accent,
    }
}

/// `path` from `root`, or whole where it is outside.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use misty::testing::TestWindow;

    use super::*;
    use crate::editors::view_tests::{drawn, text_of};

    fn keywords() -> Vec<String> {
        ["TODO", "FIXME"].map(String::from).to_vec()
    }

    fn hit(path: &str, line: usize, text: &str) -> SearchHit {
        SearchHit {
            line,
            path: PathBuf::from("/repo").join(path),
            text: text.to_string(),
        }
    }

    /// The page over `/repo` with `hits` found.
    fn found(hits: Vec<SearchHit>) -> TodoPage {
        let mut page = TodoPage::new(PathBuf::from("/repo"), &keywords());
        assert!(matches!(
            page.on_tick(Instant::now()),
            EditorOutcome::Job(JobRequest::Search(_))
        ));
        page.on_job(JobReply::Search(SearchResult { hits }));
        page
    }

    fn sample() -> TodoPage {
        found(vec![
            hit("src/a.rs", 3, "    // TODO: cache the parse"),
            hit("src/a.rs", 9, "    /* FIXME - off by one */"),
            hit("src/b.py", 1, "# TODO"),
        ])
    }

    fn selected(page: &TodoPage) -> Option<(String, usize)> {
        let todo = page.view.state().selected_todo()?;
        Some((relative(Path::new("/repo"), &todo.path), todo.line))
    }

    /// Press the first keys `act` is bound to.
    fn press(page: &mut TodoPage, act: Act) -> EditorOutcome {
        let (keys, _) = table::menu_typed(&KEYS, act).expect("bound");
        let mut last = EditorOutcome::Ignored;
        for key in keys {
            last = page.on_key(&key);
        }
        last
    }

    #[test]
    fn test_an_item_says_the_words_after_its_keyword_without_the_comments_marks() {
        let todo = |text: &str| todo_of(&hit("a.rs", 1, text), &keywords()).expect("a comment");
        assert_eq!(todo("// TODO: cache the parse").words, "cache the parse");
        assert_eq!(todo("/* FIXME - off by one */").words, "off by one");
        assert_eq!(todo("<!-- TODO: alt text -->").words, "alt text");
        assert_eq!(todo("# TODO").words, NO_WORDS);
        // The first keyword the line holds names it, whichever is listed first.
        assert_eq!(todo("// FIXME: TODO later").keyword, 1);
        assert!(
            todo_of(&hit("a.rs", 1, "let todos = TODOS;"), &keywords()).is_none(),
            "a keyword inside a word is none"
        );
    }

    #[test]
    fn test_each_file_is_named_once_above_its_comments_and_the_cursor_skips_the_names() {
        let mut page = sample();
        assert_eq!(page.view.state().rows.len(), 5, "two names, three comments");
        assert_eq!(selected(&page), Some(("src/a.rs".to_string(), 3)));
        press(&mut page, Act::Down);
        assert_eq!(selected(&page), Some(("src/a.rs".to_string(), 9)));
        press(&mut page, Act::Down);
        assert_eq!(
            selected(&page),
            Some(("src/b.py".to_string(), 1)),
            "over b.py's name"
        );
        press(&mut page, Act::Up);
        press(&mut page, Act::Up);
        press(&mut page, Act::Up);
        assert_eq!(
            selected(&page),
            Some(("src/a.rs".to_string(), 3)),
            "stops at the top"
        );
    }

    #[test]
    fn test_g_g_goes_to_the_first_comment_and_g_to_the_last() {
        let mut page = sample();
        press(&mut page, Act::Last);
        assert_eq!(selected(&page), Some(("src/b.py".to_string(), 1)));
        press(&mut page, Act::First);
        assert_eq!(
            selected(&page),
            Some(("src/a.rs".to_string(), 3)),
            "over a.rs's name"
        );
    }

    #[test]
    fn test_the_file_keys_step_a_file_at_a_time() {
        let mut page = sample();
        press(&mut page, Act::NextFile);
        assert_eq!(selected(&page), Some(("src/b.py".to_string(), 1)));
        press(&mut page, Act::PreviousFile);
        assert_eq!(selected(&page), Some(("src/a.rs".to_string(), 3)));
    }

    #[test]
    fn test_a_keyword_tab_keeps_only_its_comments_and_counts_each() {
        let mut page = sample();
        assert_eq!(page.view.state().scopes(), ["All 3", "TODO 2", "FIXME 1"]);
        press(&mut page, Act::NextKeyword);
        press(&mut page, Act::NextKeyword);
        assert_eq!(page.view.state().count(), "1 comment");
        assert_eq!(selected(&page), Some(("src/a.rs".to_string(), 9)));
        press(&mut page, Act::NextKeyword);
        assert_eq!(page.view.state().scope, 2, "the last tab stays the last");
        press(&mut page, Act::PreviousKeyword);
        press(&mut page, Act::PreviousKeyword);
        assert_eq!(page.view.state().count(), "3 comments in 2 files");
    }

    #[test]
    fn test_enter_opens_the_comments_line_and_o_opens_it_beside() {
        let mut page = sample();
        press(&mut page, Act::Down);
        let at = OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 9);
        assert_eq!(
            press(&mut page, Act::Open),
            EditorOutcome::OpenPath(at.clone())
        );
        assert_eq!(
            press(&mut page, Act::OpenBeside),
            EditorOutcome::SpawnEditor(at)
        );
    }

    #[test]
    fn test_looking_again_keeps_the_cursor_on_the_same_comment() {
        let mut page = sample();
        press(&mut page, Act::Down);
        press(&mut page, Act::Down);
        assert!(matches!(
            press(&mut page, Act::Refresh),
            EditorOutcome::Job(JobRequest::Search(_))
        ));
        // A comment above it went, and one was added in a new file.
        page.on_job(JobReply::Search(SearchResult {
            hits: vec![
                hit("src/0.rs", 2, "// TODO: new"),
                hit("src/b.py", 1, "# TODO"),
            ],
        }));
        assert_eq!(selected(&page), Some(("src/b.py".to_string(), 1)));
    }

    #[test]
    fn test_the_page_shows_the_comments_and_the_selected_ones_line() {
        let mut page = sample();
        let shown = text_of(&drawn(&mut page, &TestWindow::new(1000, 800)));
        for expected in [
            "src/a.rs",
            "cache the parse",
            "FIXME",
            "off by one",
            "TODO 2",
        ] {
            assert!(shown.contains(expected), "{expected:?} in {shown:?}");
        }
        assert!(
            shown.contains("src/a.rs:3") && shown.contains("// TODO: cache the parse"),
            "the footer has the selected comment's place and line: {shown:?}"
        );
    }

    #[test]
    fn test_the_page_scrolls_the_selected_comment_into_view() {
        let hits = (1..=200)
            .map(|line| hit("src/a.rs", line, &format!("// TODO: item {line:03}")))
            .collect();
        let mut page = found(hits);
        let mut window = TestWindow::new(1000, 800);
        // The scroll jumps rather than glides, so one frame shows where it
        // went.
        window.set_reduced_motion(true);
        let shown = text_of(&drawn(&mut page, &window));
        assert!(shown.contains("item 001") && !shown.contains("item 200"));
        press(&mut page, Act::Last);
        drawn(&mut page, &window);
        page.view_about_to_wait(&mut window.cx());
        let shown = text_of(&drawn(&mut page, &window));
        assert!(shown.contains("item 200"), "the last, selected, in view");
        assert!(!shown.contains("item 001"), "the first scrolled away");
    }

    #[test]
    fn test_with_nothing_found_the_page_says_so() {
        let mut page = found(Vec::new());
        let shown = text_of(&drawn(&mut page, &TestWindow::new(1000, 800)));
        assert!(shown.contains(NOTHING_FOUND), "{shown:?}");
    }

    #[test]
    fn test_every_control_says_what_it_is_and_lights_under_the_pointer() {
        let mut page = sample();
        let frame = drawn(&mut page, &TestWindow::new(1000, 800));
        let unnamed = misty::testing::unnamed_controls(&frame);
        assert!(unnamed.is_empty(), "unnamed: {unnamed:?}");
        let mut window = TestWindow::new(1000, 800);
        let unlit = crate::editors::view_tests::unlit(&mut page, &mut window);
        assert!(unlit.is_empty(), "unlit under the pointer: {unlit:?}");
    }
}

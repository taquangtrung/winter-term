//! Grep: the lines under a directory that hold some text, grouped by file and
//! opened straight from the list.

use std::path::{Path, PathBuf};

use crate::model::input::{CursorMove, Key, KeyCode};
use crate::model::page::{
    row_height, row_text, wrap_window, JobReply, JobRequest, OpenTarget, Page, PageContent,
    PageMenuItem, PageOutcome, PageRow, PageSpan, PageStyle, PromptMode, PromptReply,
    PromptRequest, SearchHit, SearchRequest, SearchResult,
};
use crate::model::vim::nav::buffer_end;

// ========================================================================
// Constants
// ========================================================================

/// The one question the page asks.
const ASK_QUERY: &str = "query";

/// Rows of header above the first result.
const HEADER_ROWS: usize = 1;

/// Width the line numbers are padded to, so the texts they label line up.
const LINE_WIDTH: usize = 5;

/// Shown beside the root before anything has been searched for.
const EMPTY_HINT: &str = "/ to search, q to close";

/// The page's title, before a query narrows it.
const TITLE: &str = "Grep";

/// What the menu opened over a row calls each of the entries it offers.
const LABEL_COPY_MATCH: &str = "Copy path:line";
const LABEL_OPEN: &str = "Open at This Line";
const LABEL_OPEN_IN_EDITOR: &str = "Open in $EDITOR";
const LABEL_SEARCH: &str = "New Search...";
const LABEL_SEARCH_AGAIN: &str = "Run This Search Again";

// ========================================================================
// Data Structures
// ========================================================================

/// A text search over a directory tree, and the lines it found.
#[derive(Debug)]
pub struct GrepPage {
    cursor: usize,
    hits: Vec<SearchHit>,
    /// What the last search reported, shown in the header until the next key.
    message: Option<String>,
    /// The text last searched for, which a new question starts out holding.
    query: String,
    root: PathBuf,
    rows: Vec<ResultRow>,
    scroll: usize,
    /// Whether a walk is still out, so the header can say the list is partial.
    searching: bool,
    /// Whether the walk stopped at its cap with matches left unreported.
    truncated: bool,
}

/// One painted row: its spans, the hit it leads to, and whether it is the
/// heading a file's matches sit under.
#[derive(Debug)]
struct ResultRow {
    /// Whether this row names a file rather than showing one of its lines.
    heading: bool,
    /// The hit acting on this row opens: a heading opens its first one, since
    /// landing on a file's first match beats landing on its first line.
    hit: usize,
    spans: PageRow,
}

// ========================================================================
// GrepPage
// ========================================================================

impl GrepPage {
    /// An empty search over `root`, waiting to be asked for something.
    pub fn new(root: PathBuf) -> Self {
        Self {
            cursor: 0,
            hits: Vec::new(),
            message: None,
            query: String::new(),
            root,
            rows: Vec::new(),
            scroll: 0,
            searching: false,
            truncated: false,
        }
    }

    /// Ask what to look for, starting from what was looked for last so that
    /// narrowing a query is an edit rather than a retype.
    fn ask_query(&self) -> PageOutcome {
        PageOutcome::Prompt(PromptRequest {
            initial: self.query.clone(),
            label: "grep: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_QUERY,
        })
    }

    /// Look for `query`, dropping what the last search found: a result list
    /// half from one query and half from another would answer neither.
    fn start_search(&mut self, query: &str) -> PageOutcome {
        if query.is_empty() {
            return PageOutcome::Consumed;
        }
        self.query = query.to_string();
        self.hits.clear();
        self.rows.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.message = None;
        self.searching = true;
        self.truncated = false;
        PageOutcome::Job(JobRequest::Search(SearchRequest {
            query: self.query.clone(),
            root: self.root.clone(),
        }))
    }

    /// Take what a walk found, unless it answers a query already replaced: two
    /// walks can be out at once, and the slower one would otherwise paint its
    /// matches under the newer query's name.
    fn on_found(&mut self, found: SearchResult) {
        if found.query != self.query {
            return;
        }
        self.searching = false;
        self.truncated = found.truncated;
        self.hits = found.hits;
        self.rebuild();
        if self.hits.is_empty() {
            self.message = Some(format!("no matches for {}", self.query));
        }
    }

    /// Repaint the rows: a heading for each file, then the lines it matched on.
    fn rebuild(&mut self) {
        let mut rows = Vec::new();
        let mut painted: Option<&Path> = None;
        for (index, hit) in self.hits.iter().enumerate() {
            if painted != Some(hit.path.as_path()) {
                rows.push(ResultRow {
                    heading: true,
                    hit: index,
                    spans: heading_row(&self.root, &hit.path),
                });
                painted = Some(hit.path.as_path());
            }
            rows.push(ResultRow {
                heading: false,
                hit: index,
                spans: hit_row(hit),
            });
        }
        self.rows = rows;
        self.cursor = self.cursor.min(self.rows.len().saturating_sub(1));
    }

    fn selected(&self) -> Option<&SearchHit> {
        let row = self.rows.get(self.cursor)?;
        self.hits.get(row.hit)
    }

    fn move_by(&mut self, delta: isize) {
        let last = self.rows.len().saturating_sub(1);
        let next = self.cursor as isize + delta;
        self.cursor = next.clamp(0, last as isize) as usize;
    }

    /// Move to the next or previous row of one kind: `heading` for stepping over
    /// a file with a hundred matches in it, and the other way for stepping
    /// between matches without landing on the names between them.
    fn move_to_row(&mut self, forward: bool, heading: bool) {
        let step: isize = if forward { 1 } else { -1 };
        let mut index = self.cursor as isize + step;
        while index >= 0 && (index as usize) < self.rows.len() {
            if self.rows[index as usize].heading == heading {
                self.cursor = index as usize;
                return;
            }
            index += step;
        }
    }

    /// Open the file the cursor is on, at the line that matched.
    fn open_selected(&self) -> PageOutcome {
        match self.selected() {
            Some(hit) => PageOutcome::OpenPath(OpenTarget::at_line(hit.path.clone(), hit.line)),
            None => PageOutcome::Consumed,
        }
    }

    /// Hand the file the cursor is on to `$EDITOR`, in a pane of its own, at
    /// the line that matched.
    fn open_external(&self) -> PageOutcome {
        match self.selected() {
            Some(hit) => PageOutcome::SpawnEditor(OpenTarget::at_line(hit.path.clone(), hit.line)),
            None => PageOutcome::Consumed,
        }
    }

    /// Copy where the match is, in the `path:line` form every tool understands.
    fn yank_selected(&self) -> PageOutcome {
        match self.selected() {
            Some(hit) => PageOutcome::Yank(format!(
                "{}:{}",
                relative_to(&self.root, &hit.path),
                hit.line
            )),
            None => PageOutcome::Consumed,
        }
    }

    /// The root, what was searched for, and how much of the tree it matched.
    fn header_row(&self) -> PageRow {
        let mut spans = vec![PageSpan::new(
            PageStyle::Header,
            self.root.to_string_lossy(),
        )];
        if self.query.is_empty() {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  {EMPTY_HINT}")));
            return spans;
        }
        spans.push(PageSpan::new(
            PageStyle::Accent,
            format!("  /{}", self.query),
        ));
        let detail = if self.searching {
            "  searching...".to_string()
        } else {
            let files = self.rows.iter().filter(|row| row.heading).count();
            let capped = if self.truncated { ", capped" } else { "" };
            format!("  {} in {files} files{capped}", matches(self.hits.len()))
        };
        spans.push(PageSpan::new(PageStyle::Dim, detail));
        if let Some(message) = &self.message {
            spans.push(PageSpan::new(PageStyle::Dim, format!("  {message}")));
        }
        spans
    }
}

impl Page for GrepPage {
    fn title(&self) -> String {
        if self.query.is_empty() {
            return TITLE.to_string();
        }
        format!("{TITLE}: {}", self.query)
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> PageContent {
        let mut page_rows = vec![self.header_row()];
        if self.rows.is_empty() {
            return PageContent::new(page_rows);
        }
        let texts: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
        let window = wrap_window(
            self.scroll,
            self.cursor,
            texts.len(),
            rows.saturating_sub(HEADER_ROWS),
            |index| row_height(&texts[index], cols, wrap, 0),
        );
        self.scroll = window.start;
        page_rows.extend(
            self.rows
                .iter()
                .skip(window.start)
                .take(window.count)
                .map(|row| row.spans.clone()),
        );
        PageContent::new(page_rows).with_cursor_line(HEADER_ROWS + window.cursor)
    }

    fn on_key(&mut self, key: &Key) -> PageOutcome {
        self.message = None;
        if key.ctrl && key.code == KeyCode::Char('o') {
            return self.open_external();
        }
        if key.alt {
            if let Some(motion) = buffer_end(key) {
                self.cursor = match motion {
                    CursorMove::Top => 0,
                    _ => self.rows.len().saturating_sub(1),
                };
                return PageOutcome::Consumed;
            }
            return match key.code {
                KeyCode::Char('n') => {
                    self.move_to_row(true, true);
                    PageOutcome::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_to_row(false, true);
                    PageOutcome::Consumed
                }
                _ => PageOutcome::Ignored,
            };
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_by(1);
                PageOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_by(-1);
                PageOutcome::Consumed
            }
            // `n` and `N` mean the next and previous match here, as they do in
            // every other tool: the list is already the search, so there is
            // nothing else for them to step through.
            KeyCode::Char('n') => {
                self.move_to_row(true, false);
                PageOutcome::Consumed
            }
            KeyCode::Char('N') => {
                self.move_to_row(false, false);
                PageOutcome::Consumed
            }
            KeyCode::Home => {
                self.cursor = 0;
                PageOutcome::Consumed
            }
            KeyCode::End => {
                self.cursor = self.rows.len().saturating_sub(1);
                PageOutcome::Consumed
            }
            // `/` asks for a new search here rather than searching the results:
            // the rows already are a search, and narrowing it is what a second
            // query is for.
            KeyCode::Char('/') => self.ask_query(),
            KeyCode::Char('G') => {
                let query = self.query.clone();
                self.start_search(&query)
            }
            KeyCode::Enter => self.open_selected(),
            KeyCode::Char('y') => self.yank_selected(),
            // A walk over a large tree is worth abandoning, and the key that
            // means stop is the one to abandon it with.
            KeyCode::Escape if self.searching => {
                self.searching = false;
                self.message = Some("search stopped".to_string());
                PageOutcome::CancelJobs
            }
            KeyCode::Char('q') | KeyCode::Escape => PageOutcome::Close,
            _ => PageOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> PageOutcome {
        match reply.answer {
            Some(answer) => self.start_search(&answer),
            None => PageOutcome::Consumed,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> PageOutcome {
        match reply {
            JobReply::Search(found) => self.on_found(found),
            // The page runs no commands and asks for no directory totals.
            JobReply::Command(_) | JobReply::DirSize { .. } | JobReply::Files(_) => {}
        }
        PageOutcome::Consumed
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The file the cursor is in, falling back to what was searched when
        // the cursor is on a heading or there is nothing to point at.
        self.selected()
            .and_then(|hit| hit.path.parent())
            .map(PathBuf::from)
            .or_else(|| Some(self.root.clone()))
    }

    fn context_items(&self) -> Vec<PageMenuItem> {
        // Everything here acts on the match under the cursor, so a row that
        // is not one (a file heading, the header) offers only a new search.
        let mut items = Vec::new();
        if self.selected().is_some() {
            items.push(PageMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN));
            items.push(PageMenuItem::new(
                Key::with_ctrl(KeyCode::Char('o')),
                LABEL_OPEN_IN_EDITOR,
            ));
            items.push(PageMenuItem::new(
                Key::plain(KeyCode::Char('y')),
                LABEL_COPY_MATCH,
            ));
        }
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('/')),
            LABEL_SEARCH,
        ));
        items.push(PageMenuItem::new(
            Key::plain(KeyCode::Char('G')),
            LABEL_SEARCH_AGAIN,
        ));
        items
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// A file's row: its path, shortened against the root it was searched under.
fn heading_row(root: &Path, path: &Path) -> PageRow {
    vec![PageSpan::new(PageStyle::Header, relative_to(root, path))]
}

/// A match's row: the line number, then the line.
fn hit_row(hit: &SearchHit) -> PageRow {
    vec![
        PageSpan::new(PageStyle::Dim, format!("{:>LINE_WIDTH$}: ", hit.line)),
        PageSpan::plain(hit.text.clone()),
    ]
}

/// `path` written against `root`, falling back to the whole path when it sits
/// outside: a shorter name is worth more than a uniform one.
fn relative_to(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

/// `n matches`, singular where it should be.
fn matches(count: usize) -> String {
    if count == 1 {
        return "1 match".to_string();
    }
    format!("{count} matches")
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode) -> Key {
        Key {
            alt: false,
            code,
            ctrl: false,
            shift: false,
        }
    }

    fn alt(code: KeyCode) -> Key {
        Key {
            alt: true,
            code,
            ctrl: false,
            shift: false,
        }
    }

    #[test]
    fn test_the_emacs_buffer_ends_reach_the_first_and_last_row() {
        let mut page = page_with(vec![
            hit("a.rs", 1, "thing"),
            hit("a.rs", 9, "thing again"),
            hit("b.rs", 3, "thing"),
        ]);
        page.on_key(&alt(KeyCode::Char('>')));
        assert_eq!(page.cursor, page.rows.len() - 1);
        page.on_key(&alt(KeyCode::Char('<')));
        assert_eq!(page.cursor, 0);
    }

    fn hit(path: &str, line: usize, text: &str) -> SearchHit {
        SearchHit {
            line,
            path: PathBuf::from("/repo").join(path),
            text: text.to_string(),
        }
    }

    /// A page holding `hits`, as if a walk for "thing" had just answered.
    fn page_with(hits: Vec<SearchHit>) -> GrepPage {
        let mut page = GrepPage::new(PathBuf::from("/repo"));
        page.query = "thing".to_string();
        page.on_job(JobReply::Search(SearchResult {
            hits,
            query: "thing".to_string(),
            truncated: false,
        }));
        page
    }

    #[test]
    fn test_the_start_directory_follows_the_hit_under_the_cursor() {
        // A tool opened from a result should land beside the file being read,
        // not at the top of whatever tree was searched, so stepping onto a
        // hit in another directory has to move the answer with it.
        let mut page = page_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("crates/deep/b.rs", 9, "thing"),
        ]);
        page.move_by(1);
        assert_eq!(page.cwd(), Some(PathBuf::from("/repo/src")));
        while page.selected().map(|h| h.line) != Some(9) {
            page.move_by(1);
        }
        assert_eq!(page.cwd(), Some(PathBuf::from("/repo/crates/deep")));
    }

    #[test]
    fn test_a_search_with_nothing_to_point_at_starts_where_it_searched() {
        let page = GrepPage::new(PathBuf::from("/repo"));
        assert_eq!(page.cwd(), Some(PathBuf::from("/repo")));
    }

    /// Ask for `text` through the query prompt, the way a key does.
    fn ask(page: &mut GrepPage, text: &str) -> PageOutcome {
        let outcome = page.on_key(&press(KeyCode::Char('/')));
        let PageOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_QUERY);
        page.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        })
    }

    fn row_texts(page: &GrepPage) -> Vec<String> {
        page.rows
            .iter()
            .map(|row| {
                row.spans
                    .iter()
                    .map(|span| span.text.as_str())
                    .collect::<String>()
            })
            .collect()
    }

    #[test]
    fn test_a_query_starts_a_walk_under_the_root() {
        let mut page = GrepPage::new(PathBuf::from("/repo"));
        let outcome = ask(&mut page, "needle");
        assert_eq!(
            outcome,
            PageOutcome::Job(JobRequest::Search(SearchRequest {
                query: "needle".to_string(),
                root: PathBuf::from("/repo"),
            }))
        );
        assert!(page.searching, "the header has to be able to say so");
    }

    #[test]
    fn test_an_empty_query_searches_for_nothing() {
        // A walk for "" would report every line of every file under the root.
        let mut page = GrepPage::new(PathBuf::from("/repo"));
        assert_eq!(ask(&mut page, ""), PageOutcome::Consumed);
        assert!(!page.searching);
    }

    #[test]
    fn test_matches_are_grouped_under_one_heading_per_file() {
        let page = page_with(vec![
            hit("src/a.rs", 4, "one thing"),
            hit("src/a.rs", 9, "another thing"),
            hit("src/b.rs", 2, "thing again"),
        ]);
        let texts = row_texts(&page);
        assert_eq!(texts.len(), 5, "two headings and three matches: {texts:?}");
        assert_eq!(texts[0].trim(), "src/a.rs");
        assert_eq!(texts[1].trim(), "4: one thing");
        assert_eq!(texts[3].trim(), "src/b.rs");
    }

    #[test]
    fn test_enter_opens_the_file_at_the_line_that_matched() {
        let mut page = page_with(vec![hit("src/a.rs", 4, "one thing")]);
        // Row 0 is the heading, row 1 the match.
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 4))
        );
    }

    #[test]
    fn test_enter_on_a_heading_opens_that_file_at_its_first_match() {
        // Opening it at line one would throw away what the user searched for.
        let mut page = page_with(vec![
            hit("src/a.rs", 12, "one thing"),
            hit("src/a.rs", 30, "another thing"),
        ]);
        assert_eq!(
            page.on_key(&press(KeyCode::Enter)),
            PageOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 12))
        );
    }

    #[test]
    fn test_the_file_motions_step_over_a_file_rather_than_through_it() {
        let mut page = page_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("src/a.rs", 2, "thing"),
            hit("src/b.rs", 3, "thing"),
        ]);
        page.on_key(&alt(KeyCode::Char('n')));
        assert_eq!(row_texts(&page)[page.cursor].trim(), "src/b.rs");

        page.on_key(&alt(KeyCode::Char('p')));
        assert_eq!(row_texts(&page)[page.cursor].trim(), "src/a.rs");
    }

    #[test]
    fn test_the_match_motions_skip_the_names_between_the_matches() {
        let mut page = page_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("src/b.rs", 3, "thing"),
        ]);
        // Rows: heading, match, heading, match. The cursor starts on the first
        // heading, so one step lands on a match and the next skips a heading.
        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(row_texts(&page)[page.cursor].trim(), "1: thing");

        page.on_key(&press(KeyCode::Char('n')));
        assert_eq!(row_texts(&page)[page.cursor].trim(), "3: thing");

        page.on_key(&press(KeyCode::Char('N')));
        assert_eq!(row_texts(&page)[page.cursor].trim(), "1: thing");
    }

    #[test]
    fn test_an_answer_to_a_query_already_replaced_is_dropped() {
        // Two walks can be out at once, and the slower one would otherwise
        // paint its matches under the newer query's name.
        let mut page = page_with(vec![hit("src/a.rs", 1, "thing")]);
        page.query = "other".to_string();

        page.on_job(JobReply::Search(SearchResult {
            hits: vec![hit("src/b.rs", 9, "thing")],
            query: "thing".to_string(),
            truncated: false,
        }));
        assert_eq!(row_texts(&page).len(), 2, "the older answer was ignored");
        assert_eq!(row_texts(&page)[1].trim(), "1: thing");
    }

    #[test]
    fn test_a_walk_that_found_nothing_says_so() {
        let page = page_with(Vec::new());
        assert_eq!(page.message.as_deref(), Some("no matches for thing"));
        assert!(page.rows.is_empty());
    }

    #[test]
    fn test_a_capped_walk_says_the_list_is_partial() {
        // Without this the first five hundred matches look like all of them.
        let mut page = GrepPage::new(PathBuf::from("/repo"));
        page.query = "thing".to_string();
        page.on_job(JobReply::Search(SearchResult {
            hits: vec![hit("src/a.rs", 1, "thing")],
            query: "thing".to_string(),
            truncated: true,
        }));
        let header: String = page
            .header_row()
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        assert!(header.contains("capped"), "header reads: {header}");
    }

    #[test]
    fn test_a_new_query_drops_what_the_last_one_found() {
        // Results half from one query and half from another answer neither.
        let mut page = page_with(vec![hit("src/a.rs", 1, "thing")]);
        ask(&mut page, "other");
        assert!(page.rows.is_empty());
        assert_eq!(page.query, "other");
    }

    #[test]
    fn test_escape_stops_a_running_walk_instead_of_closing_the_page() {
        let mut page = GrepPage::new(PathBuf::from("/repo"));
        ask(&mut page, "needle");
        assert_eq!(
            page.on_key(&press(KeyCode::Escape)),
            PageOutcome::CancelJobs
        );
        assert_eq!(page.on_key(&press(KeyCode::Escape)), PageOutcome::Close);
    }

    #[test]
    fn test_yanking_copies_where_the_match_is() {
        let mut page = page_with(vec![hit("src/a.rs", 4, "one thing")]);
        page.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            page.on_key(&press(KeyCode::Char('y'))),
            PageOutcome::Yank("src/a.rs:4".to_string())
        );
    }
}

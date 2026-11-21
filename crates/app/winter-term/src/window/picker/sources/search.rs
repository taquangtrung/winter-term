//! The searches: project search (`C-M-g`), which ripgrep runs for each
//! pattern as the typing pauses, and the Todo picker, a ripgrep search for
//! comment keywords filtered by what is typed. Each row is two lines: where
//! the line is (`mod.rs:184:13`, its directory after), then the line itself.
//!
//! Project search reads its pattern as written, a `*` standing for anything,
//! case ignored until the pattern has a capital, and keeps only the files a
//! filter after ` @ ` names: a glob (`src/*.rs`) or a part of the path.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex, PoisonError};

use misty::picker::{
    Action, ActionId, Feed, Highlights, MatchOn, Matching, PreviewKind, Resume, Row, RowKey, Source,
};
use regex::{Regex, RegexBuilder};
use winter_base::process;
use winter_model::editor::{OpenTarget, SearchHit};
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;

use super::stream::{place_key, stream_lines, stream_rows};
use crate::window::picker::WindowOutcome;
use crate::window::picker::outcome::SaveHits;
use crate::window::picker::replace::Replace;
use crate::window::picker::sources::IconRow;
use crate::window::picker::{FilePreview, Preview};
use crate::window::runtime::jobs::search::pdfs_under;

// ========================================================================
// Constants
// ========================================================================

/// The program searched with.
const RIPGREP: &str = "rg";

/// Said when ripgrep is not there to search with.
const NO_RIPGREP: &str = "searching needs ripgrep (rg) installed";

/// The shortest pattern searched for: one character matches nearly every
/// line of a project.
const MIN_PATTERN_CHARS: usize = 2;

/// The longest line text kept, in chars, and how many chars of it a row
/// shows around its match.
const MAX_LINE_CHARS: usize = 300;
const SHOWN_CHARS: usize = 120;

/// What a `*` in a pattern stands for in the regex it becomes.
const WILDCARD_REGEX: &str = ".*";

/// What the PDF search's saved lines are called, and what it says when it
/// cannot start.
const PDF_SEARCH_NAME: &str = "PDF Text";
const NO_PDF_SEARCH: &str = "could not start searching the PDFs";

/// What starts a project search's file filter.
const FILTER_MARK: &str = " @";

/// The actions of a search's row: open the file at the line, or beside.
const OPEN: ActionId = ActionId("open");
const OPEN_BESIDE: ActionId = ActionId("open_beside");

/// The project search's action replacing in every line it lists.
pub(in crate::window) const REPLACE: ActionId = ActionId("replace");

// ========================================================================
// Data Structures
// ========================================================================

/// The project, searched with ripgrep for the pattern typed.
pub(in crate::window) struct ProjectSearch {
    /// The places the last pattern found, in the order found, those the
    /// file filter hid among them: a change of filter alone is answered from
    /// these, not by searching again.
    found: Arc<Mutex<Vec<RowKey>>>,
    hits: Hits,
    /// The pattern last searched for.
    pattern: Option<String>,
    root: PathBuf,
}

/// The text of the PDFs under the project, searched for what is typed.
/// Each PDF's text is read once, the first time a search reaches it, and
/// kept, so typing more searches what was read rather than reading again.
pub(in crate::window) struct PdfSearch {
    hits: Hits,
    root: PathBuf,
    /// Each PDF's text read so far, a page's lines at a time.
    texts: PdfTexts,
}

/// Each PDF's text, by its path: its pages, each its lines.
type PdfTexts = Arc<Mutex<HashMap<PathBuf, Arc<Vec<Vec<String>>>>>>;

/// The comment keywords under the project, filtered by what is typed.
pub(in crate::window) struct TodoSearch {
    hits: Hits,
    /// The `todo-keywords` setting's words, as the Todo page searches for.
    keywords: Vec<String>,
    root: PathBuf,
}

/// The lines found, by their rows, filled as the stream sends them.
type Hits = Arc<Mutex<HashMap<RowKey, Hit>>>;

/// One line found.
#[derive(Clone, Debug, Eq, PartialEq)]
struct Hit {
    column: usize,
    line: usize,
    /// From the project's root, `/`-separated.
    path: String,
    text: String,
}

/// Which files a project search keeps, matched against a row's path without
/// case: a glob where the filter has `*` or `?` in it, else a part of it.
enum FileFilter {
    Glob(Regex),
    Part(String),
}

// ========================================================================
// ProjectSearch
// ========================================================================

impl ProjectSearch {
    /// A search over the project at `root`.
    pub(in crate::window) fn new(root: PathBuf) -> Self {
        Self {
            found: Arc::default(),
            hits: Hits::default(),
            pattern: None,
            root,
        }
    }

    /// The rows of what the last pattern found that `filter` keeps, in the
    /// order they were found.
    fn refiltered(&self, filter: Option<&FileFilter>) -> Vec<Row> {
        let found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        let hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        found
            .iter()
            .filter_map(|key| Some((*key, hits.get(key)?)))
            .filter(|(_, hit)| filter.is_none_or(|filter| filter.matches(&hit.path)))
            .map(|(key, hit)| place_row(key, hit))
            .collect()
    }

    /// The replacement asked for over every line listed for `query`: those
    /// its pattern found that its file filter keeps.
    fn replace(&self, query: &str) -> WindowOutcome {
        let (pattern, filter) = split_query(query);
        let filter = filter.map(FileFilter::new);
        let found = self.found.lock().unwrap_or_else(PoisonError::into_inner);
        let hits = self.hits.lock().unwrap_or_else(PoisonError::into_inner);
        let listed = found
            .iter()
            .filter_map(|key| hits.get(key))
            .filter(|hit| {
                filter
                    .as_ref()
                    .is_none_or(|filter| filter.matches(&hit.path))
            })
            .map(|hit| SearchHit {
                line: hit.line,
                path: self.root.join(&hit.path),
                text: hit.text.clone(),
            })
            .collect();
        WindowOutcome::AskReplace(Replace {
            hits: listed,
            pattern: pattern.trim().to_string(),
        })
    }
}

impl Source<WindowOutcome, Preview> for ProjectSearch {
    fn id(&self) -> &'static str {
        "project_search"
    }

    /// Every match leads with its file's icon.
    fn shows_icons(&self) -> bool {
        true
    }

    fn title(&self) -> String {
        format!("Search: {}", abbreviate_home(&self.root))
    }

    fn placeholder(&self) -> String {
        "Pattern; @ glob narrows the files".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::BySource(MatchOn::DETAIL | MatchOn::LABEL)
    }

    fn resume(&self) -> Resume {
        Resume::Continue
    }

    fn preview_kind(&self) -> PreviewKind {
        PreviewKind::Panel
    }

    fn feed(&mut self, query: &str) -> Feed {
        let (pattern, filter) = split_query(query);
        if pattern.trim().chars().count() < MIN_PATTERN_CHARS {
            return Feed::Fixed(Vec::new());
        }
        let filter = filter.map(FileFilter::new);
        if self.pattern.as_deref() == Some(pattern) {
            return Feed::Fixed(self.refiltered(filter.as_ref()));
        }
        self.pattern = Some(pattern.to_string());
        self.found = Arc::default();
        let mut command = process::command(RIPGREP);
        command.args(rg_args(pattern)).current_dir(&self.root);
        let (hits, found) = (Arc::clone(&self.hits), Arc::clone(&self.found));
        stream_lines(command, NO_RIPGREP, move |line| {
            let hit = parse_hit(line)?;
            let shown = filter
                .as_ref()
                .is_none_or(|filter| filter.matches(&hit.path));
            let row = keep_hit(&hits, hit);
            found
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(row.key);
            shown.then_some(row)
        })
    }

    fn highlights(&self, row: &Row, query: &str) -> Highlights {
        let (pattern, filter) = split_query(query);
        let mut lit = Highlights {
            detail: pattern_positions(&row.detail, pattern),
            ..Highlights::default()
        };
        if let Some(FileFilter::Part(part)) = filter.map(FileFilter::new) {
            lit.label = part_positions(&row.label, &part);
            lit.description = part_positions(&row.description, &part);
        }
        lit
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        let mut actions = open_actions();
        actions.push(Action {
            button: None,
            id: REPLACE,
            title: "Replace in Every Line Listed…".to_string(),
        });
        actions
    }

    fn run(&mut self, action: ActionId, row: RowKey, query: &str) -> Option<WindowOutcome> {
        match action {
            REPLACE => Some(self.replace(query)),
            _ => open_hit(&self.hits, &self.root, action, row),
        }
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        preview_hit(&self.hits, &self.root, row)
    }

    /// Search again on resume, for what changed since.
    fn refresh(&mut self) {
        self.pattern = None;
    }

    fn export(&self, rows: &[RowKey], query: &str) -> Option<WindowOutcome> {
        let hits = saved_hits(&self.hits, &self.root, rows);
        Some(WindowOutcome::SaveHits(SaveHits {
            hits,
            name: self.name(),
            query: query.trim().to_string(),
        }))
    }
}

// ========================================================================
// PdfSearch
// ========================================================================

impl PdfSearch {
    /// A search over the text of the PDFs under `root`.
    pub(in crate::window) fn new(root: PathBuf) -> Self {
        Self {
            hits: Hits::default(),
            root,
            texts: PdfTexts::default(),
        }
    }
}

impl Source<WindowOutcome, Preview> for PdfSearch {
    fn id(&self) -> &'static str {
        "pdf_search"
    }

    /// Every match leads with its file's icon.
    fn shows_icons(&self) -> bool {
        true
    }

    fn title(&self) -> String {
        format!("PDF Text: {}", abbreviate_home(&self.root))
    }

    fn name(&self) -> String {
        PDF_SEARCH_NAME.to_string()
    }

    fn placeholder(&self) -> String {
        "Text in a PDF".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::BySource(MatchOn::DETAIL | MatchOn::LABEL)
    }

    fn resume(&self) -> Resume {
        Resume::Continue
    }

    fn preview_kind(&self) -> PreviewKind {
        PreviewKind::None
    }

    fn feed(&mut self, query: &str) -> Feed {
        let pattern = query.trim();
        if pattern.chars().count() < MIN_PATTERN_CHARS {
            return Feed::Fixed(Vec::new());
        }
        let Ok(regex) = pattern_regex(pattern) else {
            return Feed::Fixed(Vec::new());
        };
        let (hits, texts) = (Arc::clone(&self.hits), Arc::clone(&self.texts));
        let root = self.root.clone();
        stream_rows(NO_PDF_SEARCH, move |send| {
            let cancel = AtomicBool::new(false);
            for pdf in pdfs_under(&root, &cancel) {
                let Some(pages) = pdf_text(&texts, &pdf) else {
                    continue;
                };
                let path = relative(&root, &pdf);
                for (index, lines) in pages.iter().enumerate() {
                    for line in lines {
                        let Some(found) = regex.find(line) else {
                            continue;
                        };
                        let hit = Hit {
                            column: line[..found.start()].chars().count() + 1,
                            line: index + 1,
                            path: path.clone(),
                            text: line.trim_end().chars().take(MAX_LINE_CHARS).collect(),
                        };
                        if !send(keep_pdf_hit(&hits, hit)) {
                            return;
                        }
                    }
                }
            }
        })
    }

    fn highlights(&self, row: &Row, query: &str) -> Highlights {
        Highlights {
            detail: pattern_positions(&row.detail, query.trim()),
            ..Highlights::default()
        }
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        open_actions()
    }

    /// Open the PDF at the page the line is on.
    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        let hit = hit_of(&self.hits, row)?;
        let target = OpenTarget::at_line(self.root.join(&hit.path), hit.line);
        match action {
            OPEN => Some(WindowOutcome::OpenFile(target)),
            OPEN_BESIDE => Some(WindowOutcome::OpenFileBeside(target)),
            _ => None,
        }
    }

    fn export(&self, rows: &[RowKey], query: &str) -> Option<WindowOutcome> {
        let hits = saved_hits(&self.hits, &self.root, rows);
        Some(WindowOutcome::SaveHits(SaveHits {
            hits,
            name: self.name(),
            query: query.trim().to_string(),
        }))
    }
}

// ========================================================================
// TodoSearch
// ========================================================================

impl TodoSearch {
    /// The comment `keywords` under the project at `root`.
    pub(in crate::window) fn new(root: PathBuf, keywords: &[String]) -> Self {
        Self {
            hits: Hits::default(),
            keywords: keywords.to_vec(),
            root,
        }
    }
}

impl Source<WindowOutcome, Preview> for TodoSearch {
    fn id(&self) -> &'static str {
        "todo"
    }

    fn title(&self) -> String {
        format!("TODOs: {}", abbreviate_home(&self.root))
    }

    fn placeholder(&self) -> String {
        "Filter TODOs".to_string()
    }

    fn matching(&self) -> Matching {
        Matching::Words(MatchOn::DETAIL)
    }

    /// The keyword found, lit in its comment beside the words typed.
    fn highlights(&self, row: &Row, _query: &str) -> Highlights {
        Highlights {
            detail: keyword_positions(&row.detail, &self.keywords),
            ..Highlights::default()
        }
    }

    fn resume(&self) -> Resume {
        Resume::Continue
    }

    fn preview_kind(&self) -> PreviewKind {
        PreviewKind::Panel
    }

    fn feed(&mut self, _query: &str) -> Feed {
        let keywords = format!(r"\b({})\b:?", keyword_alternation(&self.keywords));
        let mut command = process::command(RIPGREP);
        command
            .args(base_args())
            .args(["--regexp", &keywords])
            .current_dir(&self.root);
        let hits = Arc::clone(&self.hits);
        stream_lines(command, NO_RIPGREP, move |line| {
            Some(keep_hit(&hits, parse_hit(line)?))
        })
    }

    fn actions(&self, _row: RowKey) -> Vec<Action> {
        open_actions()
    }

    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        open_hit(&self.hits, &self.root, action, row)
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        preview_hit(&self.hits, &self.root, row)
    }

    fn export(&self, rows: &[RowKey], query: &str) -> Option<WindowOutcome> {
        let hits = saved_hits(&self.hits, &self.root, rows);
        Some(WindowOutcome::SaveHits(SaveHits {
            hits,
            name: self.name(),
            query: query.trim().to_string(),
        }))
    }
}

// ========================================================================
// FileFilter
// ========================================================================

impl FileFilter {
    fn new(filter: &str) -> Self {
        if !filter.contains(['*', '?']) {
            return Self::Part(filter.to_lowercase());
        }
        let pattern: String = filter
            .chars()
            .map(|c| match c {
                '*' => WILDCARD_REGEX.to_string(),
                '?' => ".".to_string(),
                c => regex::escape(&c.to_string()),
            })
            .collect();
        match RegexBuilder::new(&pattern).case_insensitive(true).build() {
            Ok(glob) => Self::Glob(glob),
            Err(_) => Self::Part(filter.to_lowercase()),
        }
    }

    fn matches(&self, path: &str) -> bool {
        match self {
            Self::Glob(glob) => glob.is_match(path),
            Self::Part(part) => path.to_lowercase().contains(part.as_str()),
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// A project search's query taken apart: the pattern, and the file filter
/// after its last ` @`, when there is one.
fn split_query(query: &str) -> (&str, Option<&str>) {
    match query.rsplit_once(FILTER_MARK) {
        Some((pattern, filter)) => (pattern.trim_end(), Some(filter.trim())),
        None => (query.trim_end(), None),
    }
}

/// What every search runs ripgrep with: one line per hit with its line and
/// column, its path ended by a NUL so a `:` in a name cannot be misread,
/// hidden files searched but version control's own directories not, and
/// the project's own ignore files followed but none above it.
fn base_args() -> [&'static str; 10] {
    [
        "--line-number",
        "--column",
        "--no-heading",
        "--with-filename",
        "--null",
        "--color=never",
        "--hidden",
        // An ignore file above the project would otherwise outrank the
        // project's own, and its build output would be searched.
        "--no-ignore-parent",
        "--glob=!.git",
        "--glob=!.hg",
    ]
}

/// What ripgrep is run with for `pattern`: case ignored unless it has a
/// capital in it, and a `*` making it a regex, each standing for anything.
fn rg_args(pattern: &str) -> Vec<String> {
    let mut args: Vec<String> = base_args().map(String::from).to_vec();
    args.push("--smart-case".into());
    match pattern.contains('*') {
        true => {
            let regex: Vec<String> = pattern.split('*').map(regex::escape).collect();
            args.push("--regexp".into());
            args.push(regex.join(WILDCARD_REGEX));
        }
        false => {
            args.push("--fixed-strings".into());
            args.push("--regexp".into());
            args.push(pattern.to_string());
        }
    }
    args
}

/// One line of ripgrep's output: `path NUL line:column:text`.
fn parse_hit(line: &[u8]) -> Option<Hit> {
    let split = line.iter().position(|byte| *byte == 0)?;
    let path = String::from_utf8_lossy(&line[..split]);
    let path = path.strip_prefix("./").unwrap_or(&path).replace('\\', "/");
    let rest = String::from_utf8_lossy(&line[split + 1..]);
    let mut parts = rest.splitn(3, ':');
    let line = parts.next()?.parse().ok()?;
    let column = parts.next()?.parse().ok()?;
    let text = parts.next()?;
    Some(Hit {
        column,
        line,
        path,
        text: text.trim_end().chars().take(MAX_LINE_CHARS).collect(),
    })
}

/// Keep `hit` for its row, and make the row: its file's name and place,
/// its directory after, and its line under, cut around the match.
fn keep_hit(hits: &Hits, hit: Hit) -> Row {
    let key = place_key(&hit.path, hit.line, hit.column);
    let row = place_row(key, &hit);
    hits.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, hit);
    row
}

/// The row of a line found: `mod.rs:184:13` and its directory, then the
/// line, indentation dropped and cut to keep the match in view.
fn place_row(key: RowKey, hit: &Hit) -> Row {
    let (dir, name) = match hit.path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", hit.path.as_str()),
    };
    let label = format!("{name}:{}:{}", hit.line, hit.column);
    Row::new(key, label)
        .with_description(dir)
        .with_detail(cut_around(&hit.text, hit.column))
        .with_icon_kind(IconKind::File {
            name: name.to_string(),
        })
}

/// `text` without its indentation, cut to at most `SHOWN_CHARS` chars
/// around the char at `column` (from one), so the match is always in view.
fn cut_around(text: &str, column: usize) -> String {
    let indent = text.chars().take_while(|c| c.is_whitespace()).count();
    let chars: Vec<char> = text.chars().skip(indent).collect();
    let at = column.saturating_sub(1).saturating_sub(indent);
    if chars.len() <= SHOWN_CHARS {
        return chars.into_iter().collect();
    }
    let start = at
        .saturating_sub(SHOWN_CHARS / 3)
        .min(chars.len() - SHOWN_CHARS);
    chars[start..start + SHOWN_CHARS].iter().collect()
}

/// Where `pattern` is found in `text`, read as project search reads it:
/// a `*` for anything, case ignored unless it has a capital.
fn pattern_positions(text: &str, pattern: &str) -> Vec<usize> {
    if pattern.is_empty() {
        return Vec::new();
    }
    let Ok(regex) = pattern_regex(pattern) else {
        return Vec::new();
    };
    regex
        .find_iter(text)
        .flat_map(|found| {
            let start = text[..found.start()].chars().count();
            start..start + found.as_str().chars().count()
        })
        .collect()
}

/// The text of the PDF at `path`, read once and kept in `texts`; `None` for
/// a file that will not read as a PDF.
fn pdf_text(texts: &PdfTexts, path: &Path) -> Option<Arc<Vec<Vec<String>>>> {
    if let Some(kept) = texts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(path)
    {
        return Some(Arc::clone(kept));
    }
    let pages: Vec<Vec<String>> = winter_pdf::pages_lines(std::fs::read(path).ok()?)?.collect();
    let pages = Arc::new(pages);
    texts
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(path.to_path_buf(), Arc::clone(&pages));
    Some(pages)
}

/// Keep a PDF's line found for its row, and make the row: the PDF's name
/// and page, its directory after, and the line under, cut around the match.
fn keep_pdf_hit(hits: &Hits, hit: Hit) -> Row {
    let key = place_key(&hit.path, hit.line, hit.column);
    let (dir, name) = match hit.path.rsplit_once('/') {
        Some((dir, name)) => (dir, name),
        None => ("", hit.path.as_str()),
    };
    let row = Row::new(key, format!("{name}  page {}", hit.line))
        .with_description(dir)
        .with_detail(cut_around(&hit.text, hit.column))
        .with_icon_kind(IconKind::File {
            name: name.to_string(),
        });
    hits.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(key, hit);
    row
}

/// `path` from `root`, `/`-separated, or whole where it is outside.
fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// The regular expression a search's `pattern` is read as: case counting
/// only where it has a capital, a `*` standing for anything, the rest as
/// written.
pub(in crate::window) fn pattern_regex(pattern: &str) -> Result<Regex, String> {
    let source: String = pattern
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(WILDCARD_REGEX);
    let smart = !pattern.chars().any(char::is_uppercase);
    RegexBuilder::new(&source)
        .case_insensitive(smart)
        .build()
        .map_err(|why| why.to_string())
}

/// `keywords` as a regex alternation, each matched literally.
fn keyword_alternation(keywords: &[String]) -> String {
    keywords
        .iter()
        .map(|keyword| regex::escape(keyword))
        .collect::<Vec<_>>()
        .join("|")
}

/// Where one of `keywords` stands in `text`, as a whole word in its own case.
fn keyword_positions(text: &str, keywords: &[String]) -> Vec<usize> {
    let words = format!(r"\b({})\b", keyword_alternation(keywords));
    let Ok(regex) = Regex::new(&words) else {
        return Vec::new();
    };
    regex
        .find_iter(text)
        .flat_map(|found| {
            let start = text[..found.start()].chars().count();
            start..start + found.as_str().chars().count()
        })
        .collect()
}

/// Where `part` (lower case) is found in `text`, in any case.
fn part_positions(text: &str, part: &str) -> Vec<usize> {
    if part.is_empty() {
        return Vec::new();
    }
    let lower = text.to_lowercase();
    lower
        .match_indices(part)
        .flat_map(|(at, found)| {
            let start = lower[..at].chars().count();
            start..start + found.chars().count()
        })
        .collect()
}

fn open_actions() -> Vec<Action> {
    vec![
        Action {
            button: None,
            id: OPEN,
            title: "Open".to_string(),
        },
        Action {
            button: None,
            id: OPEN_BESIDE,
            title: "Open Beside".to_string(),
        },
    ]
}

fn hit_of(hits: &Hits, row: RowKey) -> Option<Hit> {
    hits.lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(&row)
        .cloned()
}

fn open_hit(hits: &Hits, root: &Path, action: ActionId, row: RowKey) -> Option<WindowOutcome> {
    let hit = hit_of(hits, row)?;
    let target = OpenTarget::at(root.join(&hit.path), hit.line, Some(hit.column));
    match action {
        OPEN => Some(WindowOutcome::OpenFile(target)),
        OPEN_BESIDE => Some(WindowOutcome::OpenFileBeside(target)),
        _ => None,
    }
}

fn preview_hit(hits: &Hits, root: &Path, row: RowKey) -> Option<Preview> {
    let hit = hit_of(hits, row)?;
    Some(Preview::File(FilePreview {
        column: Some(hit.column),
        line: hit.line,
        path: root.join(&hit.path),
    }))
}

fn saved_hits(hits: &Hits, root: &Path, rows: &[RowKey]) -> Vec<SearchHit> {
    let hits = hits.lock().unwrap_or_else(PoisonError::into_inner);
    rows.iter()
        .filter_map(|row| hits.get(row))
        .map(|hit| SearchHit {
            line: hit.line,
            path: root.join(&hit.path),
            text: hit.text.clone(),
        })
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_pdf_search_finds_the_page_a_line_is_on_and_reads_each_pdf_once() {
        let dir = std::env::temp_dir().join(format!("winter-pdf-search-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        std::fs::write(dir.join("notes.txt"), "a needle in text\n").expect("notes");
        let paper = winter_pdf::test_support::pdf_with_text(&[&["first page"], &["a needle here"]]);
        std::fs::write(dir.join("paper.pdf"), paper).expect("paper");
        let mut search = PdfSearch::new(dir.clone());
        let rows = |search: &mut PdfSearch, query: &str| -> Vec<Row> {
            let Feed::Stream(batches) = search.feed(query) else {
                panic!("a stream");
            };
            batches.iter().flat_map(|batch| batch.rows).collect()
        };
        let found = rows(&mut search, "needle");
        assert_eq!(found.len(), 1, "the text file is not read: {found:?}");
        assert_eq!(found[0].label, "paper.pdf  page 2");
        assert_eq!(
            search.run(OPEN, found[0].key, "needle"),
            Some(WindowOutcome::OpenFile(OpenTarget::at_line(
                dir.join("paper.pdf"),
                2
            )))
        );
        std::fs::remove_file(dir.join("paper.pdf")).expect("gone");
        assert_eq!(
            rows(&mut search, "first").len(),
            0,
            "a PDF gone from the folder is no longer listed"
        );
        assert_eq!(
            search.texts.lock().expect("texts").len(),
            1,
            "read once, kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn hit(path: &str, line: usize, column: usize, text: &str) -> Hit {
        Hit {
            column,
            line,
            path: path.to_string(),
            text: text.to_string(),
        }
    }

    #[test]
    fn test_a_search_row_is_its_place_then_its_text() {
        let row = place_row(RowKey(0), &hit("src/a.rs", 3, 5, "    let x = 1;"));
        assert_eq!(row.label, "a.rs:3:5");
        assert_eq!(row.description, "src");
        assert_eq!(row.detail, "let x = 1;");
    }

    #[test]
    fn test_a_long_line_is_cut_around_its_match() {
        let text = format!("{}needle{}", "a".repeat(350), "b".repeat(50));
        let shown = cut_around(&text, 351);
        assert!(shown.contains("needle"));
        assert_eq!(shown.chars().count(), SHOWN_CHARS);
    }

    #[test]
    fn test_ripgrep_is_asked_for_columns_and_its_lines_read_with_them() {
        assert!(rg_args("foo").iter().any(|arg| arg == "--column"));
        let parsed = parse_hit(b"./src/a.rs\x003:5:let x = 1;").expect("a hit");
        assert_eq!(parsed, hit("src/a.rs", 3, 5, "let x = 1;"));
    }

    #[test]
    fn test_the_pattern_lights_the_text_and_the_filter_the_place() {
        let search = ProjectSearch::new(PathBuf::from("/p"));
        let row = place_row(RowKey(0), &hit("src/foo.rs", 1, 1, "fn foo() {}"));
        let lit = search.highlights(&row, "foo @ src");
        assert_eq!(lit.detail, [3, 4, 5]);
        assert_eq!(lit.description, [0, 1, 2]);
        assert!(
            lit.label.is_empty(),
            "the filter's part is not in the label"
        );
    }

    #[test]
    fn test_the_todo_keyword_is_lit_in_its_comment_never_inside_a_word() {
        let keywords = ["TODO".to_string(), "NOTE".to_string()];
        assert_eq!(keyword_positions("// TODO: cache", &keywords), [3, 4, 5, 6]);
        assert!(keyword_positions("// todos and NOTED", &keywords).is_empty());
    }

    #[test]
    fn test_only_the_configured_keywords_are_lit() {
        let keywords = ["NEXT".to_string()];
        assert_eq!(keyword_positions("// NEXT: x", &keywords), [3, 4, 5, 6]);
        assert!(keyword_positions("// TODO: x", &keywords).is_empty());
    }

    #[test]
    fn test_a_filter_keeps_only_the_files_it_names() {
        assert!(FileFilter::new("src/*.rs").matches("src/a.rs"));
        assert!(!FileFilter::new("src/*.rs").matches("docs/a.md"));
        assert!(FileFilter::new("Recent").matches("window/recent.rs"));
        assert_eq!(split_query("foo @ src/*.rs"), ("foo", Some("src/*.rs")));
    }

    #[test]
    fn test_a_change_of_filter_alone_refilters_what_was_found_without_searching() {
        if process::command(RIPGREP).arg("--version").output().is_err() {
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut search = ProjectSearch::new(root);
        let Feed::Stream(batches) = search.feed("place_row(key, &hit)") else {
            panic!("a first search streams");
        };
        let all: Vec<Row> = batches.iter().flat_map(|batch| batch.rows).collect();
        let Feed::Fixed(kept) = search.feed("place_row(key, &hit) @ nowhere") else {
            panic!("a new filter alone is answered from what was found");
        };
        assert!(!all.is_empty());
        assert!(kept.is_empty(), "no file is named nowhere");
        let Feed::Fixed(again) = search.feed("place_row(key, &hit) @ search") else {
            panic!("answered again from what was found");
        };
        assert_eq!(again.len(), all.len());
    }

    #[test]
    fn test_project_search_finds_a_line_in_this_repository() {
        if process::command(RIPGREP).arg("--version").output().is_err() {
            return;
        }
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        let mut search = ProjectSearch::new(root);
        let Feed::Stream(batches) = search.feed("place_row(key, &hit)") else {
            panic!("a stream");
        };
        let rows: Vec<Row> = batches.iter().flat_map(|batch| batch.rows).collect();
        assert!(rows.iter().any(|row| row.label.starts_with("search.rs:")));
    }
}

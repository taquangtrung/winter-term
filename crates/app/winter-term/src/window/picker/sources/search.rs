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
use std::sync::{Arc, Mutex, PoisonError};

use regex::{Regex, RegexBuilder};
use winter_base::process;
use winter_model::editor::{OpenTarget, SearchHit};
use winter_model::paint::IconKind;
use winter_model::path::abbreviate_home;
use winter_picker::{
    Action, ActionId, Feed, FilePreview, Highlights, MatchOn, Matching, Preview, PreviewKind,
    Resume, Row, RowKey, Source,
};

use super::stream::{place_key, stream_lines};
use crate::window::picker::outcome::SaveHits;
use crate::window::picker::WindowOutcome;

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

/// How many lines a search keeps, and how many more each resume adds.
const MAX_RESULTS: usize = 10_000;
const RESUME_MORE: usize = 10_000;

/// How many comment keywords the Todo picker keeps, and adds on resume.
const MAX_TODOS: usize = 2_000;

/// The longest line text kept, in chars, and how many chars of it a row
/// shows around its match.
const MAX_LINE_CHARS: usize = 300;
const SHOWN_CHARS: usize = 120;

/// What a `*` in a pattern stands for in the regex it becomes.
const WILDCARD_REGEX: &str = ".*";

/// What starts a project search's file filter.
const FILTER_MARK: &str = " @";

/// The comment keywords the Todo picker lists.
pub(in crate::window) const TODO_KEYWORDS: [&str; 6] =
    ["TODO", "FIXME", "BUG", "NOTE", "HACK", "XXX"];

/// The actions of a search's row: open the file at the line, or beside.
const OPEN: ActionId = ActionId("open");
const OPEN_BESIDE: ActionId = ActionId("open_beside");

// ========================================================================
// Data Structures
// ========================================================================

/// The project, searched with ripgrep for the pattern typed.
pub(in crate::window) struct ProjectSearch {
    cap: usize,
    /// The places the last pattern found, in the order found, those the
    /// file filter hid among them: a change of filter alone is answered from
    /// these, not by searching again.
    found: Arc<Mutex<Vec<RowKey>>>,
    hits: Hits,
    /// The pattern last searched for.
    pattern: Option<String>,
    root: PathBuf,
}

/// The comment keywords under the project, filtered by what is typed.
pub(in crate::window) struct TodoSearch {
    cap: usize,
    hits: Hits,
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
            cap: MAX_RESULTS,
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
}

impl Source<WindowOutcome> for ProjectSearch {
    fn id(&self) -> &'static str {
        "project_search"
    }

    fn title(&self) -> String {
        format!("Search: {}", abbreviate_home(&self.root))
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
        stream_lines(command, self.cap, NO_RIPGREP, move |line| {
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
        open_actions()
    }

    fn run(&mut self, action: ActionId, row: RowKey, _query: &str) -> Option<WindowOutcome> {
        open_hit(&self.hits, &self.root, action, row)
    }

    fn preview(&self, row: RowKey) -> Option<Preview> {
        preview_hit(&self.hits, &self.root, row)
    }

    fn can_save(&self) -> bool {
        true
    }

    /// Search again on resume, a batch further, past the rows kept.
    fn refresh(&mut self) {
        self.cap += RESUME_MORE;
        self.pattern = None;
    }

    fn export(&self, rows: &[RowKey], query: &str) -> Option<WindowOutcome> {
        let hits = saved_hits(&self.hits, &self.root, rows);
        Some(WindowOutcome::SaveHits(SaveHits {
            capped: hits.len() >= self.cap,
            filtered: None,
            hits,
            query: split_query(query).0.to_string(),
            root: self.root.clone(),
            todo: None,
        }))
    }
}

// ========================================================================
// TodoSearch
// ========================================================================

impl TodoSearch {
    /// The comment keywords under the project at `root`.
    pub(in crate::window) fn new(root: PathBuf) -> Self {
        Self {
            cap: MAX_TODOS,
            hits: Hits::default(),
            root,
        }
    }
}

impl Source<WindowOutcome> for TodoSearch {
    fn id(&self) -> &'static str {
        "todo"
    }

    fn title(&self) -> String {
        format!("TODOs: {}", abbreviate_home(&self.root))
    }

    fn matching(&self) -> Matching {
        Matching::Words(MatchOn::DETAIL)
    }

    /// The keyword found, lit in its comment beside the words typed.
    fn highlights(&self, row: &Row, _query: &str) -> Highlights {
        Highlights {
            detail: keyword_positions(&row.detail),
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
        let keywords = format!(r"\b({})\b:?", TODO_KEYWORDS.join("|"));
        let mut command = process::command(RIPGREP);
        command
            .args(base_args())
            .args(["--regexp", &keywords])
            .current_dir(&self.root);
        let hits = Arc::clone(&self.hits);
        stream_lines(command, self.cap, NO_RIPGREP, move |line| {
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

    fn can_save(&self) -> bool {
        true
    }

    fn refresh(&mut self) {
        self.cap += MAX_TODOS;
    }

    fn export(&self, rows: &[RowKey], query: &str) -> Option<WindowOutcome> {
        let hits = saved_hits(&self.hits, &self.root, rows);
        let query = query.trim();
        Some(WindowOutcome::SaveHits(SaveHits {
            capped: hits.len() >= self.cap,
            filtered: (!query.is_empty()).then(|| query.to_string()),
            hits,
            query: TODO_KEYWORDS.join(" "),
            root: self.root.clone(),
            todo: Some(TODO_KEYWORDS.iter().map(ToString::to_string).collect()),
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
        .with_icon(IconKind::File {
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
    let source: String = pattern
        .split('*')
        .map(regex::escape)
        .collect::<Vec<_>>()
        .join(WILDCARD_REGEX);
    let smart = !pattern.chars().any(char::is_uppercase);
    let Ok(regex) = RegexBuilder::new(&source).case_insensitive(smart).build() else {
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

/// Where a comment keyword stands in `text`, as a whole word in its own case.
fn keyword_positions(text: &str) -> Vec<usize> {
    let words = format!(r"\b({})\b", TODO_KEYWORDS.join("|"));
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
        OPEN_BESIDE => Some(WindowOutcome::OpenFileBeside(target)),
        _ => Some(WindowOutcome::OpenFile(target)),
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
        assert_eq!(keyword_positions("// TODO: cache"), [3, 4, 5, 6]);
        assert!(keyword_positions("// todos and NOTED").is_empty());
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

//! Grep: the lines under a directory that hold some text, grouped by file and
//! opened straight from the list, or edited in place and written back to the
//! files they came from (`C-x C-q`), in the manner of Emacs' `wgrep`.
//!
//! A search reads its text the way VS Code's search toggles say: in any case
//! or only in its own (`M-c`), as text or a regular expression (`M-r`),
//! anywhere or as a whole word (`M-w`), in every file or only those a glob
//! names (`M-f`, or `M-t` for the type of the file under the cursor). `M-h`
//! goes back to an earlier search.
//!
//! The same editor is the Todo list: a search for comment keywords (`TODO`,
//! `FIXME`) as whole words, run as soon as it opens.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::editors::background::{self, Background};
use crate::editors::list::{ListPlace, ListView};
use crate::editors::text_file;
use crate::model::editor::{
    menu_groups, Editor, EditorMenuItem, EditorOutcome, EditorStatus, JobReply, JobRequest, Notice,
    OpenTarget, PickPlacement, PickRequest, PromptMode, PromptReply, PromptRequest, SearchHit, SearchOptions,
    SearchPattern, SearchRequest, SearchResult,
};
use crate::model::input::table::{self, Binding, KeyHelp, TableMatch};
use crate::model::input::{format_key, Key, KeyCode, Mods};
use crate::model::paint::{row_height, row_text, Caret, Frame, Row, Shown, Span, Style};
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditOutcome;
use crate::model::vim::nav::{VimKey, VimNav};

// ========================================================================
// Constants
// ========================================================================

/// The questions past the query: which files to look in, and which earlier
/// search to run again.
const ASK_GLOB: &str = "glob";
const ASK_HISTORY: &str = "history";

/// How many earlier searches are kept to go back to.
const HISTORY_MAX: usize = 50;

/// The one question the editor asks.
const ASK_QUERY: &str = "query";

/// Rows of header above the first result.
const HEADER_ROWS: usize = 1;

/// Width the line numbers are padded to, so the texts they label line up.
const LINE_WIDTH: usize = 5;

/// What sits between a line number and its text.
const LINE_GAP: &str = ": ";

/// Shown beside the root before anything has been searched for, after the
/// key that asks for a search.
const EMPTY_HINT: &str = "to search";

/// The editor's title, before a query narrows it.
const TITLE: &str = "Grep";

/// The title of the editor while it lists comment keywords.
const TODO_TITLE: &str = "Todo";

/// What the question for a text search starts with.
const TEXT_LABEL: &str = "grep: ";

/// What the question for the keywords starts with.
const WORDS_LABEL: &str = "todo keywords: ";

/// What the thread writing edited matches back is called.
const WRITER_THREAD: &str = "winter grep write";

/// How long the key that writes the edits waits for them to be written
/// before the frame is drawn without the answer. A handful of small files
/// is written well inside it; a batch over many large ones finishes on its
/// own while the window stays live.
#[cfg(not(test))]
const WRITE_WAIT: Duration = Duration::from_millis(40);

/// Long enough that a test's write always lands with its key.
#[cfg(test)]
const WRITE_WAIT: Duration = Duration::from_secs(10);

/// The results' own keys.
const KEYS: &[Binding<Command>] = &[
    Binding {
        action: Command::Open,
        keys: &["RET"],
        label: "Open at This Line",
    },
    Binding {
        action: Command::OpenExternal,
        keys: &["C-o"],
        label: "Open in $EDITOR",
    },
    Binding {
        action: Command::Yank,
        keys: &["y"],
        label: "Copy path:line",
    },
    // `/` asks for a new search here rather than searching the results: the
    // rows already are a search, and narrowing it is what a second query is
    // for.
    Binding {
        action: Command::Search,
        keys: &["/"],
        label: "New Search...",
    },
    // Rerunning is `C-r` in every editor, so `G` is left to mean the bottom.
    Binding {
        action: Command::Rerun,
        keys: &["C-r"],
        label: "Run This Search Again",
    },
    // A walk over a large tree is worth abandoning, and the key that means
    // stop is the one to abandon it with.
    Binding {
        action: Command::Stop,
        keys: &["ESC"],
        label: "Stop the Search",
    },
    Binding {
        action: Command::Down,
        keys: &["j", "<down>"],
        label: "Next Row",
    },
    Binding {
        action: Command::Up,
        keys: &["k", "<up>"],
        label: "Previous Row",
    },
    // `n` and `N` mean the next and previous match here, as they do in every
    // other editor: the list is already the search, so there is nothing else
    // for them to step through.
    Binding {
        action: Command::NextMatch,
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Command::PreviousMatch,
        keys: &["N"],
        label: "Previous Match",
    },
    Binding {
        action: Command::NextFile,
        keys: &["M-n"],
        label: "Next File",
    },
    Binding {
        action: Command::PreviousFile,
        keys: &["M-p"],
        label: "Previous File",
    },
    Binding {
        action: Command::First,
        keys: &["<home>"],
        label: "First Row",
    },
    Binding {
        action: Command::Last,
        keys: &["<end>"],
        label: "Last Row",
    },
    Binding {
        action: Command::Edit,
        keys: &["C-x C-q"],
        label: "Edit the Matches as Text",
    },
    Binding {
        action: Command::ToggleCase,
        keys: &["M-c"],
        label: "Toggle Match Case",
    },
    Binding {
        action: Command::ToggleRegex,
        keys: &["M-r"],
        label: "Toggle Regular Expression",
    },
    Binding {
        action: Command::ToggleWord,
        keys: &["M-w"],
        label: "Toggle Whole Word",
    },
    Binding {
        action: Command::Glob,
        keys: &["M-f"],
        label: "Only Files Matching a Glob...",
    },
    Binding {
        action: Command::SameType,
        keys: &["M-t"],
        label: "Only Files of This Type",
    },
    Binding {
        action: Command::History,
        keys: &["M-h"],
        label: "An Earlier Search...",
    },
];

/// The keys while the matches are editable text, ahead of the editor's.
const EDIT_KEYS: &[Binding<Command>] = &[
    Binding {
        action: Command::Write,
        keys: &["C-S-c"],
        label: "Write the Edits",
    },
    Binding {
        action: Command::Edit,
        keys: &["C-x C-q", "C-S-k"],
        label: "Throw the Edits Away",
    },
    Binding {
        action: Command::Discard,
        keys: &["q"],
        label: "Throw the Edits Away, with Nothing Part-Typed",
    },
];

/// What the menu opened over a match offers, in its order.
const MATCH_MENU: [Command; 3] = [Command::Open, Command::OpenExternal, Command::Yank];

/// What the menu offers over any row.
const ROW_MENU: [Command; 2] = [Command::Search, Command::Rerun];

/// What the menu offers to change how the search reads.
const OPTION_MENU: [Command; 5] = [
    Command::ToggleCase,
    Command::ToggleRegex,
    Command::ToggleWord,
    Command::SameType,
    Command::History,
];

// ========================================================================
// Data Structures
// ========================================================================

/// What a key of the editor's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Command {
    Discard,
    Down,
    Edit,
    First,
    /// Ask for a glob the files searched must match.
    Glob,
    /// Offer the earlier searches to run one again.
    History,
    Last,
    NextFile,
    NextMatch,
    Open,
    OpenExternal,
    PreviousFile,
    PreviousMatch,
    Rerun,
    /// Only the files of the type the one under the cursor is, or every file
    /// again.
    SameType,
    Search,
    Stop,
    ToggleCase,
    ToggleRegex,
    ToggleWord,
    Up,
    /// Write the edited matches back to their files.
    Write,
    Yank,
}

/// A text search over a directory tree, and the lines it found.
#[derive(Debug)]
pub struct GrepEditor {
    /// The matched lines as editable text while they are being rewritten in
    /// place, one line per hit.
    edit: Option<TextField>,
    /// The first key of a sequence (`C-x`, which `C-q` completes into the
    /// chord that starts or ends editing), while it waits for the second.
    edit_chord: Option<Key>,
    hits: Vec<SearchHit>,
    /// The queries searched for, the newest last, to go back to.
    history: Vec<String>,
    /// What the last search or edit reported, waiting for the status bar to
    /// take it.
    notice: Option<Notice>,
    /// How a text query is read: case, regular expressions, whole words,
    /// and which files are looked in.
    options: SearchOptions,
    /// How the query is read: one run of text, or keywords as whole words.
    pattern: SearchPattern,
    /// The text last searched for, which a new question starts out holding.
    query: String,
    /// The cursor over the rows, kept on its row by what the row stands for.
    list: ListView<RowKey>,
    /// The shared Vim motion keys, for the ones the results do not claim.
    nav: VimNav,
    root: PathBuf,
    rows: Vec<ResultRow>,
    /// How long writing the edits back is waited on (see [`WRITE_WAIT`]).
    write_wait: Duration,
    /// The edits being written back, which lands on a later tick when the
    /// write takes longer than the key that asked for it waited.
    writing: Option<Background<WriteBack>>,
    /// Whether the query is to be searched for on the first tick, for an
    /// editor that opens already knowing what to look for.
    search_on_open: bool,
    /// Whether a walk is still out, so the header can say the list is partial.
    searching: bool,
    /// Whether the walk stopped at its cap with matches left unreported.
    truncated: bool,
}

/// What names a row across a rebuild: the hit it leads to, and whether it is
/// that hit's file heading rather than its line.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct RowKey {
    /// Whether the row is a file's heading.
    heading: bool,
    /// The hit the row leads to.
    hit: usize,
}

/// The edits to write back to one file, and the hits it holds, whose text
/// is read again from what was written.
#[derive(Clone, Debug, Default)]
struct FileEdits {
    /// The edited lines.
    edits: Vec<LineEdit>,
    /// Every hit in the file, edited or not.
    hits: Vec<HitLine>,
}

/// One edited line: where it is, what the search found there, and what it
/// now says.
#[derive(Clone, Debug)]
struct LineEdit {
    /// What the search found on the line, which the file must still say.
    found: String,
    /// The line, counting from one.
    line: usize,
    /// What the line now says.
    text: String,
}

/// A hit and the line it is on.
#[derive(Clone, Copy, Debug)]
struct HitLine {
    /// The hit.
    hit: usize,
    /// Its line, counting from one.
    line: usize,
}

/// What writing the edits back came to.
#[derive(Clone, Debug, Default)]
struct WriteBack {
    /// How many files were written.
    files: usize,
    /// Each hit in a written file, with its line's text as written.
    lines: Vec<WrittenLine>,
    /// How many edited lines were left alone: changed on disk since the
    /// search, cut short by it, or in a file that could not be written.
    refused: usize,
    /// How many edited lines were written.
    written: usize,
}

/// A hit's line as it reads now.
#[derive(Clone, Debug)]
struct WrittenLine {
    /// The hit.
    hit: usize,
    /// Its line's text.
    text: String,
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
    spans: Row,
}

// ========================================================================
// GrepEditor
// ========================================================================

impl GrepEditor {
    /// An empty search over `root`, waiting to be asked for something.
    pub fn new(root: PathBuf) -> Self {
        Self::with_pattern(root, SearchPattern::Text, String::new())
    }

    /// The Todo list: every line under `root` holding one of `keywords` as a
    /// whole word, searched for as soon as the editor is ticked.
    pub fn todo(root: PathBuf, keywords: &[String]) -> Self {
        let mut editor = Self::with_pattern(root, SearchPattern::Words, keywords.join(" "));
        editor.search_on_open = !editor.query.is_empty();
        editor
    }

    fn with_pattern(root: PathBuf, pattern: SearchPattern, query: String) -> Self {
        Self {
            edit: None,
            edit_chord: None,
            hits: Vec::new(),
            history: Vec::new(),
            notice: None,
            options: SearchOptions::default(),
            pattern,
            query,
            list: ListView::new(),
            nav: VimNav::new(),
            root,
            rows: Vec::new(),
            write_wait: WRITE_WAIT,
            writing: None,
            search_on_open: false,
            searching: false,
            truncated: false,
        }
    }

    /// How the editor is named before anything narrows it.
    fn base_title(&self) -> &'static str {
        match self.pattern {
            SearchPattern::Text => TITLE,
            SearchPattern::Words => TODO_TITLE,
        }
    }

    /// Ask what to look for, starting from what was looked for last so that
    /// narrowing a query is an edit rather than a retype.
    fn ask_query(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: self.query.clone(),
            label: match self.pattern {
                SearchPattern::Text => TEXT_LABEL,
                SearchPattern::Words => WORDS_LABEL,
            }
            .to_string(),
            mode: PromptMode::Text,
            tag: ASK_QUERY,
        })
    }

    /// Look for `query`, dropping what the last search found: a result list
    /// half from one query and half from another would answer neither.
    fn start_search(&mut self, query: &str) -> EditorOutcome {
        if query.is_empty() {
            return EditorOutcome::Consumed;
        }
        // The answer to a write names hits by where they are in the list,
        // which a new search would replace under it.
        if self.writing.is_some() {
            self.notice = Some(Notice::error("still writing the last edits"));
            return EditorOutcome::Consumed;
        }
        self.query = query.to_string();
        self.history.retain(|earlier| earlier != query);
        self.history.push(query.to_string());
        if self.history.len() > HISTORY_MAX {
            self.history.remove(0);
        }
        self.search_on_open = false;
        self.hits.clear();
        self.rows.clear();
        self.list.reset_keys(Vec::new());
        self.list.set_cursor(0);
        self.list.view_mut().set_scroll(0);
        self.searching = true;
        self.truncated = false;
        let request = SearchRequest::new(self.pattern, self.query.clone(), self.root.clone())
            .with_options(self.options.clone());
        EditorOutcome::Job(JobRequest::Search(request))
    }

    /// Change how the search reads by `change`, saying what it reads as now,
    /// and search again with it.
    fn reread(&mut self, change: impl FnOnce(&mut SearchOptions)) -> EditorOutcome {
        if self.pattern == SearchPattern::Words {
            self.notice = Some(Notice::error("the Todo list reads its keywords as whole words"));
            return EditorOutcome::Consumed;
        }
        change(&mut self.options);
        self.notice = Some(Notice::info(format!("searching {}", self.reading())).unlogged());
        match self.query.is_empty() {
            true => EditorOutcome::Consumed,
            false => {
                let query = self.query.clone();
                self.start_search(&query)
            }
        }
    }

    /// How the search reads its query, in words: `text in any case`, `a
    /// regular expression as a whole word, in *.rs`.
    fn reading(&self) -> String {
        let options = &self.options;
        let what = match options.regex {
            true => "a regular expression",
            false => "text",
        };
        let word = match options.whole_word {
            true => " as a whole word",
            false => "",
        };
        let case = match options.case_sensitive {
            true => ", its case counting",
            false => " in any case",
        };
        let files = options
            .glob
            .as_ref()
            .map(|glob| format!(", in {glob}"))
            .unwrap_or_default();
        format!("{what}{word}{case}{files}")
    }

    /// Search only the files whose type the one under the cursor is, or
    /// every file again where the search already did.
    fn same_type(&mut self) -> EditorOutcome {
        let extension = self
            .selected()
            .and_then(|hit| hit.path.extension())
            .and_then(|extension| extension.to_str())
            .map(str::to_string);
        let Some(extension) = extension else {
            self.notice = Some(Notice::error("no file with a type under the cursor"));
            return EditorOutcome::Consumed;
        };
        let glob = format!("*.{extension}");
        let next = match self.options.glob.as_deref() == Some(glob.as_str()) {
            true => None,
            false => Some(glob),
        };
        self.reread(|options| options.glob = next)
    }

    /// Ask for the glob the files searched must match, starting from the
    /// one in use.
    fn ask_glob(&self) -> EditorOutcome {
        EditorOutcome::Prompt(PromptRequest {
            initial: self.options.glob.clone().unwrap_or_default(),
            label: "only files matching (empty for every file): ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_GLOB,
        })
    }

    /// The earlier searches, the newest first, to run one again.
    fn ask_history(&mut self) -> EditorOutcome {
        if self.history.is_empty() {
            self.notice = Some(Notice::error("no earlier searches"));
            return EditorOutcome::Consumed;
        }
        EditorOutcome::Pick(PickRequest {
            icons: Vec::new(),
            items: self.history.iter().rev().cloned().collect(),
            label: "Earlier searches".to_string(),
            placement: PickPlacement::Centered,
            tag: ASK_HISTORY,
        })
    }

    /// Take what a walk found, unless it answers a query already replaced: two
    /// walks can be out at once, and the slower one would otherwise paint its
    /// matches under the newer query's name.
    fn on_found(&mut self, found: SearchResult) {
        if found.query != self.query {
            return;
        }
        self.searching = false;
        if let Some(error) = found.error {
            self.notice = Some(Notice::error(error));
            return;
        }
        self.truncated = found.truncated;
        self.hits = found.hits;
        self.rebuild();
        if self.hits.is_empty() {
            self.notice = Some(Notice::info(format!("no matches for {}", self.query)));
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
        let keys = self
            .rows
            .iter()
            .map(|row| RowKey {
                heading: row.heading,
                hit: row.hit,
            })
            .collect();
        self.list.set_keys(keys);
    }

    fn selected(&self) -> Option<&SearchHit> {
        let row = self.rows.get(self.list.cursor())?;
        self.hits.get(row.hit)
    }

    fn move_by(&mut self, delta: isize) {
        self.list.move_by(delta);
    }

    /// A key the results do not bind, offered to the shared Vim motions:
    /// `gg`, `G`, the paging chords, and `H`/`M`/`L`.
    /// Do what one of the results' own keys asks. `None` for a key with
    /// nothing to do in the state the editor is in, which then reaches the
    /// motion layer.
    fn run(&mut self, command: Command) -> Option<EditorOutcome> {
        let outcome = match command {
            Command::Open => self.open_selected(),
            Command::OpenExternal => self.open_external(),
            Command::Yank => self.yank_selected(),
            Command::Search => self.ask_query(),
            Command::Rerun => {
                let query = self.query.clone();
                self.start_search(&query)
            }
            Command::Stop if !self.searching => return None,
            Command::Stop => {
                self.searching = false;
                self.notice = Some(Notice::info("search stopped"));
                EditorOutcome::CancelJobs
            }
            Command::Down => {
                self.move_by(1);
                EditorOutcome::Consumed
            }
            Command::Up => {
                self.move_by(-1);
                EditorOutcome::Consumed
            }
            Command::NextMatch => {
                self.move_to_row(true, false);
                EditorOutcome::Consumed
            }
            Command::PreviousMatch => {
                self.move_to_row(false, false);
                EditorOutcome::Consumed
            }
            Command::NextFile => {
                self.move_to_row(true, true);
                EditorOutcome::Consumed
            }
            Command::PreviousFile => {
                self.move_to_row(false, true);
                EditorOutcome::Consumed
            }
            Command::First => {
                self.list.set_cursor(0);
                EditorOutcome::Consumed
            }
            Command::Last => {
                self.list.set_cursor(usize::MAX);
                EditorOutcome::Consumed
            }
            Command::ToggleCase => self.reread(|o| o.case_sensitive = !o.case_sensitive),
            Command::ToggleRegex => self.reread(|o| o.regex = !o.regex),
            Command::ToggleWord => self.reread(|o| o.whole_word = !o.whole_word),
            Command::SameType => self.same_type(),
            Command::Glob => self.ask_glob(),
            Command::History => self.ask_history(),
            // Sequences and edit keys are read before this is reached.
            Command::Edit | Command::Discard | Command::Write => return None,
        };
        Some(outcome)
    }

    fn on_motion_key(&mut self, key: &Key) -> EditorOutcome {
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => {
                self.list.apply_motion(motion);
                EditorOutcome::Consumed
            }
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    /// Move to the next or previous row of one kind: `heading` for stepping over
    /// a file with a hundred matches in it, and the other way for stepping
    /// between matches without landing on the names between them.
    fn move_to_row(&mut self, forward: bool, heading: bool) {
        let at = self.list.cursor();
        let found = match forward {
            true => (at + 1..self.rows.len()).find(|&i| self.rows[i].heading == heading),
            false => (0..at).rev().find(|&i| self.rows[i].heading == heading),
        };
        if let Some(index) = found {
            self.list.set_cursor(index);
        }
    }

    /// Open the file the cursor is on, at the line that matched.
    fn open_selected(&self) -> EditorOutcome {
        match self.selected() {
            Some(hit) => EditorOutcome::OpenPath(OpenTarget::at_line(hit.path.clone(), hit.line)),
            None => EditorOutcome::Consumed,
        }
    }

    /// Hand the file the cursor is on to `$EDITOR`, in a pane of its own, at
    /// the line that matched.
    fn open_external(&self) -> EditorOutcome {
        match self.selected() {
            Some(hit) => {
                EditorOutcome::SpawnEditor(OpenTarget::at_line(hit.path.clone(), hit.line))
            }
            None => EditorOutcome::Consumed,
        }
    }

    /// Copy where the match is, in the `path:line` form every editor understands.
    fn yank_selected(&self) -> EditorOutcome {
        match self.selected() {
            Some(hit) => EditorOutcome::Yank(format!(
                "{}:{}",
                relative_to(&self.root, &hit.path),
                hit.line
            )),
            None => EditorOutcome::Consumed,
        }
    }

    /// The hit row standing for hit `index`.
    fn row_of_hit(&self, index: usize) -> Option<usize> {
        self.rows
            .iter()
            .position(|row| !row.heading && row.hit == index)
    }

    /// Start editing the matched lines in place, or stop, throwing away what
    /// was not written.
    fn toggle_edit(&mut self) {
        if self.edit.is_some() {
            self.discard_edit();
            return;
        }
        if self.hits.is_empty() || self.searching {
            self.notice = Some(Notice::error("nothing to edit"));
            return;
        }
        if self.writing.is_some() {
            self.notice = Some(Notice::error("still writing the last edits"));
            return;
        }
        let lines = self.hits.iter().map(|hit| hit.text.clone()).collect();
        let at = self.rows.get(self.list.cursor()).map_or(0, |row| row.hit);
        self.edit = Some(TextField::rows(lines, at));
        if let Some(row) = self.row_of_hit(at) {
            self.list.set_cursor(row);
        }
    }

    /// The hits whose text has been edited, with what it now says.
    fn edited(&self) -> Vec<(usize, String)> {
        let Some(edit) = self.edit.as_ref() else {
            return Vec::new();
        };
        self.hits
            .iter()
            .enumerate()
            .filter_map(|(index, hit)| {
                let text = edit.lines().get(index)?;
                (*text != hit.text).then(|| (index, text.clone()))
            })
            .collect()
    }

    /// Stop editing without writing anything.
    fn discard_edit(&mut self) {
        let edited = self.edited().len();
        self.edit = None;
        self.notice = (edited > 0).then(|| Notice::info(format!("discarded {edited}")));
    }

    /// Write the edited lines back to their files, each file read and written
    /// once, off the window's thread. A line whose file no longer says what
    /// the search found there is left alone and counted, since writing over
    /// it would throw away a change made since; so is a line the search cut
    /// short, which cannot be put back whole.
    fn apply_edits(&mut self) -> EditorOutcome {
        let edited = self.edited();
        self.edit = None;
        if edited.is_empty() {
            self.notice = Some(Notice::info("no changes"));
            return EditorOutcome::Consumed;
        }
        let mut by_file: BTreeMap<PathBuf, FileEdits> = BTreeMap::new();
        for (index, text) in edited {
            let hit = &self.hits[index];
            by_file
                .entry(hit.path.clone())
                .or_default()
                .edits
                .push(LineEdit {
                    found: hit.text.clone(),
                    line: hit.line,
                    text,
                });
        }
        for (index, hit) in self.hits.iter().enumerate() {
            if let Some(file) = by_file.get_mut(&hit.path) {
                file.hits.push(HitLine {
                    hit: index,
                    line: hit.line,
                });
            }
        }
        let work = Background::start(WRITER_THREAD, move |_| write_back(by_file));
        match work.wait(self.write_wait) {
            Some(report) => self.take_write(report),
            None => {
                self.writing = Some(work);
                self.notice = Some(Notice::info("writing..."));
            }
        }
        EditorOutcome::Consumed
    }

    /// Show what writing the edits back came to.
    fn take_write(&mut self, report: WriteBack) {
        for line in report.lines {
            if let Some(hit) = self.hits.get_mut(line.hit) {
                hit.text = line.text;
            }
        }
        self.rebuild();
        let mut said = format!(
            "wrote {} in {} files",
            lines_count(report.written),
            report.files
        );
        if report.refused > 0 {
            said.push_str(&format!(
                ", {} changed on disk, left alone",
                lines_count(report.refused)
            ));
        }
        self.notice = Some(Notice::info(said));
    }

    /// Keys while the matches are editable text: the editor's keys, the cursor
    /// following the line being edited. `ZZ`, `C-s`, `C-S-c`, or `Enter`
    /// while typing writes; `ZQ`, `C-S-k`, or `q` with nothing part-typed,
    /// throws it away.
    fn on_edit_key(&mut self, key: &Key) -> EditorOutcome {
        let at = self.rows.get(self.list.cursor()).map(|row| row.hit);
        let viewport = self.list.view().rows().max(1);
        // The field holds a line per hit, under the listing's rows.
        let hit = |row: usize| self.rows.get(row).map_or(0, |row| row.hit);
        let shown = self.list.view().shown();
        let shown = Shown {
            bottom: hit(shown.bottom),
            middle: hit(shown.middle),
            top: hit(shown.top),
        };
        let Some(edit) = self.edit.as_mut() else {
            return EditorOutcome::Ignored;
        };
        // `q` throws the edits away only with nothing part-typed, since it
        // may be a letter of the text; the chords never are.
        match table::find(EDIT_KEYS, key) {
            Some(Command::Write) => return self.apply_edits(),
            Some(Command::Edit) => {
                self.discard_edit();
                return EditorOutcome::Consumed;
            }
            Some(Command::Discard) if edit.modal().is_idle() => {
                self.discard_edit();
                return EditorOutcome::Consumed;
            }
            Some(_) | None => {}
        }
        let Some(edit) = self.edit.as_mut() else {
            return EditorOutcome::Ignored;
        };
        if let Some(at) = at {
            edit.move_to_row(at);
        }
        edit.show(shown);
        let outcome = edit.on_key(key, viewport);
        let row = edit.row();
        if let Some(notice) = edit.take_notice() {
            self.notice = Some(notice);
        }
        if let Some(cursor) = self.row_of_hit(row) {
            self.list.set_cursor(cursor);
        }
        match outcome {
            EditOutcome::Quit { save: true } | EditOutcome::Save | EditOutcome::Submit => {
                self.apply_edits()
            }
            EditOutcome::Quit { save: false } | EditOutcome::Cancel => {
                self.discard_edit();
                EditorOutcome::Consumed
            }
            EditOutcome::Yank(text) => EditorOutcome::Yank(text),
            EditOutcome::Cut(text) => EditorOutcome::Cut(text),
            EditOutcome::Paste => EditorOutcome::Paste,
            EditOutcome::Place(place) => {
                self.list.view_mut().request(place);
                EditorOutcome::Consumed
            }
            EditOutcome::Ignored if key.chord() == (Mods::NONE, KeyCode::Escape) => EditorOutcome::Consumed,
            EditOutcome::Ignored => EditorOutcome::Ignored,
            EditOutcome::Consumed
            | EditOutcome::Escalate
            | EditOutcome::ListBuffers
            | EditOutcome::Search { .. }
            | EditOutcome::StepBuffer { .. }
            | EditOutcome::Macro(_) => EditorOutcome::Consumed,
        }
    }

    /// A hit's row as it stands mid-edit: the edited text, marked where it
    /// differs from what was found.
    fn edited_row(&self, row: &ResultRow) -> Row {
        let (Some(edit), Some(hit)) = (self.edit.as_ref(), self.hits.get(row.hit)) else {
            return row.spans.clone();
        };
        let text = edit.lines().get(row.hit).cloned().unwrap_or_default();
        let style = match text == hit.text {
            true => Style::Normal,
            false => Style::Accent,
        };
        vec![
            Span::new(Style::Dim, line_label(hit.line)),
            Span::new(style, text),
        ]
    }

    /// The root, what was searched for, and how much of the tree it matched.
    fn header_row(&self) -> Row {
        let mut spans = vec![Span::new(Style::Header, self.root.to_string_lossy())];
        if self.query.is_empty() {
            if let Some((key, _)) = table::menu_entry(KEYS, Command::Search) {
                spans.push(Span::new(
                    Style::Dim,
                    format!("  {} {EMPTY_HINT}", format_key(&key)),
                ));
            }
            return spans;
        }
        let asked = match self.pattern {
            SearchPattern::Text => format!("  /{}", self.query),
            SearchPattern::Words => format!("  {}", self.query),
        };
        spans.push(Span::new(Style::Accent, asked));
        if self.pattern == SearchPattern::Text && self.options != SearchOptions::default() {
            spans.push(Span::new(Style::Dim, format!("  ({})", self.reading())));
        }
        let detail = if self.searching {
            "  searching...".to_string()
        } else {
            let files = self.rows.iter().filter(|row| row.heading).count();
            let capped = if self.truncated { ", capped" } else { "" };
            format!("  {} in {files} files{capped}", matches(self.hits.len()))
        };
        spans.push(Span::new(Style::Dim, detail));
        spans
    }
}

impl Editor for GrepEditor {
    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.list.saved_place()))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(place) = state.downcast::<ListPlace<RowKey>>() {
            self.list.restore_place(*place);
        }
    }

    fn title(&self) -> String {
        let title = self.base_title();
        match (self.pattern, self.query.is_empty()) {
            (_, true) | (SearchPattern::Words, false) => title.to_string(),
            (SearchPattern::Text, false) => format!("{title}: {}", self.query),
        }
    }

    fn set_scroll_margin(&mut self, rows: usize) {
        self.list.view_mut().set_margin(rows);
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        // The view moves and the cursor stays, as in an editor.
        self.list.view_mut().wheel(lines);
        EditorOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let mut frame_rows = vec![self.header_row()];
        if self.rows.is_empty() {
            return Frame::new(frame_rows);
        }
        let texts: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.list.view_mut().set_rows(visible);
        let placed = self
            .list
            .place(visible, |index| row_height(&texts[index], cols, wrap, 0));
        let window = placed.window;
        frame_rows.extend(
            self.rows
                .iter()
                .skip(window.start)
                .take(window.count)
                .map(|row| match self.edit.is_some() && !row.heading {
                    true => self.edited_row(row),
                    false => row.spans.clone(),
                }),
        );
        placed.apply(Frame::new(frame_rows), HEADER_ROWS)
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    /// The edit mode while the matched lines are being rewritten.
    fn status(&self) -> EditorStatus {
        EditorStatus {
            mode: self.edit.as_ref().map(|edit| edit.modal().mode()),
            segments: Vec::new(),
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        self.list.view_mut().release();
        if let Some(first) = self.edit_chord.take() {
            if table::lookup(KEYS, &[first, *key]) == TableMatch::Exact(Command::Edit) {
                self.toggle_edit();
            }
            return EditorOutcome::Consumed;
        }
        if table::lookup(KEYS, std::slice::from_ref(key)) == TableMatch::Prefix {
            self.edit_chord = Some(*key);
            return EditorOutcome::Consumed;
        }
        if self.edit.is_some() {
            return self.on_edit_key(&key.typed());
        }
        if self.nav.in_sequence() {
            return self.on_motion_key(key);
        }
        match table::find(KEYS, key).and_then(|command| self.run(command)) {
            Some(outcome) => outcome,
            None => self.on_motion_key(key),
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        if self.edit.is_some() {
            return table::help(EDIT_KEYS);
        }
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        self.list.view_mut().release();
        let Some(answer) = reply.answer else {
            return EditorOutcome::Consumed;
        };
        match reply.tag {
            ASK_GLOB => {
                let glob = Some(answer.trim().to_string()).filter(|glob| !glob.is_empty());
                self.reread(|options| options.glob = glob)
            }
            _ => self.start_search(&answer),
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_QUERY && query.trim().len() >= 2 {
            self.start_search(query)
        } else {
            EditorOutcome::Consumed
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match reply {
            JobReply::Search(found) => self.on_found(found),
            // The editor runs no commands and asks for no directory totals.
            JobReply::Command(_)
            | JobReply::DirSize { .. }
            | JobReply::Files(_)
            | JobReply::FileOps(_)
            | JobReply::Processes(_)
            | JobReply::System(_) => {}
        }
        EditorOutcome::Consumed
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        Some(OpenTarget::file(self.root.clone()))
    }

    fn next_tick(&self) -> Option<Instant> {
        if self.search_on_open {
            return Some(Instant::now());
        }
        self.writing
            .as_ref()
            .map(|_| Instant::now() + background::POLL)
    }

    fn on_tick(&mut self, _now: Instant) -> EditorOutcome {
        if self.search_on_open {
            let query = self.query.clone();
            return self.start_search(&query);
        }
        if let Some(report) = self.writing.as_ref().and_then(Background::poll) {
            self.writing = None;
            self.take_write(report);
        }
        EditorOutcome::Consumed
    }

    fn caret(&self) -> Option<Caret> {
        let edit = self.edit.as_ref()?;
        if self.rows.get(self.list.cursor())?.heading {
            return None;
        }
        Some(Caret {
            col: LINE_WIDTH + LINE_GAP.len() + edit.col(),
            insert: edit.modal().is_typing(),
        })
    }

    /// Clipboard text goes into the line being edited, and nowhere else.
    fn on_paste(&mut self, text: String) -> EditorOutcome {
        if let Some(edit) = self.edit.as_mut() {
            edit.paste(&text);
        }
        EditorOutcome::Consumed
    }

    fn is_dirty(&self) -> bool {
        !self.edited().is_empty()
    }

    fn cwd(&self) -> Option<PathBuf> {
        // The file the cursor is in, falling back to what was searched when
        // the cursor is on a heading or there is nothing to point at.
        self.selected()
            .and_then(|hit| hit.path.parent())
            .map(PathBuf::from)
            .or_else(|| Some(self.root.clone()))
    }

    fn address(&self) -> Option<String> {
        Some(crate::model::path::abbreviate_home(&self.root))
    }

    fn file_reference(&self) -> Option<String> {
        if let Some(hit) = self.selected() {
            let base = crate::model::path::abbreviate_home(&hit.path);
            return Some(format!("{base}:{}", hit.line));
        }
        Some(crate::model::path::abbreviate_home(&self.root))
    }

    fn context_items(&self) -> Vec<EditorMenuItem> {
        // Everything but a new search acts on the match under the cursor, so
        // a row that is not one (a file heading, the header) offers only that.
        let on_match: &[Command] = match self.selected() {
            Some(_) => &MATCH_MENU,
            None => &[],
        };
        let group = |commands: &[Command]| {
            commands
                .iter()
                .filter_map(|&command| table::menu_entry(KEYS, command))
                .map(|(key, label)| EditorMenuItem::new(key, label))
                .collect()
        };
        let options: &[Command] = match self.pattern {
            SearchPattern::Text => &OPTION_MENU,
            SearchPattern::Words => &[],
        };
        menu_groups([group(on_match), group(&ROW_MENU), group(options)])
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// Write `by_file`'s edits back, each file read and written once. A line is
/// written only where the file still says what the search found there, its
/// trailing whitespace, which the search never showed, kept.
fn write_back(by_file: BTreeMap<PathBuf, FileEdits>) -> WriteBack {
    let mut report = WriteBack::default();
    for (path, file) in by_file {
        let Ok(mut loaded) = text_file::load(&path) else {
            report.refused += file.edits.len();
            continue;
        };
        let mut wrote_here = 0;
        for edit in &file.edits {
            let Some(line) = loaded.lines.get_mut(edit.line.saturating_sub(1)) else {
                report.refused += 1;
                continue;
            };
            let trimmed = line.trim_end();
            if trimmed != edit.found {
                report.refused += 1;
                continue;
            }
            let trailing = line[trimmed.len()..].to_string();
            *line = format!("{}{trailing}", edit.text);
            wrote_here += 1;
        }
        if wrote_here == 0 {
            continue;
        }
        match text_file::save(&path, &loaded.lines, loaded.shape) {
            Ok(_) => {
                report.lines.extend(file.hits.iter().filter_map(|hit| {
                    let text = loaded.lines.get(hit.line.saturating_sub(1))?;
                    Some(WrittenLine {
                        hit: hit.hit,
                        text: text.trim_end().to_string(),
                    })
                }));
                report.written += wrote_here;
                report.files += 1;
            }
            Err(_) => report.refused += wrote_here,
        }
    }
    report
}

/// A file's row: its path, shortened against the root it was searched under.
fn heading_row(root: &Path, path: &Path) -> Row {
    vec![Span::new(Style::Header, relative_to(root, path))]
}

/// A match's row: the line number, then the line.
fn hit_row(hit: &SearchHit) -> Row {
    vec![
        Span::new(Style::Dim, line_label(hit.line)),
        Span::plain(hit.text.clone()),
    ]
}

/// A line number as it labels its text.
fn line_label(line: usize) -> String {
    format!("{line:>LINE_WIDTH$}{LINE_GAP}")
}

/// `n lines`, singular where it should be.
fn lines_count(count: usize) -> String {
    match count {
        1 => "1 line".to_string(),
        n => format!("{n} lines"),
    }
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
            sup: false,
        }
    }

    fn alt(code: KeyCode) -> Key {
        Key {
            alt: true,
            code,
            ctrl: false,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_the_emacs_buffer_ends_reach_the_first_and_last_row() {
        let mut editor = editor_with(vec![
            hit("a.rs", 1, "thing"),
            hit("a.rs", 9, "thing again"),
            hit("b.rs", 3, "thing"),
        ]);
        editor.on_key(&Key {
            shift: true,
            ..alt(KeyCode::Char('>'))
        });
        assert_eq!(editor.list.cursor(), editor.rows.len() - 1);
        editor.on_key(&Key {
            shift: true,
            ..alt(KeyCode::Char('<'))
        });
        assert_eq!(editor.list.cursor(), 0);
    }

    fn hit(path: &str, line: usize, text: &str) -> SearchHit {
        SearchHit {
            line,
            path: PathBuf::from("/repo").join(path),
            text: text.to_string(),
        }
    }

    /// An editor holding `hits`, as if a walk for "thing" had just answered.
    fn editor_with(hits: Vec<SearchHit>) -> GrepEditor {
        let mut editor = GrepEditor::new(PathBuf::from("/repo"));
        editor.query = "thing".to_string();
        editor.on_job(JobReply::Search(SearchResult {
            error: None,
            hits,
            query: "thing".to_string(),
            truncated: false,
        }));
        editor
    }

    #[test]
    fn test_g_and_gg_reach_the_last_and_first_row() {
        // `G` used to run the search again, and the results had no `gg` at
        // all, since nothing past `j` and `k` reached the shared motions.
        let mut editor = editor_with(vec![hit("a.rs", 1, "thing"), hit("b.rs", 2, "thing")]);
        editor.on_key(&press(KeyCode::Char('G')));
        assert_eq!(editor.list.cursor(), editor.rows.len() - 1);
        editor.on_key(&press(KeyCode::Char('g')));
        editor.on_key(&press(KeyCode::Char('g')));
        assert_eq!(editor.list.cursor(), 0);
    }

    #[test]
    fn test_the_start_directory_follows_the_hit_under_the_cursor() {
        // An editor opened from a result should land beside the file being read,
        // not at the top of whatever tree was searched, so stepping onto a
        // hit in another directory has to move the answer with it.
        let mut editor = editor_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("crates/deep/b.rs", 9, "thing"),
        ]);
        editor.move_by(1);
        assert_eq!(editor.cwd(), Some(PathBuf::from("/repo/src")));
        while editor.selected().map(|h| h.line) != Some(9) {
            editor.move_by(1);
        }
        assert_eq!(editor.cwd(), Some(PathBuf::from("/repo/crates/deep")));
    }

    #[test]
    fn test_a_write_slower_than_its_key_lands_on_a_later_tick() {
        // The write runs off the window's thread; one that outlasts the
        // key's wait is taken in by the tick, and a second edit waits on it.
        let dir = std::env::temp_dir().join(format!("winter-grep-slow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("a.txt");
        std::fs::write(&file, "thing\n").expect("write");
        let mut editor = GrepEditor::new(dir.clone());
        editor.query = "thing".to_string();
        editor.on_job(JobReply::Search(SearchResult {
            error: None,
            hits: vec![SearchHit {
                line: 1,
                path: file.clone(),
                text: "thing".to_string(),
            }],
            query: "thing".to_string(),
            truncated: false,
        }));
        editor.write_wait = Duration::ZERO;
        editor.toggle_edit();
        if let Some(edit) = editor.edit.as_mut() {
            edit.paste(" more");
        }
        editor.apply_edits();
        let deadline = Instant::now() + Duration::from_secs(10);
        while editor.writing.is_some() && Instant::now() < deadline {
            assert!(editor.next_tick().is_some(), "a tick is asked for");
            std::thread::sleep(Duration::from_millis(5));
            editor.on_tick(Instant::now());
        }
        assert!(editor.writing.is_none(), "the write landed");
        assert_eq!(
            std::fs::read_to_string(&file).expect("read"),
            "thing more\n"
        );
        assert_eq!(
            editor.hits[0].text, "thing more",
            "the hit reads what was written"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_edited_matches_write_back_and_a_line_changed_on_disk_is_left_alone() {
        let dir = std::env::temp_dir().join(format!("winter-grep-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let kept = dir.join("kept.txt");
        let moved = dir.join("moved.txt");
        std::fs::write(&kept, "one\r\nthing here  \r\nthree\r\n").expect("write");
        std::fs::write(&moved, "thing\n").expect("write");
        let mut editor = GrepEditor::new(dir.clone());
        editor.query = "thing".to_string();
        editor.on_job(JobReply::Search(SearchResult {
            error: None,
            hits: vec![
                SearchHit {
                    line: 2,
                    path: kept.clone(),
                    text: "thing here".to_string(),
                },
                SearchHit {
                    line: 1,
                    path: moved.clone(),
                    text: "thing".to_string(),
                },
            ],
            query: "thing".to_string(),
            truncated: false,
        }));
        // Something else rewrites one of the files after the search.
        std::fs::write(&moved, "other\n").expect("write");

        let ctrl = |c| Key {
            ctrl: true,
            ..press(KeyCode::Char(c))
        };
        editor.on_key(&ctrl('x'));
        editor.on_key(&ctrl('q'));
        assert!(editor.edit.is_some(), "editing");
        editor.on_key(&press(KeyCode::Char('j')));
        for c in "ccstuff".chars() {
            editor.on_key(&press(KeyCode::Char(c)));
        }
        editor.on_key(&press(KeyCode::Escape));
        editor.move_by(-3);
        for c in "0cwnew".chars() {
            editor.on_key(&press(KeyCode::Char(c)));
        }
        editor.on_key(&press(KeyCode::Enter));

        assert!(editor.edit.is_none(), "Enter wrote and left editing");
        assert_eq!(
            std::fs::read_to_string(&kept).expect("read"),
            "one\r\nnew here  \r\nthree\r\n",
            "the line changed, its trailing blanks and the CRLF endings kept"
        );
        assert_eq!(std::fs::read_to_string(&moved).expect("read"), "other\n");
        let message = editor
            .take_notice()
            .map(|notice| notice.text)
            .unwrap_or_default();
        assert!(
            message.contains("1 line changed on disk"),
            "got {message:?}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_a_search_with_nothing_to_point_at_starts_where_it_searched() {
        let editor = GrepEditor::new(PathBuf::from("/repo"));
        assert_eq!(editor.cwd(), Some(PathBuf::from("/repo")));
    }

    /// Ask for `text` through the query prompt, the way a key does.
    fn ask(editor: &mut GrepEditor, text: &str) -> EditorOutcome {
        let outcome = editor.on_key(&press(KeyCode::Char('/')));
        let EditorOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_QUERY);
        editor.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        })
    }

    fn row_texts(editor: &GrepEditor) -> Vec<String> {
        editor
            .rows
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
        let mut editor = GrepEditor::new(PathBuf::from("/repo"));
        let outcome = ask(&mut editor, "needle");
        assert_eq!(
            outcome,
            EditorOutcome::Job(JobRequest::Search(SearchRequest::new(
                SearchPattern::Text,
                "needle".to_string(),
                PathBuf::from("/repo")
            )))
        );
        assert!(editor.searching, "the header has to be able to say so");
    }

    #[test]
    fn test_the_todo_list_searches_its_keywords_as_words_on_its_first_tick() {
        let keywords = ["TODO".to_string(), "FIXME".to_string()];
        let mut editor = GrepEditor::todo(PathBuf::from("/repo"), &keywords);
        assert_eq!(editor.title(), TODO_TITLE);
        assert!(editor.next_tick().is_some(), "it has to ask to be ticked");
        assert_eq!(
            editor.on_tick(Instant::now()),
            EditorOutcome::Job(JobRequest::Search(SearchRequest::new(
                SearchPattern::Words,
                "TODO FIXME",
                PathBuf::from("/repo"),
            )))
        );
        // Asked once: a second tick would throw away what the first found.
        assert!(editor.next_tick().is_none());
    }

    #[test]
    fn test_a_new_question_in_the_todo_list_still_reads_words() {
        let mut editor = GrepEditor::todo(PathBuf::from("/repo"), &["TODO".to_string()]);
        let EditorOutcome::Prompt(request) = editor.on_key(&press(KeyCode::Char('/'))) else {
            panic!("expected a prompt");
        };
        assert_eq!(request.initial, "TODO", "the keywords are there to edit");
        let outcome = editor.on_prompt(PromptReply {
            answer: Some("HACK".to_string()),
            tag: request.tag,
        });
        let EditorOutcome::Job(JobRequest::Search(search)) = outcome else {
            panic!("expected a search, got {outcome:?}");
        };
        assert_eq!(search.pattern, SearchPattern::Words);
        assert_eq!(search.query, "HACK");
    }

    #[test]
    fn test_an_empty_query_searches_for_nothing() {
        // A walk for "" would report every line of every file under the root.
        let mut editor = GrepEditor::new(PathBuf::from("/repo"));
        assert_eq!(ask(&mut editor, ""), EditorOutcome::Consumed);
        assert!(!editor.searching);
    }

    #[test]
    fn test_matches_are_grouped_under_one_heading_per_file() {
        let editor = editor_with(vec![
            hit("src/a.rs", 4, "one thing"),
            hit("src/a.rs", 9, "another thing"),
            hit("src/b.rs", 2, "thing again"),
        ]);
        let texts = row_texts(&editor);
        assert_eq!(texts.len(), 5, "two headings and three matches: {texts:?}");
        assert_eq!(texts[0].trim(), "src/a.rs");
        assert_eq!(texts[1].trim(), "4: one thing");
        assert_eq!(texts[3].trim(), "src/b.rs");
    }

    #[test]
    fn test_enter_opens_the_file_at_the_line_that_matched() {
        let mut editor = editor_with(vec![hit("src/a.rs", 4, "one thing")]);
        // Row 0 is the heading, row 1 the match.
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 4))
        );
    }

    #[test]
    fn test_enter_on_a_heading_opens_that_file_at_its_first_match() {
        // Opening it at line one would throw away what the user searched for.
        let mut editor = editor_with(vec![
            hit("src/a.rs", 12, "one thing"),
            hit("src/a.rs", 30, "another thing"),
        ]);
        assert_eq!(
            editor.on_key(&press(KeyCode::Enter)),
            EditorOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 12))
        );
    }

    #[test]
    fn test_the_file_motions_step_over_a_file_rather_than_through_it() {
        let mut editor = editor_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("src/a.rs", 2, "thing"),
            hit("src/b.rs", 3, "thing"),
        ]);
        editor.on_key(&alt(KeyCode::Char('n')));
        assert_eq!(row_texts(&editor)[editor.list.cursor()].trim(), "src/b.rs");

        editor.on_key(&alt(KeyCode::Char('p')));
        assert_eq!(row_texts(&editor)[editor.list.cursor()].trim(), "src/a.rs");
    }

    #[test]
    fn test_the_match_motions_skip_the_names_between_the_matches() {
        let mut editor = editor_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("src/b.rs", 3, "thing"),
        ]);
        // Rows: heading, match, heading, match. The cursor starts on the first
        // heading, so one step lands on a match and the next skips a heading.
        editor.on_key(&press(KeyCode::Char('n')));
        assert_eq!(row_texts(&editor)[editor.list.cursor()].trim(), "1: thing");

        editor.on_key(&press(KeyCode::Char('n')));
        assert_eq!(row_texts(&editor)[editor.list.cursor()].trim(), "3: thing");

        editor.on_key(&press(KeyCode::Char('N')));
        assert_eq!(row_texts(&editor)[editor.list.cursor()].trim(), "1: thing");
    }

    #[test]
    fn test_an_answer_to_a_query_already_replaced_is_dropped() {
        // Two walks can be out at once, and the slower one would otherwise
        // paint its matches under the newer query's name.
        let mut editor = editor_with(vec![hit("src/a.rs", 1, "thing")]);
        editor.query = "other".to_string();

        editor.on_job(JobReply::Search(SearchResult {
            error: None,
            hits: vec![hit("src/b.rs", 9, "thing")],
            query: "thing".to_string(),
            truncated: false,
        }));
        assert_eq!(row_texts(&editor).len(), 2, "the older answer was ignored");
        assert_eq!(row_texts(&editor)[1].trim(), "1: thing");
    }

    #[test]
    fn test_a_walk_that_found_nothing_says_so() {
        let mut editor = editor_with(Vec::new());
        assert!(editor.take_notice().is_some(), "the miss is reported");
        assert!(editor.rows.is_empty());
    }

    #[test]
    fn test_a_capped_walk_says_the_list_is_partial() {
        // Without this the first five hundred matches look like all of them.
        let mut editor = GrepEditor::new(PathBuf::from("/repo"));
        editor.query = "thing".to_string();
        editor.on_job(JobReply::Search(SearchResult {
            error: None,
            hits: vec![hit("src/a.rs", 1, "thing")],
            query: "thing".to_string(),
            truncated: true,
        }));
        let header: String = editor
            .header_row()
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        assert!(header.contains("capped"), "header reads: {header}");
    }

    #[test]
    fn test_a_new_query_drops_what_the_last_one_found() {
        // Results half from one query and half from another answer neither.
        let mut editor = editor_with(vec![hit("src/a.rs", 1, "thing")]);
        ask(&mut editor, "other");
        assert!(editor.rows.is_empty());
        assert_eq!(editor.query, "other");
    }

    #[test]
    fn test_ctrl_shift_c_writes_and_ctrl_shift_k_throws_away_the_edits_mid_typing() {
        let dir = std::env::temp_dir().join(format!("winter-grep-chords-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("a.txt");
        std::fs::write(&file, "thing\n").expect("write");
        let mut editor = GrepEditor::new(dir.clone());
        editor.query = "thing".to_string();
        editor.on_job(JobReply::Search(SearchResult {
            error: None,
            hits: vec![SearchHit {
                line: 1,
                path: file.clone(),
                text: "thing".to_string(),
            }],
            query: "thing".to_string(),
            truncated: false,
        }));
        let chord = |c: char, shift: bool| Key {
            ctrl: true,
            shift,
            ..press(KeyCode::Char(c))
        };
        let edit_to = |editor: &mut GrepEditor, text: &str| {
            editor.on_key(&chord('x', false));
            editor.on_key(&chord('q', false));
            assert!(editor.edit.is_some(), "editing");
            editor.on_key(&press(KeyCode::Char('j')));
            for c in format!("cc{text}").chars() {
                editor.on_key(&press(KeyCode::Char(c)));
            }
        };
        edit_to(&mut editor, "gone");
        editor.on_key(&chord('K', true));
        assert!(editor.edit.is_none(), "thrown away while still typing");
        assert_eq!(std::fs::read_to_string(&file).expect("read"), "thing\n");

        edit_to(&mut editor, "stuff");
        editor.on_key(&chord('C', true));
        let written = std::fs::read_to_string(&file).unwrap_or_default();
        let _ = std::fs::remove_dir_all(&dir);
        assert!(editor.edit.is_none());
        assert_eq!(written, "stuff\n");
    }

    #[test]
    fn test_escape_stops_a_running_walk_and_never_closes_the_editor() {
        let mut editor = GrepEditor::new(PathBuf::from("/repo"));
        ask(&mut editor, "needle");
        assert_eq!(
            editor.on_key(&press(KeyCode::Escape)),
            EditorOutcome::CancelJobs
        );
        assert_ne!(editor.on_key(&press(KeyCode::Escape)), EditorOutcome::Close);
    }

    #[test]
    fn test_yanking_copies_where_the_match_is() {
        let mut editor = editor_with(vec![hit("src/a.rs", 4, "one thing")]);
        editor.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            editor.on_key(&press(KeyCode::Char('y'))),
            EditorOutcome::Yank("src/a.rs:4".to_string())
        );
    }

    #[test]
    fn test_every_key_of_the_editor_is_spelled_right() {
        assert_eq!(table::invalid_specs(KEYS), Vec::<&str>::new());
        assert_eq!(table::invalid_specs(EDIT_KEYS), Vec::<&str>::new());
    }

    /// The search an outcome starts.
    fn search_of(outcome: &EditorOutcome) -> SearchRequest {
        match outcome {
            EditorOutcome::Job(JobRequest::Search(request)) => request.clone(),
            other => panic!("expected a search, got {other:?}"),
        }
    }

    #[test]
    fn test_a_toggle_searches_again_reading_the_query_its_new_way() {
        let mut editor = GrepEditor::new(PathBuf::from("/tree"));
        ask(&mut editor, "Needle");
        let again = search_of(&editor.on_key(&alt(KeyCode::Char('c'))));
        assert_eq!(again.query, "Needle");
        assert!(again.options.case_sensitive);
        let again = search_of(&editor.on_key(&alt(KeyCode::Char('w'))));
        assert!(again.options.case_sensitive && again.options.whole_word);
        let again = search_of(&editor.on_key(&alt(KeyCode::Char('c'))));
        assert!(!again.options.case_sensitive, "toggled back");
    }

    #[test]
    fn test_this_type_narrows_to_the_files_like_the_one_under_the_cursor() {
        let mut editor = editor_with(vec![hit("/tree/src/a.rs", 4, "needle")]);
        editor.list.set_cursor(1);
        let narrowed = search_of(&editor.on_key(&alt(KeyCode::Char('t'))));
        assert_eq!(narrowed.options.glob.as_deref(), Some("*.rs"));
        editor.on_found(SearchResult {
            hits: vec![hit("/tree/src/a.rs", 4, "needle")],
            query: editor.query.clone(),
            ..SearchResult::default()
        });
        editor.list.set_cursor(1);
        let widened = search_of(&editor.on_key(&alt(KeyCode::Char('t'))));
        assert_eq!(widened.options.glob, None, "pressed again: every file");
    }

    #[test]
    fn test_an_earlier_search_is_offered_newest_first_and_run_again() {
        let mut editor = GrepEditor::new(PathBuf::from("/tree"));
        ask(&mut editor, "first");
        ask(&mut editor, "second");
        let EditorOutcome::Pick(pick) = editor.on_key(&alt(KeyCode::Char('h'))) else {
            panic!("a list of earlier searches");
        };
        assert_eq!(pick.items, ["second", "first"]);
        let rerun = search_of(&editor.on_prompt(PromptReply {
            answer: Some("first".to_string()),
            tag: pick.tag,
        }));
        assert_eq!(rerun.query, "first");
    }

    #[test]
    fn test_a_regular_expression_that_does_not_parse_is_said_rather_than_found_nothing() {
        let mut editor = GrepEditor::new(PathBuf::from("/tree"));
        editor.on_key(&alt(KeyCode::Char('r')));
        ask(&mut editor, "(oops");
        editor.on_found(SearchResult {
            error: Some("not a regular expression: unclosed group".to_string()),
            query: "(oops".to_string(),
            ..SearchResult::default()
        });
        let notice = editor.take_notice().expect("a notice");
        assert!(notice.text.contains("not a regular expression"));
    }

    #[test]
    fn test_the_todo_list_keeps_reading_whole_words() {
        let mut editor = GrepEditor::todo(PathBuf::from("/tree"), &["TODO".to_string()]);
        assert_eq!(
            editor.on_key(&alt(KeyCode::Char('c'))),
            EditorOutcome::Consumed
        );
        assert_eq!(editor.options, SearchOptions::default());
    }
}

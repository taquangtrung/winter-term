//! Native command palette: a lightweight text-mode overlay activated by
//! `M-x` (or the configured key). Renders as GPU quads at the top
//! of the focused pane, no WebView required.

mod disk;

use std::collections::HashSet;
use std::fmt::Debug;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use disk::{disk_dirs, read_shell_history};

use super::editor::JumpTarget;
use super::history::EditHistory;
use super::input::{Key, Keymap};
use super::paint::IconKind;
use super::path::{clean_path, expand_home};
use super::textedit::field::TextField;
use super::textedit::modal::EditOutcome;

use regex::RegexBuilder;

// ========================================================================
// Constants
// ========================================================================

const HISTORY_MAX: usize = 500;

/// zsh's meta byte: in its history file a byte that is not plain text is
/// written as this, then the byte with [`ZSH_META_FLIP`] flipped.
const ZSH_META: u8 = 0x83;
const ZSH_META_FLIP: u8 = 0x20;

// ========================================================================
// Data Structures
// ========================================================================

/// Whether the palette is showing built-in commands, shell history, or recent dirs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PaletteMode {
    /// Choosing a command to run.
    #[default]
    Commands,
    /// Browsing a directory: a row is somewhere to go or something to open,
    /// and the query filters the one directory being shown rather than the
    /// tree under it.
    Files,
    /// Recalling an earlier query.
    History,
    /// Every editor tab: the ones open somewhere, then what was closed recently.
    /// Selecting an open one goes to it, and a closed one comes back as the
    /// editor it was rather than as a fresh editor of its kind.
    TabList,
    /// The editors able to show the focused tab's file, other than the one
    /// showing it: selecting one opens the file in it.
    OpenWith,
    /// Pane switcher: selecting an entry focuses that pane.
    Panes,
    /// The keys of the focused tab, each beside what it does. Nothing is run
    /// from it: the list is read and filtered, then put away.
    TabKeys,
    /// The recent list: every tab open now, then the files and directories
    /// opened lately. Selecting an entry goes to the tab or opens the place.
    Recent,
    /// `cd` target picker: selecting an entry executes `cd <dir>` immediately.
    RecentDirs,
    /// Every file in the focused tab's project, by its path in the project:
    /// selecting one opens it.
    ProjectFiles,
    /// An editor's own list: the branches to check out, the tags to delete.
    /// Selecting an entry answers the editor's question rather than running a
    /// command of the host's.
    EditorPick,
    /// Buffer swoop: fuzzy line search over the active pane's scrollback and grid.
    Swoop,
    /// Jump to target in current buffer view (fuzzy completion dialog).
    Jump,
    /// Mux session switcher: selecting an entry attaches or switches to that daemon session.
    MuxSessions,
    /// Mux session killer: selecting an entry terminates that daemon session.
    MuxKill,
    /// Mux session creator: a free-text prompt: the *query* is the payload
    /// ("name [command]"), never a filter, and `Enter` spawns the session.
    MuxNew,
    /// Remote mux attach: a free-text prompt: the *query* is the payload
    /// ("host [session]"), never a filter, and `Enter` attaches over ssh.
    MuxAttachRemote,
    /// The project searched as the query is typed: the query is a pattern
    /// for ripgrep, optionally followed by ` @ <file filter>`, and the rows
    /// are the lines it finds, streamed in as they come. Only the filter
    /// narrows the rows here; the pattern is matched by the search itself.
    LiveSearch,
}

/// One selectable row: what it shows and the command it runs.
#[derive(Clone, Debug)]
pub struct PaletteEntry {
    /// The command dispatched when this row is chosen.
    pub action: String,
    /// What the row is, drawn as an icon before its label: a file's type, a
    /// folder. `None` for a row that is not a place, such as a command.
    pub icon: Option<IconKind>,
    /// The text shown for this row.
    pub label: String,
    /// Char indices in `label` that matched the current query (for highlight).
    pub match_positions: Vec<usize>,
    /// Keyboard shortcut hint shown on the right (e.g. `"C-S-t"`).
    pub shortcut: String,
}

/// One entry of a directory, as the file browser lists it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DirItem {
    /// Whether it is a directory, somewhere to go rather than to open.
    pub is_dir: bool,
    /// Its name within the directory.
    pub name: String,
    /// Its full path.
    pub path: PathBuf,
}

/// The palette's open state: its query, matching entries, and selection.
#[derive(Clone, Debug)]
pub struct Palette {
    /// Whether the palette is currently open.
    pub active: bool,
    /// The directory a file browser is showing, and nothing for every other
    /// mode, which lists something other than a place.
    pub dir: Option<PathBuf>,
    /// Where the file browser reads directories from, as it re-roots.
    pub dirs: Arc<dyn DirSource>,
    /// Every candidate row, before filtering.
    pub entries: Vec<PaletteEntry>,
    /// The query as a one-line field, for its Emacs keys: it keeps the caret
    /// and the kill ring between keys. The query is what the rest reads; this
    /// follows it wherever something else sets it.
    field: Box<TextField>,
    /// Indices into the entries that match the query, in match order.
    pub filtered: Vec<usize>,
    /// Undo/redo stack over `query`, driven by `C-/` and `C-\`.
    pub history: EditHistory<String>,
    /// Position in `query_history` currently recalled (`0` = most recent).
    /// `None` indicates the live, uncommitted query is displayed.
    pub history_index: Option<usize>,
    /// The live query snapshot saved when history navigation begins.
    pub live_query: String,
    /// Whether the palette is listing commands or past queries.
    pub mode: PaletteMode,
    /// What the user has typed so far.
    pub query: String,
    /// Executed query history (most-recent-first).
    pub query_history: Vec<String>,
    /// Index into the filtered rows, not into the entries.
    pub selected: usize,
    /// Custom title shown above the palette query, or None to infer from mode.
    pub title: Option<String>,
}

// ========================================================================
// Traits
// ========================================================================

/// Where the file browser reads the directories it shows. The palette itself
/// touches no filesystem: the app hands it the real one, and a test a fixed
/// tree.
pub trait DirSource: Debug + Send + Sync {
    /// The entries of `dir`, or `None` when it cannot be read.
    fn read_dir(&self, dir: &Path) -> Option<Vec<DirItem>>;

    /// Whether `path` is a directory that can be browsed.
    fn is_dir(&self, path: &Path) -> bool;

    /// The directory `..` leads to from `dir`, or `None` at the top.
    fn parent(&self, dir: &Path) -> Option<PathBuf> {
        dir.parent().map(Path::to_path_buf)
    }

    /// The root of the filesystem `dir` is on, where a `/` typed alone goes.
    fn root(&self, dir: &Path) -> PathBuf {
        dir.ancestors().last().unwrap_or(dir).to_path_buf()
    }

    /// Where `query`, typed as a path, leads from `dir`.
    fn join(&self, dir: &Path, query: &str) -> PathBuf {
        clean_path(&dir.join(expand_home(query)))
    }
}

// ========================================================================
// Implementation
// ========================================================================

impl Default for Palette {
    fn default() -> Self {
        Self {
            active: false,
            dir: None,
            dirs: disk_dirs(),
            entries: Vec::new(),
            field: Box::new(TextField::line("")),
            filtered: Vec::new(),
            history: EditHistory::default(),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::default(),
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }
}

impl PaletteEntry {
    /// A row called `label` that runs `action`, with `shortcut` on its right,
    /// drawn without an icon.
    pub fn new(
        action: impl Into<String>,
        label: impl Into<String>,
        shortcut: impl Into<String>,
    ) -> Self {
        Self {
            action: action.into(),
            icon: None,
            label: label.into(),
            match_positions: Vec::new(),
            shortcut: shortcut.into(),
        }
    }

    /// The same row, drawn with `icon` before its label.
    pub fn with_icon(self, icon: IconKind) -> Self {
        Self {
            icon: Some(icon),
            ..self
        }
    }
}

impl Palette {
    /// Open over the command list, labelling entries with their bound chords.
    pub fn open(keymap: &Keymap) -> Self {
        let entries = builtin_commands(keymap);
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Commands,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open over the shell's own history, read from where the shell keeps it.
    pub fn open_history() -> Self {
        Self::open_history_of(&read_shell_history())
    }

    /// Open over the commands of a shell history file's raw `contents`, most
    /// recent first.
    pub fn open_history_of(contents: &[u8]) -> Self {
        let entries = history_entries(contents);
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::History,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette in recent-dirs mode. `dirs` is a deduplicated list of
    /// working directories ordered most-recently-used first.
    pub fn open_recent_dirs(dirs: Vec<String>) -> Self {
        let entries = dirs
            .into_iter()
            .map(|dir| PaletteEntry {
                action: dir.clone(),
                icon: Some(folder_icon(Path::new(&dir))),
                label: dir,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::RecentDirs,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette over every editor tab: the ones open somewhere first,
    /// then what was closed recently, newest first.
    ///
    /// Each entry's action says which pane a row goes to or which closed
    /// thing it puts back, named rather than numbered by position, so a row
    /// means the same thing however the list has changed since it was drawn.
    pub fn open_tab_list(entries: Vec<PaletteEntry>) -> Self {
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::TabList,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette over the recent list: each entry's action says which
    /// tab, file, or directory it stands for.
    pub fn open_recent(entries: Vec<PaletteEntry>) -> Self {
        Palette {
            mode: PaletteMode::Recent,
            ..Self::open_tab_list(entries)
        }
    }

    /// Open the palette for a project search, with no rows until the query
    /// finds some. `title` names what is searched.
    pub fn open_live_search(title: String) -> Self {
        Palette {
            mode: PaletteMode::LiveSearch,
            title: Some(title),
            ..Self::open_tab_list(Vec::new())
        }
    }

    /// Name what the palette shows, over its query line.
    pub fn set_title(&mut self, title: String) {
        self.title = Some(title);
    }

    /// Add rows found since the last ones, keeping the highlighted row where
    /// it is: new rows land after it, so the eye's place does not move under
    /// it while a search is still streaming in.
    pub fn append_entries(&mut self, rows: Vec<PaletteEntry>) {
        let selected = self.selected;
        self.entries.extend(rows);
        self.update_filter();
        self.selected = selected.min(self.filtered.len().saturating_sub(1));
    }

    /// Drop every row, for a search started over.
    pub fn clear_entries(&mut self) {
        self.entries.clear();
        self.filtered.clear();
        self.selected = 0;
    }

    /// Open the palette over a project's files, `title` naming the project.
    /// Each entry's action is the file's full path.
    pub fn open_project_files(entries: Vec<PaletteEntry>, title: String) -> Self {
        Palette {
            mode: PaletteMode::ProjectFiles,
            title: Some(title),
            ..Self::open_tab_list(entries)
        }
    }

    /// Open the palette over the keys of the focused tab, under `title`.
    /// Each entry's label says what a key does, its shortcut the key.
    pub fn open_tab_keys(entries: Vec<PaletteEntry>, title: String) -> Self {
        Palette {
            mode: PaletteMode::TabKeys,
            title: Some(title),
            ..Self::open_tab_list(entries)
        }
    }

    /// Open the palette over the editors that can show a file, under `title`.
    /// Each entry's action names an editor kind.
    pub fn open_with(entries: Vec<PaletteEntry>, title: String) -> Self {
        Palette {
            mode: PaletteMode::OpenWith,
            title: Some(title),
            ..Self::open_tab_list(entries)
        }
    }

    /// Open the palette in pane switcher mode. Each entry's action is the
    /// tab's id, its shortcut the tab's place on its pane's strip.
    pub fn open_panes(entries: Vec<PaletteEntry>) -> Self {
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Panes,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette over an editor's own list, where each entry stands
    /// for itself: what is chosen is the text of the row.
    ///
    /// `icons` are drawn before the items they line up with, and an item past
    /// the last of them is drawn without one.
    pub fn open_pick(items: Vec<String>, icons: Vec<IconKind>) -> Self {
        let entries = items
            .into_iter()
            .zip(icons.into_iter().map(Some).chain(std::iter::repeat(None)))
            .map(|(item, icon)| PaletteEntry {
                action: item.clone(),
                icon,
                label: item,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::EditorPick,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette in buffer swoop mode over `lines`, which are `(abs_row, text)` pairs.
    pub fn open_swoop(lines: Vec<(usize, String)>) -> Self {
        let entries = lines
            .into_iter()
            .map(|(abs_row, line)| {
                let line_num = abs_row + 1;
                PaletteEntry {
                    action: abs_row.to_string(),
                    icon: None,
                    label: format!("{line_num:>5}  {line}"),
                    match_positions: Vec::new(),
                    shortcut: String::new(),
                }
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Swoop,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette in jump mode over `targets`.
    pub fn open_jump(targets: Vec<JumpTarget>, title: Option<String>) -> Self {
        let entries = targets
            .into_iter()
            .map(|target| PaletteEntry {
                action: target.target.to_string(),
                icon: target.icon,
                label: target.label,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Jump,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title,
        }
    }

    /// Preselect the entry closest to `target_id` among the filtered items.
    pub fn preselect_nearest(&mut self, target_id: usize) {
        if self.filtered.is_empty() {
            return;
        }
        let mut best_index = 0;
        let mut min_diff = usize::MAX;
        for (filtered_idx, &entry_idx) in self.filtered.iter().enumerate() {
            if let Ok(id) = self.entries[entry_idx].action.parse::<usize>() {
                if id >= target_id {
                    self.selected = filtered_idx;
                    return;
                }
                let diff = target_id.abs_diff(id);
                if diff < min_diff {
                    min_diff = diff;
                    best_index = filtered_idx;
                }
            }
        }
        self.selected = best_index;
    }

    /// Open the palette in mux session switcher mode.
    pub fn open_mux_sessions(sessions: Vec<String>) -> Self {
        let entries = sessions
            .into_iter()
            .map(|name| PaletteEntry {
                action: name.clone(),
                icon: None,
                label: name,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxSessions,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette in mux session killer mode.
    pub fn open_mux_kill(sessions: Vec<String>) -> Self {
        let entries = sessions
            .into_iter()
            .map(|name| PaletteEntry {
                action: name.clone(),
                icon: None,
                label: name,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxKill,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette in mux session creator mode: the query is the input
    /// ("name [command]"), shown against a single static prompt line that
    /// stays visible no matter what is typed.
    pub fn open_mux_new() -> Self {
        let entries = vec![PaletteEntry {
            action: "spawn".to_string(),
            icon: None,
            label: "new session: name [command]".to_string(),
            match_positions: Vec::new(),
            shortcut: String::new(),
        }];
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxNew,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette in remote mux attach mode: the query is the input
    /// ("host [session]"), shown against a single static prompt line that
    /// stays visible no matter what is typed.
    pub fn open_mux_attach_remote() -> Self {
        let entries = vec![PaletteEntry {
            action: "attach".to_string(),
            icon: None,
            label: "attach remote: host [session]".to_string(),
            match_positions: Vec::new(),
            shortcut: String::new(),
        }];
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: None,
            dirs: disk_dirs(),
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxAttachRemote,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Open the palette over `dir`'s own entries: a directory is somewhere
    /// to go, a file is something to open, and `..` is the way back up.
    pub fn open_files(dir: PathBuf) -> Self {
        Self::open_files_in(dir, disk_dirs())
    }

    /// [`Self::open_files`], reading the directories from `dirs`.
    pub fn open_files_in(dir: PathBuf, dirs: Arc<dyn DirSource>) -> Self {
        let entries = file_entries(dirs.as_ref(), &dir);
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            dir: Some(dir),
            dirs,
            entries,
            field: Box::new(TextField::line("")),
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Files,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
            title: None,
        }
    }

    /// Attach executed query history to this palette instance.
    pub fn with_query_history(mut self, history: Vec<String>) -> Self {
        self.query_history = history;
        self
    }

    /// Close the palette and discard the current query.
    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.history.reset(String::new());
        self.history_index = None;
        self.live_query.clear();
        self.selected = 0;
    }

    /// Move the selection to the row for `path`, when the listing shows it.
    pub fn select_path(&mut self, path: &Path) {
        let position = self
            .filtered
            .iter()
            .position(|&index| Path::new(&self.entries[index].action) == path);
        if let Some(position) = position {
            self.selected = position;
        }
    }

    /// Append a character to the query and refilter the entries.
    ///
    /// In the browser a path separator ends a directory name rather than
    /// filtering by it: the listing re-roots at the directory typed so far
    /// (see [`Self::typed_dir`]) with a fresh query, so completion goes on
    /// from there.
    pub fn push_char(&mut self, c: char) {
        if self.mode == PaletteMode::Files && std::path::is_separator(c) {
            if let Some(dir) = self.typed_dir() {
                let query_history = std::mem::take(&mut self.query_history);
                let dirs = Arc::clone(&self.dirs);
                *self = Palette::open_files_in(dir, dirs).with_query_history(query_history);
                return;
            }
        }
        self.history_index = None;
        self.query.push(c);
        self.history.record(self.query.clone());
        self.update_filter();
    }

    /// The directory a separator typed after the current query names, for
    /// the browser to re-root at: the root for an empty query (`/` alone),
    /// home for `~`, the query as a path (`src`, `..`, `~/notes`, `/etc`)
    /// against the listed directory when that is a directory, else the
    /// selected row when it is one (`sr` + `/` enters `src/`). `None` when
    /// none of these is a directory, which leaves the separator to filter.
    fn typed_dir(&self) -> Option<PathBuf> {
        let dir = self.dir.as_ref()?;
        let candidate = match self.query.as_str() {
            "" => Some(self.dirs.root(dir)),
            query => Some(self.dirs.join(dir, query))
                .filter(|path| self.dirs.is_dir(path))
                .or_else(|| self.selected_action().map(PathBuf::from)),
        };
        candidate.filter(|path| self.dirs.is_dir(path))
    }

    /// How many characters of the query sit before the caret: wherever the
    /// field's keys left it, or the end when something else set the query.
    pub fn caret(&self) -> usize {
        match self.field.text() == self.query {
            true => self.field.col(),
            false => self.query.chars().count(),
        }
    }

    /// Whether the caret is at the query's end, where typing appends.
    pub fn is_caret_at_end(&self) -> bool {
        self.caret() == self.query.chars().count()
    }

    /// Edit the query at the caret with `key`, as a one-line field takes it
    /// (the Emacs keys of typing), and refilter the entries if it changed.
    /// What the field answered: [`EditOutcome::Cancel`] for `C-g`.
    pub fn edit_query(&mut self, key: &Key) -> EditOutcome {
        if self.field.text() != self.query {
            self.field.set_text(&self.query);
        }
        let outcome = self.field.on_key(key, 1);
        if self.field.text() != self.query {
            self.history_index = None;
            self.query = self.field.text().to_string();
            self.history.record(self.query.clone());
            self.update_filter();
        }
        outcome
    }

    /// Delete the query's last character and refilter the entries.
    pub fn pop_char(&mut self) {
        self.history_index = None;
        self.query.pop();
        self.history.record(self.query.clone());
        self.update_filter();
    }

    /// Recall an older executed query (`M-p`). Replaces the current query
    /// and updates filtering. No-op when query history is empty or already at
    /// the oldest entry.
    pub fn history_prev(&mut self) {
        if self.query_history.is_empty() {
            return;
        }
        let next_idx = match self.history_index {
            None => {
                self.live_query = self.query.clone();
                0
            }
            Some(i) => {
                if i + 1 < self.query_history.len() {
                    i + 1
                } else {
                    return;
                }
            }
        };
        self.history_index = Some(next_idx);
        self.query = self.query_history[next_idx].clone();
        self.history.record(self.query.clone());
        self.update_filter();
    }

    /// Recall a newer executed query (`M-n`), returning to the live typed
    /// query once the newest entry is passed. No-op when not currently
    /// navigating history.
    pub fn history_next(&mut self) {
        let Some(i) = self.history_index else {
            return;
        };
        if i == 0 {
            self.history_index = None;
            self.query = self.live_query.clone();
        } else {
            let next_idx = i - 1;
            self.history_index = Some(next_idx);
            self.query = self.query_history[next_idx].clone();
        }
        self.history.record(self.query.clone());
        self.update_filter();
    }

    /// Restore the previous query state (`C-/`). No-op when nothing to undo.
    pub fn undo(&mut self) {
        self.history_index = None;
        if let Some(query) = self.history.undo() {
            self.query = query.clone();
            self.update_filter();
        }
    }

    /// Re-apply an undone query state (`C-\`). No-op when nothing to redo.
    pub fn redo(&mut self) {
        self.history_index = None;
        if let Some(query) = self.history.redo() {
            self.query = query.clone();
            self.update_filter();
        }
    }

    /// Move the selection one row up, stopping at the first.
    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    /// Move the selection one row down, stopping at the last.
    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    /// The command the highlighted row would run, if any row is highlighted.
    pub fn selected_action(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .map(|&i| self.entries[i].action.as_str())
    }

    /// Whether the browser's selected row is a directory, as the listing
    /// that drew it found: asking the filesystem again would cost a round
    /// trip to a remote one.
    pub fn selected_is_dir(&self) -> bool {
        self.filtered
            .get(self.selected)
            .is_some_and(|&i| matches!(self.entries[i].icon, Some(IconKind::Dir { .. })))
    }

    /// The index into `filtered` of the visible entry whose displayed
    /// shortcut hint exactly matches `shortcut`, if any. Used to jump
    /// straight to a pane switcher entry by its shown digit.
    pub fn position_by_shortcut(&self, shortcut: &str) -> Option<usize> {
        self.filtered
            .iter()
            .position(|&i| self.entries[i].shortcut == shortcut)
    }

    fn update_filter(&mut self) {
        if matches!(
            self.mode,
            PaletteMode::MuxNew | PaletteMode::MuxAttachRemote
        ) {
            // The query is the payload (a session spec), not a filter: the
            // prompt line stays visible no matter what is typed.
            for entry in &mut self.entries {
                entry.match_positions.clear();
            }
            self.filtered = (0..self.entries.len()).collect();
            self.selected = 0;
            return;
        }
        if self.mode == PaletteMode::LiveSearch {
            let (pattern, filter) = split_live_query(&self.query);
            let filter = filter.map(FileFilter::new);
            // The pattern is lit where it is a literal; a `*` makes it a
            // regex, whose matches only the search knows.
            let literal = (!pattern.contains('*')).then(|| pattern.to_lowercase());
            let mut filtered = Vec::new();
            for (index, entry) in self.entries.iter_mut().enumerate() {
                if filter.as_ref().is_some_and(|f| !f.matches(&entry.shortcut)) {
                    continue;
                }
                entry.match_positions = literal
                    .as_deref()
                    .map(|literal| literal_positions(&entry.label, literal))
                    .unwrap_or_default();
                filtered.push(index);
            }
            self.filtered = filtered;
            self.selected = 0;
            return;
        }
        let q = self.query.to_lowercase();
        if q.is_empty() {
            for entry in &mut self.entries {
                entry.match_positions.clear();
            }
            self.filtered = (0..self.entries.len()).collect();
            self.selected = 0;
            return;
        }

        // Compute score + positions for every entry without mutating them yet.
        let scored: Vec<Option<(u8, Vec<usize>)>> = self
            .entries
            .iter()
            .map(|e| score_and_positions(&e.label, &q))
            .collect();

        // Write positions back.
        for (entry, result) in self.entries.iter_mut().zip(scored.iter()) {
            entry.match_positions = result.as_ref().map(|(_, p)| p.clone()).unwrap_or_default();
        }

        let mut filtered: Vec<(usize, u8)> = scored
            .iter()
            .enumerate()
            .filter_map(|(i, r)| r.as_ref().map(|(s, _)| (i, *s)))
            .collect();
        filtered.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        self.filtered = filtered.into_iter().map(|(i, _)| i).collect();
        self.selected = 0;
    }
}

// ========================================================================
// Listings
// ========================================================================

/// One row per entry of `dir`, directories first and each group by name, with
/// the hidden ones after the rest: a listing opens on what is usually wanted,
/// and typing reaches the rest. `..` leads, where there is a parent to reach.
fn file_entries(dirs: &dyn DirSource, dir: &Path) -> Vec<PaletteEntry> {
    let Some(items) = dirs.read_dir(dir) else {
        return Vec::new();
    };
    let mut rows: Vec<(bool, bool, String, PaletteEntry)> = Vec::new();
    for item in items {
        let hidden = item.name.starts_with('.');
        let label = match item.is_dir {
            true => format!("{}/", item.name),
            false => item.name.clone(),
        };
        rows.push((
            !item.is_dir,
            hidden,
            item.name.to_lowercase(),
            PaletteEntry {
                action: item.path.to_string_lossy().to_string(),
                icon: Some(match item.is_dir {
                    true => IconKind::Dir {
                        expanded: false,
                        name: item.name.clone(),
                    },
                    false => IconKind::File {
                        name: item.name.clone(),
                    },
                }),
                label,
                match_positions: Vec::new(),
                // Nothing at the right: the icon says what the entry is.
                shortcut: String::new(),
            },
        ));
    }
    rows.sort_by(|one, other| (one.0, one.1, &one.2).cmp(&(other.0, other.1, &other.2)));
    let mut entries: Vec<PaletteEntry> = Vec::new();
    if let Some(parent) = dirs.parent(dir) {
        entries.push(PaletteEntry {
            action: parent.to_string_lossy().to_string(),
            icon: Some(folder_icon(&parent)),
            label: "../".to_string(),
            match_positions: Vec::new(),
            shortcut: "up".to_string(),
        });
    }
    entries.extend(rows.into_iter().map(|row| row.3));
    entries
}

/// The icon for a folder row: by the folder's own name, so `src` and `.git`
/// get their artwork, and the plain folder for a root, which has no name.
fn folder_icon(dir: &Path) -> IconKind {
    IconKind::Dir {
        expanded: false,
        name: dir
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_default(),
    }
}

// ========================================================================
// Scoring
// ========================================================================

/// Score `label` against `query` (already lowercased) and return the match
/// score plus the char indices in `label` that were matched.
/// Score levels: 3 = exact, 2 = prefix, 1 = substring, 0 = subsequence.
/// A live search's query taken apart: the pattern, and the file filter after
/// its last ` @`, when there is one. `foo @ src/*.rs` is `("foo",
/// Some("src/*.rs"))`.
pub fn split_live_query(query: &str) -> (&str, Option<&str>) {
    match query.rsplit_once(" @") {
        Some((pattern, filter)) => (pattern.trim_end(), Some(filter.trim())),
        None => (query.trim_end(), None),
    }
}

/// Which files a live search's rows are kept for, matched against a row's
/// path without case: a glob where the filter has `*` or `?` in it
/// (`src/*.rs`), else a part of the path (`recent`).
enum FileFilter {
    Glob(regex::Regex),
    Part(String),
}

impl FileFilter {
    fn new(filter: &str) -> Self {
        if !filter.contains(['*', '?']) {
            return Self::Part(filter.to_lowercase());
        }
        let pattern: String = filter
            .chars()
            .map(|c| match c {
                '*' => ".*".to_string(),
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
            Self::Part(part) => path.to_lowercase().contains(part),
        }
    }
}

/// The char indices in `label` that `literal` (lower case) covers, each
/// place it occurs, matched without case.
fn literal_positions(label: &str, literal: &str) -> Vec<usize> {
    if literal.is_empty() {
        return Vec::new();
    }
    let lower: Vec<char> = label.chars().flat_map(char::to_lowercase).collect();
    let needle: Vec<char> = literal.chars().collect();
    // A character whose lower case is longer than one would put the indices
    // out of step with the label's; nothing is lit rather than the wrong cells.
    if lower.len() != label.chars().count() || needle.len() > lower.len() {
        return Vec::new();
    }
    let mut positions = Vec::new();
    let mut at = 0;
    while at + needle.len() <= lower.len() {
        if lower[at..at + needle.len()] == needle[..] {
            positions.extend(at..at + needle.len());
            at += needle.len();
        } else {
            at += 1;
        }
    }
    positions
}

fn score_and_positions(label: &str, query: &str) -> Option<(u8, Vec<usize>)> {
    let label_lower = label.to_lowercase();
    let qlen = query.chars().count();

    if label_lower == query {
        return Some((3, (0..label.chars().count()).collect()));
    }
    if label_lower.starts_with(query) {
        return Some((2, (0..qlen).collect()));
    }
    if let Some(byte_pos) = label_lower.find(query) {
        let char_start = label_lower[..byte_pos].chars().count();
        return Some((1, (char_start..char_start + qlen).collect()));
    }
    // Subsequence: greedily find the earliest matching char position.
    let label_chars: Vec<char> = label_lower.chars().collect();
    let query_chars: Vec<char> = query.chars().collect();
    let mut positions = Vec::with_capacity(query_chars.len());
    let mut qi = 0;
    for (ci, &lc) in label_chars.iter().enumerate() {
        if qi < query_chars.len() && lc == query_chars[qi] {
            positions.push(ci);
            qi += 1;
        }
    }
    if qi == query_chars.len() {
        Some((0, positions))
    } else {
        None
    }
}

// ========================================================================
// History
// ========================================================================

/// One row per command of a shell history file's raw `contents`.
fn history_entries(contents: &[u8]) -> Vec<PaletteEntry> {
    parse_history_lines(&unmetafy(contents))
        .into_iter()
        .map(|cmd| PaletteEntry {
            action: cmd.clone(),
            icon: None,
            label: cmd,
            match_positions: Vec::new(),
            shortcut: String::new(),
        })
        .collect()
}

/// A history file's bytes as text: zsh's metafied bytes put back (see
/// [`ZSH_META`]), and anything still not UTF-8 replaced rather than costing
/// the whole file, as a strict read of it did.
fn unmetafy(contents: &[u8]) -> String {
    let mut bytes = Vec::with_capacity(contents.len());
    let mut meta = false;
    for &byte in contents {
        match (meta, byte) {
            (false, ZSH_META) => meta = true,
            (true, byte) => {
                bytes.push(byte ^ ZSH_META_FLIP);
                meta = false;
            }
            (false, byte) => bytes.push(byte),
        }
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// Parse a history file, returning commands in most-recent-first order,
/// deduplicated. Supports bash (plain lines) and zsh extended format
/// (`: timestamp:elapsed;command`).
fn parse_history_lines(content: &str) -> Vec<String> {
    let joined = join_continued_lines(content);
    let raw: Vec<&str> = joined
        .iter()
        .map(String::as_str)
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            // zsh extended history: ": 1234567890:0;command"
            if let Some(rest) = line.strip_prefix(": ") {
                return rest
                    .split_once(';')
                    .map(|(_, cmd)| cmd.trim())
                    .filter(|s| !s.is_empty());
            }
            Some(line)
        })
        .collect();

    let mut seen = HashSet::new();
    let mut result: Vec<String> = raw
        .iter()
        .rev()
        .filter(|&&cmd| seen.insert(cmd))
        .map(|&s| s.to_string())
        .collect();
    result.truncate(HISTORY_MAX);
    result
}

/// The lines of a history file with each command written over several
/// lines (each but its last ending in `\\`) joined back into one.
fn join_continued_lines(content: &str) -> Vec<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut continuing = false;
    for line in content.lines() {
        match (continuing, lines.last_mut()) {
            (true, Some(last)) => {
                last.push('\n');
                last.push_str(line);
            }
            (true, None) | (false, _) => lines.push(line.to_string()),
        }
        continuing = line.ends_with('\\');
        if continuing {
            if let Some(last) = lines.last_mut() {
                last.pop();
            }
        }
    }
    lines
}

// ========================================================================
// Built-in commands
// ========================================================================

/// Every built-in command as `(action, label, static shortcut hint)`; an
/// empty hint means the chord is looked up from the keymap. The one list the
/// palette, the keys editor, and the keybinding checks all read.
const BUILTIN_COMMANDS: &[(&str, &str, &str)] = &[
    ("cd_recent", "CD: Recent Directory", ""),
    ("close_pane", "Close Pane", ""),
    ("close_other_tabs", "Close Other Tabs", ""),
    ("close_tab", "Close Tab", ""),
    ("copy_cwd", "Copy: Working Directory", ""),
    ("copy_scrollback", "Copy: Full Scrollback", ""),
    ("copy_selection", "Copy Selection", ""),
    ("export_block_svg", "Export: Focused Block SVG to File", ""),
    (
        "export_block_text",
        "Export: Focused Block Text to Clipboard",
        "",
    ),
    (
        "export_scrollback_ansi",
        "Export: Scrollback (ANSI Colors)",
        "",
    ),
    (
        "export_scrollback_editor",
        "Export: Scrollback to Editor",
        "",
    ),
    ("export_scrollback_html", "Export: Scrollback to HTML", ""),
    ("dir_page", "Dir: Open Working Directory", ""),
    (
        "editor_show_scopes",
        "Text Editor: Show Scopes at Cursor",
        "",
    ),
    (
        "next_error",
        "Errors: Open the Next File Reference in the Last Output",
        "",
    ),
    (
        "prev_error",
        "Errors: Open the Previous File Reference in the Last Output",
        "",
    ),
    ("file_browser", "File: Browse Files", ""),
    ("file_browser_root", "File: Browse Files as Root", ""),
    ("file_browser_docker", "File: Browse Docker Containers", ""),
    ("file_browser_ssh", "File: Browse SSH Hosts", ""),
    ("focus_down", "Focus Pane Down", ""),
    ("git_page", "Git: Status", ""),
    ("grep_page", "Grep: Search Files", ""),
    ("focus_left", "Focus Pane Left", ""),
    ("focus_right", "Focus Pane Right", ""),
    ("focus_up", "Focus Pane Up", ""),
    ("font_decrease", "Font Size: Decrease", ""),
    ("font_increase", "Font Size: Increase", ""),
    ("font_reset", "Font Size: Reset", ""),
    ("jump", "Navigate: Jump to Target", ""),
    ("keys_page", "Keys View: Show Every Command", ""),
    ("live_search", "Search: Project as You Type", ""),
    ("tab_keys", "Help: Keys of This Tab", ""),
    ("proc_page", "Processes: Monitor", ""),
    ("project_files", "File: Find in Project", ""),
    ("find_file", "File: Find File", ""),
    ("quota_refresh", "Quota: Refresh", ""),
    ("quota_zai_key", "Quota: Set Z.ai API Key", ""),
    ("sys_page", "System: Monitor", ""),
    ("move_tab_down", "Move Tab to Pane Below", ""),
    ("move_tab_left", "Move Tab to Left Pane", ""),
    ("move_tab_right", "Move Tab to Right Pane", ""),
    ("move_tab_up", "Move Tab to Pane Above", ""),
    ("mux_attach_remote", "Mux: Attach Remote Session...", ""),
    ("mux_detach_session", "Mux: Detach Current Session", ""),
    ("mux_kill_session", "Mux: Kill Background Session...", ""),
    (
        "mux_list_sessions",
        "Mux: List / Attach Background Sessions",
        "",
    ),
    ("mux_new_session", "Mux: New Session...", ""),
    ("new_tab", "New Tab", ""),
    ("new_window", "New Window", ""),
    ("next_block", "Next Block", ""),
    ("next_tab", "Next Tab", ""),
    ("notifications", "Notifications: Show History", ""),
    ("clear_notifications", "Notifications: Clear", ""),
    ("open_under_cursor", "Open: Path or URL at Point", ""),
    ("open_with", "Open: This File With...", ""),
    ("open_settings", "Settings", ""),
    ("paste_from_clipboard", "Paste from Clipboard", ""),
    ("prev_block", "Previous Block", ""),
    ("prev_tab", "Previous Tab", ""),
    ("quick_select", "Quick Select", ""),
    ("recent_tab_back", "Recent Tab (Backward)", ""),
    ("recent_tab_forward", "Recent Tab (Forward)", ""),
    ("reload", "Reload Window", ""),
    ("reopen_page", "Reopen Closed Tab", ""),
    ("reveal_terminal", "Terminal: Reveal Most Recent", ""),
    ("tool_list", "Go To or Reopen Tab...", ""),
    (
        "recent_list",
        "Go To Recent: Tabs, Files, Directories...",
        "",
    ),
    ("search", "Search Blocks", ""),
    ("select_pane", "Go to Pane", ""),
    ("scroll_page_up", "Scroll Page Up", ""),
    ("scroll_page_down", "Scroll Page Down", ""),
    ("scroll_line_up", "Scroll Line Up", ""),
    ("scroll_line_down", "Scroll Line Down", ""),
    ("scroll_to_top", "Scroll to Top", ""),
    ("scroll_to_bottom", "Scroll to Bottom", ""),
    ("split_horizontal", "Split Horizontal", ""),
    ("split_vertical", "Split Vertical", ""),
    ("stop_tasks", "Stop Running Tasks", "C-g"),
    ("swoop", "Navigate: Buffer Swoop", ""),
    ("theme_auto", "Theme: Auto", ""),
    ("theme_dark", "Theme: Dark", ""),
    ("theme_light", "Theme: Light", ""),
    ("theme_new", "Theme: Create New...", ""),
    ("toggle_fold", "Toggle Fold", ""),
    ("toggle_mode", "Toggle Mode (Insert/Normal)", ""),
    ("toggle_pane_zoom", "Zoom Pane", ""),
    (
        "toggle_rainbow_parens",
        "View: Toggle Rainbow Parentheses",
        "",
    ),
    (
        "toggle_sentence_highlight",
        "View: Toggle Sentence Highlight",
        "",
    ),
    ("yank_block", "Yank Block Source", ""),
];

/// Every built-in command, labelled and annotated with the chord `keymap`
/// binds it to.
pub(crate) fn builtin_commands(keymap: &Keymap) -> Vec<PaletteEntry> {
    BUILTIN_COMMANDS
        .iter()
        .map(|(action, label, static_hint)| {
            let shortcut = if !static_hint.is_empty() {
                static_hint.to_string()
            } else {
                keymap.chord_hint(action)
            };
            let entry = PaletteEntry::new(*action, *label, shortcut);
            match command_icon(action) {
                Some(icon) => entry.with_icon(icon),
                None => entry,
            }
        })
        .collect()
}

/// The icon for a command that opens something with an icon of its own: a
/// folder, an editor, a terminal. Every other command is drawn without one, since
/// an icon shared by them all would say nothing a row does not.
fn command_icon(action: &str) -> Option<IconKind> {
    match action {
        "cd_recent"
        | "copy_cwd"
        | "file_browser"
        | "file_browser_root"
        | "file_browser_docker"
        | "file_browser_ssh"
        | "find_file"
        | "project_files" => Some(IconKind::Dir {
            expanded: false,
            name: String::new(),
        }),
        "dir_page" => Some(IconKind::Editor { kind: "dir" }),
        "git_page" => Some(IconKind::Editor { kind: "git" }),
        "grep_page" => Some(IconKind::Editor { kind: "grep" }),
        "new_tab" | "reveal_terminal" | "split_horizontal" | "split_vertical" => {
            Some(IconKind::Terminal)
        }
        _ => None,
    }
}

/// What the palette calls the built-in command `action`, or `None` when
/// there is no such command.
pub(crate) fn command_label(action: &str) -> Option<&'static str> {
    BUILTIN_COMMANDS
        .iter()
        .find(|(name, _, _)| *name == action)
        .map(|(_, label, _)| *label)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use std::env;
    use std::fs;

    use super::*;

    fn found(text: &str, at: &str) -> PaletteEntry {
        PaletteEntry::new(format!("1:{at}"), text, at)
    }

    fn chord(c: char, ctrl: bool, alt: bool) -> Key {
        Key {
            alt,
            code: crate::model::input::KeyCode::Char(c),
            ctrl,
            shift: false,
            sup: false,
        }
    }

    #[test]
    fn test_the_query_takes_emacs_edits_at_its_caret_and_refilters() {
        let mut palette = Palette::open(&Keymap::default());
        for c in "new tab".chars() {
            palette.push_char(c);
        }
        let narrowed = palette.filtered.len();
        assert_eq!(palette.caret(), 7, "typing leaves the caret at the end");

        palette.edit_query(&chord('a', true, false));
        assert_eq!(palette.caret(), 0);
        assert!(!palette.is_caret_at_end());
        palette.edit_query(&chord('d', false, true));
        assert_eq!(palette.query, " tab", "M-d killed the first word");
        assert!(
            palette.filtered.len() >= narrowed,
            "refiltered on the shorter query"
        );

        palette.edit_query(&chord('y', true, false));
        assert_eq!(palette.query, "new tab", "and C-y put it back");
        assert_eq!(palette.caret(), 3, "the caret after what was yanked");
        assert_eq!(
            palette.edit_query(&chord('g', true, false)),
            EditOutcome::Cancel
        );
    }

    #[test]
    fn test_a_query_set_from_elsewhere_puts_the_caret_at_its_end() {
        let mut palette = Palette::open(&Keymap::default());
        palette.edit_query(&chord('x', false, false));
        palette.query = "recalled".to_string();
        assert_eq!(palette.caret(), 8);
        palette.edit_query(&chord('b', true, false));
        assert_eq!(palette.caret(), 7, "the field picked the new query up");
        assert_eq!(palette.query, "recalled");
    }

    #[test]
    fn test_a_live_query_splits_its_file_filter_off_at_the_last_at_sign() {
        assert_eq!(
            split_live_query("foo @ src/*.rs"),
            ("foo", Some("src/*.rs"))
        );
        assert_eq!(split_live_query("a@b @ recent"), ("a@b", Some("recent")));
        assert_eq!(split_live_query("email@host"), ("email@host", None));
    }

    #[test]
    fn test_a_live_search_keeps_the_rows_its_filter_names_and_lights_the_pattern() {
        let mut palette = Palette::open_live_search("Search".into());
        palette.append_entries(vec![
            found("let Icon = 1;", "src/icons/mod.rs:3"),
            found("icon here", "docs/guide.md:9"),
        ]);
        for c in "icon @ src/*.RS".chars() {
            palette.push_char(c);
        }
        assert_eq!(palette.filtered, [0], "a glob, matched without case");
        assert_eq!(palette.entries[0].match_positions, [4, 5, 6, 7]);

        palette.query = "icon @ guide".into();
        palette.update_filter();
        assert_eq!(palette.filtered, [1], "a part of the path");

        palette.query = "ic*n".into();
        palette.update_filter();
        assert_eq!(palette.filtered, [0, 1], "no filter keeps every row");
        assert!(
            palette.entries[0].match_positions.is_empty(),
            "a regex's matches are the search's to know"
        );
    }

    #[test]
    fn test_rows_streaming_in_leave_the_highlighted_row_where_it_is() {
        let mut palette = Palette::open_live_search("Search".into());
        palette.append_entries(vec![found("a", "a:1"), found("b", "b:1")]);
        palette.move_down();
        palette.append_entries(vec![found("c", "c:1")]);
        assert_eq!(palette.selected, 1);
    }

    /// Where a [`FixedTree`] is rooted. Never read from disk.
    const FIXED_ROOT: &str = "/winter-fixed-tree";

    /// A directory tree that exists only in the test: `names` under
    /// [`FIXED_ROOT`], a trailing `/` marking a directory, every file two
    /// bytes long.
    #[derive(Debug)]
    struct FixedTree {
        dirs: Vec<PathBuf>,
        files: Vec<PathBuf>,
    }

    impl FixedTree {
        /// The tree's root, and the tree as a palette reads it.
        fn with_entries(names: &[&str]) -> (PathBuf, Arc<dyn DirSource>) {
            let root = PathBuf::from(FIXED_ROOT);
            let mut dirs: Vec<PathBuf> = root.ancestors().map(Path::to_path_buf).collect();
            let mut files = Vec::new();
            for name in names {
                match name.strip_suffix('/') {
                    Some(dir) => dirs.push(root.join(dir)),
                    None => files.push(root.join(name)),
                }
            }
            (root, Arc::new(Self { dirs, files }))
        }
    }

    impl DirSource for FixedTree {
        fn read_dir(&self, dir: &Path) -> Option<Vec<DirItem>> {
            if !self.is_dir(dir) {
                return None;
            }
            let item = |path: &PathBuf, is_dir: bool| {
                Some(DirItem {
                    is_dir,
                    name: path.file_name()?.to_string_lossy().to_string(),
                    path: path.clone(),
                })
            };
            let dirs = self.dirs.iter().map(|path| (path, true));
            let files = self.files.iter().map(|path| (path, false));
            Some(
                dirs.chain(files)
                    .filter(|(path, _)| path.parent() == Some(dir))
                    .filter_map(|(path, is_dir)| item(path, is_dir))
                    .collect(),
            )
        }

        fn is_dir(&self, path: &Path) -> bool {
            self.dirs.iter().any(|dir| dir == path)
        }
    }

    /// A directory on disk holding `names`, removed again when the test
    /// ends: for the one test that reads the real filesystem.
    struct TempTree(PathBuf);

    impl TempTree {
        fn new(tag: &str, names: &[&str]) -> Self {
            let dir = env::temp_dir().join(format!("winter-browser-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&dir);
            fs::create_dir_all(&dir).expect("temp dir");
            for name in names {
                match name.ends_with('/') {
                    true => fs::create_dir_all(dir.join(name.trim_end_matches('/'))),
                    false => fs::write(dir.join(name), "xy"),
                }
                .expect("temp entry");
            }
            Self(dir)
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn test_a_browsed_directory_leads_with_the_way_up_then_its_directories() {
        // A listing that opened on whatever order the filesystem handed back
        // would put `.git` above `src` as often as not.
        let (root, dirs) = FixedTree::with_entries(&[".hidden", "beta.rs", "alpha/", ".git/"]);
        let palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));

        let labels: Vec<&str> = palette
            .entries
            .iter()
            .map(|entry| entry.label.as_str())
            .collect();
        assert_eq!(labels, ["../", "alpha/", ".git/", "beta.rs", ".hidden"]);
        assert_eq!(palette.mode, PaletteMode::Files);
        assert_eq!(palette.dir.as_ref(), Some(&root), "and it says where it is");
    }

    #[test]
    fn test_a_browsed_row_carries_its_path_and_what_it_is() {
        let tree = TempTree::new("rows", &["src/", "notes.txt"]);
        let palette = Palette::open_files(tree.0.clone());

        let file = palette
            .entries
            .iter()
            .find(|entry| entry.label == "notes.txt")
            .expect("the file is listed");
        assert_eq!(file.action, tree.0.join("notes.txt").to_string_lossy());
        assert_eq!(file.shortcut, "", "the icon says what it is");

        let dir = palette
            .entries
            .iter()
            .find(|entry| entry.label == "src/")
            .expect("the directory is listed");
        assert_eq!(dir.shortcut, "");
    }

    #[test]
    fn test_a_browsed_row_is_drawn_as_a_folder_or_a_file_by_what_it_is() {
        // Keyed on the entry's kind rather than its label: `alpha/` and
        // `beta.rs` would both resolve somewhere through the file table, and a
        // folder drawn with a file's icon is the bug this pins.
        let (root, dirs) = FixedTree::with_entries(&["beta.rs", "alpha/"]);
        let palette = Palette::open_files_in(root, dirs);

        let icons: Vec<(&str, Option<&IconKind>)> = palette
            .entries
            .iter()
            .filter(|entry| entry.label != "../")
            .map(|entry| (entry.label.as_str(), entry.icon.as_ref()))
            .collect();
        assert_eq!(
            icons,
            [
                (
                    "alpha/",
                    Some(&IconKind::Dir {
                        expanded: false,
                        name: "alpha".to_string(),
                    })
                ),
                (
                    "beta.rs",
                    Some(&IconKind::File {
                        name: "beta.rs".to_string(),
                    })
                ),
            ]
        );
    }

    #[test]
    fn test_typing_filters_the_directory_being_browsed() {
        let (root, dirs) = FixedTree::with_entries(&["alpha.rs", "beta.rs"]);
        let mut palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));
        palette.push_char('b');
        let shown: Vec<&str> = palette
            .filtered
            .iter()
            .map(|&i| palette.entries[i].label.as_str())
            .collect();
        assert_eq!(shown, ["beta.rs"]);
    }

    #[test]
    fn test_palette_open_has_entries() {
        let p = Palette::open(&Keymap::default());
        assert!(p.active);
        assert!(!p.entries.is_empty());
        assert_eq!(p.filtered.len(), p.entries.len());
        assert_eq!(p.selected, 0);
        assert_eq!(p.mode, PaletteMode::Commands);
    }

    #[test]
    fn test_history_open_sets_history_mode() {
        let p = Palette::open_history();
        assert!(p.active);
        assert_eq!(p.mode, PaletteMode::History);
    }

    #[test]
    fn test_palette_filter_narrows() {
        let mut p = Palette::open(&Keymap::default());
        let total = p.entries.len();
        for c in "split".chars() {
            p.push_char(c);
        }
        assert!(p.filtered.len() < total);
        assert!(p.filtered.len() >= 2);
    }

    #[test]
    fn test_palette_selection_navigates() {
        let mut p = Palette::open(&Keymap::default());
        assert_eq!(p.selected, 0);
        p.move_down();
        assert_eq!(p.selected, 1);
        p.move_up();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn test_palette_close_resets() {
        let mut p = Palette::open(&Keymap::default());
        p.push_char('s');
        p.move_down();
        p.close();
        assert!(!p.active);
        assert!(p.query.is_empty());
    }

    #[test]
    fn test_palette_selected_action() {
        let p = Palette::open(&Keymap::default());
        let action = p.selected_action().unwrap();
        assert_eq!(action, "cd_recent");
    }

    #[test]
    fn test_position_by_shortcut_finds_matching_pane_entry() {
        let panes = vec![
            PaletteEntry::new("1", "1: main", "1"),
            PaletteEntry::new("2", "1: main", "2"),
        ];
        let p = Palette::open_panes(panes);
        assert_eq!(p.position_by_shortcut("2"), Some(1));
        assert_eq!(p.position_by_shortcut("9"), None);
    }

    #[test]
    fn test_position_by_shortcut_respects_active_filter() {
        let panes = vec![
            PaletteEntry::new("1", "1: main", "1"),
            PaletteEntry::new("2", "2: build", "1"),
        ];
        let mut p = Palette::open_panes(panes);
        for c in "build".chars() {
            p.push_char(c);
        }
        // Only the "2: build" entry survives the filter, so its shortcut "1"
        // resolves to that entry, not the "1: main" one hidden by the query.
        let pos = p.position_by_shortcut("1").unwrap();
        assert_eq!(p.entries[p.filtered[pos]].label, "2: build");
    }

    #[test]
    fn test_palette_backspace() {
        let mut p = Palette::open(&Keymap::default());
        p.push_char('s');
        p.push_char('p');
        p.pop_char();
        assert_eq!(p.query, "s");
    }

    #[test]
    fn test_palette_undo_restores_previous_query() {
        let mut p = Palette::open(&Keymap::default());
        p.push_char('s');
        p.push_char('p');
        p.undo();
        assert_eq!(p.query, "s");
        p.undo();
        assert_eq!(p.query, "");
        p.undo();
        assert_eq!(p.query, "", "undo past the start is a no-op");
    }

    #[test]
    fn test_palette_redo_reapplies_query() {
        let mut p = Palette::open(&Keymap::default());
        p.push_char('s');
        p.push_char('p');
        p.undo();
        p.undo();
        p.redo();
        assert_eq!(p.query, "s");
        p.redo();
        assert_eq!(p.query, "sp");
    }

    #[test]
    fn test_palette_undo_refilters() {
        let mut p = Palette::open(&Keymap::default());
        let total = p.entries.len();
        for c in "split".chars() {
            p.push_char(c);
        }
        assert!(p.filtered.len() < total);
        for _ in 0.."split".len() {
            p.undo();
        }
        assert_eq!(p.query, "");
        assert_eq!(
            p.filtered.len(),
            total,
            "empty query shows all entries again"
        );
    }

    #[test]
    fn test_score_and_positions_exact_highlights_all() {
        let (score, positions) = score_and_positions("Split Vertical", "split vertical").unwrap();
        assert_eq!(score, 3);
        assert_eq!(positions.len(), "split vertical".len());
    }

    #[test]
    fn test_score_and_positions_substring_returns_contiguous_range() {
        let (score, positions) = score_and_positions("Split Vertical", "vert").unwrap();
        assert_eq!(score, 1);
        assert!(
            positions.windows(2).all(|w| w[1] == w[0] + 1),
            "positions must be contiguous"
        );
    }

    #[test]
    fn test_score_and_positions_subsequence_returns_positions() {
        let (score, positions) = score_and_positions("Split Vertical", "sv").unwrap();
        assert_eq!(score, 0);
        assert_eq!(positions.len(), 2);
    }

    #[test]
    fn test_filter_sets_match_positions() {
        let mut p = Palette::open(&Keymap::default());
        for c in "new tab".chars() {
            p.push_char(c);
        }
        assert!(!p.filtered.is_empty());
        let top = p.filtered[0];
        assert!(
            !p.entries[top].match_positions.is_empty(),
            "matched entry must have positions"
        );
    }

    #[test]
    fn test_parse_history_lines_deduplicates_most_recent_first() {
        let input = "ls\npwd\nls\ngit status\n";
        let result = parse_history_lines(input);
        assert_eq!(result[0], "git status");
        assert_eq!(result[1], "ls");
        assert_eq!(result[2], "pwd");
        assert_eq!(result.len(), 3, "duplicate 'ls' must be removed");
    }

    #[test]
    fn test_parse_history_lines_strips_zsh_timestamps() {
        let input = ": 1700000000:0;echo hello\n: 1700000001:0;ls\n";
        let result = parse_history_lines(input);
        assert_eq!(result[0], "ls");
        assert_eq!(result[1], "echo hello");
    }

    #[test]
    fn test_palette_query_history_navigation_round_trip() {
        let history = vec!["quit".to_string(), "theme dark".to_string()];
        let mut p = Palette::open(&Keymap::default()).with_query_history(history);

        for c in "foo".chars() {
            p.push_char(c);
        }
        assert_eq!(p.query, "foo");

        p.history_prev();
        assert_eq!(p.query, "quit");

        p.history_prev();
        assert_eq!(p.query, "theme dark");

        p.history_prev();
        assert_eq!(p.query, "theme dark", "stepping past oldest is a no-op");

        p.history_next();
        assert_eq!(p.query, "quit");

        p.history_next();
        assert_eq!(p.query, "foo", "stepping past newest restores live query");
    }

    #[test]
    fn test_palette_query_history_empty_is_noop() {
        let mut p = Palette::open(&Keymap::default());
        p.push_char('a');
        p.history_prev();
        assert_eq!(p.query, "a");
        p.history_next();
        assert_eq!(p.query, "a");
    }

    #[test]
    fn test_palette_query_history_typing_resets_history_index() {
        let history = vec!["split".to_string()];
        let mut p = Palette::open(&Keymap::default()).with_query_history(history);
        p.history_prev();
        assert_eq!(p.query, "split");
        assert_eq!(p.history_index, Some(0));

        p.push_char('s');
        assert_eq!(p.query, "splits");
        assert_eq!(p.history_index, None);
    }

    #[test]
    fn test_palette_open_swoop_creates_formatted_entries() {
        let lines = vec![
            (0, "first line".to_string()),
            (5, "second line".to_string()),
        ];
        let p = Palette::open_swoop(lines);
        assert_eq!(p.mode, PaletteMode::Swoop);
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[0].action, "0");
        assert_eq!(p.entries[0].label, "    1  first line");
        assert_eq!(p.entries[1].action, "5");
        assert_eq!(p.entries[1].label, "    6  second line");
    }

    #[test]
    fn test_palette_open_mux_sessions() {
        let sessions = vec!["default (80x24)".to_string(), "build (120x40)".to_string()];
        let p = Palette::open_mux_sessions(sessions);
        assert_eq!(p.mode, PaletteMode::MuxSessions);
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[0].label, "default (80x24)");
        assert_eq!(p.entries[1].label, "build (120x40)");
    }

    #[test]
    fn test_palette_open_mux_kill() {
        let sessions = vec!["default (80x24)".to_string(), "build (120x40)".to_string()];
        let p = Palette::open_mux_kill(sessions);
        assert_eq!(p.mode, PaletteMode::MuxKill);
        assert_eq!(p.entries.len(), 2);
        assert_eq!(p.entries[0].label, "default (80x24)");
        assert_eq!(p.entries[1].label, "build (120x40)");
    }

    #[test]
    fn test_palette_mux_new_keeps_its_prompt_visible_while_typing() {
        // The MuxNew query is a session spec, not a filter: typing something
        // that fuzzy-matches nothing must not hide the prompt line, or Enter
        // would look dead to the user.
        let mut p = Palette::open_mux_new();
        assert_eq!(p.mode, PaletteMode::MuxNew);
        assert_eq!(p.entries.len(), 1);
        for c in "dev cargo watch -x test".chars() {
            p.push_char(c);
        }
        assert_eq!(p.filtered.len(), 1, "the prompt must never be filtered out");
        assert_eq!(p.query, "dev cargo watch -x test");
    }

    #[test]
    fn test_palette_mux_attach_remote_keeps_its_prompt_visible_while_typing() {
        let mut p = Palette::open_mux_attach_remote();
        assert_eq!(p.mode, PaletteMode::MuxAttachRemote);
        assert_eq!(p.entries.len(), 1);
        for c in "box.example.com work".chars() {
            p.push_char(c);
        }
        assert_eq!(p.filtered.len(), 1, "the prompt must never be filtered out");
        assert_eq!(p.query, "box.example.com work");
    }

    #[test]
    fn test_every_command_icon_names_an_editor_with_artwork() {
        // The command table names editors by string, apart from the kinds the
        // icons are keyed by: a misspelt one would draw the generic file icon
        // beside the command rather than fail anywhere.
        let commands = builtin_commands(&Keymap::default());
        let editors: Vec<&str> = commands
            .iter()
            .filter_map(|entry| match entry.icon.as_ref()? {
                IconKind::Editor { kind } => Some(*kind),
                IconKind::Dir { .. }
                | IconKind::File { .. }
                | IconKind::Git { .. }
                | IconKind::Gutter { .. }
                | IconKind::Terminal => None,
            })
            .collect();
        assert!(!editors.is_empty());
        for kind in editors {
            assert_ne!(
                crate::icons::name_for(&IconKind::Editor { kind }),
                crate::icons::name_for(&IconKind::Editor { kind: "" }),
                "{kind} draws the generic file icon"
            );
        }
    }

    #[test]
    fn test_palette_open_jump_and_preselect_nearest() {
        let targets = vec![
            JumpTarget::new(10, "   11  first line".to_string()),
            JumpTarget::new(25, "   26  second line".to_string()),
            JumpTarget::new(50, "   51  third line".to_string()),
        ];
        let mut p = Palette::open_jump(targets, Some("Jump: test.rs".to_string()));
        assert_eq!(p.mode, PaletteMode::Jump);
        assert_eq!(p.title.as_deref(), Some("Jump: test.rs"));
        assert_eq!(p.entries.len(), 3);
        assert_eq!(p.selected, 0);

        // Preselect nearest to line 20 -> picks line 25 (the first >= 20)
        p.preselect_nearest(20);
        assert_eq!(p.selected, 1);
        assert_eq!(p.selected_action(), Some("25"));

        // Preselect nearest to line 100 -> beyond all targets, picks closest (index 2: 50)
        p.preselect_nearest(100);
        assert_eq!(p.selected, 2);
        assert_eq!(p.selected_action(), Some("50"));

        // Fuzzy filter by typing
        for c in "third".chars() {
            p.push_char(c);
        }
        assert_eq!(p.filtered.len(), 1);
        assert_eq!(p.selected_action(), Some("50"));
    }

    #[test]
    fn test_a_separator_typed_in_the_browser_reroots_at_the_named_directory() {
        // `src` + `/` names a directory, so the listing moves into it with a
        // fresh query instead of filtering for a literal `src/`.
        let (root, dirs) = FixedTree::with_entries(&["src/", "src/main.rs", "readme.md"]);
        let mut palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));
        for ch in "src/".chars() {
            palette.push_char(ch);
        }
        assert_eq!(palette.dir.as_deref(), Some(root.join("src").as_path()));
        assert_eq!(palette.query, "");

        // `..` + `/` goes back up, resolved rather than kept as `src/..`.
        for ch in "../".chars() {
            palette.push_char(ch);
        }
        assert_eq!(palette.dir.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn test_a_separator_after_a_fuzzy_query_enters_the_selected_directory() {
        // `sr` names nothing on disk, but it selects `src/`: the separator
        // completes into it, as the selection says.
        let (root, dirs) = FixedTree::with_entries(&["src/", "readme.md"]);
        let mut palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));
        palette.push_char('s');
        palette.push_char('r');
        palette.push_char('/');
        assert_eq!(palette.dir.as_deref(), Some(root.join("src").as_path()));
    }

    #[test]
    fn test_a_separator_naming_no_directory_filters_as_typed() {
        // `readme.md` is a file and `zz` matches nothing: neither is
        // somewhere to go, so the separator stays in the query.
        let (root, dirs) = FixedTree::with_entries(&["readme.md"]);
        let mut palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));
        for ch in "zz/".chars() {
            palette.push_char(ch);
        }
        assert_eq!(palette.dir.as_deref(), Some(root.as_path()));
        assert_eq!(palette.query, "zz/");

        // An empty query plus a separator is the filesystem root.
        let mut palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));
        palette.push_char('/');
        assert_eq!(palette.dir.as_deref(), root.ancestors().last());
    }

    #[test]
    fn test_select_path_lands_on_the_named_file() {
        // Find File opens on the file the terminal named, not on `../`.
        let (root, dirs) = FixedTree::with_entries(&["alpha.rs", "beta.rs"]);
        let mut palette = Palette::open_files_in(root.clone(), Arc::clone(&dirs));
        palette.select_path(&root.join("beta.rs"));
        assert_eq!(palette.selected_action(), root.join("beta.rs").to_str());
    }

    #[test]
    fn test_a_zsh_history_with_metafied_bytes_is_read_not_skipped() {
        // Regression: zsh writes a non-ASCII byte as 0x83 and the byte with
        // 0x20 flipped, which is not UTF-8, so the strict read failed and the
        // whole history was skipped.
        let mut contents = b": 1700000000:0;echo caf".to_vec();
        for byte in "\u{e9}".bytes() {
            match byte >= ZSH_META {
                true => contents.extend_from_slice(&[ZSH_META, byte ^ ZSH_META_FLIP]),
                false => contents.push(byte),
            }
        }
        contents.extend_from_slice(b"\n: 1700000001:0;ls\n");
        let palette = Palette::open_history_of(&contents);
        let labels: Vec<&str> = palette.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["ls", "echo caf\u{e9}"]);
    }

    #[test]
    fn test_a_command_written_over_several_lines_is_one_entry() {
        let contents = b": 1:0;for x in 1 2; do\\\necho $x\\\ndone\n: 2:0;pwd\n";
        let palette = Palette::open_history_of(contents);
        let labels: Vec<&str> = palette.entries.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(labels, ["pwd", "for x in 1 2; do\necho $x\ndone"]);
    }
}

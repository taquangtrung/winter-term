//! Native command palette: a lightweight text-mode overlay activated by
//! `Ctrl-Shift-P` (or the configured key). Renders as GPU quads at the top
//! of the focused pane, no WebView required.

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::PathBuf;

use super::history::EditHistory;
use super::input::WindowKeymap;

// ========================================================================
// Constants
// ========================================================================

const HISTORY_MAX: usize = 500;

// ========================================================================
// Data Structures
// ========================================================================

/// Whether the palette is showing built-in commands, shell history, or recent dirs.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum PaletteMode {
    #[default]
    Commands,
    History,
    /// Pane switcher: selecting an entry focuses that pane.
    Panes,
    /// `cd` target picker: selecting an entry executes `cd <dir>` immediately.
    RecentDirs,
    /// Buffer swoop: fuzzy line search over the active pane's scrollback and grid.
    Swoop,
    /// Mux session switcher: selecting an entry attaches or switches to that daemon session.
    MuxSessions,
    /// Mux session killer: selecting an entry terminates that daemon session.
    MuxKill,
    /// Mux session creator: a free-text prompt — the *query* is the payload
    /// ("name [command]"), never a filter, and `Enter` spawns the session.
    MuxNew,
    /// Remote mux attach: a free-text prompt — the *query* is the payload
    /// ("host [session]"), never a filter, and `Enter` attaches over ssh.
    MuxAttachRemote,
}

#[derive(Clone, Debug)]
pub struct PaletteEntry {
    pub action: String,
    pub label: String,
    /// Char indices in `label` that matched the current query (for highlight).
    pub match_positions: Vec<usize>,
    /// Keyboard shortcut hint shown on the right (e.g. `"Ctrl-Shift-T"`).
    pub shortcut: String,
}

#[derive(Clone, Debug, Default)]
pub struct Palette {
    pub active: bool,
    pub entries: Vec<PaletteEntry>,
    pub filtered: Vec<usize>,
    /// Undo/redo stack over `query`, driven by `Ctrl-/` and `Ctrl-\`.
    pub history: EditHistory<String>,
    /// Position in `query_history` currently recalled (`0` = most recent).
    /// `None` indicates the live, uncommitted query is displayed.
    pub history_index: Option<usize>,
    /// The live query snapshot saved when history navigation begins.
    pub live_query: String,
    pub mode: PaletteMode,
    pub query: String,
    /// Executed query history (most-recent-first).
    pub query_history: Vec<String>,
    pub selected: usize,
}

// ========================================================================
// Implementation
// ========================================================================

impl Palette {
    pub fn open(keymap: &WindowKeymap) -> Self {
        let entries = builtin_commands(keymap);
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Commands,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    pub fn open_history() -> Self {
        let entries = load_history_entries();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::History,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Open the palette in recent-dirs mode. `dirs` is a deduplicated list of
    /// working directories ordered most-recently-used first.
    pub fn open_recent_dirs(dirs: Vec<String>) -> Self {
        let entries = dirs
            .into_iter()
            .map(|dir| PaletteEntry {
                action: dir.clone(),
                label: dir,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::RecentDirs,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Open the palette in pane switcher mode.
    pub fn open_panes(panes: Vec<(super::layout::PaneId, String, String)>) -> Self {
        let entries = panes
            .into_iter()
            .map(|(pane_id, label, shortcut)| PaletteEntry {
                action: pane_id.0.to_string(),
                label,
                match_positions: Vec::new(),
                shortcut,
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Panes,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
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
                    label: format!("{line_num:>5}  {line}"),
                    match_positions: Vec::new(),
                    shortcut: String::new(),
                }
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::Swoop,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Open the palette in mux session switcher mode.
    pub fn open_mux_sessions(sessions: Vec<String>) -> Self {
        let entries = sessions
            .into_iter()
            .map(|name| PaletteEntry {
                action: name.clone(),
                label: name,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxSessions,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Open the palette in mux session killer mode.
    pub fn open_mux_kill(sessions: Vec<String>) -> Self {
        let entries = sessions
            .into_iter()
            .map(|name| PaletteEntry {
                action: name.clone(),
                label: name,
                match_positions: Vec::new(),
                shortcut: String::new(),
            })
            .collect::<Vec<_>>();
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxKill,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Open the palette in mux session creator mode: the query is the input
    /// ("name [command]"), shown against a single static prompt line that
    /// stays visible no matter what is typed.
    pub fn open_mux_new() -> Self {
        let entries = vec![PaletteEntry {
            action: "spawn".to_string(),
            label: "new session: name [command]".to_string(),
            match_positions: Vec::new(),
            shortcut: String::new(),
        }];
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxNew,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Open the palette in remote mux attach mode: the query is the input
    /// ("host [session]"), shown against a single static prompt line that
    /// stays visible no matter what is typed.
    pub fn open_mux_attach_remote() -> Self {
        let entries = vec![PaletteEntry {
            action: "attach".to_string(),
            label: "attach remote: host [session]".to_string(),
            match_positions: Vec::new(),
            shortcut: String::new(),
        }];
        let filtered = (0..entries.len()).collect();
        Palette {
            active: true,
            entries,
            filtered,
            history: EditHistory::new(String::new()),
            history_index: None,
            live_query: String::new(),
            mode: PaletteMode::MuxAttachRemote,
            query: String::new(),
            query_history: Vec::new(),
            selected: 0,
        }
    }

    /// Attach executed query history to this palette instance.
    pub fn with_query_history(mut self, history: Vec<String>) -> Self {
        self.query_history = history;
        self
    }

    pub fn close(&mut self) {
        self.active = false;
        self.query.clear();
        self.history.reset(String::new());
        self.history_index = None;
        self.live_query.clear();
        self.selected = 0;
    }

    pub fn push_char(&mut self, c: char) {
        self.history_index = None;
        self.query.push(c);
        self.history.record(self.query.clone());
        self.update_filter();
    }

    pub fn pop_char(&mut self) {
        self.history_index = None;
        self.query.pop();
        self.history.record(self.query.clone());
        self.update_filter();
    }

    /// Recall an older executed query (`Alt+P`). Replaces the current query
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

    /// Recall a newer executed query (`Alt+N`), returning to the live typed
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

    /// Restore the previous query state (`Ctrl-/`). No-op when nothing to undo.
    pub fn undo(&mut self) {
        self.history_index = None;
        if let Some(query) = self.history.undo() {
            self.query = query.clone();
            self.update_filter();
        }
    }

    /// Re-apply an undone query state (`Ctrl-\`). No-op when nothing to redo.
    pub fn redo(&mut self) {
        self.history_index = None;
        if let Some(query) = self.history.redo() {
            self.query = query.clone();
            self.update_filter();
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.selected + 1 < self.filtered.len() {
            self.selected += 1;
        }
    }

    pub fn selected_action(&self) -> Option<&str> {
        self.filtered
            .get(self.selected)
            .map(|&i| self.entries[i].action.as_str())
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
// Scoring
// ========================================================================

/// Score `label` against `query` (already lowercased) and return the match
/// score plus the char indices in `label` that were matched.
/// Score levels: 3 = exact, 2 = prefix, 1 = substring, 0 = subsequence.
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

fn load_history_entries() -> Vec<PaletteEntry> {
    let content = try_read_history();
    parse_history_lines(&content)
        .into_iter()
        .map(|cmd| PaletteEntry {
            action: cmd.clone(),
            label: cmd,
            match_positions: Vec::new(),
            shortcut: String::new(),
        })
        .collect()
}

fn try_read_history() -> String {
    let candidates: Vec<PathBuf> = [
        env::var("HISTFILE").ok().map(PathBuf::from),
        home_dir().map(|h| h.join(".zsh_history")),
        home_dir().map(|h| h.join(".bash_history")),
    ]
    .into_iter()
    .flatten()
    .collect();

    for path in candidates {
        if let Ok(content) = fs::read_to_string(&path) {
            if !content.is_empty() {
                return content;
            }
        }
    }
    String::new()
}

/// Parse a history file, returning commands in most-recent-first order,
/// deduplicated. Supports bash (plain lines) and zsh extended format
/// (`: timestamp:elapsed;command`).
fn parse_history_lines(content: &str) -> Vec<String> {
    let raw: Vec<&str> = content
        .lines()
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

fn home_dir() -> Option<PathBuf> {
    env::var_os("HOME").map(PathBuf::from)
}

// ========================================================================
// Built-in commands
// ========================================================================

fn builtin_commands(keymap: &WindowKeymap) -> Vec<PaletteEntry> {
    // (action, label, static shortcut hint — empty means look up from keymap)
    let entries: &[(&str, &str, &str)] = &[
        ("cd_recent", "CD: Recent Directory", ""),
        ("close_pane", "Close Pane", ""),
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
        ("focus_down", "Focus Pane Down", ""),
        ("focus_left", "Focus Pane Left", ""),
        ("focus_right", "Focus Pane Right", ""),
        ("focus_up", "Focus Pane Up", ""),
        ("font_decrease", "Font Size: Decrease", ""),
        ("font_increase", "Font Size: Increase", ""),
        ("font_reset", "Font Size: Reset", ""),
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
        ("next_block", "Next Block", ""),
        ("next_tab", "Next Tab", ""),
        ("open_settings", "Settings", ""),
        ("paste_from_clipboard", "Paste from Clipboard", ""),
        ("prev_block", "Previous Block", ""),
        ("prev_tab", "Previous Tab", ""),
        ("quick_select", "Quick Select", ""),
        ("recent_tab_back", "Recent Tab (Backward)", ""),
        ("recent_tab_forward", "Recent Tab (Forward)", ""),
        ("reload", "Reload Winter", ""),
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
    entries
        .iter()
        .map(|(action, label, static_hint)| {
            let shortcut = if !static_hint.is_empty() {
                static_hint.to_string()
            } else {
                keymap.chord_hint(action)
            };
            PaletteEntry {
                action: action.to_string(),
                label: label.to_string(),
                match_positions: Vec::new(),
                shortcut,
            }
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
    fn test_palette_open_has_entries() {
        let p = Palette::open(&WindowKeymap::default());
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
        let mut p = Palette::open(&WindowKeymap::default());
        let total = p.entries.len();
        for c in "split".chars() {
            p.push_char(c);
        }
        assert!(p.filtered.len() < total);
        assert!(p.filtered.len() >= 2);
    }

    #[test]
    fn test_palette_selection_navigates() {
        let mut p = Palette::open(&WindowKeymap::default());
        assert_eq!(p.selected, 0);
        p.move_down();
        assert_eq!(p.selected, 1);
        p.move_up();
        assert_eq!(p.selected, 0);
    }

    #[test]
    fn test_palette_close_resets() {
        let mut p = Palette::open(&WindowKeymap::default());
        p.push_char('s');
        p.move_down();
        p.close();
        assert!(!p.active);
        assert!(p.query.is_empty());
    }

    #[test]
    fn test_palette_selected_action() {
        let p = Palette::open(&WindowKeymap::default());
        let action = p.selected_action().unwrap();
        assert_eq!(action, "cd_recent");
    }

    #[test]
    fn test_position_by_shortcut_finds_matching_pane_entry() {
        let panes = vec![
            (
                crate::model::layout::PaneId(1),
                "1: main".to_string(),
                "1".to_string(),
            ),
            (
                crate::model::layout::PaneId(2),
                "1: main".to_string(),
                "2".to_string(),
            ),
        ];
        let p = Palette::open_panes(panes);
        assert_eq!(p.position_by_shortcut("2"), Some(1));
        assert_eq!(p.position_by_shortcut("9"), None);
    }

    #[test]
    fn test_position_by_shortcut_respects_active_filter() {
        let panes = vec![
            (
                crate::model::layout::PaneId(1),
                "1: main".to_string(),
                "1".to_string(),
            ),
            (
                crate::model::layout::PaneId(2),
                "2: build".to_string(),
                "1".to_string(),
            ),
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
        let mut p = Palette::open(&WindowKeymap::default());
        p.push_char('s');
        p.push_char('p');
        p.pop_char();
        assert_eq!(p.query, "s");
    }

    #[test]
    fn test_palette_undo_restores_previous_query() {
        let mut p = Palette::open(&WindowKeymap::default());
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
        let mut p = Palette::open(&WindowKeymap::default());
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
        let mut p = Palette::open(&WindowKeymap::default());
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
        let mut p = Palette::open(&WindowKeymap::default());
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
        let mut p = Palette::open(&WindowKeymap::default()).with_query_history(history);

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
        let mut p = Palette::open(&WindowKeymap::default());
        p.push_char('a');
        p.history_prev();
        assert_eq!(p.query, "a");
        p.history_next();
        assert_eq!(p.query, "a");
    }

    #[test]
    fn test_palette_query_history_typing_resets_history_index() {
        let history = vec!["split".to_string()];
        let mut p = Palette::open(&WindowKeymap::default()).with_query_history(history);
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
}

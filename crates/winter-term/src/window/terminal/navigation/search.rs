//! In-pane text search: scrolls the viewport to the next/previous match.

use std::time::Instant;

use crate::model::editor::match_spans;
use crate::model::input;
use crate::model::layout::TabId;
use crate::model::textedit::field::TextField;

use super::vim::{char_class, line_chars};
use super::App;

// ========================================================================
// Data Structures
// ========================================================================

/// A match's position in the pane's whole buffer: `(absolute_row, column)`,
/// with the row counted from the oldest scrollback line (see
/// [`winter_core::Grid::to_absolute_row`]) so it stays valid as the viewport
/// scrolls.
pub(in crate::window) type MatchPos = (usize, usize);

/// Where a search step begins scanning.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::window) enum SearchFrom {
    /// The position the search was launched from ([`SearchState::origin`]).
    /// Used while the query is still being typed so every keystroke re-searches
    /// from the same spot (Vim's `incsearch`) instead of creeping forward
    /// through the buffer one letter at a time.
    Origin,
    /// The Normal-mode cursor, which each jump parks on the focused match.
    /// Used by `n`/`N`, so repeats step match-by-match, and so a repeat after
    /// moving the cursor by hand resumes from the cursor, like Vim.
    Cursor,
}

/// The live `/` search: the query being typed, where it started, and which
/// match it is parked on. Putting the search away clears every field except
/// [`Self::last`] and [`Self::reverse`], which is what lets `n`/`N` resume the
/// same search, the same way round, from wherever the cursor now is.
#[derive(Debug, Default)]
pub(in crate::window) struct SearchState {
    /// The match `n`/`N` is parked on: the pane and the match's absolute
    /// `(row, col)` start. Drawn in [`winter_render::Theme::search_current_bg`]
    /// so the focused match stands out from the other highlighted matches.
    pub(in crate::window) current: Option<(TabId, (usize, usize))>,
    /// When the query being typed is next searched for, and in which pane:
    /// the typing pauses first, rather than searching on every key.
    pub(in crate::window) debounce: Option<(Instant, TabId)>,
    /// The query being typed as a one-line field, for its Emacs keys: it
    /// keeps the caret and the kill ring between keys, and follows the query
    /// wherever something else sets it. Made the first time a query is typed.
    editor: Option<TextField>,
    /// The last query searched for, kept after the search is put away with `Esc`
    /// so `n`/`N` can pick it back up from wherever the cursor now is: vim keeps
    /// the pattern the same way across `:nohlsearch`.
    pub(in crate::window) last: Option<String>,
    /// 1-based index of the focused match (0 when no matches).
    pub(in crate::window) match_index: usize,
    pub(in crate::window) match_total: usize,
    /// Where the active search was launched from: the pane and the absolute
    /// buffer position (`(row, col)`) of the cursor when `/`/`?`/`*` was
    /// pressed. Every keystroke of the query re-searches from here, so typing
    /// doesn't creep forward through the buffer.
    pub(in crate::window) origin: Option<(TabId, (usize, usize))>,
    pub(in crate::window) query: Option<String>,
    /// Vim-style search direction: `?`/`#` set this so `n` repeats backward and
    /// `N` forward (both reversed from the `/`/`*` default). `SearchNext`
    /// (`n`) walks in this direction; `SearchPrevious` (`N`) walks the other.
    pub(in crate::window) reverse: bool,
}

// ========================================================================
// SearchState Implementations
// ========================================================================

impl SearchState {
    /// Edit the query being typed with `key`, at its caret, as a one-line
    /// field takes it. Returns whether the query changed; does nothing when
    /// no search is being typed.
    pub(in crate::window) fn edit_query(&mut self, key: &input::Key) -> bool {
        let Some(query) = self.query.as_mut() else {
            return false;
        };
        let editor = self.editor.get_or_insert_with(|| TextField::line(""));
        if editor.text() != query {
            editor.set_text(query);
        }
        editor.on_key(&key.typed(), 1);
        if editor.text() == query {
            return false;
        }
        *query = editor.text().to_string();
        true
    }

    /// How many characters of the query sit before the caret: where the
    /// field's keys left it, or the end when something else set the query.
    pub(in crate::window) fn caret(&self) -> usize {
        let query = self.query.as_deref().unwrap_or_default();
        match &self.editor {
            Some(editor) if editor.text() == query => editor.col(),
            _ => query.chars().count(),
        }
    }
}

// ========================================================================
// App: search
// ========================================================================

impl App {
    /// Search for the query being typed once its pause is up. When that will
    /// be, where it is still to come.
    pub(in crate::window) fn settle_search(&mut self, now: Instant) -> Option<Instant> {
        let (deadline, focused) = self.search.debounce?;
        if now < deadline {
            return Some(deadline);
        }
        self.search.debounce = None;
        self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
        self.dirty = true;
        None
    }

    /// The direction a fresh `/`/`?` search (or a `n` repeat) walks in: forward
    /// unless the active search is reversed (`?`/`#`), mirroring Vim's sticky
    /// search direction.
    pub(in crate::window) fn search_start_direction(&self) -> input::BlockNav {
        if self.search.reverse {
            input::BlockNav::Previous
        } else {
            input::BlockNav::Next
        }
    }

    /// Vim `*`/`#`: search for the whole word under the Normal-mode cursor,
    /// immediately (no input step): forward for `*`, backward for `#`. A
    /// non-word character under the cursor, or no nav cursor at all, leaves
    /// the current search untouched.
    pub(in crate::window) fn search_word_under_cursor(&mut self, focused: TabId, forward: bool) {
        let Some((row, col)) = self.nav_cursor(focused) else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let line = line_chars(terminal.grid(), row);
        let Some(word) = word_at(&line, col) else {
            return;
        };

        self.search.query = Some(word);
        self.search.reverse = !forward;
        if !self.config.status_bar.enabled {
            self.resize_all_panes();
        }
        // The cursor already sits on an occurrence, so scanning away from the
        // cursor (not from the match it lands on) makes `*` move to the *next*
        // one, as in Vim.
        self.set_search_origin(focused);
        self.search_step(focused, self.search_start_direction(), SearchFrom::Origin);
        self.dirty = true;
    }

    /// Record where the active search was launched from, so an incremental
    /// (`/`-typing) search always scans from that spot. Cleared when the search
    /// ends (`SearchCancel`, or leaving Normal mode).
    pub(in crate::window) fn set_search_origin(&mut self, focused: TabId) {
        let pos = self
            .open_tabs
            .terminal(&focused)
            .map(|terminal| {
                let grid = terminal.grid();
                let (row, col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
                let row = row.min(grid.rows().saturating_sub(1));
                (grid.to_absolute_row(row), col)
            })
            .or_else(|| {
                self.open_tabs.editor(&focused).and_then(|slot| {
                    let grid = slot.view.painted.as_ref()?;
                    let row = slot.view.cursor_line.unwrap_or(0);
                    let col = slot.editor.caret().map(|c| c.col).unwrap_or(0);
                    Some((grid.to_absolute_row(row), col))
                })
            });
        self.search.origin = pos.map(|p| (focused, p));
    }

    /// Repeat the search from the cursor (`n`/`N`). See [`App::search_step`].
    pub(in crate::window) fn search_in_pane(&mut self, focused: TabId, direction: input::BlockNav) {
        self.search_step(focused, direction, SearchFrom::Cursor);
    }

    /// Bring back the last query when `n`/`N` is pressed after the search was put
    /// away with `Esc`, so the repeat resumes from wherever the cursor now sits,
    /// vim's `n` revives the pattern (and its highlighting) the same way after
    /// `:nohlsearch`. A no-op while a search is still active, or when nothing has
    /// been searched for yet.
    pub(in crate::window) fn resume_last_search(&mut self) {
        if self.search.query.is_some() {
            return;
        }
        let Some(query) = self.search.last.clone() else {
            return;
        };
        self.search.query = Some(query);
        // Reviving the query forces the status bar back on when it's configured
        // hidden (see `status_bar_visible`), so match the pane geometry now.
        if !self.config.status_bar.enabled {
            self.resize_all_panes();
        }
    }

    /// Jump to the next/previous occurrence of the active search query and
    /// update the `[index/total]` counter.
    ///
    /// Counting and stepping are both **occurrence**-based: every individual hit
    /// across the whole buffer (scrollback plus the live screen) is one stop, so
    /// three hits on one line count as three, matching what the renderer
    /// highlights on screen, which shares [`buffer_match_starts`] with this, and
    /// `n` visits each in turn instead of skipping a line at a time. Steps are
    /// relative to `from`, not to the scroll offset, so a match already on
    /// screen still advances; running past the last (or first) match wraps
    /// around, as in Vim.
    pub(in crate::window) fn search_step(
        &mut self,
        focused: TabId,
        direction: input::BlockNav,
        from: SearchFrom,
    ) {
        let query = match &self.search.query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => {
                self.search.match_index = 0;
                self.search.match_total = 0;
                self.search.current = None;
                return;
            }
        };
        // Remembered past the end of this search so `n`/`N` can revive it.
        self.search.last = Some(query.clone());

        // Both the match list and the scan origin are read under one immutable
        // borrow of the pane, before the counters below take `&mut self`.
        let Some((matches, start)) = self.open_tabs.terminal(&focused).map(|terminal| {
            let grid = terminal.grid();
            (
                buffer_match_starts(grid, &query),
                self.search_start_pos(focused, from, grid),
            )
        }) else {
            return;
        };

        self.search.match_total = matches.len();
        if matches.is_empty() {
            self.search.match_index = 0;
            self.search.current = None;
            return;
        }

        let pos = match direction {
            input::BlockNav::Next => matches.iter().position(|&m| m > start).unwrap_or(0),
            input::BlockNav::Previous => matches
                .iter()
                .rposition(|&m| m < start)
                .unwrap_or(matches.len() - 1),
        };
        self.search.match_index = pos + 1;
        self.search.current = Some((focused, matches[pos]));
        if from == SearchFrom::Cursor {
            // A confirmed search jump is one of vim's jumplist jumps: record where
            // it started before revealing the match.
            self.push_jump(focused);
        }
        self.reveal_position(focused, matches[pos]);
        self.dirty = true;
    }

    /// Update the interactive search preview (match count and current match position)
    /// without jumping the viewport or pushing to the jumplist.
    pub(in crate::window) fn update_search_preview(&mut self, focused: TabId) {
        let found = match &self.search.query {
            Some(query) if !query.is_empty() => self.open_tabs.terminal(&focused).map(|terminal| {
                let grid = terminal.grid();
                (
                    buffer_match_starts(grid, query),
                    self.search_start_pos(focused, SearchFrom::Origin, grid),
                )
            }),
            _ => None,
        };
        let (matches, start) = found.unwrap_or_default();
        self.search.match_total = matches.len();
        if matches.is_empty() {
            self.search.match_index = 0;
            self.search.current = None;
            return;
        }
        let pos = match self.search_start_direction() {
            input::BlockNav::Next => matches.iter().position(|&m| m > start).unwrap_or(0),
            input::BlockNav::Previous => matches
                .iter()
                .rposition(|&m| m < start)
                .unwrap_or(matches.len() - 1),
        };
        self.search.match_index = pos + 1;
        self.search.current = Some((focused, matches[pos]));
    }

    /// The buffer position a search step scans away from: the launch origin
    /// while the query is being typed, otherwise the cursor (the Normal-mode nav
    /// cursor, falling back to the shell's own cursor).
    fn search_start_pos(
        &self,
        focused: TabId,
        from: SearchFrom,
        grid: &winter_core::Grid,
    ) -> MatchPos {
        if from == SearchFrom::Origin {
            if let Some((_, pos)) = self.search.origin.filter(|&(p, _)| p == focused) {
                return pos;
            }
        }
        let (row, col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let row = row.min(grid.rows().saturating_sub(1));
        (grid.to_absolute_row(row), col)
    }

    /// `gn` / `gN`: Select the next/previous search match in Visual mode.
    pub(in crate::window) fn select_search_match(&mut self, focused: TabId, forward: bool) {
        self.resume_last_search();
        let query = match &self.search.query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return,
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let spans = buffer_match_spans(grid, &query);
        if spans.is_empty() {
            return;
        }
        let (cur_row, cur_col) = self.nav_cursor(focused).unwrap_or_else(|| grid.cursor());
        let cur_abs = (grid.to_absolute_row(cur_row), cur_col);

        let target_idx = if forward {
            spans
                .iter()
                .position(|&(start, _)| start >= cur_abs)
                .unwrap_or(0)
        } else {
            spans
                .iter()
                .rposition(|&(_, end)| end <= cur_abs)
                .unwrap_or(spans.len() - 1)
        };

        let ((abs_start_row, start_col), (abs_end_row, end_col)) = spans[target_idx];
        self.open_tabs
            .set_mode(&focused, crate::model::mode::Mode::Visual);
        self.reveal_position(focused, (abs_end_row, end_col));
        if let Some(terminal) = self.open_tabs.terminal(&focused) {
            let grid = terminal.grid();
            let top = grid.scrollback_len() - grid.scroll_offset().min(grid.scrollback_len());
            let view_r2 = abs_end_row
                .saturating_sub(top)
                .min(grid.rows().saturating_sub(1));
            self.selection.visual_anchor = Some((abs_start_row, start_col));
            self.set_nav_cursor(focused, (view_r2, end_col));
            self.update_visual_selection(focused);
            self.dirty = true;
        }
    }

    /// `cgn` / `cgN`: Change the next/previous search match on the editable prompt line.
    pub(in crate::window) fn change_search_match(&mut self, focused: TabId, forward: bool) {
        self.resume_last_search();
        let query = match &self.search.query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return,
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot change: not on the editable prompt line");
            return;
        }
        let spans = buffer_match_spans(grid, &query);
        if spans.is_empty() {
            return;
        }
        let prompt_abs_row = grid.to_absolute_row(prompt_row);

        let target_span = if forward {
            spans
                .iter()
                .find(|&(start, _)| start.0 == prompt_abs_row && start.1 >= nav_col)
                .or_else(|| spans.iter().find(|&(start, _)| start.0 == prompt_abs_row))
        } else {
            spans
                .iter()
                .rfind(|&(_, end)| end.0 == prompt_abs_row && end.1 <= nav_col)
                .or_else(|| spans.iter().rfind(|&(start, _)| start.0 == prompt_abs_row))
        };

        let Some(&((_, c1), (_, c2))) = target_span else {
            self.set_error("No search match on prompt line");
            return;
        };

        self.delete_on_prompt(
            crate::window::terminal::prompt_edit::PromptDelete::Range {
                start_col: c1,
                end_col: c2,
            },
            focused,
        );
        self.open_tabs
            .set_mode(&focused, crate::model::mode::Mode::Insert);
        self.selection.span = None;
        self.selection.visual_anchor = None;
        self.dirty = true;
    }

    /// `dgn` / `dgN`: Delete the next/previous search match on the editable prompt line.
    pub(in crate::window) fn delete_search_match(&mut self, focused: TabId, forward: bool) {
        self.resume_last_search();
        let query = match &self.search.query {
            Some(q) if !q.is_empty() => q.clone(),
            _ => return,
        };
        let Some(terminal) = self.open_tabs.terminal(&focused) else {
            return;
        };
        let grid = terminal.grid();
        let (prompt_row, pty_col) = grid.cursor();
        let (nav_row, nav_col) = self.nav_cursor(focused).unwrap_or((prompt_row, pty_col));
        if nav_row != prompt_row {
            self.set_error("Cannot delete: not on the editable prompt line");
            return;
        }
        let spans = buffer_match_spans(grid, &query);
        if spans.is_empty() {
            return;
        }
        let prompt_abs_row = grid.to_absolute_row(prompt_row);

        let target_span = if forward {
            spans
                .iter()
                .find(|&(start, _)| start.0 == prompt_abs_row && start.1 >= nav_col)
                .or_else(|| spans.iter().find(|&(start, _)| start.0 == prompt_abs_row))
        } else {
            spans
                .iter()
                .rfind(|&(_, end)| end.0 == prompt_abs_row && end.1 <= nav_col)
                .or_else(|| spans.iter().rfind(|&(start, _)| start.0 == prompt_abs_row))
        };

        let Some(&((_, c1), (_, c2))) = target_span else {
            self.set_error("No search match on prompt line");
            return;
        };

        self.delete_on_prompt(
            crate::window::terminal::prompt_edit::PromptDelete::Range {
                start_col: c1,
                end_col: c2,
            },
            focused,
        );
        self.dirty = true;
    }
}

// ========================================================================
// Helpers
// ========================================================================

/// The keyword run (Vim's `w`/`b` class 1: alphanumerics and `_`) covering
/// `col` on `line`, or `None` when `col` sits on whitespace/punctuation. Used
/// by `*`/`#` to pull the search term from under the Normal-mode cursor.
fn word_at(line: &[char], col: usize) -> Option<String> {
    if col >= line.len() || char_class(line[col], false) != 1 {
        return None;
    }
    let start = (0..=col)
        .rev()
        .find(|&i| char_class(line[i], false) != 1)
        .map_or(0, |i| i + 1);
    let end = (col..line.len())
        .find(|&i| char_class(line[i], false) != 1)
        .unwrap_or(line.len());
    Some(line[start..end].iter().collect())
}

/// Every occurrence of `query` in the pane's whole buffer as inclusive
/// `((start_abs_row, start_col), (end_abs_row, end_col))` spans in buffer order.
pub(in crate::window) fn buffer_match_spans(
    grid: &winter_core::Grid,
    query: &str,
) -> Vec<((usize, usize), (usize, usize))> {
    if query.is_empty() {
        return vec![];
    }
    (0..grid.scrollback_len() + grid.rows())
        .flat_map(|abs_row| {
            let line = absolute_row_string(grid, abs_row);
            row_match_spans(&line, query)
                .into_iter()
                .filter_map(move |(start, end)| {
                    if end > start {
                        Some(((abs_row, start), (abs_row, end - 1)))
                    } else {
                        None
                    }
                })
        })
        .collect()
}

fn is_explicit_regex(query: &str) -> bool {
    query.chars().any(|c| {
        matches!(
            c,
            '^' | '$' | '*' | '+' | '?' | '[' | ']' | '(' | ')' | '{' | '}' | '|' | '\\'
        )
    })
}

/// The column spans `(start_col, end_col)` where `query` matches in `row_str`.
fn row_match_spans(row_str: &str, query: &str) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return vec![];
    }
    let has_uppercase = query.chars().any(char::is_uppercase);
    if let Ok(re) = regex::RegexBuilder::new(query)
        .case_insensitive(!has_uppercase)
        .build()
    {
        let mut spans = Vec::new();
        for m in re.find_iter(row_str) {
            let start_char_idx = row_str[..m.start()].chars().count();
            let len_chars = m.as_str().chars().count();
            if len_chars > 0 {
                spans.push((start_char_idx, start_char_idx + len_chars));
            }
        }
        if !spans.is_empty() || is_explicit_regex(query) {
            return spans;
        }
    }
    if has_uppercase {
        let query_chars: Vec<char> = query.chars().collect();
        let row_chars: Vec<char> = row_str.chars().collect();
        let qlen = query_chars.len();
        if qlen == 0 || row_chars.len() < qlen {
            return vec![];
        }
        (0..=row_chars.len() - qlen)
            .filter(|&s| row_chars[s..s + qlen] == query_chars)
            .map(|s| (s, s + qlen))
            .collect()
    } else {
        let query_lower: Vec<char> = query.to_lowercase().chars().collect();
        let row_chars: Vec<char> = row_str.to_lowercase().chars().collect();
        let qlen = query_lower.len();
        if qlen == 0 || row_chars.len() < qlen {
            return vec![];
        }
        (0..=row_chars.len() - qlen)
            .filter(|&s| row_chars[s..s + qlen] == query_lower)
            .map(|s| (s, s + qlen))
            .collect()
    }
}

/// Every occurrence of `query` in the pane's whole buffer: scrollback plus
/// the live screen, as absolute `(row, col)` start positions in buffer order.
pub(in crate::window) fn buffer_match_starts(
    grid: &winter_core::Grid,
    query: &str,
) -> Vec<MatchPos> {
    if query.is_empty() {
        return vec![];
    }
    (0..grid.scrollback_len() + grid.rows())
        .flat_map(|abs_row| {
            let line = absolute_row_string(grid, abs_row);
            row_match_spans(&line, query)
                .into_iter()
                .map(move |(start, _)| (abs_row, start))
        })
        .collect()
}

/// The on-screen cells covered by matches of `query`, as viewport `(row, col)` pairs.
pub(in crate::window) fn visible_match_cells(
    grid: &winter_core::Grid,
    query: &str,
) -> Vec<(usize, usize)> {
    if query.is_empty() {
        return vec![];
    }
    let top = grid.to_absolute_row(0);
    (0..grid.rows())
        .flat_map(|row| {
            let line = absolute_row_string(grid, top + row);
            row_match_spans(&line, query)
                .into_iter()
                .flat_map(move |(start, end)| (start..end).map(move |col| (row, col)))
        })
        .collect()
}

/// Where `query` matches on each row an editor painted into `grid`, as viewport
/// `(row, start_col, end_col)` with the end exclusive. Matched by the rule
/// every editor searches by ([`match_spans`]), not the terminal's patterns, so
/// the rows an editor's search lands on are the rows that light up.
pub(in crate::window) fn painted_match_spans(
    grid: &winter_core::Grid,
    query: &str,
) -> Vec<(usize, usize, usize)> {
    let top = grid.to_absolute_row(0);
    (0..grid.rows())
        .flat_map(|row| {
            match_spans(&absolute_row_string(grid, top + row), query)
                .into_iter()
                .map(move |(start, end)| (row, start, end))
        })
        .collect()
}

/// Absolute row `abs_row` as string, read via [`winter_core::Grid::absolute_cell`].
fn absolute_row_string(grid: &winter_core::Grid, abs_row: usize) -> String {
    (0..grid.cols())
        .filter_map(|c| grid.absolute_cell(abs_row, c))
        .map(|cell| cell.ch)
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mode::Mode;
    use crate::terminal::session::Terminal;
    use crate::window::test_support::*;
    use std::time::Instant;

    #[test]
    fn test_word_at_returns_the_keyword_run_under_the_column() {
        let line: Vec<char> = "foo_bar baz".chars().collect();
        assert_eq!(word_at(&line, 0), Some("foo_bar".to_string()));
        assert_eq!(word_at(&line, 6), Some("foo_bar".to_string()));
        assert_eq!(word_at(&line, 8), Some("baz".to_string()));
    }

    #[test]
    fn test_word_at_returns_none_on_whitespace_or_out_of_bounds() {
        let line: Vec<char> = "foo bar".chars().collect();
        assert_eq!(word_at(&line, 3), None);
        assert_eq!(word_at(&line, 99), None);
        assert_eq!(word_at(&[], 0), None);
    }

    /// A pane whose live grid holds `lines`, written straight into the grid so
    /// the fixture depends on neither the shell's prompt nor PTY timing. The
    /// spawned `cat` just parks on stdin and prints nothing of its own, leaving
    /// the grid exactly as written here.
    fn pane_with_lines(lines: &[&str]) -> Terminal {
        let mut terminal = Terminal::with_command(
            40,
            8,
            portable_pty::CommandBuilder::new("cat"),
            winter_core::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        let grid = terminal.grid_mut();
        for (row, line) in lines.iter().enumerate() {
            grid.move_to(row, 0);
            for ch in line.chars() {
                grid.print(ch);
            }
        }
        grid.move_to(0, 0);
        terminal
    }

    /// An app whose focused pane holds `lines`, with the Normal-mode cursor
    /// parked at the top-left (before every match in the fixtures below) and
    /// `query` as the active search.
    fn app_with_lines(lines: &[&str], query: &str) -> (crate::window::App, TabId) {
        let mut app = crate::window::App::new();
        // With the status bar configured on, search doesn't trigger a pane
        // resize; headless (no window) that resize collapses the pane to a
        // single row and shoves the fixture into scrollback.
        app.config.status_bar.enabled = true;
        let id = app.layout().shown_tabs()[0];
        app.open_tabs.insert_terminal(id, pane_with_lines(lines));
        app.set_nav_cursor(id, (0, 0));
        app.search.query = Some(query.to_string());
        (app, id)
    }

    /// Three occurrences of `ink`: two on one row, one on the next: the shape
    /// that used to collapse into a single navigation stop.
    const LINES: &[&str] = &["prompt$", "ink one ink", "two ink", ""];

    #[test]
    fn test_search_start_direction_follows_search_reverse() {
        let mut app = crate::window::App::new();
        assert_eq!(app.search_start_direction(), input::BlockNav::Next);
        app.search.reverse = true;
        assert_eq!(app.search_start_direction(), input::BlockNav::Previous);
    }

    #[test]
    fn test_buffer_match_starts_lists_every_occurrence_including_repeats_on_a_row() {
        let terminal = pane_with_lines(LINES);
        assert_eq!(
            buffer_match_starts(terminal.grid(), "ink"),
            vec![(1, 0), (1, 8), (2, 4)]
        );
    }

    #[test]
    fn test_visible_match_cells_cover_exactly_the_counted_matches() {
        // The counter and the highlighting read the same match list, so the
        // highlighted cell count is always the match count times the query
        // length: the mismatch that made "[1/3]" disagree with the screen.
        let terminal = pane_with_lines(LINES);
        let cells = visible_match_cells(terminal.grid(), "ink");
        assert_eq!(
            cells.len(),
            buffer_match_starts(terminal.grid(), "ink").len() * 3
        );
        assert_eq!(&cells[..3], &[(1, 0), (1, 1), (1, 2)]);
    }

    #[test]
    fn test_regex_search_matches_patterns_and_variable_lengths() {
        let terminal = pane_with_lines(&["foo123", "bar45678", "baz"]);
        let starts = buffer_match_starts(terminal.grid(), r"\d+");
        assert_eq!(starts, vec![(0, 3), (1, 3)]);
        let cells = visible_match_cells(terminal.grid(), r"\d+");
        // Row 0 has 3 digits, Row 1 has 5 digits -> total 8 cells
        assert_eq!(cells.len(), 8);
    }

    #[test]
    fn test_search_counts_occurrences_not_rows() {
        // Regression: several matches visible in one screenful used to collapse
        // into a single navigation stop (reported "1/1" for a row holding two
        // hits) because the old implementation counted distinct scroll targets,
        // not occurrences: out of step with the on-screen highlighting.
        let (mut app, id) = app_with_lines(LINES, "ink");

        app.search_in_pane(id, input::BlockNav::Next);

        assert_eq!(app.search.match_total, 3);
        assert_eq!(app.search.match_index, 1);
        assert_eq!(
            app.nav_cursor(id),
            Some((1, 0)),
            "cursor lands on the match"
        );
    }

    #[test]
    fn test_search_next_steps_through_every_match_on_screen_then_wraps() {
        // Regression: `n` compared matches against the viewport's top row, so
        // once a match was on screen the scroll target never changed and the
        // search appeared stuck. Steps are relative to the cursor now.
        let (mut app, id) = app_with_lines(LINES, "ink");

        let mut stops = Vec::new();
        for _ in 0..4 {
            app.search_in_pane(id, input::BlockNav::Next);
            stops.push((app.search.match_index, app.nav_cursor(id).unwrap()));
        }

        assert_eq!(
            stops,
            vec![(1, (1, 0)), (2, (1, 8)), (3, (2, 4)), (1, (1, 0))],
            "n should visit each occurrence in turn and wrap to the first"
        );
    }

    #[test]
    fn test_search_previous_steps_backward_and_wraps_to_the_last_match() {
        let (mut app, id) = app_with_lines(LINES, "ink");

        // From the top-left cursor there's nothing earlier, so `N` wraps to the
        // last match and then walks back through the rest.
        let mut stops = Vec::new();
        for _ in 0..3 {
            app.search_in_pane(id, input::BlockNav::Previous);
            stops.push((app.search.match_index, app.nav_cursor(id).unwrap()));
        }

        assert_eq!(stops, vec![(3, (2, 4)), (2, (1, 8)), (1, (1, 0))]);
    }

    #[test]
    fn test_incremental_search_rescans_from_the_launch_position() {
        // Typing `i`, `n`, `k` must keep previewing the first match after the
        // `/` cursor (Vim's incsearch), not step one match forward per keystroke.
        let (mut app, id) = app_with_lines(LINES, "");
        app.set_search_origin(id);

        for c in "ink".chars() {
            app.search.query.as_mut().unwrap().push(c);
            app.search_step(id, input::BlockNav::Next, SearchFrom::Origin);
        }

        assert_eq!(app.search.match_index, 1);
        assert_eq!(app.nav_cursor(id), Some((1, 0)));
    }

    #[test]
    fn test_search_with_no_matches_reports_a_zero_counter() {
        let (mut app, id) = app_with_lines(LINES, "nomatch");

        app.search_in_pane(id, input::BlockNav::Next);

        assert_eq!(app.search.match_total, 0);
        assert_eq!(app.search.match_index, 0);
    }

    #[test]
    fn test_search_scrolls_a_match_in_scrollback_into_view() {
        let mut app = crate::window::App::new();
        app.config.status_bar.enabled = true;
        let id = app.layout().shown_tabs()[0];
        let mut terminal = pane_with_lines(&["ink here"]);
        // Push the match off the top of the screen into scrollback.
        let rows = terminal.grid().rows();
        for _ in 0..rows * 2 {
            terminal.grid_mut().line_feed();
        }
        assert!(terminal.grid().scrollback_len() >= rows);
        app.open_tabs.insert_terminal(id, terminal);
        app.search.query = Some("ink".to_string());

        app.search_in_pane(id, input::BlockNav::Next);

        assert_eq!(app.search.match_total, 1);
        let grid = app.open_tabs.terminal(&id).unwrap().grid();
        let (row, col) = app.nav_cursor(id).unwrap();
        let visible: String = (0..3)
            .filter_map(|k| grid.visible_cell(row, col + k))
            .map(|cell| cell.ch)
            .collect();
        assert_eq!(visible, "ink", "the match should be scrolled into view");
    }

    #[test]
    fn test_slash_then_repeats_walk_matches_through_the_action_layer() {
        // End-to-end over the dispatched actions: `/ink<CR>` lands on the first
        // match after the cursor, `n` advances, `N` steps back: the counter
        // tracking each stop out of the three occurrences on screen.
        use crate::model::input::Action;

        let (mut app, id) = app_with_lines(LINES, "");
        app.search.query = None;

        app.handle_action(Action::SearchStart, id);
        for c in "ink".chars() {
            app.handle_action(
                Action::SearchEdit(input::Key::plain(input::KeyCode::Char(c))),
                id,
            );
        }
        app.handle_action(Action::SearchExecute, id);
        assert_eq!((app.search.match_index, app.search.match_total), (1, 3));

        app.handle_action(Action::SearchNext, id);
        assert_eq!(app.search.match_index, 2);
        app.handle_action(Action::SearchNext, id);
        assert_eq!(app.search.match_index, 3);
        app.handle_action(Action::SearchPrevious, id);
        assert_eq!(app.search.match_index, 2);

        // Esc ends the search but leaves the cursor on the match it walked to.
        app.handle_action(Action::SearchCancel, id);
        assert_eq!(app.search.query, None);
        assert_eq!(app.search.match_total, 0);
        assert_eq!(app.nav_cursor(id), Some((1, 8)));
    }

    #[test]
    fn test_n_after_esc_resumes_the_last_search_from_the_cursor() {
        // `Esc` puts the search away but keeps the pattern, so `n` picks it back
        // up from wherever the cursor now is (vim's `n` after `:nohlsearch`),
        // rather than doing nothing.
        use crate::model::input::Action;

        let (mut app, id) = app_with_lines(LINES, "ink");
        app.search_in_pane(id, input::BlockNav::Next);
        assert_eq!(app.search.match_index, 1);

        app.handle_action(Action::SearchCancel, id);
        assert_eq!(app.search.query, None);
        assert_eq!(app.search.last.as_deref(), Some("ink"));

        app.handle_action(Action::SearchNext, id);

        assert_eq!(
            app.search.query.as_deref(),
            Some("ink"),
            "highlight comes back"
        );
        assert_eq!(app.search.match_total, 3);
        // Resumed from the cursor's position (the first match), so it advances to
        // the second rather than restarting from the top.
        assert_eq!(app.search.match_index, 2);
        assert_eq!(app.nav_cursor(id), Some((1, 8)));
    }

    #[test]
    fn test_repeat_does_nothing_when_nothing_has_been_searched_for() {
        use crate::model::input::Action;

        let (mut app, id) = app_with_lines(LINES, "");
        app.search.query = None;

        app.handle_action(Action::SearchNext, id);

        assert_eq!(app.search.query, None);
        assert_eq!(app.search.match_total, 0);
    }

    #[test]
    fn test_search_current_tracks_the_focused_match_for_the_highlight() {
        // The renderer paints this one match in `search_current_bg` and the rest
        // in `search_match_bg`, so it has to follow `n`/`N` and clear with the
        // search itself.
        use crate::model::input::Action;

        let (mut app, id) = app_with_lines(LINES, "ink");

        app.search_in_pane(id, input::BlockNav::Next);
        assert_eq!(app.search.current, Some((id, (1, 0))));
        app.search_in_pane(id, input::BlockNav::Next);
        assert_eq!(app.search.current, Some((id, (1, 8))));

        app.handle_action(Action::SearchCancel, id);
        assert_eq!(app.search.current, None);
    }

    #[test]
    fn test_search_current_clears_when_the_query_stops_matching() {
        let (mut app, id) = app_with_lines(LINES, "ink");
        app.search_in_pane(id, input::BlockNav::Next);
        assert!(app.search.current.is_some());

        app.search.query = Some("nomatch".to_string());
        app.search_in_pane(id, input::BlockNav::Next);

        assert_eq!(app.search.current, None);
    }

    #[test]
    fn test_search_word_under_cursor_forward_sets_query_and_direction() {
        let (mut app, id) = app_with_lines(&["needle here", "needle"], "");
        app.search.query = None;

        app.search_word_under_cursor(id, true);

        assert_eq!(app.search.query.as_deref(), Some("needle"));
        assert!(!app.search.reverse);
        // The cursor already sat on an occurrence, so `*` moves to the next one.
        assert_eq!(app.nav_cursor(id), Some((1, 0)));
    }

    #[test]
    fn test_search_word_under_cursor_backward_sets_query_and_direction() {
        let (mut app, id) = app_with_lines(&["needle here", "needle"], "");
        app.search.query = None;
        app.set_nav_cursor(id, (1, 0));

        app.search_word_under_cursor(id, false);

        assert_eq!(app.search.query.as_deref(), Some("needle"));
        assert!(app.search.reverse);
        assert_eq!(app.nav_cursor(id), Some((0, 0)));
    }

    #[test]
    fn test_search_word_under_cursor_on_blank_cell_leaves_query_untouched() {
        let (mut app, id) = app_with_lines(&["needle"], "");
        app.search.query = None;
        // Column 30 is well past "needle": blank padding.
        app.set_nav_cursor(id, (0, 30));

        app.search_word_under_cursor(id, true);

        assert_eq!(app.search.query, None);
    }

    #[test]
    fn test_select_search_match_gn() {
        let (mut app, id) = app_with_lines(&["first match here and second match"], "match");
        app.set_nav_cursor(id, (0, 0));

        // Select next match with `gn`
        app.handle_action(input::Action::SelectSearchMatch { forward: true }, id);

        // Mode is Visual
        assert_eq!(
            app.open_tabs.mode(&id),
            Some(crate::model::mode::Mode::Visual)
        );
        // Visual anchor is at (0, 6), nav cursor is at (0, 10)
        assert_eq!(app.selection.visual_anchor, Some((0, 6)));
        assert_eq!(app.nav_cursor(id), Some((0, 10)));
    }

    #[test]
    fn test_change_search_match_cgn() {
        let (mut app, id) = app_with_lines(&["echo test match"], "match");
        app.set_nav_cursor(id, (0, 0));

        // Change next match with `cgn`
        app.handle_action(input::Action::ChangeSearchMatch { forward: true }, id);

        // Transitions to Mode::Insert
        assert_eq!(
            app.open_tabs.mode(&id),
            Some(crate::model::mode::Mode::Insert)
        );
    }

    #[test]
    fn test_search_smartcase_matching() {
        // Lowercase query matches case-insensitively
        let spans_lower = row_match_spans("Foo foo FOO", "foo");
        assert_eq!(spans_lower.len(), 3, "lowercase query matches all cases");

        // Query with uppercase character matches exact case
        let spans_upper = row_match_spans("Foo foo FOO", "Foo");
        assert_eq!(
            spans_upper,
            vec![(0, 3)],
            "uppercase query matches exact case"
        );
    }

    #[test]
    fn test_the_search_line_is_edited_at_its_caret() {
        let (mut app, id) = app_with_lines(LINES, "");
        app.handle_action(input::Action::SearchStart, id);
        let typed = |c| input::Key::plain(input::KeyCode::Char(c));
        for c in "ink".chars() {
            app.handle_action(input::Action::SearchEdit(typed(c)), id);
        }
        let ctrl_a = input::Key {
            ctrl: true,
            ..typed('a')
        };
        app.handle_action(input::Action::SearchEdit(ctrl_a), id);
        assert_eq!(app.search.caret(), 0);
        app.handle_action(input::Action::SearchEdit(typed('l')), id);
        assert_eq!(app.search.query.as_deref(), Some("link"));
        assert_eq!(
            app.search.caret(),
            1,
            "after what was typed, not at the end"
        );
    }

    #[test]
    fn test_interactive_search_preview_and_cancel_restores_origin() {
        let (mut app, id) = app_with_lines(LINES, "");
        app.set_nav_cursor(id, (0, 5));
        let jumps_before = app.vim.tab(id).map(|vim| vim.jump_list.len()).unwrap_or(0);

        app.handle_action(input::Action::SearchStart, id);
        assert_eq!(app.search.origin, Some((id, (0, 5))));

        // Typing updates match count immediately without touching jumplist
        app.handle_action(
            input::Action::SearchEdit(input::Key::plain(input::KeyCode::Char('i'))),
            id,
        );
        assert_eq!(app.search.match_total, 3);
        let jumps_during = app.vim.tab(id).map(|vim| vim.jump_list.len()).unwrap_or(0);
        assert_eq!(
            jumps_during, jumps_before,
            "typing does not push to jumplist"
        );

        app.handle_action(
            input::Action::SearchEdit(input::Key::plain(input::KeyCode::Char('n'))),
            id,
        );
        app.handle_action(
            input::Action::SearchEdit(input::Key::plain(input::KeyCode::Char('k'))),
            id,
        );
        assert_eq!(app.search.match_total, 3);
        assert_eq!(app.search.match_index, 1);
        assert_eq!(app.search.current, Some((id, (1, 0))));

        // Cancel restores nav cursor to the search origin
        app.handle_action(input::Action::SearchCancel, id);
        assert_eq!(app.search.query, None);
        assert_eq!(
            app.nav_cursor(id),
            Some((0, 5)),
            "Esc restores search origin"
        );
    }

    #[test]
    fn test_search_debounce_scheduling_and_cancellation() {
        let mut app = app_with_tabs(1);
        let focused = app.layout().focused();
        app.open_tabs.set_mode(&focused, Mode::Normal);

        app.handle_action(input::Action::SearchStart, focused);
        assert_eq!(app.search.query, Some(String::new()));
        assert!(app.search.debounce.is_none());

        // Typing a character schedules debounce
        app.handle_action(
            input::Action::SearchEdit(input::Key::plain(input::KeyCode::Char('f'))),
            focused,
        );
        assert_eq!(app.search.query, Some("f".to_string()));
        assert!(app.search.debounce.is_some());
        let (deadline, tab) = app.search.debounce.unwrap();
        assert_eq!(tab, focused);
        assert!(deadline > Instant::now());

        // Backspace reschedules debounce
        app.handle_action(
            input::Action::SearchEdit(input::Key::plain(input::KeyCode::Backspace)),
            focused,
        );
        assert_eq!(app.search.query, Some(String::new()));
        assert!(app.search.debounce.is_some());

        // SearchExecute clears debounce
        app.handle_action(input::Action::SearchExecute, focused);
        assert!(app.search.debounce.is_none());

        // Start again and test SearchCancel clears debounce
        app.handle_action(input::Action::SearchStart, focused);
        app.handle_action(
            input::Action::SearchEdit(input::Key::plain(input::KeyCode::Char('x'))),
            focused,
        );
        assert!(app.search.debounce.is_some());
        app.handle_action(input::Action::SearchCancel, focused);
        assert!(app.search.debounce.is_none());
        assert!(app.search.query.is_none());
    }
}

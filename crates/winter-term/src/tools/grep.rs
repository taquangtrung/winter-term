//! Grep: the lines under a directory that hold some text, grouped by file and
//! opened straight from the list, or edited in place and written back to the
//! files they came from (`Ctrl-X Ctrl-Q`), in the manner of Emacs' `wgrep`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::model::input::{CursorMove, Key, KeyCode};
use crate::model::paint::{
    row_height, row_text, wheel_scroll, window_anchor, wrap_window, Caret, Frame, Row, Span, Style,
};
use crate::model::textedit::field::TextField;
use crate::model::textedit::modal::EditOutcome;
use crate::model::tool::{
    JobReply, JobRequest, OpenTarget, PromptMode, PromptReply, PromptRequest, SearchHit,
    SearchRequest, SearchResult, Tool, ToolMenuItem, ToolNotice, ToolOutcome, ToolStatus,
};
use crate::model::vim::nav::buffer_end;
use crate::tools::editor::file;

// ========================================================================
// Constants
// ========================================================================

/// The one question the tool asks.
const ASK_QUERY: &str = "query";

/// Rows of header above the first result.
const HEADER_ROWS: usize = 1;

/// Width the line numbers are padded to, so the texts they label line up.
const LINE_WIDTH: usize = 5;

/// What sits between a line number and its text.
const LINE_GAP: &str = ": ";

/// Shown beside the root before anything has been searched for.
const EMPTY_HINT: &str = "/ to search";

/// The tool's title, before a query narrows it.
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
pub struct GrepTool {
    cursor: usize,
    /// The matched lines as editable text while they are being rewritten in
    /// place, one line per hit.
    edit: Option<TextField>,
    /// Whether `Ctrl-X` has just been pressed, which `Ctrl-Q` completes into
    /// the chord that starts or ends editing.
    edit_chord: bool,
    hits: Vec<SearchHit>,
    /// What the last search or edit reported, waiting for the status bar to
    /// take it.
    notice: Option<ToolNotice>,
    /// The text last searched for, which a new question starts out holding.
    query: String,
    root: PathBuf,
    rows: Vec<ResultRow>,
    scroll: usize,
    /// Whether the wheel, not the cursor, last placed the view: the view then
    /// stays where it was put until a key brings the cursor back into it.
    wheel: bool,
    /// How many rows the pane showed on the last paint, which the wheel needs
    /// between paints to stop at the last screenful.
    viewport: usize,
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
    spans: Row,
}

// ========================================================================
// GrepTool
// ========================================================================

impl GrepTool {
    /// An empty search over `root`, waiting to be asked for something.
    pub fn new(root: PathBuf) -> Self {
        Self {
            cursor: 0,
            edit: None,
            edit_chord: false,
            hits: Vec::new(),
            notice: None,
            query: String::new(),
            root,
            rows: Vec::new(),
            scroll: 0,
            wheel: false,
            viewport: 0,
            searching: false,
            truncated: false,
        }
    }

    /// Ask what to look for, starting from what was looked for last so that
    /// narrowing a query is an edit rather than a retype.
    fn ask_query(&self) -> ToolOutcome {
        ToolOutcome::Prompt(PromptRequest {
            initial: self.query.clone(),
            label: "grep: ".to_string(),
            mode: PromptMode::Text,
            tag: ASK_QUERY,
        })
    }

    /// Look for `query`, dropping what the last search found: a result list
    /// half from one query and half from another would answer neither.
    fn start_search(&mut self, query: &str) -> ToolOutcome {
        if query.is_empty() {
            return ToolOutcome::Consumed;
        }
        self.query = query.to_string();
        self.hits.clear();
        self.rows.clear();
        self.cursor = 0;
        self.scroll = 0;
        self.searching = true;
        self.truncated = false;
        ToolOutcome::Job(JobRequest::Search(SearchRequest {
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
            self.notice = Some(ToolNotice::info(format!("no matches for {}", self.query)));
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
    fn open_selected(&self) -> ToolOutcome {
        match self.selected() {
            Some(hit) => ToolOutcome::OpenPath(OpenTarget::at_line(hit.path.clone(), hit.line)),
            None => ToolOutcome::Consumed,
        }
    }

    /// Hand the file the cursor is on to `$EDITOR`, in a pane of its own, at
    /// the line that matched.
    fn open_external(&self) -> ToolOutcome {
        match self.selected() {
            Some(hit) => ToolOutcome::SpawnEditor(OpenTarget::at_line(hit.path.clone(), hit.line)),
            None => ToolOutcome::Consumed,
        }
    }

    /// Copy where the match is, in the `path:line` form every tool understands.
    fn yank_selected(&self) -> ToolOutcome {
        match self.selected() {
            Some(hit) => ToolOutcome::Yank(format!(
                "{}:{}",
                relative_to(&self.root, &hit.path),
                hit.line
            )),
            None => ToolOutcome::Consumed,
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
            self.notice = Some(ToolNotice::error("nothing to edit"));
            return;
        }
        let lines = self.hits.iter().map(|hit| hit.text.clone()).collect();
        let at = self.rows.get(self.cursor).map_or(0, |row| row.hit);
        self.edit = Some(TextField::rows(lines, at));
        if let Some(row) = self.row_of_hit(at) {
            self.cursor = row;
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
        self.notice = (edited > 0).then(|| ToolNotice::info(format!("discarded {edited}")));
    }

    /// Write the edited lines back to their files, each file read and written
    /// once. A line whose file no longer says what the search found there is
    /// left alone and counted, since writing over it would throw away a change
    /// made since; so is a line the search cut short, which cannot be put back
    /// whole.
    fn apply_edits(&mut self) -> ToolOutcome {
        let edited = self.edited();
        self.edit = None;
        if edited.is_empty() {
            self.notice = Some(ToolNotice::info("no changes"));
            return ToolOutcome::Consumed;
        }
        let mut by_file: BTreeMap<PathBuf, Vec<(usize, String)>> = BTreeMap::new();
        for (index, text) in edited {
            by_file
                .entry(self.hits[index].path.clone())
                .or_default()
                .push((index, text));
        }
        let (mut written, mut files, mut refused) = (0, 0, 0);
        for (path, changes) in by_file {
            let Ok(mut loaded) = file::load(&path) else {
                refused += changes.len();
                continue;
            };
            let mut wrote_here = 0;
            for (index, text) in changes {
                let hit = &self.hits[index];
                let Some(line) = loaded.lines.get_mut(hit.line.saturating_sub(1)) else {
                    refused += 1;
                    continue;
                };
                let trimmed = line.trim_end();
                if trimmed != hit.text {
                    refused += 1;
                    continue;
                }
                // What trailed the text on disk was never shown, so it stays.
                let trailing = line[trimmed.len()..].to_string();
                *line = format!("{text}{trailing}");
                wrote_here += 1;
            }
            if wrote_here == 0 {
                continue;
            }
            match file::save(&path, &loaded.lines, loaded.shape) {
                Ok(_) => {
                    for hit in self.hits.iter_mut().filter(|hit| hit.path == path) {
                        if let Some(line) = loaded.lines.get(hit.line.saturating_sub(1)) {
                            hit.text = line.trim_end().to_string();
                        }
                    }
                    written += wrote_here;
                    files += 1;
                }
                Err(_) => refused += wrote_here,
            }
        }
        self.rebuild();
        let mut report = format!("wrote {} in {files} files", lines_count(written));
        if refused > 0 {
            report.push_str(&format!(
                ", {} changed on disk, left alone",
                lines_count(refused)
            ));
        }
        self.notice = Some(ToolNotice::info(report));
        ToolOutcome::Consumed
    }

    /// Keys while the matches are editable text: the editor's keys, the cursor
    /// following the line being edited. `ZZ`, `Ctrl-S`, or `Enter` while
    /// typing writes; `ZQ`, or `q` with nothing part-typed, throws it away.
    fn on_edit_key(&mut self, key: &Key) -> ToolOutcome {
        let at = self.rows.get(self.cursor).map(|row| row.hit);
        let viewport = self.viewport.max(1);
        let Some(edit) = self.edit.as_mut() else {
            return ToolOutcome::Ignored;
        };
        if edit.modal().is_idle() && key.code == KeyCode::Char('q') && !key.ctrl && !key.alt {
            self.discard_edit();
            return ToolOutcome::Consumed;
        }
        if let Some(at) = at {
            edit.move_to_row(at);
        }
        let outcome = edit.on_key(key, viewport);
        let row = edit.row();
        if let Some(notice) = edit.take_notice() {
            self.notice = Some(notice);
        }
        if let Some(cursor) = self.row_of_hit(row) {
            self.cursor = cursor;
        }
        match outcome {
            EditOutcome::Quit { save: true } | EditOutcome::Save | EditOutcome::Submit => {
                self.apply_edits()
            }
            EditOutcome::Quit { save: false } | EditOutcome::Cancel => {
                self.discard_edit();
                ToolOutcome::Consumed
            }
            EditOutcome::Yank(text) => ToolOutcome::Yank(text),
            EditOutcome::Paste => ToolOutcome::Paste,
            EditOutcome::Ignored if key.code == KeyCode::Escape => ToolOutcome::Consumed,
            EditOutcome::Ignored => ToolOutcome::Ignored,
            EditOutcome::Consumed
            | EditOutcome::Escalate
            | EditOutcome::ListBuffers
            | EditOutcome::Search { .. }
            | EditOutcome::StepBuffer { .. } => ToolOutcome::Consumed,
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
            spans.push(Span::new(Style::Dim, format!("  {EMPTY_HINT}")));
            return spans;
        }
        spans.push(Span::new(Style::Accent, format!("  /{}", self.query)));
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

impl Tool for GrepTool {
    fn title(&self) -> String {
        if self.query.is_empty() {
            return TITLE.to_string();
        }
        format!("{TITLE}: {}", self.query)
    }

    fn on_scroll(&mut self, lines: isize) -> ToolOutcome {
        // The view moves and the cursor stays, as in an editor.
        self.scroll = wheel_scroll(self.scroll, lines, self.rows.len(), self.viewport);
        self.wheel = true;
        ToolOutcome::Consumed
    }

    fn content(&mut self, rows: usize, cols: usize, wrap: bool) -> Frame {
        let mut frame_rows = vec![self.header_row()];
        if self.rows.is_empty() {
            return Frame::new(frame_rows);
        }
        let texts: Vec<String> = self.rows.iter().map(|row| row_text(&row.spans)).collect();
        let visible = rows.saturating_sub(HEADER_ROWS);
        self.viewport = visible;
        let anchor = window_anchor(self.wheel, self.scroll, self.cursor, visible);
        let window = wrap_window(self.scroll, anchor, texts.len(), visible, |index| {
            row_height(&texts[index], cols, wrap, 0)
        });
        self.scroll = window.start;
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
        let content = Frame::new(frame_rows);
        match anchor == self.cursor {
            true => content.with_cursor_line(HEADER_ROWS + window.cursor),
            false => content,
        }
    }

    fn take_notice(&mut self) -> Option<ToolNotice> {
        self.notice.take()
    }

    /// The edit mode while the matched lines are being rewritten.
    fn status(&self) -> ToolStatus {
        ToolStatus {
            mode: self.edit.as_ref().map(|edit| edit.modal().mode()),
            segments: Vec::new(),
        }
    }

    fn on_key(&mut self, key: &Key) -> ToolOutcome {
        self.wheel = false;
        if std::mem::take(&mut self.edit_chord) {
            if key.ctrl && key.code == KeyCode::Char('q') {
                self.toggle_edit();
            }
            return ToolOutcome::Consumed;
        }
        if key.ctrl && key.code == KeyCode::Char('x') {
            self.edit_chord = true;
            return ToolOutcome::Consumed;
        }
        if self.edit.is_some() {
            return self.on_edit_key(&key.typed());
        }
        if key.ctrl && key.code == KeyCode::Char('o') {
            return self.open_external();
        }
        if key.alt {
            if let Some(motion) = buffer_end(key) {
                self.cursor = match motion {
                    CursorMove::Top => 0,
                    _ => self.rows.len().saturating_sub(1),
                };
                return ToolOutcome::Consumed;
            }
            return match key.code {
                KeyCode::Char('n') => {
                    self.move_to_row(true, true);
                    ToolOutcome::Consumed
                }
                KeyCode::Char('p') => {
                    self.move_to_row(false, true);
                    ToolOutcome::Consumed
                }
                _ => ToolOutcome::Ignored,
            };
        }
        match key.code {
            KeyCode::Char('j') | KeyCode::Down => {
                self.move_by(1);
                ToolOutcome::Consumed
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.move_by(-1);
                ToolOutcome::Consumed
            }
            // `n` and `N` mean the next and previous match here, as they do in
            // every other tool: the list is already the search, so there is
            // nothing else for them to step through.
            KeyCode::Char('n') => {
                self.move_to_row(true, false);
                ToolOutcome::Consumed
            }
            KeyCode::Char('N') => {
                self.move_to_row(false, false);
                ToolOutcome::Consumed
            }
            KeyCode::Home => {
                self.cursor = 0;
                ToolOutcome::Consumed
            }
            KeyCode::End => {
                self.cursor = self.rows.len().saturating_sub(1);
                ToolOutcome::Consumed
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
                self.notice = Some(ToolNotice::info("search stopped"));
                ToolOutcome::CancelJobs
            }
            _ => ToolOutcome::Ignored,
        }
    }

    fn on_prompt(&mut self, reply: PromptReply) -> ToolOutcome {
        self.wheel = false;
        match reply.answer {
            Some(answer) => self.start_search(&answer),
            None => ToolOutcome::Consumed,
        }
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> ToolOutcome {
        if tag == ASK_QUERY && query.trim().len() >= 2 {
            self.start_search(query)
        } else {
            ToolOutcome::Consumed
        }
    }

    fn on_job(&mut self, reply: JobReply) -> ToolOutcome {
        match reply {
            JobReply::Search(found) => self.on_found(found),
            // The tool runs no commands and asks for no directory totals.
            JobReply::Command(_)
            | JobReply::DirSize { .. }
            | JobReply::Files(_)
            | JobReply::Processes(_)
            | JobReply::System(_) => {}
        }
        ToolOutcome::Consumed
    }

    fn snapshot(&self) -> Option<OpenTarget> {
        Some(OpenTarget::file(self.root.clone()))
    }

    fn caret(&self) -> Option<Caret> {
        let edit = self.edit.as_ref()?;
        if self.rows.get(self.cursor)?.heading {
            return None;
        }
        Some(Caret {
            col: LINE_WIDTH + LINE_GAP.len() + edit.col(),
            insert: edit.modal().is_typing(),
        })
    }

    /// Clipboard text goes into the line being edited, and nowhere else.
    fn on_paste(&mut self, text: String) -> ToolOutcome {
        if let Some(edit) = self.edit.as_mut() {
            edit.paste(&text);
        }
        ToolOutcome::Consumed
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

    fn context_items(&self) -> Vec<ToolMenuItem> {
        // Everything here acts on the match under the cursor, so a row that
        // is not one (a file heading, the header) offers only a new search.
        let mut items = Vec::new();
        if self.selected().is_some() {
            items.push(ToolMenuItem::new(Key::plain(KeyCode::Enter), LABEL_OPEN));
            items.push(ToolMenuItem::new(
                Key::with_ctrl(KeyCode::Char('o')),
                LABEL_OPEN_IN_EDITOR,
            ));
            items.push(ToolMenuItem::new(
                Key::plain(KeyCode::Char('y')),
                LABEL_COPY_MATCH,
            ));
        }
        items.push(ToolMenuItem::new(
            Key::plain(KeyCode::Char('/')),
            LABEL_SEARCH,
        ));
        items.push(ToolMenuItem::new(
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
        let mut tool = tool_with(vec![
            hit("a.rs", 1, "thing"),
            hit("a.rs", 9, "thing again"),
            hit("b.rs", 3, "thing"),
        ]);
        tool.on_key(&alt(KeyCode::Char('>')));
        assert_eq!(tool.cursor, tool.rows.len() - 1);
        tool.on_key(&alt(KeyCode::Char('<')));
        assert_eq!(tool.cursor, 0);
    }

    fn hit(path: &str, line: usize, text: &str) -> SearchHit {
        SearchHit {
            line,
            path: PathBuf::from("/repo").join(path),
            text: text.to_string(),
        }
    }

    /// A tool holding `hits`, as if a walk for "thing" had just answered.
    fn tool_with(hits: Vec<SearchHit>) -> GrepTool {
        let mut tool = GrepTool::new(PathBuf::from("/repo"));
        tool.query = "thing".to_string();
        tool.on_job(JobReply::Search(SearchResult {
            hits,
            query: "thing".to_string(),
            truncated: false,
        }));
        tool
    }

    #[test]
    fn test_the_start_directory_follows_the_hit_under_the_cursor() {
        // A tool opened from a result should land beside the file being read,
        // not at the top of whatever tree was searched, so stepping onto a
        // hit in another directory has to move the answer with it.
        let mut tool = tool_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("crates/deep/b.rs", 9, "thing"),
        ]);
        tool.move_by(1);
        assert_eq!(tool.cwd(), Some(PathBuf::from("/repo/src")));
        while tool.selected().map(|h| h.line) != Some(9) {
            tool.move_by(1);
        }
        assert_eq!(tool.cwd(), Some(PathBuf::from("/repo/crates/deep")));
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
        let mut tool = GrepTool::new(dir.clone());
        tool.query = "thing".to_string();
        tool.on_job(JobReply::Search(SearchResult {
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
        tool.on_key(&ctrl('x'));
        tool.on_key(&ctrl('q'));
        assert!(tool.edit.is_some(), "editing");
        tool.on_key(&press(KeyCode::Char('j')));
        for c in "ccstuff".chars() {
            tool.on_key(&press(KeyCode::Char(c)));
        }
        tool.on_key(&press(KeyCode::Escape));
        tool.move_by(-3);
        for c in "0cwnew".chars() {
            tool.on_key(&press(KeyCode::Char(c)));
        }
        tool.on_key(&press(KeyCode::Enter));

        assert!(tool.edit.is_none(), "Enter wrote and left editing");
        assert_eq!(
            std::fs::read_to_string(&kept).expect("read"),
            "one\r\nnew here  \r\nthree\r\n",
            "the line changed, its trailing blanks and the CRLF endings kept"
        );
        assert_eq!(std::fs::read_to_string(&moved).expect("read"), "other\n");
        let message = tool
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
        let tool = GrepTool::new(PathBuf::from("/repo"));
        assert_eq!(tool.cwd(), Some(PathBuf::from("/repo")));
    }

    /// Ask for `text` through the query prompt, the way a key does.
    fn ask(tool: &mut GrepTool, text: &str) -> ToolOutcome {
        let outcome = tool.on_key(&press(KeyCode::Char('/')));
        let ToolOutcome::Prompt(request) = outcome else {
            panic!("expected a prompt, got {outcome:?}");
        };
        assert_eq!(request.tag, ASK_QUERY);
        tool.on_prompt(PromptReply {
            answer: Some(text.to_string()),
            tag: request.tag,
        })
    }

    fn row_texts(tool: &GrepTool) -> Vec<String> {
        tool.rows
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
        let mut tool = GrepTool::new(PathBuf::from("/repo"));
        let outcome = ask(&mut tool, "needle");
        assert_eq!(
            outcome,
            ToolOutcome::Job(JobRequest::Search(SearchRequest {
                query: "needle".to_string(),
                root: PathBuf::from("/repo"),
            }))
        );
        assert!(tool.searching, "the header has to be able to say so");
    }

    #[test]
    fn test_an_empty_query_searches_for_nothing() {
        // A walk for "" would report every line of every file under the root.
        let mut tool = GrepTool::new(PathBuf::from("/repo"));
        assert_eq!(ask(&mut tool, ""), ToolOutcome::Consumed);
        assert!(!tool.searching);
    }

    #[test]
    fn test_matches_are_grouped_under_one_heading_per_file() {
        let tool = tool_with(vec![
            hit("src/a.rs", 4, "one thing"),
            hit("src/a.rs", 9, "another thing"),
            hit("src/b.rs", 2, "thing again"),
        ]);
        let texts = row_texts(&tool);
        assert_eq!(texts.len(), 5, "two headings and three matches: {texts:?}");
        assert_eq!(texts[0].trim(), "src/a.rs");
        assert_eq!(texts[1].trim(), "4: one thing");
        assert_eq!(texts[3].trim(), "src/b.rs");
    }

    #[test]
    fn test_enter_opens_the_file_at_the_line_that_matched() {
        let mut tool = tool_with(vec![hit("src/a.rs", 4, "one thing")]);
        // Row 0 is the heading, row 1 the match.
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 4))
        );
    }

    #[test]
    fn test_enter_on_a_heading_opens_that_file_at_its_first_match() {
        // Opening it at line one would throw away what the user searched for.
        let mut tool = tool_with(vec![
            hit("src/a.rs", 12, "one thing"),
            hit("src/a.rs", 30, "another thing"),
        ]);
        assert_eq!(
            tool.on_key(&press(KeyCode::Enter)),
            ToolOutcome::OpenPath(OpenTarget::at_line(PathBuf::from("/repo/src/a.rs"), 12))
        );
    }

    #[test]
    fn test_the_file_motions_step_over_a_file_rather_than_through_it() {
        let mut tool = tool_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("src/a.rs", 2, "thing"),
            hit("src/b.rs", 3, "thing"),
        ]);
        tool.on_key(&alt(KeyCode::Char('n')));
        assert_eq!(row_texts(&tool)[tool.cursor].trim(), "src/b.rs");

        tool.on_key(&alt(KeyCode::Char('p')));
        assert_eq!(row_texts(&tool)[tool.cursor].trim(), "src/a.rs");
    }

    #[test]
    fn test_the_match_motions_skip_the_names_between_the_matches() {
        let mut tool = tool_with(vec![
            hit("src/a.rs", 1, "thing"),
            hit("src/b.rs", 3, "thing"),
        ]);
        // Rows: heading, match, heading, match. The cursor starts on the first
        // heading, so one step lands on a match and the next skips a heading.
        tool.on_key(&press(KeyCode::Char('n')));
        assert_eq!(row_texts(&tool)[tool.cursor].trim(), "1: thing");

        tool.on_key(&press(KeyCode::Char('n')));
        assert_eq!(row_texts(&tool)[tool.cursor].trim(), "3: thing");

        tool.on_key(&press(KeyCode::Char('N')));
        assert_eq!(row_texts(&tool)[tool.cursor].trim(), "1: thing");
    }

    #[test]
    fn test_an_answer_to_a_query_already_replaced_is_dropped() {
        // Two walks can be out at once, and the slower one would otherwise
        // paint its matches under the newer query's name.
        let mut tool = tool_with(vec![hit("src/a.rs", 1, "thing")]);
        tool.query = "other".to_string();

        tool.on_job(JobReply::Search(SearchResult {
            hits: vec![hit("src/b.rs", 9, "thing")],
            query: "thing".to_string(),
            truncated: false,
        }));
        assert_eq!(row_texts(&tool).len(), 2, "the older answer was ignored");
        assert_eq!(row_texts(&tool)[1].trim(), "1: thing");
    }

    #[test]
    fn test_a_walk_that_found_nothing_says_so() {
        let mut tool = tool_with(Vec::new());
        assert_eq!(
            tool.take_notice().map(|notice| notice.text).as_deref(),
            Some("no matches for thing")
        );
        assert!(tool.rows.is_empty());
    }

    #[test]
    fn test_a_capped_walk_says_the_list_is_partial() {
        // Without this the first five hundred matches look like all of them.
        let mut tool = GrepTool::new(PathBuf::from("/repo"));
        tool.query = "thing".to_string();
        tool.on_job(JobReply::Search(SearchResult {
            hits: vec![hit("src/a.rs", 1, "thing")],
            query: "thing".to_string(),
            truncated: true,
        }));
        let header: String = tool
            .header_row()
            .iter()
            .map(|span| span.text.as_str())
            .collect();
        assert!(header.contains("capped"), "header reads: {header}");
    }

    #[test]
    fn test_a_new_query_drops_what_the_last_one_found() {
        // Results half from one query and half from another answer neither.
        let mut tool = tool_with(vec![hit("src/a.rs", 1, "thing")]);
        ask(&mut tool, "other");
        assert!(tool.rows.is_empty());
        assert_eq!(tool.query, "other");
    }

    #[test]
    fn test_escape_stops_a_running_walk_and_never_closes_the_tool() {
        let mut tool = GrepTool::new(PathBuf::from("/repo"));
        ask(&mut tool, "needle");
        assert_eq!(
            tool.on_key(&press(KeyCode::Escape)),
            ToolOutcome::CancelJobs
        );
        assert_ne!(tool.on_key(&press(KeyCode::Escape)), ToolOutcome::Close);
    }

    #[test]
    fn test_yanking_copies_where_the_match_is() {
        let mut tool = tool_with(vec![hit("src/a.rs", 4, "one thing")]);
        tool.on_key(&press(KeyCode::Char('j')));
        assert_eq!(
            tool.on_key(&press(KeyCode::Char('y'))),
            ToolOutcome::Yank("src/a.rs:4".to_string())
        );
    }
}

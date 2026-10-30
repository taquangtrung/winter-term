//! The PDF viewer as an editor tab: what the window asks of every editor, and
//! what it asks of this one alone.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use crate::editors::background::POLL;
use crate::model::editor::{
    menu_groups, Editor, EditorHint, EditorMenuItem, EditorOutcome, EditorStatus,
    JobReply, Notice, OpenTarget, PdfMark, Popups, PromptReply, SearchReport, Toolbar,
};
use crate::model::input::table::{KeyHelp, TableMatch};
use crate::model::input::{table, Key, KeyCode, Mods};
use crate::model::paint::{Frame, IconKind, Point, Row};
use crate::model::path::abbreviate_home;
use crate::model::picture::{Content, Tile};
use crate::model::textedit::search::Search;

use super::caret::{Effect, Operator};
use super::keys::{COMMON_KEYS, CURSOR_KEYS, WINDOW_KEYS};
use crate::editors::synctex::Source;
use super::toolbar::Button;
use super::{caret, open_source, toolbar, Act, BackSync, Command, Mode, PdfEditor, PdfView, Pending, Prefix, ASK_PAGE, ASK_SEARCH, ASK_SEARCH_BACK, ASK_ZOOM, GLIDE_FRAME, MENU, MENU_COPY, TOOLBAR_ROWS, WATCH_INTERVAL};
use crate::editors::synctex;

// ========================================================================
// PdfEditor: what only the window asks of it
// ========================================================================

impl PdfEditor {
    /// Bring `mark` into view and mark it, for an editor showing a PDF.
    /// Editors showing anything else never implement it.
    pub(crate) fn show_mark(&mut self, mark: PdfMark) {
        self.waiting_mark = Some(mark);
        self.reveal_waiting_mark();
    }
}

impl Editor for PdfEditor {
    fn context_items(&self) -> Vec<EditorMenuItem> {
        let has_outline = self
            .document
            .as_ref()
            .is_some_and(|doc| !doc.outline().is_empty());
        let entry = |act: Act, label: &str| {
            [COMMON_KEYS, CURSOR_KEYS, WINDOW_KEYS]
                .into_iter()
                .find_map(|keys| table::menu_entry(keys, act))
                .map(|(key, _)| EditorMenuItem::new(key, label))
        };
        let copy = match self.caret.selection() {
            Some(_) => entry(Act::Yank, MENU_COPY).into_iter().collect(),
            None => Vec::new(),
        };
        let has_synctex = synctex::has_data(&self.path);
        let rest = MENU.iter().map(|group| {
            group
                .iter()
                .filter(|(act, _)| *act != Act::Outline || has_outline)
                .filter(|(act, _)| *act != Act::SyncBack || has_synctex)
                .filter_map(|&(act, label)| entry(act, label))
                .collect()
        });
        menu_groups(std::iter::once(copy).chain(rest))
    }

    /// A right-click moves nothing and presses no toolbar button: the
    /// selection stays for the menu's Copy, and the menu's entries act on
    /// the document rather than on a spot in it, save SyncTeX Backward,
    /// which goes back from the character the menu was opened over.
    fn on_context_mouse(&mut self, at: Point) -> EditorOutcome {
        self.menu_spot = self.char_under(at);
        EditorOutcome::Consumed
    }

    /// A double-click on the pages goes back to the LaTeX source there.
    fn on_double_click(&mut self, at: Point) -> EditorOutcome {
        match self.char_under(at) {
            Some(pos) => self.sync_back(Some(pos)),
            None => EditorOutcome::Ignored,
        }
    }

    fn on_job(&mut self, reply: JobReply) -> EditorOutcome {
        match (self.sync.take(), reply) {
            (Some(BackSync::Locating(context)), JobReply::Command(output)) => {
                self.sync_located(context, output)
            }
            (Some(BackSync::Reading(context, source)), JobReply::Files(files)) => {
                self.sync_read(context, source, files)
            }
            (Some(BackSync::Including(context, source)), JobReply::Files(files)) => {
                let found = files.into_iter().find_map(|(path, content)| {
                    synctex::find_in(&content, &context, None).map(|line| Source { line, path })
                });
                open_source(found.unwrap_or(source))
            }
            _ => EditorOutcome::Consumed,
        }
    }

    fn title(&self) -> String {
        self.file_name()
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::File {
            name: self.file_name(),
        })
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        // A blank row for the toolbar the window draws over it (see
        // `toolbar`); the pages the host draws cover every row below, and the
        // outline, while it is open, drops down over them (see `drop_list`).
        self.grid = (rows, cols);
        self.outline_drop = self.outline_list(rows, cols);
        Frame::new(vec![Row::new()])
    }

    fn header_rows(&self) -> usize {
        TOOLBAR_ROWS
    }

    fn popups(&self) -> Popups {
        Popups {
            drop_list: self.outline.as_ref().and(self.outline_drop.clone()),
            ..Popups::default()
        }
    }

    fn toolbar(&self) -> Option<Toolbar> {
        Some(toolbar::bar(&self.bar_state(), TOOLBAR_ROWS))
    }

    fn on_toolbar(&mut self, button: u16) -> EditorOutcome {
        match Button::from_id(button) {
            Some(button) => self.press(button),
            None => EditorOutcome::Ignored,
        }
    }

    fn toolbar_tooltip(&self, button: u16) -> Option<String> {
        Button::from_id(button).map(|b| b.tooltip().to_string())
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    /// What was looked for, how many times the document holds it, and
    /// which of those the cursor is on.
    fn take_search(&mut self) -> Option<SearchReport> {
        if !std::mem::take(&mut self.searched) {
            return None;
        }
        let search = self.search.as_ref()?;
        let at = self.caret.found.map(|found| found.at);
        let (index, total) = match self.document.as_mut() {
            Some(doc) => caret::count_matches(doc, &search.pattern, at),
            None => (0, 0),
        };
        Some(SearchReport {
            index,
            query: search.pattern.clone(),
            reverse: !search.forward,
            total,
        })
    }

    /// The page, the cursor's line once there is a cursor, and what has been
    /// typed towards a command, the way vim shows a pending count.
    fn status(&self) -> EditorStatus {
        let mut segments = Vec::new();
        let page = self.page_label();
        if !page.is_empty() {
            segments.push(page);
        }
        if let Some(pos) = self.caret.shown() {
            segments.push(format!("line {}", pos.line + 1));
        }
        let typed = self.typed();
        if !typed.is_empty() {
            segments.push(typed);
        }
        segments.extend(self.hovered_uri().map(str::to_string));
        EditorStatus {
            mode: self.edit_mode(),
            segments,
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        // The space bar may come as the character it types.
        let key = &match key.code { // key-exact: rewrites the code alone, keeping every modifier
            KeyCode::Char(' ') => Key {
                code: KeyCode::Space,
                ..*key
            },
            _ => *key,
        };
        // The spot a right-click opened the menu over lasts for the one key
        // the menu sends, and no longer.
        let menu_spot = self.menu_spot.take();
        self.mark = None;
        if self.hints.is_some() {
            return self.on_hint_key(key);
        }
        // A preview is followed with Enter, put away with Escape, and put
        // away by any other key, which then goes on to mean what it means.
        if let Some(preview) = self.preview.take() {
            match key.chord() {
                (Mods::NONE, KeyCode::Enter) => return self.follow(preview.link),
                (Mods::NONE, KeyCode::Escape) => return EditorOutcome::Consumed,
                _ => {}
            }
        }
        if self.outline.is_some() {
            return self.on_outline_key(key);
        }
        // A prefix owns the key after it: the `t` of `zt` is not the till
        // find, and the `g` of `gg` is not a fresh prefix.
        match self.pending.prefix {
            Prefix::Find(kind) => return self.on_find_key(kind, key),
            Prefix::Keys(first) => return self.on_sequence_key(first, key),
            Prefix::None => {}
        }
        // A count is typed ahead of the motion it multiplies.
        if let (Mods::NONE, KeyCode::Char(digit)) = key.chord() {
            if self.take_count_digit(digit) {
                return EditorOutcome::Consumed;
            }
        }
        match table::lookup(COMMON_KEYS, std::slice::from_ref(key)) {
            TableMatch::Exact(act) => return self.on_key_act(act, menu_spot),
            TableMatch::Prefix => {
                self.pending.prefix = Prefix::Keys(*key);
                return EditorOutcome::Consumed;
            }
            TableMatch::Miss => {}
        }
        // Past the jumps' browser keys, the Alt chords are the window's: no
        // mode table binds one, so each misses below and is ignored.
        let act = self
            .mode_tables()
            .into_iter()
            .find_map(|keys| table::find(keys, key));
        match act {
            Some(act) => self.on_key_act(act, menu_spot),
            None => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(COMMON_KEYS);
        for keys in self.mode_tables() {
            rows.extend(table::help(keys));
        }
        rows
    }

    fn hint(&self) -> Option<EditorHint> {
        let items = match self.pending.prefix {
            Prefix::Find(_) => vec![(
                "{char}".to_string(),
                "the character on this line to move to".to_string(),
            )],
            Prefix::Keys(first) => table::continuations(COMMON_KEYS, &[first]),
            Prefix::None => return None,
        };
        Some(EditorHint {
            items,
            menu: false,
            title: self.typed(),
        })
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        let (rows, cols) = self.grid;
        if rows == 0 || cols == 0 {
            return EditorOutcome::Ignored;
        }
        if let Some(outcome) = self.on_chrome_mouse(at) {
            return outcome;
        }
        self.mark = None;
        self.hints = None;
        self.hover = None;
        self.preview = None;
        if self.thumbs_width() > 0.0 && (at.col as f32 + 0.5) * self.cell.0 < self.thumbs_width() {
            if let (false, Some(page)) = (at.drag, self.thumb_at(at)) {
                return self.act(Command::GoToPage(page + 1));
            }
            return EditorOutcome::Consumed;
        }
        let (x, y) = self.page_point(at);
        if !at.drag {
            if let Some((_, link)) = self.link_at(x, y) {
                return self.follow(link);
            }
        }
        let Some(pos) = self.char_at(x, y) else {
            return EditorOutcome::Consumed;
        };
        // With no cursor in the text a click only lets go of a selection:
        // the cursor is asked for with `i`, never put down by the pointer.
        // A drag from it still selects, and copies what it selects.
        let no_cursor = self.mode() == Mode::Normal;
        match (at.drag, no_cursor) {
            (true, _) => self.caret.point(pos, self.drag_origin),
            (false, true) => {
                self.drag_origin = Some(pos);
                self.caret.leave();
            }
            (false, false) => {
                self.drag_origin = Some(pos);
                self.caret.point(pos, None);
            }
        }
        // What a drag selects is copied as it grows, the way a terminal's
        // selection is: the host hears of no release to wait for.
        if !at.drag {
            return EditorOutcome::Consumed;
        }
        let selected = self
            .document
            .as_mut()
            .and_then(|doc| self.caret.selected_text(doc));
        match selected {
            Some(text) if !text.is_empty() => EditorOutcome::copied(text),
            _ => EditorOutcome::Consumed,
        }
    }

    fn scrolls_smoothly(&self) -> bool {
        true
    }

    fn on_scroll_pixels(&mut self, pixels: f32) -> EditorOutcome {
        self.hover_link(None);
        self.scroll_pixels(pixels);
        EditorOutcome::Consumed
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        // The link under the pointer slides away from it.
        self.hover_link(None);
        // The wheel reports rows the way the grid counts them, positive
        // upwards, which is the opposite of how far the pages move.
        self.execute(Command::ScrollLines(-lines))
    }

    fn on_side_button(&mut self, back: bool) -> EditorOutcome {
        self.jump(back)
    }

    fn on_hover(&mut self, at: Option<Point>) -> EditorOutcome {
        // Over the toolbar or the open outline, no link is under it.
        let link = at.filter(|at| at.row >= self.header_rows()).and_then(|at| {
            let (x, y) = self.page_point(at);
            self.link_at(x, y)
        });
        self.hover_link(link)
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        match (reply.tag, reply.answer) {
            (ASK_PAGE, answer) => match answer.and_then(|page| page.trim().parse::<usize>().ok()) {
                Some(page) => self.act(Command::GoToPage(page)),
                None => EditorOutcome::Consumed,
            },
            (ASK_ZOOM, answer) => match answer.as_deref().and_then(Self::parse_zoom) {
                Some(zoom) => self.act(Command::SetZoom(zoom)),
                None => EditorOutcome::Consumed,
            },
            (ASK_SEARCH, Some(pattern)) => self.start_search(pattern, true),
            (ASK_SEARCH_BACK, Some(pattern)) => self.start_search(pattern, false),
            _ => {
                // A prompt escaped out of leaves the cursor and the last
                // search as they were, and takes the half-typed command
                // with it.
                self.back_to_search_origin();
                if let Some((_, search)) = self.search_origin.take() {
                    self.search = search;
                }
                self.pending = Pending::default();
                EditorOutcome::Consumed
            }
        }
    }

    /// Each letter typed at the prompt searches again from where it opened,
    /// so the match follows the typing. Not under an operator: `y/` copies
    /// once, when the answer is in.
    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        let forward = tag == ASK_SEARCH;
        if !(forward || tag == ASK_SEARCH_BACK) || self.pending.operator != Operator::Move {
            return EditorOutcome::Consumed;
        }
        self.back_to_search_origin();
        if query.is_empty() {
            return EditorOutcome::Consumed;
        }
        let from = self.screen_line(0.0);
        if let Some(doc) = self.document.as_mut() {
            let found = self
                .caret
                .search(doc, query, forward, 1, Operator::Move, from);
            if found == Effect::Moved {
                self.scroll_caret_into_view();
            }
        }
        // Shown in the bar as it is typed, with how many there are.
        self.search = Some(Search::new(query.to_string(), forward));
        self.searched = true;
        EditorOutcome::Consumed
    }

    /// Soon while a drawing or a reread is on its way, and otherwise in
    /// time for the next look at the file for a rebuild.
    fn set_shown(&mut self, shown: bool) {
        self.hidden = !shown;
    }

    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(PdfView {
            caret: self.caret.clone(),
            cell: self.cell,
            grid: self.grid,
            jumps_ahead: self.jumps_ahead.clone(),
            jumps_back: self.jumps_back.clone(),
            outer: self.outer,
            pane: self.pane,
            scroll: self.scroll,
            zoom: self.zoom,
        }))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        let Ok(view) = state.downcast::<PdfView>() else {
            return;
        };
        self.caret = view.caret;
        self.cell = view.cell;
        self.grid = view.grid;
        self.jumps_ahead = view.jumps_ahead;
        self.jumps_back = view.jumps_back;
        self.outer = view.outer;
        self.pane = view.pane;
        self.scroll = view.scroll;
        self.zoom = view.zoom;
        self.request_visible();
    }

    fn shed_caches(&mut self) {
        self.pages.clear();
        self.wanted_at.clear();
        self.requested.clear();
        if let Some(thumbnails) = self.thumbnails.as_mut() {
            thumbnails.drawn.clear();
            thumbnails.requested.clear();
        }
    }

    fn next_tick(&self) -> Option<Instant> {
        if self.hidden {
            return None;
        }
        let now = Instant::now();
        let waiting = self.document.is_none() || !self.requested.is_empty();
        let busy = (self.worker.is_some() && waiting) || self.reload.is_some();
        let next = match (self.glide.is_some(), busy) {
            (true, _) => now + GLIDE_FRAME,
            (false, true) => now + POLL,
            (false, false) => now + WATCH_INTERVAL,
        };
        let hover = self.hover.as_ref().and_then(|hover| hover.due);
        Some(hover.map_or(next, |due| due.min(next)))
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        self.step_glide(now);
        self.show_hover(now);
        self.take_replies();
        self.fill_preview();
        self.take_reload();
        self.watch_disk();
        EditorOutcome::Consumed
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn address(&self) -> Option<String> {
        Some(abbreviate_home(&self.path))
    }

    fn file_reference(&self) -> Option<String> {
        let base = abbreviate_home(&self.path);
        match self.page_count() {
            0 => Some(base),
            _ => Some(format!("{base}:{}", self.current_page() + 1)),
        }
    }

    /// The document, at the page being read, so a restart brings the tab
    /// back where it was.
    fn snapshot(&self) -> Option<OpenTarget> {
        let page = match self.page_count() {
            0 => self.resume_page.unwrap_or(1),
            _ => self.current_page() + 1,
        };
        Some(OpenTarget::at_line(self.path.clone(), page))
    }

    fn tiles(&self) -> Vec<Tile> {
        let Some(column) = self.column() else {
            return Vec::new();
        };
        let scale = self.scale();
        let mut tiles: Vec<Tile> = column
            .visible(self.scroll.1, self.pane.1)
            .map(|page| {
                let rect = column.pages[page];
                let blank = match self.inverted {
                    true => &self.overlays.blank_inverted,
                    false => &self.overlays.blank,
                };
                let crop = self.crop();
                let (key, pixels) = match self.pages.get(&page) {
                    Some(drawn) => match (&drawn.cropped, crop) {
                        (Some((cut, key, pixels)), Some(crop)) if *cut == crop => {
                            (*key, Arc::clone(pixels))
                        }
                        _ => (drawn.key, Arc::clone(&drawn.pixels)),
                    },
                    None => (blank.key, Arc::clone(&blank.pixels)),
                };
                // A page drawn at the scale it is shown at is shown at its
                // own pixel size, one pixel for one: the column's size is a
                // fraction of a pixel off it, and stretching across that
                // blurs every line of text.
                let (width, height) = match self.pages.get(&page) {
                    Some(drawn) if drawn.scale == scale && !drawn.stale => {
                        (pixels.width as f32, pixels.height as f32)
                    }
                    _ => (rect.width, rect.height),
                };
                Tile {
                    content: Content::Pixels(pixels),
                    height,
                    key,
                    width,
                    x: rect.x - self.scroll.0,
                    y: rect.y - self.scroll.1,
                }
            })
            .collect();
        tiles.extend(self.sentence_tiles(&column));
        tiles.extend(self.spelling_tiles(&column));
        tiles.extend(self.link_box_tiles(&column));
        tiles.extend(self.overlay_tiles(&column));
        tiles.extend(self.link_tiles(&column));
        // Laid out in the pane below the toolbar and right of the
        // thumbnails, and placed in the whole.
        let left = self.thumbs_width();
        for tile in &mut tiles {
            tile.x += left;
        }
        tiles.extend(self.thumb_tiles());
        let top = self.toolbar_height();
        for tile in &mut tiles {
            tile.y += top;
        }
        tiles
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        // The pages are laid out below the toolbar, measured in the cells the
        // last paint counted, and right of the thumbnails.
        self.outer = pane;
        let (rows, cols) = self.grid;
        if rows > 0 && cols > 0 {
            self.cell = (pane.0 / cols as f32, pane.1 / rows as f32);
        }
        let pane = (
            (pane.0 - self.thumbs_width()).max(0.0),
            (pane.1 - self.toolbar_height()).max(0.0),
        );
        if pane != self.pane {
            // A resize keeps the reader on the same spot of the same page, as
            // a zoom does: fitted to the width, it is one.
            let zoom = self.zoom;
            let resized = self.pane != (0.0, 0.0);
            self.pane = pane;
            match resized {
                true => self.set_zoom(zoom),
                false => self.request_visible(),
            }
            self.reveal_waiting_mark();
        }
        self.read_visible();
    }

    /// The document it shows, turned to the page `target` names as its
    /// line, the way a tab restored after a restart is.
    fn open_file(&mut self, target: OpenTarget) -> bool {
        if target.path != self.path {
            return false;
        }
        if let Some(page) = target.line.filter(|page| *page >= 1) {
            match self.document.is_some() {
                true => self.go_to_page(page - 1),
                false => self.resume_page = Some(page),
            }
        }
        true
    }

    fn holds_file(&self, path: &Path) -> bool {
        self.path == path || self.path.canonicalize().ok() == path.canonicalize().ok()
    }

    fn run_command(&mut self, name: &str) -> Option<EditorOutcome> {
        match name {
            crate::model::editor::SYNCTEX_BACKWARD => Some(self.sync_back(None)),
            _ => None,
        }
    }
}

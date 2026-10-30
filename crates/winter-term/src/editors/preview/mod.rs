//! Preview: a Markdown document as it reads rendered, beside the file being
//! written. The document is written out as HTML, laid out and painted by a
//! web engine, and drawn on the GPU in tiles a screen tall, so scrolling
//! moves tiles rather than painting anew. While a text editor has the file
//! open the preview follows its text as it is typed, unsaved, and keeps the
//! cursor's line in view; with none it reads the file from disk, again
//! whenever the file changes.
//!
//! - [`page`]: the document as HTML, each block naming its source line.
//! - [`engine`]: the web engine that lays the page out and paints it.

mod engine;
mod page;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use crate::model::editor::{
    find_match, Appearance, Editor, EditorOutcome, EditorStatus, Notice, OpenTarget,
    PromptReply, PromptRequest, SharedText,
};
use crate::model::input::table::{self, Binding, KeyHelp};
use crate::model::input::Key;
use crate::model::paint::{Frame, IconKind, Point, Row, Span, Style};
use crate::model::picture::{Content, SizedScene, Tile};
use crate::model::vim::motion::CursorMove;
use crate::model::vim::nav::{VimKey, VimNav};

use engine::WebPage;

// ========================================================================
// Constants
// ========================================================================

/// The one question the preview asks.
const ASK_SEARCH: &str = "preview-search";

/// How often the file is looked at on disk while no editor has it open.
const DISK_CHECK: Duration = Duration::from_secs(1);

/// What the preview says of a file it cannot read.
const EMPTY: &str = "Nothing to show";

/// How long a layout may take before typing waits to be laid out: one frame.
const SLOW_LAYOUT: Duration = Duration::from_millis(16);

/// How long typing must pause before a page slow to lay out (one with large
/// images, decoded anew each time) is laid out again, so it never holds
/// back the keys.
const SETTLE: Duration = Duration::from_millis(100);

/// The tallest tile painted, in pixels: a pane taller than this is covered
/// by more of them, so no texture outgrows what every GPU holds.
const MAX_TILE_HEIGHT: f32 = 2048.0;

/// How far down the pane a block brought into view lands, as a fraction of
/// its height: near the top, with what leads up to it still showing.
const REVEAL_AT: f32 = 0.25;

/// How many bits of a tile's key count the tile; those above count the
/// layout it was painted from.
const TILE_BITS: u32 = 20;

/// The preview's keys, ahead of the motions that scroll it.
const KEYS: &[Binding<Act>] = &[
    Binding {
        action: Act::Follow,
        keys: &["f"],
        label: "Follow the Cursor of the File Being Written",
    },
    Binding {
        action: Act::Source,
        keys: &["RET", "o"],
        label: "Go to the Top Block in the Source",
    },
    Binding {
        action: Act::Search,
        keys: &["/"],
        label: "Find",
    },
    Binding {
        action: Act::SearchStep(true),
        keys: &["n"],
        label: "Next Match",
    },
    Binding {
        action: Act::SearchStep(false),
        keys: &["N"],
        label: "Previous Match",
    },
    Binding {
        action: Act::Unselect,
        keys: &["ESC"],
        label: "Let Go of the Selection",
    },
];

// ========================================================================
// Data Structures
// ========================================================================

/// A Markdown document, rendered, and where it is read from.
pub struct PreviewEditor {
    /// When the file is next looked at on disk.
    check_at: Instant,
    /// Whether the page is laid out in a dark color scheme.
    dark: bool,
    /// When an editor last handed the text over: while one does, the disk
    /// is left alone.
    fed: bool,
    /// Whether the view keeps the source cursor's line in view.
    follow: bool,
    /// Counts the layouts the page has had, so a tile painted from an old
    /// one is never taken for a new one.
    generation: u64,
    /// The rows and columns the pane last held, which size a row of
    /// scrolling and place a click.
    grid: (usize, usize),
    /// Whether no tab shows the preview, its buffer buried: the disk is not
    /// looked at until one does.
    hidden: bool,
    /// The page as HTML, written again whenever the text or the window's
    /// look changes; `None` until the look is known.
    html: Option<String>,
    /// How long the page last took to lay out.
    laid_out_in: Duration,
    /// The Markdown last read or handed over.
    markdown: String,
    nav: VimNav,
    notice: Option<Notice>,
    /// The page laid out, `None` until there is HTML and a pane to lay it
    /// out in, and again after either changes.
    page: Option<WebPage>,
    /// The tiles painted for the current layout, by number down the page.
    painted: Vec<(usize, Arc<SizedScene>)>,
    /// The pane's size in pixels.
    pane: (f32, f32),
    path: PathBuf,
    /// Whether the source cursor's line is to be brought into view once the
    /// page is laid out.
    reveal: bool,
    /// The revision of the text an editor last handed over.
    revision: Option<usize>,
    /// Screen pixels to a CSS pixel.
    scale: f32,
    /// How far down the page the pane's top is, in pixels.
    scroll: f32,
    /// What was last searched for.
    search: Option<String>,
    /// When text changed since the HTML was written is written out, for a
    /// page slow to lay out: once typing pauses. `None` for one quick to lay
    /// out, written out at the next paint.
    settle_at: Option<Instant>,
    /// The fill a text selection is painted in: the theme's.
    selection: (u8, u8, u8),
    /// The source cursor's line, as last handed over.
    source_cursor: Option<usize>,
    /// When the file read from disk was last written.
    stamp: Option<SystemTime>,
    /// Whether the Markdown changed since the HTML was written.
    stale: bool,
    /// The stylesheet the HTML was written with, which changes with the
    /// theme.
    stylesheet: String,
    /// What is drawn this frame.
    tiles: Vec<Tile>,
}

/// What a key of the preview's own does.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Act {
    Follow,
    Search,
    SearchStep(bool),
    Source,
    Unselect,
}

// ========================================================================
// PreviewEditor
// ========================================================================

impl PreviewEditor {
    /// A preview of the Markdown file at `path`, read from disk until an
    /// editor holding it hands its text over.
    pub fn new(path: PathBuf) -> Self {
        let mut preview = Self {
            check_at: Instant::now(),
            dark: true,
            fed: false,
            follow: true,
            generation: 0,
            grid: (0, 0),
            hidden: false,
            html: None,
            laid_out_in: Duration::ZERO,
            markdown: String::new(),
            nav: VimNav::new(),
            notice: None,
            page: None,
            painted: Vec::new(),
            pane: (0.0, 0.0),
            path,
            reveal: false,
            revision: None,
            scale: 1.0,
            scroll: 0.0,
            search: None,
            selection: (0, 0, 0),
            settle_at: None,
            source_cursor: None,
            stamp: None,
            stale: false,
            stylesheet: String::new(),
            tiles: Vec::new(),
        };
        preview.read_disk();
        preview
    }

    /// Whether `path` is a Markdown file a preview shows.
    pub fn handles(path: &Path) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown" | "mdx"))
    }

    /// Read the file from disk where it changed since last read.
    fn read_disk(&mut self) {
        let stamp = std::fs::metadata(&self.path).and_then(|meta| meta.modified()).ok();
        if stamp.is_some() && stamp == self.stamp {
            return;
        }
        self.stamp = stamp;
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        self.set_text(text);
    }

    /// Take `text` as the document, to be written out again before the
    /// next paint.
    fn set_text(&mut self, text: String) {
        if text == self.markdown {
            return;
        }
        self.markdown = text;
        self.stale = true;
        self.reveal = self.follow;
        self.settle_at = (self.laid_out_in > SLOW_LAYOUT).then(|| Instant::now() + SETTLE);
    }

    /// Drop the layout, so the next paint lays the page out afresh.
    fn forget_layout(&mut self) {
        self.page = None;
        self.forget_tiles();
    }

    /// Drop the painted tiles, so the next paint paints them afresh.
    fn forget_tiles(&mut self) {
        self.painted.clear();
        self.generation += 1;
    }

    /// The pixels a row of the pane is tall: what a line of scrolling moves.
    fn row_height(&self) -> f32 {
        match self.grid.0 {
            0 => 0.0,
            rows => self.pane.1 / rows as f32,
        }
    }

    /// The point in the page a click on pane cell `at` lands on.
    fn page_point(&self, at: Point) -> (f32, f32) {
        let width = match self.grid.1 {
            0 => 0.0,
            cols => self.pane.0 / cols as f32,
        };
        let height = self.row_height();
        ((at.col as f32 + 0.5) * width, (at.row as f32 + 0.5) * height + self.scroll)
    }

    /// The farthest the pane's top goes down the page.
    fn max_scroll(&self) -> f32 {
        self.page.as_ref().map_or(0.0, |page| (page.height() - self.pane.1).max(0.0))
    }

    /// Scroll the pane's top to `scroll`, kept on the page.
    fn scroll_to(&mut self, scroll: f32) {
        self.scroll = scroll.clamp(0.0, self.max_scroll()).round();
    }

    /// Bring the block at `top` into view, a little below the pane's top,
    /// unless it already shows above the pane's lower part.
    fn bring_into_view(&mut self, top: f32) {
        let shows = top >= self.scroll && top <= self.scroll + self.pane.1 * (1.0 - REVEAL_AT);
        if !shows {
            self.scroll_to(top - self.pane.1 * REVEAL_AT);
        }
    }

    /// Bring the block holding the source cursor's line into view, while
    /// following.
    fn follow_source(&mut self) {
        let line = self.source_cursor.filter(|_| self.follow);
        let top = line.and_then(|line| self.page.as_ref()?.line_top(line));
        if let Some(top) = top {
            self.bring_into_view(top);
        }
    }

    /// The source line of the block at the pane's top.
    fn top_line(&self) -> usize {
        self.page
            .as_ref()
            .and_then(|page| page.line_at(self.scroll + 1.0))
            .unwrap_or(0)
    }

    /// Lay the page out where it is not, paint the tiles the pane shows that
    /// are not painted yet, and place them.
    fn paint_visible(&mut self) {
        if self.page.is_none() {
            let Some(html) = self.html.as_deref() else {
                return;
            };
            let size = pixel_size(self.pane);
            let base = url::Url::from_file_path(&self.path).ok().map(String::from);
            let started = Instant::now();
            self.page = Some(WebPage::new(html, base, size, self.scale, self.dark));
            self.laid_out_in = started.elapsed();
            self.forget_tiles();
        }
        if std::mem::take(&mut self.reveal) {
            self.follow_source();
        }
        self.scroll_to(self.scroll);
        let width = self.pane.0.round().max(1.0);
        let tile = self.pane.1.ceil().clamp(1.0, MAX_TILE_HEIGHT);
        let first = (self.scroll / tile).floor() as usize;
        let last = ((self.scroll + self.pane.1) / tile).floor() as usize;
        self.painted.retain(|(index, _)| (first..=last).contains(index));
        let Some(page) = self.page.as_mut() else {
            return;
        };
        let mut tiles = Vec::new();
        for index in first..=last {
            let top = index as f32 * tile;
            if top >= page.height() {
                break;
            }
            let scene = match self.painted.iter().find(|(painted, _)| *painted == index) {
                Some((_, scene)) => Arc::clone(scene),
                None => {
                    let scene = Arc::new(page.paint(top, (width as u32, tile as u32), self.selection));
                    self.painted.push((index, Arc::clone(&scene)));
                    scene
                }
            };
            tiles.push(Tile {
                content: Content::Scene(scene),
                height: tile,
                key: (self.generation << TILE_BITS) | index as u64,
                width,
                x: 0.0,
                y: top - self.scroll,
            });
        }
        self.tiles = tiles;
    }

    /// Bring the next (or, not `forward`, the previous) source line holding
    /// `query` into view, counting from the block at the pane's top.
    fn find(&mut self, query: &str, forward: bool) {
        let lines: Vec<&str> = self.markdown.lines().collect();
        let found = find_match(&lines, query, self.top_line(), forward);
        let top = found.and_then(|line| self.page.as_ref()?.line_top(line));
        match top {
            Some(top) => self.scroll_to(top - self.pane.1 * REVEAL_AT),
            None => self.notice = Some(Notice::info(format!("no match for {query}")).unlogged()),
        }
    }

    /// Where a click on the link `href` goes: within the page, to a file
    /// beside this one, or out to the system's handler.
    fn follow_link(&mut self, href: &str) -> EditorOutcome {
        if let Some(name) = page::fragment(href) {
            if let Some(top) = self.page.as_ref().and_then(|page| page.fragment_top(name)) {
                self.scroll_to(top);
            }
            return EditorOutcome::Consumed;
        }
        let local = href.split('#').next().unwrap_or_default();
        let target = self.path.parent().map(|dir| dir.join(local)).filter(|path| path.exists());
        match (target, url::Url::parse(href)) {
            (_, Ok(_)) => EditorOutcome::OpenExternal(PathBuf::from(href)),
            (Some(path), Err(_)) => EditorOutcome::OpenPath(OpenTarget::file(path)),
            (None, Err(_)) => {
                self.notice = Some(Notice::warning(format!("no such file: {local}")));
                EditorOutcome::Consumed
            }
        }
    }

    /// Scroll as a motion asks, where it is one that scrolls.
    fn apply_motion(&mut self, motion: CursorMove) -> EditorOutcome {
        let row = self.row_height();
        let screen = self.pane.1;
        let by = match motion {
            CursorMove::Down => row,
            CursorMove::Up => -row,
            CursorMove::HalfPageDown => screen / 2.0,
            CursorMove::HalfPageUp => -screen / 2.0,
            CursorMove::PageDown => screen - row,
            CursorMove::PageUp => row - screen,
            CursorMove::Top => -self.scroll,
            CursorMove::Bottom => self.max_scroll() - self.scroll,
            _ => return EditorOutcome::Ignored,
        };
        self.scroll_to(self.scroll + by);
        EditorOutcome::Consumed
    }

    fn run(&mut self, act: Act) -> EditorOutcome {
        match act {
            Act::Follow => {
                self.follow = !self.follow;
                let state = if self.follow { "on" } else { "off" };
                self.notice = Some(Notice::info(format!("following the source {state}")).unlogged());
                self.follow_source();
                EditorOutcome::Consumed
            }
            Act::Search => EditorOutcome::Prompt(PromptRequest::search(ASK_SEARCH)),
            Act::SearchStep(forward) => {
                if let Some(query) = self.search.clone() {
                    self.find(&query, forward);
                }
                EditorOutcome::Consumed
            }
            Act::Source => EditorOutcome::OpenBeside(OpenTarget::at_line(self.path.clone(), self.top_line() + 1)),
            Act::Unselect => match self.page.as_mut().is_some_and(WebPage::clear_selection) {
                true => {
                    self.forget_tiles();
                    EditorOutcome::Consumed
                }
                false => EditorOutcome::Ignored,
            },
        }
    }
}

// ========================================================================
// PreviewEditor: what only the window asks of it
// ========================================================================

impl PreviewEditor {
    /// The file a view follows the text of, as another editor edits it, and
    /// the revision of that text it last took.
    pub(crate) fn follows_text(&self) -> Option<(PathBuf, Option<usize>)> {
        Some((self.path.clone(), self.revision))
    }

    /// Take the text of the file this view follows.
    pub(crate) fn take_shared_text(&mut self, text: SharedText) {
        self.fed = true;
        let moved = self.source_cursor != Some(text.cursor);
        self.source_cursor = Some(text.cursor);
        if let Some(lines) = text.lines {
            self.revision = Some(text.revision);
            self.set_text(lines.join("\n"));
        }
        if moved && self.follow {
            self.follow_source();
        }
    }
}

impl Editor for PreviewEditor {
    fn save_view(&self) -> Option<Box<dyn std::any::Any>> {
        Some(Box::new(self.scroll))
    }

    fn restore_view(&mut self, state: Box<dyn std::any::Any>) {
        if let Ok(scroll) = state.downcast::<f32>() {
            self.scroll = *scroll;
        }
    }

    fn title(&self) -> String {
        let name = self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |name| name.to_string_lossy().into_owned(),
        );
        format!("Preview {name}")
    }

    fn icon(&self) -> Option<IconKind> {
        Some(IconKind::Editor { kind: "preview" })
    }

    fn content(&mut self, rows: usize, cols: usize, _wrap: bool) -> Frame {
        self.grid = (rows, cols);
        // The page the host draws covers every row.
        match self.markdown.trim().is_empty() {
            true => Frame::new(vec![vec![Span::new(Style::Dim, EMPTY)]]),
            false => Frame::new(vec![Row::new()]),
        }
    }

    fn on_key(&mut self, key: &Key) -> EditorOutcome {
        if !self.nav.in_sequence() {
            if let Some(act) = table::find(KEYS, key) {
                return self.run(act);
            }
        }
        match self.nav.row_key(key) {
            VimKey::Motion(motion) => self.apply_motion(motion),
            VimKey::Pending => EditorOutcome::Consumed,
            VimKey::Unhandled => EditorOutcome::Ignored,
        }
    }

    fn keys(&self) -> Vec<KeyHelp> {
        let mut rows = table::help(KEYS);
        rows.extend(VimNav::help());
        rows
    }

    fn scrolls_smoothly(&self) -> bool {
        true
    }

    fn on_scroll_pixels(&mut self, pixels: f32) -> EditorOutcome {
        self.scroll_to(self.scroll - pixels);
        EditorOutcome::Consumed
    }

    fn on_scroll(&mut self, lines: isize) -> EditorOutcome {
        // The wheel counts rows positive upwards, the opposite of how far
        // the page moves.
        self.scroll_to(self.scroll - lines as f32 * self.row_height());
        EditorOutcome::Consumed
    }

    fn on_mouse(&mut self, at: Point) -> EditorOutcome {
        let (x, y) = self.page_point(at);
        let Some(page) = self.page.as_mut() else {
            return EditorOutcome::Consumed;
        };
        // What a drag selects is copied as it grows, the way a terminal's
        // selection is: the host hears of no release to wait for.
        if at.drag {
            if !page.select_to(x, y) {
                return EditorOutcome::Consumed;
            }
            let selected = page.selected_text();
            self.forget_tiles();
            return selected.map_or(EditorOutcome::Consumed, EditorOutcome::copied);
        }
        if let Some(href) = page.link_at(x, y) {
            return self.follow_link(&href);
        }
        let had = page.clear_selection();
        page.select_from(x, y);
        if had {
            self.forget_tiles();
        }
        EditorOutcome::Consumed
    }

    fn on_double_click(&mut self, at: Point) -> EditorOutcome {
        let (_, y) = self.page_point(at);
        let line = self.page.as_ref().and_then(|page| page.line_at(y)).unwrap_or(0);
        EditorOutcome::OpenBeside(OpenTarget::at_line(self.path.clone(), line + 1))
    }

    fn on_prompt(&mut self, reply: PromptReply) -> EditorOutcome {
        if let (ASK_SEARCH, Some(query)) = (reply.tag, reply.answer) {
            self.find(&query, true);
            self.search = Some(query);
        }
        EditorOutcome::Consumed
    }

    fn on_prompt_input(&mut self, query: &str, tag: &'static str) -> EditorOutcome {
        if tag == ASK_SEARCH && !query.trim().is_empty() {
            self.find(query, true);
        }
        EditorOutcome::Consumed
    }

    fn take_notice(&mut self) -> Option<Notice> {
        self.notice.take()
    }

    fn status(&self) -> EditorStatus {
        let source = match self.fed {
            true => "live",
            false => "from disk",
        };
        let follow = match self.follow {
            true => "following",
            false => "free",
        };
        EditorStatus {
            mode: None,
            segments: vec![source.to_string(), follow.to_string()],
        }
    }

    fn set_appearance(&mut self, appearance: &Appearance) {
        let stylesheet = page::stylesheet(appearance.theme);
        if appearance.scale != self.scale {
            self.scale = appearance.scale;
            self.forget_layout();
        }
        let settled = self.settle_at.is_none_or(|at| Instant::now() >= at);
        if stylesheet != self.stylesheet || (self.stale && settled) || self.html.is_none() {
            self.dark = appearance.theme.background.luminance() < 0.5;
            let fill = appearance.theme.selection_bg;
            self.selection = (fill.r, fill.g, fill.b);
            let diagrams = page::diagram_options(appearance.theme);
            self.html = Some(page::html(&self.markdown, &stylesheet, &diagrams, appearance.styled_html));
            self.stylesheet = stylesheet;
            self.stale = false;
            self.settle_at = None;
            self.forget_layout();
        }
    }

    fn set_pane_size(&mut self, pane: (f32, f32)) {
        if pane != self.pane {
            self.pane = pane;
            if let Some(page) = self.page.as_mut() {
                page.resize(pixel_size(pane), self.scale, self.dark);
            }
            self.forget_tiles();
        }
        if pane.0 >= 1.0 && pane.1 >= 1.0 {
            self.paint_visible();
        }
    }

    fn tiles(&self) -> Vec<Tile> {
        self.tiles.clone()
    }

    fn set_shown(&mut self, shown: bool) {
        if self.hidden && shown {
            self.check_at = Instant::now();
        }
        self.hidden = !shown;
    }

    fn next_tick(&self) -> Option<Instant> {
        let disk = (!self.fed && !self.hidden).then_some(self.check_at);
        [disk, self.settle_at].into_iter().flatten().min()
    }

    fn on_tick(&mut self, now: Instant) -> EditorOutcome {
        if !self.fed && now >= self.check_at {
            self.check_at = now + DISK_CHECK;
            self.read_disk();
        }
        EditorOutcome::Consumed
    }

    fn cwd(&self) -> Option<PathBuf> {
        self.path.parent().map(Path::to_path_buf)
    }

    fn holds_file(&self, path: &Path) -> bool {
        self.path == path
    }

    fn address(&self) -> Option<String> {
        Some(crate::model::path::abbreviate_home(&self.path))
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// A pane's size in whole pixels, never none.
fn pixel_size(pane: (f32, f32)) -> (u32, u32) {
    (pane.0.round().max(1.0) as u32, pane.1.round().max(1.0) as u32)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::input::KeyCode;

    /// A document of `count` paragraphs, one every other line.
    fn paragraphs(count: usize) -> Vec<String> {
        (0..count).flat_map(|at| [format!("para {at}"), String::new()]).collect()
    }

    fn shared(lines: &[String], cursor: usize, revision: usize) -> SharedText {
        SharedText {
            cursor,
            lines: Some(lines.to_vec()),
            revision,
        }
    }

    /// Lay `preview` out in a 400 by 300 pane, the way a frame does.
    fn draw(preview: &mut PreviewEditor) {
        let plain = |rows: &[Row]| format!("<pre>{}</pre>", rows.len());
        let theme = winter_render::Theme::dark();
        preview.set_appearance(&Appearance {
            scale: 1.0,
            styled_html: &plain,
            theme: &theme,
        });
        preview.content(15, 40, false);
        preview.set_pane_size((400.0, 300.0));
    }

    #[test]
    fn test_text_handed_over_is_laid_out_and_the_source_cursor_kept_in_view() {
        let mut preview = PreviewEditor::new(PathBuf::from("/nowhere/notes.md"));
        let lines = paragraphs(60);
        preview.take_shared_text(shared(&lines, 100, 1));
        assert!(preview.status().segments.contains(&"live".to_string()));
        assert_eq!(preview.next_tick(), None, "an editor's text leaves the disk alone");
        draw(&mut preview);
        let top = preview.page.as_ref().and_then(|page| page.line_top(100)).expect("laid out");
        assert!(preview.scroll > 0.0, "the cursor's block is far down the page");
        assert!((preview.scroll..preview.scroll + 300.0).contains(&top), "and brought into view");
        assert!(
            preview.tiles().iter().all(|tile| matches!(tile.content, Content::Scene(_))),
            "the page is drawn as painted scenes"
        );
        preview.on_key(&Key::plain(KeyCode::Char('f')));
        let scroll = preview.scroll;
        preview.take_shared_text(SharedText { cursor: 0, lines: None, revision: 1 });
        assert_eq!(preview.scroll, scroll, "not following");
    }

    #[test]
    fn test_going_to_the_source_opens_the_file_at_the_top_block_s_line() {
        let mut preview = PreviewEditor::new(PathBuf::from("/nowhere/notes.md"));
        preview.take_shared_text(shared(&paragraphs(60), 0, 1));
        draw(&mut preview);
        preview.on_key(&Key::plain(KeyCode::Char('G')));
        let line = preview.top_line();
        assert!(line > 0, "scrolled to the bottom");
        let outcome = preview.on_key(&Key::plain(KeyCode::Char('o')));
        assert_eq!(outcome, EditorOutcome::OpenBeside(OpenTarget::at_line(PathBuf::from("/nowhere/notes.md"), line + 1)));
    }

    #[test]
    fn test_a_drag_over_a_paragraph_copies_what_it_selects_and_esc_lets_go() {
        let mut preview = PreviewEditor::new(PathBuf::from("/nowhere/notes.md"));
        preview.take_shared_text(shared(&paragraphs(3), 0, 1));
        draw(&mut preview);
        let at = |col, drag| Point { col, drag, row: 1 };
        assert_eq!(preview.on_mouse(at(0, false)), EditorOutcome::Consumed, "from the margin");
        let EditorOutcome::Copy { text, .. } = preview.on_mouse(at(39, true)) else {
            panic!("a drag out past the text copies what it selects");
        };
        assert_eq!(text, "para 0");
        assert_eq!(preview.on_key(&Key::plain(KeyCode::Escape)), EditorOutcome::Consumed);
        assert_eq!(preview.on_key(&Key::plain(KeyCode::Escape)), EditorOutcome::Ignored, "nothing left to let go of");
    }

    #[test]
    fn test_a_page_slow_to_lay_out_waits_for_typing_to_pause() {
        let mut preview = PreviewEditor::new(PathBuf::from("/nowhere/notes.md"));
        preview.take_shared_text(shared(&paragraphs(2), 0, 1));
        draw(&mut preview);
        preview.laid_out_in = SLOW_LAYOUT * 3;
        preview.take_shared_text(shared(&["fresh".to_string()], 0, 2));
        draw(&mut preview);
        let written = |preview: &PreviewEditor| preview.html.as_deref().is_some_and(|html| html.contains("fresh"));
        assert!(!written(&preview), "still typing: the page waits");
        assert!(preview.next_tick().is_some(), "and a tick is asked for to catch up");
        preview.settle_at = Some(Instant::now());
        draw(&mut preview);
        assert!(written(&preview), "once typing pauses it is laid out");
        assert_eq!(preview.next_tick(), None);
    }
}

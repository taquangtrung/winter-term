//! Moving through a PDF by what it links to: links and their labelled hints,
//! the preview of where a link goes, places to go back to, and SyncTeX's way
//! back to the source.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::model::editor::{CommandOutput, EditorOutcome, JobRequest, Notice};
use crate::model::input::{Key, KeyCode, Mods};
use crate::model::paint::Point;
use crate::model::picture::{Content, Tile};

use super::caret::Pos;
use super::layout::Column;
use super::links::{Link, LinkTo};
use super::outline::Target;
use crate::editors::synctex::{Context, Source, Spot};
use super::{crop_rect, hint_labels, layout, open_source, BackSync, Bounds, HintTo, HintUse, Hinted, Hints, Hover, PdfEditor, Pending, Place, Preview, HOVER_DELAY, NEXT_KEY, PREVIEW_ABOVE, PREVIEW_FRAME, PREVIEW_GAP, PREVIEW_HEIGHT, PREVIEW_WIDTH_SHARE, READING_LINE, SAME_PLACE};
use crate::editors::synctex;

// ========================================================================
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// Follow `link`: to its spot, the place left kept for `C-o`, or out to
    /// its URL.
    pub(super) fn follow(&mut self, link: Link) -> EditorOutcome {
        self.pending = Pending::default();
        match link.to {
            LinkTo::Uri(uri) => EditorOutcome::OpenExternal(PathBuf::from(uri)),
            LinkTo::Spot(target) => {
                self.remember_place();
                self.go_to_target(target);
                EditorOutcome::Consumed
            }
        }
    }

    /// Scroll so `target` is at the top.
    pub(super) fn go_to_target(&mut self, target: Target) {
        let Some(rect) = self
            .column()
            .and_then(|column| column.pages.get(target.page).copied())
        else {
            return;
        };
        let y = rect.origin().1 + target.y * self.scale() - layout::GAP;
        self.scroll_to((self.scroll.0, y));
    }

    /// Where the reader is: the page being read, and how far down it the
    /// view's top is.
    pub(super) fn place(&self) -> Option<Place> {
        let column = self.column()?;
        let page = self.current_page();
        let offset = (self.scroll.1 - column.pages.get(page)?.origin().1) / self.scale();
        Some(Place { offset, page })
    }

    /// Scroll back to `place`, its page clamped to the document's.
    pub(super) fn go_to_place(&mut self, place: Place) {
        let Some(column) = self.column() else {
            return;
        };
        let page = place.page.min(column.pages.len().saturating_sub(1));
        if let Some(rect) = column.pages.get(page) {
            let y = rect.origin().1 + place.offset * self.scale();
            self.scroll_to((self.scroll.0, y));
        }
    }

    /// Keep where the reader is for `C-o`, ahead of a jump away from it,
    /// and drop the way forward. A place within [`SAME_PLACE`] of the last
    /// one kept takes its spot rather than stacking a twin of it.
    pub(super) fn remember_place(&mut self) {
        let Some(here) = self.place() else {
            return;
        };
        self.jumps_ahead.clear();
        match self.jumps_back.last_mut() {
            Some(last)
                if last.page == here.page && (last.offset - here.offset).abs() < SAME_PLACE =>
            {
                *last = here;
            }
            _ => self.jumps_back.push(here),
        }
    }

    /// Go back to where the last jump left (`back`), or forward again to
    /// where it led.
    pub(super) fn jump(&mut self, back: bool) -> EditorOutcome {
        self.pending = Pending::default();
        let here = self.place();
        let (from, to) = match back {
            true => (&mut self.jumps_back, &mut self.jumps_ahead),
            false => (&mut self.jumps_ahead, &mut self.jumps_back),
        };
        let Some(place) = from.pop() else {
            return EditorOutcome::Consumed;
        };
        to.extend(here);
        self.go_to_place(place);
        EditorOutcome::Consumed
    }

    /// The links any part of which is in view, with their pages.
    pub(super) fn links_in_view(&mut self) -> Vec<(usize, Link)> {
        let Some(column) = self.column() else {
            return Vec::new();
        };
        let pane = self.pane;
        let mut found = Vec::new();
        for page in column.visible(self.scroll.1, self.pane.1) {
            let links = match self.document.as_mut() {
                Some(doc) => doc.links(page).to_vec(),
                None => continue,
            };
            for link in links {
                let (x, y, width, height) = self.on_screen(&column, page, link.bounds);
                if x < pane.0 && y < pane.1 && x + width > 0.0 && y + height > 0.0 {
                    found.push((page, link));
                }
            }
        }
        found
    }

    /// Label the links in view, for typing one's label to follow it, or to
    /// preview where it leads when `preview` is set.
    pub(super) fn show_hints(&mut self, purpose: HintUse) -> EditorOutcome {
        self.pending = Pending::default();
        let targets: Vec<(usize, Bounds, HintTo)> = match purpose {
            HintUse::Sync => self
                .lines_in_view()
                .into_iter()
                .map(|(pos, bounds)| (pos.page, bounds, HintTo::Line(pos)))
                .collect(),
            HintUse::Follow | HintUse::Preview => self
                .links_in_view()
                .into_iter()
                .map(|(page, link)| (page, link.bounds, HintTo::Link(link)))
                .collect(),
        };
        if targets.is_empty() {
            let none = match purpose {
                HintUse::Sync => "no text in view",
                HintUse::Follow | HintUse::Preview => "no links in view",
            };
            self.notice = Some(Notice::info(none));
            return EditorOutcome::Consumed;
        }
        let labels = hint_labels(targets.len());
        let links = targets
            .into_iter()
            .zip(labels)
            .map(|((page, bounds, to), label)| Hinted {
                bounds,
                key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
                label,
                page,
                to,
            })
            .collect();
        self.hints = Some(Hints {
            purpose,
            links,
            typed: String::new(),
        });
        EditorOutcome::Consumed
    }

    /// The start of each line of text any part of which is in view, with
    /// the box of its first character.
    pub(super) fn lines_in_view(&mut self) -> Vec<(Pos, Bounds)> {
        let Some(column) = self.column() else {
            return Vec::new();
        };
        let mut found = Vec::new();
        for page in column.visible(self.scroll.1, self.pane.1) {
            let Some(text) = self.document.as_mut().and_then(|doc| doc.text(page)) else {
                continue;
            };
            let starts: Vec<(usize, Bounds)> = text
                .lines
                .iter()
                .enumerate()
                .filter_map(|(line, text)| Some((line, text.boxes.first()?.bounds())))
                .collect();
            for (line, bounds) in starts {
                let (_, y, _, height) = self.on_screen(&column, page, bounds);
                if y < self.pane.1 && y + height > 0.0 {
                    found.push((
                        Pos {
                            char: 0,
                            line,
                            page,
                        },
                        bounds,
                    ));
                }
            }
        }
        found
    }

    /// A key while the links' labels are up: a label's next letter, typed
    /// whole choosing its link; `Backspace` takes one back; anything else,
    /// or a letter no label goes on with, puts the labels away.
    pub(super) fn on_hint_key(&mut self, key: &Key) -> EditorOutcome {
        let Some(mut hints) = self.hints.take() else {
            return EditorOutcome::Consumed;
        };
        match key.chord() {
            (Mods::NONE, KeyCode::Backspace) => {
                hints.typed.pop();
                self.hints = Some(hints);
            }
            (Mods::NONE, KeyCode::Char(c)) => {
                hints.typed.push(c.to_ascii_lowercase());
                if let Some(index) = hints
                    .links
                    .iter()
                    .position(|hint| hint.label == hints.typed)
                {
                    let chosen = hints.links.swap_remove(index);
                    return match (chosen.to, hints.purpose) {
                        (HintTo::Line(pos), _) => self.sync_back(Some(pos)),
                        (HintTo::Link(link), HintUse::Preview) => {
                            self.show_preview(link, chosen.page)
                        }
                        (HintTo::Link(link), _) => self.follow(link),
                    };
                }
                if hints
                    .links
                    .iter()
                    .any(|hint| hint.label.starts_with(&hints.typed))
                {
                    self.hints = Some(hints);
                }
            }
            _ => {}
        }
        EditorOutcome::Consumed
    }

    /// The link under the cursor, with its page.
    pub(super) fn link_at_caret(&mut self) -> Option<(usize, Link)> {
        let pos = self.caret.pos?;
        let (spot, _) = self.spot_and_context(pos)?;
        let link = self
            .document
            .as_mut()?
            .links(pos.page)
            .iter()
            .find(|link| link.holds(spot.x, spot.y))?
            .clone();
        Some((pos.page, link))
    }

    /// The link under the pane point `(x, y)`, from the pages' top, with
    /// its page.
    pub(super) fn link_at(&mut self, x: f32, y: f32) -> Option<(usize, Link)> {
        let column = self.column()?;
        let scale = self.scale();
        let page = column.page_at(self.scroll.1 + y);
        let origin = column.pages.get(page)?.origin();
        let at = (
            (x + self.scroll.0 - origin.0) / scale,
            (y + self.scroll.1 - origin.1) / scale,
        );
        self.document
            .as_mut()?
            .links(page)
            .iter()
            .find(|link| link.holds(at.0, at.1))
            .map(|link| (page, link.clone()))
    }

    /// Show where `link`, on page `page`, leads under it, for `Enter` to
    /// follow: a URL is said rather than shown.
    pub(super) fn show_preview(&mut self, link: Link, page: usize) -> EditorOutcome {
        self.pending = Pending::default();
        let target = match &link.to {
            LinkTo::Uri(uri) => {
                self.notice = Some(Notice::info(uri.clone()));
                return EditorOutcome::Consumed;
            }
            LinkTo::Spot(target) => *target,
        };
        self.open_preview(link, page, target, false);
        EditorOutcome::Consumed
    }

    /// Show `target`, where `link` on page `page` leads, under the link.
    pub(super) fn open_preview(&mut self, link: Link, page: usize, target: Target, hovered: bool) {
        self.preview = Some(Preview {
            crop: None,
            hovered,
            link,
            page,
            target,
        });
        self.request_visible();
        self.fill_preview();
    }

    /// Hear the pointer rest on `link`, on page `page`, or on no link: a new
    /// link is previewed after [`HOVER_DELAY`], and leaving the one that
    /// opened a preview closes it.
    pub(super) fn hover_link(&mut self, link: Option<(usize, Link)>) -> EditorOutcome {
        let same = match (&self.hover, &link) {
            (Some(hover), Some((page, link))) => hover.page == *page && hover.link == *link,
            (None, None) => true,
            _ => false,
        };
        if same {
            return EditorOutcome::Ignored;
        }
        if self.preview.as_ref().is_some_and(|preview| preview.hovered) {
            self.preview = None;
        }
        self.hover = link.map(|(page, link)| Hover {
            due: Some(Instant::now() + HOVER_DELAY),
            link,
            page,
        });
        EditorOutcome::Consumed
    }

    /// Show where the link the pointer rests on leads, once it has rested
    /// there [`HOVER_DELAY`]. A preview asked for with a key is left be; a
    /// URL is said by the status bar instead.
    pub(super) fn show_hover(&mut self, now: Instant) {
        let Some(hover) = self
            .hover
            .as_mut()
            .filter(|hover| hover.due.is_some_and(|due| now >= due))
        else {
            return;
        };
        hover.due = None;
        let (link, page) = (hover.link.clone(), hover.page);
        if self.preview.is_some() || self.hints.is_some() {
            return;
        }
        if let LinkTo::Spot(target) = link.to {
            self.open_preview(link, page, target, true);
        }
    }

    /// The URL of the link the pointer rests on, for the status bar.
    pub(super) fn hovered_uri(&self) -> Option<&str> {
        match &self.hover.as_ref()?.link.to {
            LinkTo::Uri(uri) => Some(uri),
            LinkTo::Spot(_) => None,
        }
    }

    /// Cut the preview from its target's page, once the page is drawn.
    pub(super) fn fill_preview(&mut self) {
        let Some(target) = self
            .preview
            .as_ref()
            .filter(|preview| preview.crop.is_none())
            .map(|preview| preview.target)
        else {
            return;
        };
        let Some(drawn) = self.pages.get(&target.page).filter(|drawn| !drawn.stale) else {
            return;
        };
        let top = ((target.y - PREVIEW_ABOVE).max(0.0) * drawn.scale) as u32;
        let height = (PREVIEW_HEIGHT * drawn.scale) as u32;
        let crop = crop_rect(&drawn.pixels, (0, top, drawn.pixels.width, height));
        if let Some(preview) = self.preview.as_mut() {
            preview.crop = Some((NEXT_KEY.fetch_add(1, Ordering::Relaxed), Arc::new(crop)));
        }
    }

    /// The tiles of the links' labels, and of the preview in its frame,
    /// over the pages.
    pub(super) fn link_tiles(&self, column: &Column) -> Vec<Tile> {
        let mut tiles = Vec::new();
        if let Some(hints) = &self.hints {
            for hint in hints
                .links
                .iter()
                .filter(|hint| hint.label.starts_with(&hints.typed))
            {
                let (x, y, _, _) = self.on_screen(column, hint.page, hint.bounds);
                let width = (hint.label.chars().count() as f32 + 0.5) * self.cell.0;
                tiles.push(Tile {
                    content: Content::Text(hint.label.to_uppercase().into()),
                    height: self.cell.1,
                    key: hint.key,
                    width,
                    x: x.round(),
                    y: y.round(),
                });
            }
        }
        let Some(preview) = &self.preview else {
            return tiles;
        };
        let Some((key, crop)) = &preview.crop else {
            return tiles;
        };
        let fit = (self.pane.0 * PREVIEW_WIDTH_SHARE / crop.width.max(1) as f32).min(1.0);
        let (width, height) = (crop.width as f32 * fit, crop.height as f32 * fit);
        let (x, y, _, link_height) = self.on_screen(column, preview.page, preview.link.bounds);
        let below = y + link_height + PREVIEW_GAP;
        let top = match below + height > self.pane.1 && y - PREVIEW_GAP - height >= 0.0 {
            true => y - PREVIEW_GAP - height,
            false => below,
        };
        let left = x.clamp(
            PREVIEW_GAP,
            (self.pane.0 - width - PREVIEW_GAP).max(PREVIEW_GAP),
        );
        tiles.push(self.overlays.caret.tile((
            left - PREVIEW_FRAME,
            top - PREVIEW_FRAME,
            width + 2.0 * PREVIEW_FRAME,
            height + 2.0 * PREVIEW_FRAME,
        )));
        tiles.push(Tile {
            content: Content::Pixels(Arc::clone(crop)),
            height,
            key: *key,
            width,
            x: left.round(),
            y: top.round(),
        });
        tiles
    }

    // --------------------------------------------------------------------
    // SyncTeX
    // --------------------------------------------------------------------

    /// Go back to the LaTeX source of the character `at`, else the cursor's,
    /// else the one on the line being read: `synctex edit` names the
    /// source, and the text around the spot narrows its line down.
    pub(super) fn sync_back(&mut self, at: Option<Pos>) -> EditorOutcome {
        self.pending = Pending::default();
        let pos = at.or(self.caret.pos).or_else(|| self.reading_pos());
        let Some((spot, context)) = pos.and_then(|pos| self.spot_and_context(pos)) else {
            self.notice = Some(Notice::error("SyncTeX: no text here to go back from"));
            return EditorOutcome::Consumed;
        };
        self.sync = Some(BackSync::Locating(context));
        EditorOutcome::Job(JobRequest::Command(synctex::edit_request(&self.path, spot)))
    }

    /// The character at the reading line, a third of the way down the pane,
    /// or falling back to the center or first visible text in view.
    pub(super) fn reading_pos(&mut self) -> Option<Pos> {
        self.char_at(self.pane.0 / 2.0, self.pane.1 * READING_LINE)
            .or_else(|| self.char_at(self.pane.0 / 2.0, self.pane.1 / 2.0))
            .or_else(|| self.first_visible_char())
    }

    /// The first character on any page currently in view.
    pub(super) fn first_visible_char(&mut self) -> Option<Pos> {
        let column = self.column()?;
        let shown: Vec<usize> = column.visible(self.scroll.1, self.pane.1).collect();
        for page in shown {
            let text = self.document.as_mut()?.text(page)?;
            if let Some((line, _)) = text
                .lines
                .iter()
                .enumerate()
                .find(|(_, l)| !l.boxes.is_empty())
            {
                return Some(Pos::new(page, line, 0));
            }
        }
        None
    }

    /// Where the character `pos` stands on its page, as SyncTeX takes a
    /// spot, and the text of its line either side of it.
    pub(super) fn spot_and_context(&mut self, pos: Pos) -> Option<(Spot, Context)> {
        let line = self
            .document
            .as_mut()?
            .text(pos.page)?
            .lines
            .get(pos.line)?;
        let at = pos.char.min(line.boxes.len().checked_sub(1)?);
        let (left, top, width, height) = line.boxes[at].bounds();
        let spot = Spot {
            page: pos.page,
            x: left + width / 2.0,
            y: top + height / 2.0,
        };
        let split = pos.char.min(line.text.len());
        let context = Context {
            after: line.text[split..].iter().collect(),
            before: line.text[..split].iter().collect(),
        };
        Some((spot, context))
    }

    /// `synctex edit` answered: open the source it named, once the text
    /// around the spot has had its say about the line.
    pub(super) fn sync_located(
        &mut self,
        context: Context,
        output: CommandOutput,
    ) -> EditorOutcome {
        if !output.succeeded() {
            self.notice = Some(Notice::error(format!("SyncTeX: {}", output.failure())));
            return EditorOutcome::Consumed;
        }
        let dir = self.path.parent().unwrap_or(Path::new(""));
        let Some(source) = synctex::parse_edit(&output.stdout, dir) else {
            self.notice = Some(Notice::error(
                "SyncTeX: no source for this spot (built with -synctex=1?)",
            ));
            return EditorOutcome::Consumed;
        };
        if context.is_empty() {
            return open_source(source);
        }
        let path = source.path.clone();
        self.sync = Some(BackSync::Reading(context, source));
        EditorOutcome::Job(JobRequest::ReadFiles(vec![path]))
    }

    /// The source SyncTeX named was read: the line nearest SyncTeX's holding
    /// the text around the spot, else a look through the files it includes
    /// near there, where the text often really is.
    pub(super) fn sync_read(
        &mut self,
        context: Context,
        source: Source,
        files: Vec<(PathBuf, String)>,
    ) -> EditorOutcome {
        let Some((_, content)) = files.into_iter().next() else {
            return open_source(source);
        };
        if let Some(line) = synctex::find_in(&content, &context, Some(source.line)) {
            return open_source(Source { line, ..source });
        }
        let dir = source.path.parent().unwrap_or(Path::new(""));
        let included = synctex::included_files(&content, dir, source.line);
        if included.is_empty() {
            return open_source(source);
        }
        self.sync = Some(BackSync::Including(context, source));
        EditorOutcome::Job(JobRequest::ReadFiles(included))
    }

    /// The pane point at the middle of the cell `at`, from the pages' top
    /// edge: above it is the toolbar.
    pub(super) fn page_point(&self, at: Point) -> (f32, f32) {
        (
            (at.col as f32 + 0.5) * self.cell.0 - self.thumbs_width(),
            (at.row as f32 + 0.5) * self.cell.1 - self.toolbar_height(),
        )
    }

    /// The character under the cell `at`, where `at` is over the pages
    /// rather than the toolbar or the open outline.
    pub(super) fn char_under(&mut self, at: Point) -> Option<Pos> {
        let (x, y) = self.page_point(at);
        if self.outline.is_some() || y < 0.0 {
            return None;
        }
        self.char_at(x, y)
    }
}

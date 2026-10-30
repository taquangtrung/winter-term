//! The pages the PDF viewer draws: asking the worker for them and their text,
//! the thumbnails beside them, and reading the file again when it changes on
//! disk.

use std::collections::hash_map::Entry;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use crate::model::editor::{Notice, PdfMark};
use crate::model::paint::Point;
use crate::model::picture::{Content, Tile};

use super::document::Document;
use super::layout::{Column, Crop};
use super::worker::{Reply, Request, Worker};
use super::{
    crop_rect, frame, layout, sentences, stamp, DrawnPage, DrawnThumb, PdfEditor, Spelling,
    Thumbnails, Zoom, CROP_MARGIN, KEEP_UNWANTED, LOOKAHEAD, MAX_DRAWN_SIDE, NEXT_KEY,
    READING_LINE, RELOAD_ATTEMPTS, THUMB_FRAME, THUMB_LOOKAHEAD, THUMB_MAX_WIDTH, THUMB_MIN_WIDTH,
    THUMB_SHARE,
};

// ========================================================================
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// The scale a page of `size` points is drawn at: the one it is shown
    /// at, short of [`MAX_DRAWN_SIDE`].
    pub(super) fn drawn_scale(&self, size: (f32, f32)) -> f32 {
        self.scale().min(MAX_DRAWN_SIDE / size.0.max(size.1))
    }

    /// Ask for every page in view, and those just past it, that is not drawn
    /// at the scale it is shown at, and let go of those far from it.
    pub(super) fn request_visible(&mut self) {
        let (Some(column), Some(doc), Some(worker)) =
            (self.column(), self.document.as_ref(), self.worker.as_ref())
        else {
            return;
        };
        if self.pane.1 <= 0.0 {
            return;
        }
        let shown = column.visible(self.scroll.1, self.pane.1);
        let mut wanted: Vec<usize> = (shown.start.saturating_sub(LOOKAHEAD)
            ..(shown.end + LOOKAHEAD).min(doc.page_count()))
            .collect();
        // A preview's target is drawn wherever it is, to be cut from.
        if let Some(preview) = &self.preview {
            if preview.target.page < doc.page_count() && !wanted.contains(&preview.target.page) {
                wanted.push(preview.target.page);
            }
        }
        let now = Instant::now();
        for &page in &wanted {
            self.wanted_at.insert(page, now);
            let scale = self.drawn_scale(doc.sizes()[page]);
            // Drawn sharper than it is shown is good enough: another tab
            // showing the document larger drew it so, and drawing it again
            // smaller would only have that tab draw it again in turn.
            let drawn = self.pages.get(&page).is_some_and(|drawn| {
                drawn.scale >= scale && drawn.inverted == self.inverted && !drawn.stale
            });
            if drawn || self.requested.contains(&page) || self.failed.contains(&page) {
                continue;
            }
            worker.request(Request {
                generation: self.generation,
                invert: self.inverted,
                page,
                scale,
                thumb: false,
            });
            self.requested.insert(page);
        }
        self.wanted_at
            .retain(|_, at| now.saturating_duration_since(*at) < KEEP_UNWANTED);
        let wanted_at = &self.wanted_at;
        self.pages.retain(|page, _| wanted_at.contains_key(page));
        self.request_thumbs();
    }

    // --------------------------------------------------------------------
    // Thumbnails
    // --------------------------------------------------------------------

    /// How wide the thumbnails' strip is: nothing while it is not shown.
    pub(super) fn thumbs_width(&self) -> f32 {
        match self.thumbnails {
            Some(_) => (self.outer.0 * THUMB_SHARE).clamp(THUMB_MIN_WIDTH, THUMB_MAX_WIDTH),
            None => 0.0,
        }
    }

    /// The thumbnails laid out down their strip, and how far down it is
    /// scrolled: as far as keeps the page being read in the middle.
    pub(super) fn thumbs_column(&self) -> Option<(Column, f32)> {
        self.thumbnails.as_ref()?;
        let sizes = self.document.as_ref()?.sizes();
        let width = self.thumbs_width();
        let column = Column::new(sizes, layout::fit_width(sizes, width), width, None);
        let rect = column.pages.get(self.current_page())?;
        let middle = rect.y + rect.height / 2.0 - self.pane.1 / 2.0;
        let scroll = middle.clamp(0.0, column.max_scroll((width, self.pane.1)).1);
        Some((column, scroll))
    }

    /// Ask for the thumbnails in the strip, and a few past it, that are not
    /// drawn as they are shown yet.
    pub(super) fn request_thumbs(&mut self) {
        let Some((column, scroll)) = self.thumbs_column() else {
            return;
        };
        let (Some(worker), Some(thumbs)) = (self.worker.as_ref(), self.thumbnails.as_mut()) else {
            return;
        };
        let shown = column.visible(scroll, self.pane.1);
        let from = shown.start.saturating_sub(THUMB_LOOKAHEAD);
        let to = (shown.end + THUMB_LOOKAHEAD).min(column.pages.len());
        let scale = column.pages.first().map_or(1.0, |first| first.width)
            / self
                .document
                .as_ref()
                .and_then(|doc| doc.sizes().first())
                .map_or(1.0, |size| size.0);
        for page in from..to {
            let drawn = thumbs
                .drawn
                .get(&page)
                .is_some_and(|drawn| drawn.inverted == self.inverted);
            let asked = thumbs.requested.get(&page) == Some(&self.generation);
            if drawn || asked || self.failed.contains(&page) {
                continue;
            }
            worker.request(Request {
                generation: self.generation,
                invert: self.inverted,
                page,
                scale,
                thumb: true,
            });
            thumbs.requested.insert(page, self.generation);
        }
    }

    /// The thumbnails in the strip, the page being read framed, in the
    /// pane below the toolbar.
    pub(super) fn thumb_tiles(&self) -> Vec<Tile> {
        let (Some((column, scroll)), Some(thumbs)) = (self.thumbs_column(), &self.thumbnails)
        else {
            return Vec::new();
        };
        let blank = match self.inverted {
            true => &self.overlays.blank_inverted,
            false => &self.overlays.blank,
        };
        let current = self.current_page();
        let mut tiles = Vec::new();
        for page in column.visible(scroll, self.pane.1) {
            let rect = column.pages[page];
            let (key, pixels) = match thumbs.drawn.get(&page) {
                Some(drawn) => (drawn.key, Arc::clone(&drawn.pixels)),
                None => (blank.key, Arc::clone(&blank.pixels)),
            };
            let (x, y) = (rect.x.round(), (rect.y - scroll).round());
            tiles.push(Tile {
                content: Content::Pixels(pixels),
                height: rect.height,
                key,
                width: rect.width,
                x,
                y,
            });
            if page == current {
                let around = (
                    x - THUMB_FRAME,
                    y - THUMB_FRAME,
                    rect.width + 2.0 * THUMB_FRAME,
                    rect.height + 2.0 * THUMB_FRAME,
                );
                tiles.extend(frame(&self.overlays.caret, around, THUMB_FRAME));
            }
        }
        tiles
    }

    /// The page whose thumbnail is under the cell `at`, when the strip is
    /// shown and `at` is in it.
    pub(super) fn thumb_at(&self, at: Point) -> Option<usize> {
        let (column, scroll) = self.thumbs_column()?;
        let x = (at.col as f32 + 0.5) * self.cell.0;
        let y = (at.row as f32 + 0.5) * self.cell.1 - self.toolbar_height();
        if x >= self.thumbs_width() || y < 0.0 {
            return None;
        }
        let page = column.page_at(y + scroll);
        let rect = column.pages.get(page)?;
        (y + scroll >= rect.y && y + scroll < rect.y + rect.height).then_some(page)
    }

    /// Take in what the drawing thread has said.
    pub(super) fn take_replies(&mut self) {
        let Some(worker) = self.worker.as_ref() else {
            return;
        };
        for reply in worker.poll() {
            match reply {
                Reply::Opened(Ok(opened)) => {
                    self.take_document(Document::new(opened.pdf, opened.sizes, opened.outline));
                }
                Reply::Opened(Err(why)) => {
                    self.notice = Some(Notice::error(format!("{}: {why}", self.file_name())));
                    self.worker = None;
                    return;
                }
                Reply::Page(drawn) if drawn.request.thumb => {
                    if let Some(thumbs) = self.thumbnails.as_mut() {
                        let page = drawn.request.page;
                        thumbs.requested.remove(&page);
                        let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
                        let thumb = DrawnThumb {
                            inverted: drawn.request.invert,
                            key,
                            pixels: drawn.pixels,
                        };
                        thumbs.drawn.insert(page, thumb);
                    }
                }
                Reply::Page(drawn) => {
                    let page = drawn.request.page;
                    let current = drawn.request.generation == self.generation;
                    if current {
                        self.requested.remove(&page);
                    }
                    // A page drawn for a layout since left still beats a
                    // blank one until the current drawing arrives.
                    if current || !self.pages.contains_key(&page) {
                        self.pages.insert(
                            page,
                            DrawnPage {
                                cropped: None,
                                inverted: drawn.request.invert,
                                key: NEXT_KEY.fetch_add(1, Ordering::Relaxed),
                                pixels: drawn.pixels,
                                scale: drawn.request.scale,
                                stale: false,
                            },
                        );
                    }
                }
                Reply::Text(page, text) => {
                    if let Some(doc) = self.document.as_mut() {
                        doc.insert_text(page, text);
                    }
                }
                Reply::Failed(page) => {
                    self.requested.remove(&page);
                    self.failed.insert(page);
                    self.notice = Some(Notice::error(format!(
                        "{}: page {} could not be drawn",
                        self.file_name(),
                        page + 1
                    )));
                }
            }
        }
        self.crop_drawn();
    }

    /// Cut each drawn page to the crop, the ones not cut to it yet, while
    /// the pages are cropped to their text.
    pub(super) fn crop_drawn(&mut self) {
        let (Some(crop), Some(doc)) = (self.crop(), self.document.as_ref()) else {
            return;
        };
        for (page, drawn) in &mut self.pages {
            if drawn
                .cropped
                .as_ref()
                .is_some_and(|(cut, _, _)| *cut == crop)
            {
                continue;
            }
            let Some(size) = doc.sizes().get(*page) else {
                continue;
            };
            let (left, top, width, height) = layout::shown_part(*size, Some(crop));
            let at = |points: f32| (points * drawn.scale).round() as u32;
            let pixels = crop_rect(&drawn.pixels, (at(left), at(top), at(width), at(height)));
            let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
            drawn.cropped = Some((crop, key, Arc::new(pixels)));
        }
    }

    /// What of every page is shown while the pages are cropped to their
    /// text: the text's box, a margin round it.
    pub(super) fn crop(&self) -> Option<Crop> {
        let (left, top, width, height) = self.text_box.filter(|_| self.cropped)?;
        Some((
            left - CROP_MARGIN,
            top - CROP_MARGIN,
            width + 2.0 * CROP_MARGIN,
            height + 2.0 * CROP_MARGIN,
        ))
    }

    /// Measure the box the document's text falls in, the first time a
    /// crop or a fit to it needs it.
    pub(super) fn measure_text(&mut self) -> Option<Crop> {
        if self.text_box.is_none() {
            self.text_box = self.document.as_mut().and_then(Document::text_box);
        }
        self.text_box
    }

    /// Show `document`, at the page a restored tab was left on.
    pub(super) fn take_document(&mut self, document: Document) {
        self.document = Some(document);
        if let Some(page) = self.resume_page.take() {
            self.go_to_page(page - 1);
        }
        self.reveal_waiting_mark();
        self.request_visible();
    }

    /// Show the SyncTeX mark that was waiting, once there is a document and
    /// a pane to place it in.
    pub(super) fn reveal_waiting_mark(&mut self) {
        if self.document.is_none() || self.pane.1 <= 0.0 {
            return;
        }
        if let Some(mark) = self.waiting_mark.take() {
            self.reveal_mark(mark);
        }
    }

    /// Scroll `mark` to the reading line, sideways too where it is out of
    /// view, and mark it.
    pub(super) fn reveal_mark(&mut self, mark: PdfMark) {
        self.mark = Some(mark);
        let Some(column) = self.column() else {
            return;
        };
        let Some(rect) = column.pages.get(mark.page) else {
            return;
        };
        let scale = self.scale();
        let (origin_x, origin_y) = rect.origin();
        let left = origin_x + mark.left * scale;
        let x = match left < self.scroll.0 || left > self.scroll.0 + self.pane.0 {
            true => left - layout::GAP,
            false => self.scroll.0,
        };
        let y = origin_y + mark.top * scale - self.pane.1 * READING_LINE;
        self.scroll_to((x, y));
    }

    // --------------------------------------------------------------------
    // Rebuilds
    // --------------------------------------------------------------------

    /// Look at the file for a rebuild. A change is read once it has held
    /// still from one look to the next, so a build still writing the file
    /// is not read half done.
    pub(super) fn watch_disk(&mut self) {
        if self.reload.is_some() {
            return;
        }
        let now = stamp(&self.path);
        if now.is_none() || now == self.disk {
            self.changed = None;
            return;
        }
        if self.changed != now {
            self.changed = now;
            return;
        }
        self.changed = None;
        self.disk = now;
        self.reload = Some(Worker::open(self.path.clone()));
    }

    /// Take in what the rebuilt file's thread has said: once it has opened,
    /// it is the document. One that will not open is read again a few
    /// times, the build perhaps not done with it, then left until it changes.
    pub(super) fn take_reload(&mut self) {
        let Some(worker) = self.reload.as_ref() else {
            return;
        };
        for reply in worker.poll() {
            match reply {
                Reply::Opened(Ok(opened)) => {
                    self.worker = self.reload.take();
                    self.reload_failures = 0;
                    self.take_reloaded(Document::new(opened.pdf, opened.sizes, opened.outline));
                    return;
                }
                Reply::Opened(Err(why)) => {
                    self.reload = None;
                    self.reload_failures += 1;
                    match self.reload_failures < RELOAD_ATTEMPTS {
                        // Forgetting what was read makes the next look a change.
                        true => self.disk = None,
                        false => {
                            self.reload_failures = 0;
                            self.notice = Some(Notice::error(format!(
                                "{}: could not read it again: {why}",
                                self.file_name()
                            )));
                        }
                    }
                    return;
                }
                Reply::Failed(_) | Reply::Page(_) | Reply::Text(..) => {}
            }
        }
    }

    /// Show the document read again after a rebuild where the reader was:
    /// the same spot of the same page, or the SyncTeX mark being shown. The
    /// pages drawn from the old file stay up until they are drawn anew, and
    /// the cursor goes, its line perhaps moved.
    pub(super) fn take_reloaded(&mut self, document: Document) {
        let place = self.place();
        let count = document.page_count();
        self.document = Some(document);
        self.pages.retain(|page, _| *page < count);
        for drawn in self.pages.values_mut() {
            drawn.stale = true;
        }
        self.requested.clear();
        self.failed.clear();
        self.generation += 1;
        self.caret.forget_places();
        self.drag_origin = None;
        self.hints = None;
        self.menu_spot = None;
        self.outline = None;
        self.preview = None;
        // The text may have moved in the rebuild: measured again for the
        // crop or the fit that needs it.
        self.text_box = None;
        if let Some(split) = self.sentences.as_mut() {
            split.clear();
        }
        if let Some(thumbs) = self.thumbnails.as_mut() {
            *thumbs = Thumbnails::default();
        }
        if let Some(spelling) = self.spelling.as_mut() {
            *spelling = Spelling::default();
        }
        if self.cropped || self.zoom == Zoom::FitContent {
            self.measure_text();
        }
        match (self.mark, place) {
            (Some(mark), _) if mark.page < count => self.reveal_mark(mark),
            (_, Some(place)) => self.go_to_place(place),
            _ => {}
        }
        self.reveal_waiting_mark();
        self.request_visible();
    }

    /// Read the text of the pages in view while there is a cursor or a
    /// match to draw over them, which is drawn from the text, and their
    /// links while every link is outlined.
    pub(super) fn read_visible(&mut self) {
        let (text, links) = (self.caret.pos.is_some(), self.links_shown);
        self.split_visible();
        self.check_visible();
        if !text && !links {
            return;
        }
        let Some(column) = self.column() else {
            return;
        };
        let Some(doc) = self.document.as_mut() else {
            return;
        };
        for page in column.visible(self.scroll.1, self.pane.1) {
            if text {
                doc.text(page);
            }
            if links {
                doc.links(page);
            }
        }
    }

    /// Cut the pages in view into sentences while they are tinted, the
    /// ones not cut yet.
    pub(super) fn split_visible(&mut self) {
        let (Some(column), Some(split), Some(doc)) = (
            self.column(),
            self.sentences.as_mut(),
            self.document.as_mut(),
        ) else {
            return;
        };
        for page in column.visible(self.scroll.1, self.pane.1) {
            if let Entry::Vacant(slot) = split.entry(page) {
                if let Some(text) = doc.text(page) {
                    slot.insert(sentences::sentences(text));
                }
            }
        }
    }

    /// Check the pages in view for words spelled wrong while they are
    /// underlined, the ones not checked yet.
    pub(super) fn check_visible(&mut self) {
        if self.spelling.is_none() {
            return;
        }
        let Some(column) = self.column() else {
            return;
        };
        for page in column.visible(self.scroll.1, self.pane.1) {
            self.misspellings_on(page);
        }
    }
}

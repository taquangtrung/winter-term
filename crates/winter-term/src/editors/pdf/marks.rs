//! What the PDF viewer draws over its pages: words spelled wrong, sentence
//! tints, link boxes, and the caret, selection, and matches.

use std::collections::hash_map::Entry;

use winter_spell::DEFAULT_SUGGESTION_LIMIT;

use crate::editors::spell;
use crate::model::editor::{EditorOutcome, Notice, PdfMark};
use crate::model::picture::Tile;

use super::layout::Column;
use super::links::{Link, LinkTo};
use super::text::union;
use super::{
    frame, spelling, Bounds, PdfEditor, Spelling, CARET_WIDTH, LINK_LINE, MIN_CARET_HEIGHT,
    MIN_MARK_WIDTH, SENTENCE_COLORS, SPELLING_LINE,
};

// ========================================================================
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// How many words page `page` has spelled wrong, the page checked the
    /// first time it is asked about.
    pub(super) fn misspellings_on(&mut self, page: usize) -> usize {
        let (Some(spelling), Some(doc)) = (self.spelling.as_mut(), self.document.as_mut()) else {
            return 0;
        };
        if let Entry::Vacant(slot) = spelling.found.entry(page) {
            let found = doc
                .text(page)
                .map(|text| spelling::misspelled(text, &spell::dictionary()))
                .unwrap_or_default();
            slot.insert(found);
        }
        spelling.found.get(&page).map_or(0, Vec::len)
    }

    /// Go to the next word spelled wrong (`forward`), or the previous, from
    /// the last one stepped to or from the page being read, round past the
    /// document's end; checking starts if it had not.
    pub(super) fn step_misspelling(&mut self, forward: bool) -> EditorOutcome {
        let count = self.page_count();
        if count == 0 {
            return EditorOutcome::Consumed;
        }
        let first = self.current_page();
        let at = self.spelling.get_or_insert_with(Spelling::default).at;
        let from = at.map_or(first, |(page, _)| page);
        for step in 0..=count {
            let page = match forward {
                true => (from + step) % count,
                false => (from + count - step % count) % count,
            };
            let found = self.misspellings_on(page);
            let pick = match at {
                Some((last, index)) if step == 0 && last == page => match forward {
                    true => (index + 1 < found).then_some(index + 1),
                    false => index.checked_sub(1),
                },
                _ if found == 0 => None,
                _ => Some(if forward { 0 } else { found - 1 }),
            };
            if let Some(index) = pick {
                return self.land_on_misspelling(page, index);
            }
        }
        self.notice = Some(Notice::info("no words spelled wrong"));
        EditorOutcome::Consumed
    }

    /// Mark misspelling `index` of page `page` and bring it into view,
    /// saying how it might be spelled.
    pub(super) fn land_on_misspelling(&mut self, page: usize, index: usize) -> EditorOutcome {
        let Some(spelling) = self.spelling.as_mut() else {
            return EditorOutcome::Consumed;
        };
        spelling.at = Some((page, index));
        let Some((word, bounds)) = self.misspelled_word(page, index) else {
            return EditorOutcome::Consumed;
        };
        let (left, top, width, height) = bounds;
        self.reveal_mark(PdfMark {
            height,
            left,
            page,
            top,
            width,
        });
        let suggested = spell::dictionary().suggest(&word, DEFAULT_SUGGESTION_LIMIT);
        let text = match suggested.is_empty() {
            true => format!("{word}: no suggestions (z g to take it as right)"),
            false => format!("{word}: {}", suggested.join(", ")),
        };
        self.notice = Some(Notice::info(text));
        EditorOutcome::Consumed
    }

    /// Misspelling `index` of page `page`: the word, and its box.
    pub(super) fn misspelled_word(&self, page: usize, index: usize) -> Option<(String, Bounds)> {
        let run = *self.spelling.as_ref()?.found.get(&page)?.get(index)?;
        let line = self.document.as_ref()?.read(page)?.lines.get(run.line)?;
        let word = line.text.get(run.from..=run.to)?.iter().collect();
        let bounds = union(
            line.boxes.get(run.from)?.bounds(),
            line.boxes.get(run.to)?.bounds(),
        );
        Some((word, bounds))
    }

    /// Take the word spelled wrong last stepped to as right, from now on and
    /// in every editor, and check the pages again for it.
    pub(super) fn add_word(&mut self) -> EditorOutcome {
        let word = self
            .spelling
            .as_ref()
            .and_then(|spelling| spelling.at)
            .and_then(|(page, index)| self.misspelled_word(page, index));
        let Some((word, _)) = word else {
            self.notice = Some(Notice::info(
                "step to a word spelled wrong first, with C-M-n",
            ));
            return EditorOutcome::Consumed;
        };
        self.notice = Some(match spell::add_word(&word) {
            Ok(()) => Notice::info(format!("\"{word}\" is taken as spelled right")),
            Err(why) => Notice::error(format!("could not add \"{word}\": {why}")),
        });
        if let Some(spelling) = self.spelling.as_mut() {
            *spelling = Spelling::default();
        }
        self.read_visible();
        EditorOutcome::Consumed
    }

    /// The lines under the words spelled wrong in view, while they are
    /// underlined.
    pub(super) fn spelling_tiles(&self, column: &Column) -> Vec<Tile> {
        let Some(spelling) = &self.spelling else {
            return Vec::new();
        };
        let mut tiles = Vec::new();
        for page in column.visible(self.scroll.1, self.pane.1) {
            for index in 0..spelling.found.get(&page).map_or(0, Vec::len) {
                let Some((_, bounds)) = self.misspelled_word(page, index) else {
                    continue;
                };
                let (x, y, width, height) = self.on_screen(column, page, bounds);
                let under = (x, y + height, width, SPELLING_LINE);
                tiles.push(self.overlays.spelling.tile(under));
            }
        }
        tiles
    }

    /// The tints over the sentences of the pages in view, while they are
    /// tinted: each its own color after the one before.
    pub(super) fn sentence_tiles(&self, column: &Column) -> Vec<Tile> {
        let (Some(split), Some(doc)) = (&self.sentences, &self.document) else {
            return Vec::new();
        };
        let mut tiles = Vec::new();
        for page in column.visible(self.scroll.1, self.pane.1) {
            let (Some(found), Some(text)) = (split.get(&page), doc.read(page)) else {
                continue;
            };
            for (index, runs) in found.iter().enumerate() {
                let tint = &self.overlays.sentences[index % SENTENCE_COLORS.len()];
                for run in runs {
                    let Some(line) = text.lines.get(run.line) else {
                        continue;
                    };
                    let (Some(first), Some(last)) =
                        (line.boxes.get(run.from), line.boxes.get(run.to))
                    else {
                        continue;
                    };
                    let bounds = union(first.bounds(), last.bounds());
                    tiles.push(tint.tile(self.on_screen(column, page, bounds)));
                }
            }
        }
        tiles
    }

    /// The boxes around the links in view while every link is outlined, or
    /// around the one under the pointer: a frame for a link to a spot in the
    /// document, a line under a URL.
    pub(super) fn link_box_tiles(&self, column: &Column) -> Vec<Tile> {
        let Some(doc) = self.document.as_ref() else {
            return Vec::new();
        };
        let shown = column.visible(self.scroll.1, self.pane.1);
        let mut links: Vec<(usize, &Link)> = Vec::new();
        if self.links_shown {
            for page in shown.clone() {
                links.extend(doc.read_links(page).iter().map(|link| (page, link)));
            }
        }
        if let Some(hover) = self.hover.as_ref().filter(|_| !self.links_shown) {
            if shown.contains(&hover.page) {
                links.push((hover.page, &hover.link));
            }
        }
        let mut tiles = Vec::new();
        for (page, link) in links {
            // On whole pixels: a one-pixel line at a fractional offset is
            // drawn as a soft two-pixel smear.
            let (x, y, width, height) = self.on_screen(column, page, link.bounds);
            let (x, y, width, height) = (x.round(), y.round(), width.round(), height.round());
            let bottom = (x, y + height - LINK_LINE, width, LINK_LINE);
            match link.to {
                LinkTo::Uri(_) => tiles.push(self.overlays.url.tile(bottom)),
                LinkTo::Spot(_) => {
                    tiles.extend(frame(&self.overlays.link, (x, y, width, height), LINK_LINE));
                }
            }
        }
        tiles
    }

    /// What is drawn over the pages in view: what a search found, what is
    /// selected, and the cursor's bar.
    pub(super) fn overlay_tiles(&self, column: &Column) -> Vec<Tile> {
        let Some(doc) = self.document.as_ref() else {
            return Vec::new();
        };
        let shown = column.visible(self.scroll.1, self.pane.1);
        let line_span = |page: usize, line: usize, from: usize, to: usize| {
            let line = doc.read(page)?.lines.get(line)?;
            let last = line.boxes.len().checked_sub(1)?;
            let first = line.boxes[from.min(last)].bounds();
            let end = line.boxes[to.min(last)].bounds();
            Some(self.on_screen(column, page, union(first, end)))
        };
        let mut tiles = Vec::new();
        if let Some(mark) = self.mark.filter(|mark| shown.contains(&mark.page)) {
            let rect = (
                mark.left,
                mark.top,
                mark.width.max(MIN_MARK_WIDTH),
                mark.height,
            );
            tiles.push(
                self.overlays
                    .found
                    .tile(self.on_screen(column, mark.page, rect)),
            );
        }
        if let Some(found) = self
            .caret
            .found
            .filter(|found| shown.contains(&found.at.page))
        {
            let at = found.at;
            let end = at.char + found.length.saturating_sub(1);
            if let Some(rect) = line_span(at.page, at.line, at.char, end) {
                tiles.push(self.overlays.found.tile(rect));
            }
        }
        if let Some((start, end, linewise)) = self.caret.selection() {
            for page in start.page.max(shown.start)..=end.page.min(shown.end.saturating_sub(1)) {
                let Some(text) = doc.read(page) else {
                    continue;
                };
                let first = if page == start.page { start.line } else { 0 };
                let last = match page == end.page {
                    true => end.line,
                    false => text.lines.len().saturating_sub(1),
                };
                for line in first..=last.min(text.lines.len().saturating_sub(1)) {
                    let opens = page == start.page && line == start.line;
                    let closes = page == end.page && line == end.line;
                    let from = if linewise || !opens { 0 } else { start.char };
                    let to = if linewise || !closes {
                        usize::MAX
                    } else {
                        end.char
                    };
                    if let Some(rect) = line_span(page, line, from, to) {
                        tiles.push(self.overlays.selection.tile(rect));
                    }
                }
            }
        }
        if let Some(pos) = self.caret.shown().filter(|pos| shown.contains(&pos.page)) {
            if let Some((x, y, _, height)) = line_span(pos.page, pos.line, pos.char, pos.char) {
                // On whole pixels: a two-pixel bar at a fractional offset is
                // drawn as a soft three-pixel smear.
                let rect = (
                    x.round(),
                    y.round(),
                    CARET_WIDTH,
                    height.max(MIN_CARET_HEIGHT).round(),
                );
                tiles.push(self.overlays.caret.tile(rect));
            }
        }
        tiles
    }

    // --------------------------------------------------------------------
    // Saying where it is
    // --------------------------------------------------------------------
}

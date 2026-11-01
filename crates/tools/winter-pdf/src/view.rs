//! Where the PDF viewer is looking: the page and the scroll, the zoom, and
//! the glide between places.

use std::time::Instant;

use super::caret::Pos;
use super::document::Document;
use super::layout::Column;
use super::{layout, outside, Glide, PdfEditor, Zoom, GLIDE, READING_LINE};

// ========================================================================
// PdfEditor
// ========================================================================

impl PdfEditor {
    /// How many pages the document has, once it has opened.
    pub(super) fn page_count(&self) -> usize {
        self.document.as_ref().map_or(0, Document::page_count)
    }

    /// Pixels a point the pages are shown at.
    pub(super) fn scale(&self) -> f32 {
        let Some(doc) = &self.document else {
            return match self.zoom {
                Zoom::Scale(scale) => scale,
                _ => 1.0,
            };
        };
        // A page cropped to its text is fitted by what is shown of it.
        let sizes: Vec<(f32, f32)> = doc
            .sizes()
            .iter()
            .map(|size| {
                let (_, _, width, height) = layout::shown_part(*size, self.crop());
                (width, height)
            })
            .collect();
        match self.zoom {
            Zoom::Scale(scale) => scale,
            Zoom::FitContent => match self.text_box {
                Some((_, _, width, height)) => layout::fit_width(&[(width, height)], self.pane.0),
                None => layout::fit_width(&sizes, self.pane.0),
            },
            Zoom::FitWidth => layout::fit_width(&sizes, self.pane.0),
            Zoom::FitHeight => layout::fit_height(&sizes, self.pane.1),
            Zoom::FitPage => {
                layout::fit_width(&sizes, self.pane.0).min(layout::fit_height(&sizes, self.pane.1))
            }
        }
    }

    /// The pages laid out at the scale they are shown at.
    pub(super) fn column(&self) -> Option<Column> {
        let doc = self.document.as_ref()?;
        Some(Column::new(
            doc.sizes(),
            self.scale(),
            self.pane.0,
            self.crop(),
        ))
    }

    /// Scroll sideways so the text's box starts a gap in from the pane's
    /// left, as a fit to the text shows it.
    pub(super) fn align_to_text(&mut self) {
        let (Some(column), Some((left, _, _, _))) = (self.column(), self.text_box) else {
            return;
        };
        let Some(rect) = column.pages.get(self.current_page()) else {
            return;
        };
        let x = rect.origin().0 + left * self.scale() - layout::GAP;
        self.scroll_to((x, self.scroll.1));
    }

    /// The page being read: the one under the reading line.
    pub(super) fn current_page(&self) -> usize {
        self.column().map_or(0, |column| {
            column.page_at(self.scroll.1 + self.pane.1 * READING_LINE)
        })
    }

    /// Scroll to `scroll` at once, a glide on its way left where it is.
    pub(super) fn scroll_to(&mut self, scroll: (f32, f32)) {
        self.glide = None;
        self.set_scroll(scroll);
    }

    /// Put the view at `scroll`, kept where no gap opens past the column's
    /// edges.
    pub(super) fn set_scroll(&mut self, scroll: (f32, f32)) {
        self.scroll = self.clamped(scroll);
        self.request_visible();
    }

    /// `scroll` kept where no gap opens past the column's edges.
    pub(super) fn clamped(&self, scroll: (f32, f32)) -> (f32, f32) {
        let Some(column) = self.column() else {
            return scroll;
        };
        let (max_x, max_y) = column.max_scroll(self.pane);
        (scroll.0.clamp(0.0, max_x), scroll.1.clamp(0.0, max_y))
    }

    /// Where the view is going: a glide's end while one is on its way, so
    /// scrolls made one after another add up rather than start over.
    pub(super) fn scroll_target(&self) -> (f32, f32) {
        self.glide.map_or(self.scroll, |glide| glide.to)
    }

    /// Glide from where the view is to `scroll`, the host drawing each step
    /// of the way on the editor's ticks.
    pub(super) fn glide_to(&mut self, scroll: (f32, f32)) {
        let to = self.clamped(scroll);
        if to == self.scroll {
            self.glide = None;
            return;
        }
        self.glide = Some(Glide {
            from: self.scroll,
            started: Instant::now(),
            to,
        });
    }

    /// Move a glide on to where it is at `now`, ending it once it has
    /// landed: eased out, it starts fast and slows into place.
    pub(super) fn step_glide(&mut self, now: Instant) {
        let Some(glide) = self.glide else {
            return;
        };
        let t = (now.saturating_duration_since(glide.started).as_secs_f32() / GLIDE.as_secs_f32())
            .min(1.0);
        let eased = 1.0 - (1.0 - t).powi(3);
        let at = (
            glide.from.0 + (glide.to.0 - glide.from.0) * eased,
            glide.from.1 + (glide.to.1 - glide.from.1) * eased,
        );
        self.set_scroll(at);
        if t >= 1.0 {
            self.glide = None;
        }
    }

    /// Scroll `pixels` up the document (a touchpad's own measure), at once:
    /// the touchpad already moves smoothly, so it is followed exactly.
    pub(super) fn scroll_pixels(&mut self, pixels: f32) {
        let (x, y) = self.scroll_target();
        self.scroll_to((x, y - pixels));
    }

    /// Scroll so page `page`, counted from zero, starts at the top.
    pub(super) fn go_to_page(&mut self, page: usize) {
        let Some(column) = self.column() else {
            return;
        };
        let Some(rect) = column
            .pages
            .get(page.min(column.pages.len().saturating_sub(1)))
        else {
            return;
        };
        self.scroll_to((self.scroll.0, rect.y - layout::GAP));
    }

    /// Show the pages at `zoom`, keeping the point at the reading line on the
    /// same spot of the same page.
    pub(super) fn set_zoom(&mut self, zoom: Zoom) {
        self.relayout(|pdf| pdf.zoom = zoom);
    }

    /// Make `change` to how the pages are laid out, keeping the point at
    /// the reading line on the same spot of the same page.
    pub(super) fn relayout(&mut self, change: impl FnOnce(&mut Self)) {
        let Some(before) = self.column() else {
            change(self);
            return;
        };
        let line = self.pane.1 * READING_LINE;
        let page = before.page_at(self.scroll.1 + line);
        let down = (self.scroll.1 + line - before.pages[page].origin().1) / self.scale();
        let across = (self.scroll.0 + self.pane.0 / 2.0) / before.width;
        change(self);
        self.generation += 1;
        self.requested.clear();
        let Some(after) = self.column() else {
            return;
        };
        // A spot in a margin the crop cut off is kept at the nearest edge
        // of what is shown.
        let rect = after.pages[page];
        let scale = self.scale();
        let down = down.clamp(rect.top / scale, (rect.top + rect.height) / scale);
        let y = rect.origin().1 + down * scale - line;
        self.scroll_to((across * after.width - self.pane.0 / 2.0, y));
    }

    /// Where `bounds`, `(left, top, width, height)` in page `page`'s points,
    /// fall in the pane.
    pub(super) fn on_screen(
        &self,
        column: &Column,
        page: usize,
        bounds: (f32, f32, f32, f32),
    ) -> (f32, f32, f32, f32) {
        let (x, y) = column.pages[page].origin();
        let scale = self.scale();
        (
            x - self.scroll.0 + bounds.0 * scale,
            y - self.scroll.1 + bounds.1 * scale,
            bounds.2 * scale,
            bounds.3 * scale,
        )
    }

    /// Where the cursor's line is in the pane.
    pub(super) fn caret_line_rect(&mut self) -> Option<(f32, f32, f32, f32)> {
        let pos = self.caret.pos?;
        let column = self.column()?;
        let bounds = self
            .document
            .as_mut()?
            .text(pos.page)?
            .lines
            .get(pos.line)?
            .bounds()?;
        Some(self.on_screen(&column, pos.page, bounds))
    }

    /// Scroll as little as brings the cursor's character into view: not at
    /// all while it already is.
    pub(super) fn scroll_caret_into_view(&mut self) {
        let (Some(pos), Some(column)) = (self.caret.pos, self.column()) else {
            return;
        };
        let Some(bounds) = self.document.as_mut().and_then(|doc| {
            let line = doc.text(pos.page)?.lines.get(pos.line)?;
            let index = pos.char.min(line.boxes.len().checked_sub(1)?);
            Some(line.boxes[index].bounds())
        }) else {
            return;
        };
        let (x, y, width, height) = self.on_screen(&column, pos.page, bounds);
        let (mut left, mut top) = self.scroll;
        if y < 0.0 {
            top += y;
        } else if y + height > self.pane.1 {
            top += y + height - self.pane.1;
        }
        if x < 0.0 {
            left += x;
        } else if x + width > self.pane.0 {
            left += x + width - self.pane.0;
        }
        self.scroll_to((left, top));
    }

    /// The start of the line in view nearest `want` pixels down the pane:
    /// where `H`, `M`, `L`, and a cursor put in by `i` land.
    pub(super) fn screen_line(&mut self, want: f32) -> Option<Pos> {
        let column = self.column()?;
        let scale = self.scale();
        let (_, top) = self.scroll;
        let shown = column.visible(top, self.pane.1);
        let doc = self.document.as_mut()?;
        let mut best: Option<(f32, Pos)> = None;
        for page in shown {
            let page_top = column.pages[page].origin().1 - top;
            let Some(text) = doc.text(page) else {
                continue;
            };
            for (index, line) in text.lines.iter().enumerate() {
                let Some((_, line_top, _, height)) = line.bounds() else {
                    continue;
                };
                let middle = page_top + (line_top + height / 2.0) * scale;
                let distance = (middle - want).abs();
                if best.is_none_or(|(nearest, _)| distance < nearest) {
                    best = Some((distance, Pos::new(page, index, 0)));
                }
            }
        }
        best.map(|(_, pos)| pos)
    }

    /// The character nearest `(x, y)` in the pane: on the line the point is
    /// least far outside of, across and down at once, so a point in a
    /// column's margin lands in that column.
    pub(super) fn char_at(&mut self, x: f32, y: f32) -> Option<Pos> {
        let column = self.column()?;
        let scale = self.scale();
        let page = column.page_at(self.scroll.1 + y);
        let origin = column.pages[page].origin();
        let at = (
            (x + self.scroll.0 - origin.0) / scale,
            (y + self.scroll.1 - origin.1) / scale,
        );
        let text = self.document.as_mut()?.text(page)?;
        let mut best: Option<(f32, usize)> = None;
        for (index, line) in text.lines.iter().enumerate() {
            let Some((left, top, width, height)) = line.bounds() else {
                continue;
            };
            let distance = outside(at.0, left, left + width) + outside(at.1, top, top + height);
            if best.is_none_or(|(nearest, _)| distance < nearest) {
                best = Some((distance, index));
            }
        }
        let (_, line) = best?;
        let boxes = &text.lines[line].boxes;
        let char = boxes
            .iter()
            .position(|cell| at.0 < cell.x + cell.width)
            .unwrap_or(boxes.len().saturating_sub(1));
        Some(Pos::new(page, line, char))
    }

    // --------------------------------------------------------------------
    // Links
    // --------------------------------------------------------------------
}

//! Where each page of a document sits: one column of pages down the pane,
//! a gap around each, in pane pixels at a scale.

use std::ops::Range;

// ========================================================================
// Constants
// ========================================================================

/// Pixels between pages, and between the pages and the column's edges.
pub const GAP: f32 = 12.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The pages of a document laid out at one scale.
#[derive(Clone, Debug, PartialEq)]
pub struct Column {
    /// The whole column's height, the gaps included.
    pub height: f32,
    /// Each page's place, first page first.
    pub pages: Vec<PageRect>,
    /// The column's width: the pane's, or the widest page and its gaps when
    /// that is wider.
    pub width: f32,
}

/// One page's place in a [`Column`], in pixels from the column's top-left.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageRect {
    /// How tall the page is drawn.
    pub height: f32,
    /// How wide it is drawn.
    pub width: f32,
    /// Its left edge.
    pub x: f32,
    /// Its top edge.
    pub y: f32,
}

// ========================================================================
// Column
// ========================================================================

impl Column {
    /// Pages `sizes`, in points, at `scale` pixels a point, centered in a
    /// pane `pane_width` wide.
    pub fn new(sizes: &[(f32, f32)], scale: f32, pane_width: f32) -> Self {
        let widest = sizes.iter().map(|(w, _)| w * scale).fold(0.0, f32::max);
        let width = pane_width.max(widest + 2.0 * GAP);
        let mut y = GAP;
        let pages = sizes
            .iter()
            .map(|(w, h)| {
                let page = PageRect {
                    height: h * scale,
                    width: w * scale,
                    x: (width - w * scale) / 2.0,
                    y,
                };
                y += page.height + GAP;
                page
            })
            .collect();
        Self {
            height: y,
            pages,
            width,
        }
    }

    /// The pages any part of which is between `top` and `top + height`.
    pub fn visible(&self, top: f32, height: f32) -> Range<usize> {
        let bottom = top + height;
        let first = self
            .pages
            .partition_point(|page| page.y + page.height <= top);
        let end = self.pages.partition_point(|page| page.y < bottom);
        first..end.max(first)
    }

    /// The page at `y`, or the one before the gap `y` falls in, or the last
    /// for a `y` past them all.
    pub fn page_at(&self, y: f32) -> usize {
        self.pages
            .partition_point(|page| page.y <= y)
            .saturating_sub(1)
            .min(self.pages.len().saturating_sub(1))
    }

    /// How far the column scrolls before its far edges meet a pane of size
    /// `pane`: nothing along a side it fits.
    pub fn max_scroll(&self, pane: (f32, f32)) -> (f32, f32) {
        (
            (self.width - pane.0).max(0.0),
            (self.height - pane.1).max(0.0),
        )
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The scale that fits the widest of pages `sizes` across a pane
/// `pane_width` wide, its gaps included.
pub fn fit_width(sizes: &[(f32, f32)], pane_width: f32) -> f32 {
    fit(sizes.iter().map(|(w, _)| *w), pane_width)
}

/// The scale that fits the tallest of pages `sizes` down a pane
/// `pane_height` tall, its gaps included.
pub fn fit_height(sizes: &[(f32, f32)], pane_height: f32) -> f32 {
    fit(sizes.iter().map(|(_, h)| *h), pane_height)
}

/// The scale that fits the longest of `extents` into `room`, a gap either
/// side: one for no extents at all.
fn fit(extents: impl Iterator<Item = f32>, room: f32) -> f32 {
    let longest = extents.fold(0.0, f32::max);
    match longest > 0.0 {
        true => ((room - 2.0 * GAP) / longest).max(f32::EPSILON),
        false => 1.0,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const LETTER: (f32, f32) = (612.0, 792.0);

    #[test]
    fn test_pages_stack_down_the_middle_with_a_gap_round_each() {
        let column = Column::new(&[LETTER, (306.0, 396.0)], 1.0, 1000.0);
        assert_eq!(column.pages[0].y, GAP);
        assert_eq!(column.pages[0].x, (1000.0 - 612.0) / 2.0);
        assert_eq!(column.pages[1].y, GAP + 792.0 + GAP);
        assert_eq!(column.pages[1].x, (1000.0 - 306.0) / 2.0, "each centered");
        assert_eq!(column.height, GAP + 792.0 + GAP + 396.0 + GAP);
    }

    #[test]
    fn test_a_page_wider_than_the_pane_widens_the_column() {
        let column = Column::new(&[LETTER], 2.0, 800.0);
        assert_eq!(column.width, 1224.0 + 2.0 * GAP);
        assert_eq!(column.pages[0].x, GAP);
        assert_eq!(column.max_scroll((800.0, 600.0)).0, column.width - 800.0);
    }

    #[test]
    fn test_only_the_pages_in_view_are_visible() {
        let column = Column::new(&[LETTER; 5], 1.0, 800.0);
        let second = column.pages[1];
        // From inside the first page's bottom to just into the second's.
        assert_eq!(column.visible(second.y - 100.0, 150.0), 0..2);
        // Wholly inside the gap between the second and third.
        assert_eq!(column.visible(second.y + second.height + 1.0, 5.0), 2..2);
        assert_eq!(column.visible(0.0, column.height), 0..5);
    }

    #[test]
    fn test_the_page_at_a_point_counts_a_gap_with_the_page_above() {
        let column = Column::new(&[LETTER; 3], 1.0, 800.0);
        assert_eq!(column.page_at(0.0), 0, "the top gap belongs to page one");
        assert_eq!(column.page_at(column.pages[1].y + 1.0), 1);
        assert_eq!(column.page_at(column.pages[1].y - 1.0), 0);
        assert_eq!(column.page_at(column.height + 100.0), 2);
    }

    #[test]
    fn test_fitting_the_width_leaves_the_gaps() {
        let scale = fit_width(&[LETTER, (306.0, 396.0)], 636.0);
        assert_eq!(scale, 1.0, "the widest page fills it between the gaps");
        assert_eq!(fit_width(&[], 636.0), 1.0, "no pages, no scale to fit");
    }

    #[test]
    fn test_fitting_the_height_fits_the_tallest_page() {
        let scale = fit_height(&[(306.0, 396.0), LETTER], 792.0 + 2.0 * GAP);
        assert_eq!(scale, 1.0);
    }
}

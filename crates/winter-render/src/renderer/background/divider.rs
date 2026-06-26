//! The divider between two adjacent panes.

use super::super::PaneRect;
use super::{quad_vertices, BgVertex};

// ========================================================================
// Constants
// ========================================================================

/// Thickness of a pane divider, in pixels.
pub(crate) const DIVIDER_THICKNESS: f32 = 1.0;

// ========================================================================
// Divider
// ========================================================================

pub(crate) fn compute_divider(
    a: PaneRect,
    b: PaneRect,
    surface_w: f32,
    surface_h: f32,
    divider_color: (f32, f32, f32),
    width: f32,
) -> Option<[BgVertex; 6]> {
    // A vertical divider exists when one pane's right edge meets the other's left
    // edge and their y-ranges overlap (handles mixed-height layouts).
    //
    // Pane rects arrive pixel-snapped to integers. Adjacent panes share an
    // exact pixel edge when snapped corner-to-corner (see the app's
    // `layout_rect_to_pane`); the `<= 1.0` tolerance also covers a 1 px
    // sub-pixel snap mismatch so a divider is never dropped over a rounding
    // artifact.
    let a_right = a.x + a.width;
    let b_right = b.x + b.width;
    let a_bot = a.y + a.height;
    let b_bot = b.y + b.height;

    let (px0, py0, px1, py1) = if (a_right - b.x).abs() <= 1.0 || (b_right - a.x).abs() <= 1.0 {
        let y0 = a.y.max(b.y);
        let y1 = a_bot.min(b_bot);
        if y1 <= y0 {
            return None;
        }
        let x = if a.x < b.x { a_right } else { b_right };
        let x = x - width / 2.0;
        (x, y0, x + width, y1)
    } else if (a_bot - b.y).abs() <= 1.0 || (b_bot - a.y).abs() <= 1.0 {
        // A horizontal divider when one pane's bottom edge meets the other's top
        // and their x-ranges overlap (handles mixed-width layouts). The same 1 px
        // rounding tolerance as the vertical case above applies here.
        // Span the full overlapping width with no edge inset, so this divider
        // meets any crossing vertical divider (and the window's left/right
        // edges) cleanly, mirroring how the vertical divider runs the full
        // overlapping height above.
        let x0 = a.x.max(b.x);
        let x1 = a_right.min(b_right);
        if x1 <= x0 {
            return None;
        }
        let y = if a.y < b.y { a_bot } else { b_bot };
        let y = y - width / 2.0;
        (x0, y, x1, y + width)
    } else {
        return None;
    };

    Some(quad_vertices(
        px0,
        py0,
        px1,
        py1,
        divider_color,
        surface_w,
        surface_h,
    ))
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // Two horizontally-split panes whose pixel-snapped rects are off by exactly
    // 1 px at the shared edge (top bottom = 411, bottom top = 410) must still
    // produce a horizontal divider. Reproduces the missing-divider bug caused by
    // `layout_rect_to_pane` rounding each field independently.
    #[test]
    fn test_horizontal_divider_drawn_with_one_pixel_rounding_gap() {
        let a = PaneRect {
            x: 0.0,
            y: 55.0,
            width: 800.0,
            height: 356.0,
        };
        let b = PaneRect {
            x: 0.0,
            y: 410.0,
            width: 800.0,
            height: 356.0,
        };
        assert_eq!(a.y + a.height, 411.0); // top pane bottom edge
        assert_eq!((a.y + a.height) - b.y, 1.0); // one-pixel rounding discrepancy
        let dv = compute_divider(a, b, 800.0, 800.0, (1.0, 1.0, 1.0), 1.0);
        assert!(dv.is_some(), "divider must be drawn despite the 1 px gap");
    }

    // Same check for a vertical split whose left/right shared edge disagrees by
    // 1 px after independent rounding.
    #[test]
    fn test_vertical_divider_drawn_with_one_pixel_rounding_gap() {
        let a = PaneRect {
            x: 0.0,
            y: 0.0,
            width: 401.0,
            height: 800.0,
        };
        let b = PaneRect {
            x: 400.0,
            y: 0.0,
            width: 400.0,
            height: 800.0,
        };
        assert_eq!((a.x + a.width) - b.x, 1.0); // one-pixel rounding discrepancy
        let dv = compute_divider(a, b, 800.0, 800.0, (1.0, 1.0, 1.0), 1.0);
        assert!(dv.is_some(), "divider must be drawn despite the 1 px gap");
    }
}

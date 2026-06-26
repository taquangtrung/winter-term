//! The shell cursor's quads: its filled shape in a cell, and the outline
//! an unfocused cursor draws instead.

use crate::grid::CursorShape;

// ========================================================================
// Constants
// ========================================================================

const CURSOR_UNDERLINE_HEIGHT_RATIO: f32 = 0.2;

/// Width in pixels of a `Bar` (I-beam) cursor's vertical stroke.
const CURSOR_BAR_WIDTH: f32 = 2.0;

/// Offset in pixels applied to a `Bar` cursor's left position to keep it clear
/// of the glyph's ink at the left edge of a cell.
const CURSOR_BAR_OFFSET: f32 = -0.5;

/// Width in pixels of a hollow cursor's outline stroke (see [`cursor_outline_quads`]),
/// matching the window's own 1px [`Theme::window_border`] hairline.
pub(crate) const CURSOR_HOLLOW_STROKE_WIDTH: f32 = 1.0;

// ========================================================================
// Cursor geometry
// ========================================================================

/// The pixel rect of a cursor of `shape` whose full cell starts at `(x, y)`
/// with size `(cw, ch)`. Block fills the cell; Bar covers a thin strip on the
/// left edge (or, with `wrap_pending`, protrudes just past the cell's right
/// edge instead); Underline covers a thin strip on the bottom edge.
///
/// `wrap_pending` is set when the grid has deferred a line wrap (VT100 "last
/// column" behavior): the cursor is parked on the cell holding the glyph just
/// printed rather than an empty cell after it. A monospace glyph's ink
/// typically spans close to both edges of its cell, so a `Bar` drawn at
/// *either* edge of that same cell still touches it; parking the stroke just
/// past the cell's right edge instead keeps it clear of the glyph entirely,
/// matching how the `Bar` already sits past the last character on every other
/// column. `Block` and `Underline` already cover the full cell width and need
/// no adjustment.
pub(crate) fn cursor_quad(
    shape: CursorShape,
    x: f32,
    y: f32,
    cw: f32,
    ch: f32,
    wrap_pending: bool,
) -> (f32, f32, f32, f32) {
    let x1_full = x + cw;
    let y1_full = y + ch;
    match shape {
        CursorShape::Block => (x, y, x1_full, y1_full),
        CursorShape::Bar => {
            if wrap_pending {
                let x0 = x1_full;
                (x0, y, x0 + CURSOR_BAR_WIDTH, y1_full)
            } else {
                let x0 = x + CURSOR_BAR_OFFSET;
                (x0, y, x0 + CURSOR_BAR_WIDTH, y1_full)
            }
        }
        CursorShape::Underline => {
            let y0 = y1_full - ch * CURSOR_UNDERLINE_HEIGHT_RATIO;
            (x, y0, x1_full, y1_full)
        }
    }
}

/// The four pixel rects (top, bottom, left, right) that frame a cell of size
/// `(cw, ch)` starting at `(x, y)` with a `stroke`-wide outline, used to draw a
/// hollow cursor. Each rect is `(px0, py0, px1, py1)`, ready for
/// [`quad_vertices`]. The side rects run the cell's full height so the corners
/// aren't left with gaps.
pub(crate) fn cursor_outline_quads(
    x: f32,
    y: f32,
    cw: f32,
    ch: f32,
    stroke: f32,
) -> [(f32, f32, f32, f32); 4] {
    let x1 = x + cw;
    let y1 = y + ch;
    [
        (x, y, x1, y + stroke),
        (x, y1 - stroke, x1, y1),
        (x, y, x + stroke, y1),
        (x1 - stroke, y, x1, y1),
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_cursor_quad_bar_wrap_pending_clears_the_cell() {
        // Regression: a Bar cursor parked on the last-typed glyph (deferred
        // wrap) must not draw inside that glyph's cell on either edge, since
        // monospace ink commonly spans close to both edges.
        let (x0, _, x1, _) = cursor_quad(CursorShape::Bar, 100.0, 0.0, 10.0, 20.0, true);
        assert!(
            x0 >= 110.0,
            "bar must start at or past the cell's right edge, got x0={x0}"
        );
        assert!(x1 > x0);

        let (x0_no_wrap, _, _, _) = cursor_quad(CursorShape::Bar, 100.0, 0.0, 10.0, 20.0, false);
        assert_eq!(
            x0_no_wrap,
            100.0 + CURSOR_BAR_OFFSET,
            "without a pending wrap the bar sits slightly left of the cell's left edge"
        );
    }

    #[test]
    fn test_cursor_outline_quads_frames_the_cell_on_all_four_sides() {
        let [top, bottom, left, right] = cursor_outline_quads(100.0, 50.0, 10.0, 20.0, 2.0);
        assert_eq!(top, (100.0, 50.0, 110.0, 52.0));
        assert_eq!(bottom, (100.0, 68.0, 110.0, 70.0));
        assert_eq!(left, (100.0, 50.0, 102.0, 70.0));
        assert_eq!(right, (108.0, 50.0, 110.0, 70.0));
    }
}

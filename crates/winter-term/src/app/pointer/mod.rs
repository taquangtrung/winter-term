//! Pointer submodule: mouse hit-testing, selection state, clipboard, and
//! PTY mouse event forwarding.

mod clipboard;
mod mouse;

use std::time::Instant;

use crate::model::input::VisualKind;
use crate::model::layout::{PaneId, Rect};
use winter_render::renderer::PaneRect;

use super::{App, LastVisual, Selection};

// ========================================================================
// Data Structures
// ========================================================================

/// Transient pointer input: where the pointer is, what it is dragging, and
/// the timing state that turns clicks into double- and triple-clicks.
#[derive(Debug)]
pub(crate) struct PointerState {
    /// Next instant at which a held-button selection drag near the top/bottom
    /// viewport edge is allowed to auto-scroll by another line. Throttles
    /// [`App::auto_scroll_selection`] against the ~16ms `about_to_wait` tick.
    pub(crate) auto_scroll_next: Instant,
    pub(crate) cursor_pos: (f32, f32),
    /// Previous cursor pixel position during a split-divider drag, or `None`
    /// when no drag is in progress. Cleared on mouse release.
    pub(crate) divider_drag: Option<(f32, f32)>,
    /// The URL of the hyperlinked cell currently under the pointer, if any.
    /// Drives the pointer-cursor icon and Ctrl+click to open.
    pub(crate) hovered_url: Option<String>,
    pub(crate) last_click: Option<(Instant, f32, f32)>,
    pub(crate) mouse_down: bool,
    /// Where the held left button went down: the pane, and the absolute
    /// `(row, col)` under the pointer at that instant (see
    /// [`winter_render::Grid::to_absolute_row`]). `None` when no button is
    /// held.
    ///
    /// A drag's selection is anchored here rather than at the first motion
    /// event, which is a different cell whenever the pointer has already
    /// travelled between the press and the first event the compositor
    /// delivers. A quick flick starts a selection lines away from where the
    /// click landed.
    pub(crate) press_cell: Option<(PaneId, usize, usize)>,
    /// Which pane's scrollbar is being dragged, if any. Cleared on mouse release.
    pub(crate) scrollbar_drag: Option<PaneId>,
}

impl PointerState {
    /// The absolute `(row, col)` a fresh drag selection should anchor at,
    /// given the cell `motion` the pointer has just moved onto in `pane`.
    ///
    /// The press cell, when the button went down in this same pane: a drag
    /// starts where you clicked, not where the compositor happened to deliver
    /// the first motion event, which on a quick flick is already lines away.
    /// A press in another pane names a row in a different grid, so there the
    /// motion's own cell is the only meaningful anchor.
    pub(crate) fn drag_anchor(&self, pane: PaneId, motion: (usize, usize)) -> (usize, usize) {
        match self.press_cell {
            Some((press_pane, row, col)) if press_pane == pane => (row, col),
            _ => motion,
        }
    }
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            auto_scroll_next: Instant::now(),
            cursor_pos: (0.0, 0.0),
            divider_drag: None,
            hovered_url: None,
            last_click: None,
            mouse_down: false,
            press_cell: None,
            scrollbar_drag: None,
        }
    }
}

/// The active selection and the Visual-mode state behind it. [`Self::span`]
/// is set either by a mouse drag or by a Visual-mode motion, so it is not
/// tied to the mode: leaving Visual mode clears it explicitly.
pub(crate) struct SelectionState {
    /// The last Visual selection, restored by `gv` (see [`LastVisual`]).
    pub(crate) last_visual: Option<LastVisual>,
    pub(crate) span: Option<Selection>,
    /// Visual-mode anchor: the absolute `(row, col)` the selection was started
    /// from (see [`winter_render::Grid::to_absolute_row`]), so it keeps naming
    /// the same text as the view scrolls. Held viewport-relative it slid by the
    /// scroll distance and the selection detached from where `v` was pressed.
    /// `Some` only while the focused pane is in Visual mode.
    pub(crate) visual_anchor: Option<(usize, usize)>,
    /// The active Visual selection kind (Block, Char, Line).
    pub(crate) visual_kind: VisualKind,
}

impl Default for SelectionState {
    fn default() -> Self {
        Self {
            last_visual: None,
            span: None,
            visual_anchor: None,
            visual_kind: VisualKind::Char,
        }
    }
}

// ========================================================================
// App: pixel hit-testing helpers
// ========================================================================

impl App {
    pub(crate) fn pixel_to_cell(&self, x: f32, y: f32, pane_rect: PaneRect) -> (usize, usize) {
        let (cw, ch) = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size())
            .unwrap_or((9.0, 20.0));
        let col = ((x - pane_rect.x) / cw).floor() as usize;
        let row = ((y - pane_rect.y) / ch).floor() as usize;
        (row, col)
    }

    pub(crate) fn pane_at_pixel(&self, x: f32, y: f32) -> Option<(PaneId, PaneRect)> {
        let vp = self.viewport_rect();
        let layout_vp = Rect::new(vp.x, vp.y, vp.width, vp.height);
        for (id, rect) in self.tab().rects(layout_vp) {
            let pr = Self::layout_rect_to_pane(rect);
            if x >= pr.x && x < pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                return Some((id, pr));
            }
        }
        None
    }

    /// The hyperlink URL of the cell currently under the pointer, if any.
    pub(crate) fn hovered_link_at(&self, x: f32, y: f32) -> Option<String> {
        let (pane_id, pane_rect) = self.pane_at_pixel(x, y)?;
        let pane = self.panes.get(&pane_id)?;
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        pane.grid().cell_link(row, col).map(str::to_string)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_drag_anchor_starts_at_the_press_not_at_the_first_motion() {
        // Regression: the anchor was whatever cell the first motion event
        // landed on. Pointer motion is compressed, so a quick drag's first
        // event can arrive many rows below the click, and the selection then
        // started well past the line the user actually clicked on.
        let pointer = PointerState {
            press_cell: Some((PaneId(1), 40, 3)),
            ..Default::default()
        };
        assert_eq!(pointer.drag_anchor(PaneId(1), (57, 12)), (40, 3));
    }

    #[test]
    fn test_drag_anchor_falls_back_when_the_press_was_in_another_pane() {
        // A press in a different pane names a row in a different grid, so it
        // cannot anchor this pane's selection; nor can a drag with no press
        // behind it at all (a button held since before the window had focus).
        let mut pointer = PointerState {
            press_cell: Some((PaneId(2), 40, 3)),
            ..Default::default()
        };
        assert_eq!(pointer.drag_anchor(PaneId(1), (57, 12)), (57, 12));
        pointer.press_cell = None;
        assert_eq!(pointer.drag_anchor(PaneId(1), (57, 12)), (57, 12));
    }
}

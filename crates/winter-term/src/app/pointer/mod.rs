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
    /// Which pane's scrollbar is being dragged, if any. Cleared on mouse release.
    pub(crate) scrollbar_drag: Option<PaneId>,
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
    /// Visual-mode anchor (viewport `(row, col)`) where the selection began.
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

//! Pointer submodule: mouse hit-testing, selection state, clipboard, and
//! PTY mouse event forwarding.

mod clipboard;
mod mouse;

pub(in crate::window) use clipboard::Clipboards;

use std::time::Instant;

use crate::model::input::VisualKind;
use crate::model::layout::TabId;
use winter_render::renderer::PaneRect;

use crate::window::terminal::navigation::open::find_url_at;
use crate::window::{App, LastVisual, Selection};

// ========================================================================
// Data Structures
// ========================================================================

/// Transient pointer input: where the pointer is, what it is dragging, and
/// the timing state that turns clicks into double- and triple-clicks.
#[derive(Debug)]
pub(in crate::window) struct PointerState {
    /// Next instant at which a held-button selection drag near the top/bottom
    /// viewport edge is allowed to auto-scroll by another line. Throttles
    /// [`App::auto_scroll_selection`] against the ~16ms `about_to_wait` tick.
    pub(in crate::window) auto_scroll_next: Instant,
    pub(in crate::window) cursor_pos: (f32, f32),
    /// Previous cursor pixel position during a split-divider drag, or `None`
    /// when no drag is in progress. Cleared on mouse release.
    pub(in crate::window) divider_drag: Option<(f32, f32)>,
    /// The editor tab the pointer last rested on, told when it leaves.
    pub(in crate::window) hovered_editor: Option<TabId>,
    /// The editor toolbar button currently under the pointer, as `(owner_pane_id, button_id)`.
    pub(in crate::window) hovered_toolbar_button: Option<(u64, u16)>,
    /// The URL of the hyperlinked cell currently under the pointer, if any.
    /// Drives the pointer-cursor icon and Ctrl+click to open.
    pub(in crate::window) hovered_url: Option<String>,
    pub(in crate::window) last_click: Option<(Instant, f32, f32)>,
    /// The button the mouse-tracking program was last handed a press of and
    /// not yet its release: the one its motion reports name as held.
    pub(in crate::window) forwarded_button: Option<u8>,
    /// The `(row, col)` the last motion report sent to a mouse-tracking
    /// program named: motion is reported once per cell entered, as xterm
    /// does, not once per pixel the pointer moves.
    pub(in crate::window) last_motion_cell: Option<(usize, usize)>,
    pub(in crate::window) mouse_down: bool,
    /// Where the held left button went down: the pane, and the absolute
    /// `(row, col)` under the pointer at that instant (see
    /// [`winter_core::Grid::to_absolute_row`]). `None` when no button is
    /// held.
    ///
    /// A drag's selection is anchored here rather than at the first motion
    /// event, which is a different cell whenever the pointer has already
    /// travelled between the press and the first event the compositor
    /// delivers. A quick flick starts a selection lines away from where the
    /// click landed.
    pub(in crate::window) press_cell: Option<(TabId, usize, usize)>,
    /// Which pane's scrollbar is being dragged, if any. Cleared on mouse release.
    pub(in crate::window) scrollbar_drag: Option<TabId>,
    /// Whether the pointer is currently resting on a toast notification's close button.
    pub(in crate::window) toast_close_hovered: bool,
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
    ///
    /// `None` while the pointer is still on the press cell: a hand moves a
    /// pixel or two while clicking, and a selection started there would light
    /// the clicked cell, stay lit while typing, and copy its one character
    /// over the clipboard on release.
    pub(in crate::window) fn drag_anchor(
        &self,
        tab: TabId,
        motion: (usize, usize),
    ) -> Option<(usize, usize)> {
        match self.press_cell {
            Some((press_pane, row, col)) if press_pane == tab => {
                ((row, col) != motion).then_some((row, col))
            }
            _ => Some(motion),
        }
    }
}

impl Default for PointerState {
    fn default() -> Self {
        Self {
            auto_scroll_next: Instant::now(),
            cursor_pos: (0.0, 0.0),
            divider_drag: None,
            hovered_editor: None,
            hovered_toolbar_button: None,
            hovered_url: None,
            last_click: None,
            forwarded_button: None,
            last_motion_cell: None,
            mouse_down: false,
            press_cell: None,
            scrollbar_drag: None,
            toast_close_hovered: false,
        }
    }
}

/// The active selection and the Visual-mode state behind it. [`Self::span`]
/// is set either by a mouse drag or by a Visual-mode motion, so it is not
/// tied to the mode: leaving Visual mode clears it explicitly.
pub(in crate::window) struct SelectionState {
    /// The last Visual selection, restored by `gv` (see [`LastVisual`]).
    pub(in crate::window) last_visual: Option<LastVisual>,
    pub(in crate::window) span: Option<Selection>,
    /// Visual-mode anchor: the absolute `(row, col)` the selection was started
    /// from (see [`winter_core::Grid::to_absolute_row`]), so it keeps naming
    /// the same text as the view scrolls. Held viewport-relative it slid by the
    /// scroll distance and the selection detached from where `v` was pressed.
    /// `Some` only while the focused pane is in Visual mode.
    pub(in crate::window) visual_anchor: Option<(usize, usize)>,
    /// The active Visual selection kind (Block, Char, Line).
    pub(in crate::window) visual_kind: VisualKind,
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
    pub(in crate::window) fn pixel_to_cell(
        &self,
        x: f32,
        y: f32,
        pane_rect: PaneRect,
    ) -> (usize, usize) {
        let (cw, ch) = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size())
            .unwrap_or((9.0, 20.0));
        // The inverse of where the renderer draws a cell: a click in the
        // slice of a row left over above the rows is the first row's, and
        // one below them the last row's.
        pane_rect.cell_at((x, y), (cw, ch))
    }

    pub(in crate::window) fn pane_at_pixel(&self, x: f32, y: f32) -> Option<(TabId, PaneRect)> {
        for (id, rect) in self.pane_rects() {
            let pr = Self::layout_rect_to_pane(rect);
            if x >= pr.x && x < pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                return Some((id, pr));
            }
        }
        None
    }

    /// The on-screen rectangle of one shown tab, below its pane's strip.
    pub(in crate::window) fn pane_rect(&self, tab: TabId) -> Option<PaneRect> {
        self.pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .map(|(_, rect)| Self::layout_rect_to_pane(rect))
    }

    /// The cell a held drag is pointing at, clamped into the pane the drag
    /// started in.
    ///
    /// Unlike [`Self::pane_at_pixel`], this does not give up once the pointer
    /// leaves the pane. A drag that runs off the bottom edge selects through
    /// to the end of the last row, and one that runs off the top selects back
    /// to the start of the first, which is how a drag past the edge behaves in
    /// every other terminal. Resolving only the pane under the pointer froze
    /// the selection at whatever column the pointer happened to cross the edge
    /// at, so the last (or first) line came out half-selected and dragging
    /// further did nothing at all.
    pub(in crate::window) fn drag_cell_at(&self, x: f32, y: f32) -> Option<(TabId, usize, usize)> {
        if let Some((pane, rect)) = self.pane_at_pixel(x, y) {
            let (row, col) = self.pixel_to_cell(x, y, rect);
            return Some((pane, row, col));
        }
        // Outside every pane: the drag still belongs to the pane it started
        // in, named by the press, or by the span already being extended when
        // the button has been held since before this window had focus.
        let tab = self
            .pointer
            .press_cell
            .map(|(id, _, _)| id)
            .or_else(|| self.selection.span.as_ref().map(|sel| sel.tab))?;
        let rect = self.pane_rect(tab)?;
        let grid = self.open_tabs.terminal(&tab)?.grid();
        let last_row = grid.rows().saturating_sub(1);
        let last_col = grid.cols().saturating_sub(1);
        if y < rect.y {
            return Some((tab, 0, 0));
        }
        if y >= rect.y + rect.height {
            return Some((tab, last_row, last_col));
        }
        // Beside the pane rather than above or below it: the row the pointer
        // is level with still counts, with the column pinned to the edge.
        let clamped_x = x.clamp(rect.x, rect.x + rect.width);
        let (row, col) = self.pixel_to_cell(clamped_x, y, rect);
        Some((tab, row.min(last_row), col.min(last_col)))
    }

    /// The hyperlink URL of the cell currently under the pointer, if any.
    pub(in crate::window) fn hovered_link_at(&self, x: f32, y: f32) -> Option<String> {
        let (tab_id, pane_rect) = self.pane_at_pixel(x, y)?;
        let terminal = self.open_tabs.terminal(&tab_id)?;
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        find_url_at(terminal.grid(), row, col)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_click_that_never_leaves_its_cell_starts_no_selection() {
        // Regression: the jitter of a click started a selection from the
        // pressed cell to itself, which lit that one cell in the selection
        // color and kept it lit while typing.
        let pointer = PointerState {
            press_cell: Some((TabId(1), 40, 3)),
            ..Default::default()
        };
        assert_eq!(pointer.drag_anchor(TabId(1), (40, 3)), None);
        assert_eq!(
            pointer.drag_anchor(TabId(1), (40, 4)),
            Some((40, 3)),
            "one cell over is a drag"
        );
    }

    #[test]
    fn test_drag_anchor_starts_at_the_press_not_at_the_first_motion() {
        // Regression: the anchor was whatever cell the first motion event
        // landed on. Pointer motion is compressed, so a quick drag's first
        // event can arrive many rows below the click, and the selection then
        // started well past the line the user actually clicked on.
        let pointer = PointerState {
            press_cell: Some((TabId(1), 40, 3)),
            ..Default::default()
        };
        assert_eq!(pointer.drag_anchor(TabId(1), (57, 12)), Some((40, 3)));
    }

    #[test]
    fn test_a_click_lands_on_the_row_drawn_under_it() {
        // The slice of a row left over is split above and below the rows. A
        // click has to be read the same way, or it would land on a row other
        // than the one drawn under it.
        let app = App::new();
        // With no renderer the cell is the default 9 by 20, so a pane 189
        // tall holds nine rows, with 4 pixels left over above them and 5
        // below.
        let pane = PaneRect {
            height: 189.0,
            width: 200.0,
            x: 0.0,
            y: 0.0,
        };
        assert_eq!(
            app.pixel_to_cell(5.0, 43.0, pane).0,
            1,
            "just above the third row"
        );
        assert_eq!(app.pixel_to_cell(5.0, 45.0, pane).0, 2, "just inside it");
        assert_eq!(
            app.pixel_to_cell(5.0, 2.0, pane).0,
            0,
            "the slice above is the first row"
        );
        assert_eq!(
            app.pixel_to_cell(5.0, 187.0, pane).0,
            8,
            "the slice below is the last"
        );
    }

    #[test]
    fn test_drag_cell_at_runs_a_drag_off_the_edge_to_the_end_of_the_line() {
        // Regression: resolving the drag target with `pane_at_pixel` alone
        // returned `None` the moment the pointer left the pane, so a drag
        // carried past the bottom edge stopped at whatever column it happened
        // to cross at and left the last line half-selected. Past the bottom
        // the drag should reach the end of the last row, and past the top the
        // start of the first.
        let mut app = App::new();
        // The default tab lays out a single pane, `TabId(0)`.
        let terminal = TabId(0);
        app.open_tabs.insert_terminal(
            terminal,
            crate::terminal::session::Terminal::with_command(
                40,
                10,
                portable_pty::CommandBuilder::new("true"),
                winter_core::MAX_SCROLLBACK,
            )
            .expect("test pane spawn"),
        );
        app.pointer.press_cell = Some((terminal, 0, 5));
        let rect = app.pane_rect(terminal).expect("the pane is laid out");
        let (cols, rows) = {
            let grid = app.open_tabs.terminal(&terminal).unwrap().grid();
            (grid.cols(), grid.rows())
        };

        let below = app.drag_cell_at(rect.x + 10.0, rect.y + rect.height + 200.0);
        assert_eq!(below, Some((terminal, rows - 1, cols - 1)));

        let above = app.drag_cell_at(rect.x + 10.0, rect.y - 200.0);
        assert_eq!(above, Some((terminal, 0, 0)));
    }

    #[test]
    fn test_drag_anchor_falls_back_when_the_press_was_in_another_pane() {
        // A press in a different pane names a row in a different grid, so it
        // cannot anchor this pane's selection; nor can a drag with no press
        // behind it at all (a button held since before the window had focus).
        let mut pointer = PointerState {
            press_cell: Some((TabId(2), 40, 3)),
            ..Default::default()
        };
        assert_eq!(pointer.drag_anchor(TabId(1), (57, 12)), Some((57, 12)));
        pointer.press_cell = None;
        assert_eq!(pointer.drag_anchor(TabId(1), (57, 12)), Some((57, 12)));
    }
}

//! Window geometry: chrome insets, viewports, and pane hit-testing.

use std::time::Instant;

use winit::window::ResizeDirection;

use crate::config::TitleBarStyle;
use crate::model::layout::Rect;
use winter_render::renderer::PaneRect;
use winter_render::MenuStyle;

use super::App;
use super::{
    content_band, edge_resize_direction_at, snap_height_to_rows, snap_width_to_cols,
    AUTO_SCROLL_EDGE_MARGIN, AUTO_SCROLL_INTERVAL, AUTO_SCROLL_MAX_LINES_PER_TICK,
    WINDOW_RESIZE_BORDER_PX,
};

// ========================================================================
// App: geometry
// ========================================================================

impl App {
    /// Number of top cell rows reserved for the tabbar/menubar.
    pub(crate) fn top_chrome_rows(&self) -> usize {
        winter_render::tabbar_rows(self.config.menu_style)
    }
    /// The outer-edge/corner this point should resize when pressed, or `None`
    /// away from the border. Only the Modern title bar needs this: the System
    /// style keeps the OS decorations, which already carry a resize border.
    pub(crate) fn edge_resize_direction(&self, x: f32, y: f32) -> Option<ResizeDirection> {
        if self.config.title_bar_style != TitleBarStyle::Modern {
            return None;
        }
        let size = self.window.as_ref()?.inner_size();
        edge_resize_direction_at(
            x,
            y,
            size.width as f32,
            size.height as f32,
            WINDOW_RESIZE_BORDER_PX,
        )
    }
    /// Whether the status bar is actually shown this frame: either configured
    /// on, or forced on for the duration of a live `/` search, which is the
    /// only place search feedback (query text, match position) is displayed.
    /// Shared by `viewport_rect`/`resize_all_panes` (pane geometry and PTY
    /// size) and `render_frame` (what's drawn), so they always agree on how
    /// much space is reserved at the bottom of the window.
    pub(crate) fn status_bar_visible(&self) -> bool {
        self.config.status_bar.enabled || self.search_query.is_some()
    }
    pub(crate) fn viewport_rect(&self) -> PaneRect {
        let (w, h) = match (&self.window, &self.renderer) {
            (Some(win), _) => {
                let size = win.inner_size();
                (size.width as f32, size.height as f32)
            }
            (None, Some(r)) => {
                let (cols, rows) = r.grid_size();
                let (cw, ch) = r.cell_size();
                (cols as f32 * cw, rows as f32 * ch)
            }
            (None, None) => (800.0, 600.0),
        };
        // Reserve the status bar row at the bottom (when enabled) and the
        // tabbar/menubar rows at the top, so pane hit-testing and focus geometry
        // match the area actually drawn to panes; the grid is centered in
        // whatever space remains below the tabbar (must match `render_frame`).
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(0.0);
        let top_rows = self.top_chrome_rows();
        let status_enabled = self.status_bar_visible();

        let top_h_on_screen = if self.config.menu_style == MenuStyle::Modern {
            winter_render::modern_tabbar_height_px(ch)
        } else {
            top_rows as f32 * ch
        };
        let status_h = if status_enabled {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };

        // Floor to whole cell rows and center the leftover sub-row slack above
        // and below the pane band, whether or not the status bar eats into it,
        // so a window height that isn't an exact multiple of the cell height
        // never leaves a dead, un-drawable strip pinned to one edge.
        let (content_rows, top_padding) = content_band(h - top_h_on_screen - status_h, ch);
        PaneRect {
            x: 0.0,
            y: top_h_on_screen + top_padding,
            width: w,
            height: (content_rows as f32 * ch).max(1.0),
        }
    }
    /// The pane area as a layout `Rect` (same coordinates as [`PaneRect`]).
    pub(crate) fn content_viewport(&self) -> Rect {
        let vp = self.viewport_rect();
        Rect::new(vp.x, vp.y, vp.width, vp.height)
    }
    /// While a selection drag is held near the top/bottom edge of the content
    /// viewport, scroll the selection's pane one line into (or back out of)
    /// history and extend the drag's live end to the new edge row, so text
    /// outside the current page can be reached, and pulled into the
    /// selection, without the pointer leaving the window. Because `Selection`
    /// rows are absolute ([`winter_render::Grid::to_absolute_row`]),
    /// re-deriving the edge row from the post-scroll view on every call grows
    /// `end_row` further each time rather than snapping back to a fixed
    /// viewport position. Scrolling itself is throttled to one line per
    /// [`AUTO_SCROLL_INTERVAL`] via `auto_scroll_next`; a no-op when the
    /// button isn't held, there's no active selection, or the pointer isn't
    /// within [`AUTO_SCROLL_EDGE_MARGIN`] of an edge.
    pub(crate) fn auto_scroll_selection(&mut self) {
        if !self.mouse_down {
            return;
        }
        let Some(pane_id) = self.selection.as_ref().map(|sel| sel.pane) else {
            return;
        };
        let vp = self.viewport_rect();
        let (_, y) = self.cursor_pos;
        let scroll_up = if y < vp.y + AUTO_SCROLL_EDGE_MARGIN {
            true
        } else if y > vp.y + vp.height - AUTO_SCROLL_EDGE_MARGIN {
            false
        } else {
            return;
        };
        let Some(pane) = self.panes.get_mut(&pane_id) else {
            return;
        };

        // Speed scales with how deep into the edge margin the pointer sits:
        // just inside crawls, at the margin's full depth (the viewport's own
        // edge) hits AUTO_SCROLL_MAX_LINES_PER_TICK. A pointer past the margin
        // (above the viewport entirely) clamps at that same cap.
        let depth = if scroll_up {
            vp.y + AUTO_SCROLL_EDGE_MARGIN - y
        } else {
            y - (vp.y + vp.height - AUTO_SCROLL_EDGE_MARGIN)
        };
        let extra = (depth / AUTO_SCROLL_EDGE_MARGIN * (AUTO_SCROLL_MAX_LINES_PER_TICK - 1) as f32)
            .floor() as usize;
        let lines = (1 + extra).min(AUTO_SCROLL_MAX_LINES_PER_TICK);

        let now = Instant::now();
        if now >= self.auto_scroll_next {
            self.auto_scroll_next = now + AUTO_SCROLL_INTERVAL;
            let grid = pane.grid_mut();
            if scroll_up {
                grid.scroll_up_history(lines);
            } else {
                grid.scroll_down_history(lines);
            }
        }

        let grid = pane.grid();
        let (edge_row, edge_col) = if scroll_up {
            (0, 0)
        } else {
            (grid.rows().saturating_sub(1), grid.cols().saturating_sub(1))
        };
        let abs_edge_row = grid.to_absolute_row(edge_row);
        if let Some(sel) = &mut self.selection {
            sel.end_row = abs_edge_row;
            sel.end_col = edge_col;
        }
        // The view scrolls under a held pointer during edge auto-scroll, so the
        // traversal cursor must move to the drag's live end too — otherwise it
        // would freeze mid-screen while the selection grows past it.
        self.track_nav_cursor_to_mouse(pane_id, edge_row, edge_col);
        self.dirty = true;
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    pub(crate) fn layout_rect_to_pane(rect: Rect) -> PaneRect {
        // Snap the pane to the pixel grid by rounding its top-left AND
        // bottom-right corners, then deriving width/height from the rounded
        // corners. Rounding width/height independently (the obvious
        // alternative) lets two siblings of a split disagree about their shared
        // edge by 1 px — because `round(x) + round(w) != round(x + w)` — which
        // both drops the divider between them (the shared edge no longer
        // coincides) and opens a 1 px gap at divider crossings. Corner-rounding
        // keeps the shared edge exact: a split produces
        // `first.x + first.width == second.x` in the unrounded tree, so rounding
        // that same value for both panes gives identical pixel edges.
        let x = rect.x.round();
        let y = rect.y.round();
        let right = (rect.x + rect.width).round();
        let bottom = (rect.y + rect.height).round();
        PaneRect {
            x,
            y,
            width: right - x,
            height: bottom - y,
        }
    }
    /// Rounds the window size to the nearest whole row/column fit so a drag
    /// settles with zero leftover slack. Skipped while maximized/fullscreen;
    /// a no-op once already snapped, so `Resized` can't loop.
    pub(crate) fn snap_window_to_cell_grid(&mut self) {
        let Some(window) = self.window.clone() else {
            return;
        };
        if window.is_maximized() || window.fullscreen().is_some() {
            return;
        }
        let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) else {
            return;
        };
        if cw <= 0.0 || ch <= 0.0 {
            return;
        }
        let size = window.inner_size();
        let top_h_on_screen = if self.config.menu_style == MenuStyle::Modern {
            winter_render::modern_tabbar_height_px(ch)
        } else {
            self.top_chrome_rows() as f32 * ch
        };
        let status_h = if self.status_bar_visible() {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };
        let ideal_h = snap_height_to_rows(size.height as f32, top_h_on_screen, status_h, ch) as u32;
        let ideal_w =
            snap_width_to_cols(size.width as f32, 2.0 * winter_render::PANE_H_PAD, cw) as u32;
        if ideal_h == size.height && ideal_w == size.width {
            return;
        }
        let applied = window.request_inner_size(winit::dpi::PhysicalSize::new(ideal_w, ideal_h));
        // Some platforms apply the requested size synchronously and never send
        // the follow-up `Resized` that would otherwise drive this; without
        // this, the GPU surface and pane grids are left at the old size while
        // the OS-reported window is already the new one, showing as a gap.
        if let Some(actual) = applied {
            let scale_factor = window.scale_factor();
            if let Some(renderer) = &mut self.renderer {
                renderer.resize(actual.width, actual.height, scale_factor);
            }
            self.resize_all_panes();
        }
    }
}

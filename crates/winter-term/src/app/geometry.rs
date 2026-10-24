//! Window geometry: chrome insets, viewports, and pane hit-testing.

use std::time::Instant;

use winit::window::ResizeDirection;

use crate::config::TitleBarStyle;
use crate::model::layout::Rect;
use winter_render::renderer::PaneRect;
use winter_render::MenuStyle;

use super::App;
use super::{
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
        self.config.status_bar.enabled || self.search.query.is_some()
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
            winter_render::modern_titlebar_height_px(ch)
        } else {
            top_rows as f32 * ch
        };
        let status_h = if status_enabled {
            winter_render::STATUS_BAR_HEIGHT * ch
        } else {
            0.0
        };

        // Every pixel between the title bar and the status bar belongs to the
        // pane band, so split dividers and pane backgrounds reach both. Panes
        // floor their own row counts, so a sub-row remainder is just padding at
        // the foot of the lowest panes.
        PaneRect {
            x: 0.0,
            y: top_h_on_screen,
            width: w,
            height: (h - top_h_on_screen - status_h).max(1.0),
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
    /// outside the current editor can be reached, and pulled into the
    /// selection, without the pointer leaving the window. Because `Selection`
    /// rows are absolute ([`winter_render::Grid::to_absolute_row`]),
    /// re-deriving the edge row from each post-scroll view grows `end_row`
    /// further each time rather than snapping back to a fixed viewport
    /// position. Scrolling itself is throttled to one line per
    /// [`AUTO_SCROLL_INTERVAL`] via `pointer.auto_scroll_next`; a no-op when the
    /// button isn't held, there's no active selection, or the pointer isn't
    /// within [`AUTO_SCROLL_EDGE_MARGIN`] of an edge.
    ///
    /// Extending the selection is a *consequence* of scrolling, so it happens
    /// only on a tick that actually moved the view. A tick that scrolled
    /// nothing leaves the span alone and lets the pointer's own motion own the
    /// live end: otherwise, with no history left to consume, this would pin
    /// `end` to the edge cell ~60 times a second and the drag could never grow
    /// past the cell it started on. That is the common case on the alternate
    /// screen, which has no history at all, so a drag over the top row of any
    /// full-screen app collapsed to a single cell.
    pub(crate) fn auto_scroll_selection(&mut self) {
        if !self.pointer.mouse_down {
            return;
        }
        let Some(tab_id) = self.selection.span.as_ref().map(|sel| sel.tab) else {
            return;
        };
        let vp = self.viewport_rect();
        let (_, y) = self.pointer.cursor_pos;
        let scroll_up = if y < vp.y + AUTO_SCROLL_EDGE_MARGIN {
            true
        } else if y > vp.y + vp.height - AUTO_SCROLL_EDGE_MARGIN {
            false
        } else {
            return;
        };
        let Some(terminal) = self.open_tabs.terminal_mut(&tab_id) else {
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
        if now < self.pointer.auto_scroll_next {
            return;
        }
        self.pointer.auto_scroll_next = now + AUTO_SCROLL_INTERVAL;
        let grid = terminal.grid_mut();
        let before = grid.scroll_offset();
        if scroll_up {
            grid.scroll_up_history(lines);
        } else {
            grid.scroll_down_history(lines);
        }
        if grid.scroll_offset() == before {
            return;
        }

        let grid = terminal.grid();
        let (edge_row, edge_col) = if scroll_up {
            (0, 0)
        } else {
            (grid.rows().saturating_sub(1), grid.cols().saturating_sub(1))
        };
        let abs_edge_row = grid.to_absolute_row(edge_row);
        if let Some(sel) = &mut self.selection.span {
            sel.end_row = abs_edge_row;
            sel.end_col = edge_col;
        }
        // The view scrolls under a held pointer during edge auto-scroll, so the
        // traversal cursor must move to the drag's live end too: otherwise it
        // would freeze mid-screen while the selection grows past it.
        self.track_nav_cursor_to_mouse(tab_id, edge_row, edge_col);
        self.dirty = true;
    }
    pub(crate) fn layout_rect_to_pane(rect: Rect) -> PaneRect {
        // Snap the pane to the pixel grid by rounding its top-left AND
        // bottom-right corners, then deriving width/height from the rounded
        // corners. Rounding width/height independently (the obvious
        // alternative) lets two siblings of a split disagree about their shared
        // edge by 1 px, because `round(x) + round(w) != round(x + w)`, which
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
}

// ========================================================================
// Functions
// ========================================================================

/// The outer-edge/corner resize direction for a point at `(x, y)` in a `w`x`h`
/// window, or `None` once it's more than `border` from every edge. Corners
/// win over edges, so a top-left pixel resolves to `NorthWest`, not `North`.
pub(crate) fn edge_resize_direction_at(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    border: f32,
) -> Option<ResizeDirection> {
    let west = x <= border;
    let east = x >= w - border;
    let north = y <= border;
    let south = y >= h - border;
    if north && west {
        Some(ResizeDirection::NorthWest)
    } else if north && east {
        Some(ResizeDirection::NorthEast)
    } else if south && west {
        Some(ResizeDirection::SouthWest)
    } else if south && east {
        Some(ResizeDirection::SouthEast)
    } else if west {
        Some(ResizeDirection::West)
    } else if east {
        Some(ResizeDirection::East)
    } else if north {
        Some(ResizeDirection::North)
    } else if south {
        Some(ResizeDirection::South)
    } else {
        None
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::editor;
    use crate::app::overlay::Overlay;
    use crate::app::test_support::*;
    use crate::app::Selection;
    use crate::model::layout::TabId;
    use crate::terminal::session::Terminal;

    #[test]
    fn test_edge_resize_direction_at_prefers_corners_over_edges() {
        let (w, h, border) = (100.0, 80.0, 6.0);
        // Each corner's border band overlaps both an X and a Y edge band; the
        // diagonal direction must win over collapsing to just North/South or
        // just West/East.
        assert_eq!(
            edge_resize_direction_at(0.0, 0.0, w, h, border),
            Some(ResizeDirection::NorthWest)
        );
        assert_eq!(
            edge_resize_direction_at(w, 0.0, w, h, border),
            Some(ResizeDirection::NorthEast)
        );
        assert_eq!(
            edge_resize_direction_at(0.0, h, w, h, border),
            Some(ResizeDirection::SouthWest)
        );
        assert_eq!(
            edge_resize_direction_at(w, h, w, h, border),
            Some(ResizeDirection::SouthEast)
        );
        // Mid-edge points (away from any corner band) resolve to a single
        // cardinal direction.
        assert_eq!(
            edge_resize_direction_at(w / 2.0, 0.0, w, h, border),
            Some(ResizeDirection::North)
        );
        assert_eq!(
            edge_resize_direction_at(w / 2.0, h, w, h, border),
            Some(ResizeDirection::South)
        );
        assert_eq!(
            edge_resize_direction_at(0.0, h / 2.0, w, h, border),
            Some(ResizeDirection::West)
        );
        assert_eq!(
            edge_resize_direction_at(w, h / 2.0, w, h, border),
            Some(ResizeDirection::East)
        );
        // The window's interior, beyond the border on every side, resizes nothing.
        assert_eq!(
            edge_resize_direction_at(w / 2.0, h / 2.0, w, h, border),
            None
        );
    }

    #[test]
    fn test_status_bar_visible_forces_on_during_search_even_when_configured_off() {
        let mut app = App::new();
        app.config.status_bar.enabled = false;
        assert!(!app.status_bar_visible());

        app.search.query = Some("foo".to_string());
        assert!(app.status_bar_visible());

        app.search.query = None;
        assert!(!app.status_bar_visible());
    }

    #[test]
    fn test_status_bar_visible_stays_on_when_configured_on_regardless_of_search() {
        let mut app = App::new();
        app.config.status_bar.enabled = true;
        assert!(app.status_bar_visible());

        app.search.query = Some("foo".to_string());
        assert!(app.status_bar_visible());
    }

    #[test]
    fn test_a_question_leaves_a_hidden_status_bar_hidden() {
        // Questions are asked in a dialog, so nothing about one needs the
        // status bar.
        let mut app = App::new();
        app.config.status_bar.enabled = false;
        assert!(!app.status_bar_visible());

        app.overlay = Some(Overlay::ThemeName(
            crate::model::textedit::field::TextField::line("custom"),
        ));
        assert!(!app.status_bar_visible());

        let prompt = editor::ActivePrompt {
            buttons: None,
            field: crate::model::textedit::field::TextField::line("query"),
            recall: crate::model::textedit::recall::Recall::default(),
            tab: TabId(0),
            request: crate::model::editor::PromptRequest {
                initial: String::new(),
                label: "search: ".to_string(),
                mode: crate::model::editor::PromptMode::Text,
                tag: "search",
            },
        };
        app.overlay = Some(Overlay::EditorPrompt(prompt));
        assert!(!app.status_bar_visible());
    }

    #[test]
    fn test_layout_rect_conversion() {
        let rect = Rect::new(10.0, 20.0, 400.0, 300.0);
        let pane_rect = App::layout_rect_to_pane(rect);
        assert_eq!(pane_rect.x, 10.0);
        assert_eq!(pane_rect.y, 20.0);
    }

    /// A pane holding many pages of scrollback, so auto-scroll runs against real
    /// history rather than the live-bottom edge case. The lines are written
    /// straight into the grid, because waiting on a shell to emit them failed
    /// whenever the machine was busy.
    /// A pointer height one pixel inside the auto-scroll margin at the top of
    /// the viewport, wherever the title bar leaves that.
    fn just_inside_top_edge(app: &App) -> f32 {
        app.viewport_rect().y + AUTO_SCROLL_EDGE_MARGIN - 1.0
    }

    #[test]
    fn test_auto_scroll_selection_scrolls_up_when_pointer_near_top_edge() {
        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(id, pane_with_scrollback());
        app.pointer.mouse_down = true;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            tab: id,
        });
        // Just inside the top edge margin of the (window-less, so default
        // 800x600) viewport: barely deep enough to scroll, so one step moves
        // the minimum one line (see test_auto_scroll_selection_speed_scales_
        // with_edge_depth for the deep-in-margin case).
        app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));

        app.auto_scroll_selection();

        let terminal = &app.open_tabs.terminal(&id).unwrap();
        assert_eq!(
            terminal.grid().scroll_offset(),
            1,
            "should scroll one line into history"
        );
        let sel = app.selection.span.as_ref().expect("selection still active");
        assert_eq!(
            (sel.end_row, sel.end_col),
            (terminal.grid().to_absolute_row(0), 0),
            "drag end follows the new top row's absolute address"
        );
    }

    #[test]
    fn test_auto_scroll_selection_expands_further_back_each_tick() {
        // Regression: auto-scroll used to pin the drag's live end to the
        // viewport's fixed row 0, so each tick reinterpreted that same row
        // number as whatever content had just scrolled into it instead of
        // growing the selection to cover everything scrolled past. Since
        // `Selection` rows are absolute, the end should move strictly
        // further from the start on every successful scroll tick.
        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(id, pane_with_scrollback());
        app.pointer.mouse_down = true;
        let start_abs = app
            .open_tabs
            .terminal(&id)
            .unwrap()
            .grid()
            .to_absolute_row(5);
        app.selection.span = Some(Selection {
            block: false,
            start_row: start_abs,
            start_col: 0,
            end_row: start_abs,
            end_col: 0,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));

        let mut ends = Vec::new();
        for _ in 0..3 {
            app.pointer.auto_scroll_next = Instant::now(); // Bypass the interval throttle.
            app.auto_scroll_selection();
            ends.push(app.selection.span.as_ref().unwrap().end_row);
        }

        assert!(
            ends[0] > ends[1] && ends[1] > ends[2],
            "end_row should move further back each tick, not reset: {ends:?}"
        );
        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            3
        );
    }

    #[test]
    fn test_auto_scroll_selection_speed_scales_with_edge_depth() {
        // Regression: the drag auto-scroll used to move a fixed one line per
        // tick no matter how deep into the edge margin the pointer sat, which
        // reads as sluggish when reaching for text well above the viewport.
        // The step now scales with depth: barely inside crawls, near the
        // viewport's own edge moves AUTO_SCROLL_MAX_LINES_PER_TICK.
        let shallow = {
            let mut app = App::new();
            let id = TabId(1);
            app.open_tabs.insert_terminal(id, pane_with_scrollback());
            app.pointer.mouse_down = true;
            app.selection.span = Some(Selection {
                block: false,
                start_row: 5,
                start_col: 0,
                end_row: 5,
                end_col: 0,
                tab: id,
            });
            app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));
            app.auto_scroll_selection();
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset()
        };
        let deep = {
            let mut app = App::new();
            let id = TabId(1);
            app.open_tabs.insert_terminal(id, pane_with_scrollback());
            app.pointer.mouse_down = true;
            app.selection.span = Some(Selection {
                block: false,
                start_row: 5,
                start_col: 0,
                end_row: 5,
                end_col: 0,
                tab: id,
            });
            app.pointer.cursor_pos = (10.0, 0.0);
            app.auto_scroll_selection();
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset()
        };
        assert_eq!(shallow, 1, "barely inside the margin scrolls one line");
        assert_eq!(
            deep, AUTO_SCROLL_MAX_LINES_PER_TICK,
            "at the viewport's own edge the step hits its cap"
        );
    }

    #[test]
    fn test_selected_text_covers_every_line_after_auto_scrolling_past_one_page() {
        // Regression: `selected_text` used to read rows via `visible_cell`,
        // which only resolves the currently-visible editor (`0..rows`). Once a
        // drag's absolute span grew past one page via auto-scroll, rows
        // outside that editor read back as blanks instead of the real
        // scrolled-past text. `absolute_cell` fixes this by addressing the
        // scrollback/live buffer directly, independent of scroll position.
        //
        // Content is written directly to the grid (no live shell) so each
        // row holds a known, distinguishable number instead of depending on
        // PTY timing.
        let mut terminal =
            Terminal::with_command(10, 3, portable_pty::CommandBuilder::new("true"), 1000)
                .expect("test terminal spawn");
        {
            let grid = terminal.grid_mut();
            for n in 0..20 {
                for ch in n.to_string().chars() {
                    grid.print(ch);
                }
                grid.carriage_return();
                grid.line_feed();
            }
        }
        let rows = terminal.grid().rows();

        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(id, terminal);
        app.pointer.mouse_down = true;
        // Anchor the drag on the live view's current top row (still holding
        // real numbers, not the fresh blank row the cursor just moved to).
        let start_abs = app
            .open_tabs
            .terminal(&id)
            .unwrap()
            .grid()
            .to_absolute_row(0);
        app.selection.span = Some(Selection {
            block: false,
            start_row: start_abs,
            start_col: 0,
            end_row: start_abs,
            end_col: 0,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));

        // Scroll well past both a single page's worth of rows and the whole
        // available scrollback, so the offset saturates at the oldest line.
        for _ in 0..(rows + 25) {
            app.pointer.auto_scroll_next = Instant::now();
            app.auto_scroll_selection();
        }
        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            app.open_tabs.terminal(&id).unwrap().grid().scrollback_len(),
            "should have scrolled all the way back"
        );

        let sel = app.selection.span.as_ref().unwrap();
        let expected_lines = sel.start_row.max(sel.end_row) - sel.start_row.min(sel.end_row) + 1;
        let text = app.selected_text().expect("selection has content");
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(
            lines.len(),
            expected_lines,
            "should cover every scrolled-through line, not just one page: {lines:?}"
        );
        // The oldest scrolled-into row is "0" (the first line printed), not
        // a blank filler row `visible_cell` would have produced past the
        // live editor's bound.
        assert_eq!(lines[0].trim(), "0");
    }

    #[test]
    fn test_auto_scroll_selection_throttles_repeated_ticks() {
        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(id, pane_with_scrollback());
        app.pointer.mouse_down = true;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));

        app.auto_scroll_selection();
        app.auto_scroll_selection();

        // The second call lands before `pointer.auto_scroll_next`, so it must not
        // advance the scrollback a second time.
        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            1
        );
    }

    #[test]
    fn test_auto_scroll_selection_noop_when_pointer_not_near_an_edge() {
        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(id, pane_with_scrollback());
        app.pointer.mouse_down = true;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, 300.0);

        app.auto_scroll_selection();

        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            0
        );
    }

    #[test]
    fn test_auto_scroll_selection_noop_when_mouse_not_held() {
        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(id, pane_with_scrollback());
        app.pointer.mouse_down = false;
        app.selection.span = Some(Selection {
            block: false,
            start_row: 5,
            start_col: 0,
            end_row: 5,
            end_col: 0,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, 1.0);

        app.auto_scroll_selection();

        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            0
        );
    }

    #[test]
    fn test_auto_scroll_selection_leaves_the_drag_end_alone_on_the_alt_screen() {
        // Regression: the alternate screen has no history of its own, so an
        // edge auto-scroll tick moves nothing. It used to extend the drag to
        // the edge cell anyway, on every ~16ms tick, overwriting what the
        // pointer's own motion had just set. A drag over the top row of any
        // full-screen app (vim, htop, a TUI agent) collapsed to the cell it
        // started on and would not grow.
        let mut app = App::new();
        let id = TabId(1);
        let mut terminal = pane_with_scrollback();
        terminal.grid_mut().enter_alt_screen();
        app.open_tabs.insert_terminal(id, terminal);
        app.pointer.mouse_down = true;
        let anchor = app
            .open_tabs
            .terminal(&id)
            .unwrap()
            .grid()
            .to_absolute_row(0);
        app.selection.span = Some(Selection {
            block: false,
            start_row: anchor,
            start_col: 2,
            end_row: anchor,
            end_col: 30,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));

        app.auto_scroll_selection();

        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scroll_offset(),
            0,
            "the retained scrollback belongs to the primary buffer, so a \
             full-screen app's viewport must not scroll into it"
        );
        let sel = app.selection.span.as_ref().expect("selection still active");
        assert_eq!(
            (sel.end_row, sel.end_col),
            (anchor, 30),
            "a tick that scrolled nothing leaves the drag's live end to the pointer"
        );
    }

    #[test]
    fn test_auto_scroll_selection_leaves_the_drag_end_alone_once_history_runs_out() {
        // The same failure on the primary screen: once the view is already at
        // the oldest retained row there is nothing left to scroll, and the
        // drag must keep following the pointer instead of being pinned to the
        // top-left cell.
        let mut app = App::new();
        let id = TabId(1);
        app.open_tabs.insert_terminal(
            id,
            Terminal::with_command(
                40,
                10,
                portable_pty::CommandBuilder::new("true"),
                winter_render::MAX_SCROLLBACK,
            )
            .expect("test pane spawn"),
        );
        assert_eq!(
            app.open_tabs.terminal(&id).unwrap().grid().scrollback_len(),
            0,
            "fixture needs a pane with no history to consume"
        );
        app.pointer.mouse_down = true;
        let anchor = app
            .open_tabs
            .terminal(&id)
            .unwrap()
            .grid()
            .to_absolute_row(0);
        app.selection.span = Some(Selection {
            block: false,
            start_row: anchor,
            start_col: 2,
            end_row: anchor,
            end_col: 30,
            tab: id,
        });
        app.pointer.cursor_pos = (10.0, just_inside_top_edge(&app));

        app.auto_scroll_selection();

        let sel = app.selection.span.as_ref().expect("selection still active");
        assert_eq!((sel.end_row, sel.end_col), (anchor, 30));
    }
}

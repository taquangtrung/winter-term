//! Where the pointer goes: presses and releases, motion and drags, and the
//! wheel, routed to the tabbar, menus, strips, status bar, or the pane under
//! the pointer.

use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::event_loop::ActiveEventLoop;
use winit::window::CursorIcon;

use crate::model::layout::TabId;
use crate::model::paint::Point;

use crate::window::strip::{StripHit, DRAG_THRESHOLD_PX};
use crate::window::{
    App, Selection, APPROX_CELL_HEIGHT, SCROLLBAR_CLICK_WIDTH, SCROLL_LINES_PER_WHEEL_NOTCH,
};

// ========================================================================
// Constants
// ========================================================================

/// Longest gap between two presses that still reads as a double click.
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

/// Farthest, in pixels, a second press may land from the first and still
/// count as a double click.
const DOUBLE_CLICK_SLOP: f32 = 5.0;

/// How long apart the rows of a wheel's turn reach an editor: a frame.
const WHEEL_FRAME: Duration = Duration::from_millis(16);

/// How many frames a wheel's rows are spread over at most: a faster spin
/// queues more rows, and each frame then takes a bigger share to catch up.
const WHEEL_FRAMES: isize = 3;

/// How near a whole row a sum of a wheel's fractions counts as one: ten
/// tenths of a notch, added in floating point, fall a hair short.
const WHEEL_ROW_SLACK: f64 = 1e-4;

// ========================================================================
// Data Structures
// ========================================================================

/// A wheel's rows on their way to an editor, and a touchpad's part row.
#[derive(Debug, Default)]
pub(in crate::window) struct Wheel {
    /// The part of a row a touchpad has scrolled and no editor heard yet.
    pub(in crate::window) fraction: f64,
    /// The editor the rows go to, the rows left (positive up), and when
    /// the next of them is due.
    queued: Option<(TabId, isize, Instant)>,
}

// ========================================================================
// Wheel
// ========================================================================

impl Wheel {
    /// Queue `rows` more for the editor in `tab`: added to the ones on their
    /// way while it is the same editor and the same way, else in their
    /// place.
    fn queue(&mut self, tab: TabId, rows: isize) {
        let left = match self.queued {
            Some((queued, left, _)) if queued == tab && left.signum() == rows.signum() => left,
            _ => 0,
        };
        self.queued = Some((tab, left + rows, Instant::now()));
    }
}

// ========================================================================
// App: mouse routing
// ========================================================================

impl App {
    /// A left press on grid cell `(row, col)` of `tab`, at the pointer's
    /// current pixel position. It anchors any drag that follows at the press
    /// rather than at the first motion event (see `PointerState::press_cell`)
    /// and turns a quick second press into a word selection. The traversal
    /// cursor is left where it is: only a drag moves it, so a plain click in
    /// Normal mode does not make the block cursor jump to the pointer.
    fn press_grid_cell(&mut self, tab: TabId, row: usize, col: usize) {
        self.pointer.press_cell = self
            .selection_grid(tab)
            .map(|grid| (tab, grid.to_absolute_row(row), col));
        if self.take_double_click() {
            self.select_word_at(tab, row, col);
        }
    }

    /// Whether a press now, where the pointer is, ends a double click: soon
    /// enough after the last and near enough to it. Either way the press is
    /// remembered as the first of the next.
    fn take_double_click(&mut self) -> bool {
        let (x, y) = self.pointer.cursor_pos;
        let now = Instant::now();
        let double = self
            .pointer
            .last_click
            .is_some_and(|(prev_time, prev_x, prev_y)| {
                let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
                now.duration_since(prev_time) < DOUBLE_CLICK_INTERVAL && dist < DOUBLE_CLICK_SLOP
            });
        self.pointer.last_click = Some((now, x, y));
        double
    }

    /// Route a mouse button press or release. The tabbar and any open menu
    /// take precedence over the panes, so a click on chrome never reaches a
    /// pane underneath it.
    pub(in crate::window) fn on_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        event_loop: &ActiveEventLoop,
    ) {
        let focused = self.layout().focused();

        // Tabbar/menubar clicks take precedence over the panes (including
        // mouse-tracking apps), and any click resolves an open menu.
        if let (ElementState::Pressed, MouseButton::Left) = (state, button) {
            let (x, y) = self.pointer.cursor_pos;
            if let Some(direction) = self.edge_resize_direction(x, y) {
                if let Some(window) = &self.window {
                    let _ = window.drag_resize_window(direction);
                }
                return;
            }
            if self.chrome.menus.open.is_none() && self.press_notifications(x, y) {
                return;
            }
            // A click on the find popup is the popup's, not the pane's under it.
            if self.renderer.as_ref().is_some_and(|r| r.hits().find_at(x, y)) {
                return;
            }
            // Confirmation dialog buttons or input dialog clicks
            if let Some(btn_idx) = self.renderer.as_ref().and_then(|r| r.hits().confirm_button_at(x, y)) {
                self.click_dialog_button(btn_idx);
                self.quit_if_requested(event_loop);
                self.dirty = true;
                return;
            }
            if self.renderer.as_ref().is_some_and(|r| r.hits().input_at(x, y)) {
                return;
            }
            if (self.chrome.menus.open.is_some() || y < self.top_chrome_height())
                && self.handle_tabbar_click(x, y)
            {
                if self.quit_if_requested(event_loop) {
                    return;
                }
                self.dirty = true;
                return;
            }
        }

        // A press on a pane's strip of tabs belongs to the strip: it shows,
        // closes, or opens a tab rather than reaching the pane below it. A
        // split divider running past the strip still wins, so the drag that
        // resizes panes can start anywhere along it.
        if state == ElementState::Pressed
            && matches!(button, MouseButton::Left | MouseButton::Middle)
        {
            let (x, y) = self.pointer.cursor_pos;
            let on_divider = self
                .layout()
                .divider_at(x, y, self.content_viewport())
                .is_some();
            if !on_divider && self.handle_strip_press(x, y, button == MouseButton::Middle) {
                if self.quit_if_requested(event_loop) {
                    return;
                }
                if button == MouseButton::Left {
                    self.pointer.mouse_down = true;
                }
                self.dirty = true;
                return;
            }
        }

        // Ctrl+click on a hyperlinked cell or printed URL/file opens it.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.input.modifiers.state().control_key()
        {
            let (x, y) = self.pointer.cursor_pos;
            if self.open_file_at_pixel(x, y) {
                self.dirty = true;
                return;
            }
            // A `file://` link was the file above, if it is a local one; the
            // OS is handed only the schemes safe to hand it.
            if let Some(url) = &self.pointer.hovered_url {
                if crate::terminal::session::is_os_openable(url) {
                    let _ = open::that_detached(url);
                    return;
                }
            }
        }

        // Plain clicks always drive Winter's own selection, even over a
        // full-screen app (vim, htop) that has turned on mouse reporting.
        // Hold Shift to forward the click to that app's mouse handling instead.
        // A program that was handed a press is handed its release too, even
        // if Shift went up first, or it would see the button held forever.
        let completes_forwarded_press =
            state == ElementState::Released && self.pointer.forwarded_button.is_some();
        let mouse_active = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_tracking())
            && (self.input.modifiers.state().shift_key() || completes_forwarded_press);

        if mouse_active {
            self.forward_mouse_event(state, button, focused);
            return;
        }

        // The side buttons step back and forth through the places the focused
        // editor has been, as a browser's do; with Ctrl, through the pane's
        // tabs instead.
        if state == ElementState::Pressed
            && matches!(button, MouseButton::Back | MouseButton::Forward)
        {
            let back = button == MouseButton::Back;
            if self.input.modifiers.state().control_key() {
                self.cycle_tab(!back);
                self.dirty = true;
            } else if self.offer_side_button_to_editor(focused, back) {
                self.dirty = true;
            }
            return;
        }

        // Right-click: paste if configured, otherwise open the context menu.
        if state == ElementState::Pressed && button == MouseButton::Right {
            if self.config.paste_on_right_click {
                self.paste_from_clipboard();
            } else {
                let (x, y) = self.pointer.cursor_pos;
                self.open_context_menu(x, y);
                self.dirty = true;
            }
            return;
        }

        // Any left-press dismisses an open context menu before other handling.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.chrome.menus.context_pos.is_some()
        {
            let (x, y) = self.pointer.cursor_pos;
            let surface_w = self.viewport_rect().width;
            if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
                let tabbar = self.build_top_tabbar();
                let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
                if let winter_render::TabbarHit::ContextMenuItem(i) = hit {
                    if let Some(action) = self.chrome.context_entry(i) {
                        self.close_context_menu();
                        self.run_context_action(action);
                        self.dirty = true;
                        return;
                    }
                }
            }
            self.close_context_menu();
        }

        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => {
                self.pointer.mouse_down = true;
                self.selection.span = None;

                let (x, y) = self.pointer.cursor_pos;

                // Divider click: start a drag; skip scrollbar/focus changes.
                let on_divider = {
                    let viewport = self.content_viewport();
                    self.layout().divider_at(x, y, viewport).is_some()
                };
                if on_divider {
                    self.pointer.divider_drag = Some((x, y));
                } else {
                    // Scrollbar click: right-edge strip of a pane navigates scrollback.
                    'scroll: for (id, rect) in self.pane_rects() {
                        let pr = Self::layout_rect_to_pane(rect);
                        let sb_x = pr.x + pr.width - SCROLLBAR_CLICK_WIDTH;
                        if x >= sb_x && x <= pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                            if let Some(terminal) = self.open_tabs.terminal_mut(&id) {
                                let sbl = terminal.grid().scrollback_len();
                                if sbl > 0 {
                                    let rows = terminal.grid().rows();
                                    let total = (rows + sbl) as f32;
                                    let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                                    let top_virtual = (frac * total) as usize;
                                    let new_offset = sbl.saturating_sub(top_virtual);
                                    terminal.grid_mut().set_scroll_offset(new_offset);
                                    self.pointer.scrollbar_drag = Some(id);
                                    self.dirty = true;
                                    break 'scroll;
                                }
                            }
                        }
                    }

                    if let Some((tab_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        self.set_focus(tab_id);
                        // An editor's toolbar is drawn in the window's font,
                        // off the pane's cells, so a press on it is told
                        // which button by where the button was drawn.
                        let button = self
                            .renderer
                            .as_ref()
                            .and_then(|renderer| renderer.hits().toolbar_button_at(x, y))
                            .filter(|(owner, _)| *owner == tab_id.0);
                        if let Some((_, button)) = button {
                            if let Some(slot) = self.open_tabs.editor_mut(&tab_id) {
                                let outcome = slot.editor.on_toolbar(button);
                                self.act_on_editor_outcome(tab_id, outcome);
                                self.pointer.press_cell = None;
                                self.dirty = true;
                                return;
                            }
                        }
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        // An editor tab owns what is drawn there, so the click is
                        // its to answer before the grid's.
                        let at = Point {
                            col,
                            drag: false,
                            row,
                        };
                        if self.offer_mouse_to_editor(tab_id, at) {
                            self.pointer.press_cell = None;
                            // The column cursor goes to the cell clicked,
                            // on the row the click took the editor's cursor
                            // to.
                            self.place_editor_column(tab_id, col);
                            // A third press starts over rather than ending
                            // a second double click.
                            if self.take_double_click() {
                                self.pointer.last_click = None;
                                self.offer_double_click_to_editor(tab_id, at);
                            }
                            self.dirty = true;
                            return;
                        }
                        self.press_grid_cell(tab_id, row, col);
                    }
                }

                self.dirty = true;
            }
            (ElementState::Released, MouseButton::Left) => {
                self.pointer.mouse_down = false;
                self.pointer.divider_drag = None;
                self.pointer.press_cell = None;
                self.pointer.scrollbar_drag = None;
                self.finalize_tab_drag();
                self.copy_selection();
                self.copy_selection_to_primary();
                if let Some(window) = &self.window {
                    window.set_cursor(CursorIcon::Default);
                }
                self.dirty = true;
            }
            (ElementState::Pressed, MouseButton::Middle) => {
                self.paste_from_primary();
            }
            _ => {}
        }
    }
    /// Track the pointer: hover highlights, drag-selection extension, split
    /// dragging, and the resize-edge cursor shape.
    /// Light the part of the status bar's right side the pointer is on, and
    /// put out the one it left.
    fn hover_status_bar(&mut self, x: f32, y: f32) {
        let hover = match self.status_bar_visible() {
            true => self
                .renderer
                .as_ref()
                .and_then(|renderer| renderer.hits().status_hover_at(x, y)),
            false => None,
        };
        if hover != self.chrome.status_hover {
            self.chrome.status_hover = hover;
            self.dirty = true;
        }
    }

    /// Tell the editor under the pointer the cell it rests on, and the one
    /// it rested on before that it has left.
    fn hover_editor(&mut self, x: f32, y: f32) {
        let under = self
            .pane_at_pixel(x, y)
            .filter(|(id, _)| self.open_tabs.editor(id).is_some())
            .map(|(id, rect)| (id, self.pixel_to_cell(x, y, rect)));
        let left = self
            .pointer
            .hovered_editor
            .filter(|tab| under.map(|(id, _)| id) != Some(*tab));
        if let Some(tab) = left {
            self.dirty |= self.offer_hover_to_editor(tab, None);
        }
        self.pointer.hovered_editor = under.map(|(id, _)| id);
        if let Some((tab, (row, col))) = under {
            let at = Point {
                col,
                drag: false,
                row,
            };
            self.dirty |= self.offer_hover_to_editor(tab, Some(at));
        }
    }

    /// Tell the editor the pointer rested on that it has left the window.
    pub(in crate::window) fn on_cursor_left(&mut self) {
        if let Some(tab) = self.pointer.hovered_editor.take() {
            self.dirty |= self.offer_hover_to_editor(tab, None);
        }
    }

    pub(in crate::window) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let x = position.x as f32;
        let y = position.y as f32;
        self.pointer.cursor_pos = (x, y);
        // A toast stays up while the pointer rests on it.
        self.hover_toasts(x, y);
        self.hover_status_bar(x, y);

        // Divider drag: highest priority, blocks selection and PTY forwarding.
        if self.pointer.mouse_down {
            if let Some((prev_x, prev_y)) = self.pointer.divider_drag {
                let dx = x - prev_x;
                let dy = y - prev_y;
                let viewport = self.content_viewport();
                if self
                    .layout_mut()
                    .drag_divider(prev_x, prev_y, dx, dy, viewport)
                {
                    self.pointer.divider_drag = Some((x, y));
                    self.resize_all_panes();
                    self.dirty = true;
                }
                return;
            }

            // Scrollbar drag: update scroll position proportionally.
            if let Some(sb_pane_id) = self.pointer.scrollbar_drag {
                if let Some((_, rect)) = self
                    .pane_rects()
                    .into_iter()
                    .find(|(id, _)| *id == sb_pane_id)
                {
                    let pr = Self::layout_rect_to_pane(rect);
                    if let Some(terminal) = self.open_tabs.terminal_mut(&sb_pane_id) {
                        let sbl = terminal.grid().scrollback_len();
                        if sbl > 0 {
                            let rows = terminal.grid().rows();
                            let total = (rows + sbl) as f32;
                            let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                            let top_virtual = (frac * total) as usize;
                            terminal
                                .grid_mut()
                                .set_scroll_offset(sbl.saturating_sub(top_virtual));
                            self.dirty = true;
                        }
                    }
                }
                return;
            }
        }

        if self.chrome.menus.open.is_some() {
            self.update_menu_hover(x, y);
        }
        if self.chrome.menus.context_pos.is_some() {
            self.update_context_menu_hover(x, y);
        }

        // Update tabbar hover state so the renderer can show hover highlights.
        if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
            let surface_w = self.viewport_rect().width;
            let tabbar = self.build_top_tabbar();
            let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
            if hit != self.tabs.hover {
                self.tabs.hover = hit;
                self.dirty = true;
            }
        }

        let tb_button = self
            .renderer
            .as_ref()
            .and_then(|renderer| renderer.hits().toolbar_button_at(x, y));
        if tb_button != self.pointer.hovered_toolbar_button {
            self.pointer.hovered_toolbar_button = tb_button;
            self.dirty = true;
        }

        // The pane strips' hover, which also carries a tab's full title as a
        // tooltip, since a strip may have had to cut it short.
        let strip_before = self.tabs.strip_hover;
        self.update_strip_hover(x, y);
        if self.tabs.strip_hover != strip_before {
            self.tabs.hover_anchor = match self.tabs.strip_hover {
                StripHit::Tab(tab) => self.strip_tab_region(tab, x, y),
                _ => None,
            };
            self.update_window_title();
            self.dirty = true;
        }

        // With Shift held (Winter's "this goes to the app" chord), motion
        // reaches a mouse-tracking program: drags under drag tracking
        // (?1002), and bare motion too under any-event tracking (?1003).
        let focused = self.layout().focused();
        let reports_motion = self.open_tabs.terminal(&focused).is_some_and(|p| {
            if self.pointer.forwarded_button.is_some() {
                p.mouse_drag_tracking()
            } else {
                p.mouse_any_event()
            }
        });
        if reports_motion && self.input.modifiers.state().shift_key() {
            self.forward_mouse_motion(focused);
            return;
        }

        let is_tab_dragging = self.tabs.drag_start.is_some_and(|(_, sx, sy)| {
            ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() >= DRAG_THRESHOLD_PX
        });
        if self.tabs.drag_start.is_some() {
            self.dirty = true;
        }

        if self.pointer.mouse_down && self.tabs.drag_start.is_none() {
            // Resolved against the pane the drag started in, not just the one
            // under the pointer, so running off an edge keeps selecting.
            if let Some((tab_id, row, col)) = self.drag_cell_at(x, y) {
                let at = Point {
                    col,
                    drag: true,
                    row,
                };
                if self.offer_mouse_to_editor(tab_id, at) {
                    self.dirty = true;
                    return;
                }
                // Selection rows are absolute (see `Grid::to_absolute_row`)
                // so they keep naming the same line if auto-scroll (or a
                // wheel scroll) moves the view mid-drag.
                let abs_row = self
                    .selection_grid(tab_id)
                    .map(|grid| grid.to_absolute_row(row))
                    .unwrap_or(row);
                if let Some(sel) = &mut self.selection.span {
                    sel.end_row = abs_row;
                    sel.end_col = col;
                    sel.tab = tab_id;
                } else if let Some((start_row, start_col)) =
                    self.pointer.drag_anchor(tab_id, (abs_row, col))
                {
                    self.selection.span = Some(Selection {
                        block: false,
                        start_row,
                        start_col,
                        end_row: abs_row,
                        end_col: col,
                        tab: tab_id,
                    });
                }
                // The traversal cursor rides along with the drag's
                // live end, so the block cursor keeps following the
                // mouse while the selection extends.
                self.track_nav_cursor_to_mouse(tab_id, row, col);
                self.dirty = true;
            }
        }

        if !self.pointer.mouse_down {
            self.hover_editor(x, y);
        }

        // Update hovered hyperlink and cursor icon (divider resize, tab drag, or pointer).
        let new_url = self.hovered_link_at(x, y);
        self.pointer.hovered_url = new_url;
        let on_dialog_button = self
            .renderer
            .as_ref()
            .and_then(|r| r.hits().confirm_button_at(x, y));
        if let Some(btn_idx) = on_dialog_button {
            if self.chrome.hover_dialog_button(btn_idx) {
                self.dirty = true;
            }
        }
        let vp = self.content_viewport();
        let icon = if let Some(direction) = self.edge_resize_direction(x, y) {
            CursorIcon::from(direction)
        } else if is_tab_dragging {
            CursorIcon::Grabbing
        } else if on_dialog_button.is_some() {
            CursorIcon::Pointer
        } else {
            match self.layout().divider_at(x, y, vp) {
                Some(crate::model::layout::Direction::Vertical) => CursorIcon::EwResize,
                Some(crate::model::layout::Direction::Horizontal) => CursorIcon::NsResize,
                None => {
                    if self.pointer.hovered_url.is_some()
                        || matches!(
                            self.tabs.strip_hover,
                            StripHit::Tab(_) | StripHit::Close(_) | StripHit::NewTab(_)
                        )
                    {
                        CursorIcon::Pointer
                    } else {
                        CursorIcon::Default
                    }
                }
            }
        };
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }
    /// Scroll the pane under the pointer (the focused one when the pointer is
    /// over no pane), or forward the wheel to its PTY when a full-screen
    /// application has asked for mouse reporting. Focus stays where it was.
    pub(in crate::window) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        // The part of a row scrolled short of a whole one is kept for the
        // next movement, so a slow swipe moves steadily rather than not at
        // all and then a row at once. A high-resolution wheel, and a touchpad
        // under X11, report a notch as several fractions of one, each of
        // which would otherwise round to no row.
        let rows = match delta {
            MouseScrollDelta::LineDelta(_, y) => f64::from(y * SCROLL_LINES_PER_WHEEL_NOTCH),
            MouseScrollDelta::PixelDelta(pos) => -pos.y / APPROX_CELL_HEIGHT as f64,
        };
        // What is left over from scrolling the other way is dropped, so a
        // turn back is not eaten into; and a sum of fractions a rounding
        // error short of a row counts as the row.
        let kept = match self.input.wheel.fraction.signum() == rows.signum() {
            true => self.input.wheel.fraction,
            false => 0.0,
        };
        let rows = kept + rows;
        let rows = match (rows - rows.round()).abs() < WHEEL_ROW_SLACK {
            true => rows.round(),
            false => rows,
        };
        self.input.wheel.fraction = rows.fract();
        let scroll_lines = rows.trunc() as isize;

        let (x, y) = self.pointer.cursor_pos;
        let focused = self
            .pane_at_pixel(x, y)
            .map(|(id, _)| id)
            .unwrap_or_else(|| self.layout().focused());
        // A touchpad measures in pixels, which an editor that scrolls by the
        // pixel follows exactly rather than in whole rows.
        if let MouseScrollDelta::PixelDelta(pos) = delta {
            if self.offer_pixel_scroll_to_editor(focused, -pos.y as f32) {
                self.dirty = true;
                return;
            }
        }
        // A wheel's turn reaches an editor that scrolls by whole rows a row
        // a frame, so its view runs through them rather than jumping; one
        // that glides by the pixel takes the turn whole. A touchpad already
        // moves a little at a time.
        let glides = matches!(delta, MouseScrollDelta::LineDelta(..))
            && self
                .open_tabs
                .editor(&focused)
                .is_some_and(|slot| !slot.editor.scrolls_smoothly());
        if scroll_lines != 0 && glides {
            self.input.wheel.queue(focused, scroll_lines);
            self.step_wheel(Instant::now());
            return;
        }
        if scroll_lines != 0 && self.offer_scroll_to_editor(focused, scroll_lines) {
            self.dirty = true;
            return;
        }
        if self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_tracking())
        {
            self.drop_selection_in_tab(focused);
            self.forward_mouse_scroll(scroll_lines, focused);
            return;
        }

        if scroll_lines != 0 {
            let alt_screen = self
                .open_tabs
                .terminal(&focused)
                .is_some_and(|p| p.grid().is_alt_screen());
            if alt_screen {
                self.drop_selection_in_tab(focused);
            }
            if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                // Alt-screen apps (vim, less, etc.) own their viewport: send
                // arrow keys so they respond to the scroll gesture instead of
                // us scrolling their non-existent scrollback.
                if alt_screen {
                    let arrow = if scroll_lines > 0 {
                        b"\x1b[A" as &[u8]
                    } else {
                        b"\x1b[B"
                    };
                    let count = scroll_lines.unsigned_abs();
                    for _ in 0..count {
                        terminal.write(arrow);
                    }
                } else {
                    let grid = terminal.grid_mut();
                    if scroll_lines > 0 {
                        grid.scroll_up_history(scroll_lines as usize);
                    } else {
                        grid.scroll_down_history((-scroll_lines) as usize);
                    }
                }
            }
            self.dirty = true;
        }
    }
}

// ========================================================================
// App: the wheel's rows
// ========================================================================

impl App {
    /// Hand the editor the wheel's next rows, if they are due at `now`, and
    /// say when the ones after them are, for the event loop to wake at.
    pub(in crate::window) fn step_wheel(&mut self, now: Instant) -> Option<Instant> {
        let (tab, left, due) = self.input.wheel.queued?;
        if now < due {
            return Some(due);
        }
        // A share of what is left, at least a row: three rows go a row a
        // frame, a fast spin catches up in a few frames.
        let step = left.signum() * ((left.abs() + WHEEL_FRAMES - 1) / WHEEL_FRAMES).max(1);
        self.offer_scroll_to_editor(tab, step);
        self.dirty = true;
        match left - step {
            0 => {
                self.input.wheel.queued = None;
                None
            }
            left => {
                let next = now + WHEEL_FRAME;
                self.input.wheel.queued = Some((tab, left, next));
                Some(next)
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::model::mode::Mode;
    use crate::terminal::session::Terminal;
    use crate::window::test_support::*;

    use super::*;

    #[test]
    fn test_a_click_leaves_the_block_cursor_and_a_drag_moves_it() {
        // Regression: a plain click in Normal mode parked the block cursor
        // under the pointer, so it seemed to appear wherever you clicked.
        let (mut app, id) = app_pane_and_id();
        app.open_tabs.set_mode(&id, Mode::Normal);
        app.set_nav_cursor(id, (1, 1));

        app.press_grid_cell(id, 3, 7);
        assert_eq!(app.nav_cursor(id), Some((1, 1)), "a click keeps the cursor");
        assert!(app.pointer.press_cell.is_some(), "a drag can still anchor");

        app.track_nav_cursor_to_mouse(id, 4, 2);
        assert_eq!(app.nav_cursor(id), Some((4, 2)), "a drag moves it");
    }

    #[test]
    fn test_the_wheel_scrolls_the_pane_under_the_pointer_not_the_focused_one() {
        let (mut app, left) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&left)
            .expect("pane")
            .grid_mut()
            .scroll_up(20);
        let right = app.alloc_tab_id();
        app.layout_mut()
            .split(crate::model::layout::Direction::Vertical, 0.5, right);
        let mut terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_core::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        terminal.grid_mut().scroll_up(20);
        app.open_tabs.insert_terminal(right, terminal);
        app.open_tabs.set_mode(&right, Mode::Insert);

        // Focus is on `right` after the split; point at `left`.
        assert_eq!(app.layout().focused(), right);
        let rect = app.pane_rect(left).expect("left is shown");
        app.pointer.cursor_pos = (rect.x + 1.0, rect.y + 1.0);

        app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, 1.0));

        let offset = |app: &App, id| app.open_tabs.terminal(&id).unwrap().grid().scroll_offset();
        assert!(
            offset(&app, left) > 0,
            "the pane under the pointer scrolled"
        );
        assert_eq!(offset(&app, right), 0, "the focused pane stayed put");
        assert_eq!(app.layout().focused(), right, "focus did not move");
    }

    #[test]
    fn test_scrolling_a_full_screen_app_drops_the_stale_selection() {
        // Regression: a selection names absolute grid rows, which follow the
        // text through Winter's own scrolling but cannot follow a full-screen
        // app scrolling its own content. The wheel goes to the app, the app
        // repaints, the grid rows never move, and the highlight was left
        // sitting over whatever the app painted there: copying it returned
        // text the user never pointed at.
        let mut app = App::new();
        // The default tab lays out a single pane, `TabId(0)`.
        let id = TabId(0);
        let mut terminal = pane_with_scrollback();
        terminal.grid_mut().enter_alt_screen();
        // DECSET ?1000: the app asks for mouse reporting, so the wheel is
        // forwarded to it rather than scrolling Winter's own view.
        terminal.grid_mut().set_private_mode(1000, true);
        app.open_tabs.insert_terminal(id, terminal);
        app.selection.span = Some(Selection {
            block: false,
            start_row: 3,
            start_col: 0,
            end_row: 5,
            end_col: 10,
            tab: id,
        });

        app.on_mouse_wheel(winit::event::MouseScrollDelta::LineDelta(0.0, -1.0));

        assert!(
            app.selection.span.is_none(),
            "the text under the highlight is about to change, so the selection goes"
        );
    }

    /// An editor that scrolls by whole rows and keeps each scroll it was
    /// offered, for a test to read back.
    struct Scrolls(std::rc::Rc<std::cell::RefCell<Vec<isize>>>);

    impl crate::model::editor::Editor for Scrolls {
        fn title(&self) -> String {
            "Scrolls".to_string()
        }

        fn content(
            &mut self,
            _rows: usize,
            _cols: usize,
            _wrap: bool,
        ) -> crate::model::paint::Frame {
            crate::model::paint::Frame::new(Vec::new())
        }

        fn on_key(
            &mut self,
            _key: &crate::model::input::Key,
        ) -> crate::model::editor::EditorOutcome {
            crate::model::editor::EditorOutcome::Ignored
        }

        fn on_scroll(&mut self, lines: isize) -> crate::model::editor::EditorOutcome {
            self.0.borrow_mut().push(lines);
            crate::model::editor::EditorOutcome::Consumed
        }
    }

    #[test]
    fn test_a_wheel_turn_reaches_a_row_editor_a_row_a_frame() {
        let scrolled = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut app = App::new();
        let tab = app.show_editor(
            crate::window::editor::EditorKind::Keys,
            Box::new(Scrolls(std::rc::Rc::clone(&scrolled))),
        );
        // One notch: three rows, the first at once and the rest a frame apart.
        app.input.wheel.queue(tab, 3);
        let start = Instant::now();
        let next = app.step_wheel(start).expect("more to come");
        assert_eq!(*scrolled.borrow(), [1]);
        assert_eq!(
            app.step_wheel(next - WHEEL_FRAME / 2),
            Some(next),
            "not due yet"
        );
        let next = app.step_wheel(next).expect("one more");
        assert_eq!(app.step_wheel(next), None, "done");
        assert_eq!(*scrolled.borrow(), [1, 1, 1]);

        // A fast spin catches up a share at a time, every row delivered.
        scrolled.borrow_mut().clear();
        app.input.wheel.queue(tab, -12);
        let mut at = Some(start);
        while let Some(now) = at {
            at = app.step_wheel(now);
        }
        assert_eq!(scrolled.borrow().iter().sum::<isize>(), -12);
        assert!(scrolled.borrow().len() < 12, "{:?}", scrolled.borrow());
    }

    #[test]
    fn test_a_wheel_turn_scrolls_the_text_editor_three_rows() {
        let dir = std::env::temp_dir().join(format!("winter-wheel-text-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("long.txt");
        let text: String = (0..200).map(|n| format!("line {n}\n")).collect();
        std::fs::write(&file, text).expect("file");
        let mut app = App::new();
        let editor = crate::editors::text::TextEditor::new(file, None).expect("opens");
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, Box::new(editor));
        let top = |app: &mut App| {
            let slot = app.open_tabs.editor_mut(&tab).expect("editor");
            let frame = slot.editor.content(20, 40, true);
            crate::model::paint::row_text(&frame.rows[0])
        };
        let before = top(&mut app);
        app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));
        let mut at = Some(Instant::now());
        while let Some(now) = at {
            let _ = top(&mut app);
            at = app.step_wheel(now);
        }
        let after = top(&mut app);
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(before, after, "the view moved");
        assert!(after.contains("line 3"), "three rows down: {after:?}");
    }

    #[test]
    fn test_the_wheel_scrolls_wrapped_text_away_from_the_cursor_and_back() {
        // With lines wrapped over several rows, the cursor a little down the
        // page was taken to be in view while off screen, and each paint
        // pulled the view back to it: the wheel moved a few rows and stuck.
        let dir = std::env::temp_dir().join(format!("winter-wheel-wrap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("prose.txt");
        let words = "word ".repeat(20);
        let text: String = (1..=200).map(|n| format!("line {n} {words}\n")).collect();
        std::fs::write(&file, text).expect("file");
        let mut app = App::new();
        let target = crate::model::editor::OpenTarget::at(file, 16, None);
        let editor = crate::editors::text::TextEditor::at(target).expect("opens");
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, Box::new(editor));
        let top = |app: &mut App| {
            let slot = app.open_tabs.editor_mut(&tab).expect("editor");
            let frame = slot.editor.content(20, 40, true);
            crate::model::paint::row_text(&frame.rows[0])
        };
        let turn = |app: &mut App, notches: f32| {
            for _ in 0..notches.abs() as usize {
                app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, notches.signum()));
                let mut at = Some(Instant::now());
                while let Some(now) = at {
                    let _ = top(app);
                    at = app.step_wheel(now + WHEEL_FRAME);
                }
            }
            top(app)
        };
        let start = top(&mut app);
        let shown: Vec<String> = {
            let slot = app.open_tabs.editor_mut(&tab).expect("editor");
            slot.editor.content(20, 40, true).rows.iter().map(crate::model::paint::row_text).collect()
        };
        assert!(shown.iter().any(|row| row.contains("line 16 ")), "opened with the cursor's line in view: {shown:?}");
        let down = turn(&mut app, -10.0);
        let back = turn(&mut app, 10.0);
        let above = turn(&mut app, 20.0);
        let _ = std::fs::remove_dir_all(&dir);
        assert_ne!(down, start, "the view moved down");
        assert_eq!(back, start, "and came back where it was");
        assert!(above.contains("line 1 "), "and on up past the cursor to the top: {above:?}");
    }

    #[test]
    fn test_the_wheel_reaches_the_end_of_a_document_of_long_wrapped_lines() {
        // The last start the wheel could reach was the row count less the
        // pane's height in screen rows, as if each row took one: a note of
        // few but long lines stopped scrolling a few lines in, the rest of
        // it below and out of reach.
        let dir = std::env::temp_dir().join(format!("winter-wheel-end-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("dir");
        let file = dir.join("note.md");
        let long = "word ".repeat(200);
        let text: String = (1..=46)
            .map(|n| match n > 30 {
                true => format!("line {n} {long}end{n}\n"),
                false => format!("line {n}\n"),
            })
            .collect();
        std::fs::write(&file, text).expect("file");
        let mut app = App::new();
        let editor = crate::editors::text::TextEditor::new(file, None).expect("opens");
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, Box::new(editor));
        let top = |app: &mut App| {
            let slot = app.open_tabs.editor_mut(&tab).expect("editor");
            let frame = slot.editor.content(20, 40, true);
            frame.rows.iter().map(crate::model::paint::row_text).collect::<Vec<_>>()
        };
        let _ = top(&mut app);
        for _ in 0..200 {
            app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -1.0));
            let mut at = Some(Instant::now());
            while let Some(now) = at {
                let _ = top(&mut app);
                at = app.step_wheel(now + WHEEL_FRAME);
            }
        }
        let shown = top(&mut app);
        // A click on what shows puts the cursor there and leaves the view.
        let click = crate::model::paint::Point { col: 10, drag: false, row: 0 };
        app.open_tabs.editor_mut(&tab).expect("editor").editor.on_mouse(click);
        let clicked = top(&mut app);
        let _ = std::fs::remove_dir_all(&dir);
        // The last line is taller than the pane: its end shows, read
        // through to rather than cut off below.
        assert!(shown.last().is_some_and(|row| row.contains("end46")), "the end reached: {shown:?}");
        assert_eq!(shown.len(), 20, "a full pane");
        assert_eq!(clicked, shown, "the view stays put under the click");
    }

    #[test]
    fn test_a_high_resolution_wheel_s_fractions_of_a_notch_add_up_to_rows() {
        // A high-resolution wheel, and a touchpad under X11, report a notch
        // as several fractions of one; each rounded alone came to no row at
        // all, and the wheel scrolled nothing.
        let scrolled = std::rc::Rc::new(std::cell::RefCell::new(Vec::new()));
        let mut app = App::new();
        app.show_editor(
            crate::window::editor::EditorKind::Keys,
            Box::new(Scrolls(std::rc::Rc::clone(&scrolled))),
        );
        for _ in 0..10 {
            app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, -0.1));
            let mut at = Some(Instant::now());
            while let Some(now) = at {
                at = app.step_wheel(now + WHEEL_FRAME);
            }
        }
        assert_eq!(scrolled.borrow().iter().sum::<isize>(), -3, "{:?}", scrolled.borrow());

        // Part of a row left over going down is not taken off a notch back up.
        scrolled.borrow_mut().clear();
        for y in [-0.15, 1.0] {
            app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, y));
            let mut at = Some(Instant::now());
            while let Some(now) = at {
                at = app.step_wheel(now + WHEEL_FRAME);
            }
        }
        assert_eq!(scrolled.borrow().iter().sum::<isize>(), 3, "{:?}", scrolled.borrow());
    }

    #[test]
    fn test_the_pdf_viewer_glides_a_wheel_turn_itself() {
        let pdf = crate::editors::pdf::PdfEditor::new(std::path::PathBuf::from("/nonexistent.pdf"));
        assert!(crate::model::editor::Editor::scrolls_smoothly(&pdf));
    }
}

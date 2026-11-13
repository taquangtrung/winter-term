//! Where the pointer goes: presses and releases, motion and drags, and the
//! wheel, routed to the tabbar, menus, strips, status bar, or the pane under
//! the pointer.

use std::time::{Duration, Instant};

use crate::render::PaneRect;
use misty::AppContext;
use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, MouseButton, MouseScrollDelta};
use winit::window::CursorIcon;

use winter_model::input::{Key, KeyCode};
use winter_model::layout::TabId;
use winter_model::paint::Point;

use crate::window::input::overlay::Overlay;
use crate::window::layout::bands::crumb_of;
use crate::window::strip::StripHit;
use crate::window::terminal::pointer::ScrollbarDrag;
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

/// Shortest visible thumb on a pane scrollbar so deep content remains grabbable.
const SCROLLBAR_MIN_THUMB: f32 = 20.0;

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

/// Geometry of a pane's scrollbar track and thumb for hit-testing and dragging.
#[derive(Clone, Copy, Debug)]
struct ScrollbarGeometry {
    available_travel: f32,
    thumb_h: f32,
    thumb_y0: f32,
    thumb_y1: f32,
    track_y0: f32,
}

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
// ScrollbarGeometry
// ========================================================================

impl ScrollbarGeometry {
    fn new(pr: PaneRect, cell_h: f32, visible: usize, total: usize, current_offset: usize) -> Self {
        let grid_area = pr.grid_area(cell_h);
        let track_y0 = grid_area.y;
        let track_h = grid_area.height;
        let track_y1 = track_y0 + track_h;
        let total_f = total as f32;
        let visible_f = visible as f32;
        let thumb_h = ((visible_f / total_f) * track_h)
            .max(SCROLLBAR_MIN_THUMB)
            .min(track_h);
        let available_travel = (track_h - thumb_h).max(1.0);
        let top_virtual = (current_offset as f32 / total_f) * track_h;
        let thumb_y0 = (track_y0 + top_virtual).min(track_y1 - thumb_h);
        let thumb_y1 = thumb_y0 + thumb_h;
        Self {
            available_travel,
            thumb_h,
            thumb_y0,
            thumb_y1,
            track_y0,
        }
    }

    fn frac_from_thumb_top(self, thumb_top: f32) -> f32 {
        ((thumb_top - self.track_y0) / self.available_travel).clamp(0.0, 1.0)
    }

    fn on_click(self, y: f32) -> (f32, Option<f32>) {
        if y >= self.thumb_y0 && y <= self.thumb_y1 {
            (y - self.thumb_y0, None)
        } else {
            let grab_offset = self.thumb_h / 2.0;
            let thumb_top = y - grab_offset;
            (grab_offset, Some(self.frac_from_thumb_top(thumb_top)))
        }
    }

    fn on_drag(self, y: f32, grab_offset: f32) -> f32 {
        self.frac_from_thumb_top(y - grab_offset)
    }
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
        self.input.pointer.press_cell = self
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
        let (x, y) = self.input.pointer.cursor_pos;
        let now = Instant::now();
        let double = self
            .input
            .pointer
            .last_click
            .is_some_and(|(prev_time, prev_x, prev_y)| {
                let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
                now.duration_since(prev_time) < DOUBLE_CLICK_INTERVAL && dist < DOUBLE_CLICK_SLOP
            });
        self.input.pointer.last_click = Some((now, x, y));
        double
    }

    /// Route a mouse button press or release. The tabbar and any open menu
    /// take precedence over the panes, so a click on chrome never reaches a
    /// pane underneath it.
    pub(in crate::window) fn on_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        cx: &mut AppContext<'_, ()>,
    ) {
        self.handle_mouse_input(state, button);
        self.quit_if_requested(cx);
    }

    /// The mouse button handler proper, split from [`Self::on_mouse_input`] so
    /// tests can drive it without an `ActiveEventLoop`.
    pub(in crate::window) fn handle_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
    ) {
        if self.chrome.dialog_open() && state == ElementState::Pressed {
            self.press_in_dialog(button);
            return;
        }
        let focused = self.layout().focused();

        // Tabbar/menubar clicks take precedence over the panes (including
        // mouse-tracking apps), and any click resolves an open menu.
        if let (ElementState::Pressed, MouseButton::Left) = (state, button) {
            let (x, y) = self.input.pointer.cursor_pos;
            if let Some(direction) = self.edge_resize_direction(x, y) {
                if let Some(window) = &self.window {
                    window.drag_resize_window(direction);
                }
                return;
            }
            // A click on the find popup is the popup's, not the pane's under it.
            if self.find_card_at(x, y) {
                return;
            }
            // Confirmation dialog buttons or input dialog clicks
            if let Some(btn_idx) = self.dialog_button_at(x, y) {
                self.click_dialog_button(btn_idx);
                self.dirty = true;
                return;
            }
            if self.dialog_at(x, y) {
                return;
            }
            // The title bar is misty's; what of it misty leaves (the empty
            // bar under a system frame) selects no pane text.
            if y < self.top_chrome_height() {
                return;
            }
        }

        // Ctrl+click on a hyperlinked cell or printed URL/file opens it.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.input.modifiers.state().control_key()
        {
            let (x, y) = self.input.pointer.cursor_pos;
            if self.open_file_at_pixel(x, y) {
                self.dirty = true;
                return;
            }
            // A `file://` link was the file above, if it is a local one; the
            // OS is handed only the schemes safe to hand it.
            if let Some(url) = &self.input.pointer.hovered_url {
                if winter_terminal::session::is_os_openable(url) {
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
            state == ElementState::Released && self.input.pointer.forwarded_button.is_some();
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
                let (x, y) = self.input.pointer.cursor_pos;
                self.open_context_menu(x, y);
                self.dirty = true;
            }
            return;
        }

        match (state, button) {
            (ElementState::Pressed, MouseButton::Left) => {
                self.input.pointer.mouse_down = true;
                self.term.selection.span = None;

                let (x, y) = self.input.pointer.cursor_pos;

                // Scrollbar click: right-edge strip of a pane navigates scrollback.
                // Checked before split dividers and focus changes so clicking on a scrollbar
                // (even when adjacent to a split divider) drives scrolling rather than resizing.
                let ch = self
                    .renderer
                    .as_ref()
                    .map(|r| r.cell_size().1)
                    .unwrap_or(20.0);
                let viewport = self.content_viewport();
                'scroll: for (id, rect) in self.pane_rects() {
                    let pr = Self::layout_rect_to_pane(rect);
                    let sb_x = pr.x + pr.width - SCROLLBAR_CLICK_WIDTH;
                    let has_divider_right = self
                        .layout()
                        .divider_at(pr.x + pr.width, y, viewport)
                        .is_some();
                    let max_x = if has_divider_right {
                        pr.x + pr.width - 2.0
                    } else {
                        pr.x + pr.width + 1.0
                    };
                    if x >= sb_x && x <= max_x && y >= pr.y && y <= pr.y + pr.height {
                        if let Some(terminal) = self.open_tabs.terminal_mut(&id) {
                            let sbl = terminal.grid().scrollback_len();
                            if sbl > 0 {
                                let rows = terminal.grid().rows();
                                let total = rows + sbl;
                                let current_offset =
                                    sbl.saturating_sub(terminal.grid().scroll_offset());
                                let geom =
                                    ScrollbarGeometry::new(pr, ch, rows, total, current_offset);
                                let (grab_offset, target_frac) = geom.on_click(y);
                                if let Some(frac) = target_frac {
                                    let new_offset = ((1.0 - frac) * sbl as f32).round() as usize;
                                    terminal.grid_mut().set_scroll_offset(new_offset);
                                }
                                self.input.pointer.scrollbar_drag = Some(ScrollbarDrag {
                                    tab: id,
                                    grab_offset,
                                });
                                self.set_focus(id);
                                self.dirty = true;
                                break 'scroll;
                            }
                        } else if let Some(slot) = self.open_tabs.page_mut(&id) {
                            if let Some(pos) = slot.page.scroll_position() {
                                if pos.total > pos.visible {
                                    let geom = ScrollbarGeometry::new(
                                        pr,
                                        ch,
                                        pos.visible,
                                        pos.total,
                                        pos.offset,
                                    );
                                    let (grab_offset, target_frac) = geom.on_click(y);
                                    if let Some(frac) = target_frac {
                                        let max_scroll = pos.total.saturating_sub(pos.visible);
                                        let target = (frac * max_scroll as f32).round() as usize;
                                        let outcome = slot.page.scroll_to(target);
                                        self.act_on_editor_outcome(id, outcome);
                                    }
                                    self.input.pointer.scrollbar_drag = Some(ScrollbarDrag {
                                        tab: id,
                                        grab_offset,
                                    });
                                    self.set_focus(id);
                                    self.dirty = true;
                                    break 'scroll;
                                }
                            }
                        }
                    }
                }

                if self.input.pointer.scrollbar_drag.is_some() {
                    self.dirty = true;
                    return;
                }

                // Divider click: start a drag; only reached if not clicking on an active scrollbar.
                let on_divider = {
                    let viewport = self.content_viewport();
                    self.layout().divider_at(x, y, viewport).is_some()
                };
                if on_divider {
                    self.input.pointer.divider_drag = Some((x, y));
                    self.dirty = true;
                    return;
                }

                if let Some((tab_id, pane_rect)) = self.pane_at_pixel(x, y) {
                    self.set_focus(tab_id);
                    // An editor's toolbar is drawn in the window's font,
                    // off the pane's cells, so a press on it is told
                    // which button by where the button was drawn.
                    let button = self
                        .toolbar_button_at(x, y)
                        .filter(|(owner, _)| *owner == tab_id.0);
                    if let Some((_, button)) = button {
                        if let Some(slot) = self.open_tabs.page_mut(&tab_id) {
                            // The breadcrumbs are drawn as a toolbar too.
                            let outcome = match crumb_of(button) {
                                Some(crumb) => slot.page.on_crumb(crumb),
                                None => slot.page.on_toolbar(button),
                            };
                            self.act_on_editor_outcome(tab_id, outcome);
                            self.input.pointer.press_cell = None;
                            self.dirty = true;
                            return;
                        }
                        // A terminal's one button is its sticky header.
                        if self.open_tabs.terminal(&tab_id).is_some() {
                            self.jump_to_sticky_prompt(tab_id);
                            self.input.pointer.press_cell = None;
                            return;
                        }
                    }
                    // A press on an editor's header between its crumbs is on
                    // none of its rows.
                    if y < pane_rect.y {
                        self.input.pointer.press_cell = None;
                        return;
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
                        self.input.pointer.press_cell = None;
                        // The column cursor goes to the cell clicked,
                        // on the row the click took the editor's cursor
                        // to.
                        self.place_editor_column(tab_id, col);
                        // A third press starts over rather than ending
                        // a second double click.
                        if self.take_double_click() {
                            self.input.pointer.last_click = None;
                            self.offer_double_click_to_editor(tab_id, at);
                        }
                        self.dirty = true;
                        return;
                    }
                    self.press_grid_cell(tab_id, row, col);
                }

                self.dirty = true;
            }
            (ElementState::Released, MouseButton::Left) => {
                self.input.pointer.mouse_down = false;
                self.input.pointer.divider_drag = None;
                self.input.pointer.press_cell = None;
                let was_dragging_sb = self.input.pointer.scrollbar_drag.take().is_some();
                if was_dragging_sb {
                    let rects = self.pane_rects();
                    self.input.pointer.scrollbar_hover = self.hovered_scrollbar_pane(&rects);
                }
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

    /// Tell the editor under the pointer the cell it rests on, and the one
    /// it rested on before that it has left.
    fn hover_editor(&mut self, x: f32, y: f32) {
        let under = self
            .pane_at_pixel(x, y)
            .filter(|(id, _)| self.open_tabs.page(id).is_some())
            .map(|(id, rect)| (id, self.pixel_to_cell(x, y, rect)));
        let left = self
            .input
            .pointer
            .hovered_editor
            .filter(|tab| under.map(|(id, _)| id) != Some(*tab));
        if let Some(tab) = left {
            self.dirty |= self.offer_hover_to_editor(tab, None);
        }
        self.input.pointer.hovered_editor = under.map(|(id, _)| id);
        if let Some((tab, (row, col))) = under {
            let at = Point {
                col,
                drag: false,
                row,
            };
            self.dirty |= self.offer_hover_to_editor(tab, Some(at));
        }
    }

    /// Put out what the pointer lit as it leaves the window: the status bar
    /// part it was on, and the editor it rested on.
    pub(in crate::window) fn on_cursor_left(&mut self) {
        self.input.pointer.in_window = false;
        if self.chrome.status_hover.take().is_some() {
            self.dirty = true;
        }
        if let Some(tab) = self.input.pointer.hovered_editor.take() {
            self.dirty |= self.offer_hover_to_editor(tab, None);
        }
        if self.input.pointer.hovered_pane.take().is_some() {
            self.dirty = true;
        }
        if self.input.pointer.scrollbar_hover.take().is_some() {
            self.dirty = true;
        }
        if let Some(window) = &self.window {
            window.set_cursor(CursorIcon::Default);
        }
    }

    /// Track the pointer: hover highlights, drag-selection extension, split
    /// dragging, and the resize-edge cursor shape.
    pub(in crate::window) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let x = position.x as f32;
        let y = position.y as f32;
        self.input.pointer.cursor_pos = (x, y);
        self.input.pointer.in_window = true;
        // The pointer is in use again: its hover decides which scrollbar is lit.
        if self.input.pointer.scrollbar_keyed.take().is_some() {
            self.dirty = true;
        }
        if self.chrome.dialog_open() {
            self.hover_dialog(x, y);
            return;
        }

        // Divider drag: highest priority, blocks selection and PTY forwarding.
        if self.input.pointer.mouse_down {
            if let Some((prev_x, prev_y)) = self.input.pointer.divider_drag {
                let dx = x - prev_x;
                let dy = y - prev_y;
                let viewport = self.content_viewport();
                if self
                    .layout_mut()
                    .drag_divider(prev_x, prev_y, dx, dy, viewport)
                {
                    self.input.pointer.divider_drag = Some((x, y));
                    self.resize_all_panes();
                    self.dirty = true;
                }
                return;
            }

            // Scrollbar drag: update scroll position proportionally.
            if let Some(drag) = self.input.pointer.scrollbar_drag {
                if let Some((_, rect)) = self
                    .pane_rects()
                    .into_iter()
                    .find(|(id, _)| *id == drag.tab)
                {
                    let pr = Self::layout_rect_to_pane(rect);
                    let ch = self
                        .renderer
                        .as_ref()
                        .map(|r| r.cell_size().1)
                        .unwrap_or(20.0);
                    if let Some(terminal) = self.open_tabs.terminal_mut(&drag.tab) {
                        let sbl = terminal.grid().scrollback_len();
                        if sbl > 0 {
                            let rows = terminal.grid().rows();
                            let total = rows + sbl;
                            let current_offset =
                                sbl.saturating_sub(terminal.grid().scroll_offset());
                            let geom = ScrollbarGeometry::new(pr, ch, rows, total, current_offset);
                            let frac = geom.on_drag(y, drag.grab_offset);
                            let new_offset = ((1.0 - frac) * sbl as f32).round() as usize;
                            terminal.grid_mut().set_scroll_offset(new_offset);
                            self.dirty = true;
                        }
                    } else if let Some(slot) = self.open_tabs.page_mut(&drag.tab) {
                        if let Some(pos) = slot.page.scroll_position() {
                            if pos.total > pos.visible {
                                let geom = ScrollbarGeometry::new(
                                    pr,
                                    ch,
                                    pos.visible,
                                    pos.total,
                                    pos.offset,
                                );
                                let frac = geom.on_drag(y, drag.grab_offset);
                                let max_scroll = pos.total.saturating_sub(pos.visible);
                                let target = (frac * max_scroll as f32).round() as usize;
                                let outcome = slot.page.scroll_to(target);
                                self.act_on_editor_outcome(drag.tab, outcome);
                                self.dirty = true;
                            }
                        }
                    }
                }
                return;
            }
        }

        let tb_button = self.toolbar_button_at(x, y);
        if tb_button != self.input.pointer.hovered_toolbar_button {
            self.input.pointer.hovered_toolbar_button = tb_button;
            self.dirty = true;
        }

        let hp = self.hovered_pane(&self.pane_bounds());
        if self.input.pointer.hovered_pane != hp {
            self.input.pointer.hovered_pane = hp;
            self.dirty = true;
        }

        let sb_hover = if let Some(drag) = self.input.pointer.scrollbar_drag {
            Some(drag.tab)
        } else {
            self.hovered_scrollbar_pane(&self.pane_rects())
        };
        if self.input.pointer.scrollbar_hover != sb_hover {
            self.input.pointer.scrollbar_hover = sb_hover;
            self.dirty = true;
        }

        // The pane strips' hover, which also carries a tab's full title as a
        // tooltip, since a strip may have had to cut it short.
        let strip_before = self.tabs.strip_hover;
        self.update_strip_hover(x, y);
        if self.tabs.strip_hover != strip_before {
            self.tabs.hover_anchor = match self.tabs.strip_hover {
                StripHit::Tab(tab) => self.strip_tab_region(tab),
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
            if self.input.pointer.forwarded_button.is_some() {
                p.mouse_drag_tracking()
            } else {
                p.mouse_any_event()
            }
        });
        if reports_motion && self.input.modifiers.state().shift_key() {
            self.forward_mouse_motion(focused);
            return;
        }

        if self.input.pointer.mouse_down {
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
                if let Some(sel) = &mut self.term.selection.span {
                    sel.end_row = abs_row;
                    sel.end_col = col;
                    sel.tab = tab_id;
                } else if let Some((start_row, start_col)) =
                    self.input.pointer.drag_anchor(tab_id, (abs_row, col))
                {
                    self.term.selection.span = Some(Selection {
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

        if !self.input.pointer.mouse_down {
            self.hover_editor(x, y);
        }

        // Update hovered hyperlink and cursor icon (divider resize, tab drag, or pointer).
        let new_url = self.hovered_link_at(x, y);
        self.input.pointer.hovered_url = new_url;
        let on_dialog_button = self.dialog_button_at(x, y);
        if let Some(btn_idx) = on_dialog_button {
            if self.chrome.hover_dialog_button(btn_idx) {
                self.dirty = true;
            }
        }
        let icon = self.cursor_icon_at(x, y);
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }

    /// A press while a dialog is up: a left click on one of its buttons
    /// answers it, and one on the window's edge or title bar still resizes,
    /// moves, minimizes, or maximizes the window (see
    /// [`App::manage_window_at`]). A press on the window behind turns the
    /// dialog down, as `Esc` does; one elsewhere on the dialog does nothing.
    /// A release still goes the usual way,
    /// so a gesture begun before the dialog opened (the click that asked to
    /// close, say) finishes cleanly.
    fn press_in_dialog(&mut self, button: MouseButton) {
        if button != MouseButton::Left {
            return;
        }
        let (x, y) = self.input.pointer.cursor_pos;
        if let Some(direction) = self.edge_resize_direction(x, y) {
            if let Some(window) = &self.window {
                window.drag_resize_window(direction);
            }
            return;
        }
        if self.manage_window_at(x, y) {
            return;
        }
        if let Some(btn_idx) = self.dialog_button_at(x, y) {
            self.click_dialog_button(btn_idx);
            self.dirty = true;
            return;
        }
        // A press off the dialog turns it down, as a press off a picker or a
        // transient puts that away.
        let on_dialog = self.dialog_at(x, y);
        if !on_dialog {
            self.cancel_dialog();
            self.dirty = true;
        }
    }

    /// Turn down the dialog up, as `Esc` does: a question is answered with
    /// nothing, a confirmation with its no.
    fn cancel_dialog(&mut self) {
        let escape = Key::plain(KeyCode::Escape);
        match &self.chrome.overlay {
            Some(Overlay::Close(_)) => self.handle_close_prompt_key(&escape),
            Some(Overlay::Error(_)) => self.handle_error_dialog_key(&escape),
            Some(Overlay::EditorPrompt(_)) => self.handle_prompt_key(&escape),
            Some(Overlay::Password(_)) => self.handle_password_key(&escape),
            Some(Overlay::ZaiKey(_)) => self.handle_zai_key_key(&escape),
            Some(Overlay::ThemeName(_)) => self.chrome.overlay = None,
            Some(Overlay::Picker(_) | Overlay::Rename(_)) | None => {}
        }
    }

    /// The pointer moving while a dialog is up: its buttons light under it,
    /// and the window behind sees nothing (no hover, drag, or report to a
    /// program).
    fn hover_dialog(&mut self, x: f32, y: f32) {
        let on_button = self.dialog_button_at(x, y);
        if let Some(btn_idx) = on_button {
            if self.chrome.hover_dialog_button(btn_idx) {
                self.dirty = true;
            }
        }
        let icon = self.cursor_icon_at(x, y);
        if let Some(window) = &self.window {
            window.set_cursor(icon);
        }
    }

    /// Determine the cursor icon that should be displayed at `(x, y)`.
    pub(in crate::window) fn cursor_icon_at(&self, x: f32, y: f32) -> CursorIcon {
        // Under a dialog only its buttons and the window's edges take the
        // pointer.
        if self.chrome.dialog_open() {
            if let Some(direction) = self.edge_resize_direction(x, y) {
                return CursorIcon::from(direction);
            }
            return match self.dialog_button_at(x, y) {
                Some(_) => CursorIcon::Pointer,
                None => CursorIcon::Default,
            };
        }

        let is_tab_dragging = self.chrome.tab_drag.is_some();
        let on_dialog_button = self.dialog_button_at(x, y);

        if self.input.pointer.scrollbar_drag.is_some()
            || self.input.pointer.scrollbar_hover.is_some()
        {
            CursorIcon::Default
        } else if let Some(direction) = self.edge_resize_direction(x, y) {
            CursorIcon::from(direction)
        } else if is_tab_dragging {
            CursorIcon::Grabbing
        } else if on_dialog_button.is_some() {
            CursorIcon::Pointer
        } else {
            let vp = self.content_viewport();
            match self.layout().divider_at(x, y, vp) {
                Some(winter_model::layout::Direction::Vertical) => CursorIcon::EwResize,
                Some(winter_model::layout::Direction::Horizontal) => CursorIcon::NsResize,
                None => {
                    if self.input.pointer.hovered_url.is_some()
                        || self.input.pointer.hovered_toolbar_button.is_some()
                        || matches!(
                            self.tabs.strip_hover,
                            StripHit::Tab(_) | StripHit::Close(_) | StripHit::NewTab(_)
                        )
                    {
                        CursorIcon::Pointer
                    } else if self.is_text_cursor_at(x, y) {
                        CursorIcon::Text
                    } else {
                        CursorIcon::Default
                    }
                }
            }
        }
    }

    fn is_text_cursor_at(&self, x: f32, y: f32) -> bool {
        if let Some((tab_id, _)) = self.pane_at_pixel(x, y) {
            if let Some(term) = self.open_tabs.terminal(&tab_id) {
                if term.mouse_tracking() && !self.input.modifiers.state().shift_key() {
                    return false;
                }
                return true;
            }
            if self.open_tabs.page(&tab_id).is_some() {
                return true;
            }
        }
        false
    }

    /// Scroll the pane under the pointer (the focused one when the pointer is
    /// over no pane), or forward the wheel to its PTY when a full-screen
    /// application has asked for mouse reporting. Focus stays where it was.
    pub(in crate::window) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        if self.input.pointer.scrollbar_keyed.take().is_some() {
            self.dirty = true;
        }
        // Nothing scrolls under a dialog.
        if self.chrome.dialog_open() {
            return;
        }
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

        let (x, y) = self.input.pointer.cursor_pos;
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
                .page(&focused)
                .is_some_and(|slot| !slot.page.scrolls_smoothly());
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

    /// Click an item in the open which-key HUD or git transient popup.
    pub(in crate::window) fn click_which_key(&mut self, key_str: &str) {
        let focused = self.layout().focused();
        if let Some(switch_ch) = key_str.strip_prefix('-').and_then(|s| s.chars().next()) {
            let key_dash = Key::plain(KeyCode::Char('-'));
            let key_choice = Key::plain(KeyCode::Char(switch_ch));
            self.offer_key_to_editor(focused, &key_dash);
            self.offer_key_to_editor(focused, &key_choice);
        } else if key_str == "RET" {
            let key = Key::plain(KeyCode::Enter);
            self.offer_key_to_editor(focused, &key);
        } else if let Some(ch) = key_str.chars().next() {
            let key = Key::plain(KeyCode::Char(ch));
            self.offer_key_to_editor(focused, &key);
        }
    }

    /// Dismiss an open which-key HUD or git transient popup when clicking outside.
    pub(in crate::window) fn dismiss_which_key_popup(&mut self) {
        let focused = self.layout().focused();
        let key_esc = Key::plain(KeyCode::Escape);
        // A transient is closed level by level and no further: one more
        // `Esc` would reach the editor beneath it. A which-key card's
        // sequence is the editor's own, and one `Esc` ends it.
        let had_transient = self.transient_popup(focused).is_some();
        while self.transient_popup(focused).is_some() {
            self.offer_key_to_editor(focused, &key_esc);
        }
        if !had_transient {
            self.offer_key_to_editor(focused, &key_esc);
        }
        self.input.keys.prefix = winter_model::input::PendingPrefix::None;
        self.input.keys.prefix_since = None;
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use crate::window::test_support::*;
    use winter_model::mode::Mode;
    use winter_terminal::session::git_editor_command;
    use winter_terminal::session::Terminal;

    use super::*;

    #[test]
    fn test_leaving_the_window_puts_out_the_status_bar_part_it_lit() {
        // The part stayed lit after the pointer left, and the frame after
        // could light it again from where the pointer had last been.
        let mut app = App::new();
        app.on_cursor_moved(PhysicalPosition::new(10.0, 10.0));
        assert!(app.input.pointer.in_window);
        app.chrome.status_hover = Some(crate::render::StatusHover::Bell);
        app.dirty = false;
        app.on_cursor_left();
        assert_eq!(app.chrome.status_hover, None);
        assert!(app.dirty, "drawn again without it");
        assert!(!app.input.pointer.in_window, "so no frame lights it again");
    }

    #[test]
    fn test_click_which_key_toggles_git_switch_and_dismisses() {
        let mut app = App::new();
        let editor =
            winter_git::GitEditor::new(std::path::PathBuf::from("/repo"), git_editor_command());
        let tab = app.show_editor(crate::window::editor::EditorKind::Git, Box::new(editor));
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('c')));

        // Initially -a switch is unchecked
        let slot = app.open_tabs.page(&tab).unwrap();
        let hint = slot.page.hint().unwrap();
        assert!(hint
            .items
            .iter()
            .any(|(k, v)| k == "-a" && v.starts_with("[ ] ")));
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.title, "Commit");
        assert!(tr.switches.iter().any(|s| s.key == "-a" && !s.checked));

        // Click -a to toggle it on
        app.click_which_key("-a");
        let slot = app.open_tabs.page(&tab).unwrap();
        let hint = slot.page.hint().unwrap();
        assert!(hint
            .items
            .iter()
            .any(|(k, v)| k == "-a" && v.starts_with("[x] ")));
        let tr = app.transient_popup(tab).unwrap();
        assert!(tr.switches.iter().any(|s| s.key == "-a" && s.checked));

        // Dismiss popup
        app.dismiss_which_key_popup();
        let slot = app.open_tabs.page(&tab).unwrap();
        assert!(slot.page.hint().is_none());
    }

    #[test]
    fn test_git_rebase_popup_transient_view_opens_and_dismisses() {
        let mut app = App::new();
        let editor =
            winter_git::GitEditor::new(std::path::PathBuf::from("/repo"), git_editor_command());
        let tab = app.show_editor(crate::window::editor::EditorKind::Git, Box::new(editor));
        // Press 'r' to open Rebase menu
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('r')));

        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.title, "Rebase");
        assert_eq!(tr.commands.len(), 6);
        let kinds: Vec<_> = tr
            .switches
            .iter()
            .map(|s| (s.key.as_str(), s.kind))
            .collect();
        assert_eq!(
            kinds,
            [
                ("-c", crate::render::SwitchKind::Radio),
                ("-d", crate::render::SwitchKind::Radio),
                ("-A", crate::render::SwitchKind::Checkbox),
            ],
            "the dates switches are a pair, the stash one stands alone"
        );

        // Dismiss rebase popup
        app.dismiss_which_key_popup();
        assert!(app.transient_popup(tab).is_none());
    }

    #[test]
    fn test_git_merge_radio_switches_are_mutually_exclusive() {
        let mut app = App::new();
        let editor =
            winter_git::GitEditor::new(std::path::PathBuf::from("/repo"), git_editor_command());
        let tab = app.show_editor(crate::window::editor::EditorKind::Git, Box::new(editor));
        // Press 'm' to open Merge menu
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('m')));

        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.title, "Merge");
        assert_eq!(tr.switches[0].kind, crate::render::SwitchKind::Radio);
        assert_eq!(tr.switches[1].kind, crate::render::SwitchKind::Radio);

        // Click -f (--ff-only)
        app.click_which_key("-f");
        let tr = app.transient_popup(tab).unwrap();
        assert!(tr.switches.iter().any(|s| s.key == "-f" && s.checked));
        assert!(tr.switches.iter().any(|s| s.key == "-n" && !s.checked));

        // Click -n (--no-ff): mutual exclusion deselects -f
        app.click_which_key("-n");
        let tr = app.transient_popup(tab).unwrap();
        assert!(tr.switches.iter().any(|s| s.key == "-f" && !s.checked));
        assert!(tr.switches.iter().any(|s| s.key == "-n" && s.checked));
    }

    #[test]
    fn test_transient_keyboard_selection_and_arrows_and_ctrl_pn() {
        let mut app = App::new();
        let editor =
            winter_git::GitEditor::new(std::path::PathBuf::from("/repo"), git_editor_command());
        let tab = app.show_editor(crate::window::editor::EditorKind::Git, Box::new(editor));
        // Press 'm' to open Merge menu
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('m')));

        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 0);
        let total = tr.switches.len() + tr.commands.len();
        assert!(total > 2);

        // ArrowDown moves selection to 1
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Down));
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 1);

        // Ctrl-n moves selection to 2
        app.offer_key_to_editor(
            tab,
            &Key {
                alt: false,
                code: KeyCode::Char('n'),
                ctrl: true,
                shift: false,
                sup: false,
            },
        );
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 2);

        // ArrowUp moves selection back to 1
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Up));
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 1);

        // Ctrl-p moves selection back to 0
        app.offer_key_to_editor(
            tab,
            &Key {
                alt: false,
                code: KeyCode::Char('p'),
                ctrl: true,
                shift: false,
                sup: false,
            },
        );
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 0);

        // ArrowUp at 0 saturates at 0
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Up));
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 0);

        // ArrowDown past end clamps at total - 1
        for _ in 0..10 {
            app.offer_key_to_editor(tab, &Key::plain(KeyCode::Down));
        }
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, total - 1);
    }

    #[test]
    fn test_transient_keyboard_activation_toggle_switch_and_dismiss() {
        let mut app = App::new();
        let editor =
            winter_git::GitEditor::new(std::path::PathBuf::from("/repo"), git_editor_command());
        let tab = app.show_editor(crate::window::editor::EditorKind::Git, Box::new(editor));
        // Press 'm' to open Merge menu
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('m')));

        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.selected, 0);
        assert!(!tr.switches[0].checked);

        // Space toggles switch at index 0 (-f)
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Space));
        let tr = app.transient_popup(tab).unwrap();
        assert!(tr.switches[0].checked);

        // Enter toggles switch at index 0 off
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Enter));
        let tr = app.transient_popup(tab).unwrap();
        assert!(!tr.switches[0].checked);

        // Escape dismisses transient popup
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Escape));
        assert!(app.transient_popup(tab).is_none());
    }

    #[test]
    fn test_transient_parameter_input_returns_to_parent_window() {
        let mut app = App::new();
        let editor =
            winter_git::GitEditor::new(std::path::PathBuf::from("/repo"), git_editor_command());
        let tab = app.show_editor(crate::window::editor::EditorKind::Git, Box::new(editor));
        // Press 'c' to open Commit menu
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('c')));

        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.title, "Commit");
        assert!(!tr.commands.is_empty());

        // Press '-' to enter parameter mode
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('-')));
        let tr = app.transient_popup(tab).unwrap();
        // In parameter mode, RET back command is shown
        assert_eq!(tr.commands[0].key, "RET");

        // Press 'a' to toggle -a (Stage all)
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('a')));
        let tr = app.transient_popup(tab).unwrap();
        assert!(tr.switches.iter().any(|sw| sw.key == "-a" && sw.checked));
        // After pressing letter, returns to parent transient window!
        assert!(tr.commands.iter().any(|cmd| cmd.key == "c"));

        // Press '-' again to toggle -s (Signoff)
        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('-')));
        let tr = app.transient_popup(tab).unwrap();
        assert_eq!(tr.commands[0].key, "RET");

        app.offer_key_to_editor(tab, &Key::plain(KeyCode::Char('s')));
        let tr = app.transient_popup(tab).unwrap();
        // Back in parent transient window with normal commands
        assert!(tr.commands.iter().any(|cmd| cmd.key == "c"));
        // Both switches remain checked!
        assert!(tr.switches.iter().any(|sw| sw.key == "-a" && sw.checked));
        assert!(tr.switches.iter().any(|sw| sw.key == "-s" && sw.checked));
    }

    #[test]
    fn test_a_click_leaves_the_block_cursor_and_a_drag_moves_it() {
        // Regression: a plain click in Normal mode parked the block cursor
        // under the pointer, so it seemed to appear wherever you clicked.
        let (mut app, id) = app_pane_and_id();
        app.open_tabs.set_mode(&id, Mode::Normal);
        app.set_nav_cursor(id, (1, 1));

        app.press_grid_cell(id, 3, 7);
        assert_eq!(app.nav_cursor(id), Some((1, 1)), "a click keeps the cursor");
        assert!(
            app.input.pointer.press_cell.is_some(),
            "a drag can still anchor"
        );

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
            .split(winter_model::layout::Direction::Vertical, 0.5, right);
        let mut terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_emulator::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        terminal.grid_mut().scroll_up(20);
        app.open_tabs.insert_terminal(right, terminal);
        app.open_tabs.set_mode(&right, Mode::Insert);

        // Focus is on `right` after the split; point at `left`.
        assert_eq!(app.layout().focused(), right);
        let rect = app.pane_rect(left).expect("left is shown");
        app.input.pointer.cursor_pos = (rect.x + 1.0, rect.y + 1.0);

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
        app.term.selection.span = Some(Selection {
            block: false,
            start_row: 3,
            start_col: 0,
            end_row: 5,
            end_col: 10,
            tab: id,
        });

        app.on_mouse_wheel(winit::event::MouseScrollDelta::LineDelta(0.0, -1.0));

        assert!(
            app.term.selection.span.is_none(),
            "the text under the highlight is about to change, so the selection goes"
        );
    }

    /// An editor that scrolls by whole rows and keeps each scroll it was
    /// offered, for a test to read back.
    struct Scrolls(std::rc::Rc<std::cell::RefCell<Vec<isize>>>);

    impl winter_model::editor::Page for Scrolls {
        fn title(&self) -> String {
            "Scrolls".to_string()
        }

        fn content(
            &mut self,
            _rows: usize,
            _cols: usize,
            _wrap: bool,
        ) -> winter_model::paint::Frame {
            winter_model::paint::Frame::new(Vec::new())
        }

        fn on_key(
            &mut self,
            _key: &winter_model::input::Key,
        ) -> winter_model::editor::EditorOutcome {
            winter_model::editor::EditorOutcome::Ignored
        }

        fn on_scroll(&mut self, lines: isize) -> winter_model::editor::EditorOutcome {
            self.0.borrow_mut().push(lines);
            winter_model::editor::EditorOutcome::Consumed
        }
    }

    impl winter_model::editor::Editor for Scrolls {}

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
        assert_eq!(
            scrolled.borrow().iter().sum::<isize>(),
            -3,
            "{:?}",
            scrolled.borrow()
        );

        // Part of a row left over going down is not taken off a notch back up.
        scrolled.borrow_mut().clear();
        for y in [-0.15, 1.0] {
            app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, y));
            let mut at = Some(Instant::now());
            while let Some(now) = at {
                at = app.step_wheel(now + WHEEL_FRAME);
            }
        }
        assert_eq!(
            scrolled.borrow().iter().sum::<isize>(),
            3,
            "{:?}",
            scrolled.borrow()
        );
    }

    #[test]
    fn test_the_pdf_viewer_glides_a_wheel_turn_itself() {
        let pdf = winter_pdf::PdfEditor::new(std::path::PathBuf::from("/nonexistent.pdf"));
        assert!(winter_model::editor::Page::scrolls_smoothly(&pdf));
    }

    struct MockScrollableEditor {
        scroll: usize,
        visible: usize,
        total: usize,
    }

    impl winter_model::editor::Page for MockScrollableEditor {
        fn title(&self) -> String {
            "MockScrollable".to_string()
        }

        fn content(
            &mut self,
            _rows: usize,
            _cols: usize,
            _wrap: bool,
        ) -> winter_model::paint::Frame {
            winter_model::paint::Frame::new(Vec::new())
        }

        fn on_key(
            &mut self,
            _key: &winter_model::input::Key,
        ) -> winter_model::editor::EditorOutcome {
            winter_model::editor::EditorOutcome::Ignored
        }

        fn scroll_position(&self) -> Option<winter_model::editor::ScrollPosition> {
            Some(winter_model::editor::ScrollPosition::new(
                self.scroll,
                self.visible,
                self.total,
            ))
        }

        fn scroll_to(&mut self, offset: usize) -> winter_model::editor::EditorOutcome {
            self.scroll = offset;
            winter_model::editor::EditorOutcome::Consumed
        }
    }

    impl winter_model::editor::Editor for MockScrollableEditor {}

    #[test]
    fn test_clicking_and_dragging_editor_scrollbar() {
        let mut app = App::new();
        let editor = Box::new(MockScrollableEditor {
            scroll: 0,
            visible: 20,
            total: 100,
        });
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, editor);
        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .unwrap();
        let pr = App::layout_rect_to_pane(rect);

        // Click near the right edge on the scrollbar (halfway down)
        let sb_x = pr.x + pr.width - 2.0;
        let mid_y = pr.y + pr.height * 0.5;
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );

        assert_eq!(app.input.pointer.scrollbar_drag.map(|d| d.tab), Some(tab));
        let slot = app.open_tabs.page(&tab).unwrap();
        let pos = slot.page.scroll_position().unwrap();
        assert_eq!(pos.offset, 40);

        // Drag down to 80%
        let drag_y = pr.y + pr.height * 0.8;
        app.on_cursor_moved((sb_x as f64, drag_y as f64).into());
        let slot = app.open_tabs.page(&tab).unwrap();
        let pos = slot.page.scroll_position().unwrap();
        assert_eq!(pos.offset, 70);

        // Release ends the drag
        app.handle_mouse_input(
            winit::event::ElementState::Released,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.input.pointer.scrollbar_drag, None);
        assert_eq!(app.input.pointer.scrollbar_hover, Some(tab));
    }

    #[test]
    fn test_scrollbar_hover_tracks_pointer_position() {
        let mut app = App::new();
        let editor = Box::new(MockScrollableEditor {
            scroll: 0,
            visible: 20,
            total: 100,
        });
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, editor);
        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .unwrap();
        let pr = App::layout_rect_to_pane(rect);

        // Hover over scrollbar
        let sb_x = pr.x + pr.width - 2.0;
        let mid_y = pr.y + pr.height * 0.5;
        app.dirty = false;
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        assert_eq!(app.input.pointer.scrollbar_hover, Some(tab));
        assert!(app.dirty, "moving onto scrollbar marks window dirty");

        // Move cursor away into the content
        app.dirty = false;
        app.on_cursor_moved(((pr.x + 10.0) as f64, mid_y as f64).into());
        assert_eq!(app.input.pointer.scrollbar_hover, None);
        assert!(app.dirty, "moving off scrollbar marks window dirty");

        // Move back onto scrollbar, then leave window
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        assert_eq!(app.input.pointer.scrollbar_hover, Some(tab));
        app.dirty = false;
        app.on_cursor_left();
        assert_eq!(app.input.pointer.scrollbar_hover, None);
        assert!(app.dirty, "cursor leaving window clears hover and dirties");
    }

    #[test]
    fn test_the_pointer_takes_the_lit_scrollbar_back_from_the_keyboard() {
        let mut app = App::new();
        let tab = app.layout().focused();
        // Moving the pointer hands the lit scrollbar back to its hover.
        app.input.pointer.scrollbar_keyed = Some(tab);
        app.dirty = false;
        app.on_cursor_moved((5.0, 5.0).into());
        assert_eq!(app.input.pointer.scrollbar_keyed, None);
        assert!(app.dirty, "the scrollbar is drawn again unlit");
        // So does turning the wheel, which may scroll another pane.
        app.input.pointer.scrollbar_keyed = Some(tab);
        app.on_mouse_wheel(MouseScrollDelta::LineDelta(0.0, 1.0));
        assert_eq!(app.input.pointer.scrollbar_keyed, None);
    }

    #[test]
    fn test_clicking_and_dragging_terminal_scrollbar() {
        let (mut app, id) = app_pane_and_id();
        let terminal = pane_with_scrollback();
        let sbl = terminal.grid().scrollback_len();
        assert!(sbl > 0);
        app.open_tabs.insert_terminal(id, terminal);

        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(t, _)| *t == id)
            .unwrap();
        let pr = App::layout_rect_to_pane(rect);
        let sb_x = pr.x + pr.width - 2.0;

        // Terminal starts at live bottom (scroll_offset == 0). Click near top of track.
        let top_y = pr.y + 10.0;
        app.on_cursor_moved((sb_x as f64, top_y as f64).into());
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );

        assert_eq!(app.input.pointer.scrollbar_drag.map(|d| d.tab), Some(id));
        let term = app.open_tabs.terminal(&id).unwrap();
        assert!(
            term.grid().scroll_offset() > 0,
            "clicking near top scrolls back"
        );

        // Drag down towards bottom
        let bottom_y = pr.y + pr.height - 10.0;
        app.on_cursor_moved((sb_x as f64, bottom_y as f64).into());
        let term = app.open_tabs.terminal(&id).unwrap();
        assert_eq!(
            term.grid().scroll_offset(),
            0,
            "dragging to bottom returns to live bottom"
        );

        // Release ends the drag
        app.handle_mouse_input(
            winit::event::ElementState::Released,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.input.pointer.scrollbar_drag, None);
    }

    #[test]
    fn test_scrollbar_takes_priority_over_window_edge_resize_in_modern_mode() {
        use crate::config::TitleBarStyle;

        let mut app = App::new();
        app.config.title_bar_style = TitleBarStyle::Modern;

        let editor = Box::new(MockScrollableEditor {
            scroll: 0,
            visible: 20,
            total: 100,
        });
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, editor);
        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .unwrap();
        let pr = App::layout_rect_to_pane(rect);

        // Click right inside the 6px window resize border (2px from the window right edge)
        let sb_x = pr.x + pr.width - 2.0;
        let mid_y = pr.y + pr.height * 0.5;

        // Edge resize must be suppressed over an active scrollbar
        assert_eq!(
            app.edge_resize_direction(sb_x, mid_y),
            None,
            "scrollbar suppresses edge resize"
        );

        // Left click initiates scrollbar drag
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.input.pointer.scrollbar_drag.map(|d| d.tab), Some(tab));
    }

    #[test]
    fn test_a_press_behind_a_dialog_turns_it_down_and_reaches_nothing() {
        use crate::window::editor::PendingClose;
        use crate::window::input::overlay::{ClosePrompt, Overlay};

        let mut app = App::new();
        let editor = Box::new(MockScrollableEditor {
            scroll: 0,
            visible: 20,
            total: 100,
        });
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, editor);
        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .unwrap();
        let pr = App::layout_rect_to_pane(rect);
        app.open_overlay(Overlay::Close(ClosePrompt {
            buttons: winter_model::button::ButtonGroup::confirm_as("Discard"),
            label: "Unsaved edits in main.rs. Close the tab? (y/n) ".to_string(),
            pending: PendingClose::Tab(tab),
            title: "Discard Unsaved Changes?",
        }));

        // A press on the pane's scrollbar, behind the dialog.
        let (sb_x, mid_y) = (pr.x + pr.width - 2.0, pr.y + pr.height * 0.5);
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        assert_eq!(app.cursor_icon_at(sb_x, mid_y), CursorIcon::Default);
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );
        assert!(
            app.input.pointer.scrollbar_drag.is_none(),
            "the pane took no press"
        );
        assert!(
            app.chrome.close_prompt().is_none(),
            "a press off the dialog turns it down"
        );
        assert!(
            app.open_tabs.page(&tab).is_some(),
            "and turning it down closes nothing"
        );
    }

    #[test]
    fn test_cursor_icon_transitions_between_text_and_scrollbar() {
        let mut app = App::new();
        let editor = Box::new(MockScrollableEditor {
            scroll: 0,
            visible: 20,
            total: 100,
        });
        let tab = app.show_editor(crate::window::editor::EditorKind::Text, editor);
        let (_, rect) = app
            .pane_rects()
            .into_iter()
            .find(|(id, _)| *id == tab)
            .unwrap();
        let pr = App::layout_rect_to_pane(rect);

        // Over text area in the middle of pane -> CursorIcon::Text
        let text_x = pr.x + 20.0;
        let mid_y = pr.y + pr.height * 0.5;
        app.on_cursor_moved((text_x as f64, mid_y as f64).into());
        assert_eq!(app.cursor_icon_at(text_x, mid_y), CursorIcon::Text);

        // Over scrollbar on the right edge -> CursorIcon::Default
        let sb_x = pr.x + pr.width - 2.0;
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        assert_eq!(app.cursor_icon_at(sb_x, mid_y), CursorIcon::Default);

        // While dragging scrollbar -> CursorIcon::Default
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.cursor_icon_at(sb_x, mid_y), CursorIcon::Default);
    }

    #[test]
    fn test_clicking_scrollbar_near_split_divider_does_not_drag_divider() {
        let (mut app, left) = app_pane_and_id();
        let terminal = pane_with_scrollback();
        app.open_tabs.insert_terminal(left, terminal);

        // Split vertically so `left` is on the left and `right` is on the right
        let right = app.alloc_tab_id();
        app.layout_mut()
            .split(winter_model::layout::Direction::Vertical, 0.5, right);
        let right_term = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_emulator::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        app.open_tabs.insert_terminal(right, right_term);

        let left_rect = app.pane_rect(left).unwrap();
        let initial_width = left_rect.width;

        // Position on left pane's scrollbar, which sits right beside the divider
        let sb_x = left_rect.x + left_rect.width - 3.0;
        let mid_y = left_rect.y + left_rect.height * 0.5;

        // Over scrollbar: cursor must be Default, NOT EwResize
        app.on_cursor_moved((sb_x as f64, mid_y as f64).into());
        assert_eq!(app.cursor_icon_at(sb_x, mid_y), CursorIcon::Default);

        // Click on scrollbar: must initiate scrollbar drag, NOT divider drag
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.input.pointer.scrollbar_drag.map(|d| d.tab), Some(left));
        assert_eq!(
            app.input.pointer.divider_drag, None,
            "divider drag must not be started"
        );

        // Dragging horizontally left and right must not resize the pane
        app.on_cursor_moved(((sb_x - 50.0) as f64, mid_y as f64).into());
        let current_left_rect = app.pane_rect(left).unwrap();
        assert_eq!(
            current_left_rect.width, initial_width,
            "pane must not resize on scrollbar drag"
        );

        app.on_cursor_moved(((sb_x + 50.0) as f64, mid_y as f64).into());
        let current_left_rect = app.pane_rect(left).unwrap();
        assert_eq!(
            current_left_rect.width, initial_width,
            "pane must not resize on scrollbar drag"
        );

        app.handle_mouse_input(
            winit::event::ElementState::Released,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.input.pointer.scrollbar_drag, None);

        // Now test hovering directly over the separator bar
        let div_x = left_rect.x + left_rect.width;
        app.on_cursor_moved((div_x as f64, mid_y as f64).into());
        assert_eq!(
            app.cursor_icon_at(div_x, mid_y),
            CursorIcon::EwResize,
            "cursor over separator bar must be resize cursor"
        );
        assert_eq!(
            app.hovered_scrollbar_pane_at(div_x, mid_y, &app.pane_rects()),
            None,
            "separator bar must not be hit as a scrollbar"
        );

        // Clicking on the separator bar starts divider drag
        app.handle_mouse_input(
            winit::event::ElementState::Pressed,
            winit::event::MouseButton::Left,
        );
        assert_eq!(app.input.pointer.scrollbar_drag, None);
        assert_eq!(app.input.pointer.divider_drag, Some((div_x, mid_y)));

        // Dragging the divider resizes the panes
        app.on_cursor_moved(((div_x + 20.0) as f64, mid_y as f64).into());
        let resized_left_rect = app.pane_rect(left).unwrap();
        assert!(
            resized_left_rect.width > initial_width,
            "dragging separator bar must resize the pane"
        );
        app.handle_mouse_input(
            winit::event::ElementState::Released,
            winit::event::MouseButton::Left,
        );
    }
}

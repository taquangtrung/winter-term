//! Handlers for the winit window events the event loop dispatches.

use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::keyboard::{Key, PhysicalKey};

use crate::model::page::PagePoint;
use winit::event_loop::ActiveEventLoop;
use winit::window::CursorIcon;

use crate::model::input::{self, KeyCode};
use crate::model::layout::TabId;
use crate::model::mode::Mode;
use crate::model::textedit::modal::EditOutcome;

#[cfg(target_os = "windows")]
use super::is_pre_focus_key_leak;
use super::strip::StripHit;
use super::App;
use super::Selection;
use super::{
    escape_clears_selection, escape_forwarded_to_pty, forwarded_to_pty, winit_key_to_code,
    APPROX_CELL_HEIGHT, CURSOR_BLINK_PERIOD, SCROLLBAR_CLICK_WIDTH, SCROLL_LINES_PER_WHEEL_NOTCH,
};

// ========================================================================
// App: window-event handlers
// ========================================================================

impl App {
    /// Route one key press or release: the chrome overlays first (command
    /// palette, settings page, which-key), then the focused pane's modal
    /// keymap, and finally the PTY.
    pub(crate) fn on_keyboard_input(&mut self, event: KeyEvent, event_loop: &ActiveEventLoop) {
        if self.handle_key(event.state, event.logical_key, event.physical_key) {
            self.quit(event_loop);
        }
    }

    /// The keyboard handler proper, split from [`Self::on_keyboard_input`] so
    /// tests can drive it without an `ActiveEventLoop` (winit's `KeyEvent`
    /// has no public constructor). Returns `true` when the app should quit,
    /// standing in for the former inline `quit(event_loop); return;` sites.
    pub(super) fn handle_key(
        &mut self,
        state: ElementState,
        logical_key: Key,
        physical_key: PhysicalKey,
    ) -> bool {
        // winit synthesizes KeyboardInput::Pressed events for every
        // physically-held key on XI_FocusIn (handle_pressed_keys). Swallow
        // those here so e.g. the Tab from Alt+Tab never reaches the PTY.
        if state == ElementState::Pressed && self.suppress_synthesized_keys {
            return false;
        }

        // Windows-only: drop a key event that raced ahead of this
        // window's own focus-gain notification (see
        // `is_pre_focus_key_leak`'s doc for why this can happen).
        #[cfg(target_os = "windows")]
        if is_pre_focus_key_leak(state, self.window.as_ref().is_some_and(|w| w.has_focus())) {
            return false;
        }

        let focused = self.layout().focused();
        let mods_state = self.modifiers.state();
        let code = winit_key_to_code(&logical_key, &physical_key);
        let key = input::Key {
            alt: mods_state.alt_key(),
            code,
            ctrl: mods_state.control_key(),
            shift: mods_state.shift_key(),
        };
        if state == ElementState::Released {
            let kitty_flags = self
                .open_tabs
                .terminal(&focused)
                .map(|p| p.kitty_flags())
                .unwrap_or(0);
            let bytes = input::encode_release(&key, kitty_flags);
            if !bytes.is_empty() {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal.write(&bytes);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return false;
        }

        // A key winit cannot name, such as Ctrl or Shift going down, types
        // nothing and means nothing. Left to go on, it reaches whatever is
        // taking text, which writes the NUL it decodes to as a stray glyph.
        if key.is_unidentified() {
            return false;
        }

        // Reset blink to the "visible" phase on every key press so the
        // cursor is immediately shown as feedback for the action.
        if self.config.cursor.blink {
            self.blink_phase = true;
            self.blink_next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
        }

        // A page tab takes its own keys rather than being in a mode; `mode` is
        // the terminal's, and only means anything when the tab is not a page.
        let page_focused = self.open_tabs.is_page(&focused);
        let mode = self.open_tabs.mode(&focused).unwrap_or_default();

        // While a tab rename is in progress, intercept all keyboard
        // input: Enter confirms, Escape cancels, other keys edit the name.
        if self.tabs.rename_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let name = self.tabs.rename_input.take().unwrap();
                    let focused = self.layout().focused();
                    if name.is_empty() {
                        self.tabs.names.remove(&focused);
                    } else {
                        self.tabs.names.insert(focused, name);
                    }
                }
                KeyCode::Escape => {
                    self.tabs.rename_input = None;
                }
                KeyCode::Backspace => {
                    if let Some(input) = &mut self.tabs.rename_input {
                        input.pop();
                    }
                }
                KeyCode::Char(c) if !key.ctrl && !key.alt => {
                    if let Some(input) = &mut self.tabs.rename_input {
                        input.push(c);
                    }
                }
                _ => {}
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // A page's prompt owns the keyboard while it is up, ahead of the
        // window chords: a filename being typed must not trigger one.
        if self.page_prompt.is_some() {
            self.handle_prompt_key(&key);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // While a new-theme name is being entered, intercept all keyboard
        // input the same way: Enter confirms, Escape cancels, other keys
        // edit the name.
        if let Some(input) = self.theme_name_input.as_mut() {
            match input.on_key(&key.typed(), 1) {
                EditOutcome::Submit => {
                    let name = input.text().to_string();
                    self.theme_name_input = None;
                    self.create_named_theme(&name);
                }
                EditOutcome::Cancel => self.theme_name_input = None,
                _ => {}
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // While the settings page is up it owns all input: edits apply
        // live, Enter/Escape close it, and every key is swallowed so none
        // reaches the PTY.
        if self.settings_page.is_some() {
            let logical = logical_key.clone();
            let mut page = self.settings_page.take().unwrap();
            if self.handle_settings_input(&mut page, &logical) {
                self.settings_page = Some(page);
            } else {
                self.on_settings_closed();
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // Esc dismisses an open menu before anything else acts on it.
        if self.menus.open.is_some() && key.code == KeyCode::Escape {
            self.close_menu();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // Esc clears a mouse-drag selection before anything else acts on it.
        if escape_clears_selection(&key, mode, self.selection.span.is_some()) {
            self.selection.span = None;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // Global app shortcuts (settings, font size, tab/pane management,
        // palette toggles): configurable via the `window` block in
        // keybindings.kdl, and checked here so they intercept even while
        // an overlay (palette, tab rename, settings) would otherwise own
        // the key. Window-layout chords (split/close/focus/scroll/zoom)
        // are resolved later, once no overlay claims the key.
        let global = self
            .window_keymap
            .global_action(&key)
            .filter(|action| !self.is_copy_yielded_to_page(action, focused));
        if let Some(action) = global {
            self.handle_action(action, focused);
            if self.exit_requested {
                self.exit_requested = false;
                return true;
            }
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        if self.palette.is_some() {
            let key = logical_key.clone();
            let mut palette = self.palette.take().unwrap();
            self.handle_palette_input(&mut palette, &key, &physical_key, focused);
            if palette.active {
                self.palette = Some(palette);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // The page text cursor takes every key while it is up, ahead of the
        // page itself: sharing keys would move the selection and the listing
        // at once. See `page_cursor`.
        if self.handle_page_cursor_key(&key) {
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // A page owns the keys it binds, and only those: anything it declines
        // falls through to the window chords below, so the vim vocabulary
        // reads the same in a page as in a terminal.
        if self.offer_key_to_page(focused, &key) {
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // `v` in a page that did not bind it starts a text cursor, the way it
        // starts a Visual selection in a terminal.
        if page_focused && key.code == KeyCode::Char('v') && !key.alt && !key.ctrl && !key.shift {
            self.start_page_cursor(focused);
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return false;
        }

        // In Normal mode, a bare Escape first goes to whatever Winter is
        // holding open (a half-typed command, a search, a quick-select);
        // only when there is none, and a program is running (or the shell is
        // mid tab-completion), is it written to the PTY.
        // In Insert mode, bare Escape is not intercepted here and passes
        // through to the PTY. Normal mode is entered via Alt-Escape or Ctrl-Shift-Space.
        let bare_esc = key.code == KeyCode::Escape && !key.alt && !key.ctrl && !key.shift;
        if bare_esc && mode == Mode::Normal && !self.escape_has_winter_target() {
            let pending_tab_completion = self
                .open_tabs
                .terminal_tab_mut(&focused)
                .is_some_and(|tab| std::mem::take(&mut tab.pending_tab_completion));
            let has_foreground_process = self
                .open_tabs
                .terminal(&focused)
                .is_some_and(|p| p.has_foreground_process());
            if escape_forwarded_to_pty(has_foreground_process, pending_tab_completion) {
                if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
                    terminal.write(&[0x1b]);
                }
                return false;
            }
        }
        let at_prompt = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.is_at_prompt());
        let kitty_flags = self
            .open_tabs
            .terminal(&focused)
            .map(|p| p.kitty_flags())
            .unwrap_or(0);
        let modify_other_keys = self
            .open_tabs
            .terminal(&focused)
            .map(|p| p.modify_other_keys())
            .unwrap_or(None);
        let is_alt_screen = self
            .open_tabs
            .terminal(&focused)
            .map(|p| p.grid().is_alt_screen())
            .unwrap_or(false);
        let prev_pending = self.pending;
        let action = match page_focused {
            true => input::resolve_page(&key, &self.window_keymap),
            false => input::resolve_with(
                mode,
                &key,
                &mut self.pending,
                &self.window_keymap,
                kitty_flags,
                modify_other_keys,
                is_alt_screen,
            ),
        };
        if self.pending != prev_pending {
            self.pending_since = if self.pending.hint().is_some() {
                Some(Instant::now())
            } else {
                None
            };
        } else if self.pending == input::PendingPrefix::None {
            self.pending_since = None;
        }
        // Keep the per-pane prompt shadow in step with this key so
        // `Ctrl-/`/`Ctrl-\` can replay edits. Only forwarded keys mutate
        // the line (`apply_insert_key` models them or desyncs on the
        // unmodeled ones); `Edit`/undo/redo update it in their handlers.
        // Mode switches, focus and tab commands leave this line untouched,
        // so the shadow is preserved (undo still works in Normal mode).
        if !page_focused && mode == Mode::Insert && forwarded_to_pty(&action) {
            self.record_insert_key(focused, &key, &action, at_prompt);
            // Deliberately does NOT queue an automatic switch to
            // Normal mode once a submitted command's new prompt
            // settles - Insert mode always resumes after running a
            // command, exactly as it was before that command ran;
            // only an explicit mode-switch chord enters Normal mode.
        }
        self.handle_action(action, focused);
        if self.exit_requested {
            self.exit_requested = false;
            return true;
        }
        self.update_window_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
        false
    }
    /// Route a mouse button press or release. The tabbar and any open menu
    /// take precedence over the panes, so a click on chrome never reaches a
    /// pane underneath it.
    /// Whether the global copy chord, with nothing selected, should pass to
    /// the page in tab `focused` instead: with nothing to copy it
    /// would do nothing, and a page may bind the same chord (the Git page
    /// commits on `Ctrl-Shift-c`). A page that declines it still reaches the
    /// ordinary keymap, where it resolves to the same harmless copy.
    fn is_copy_yielded_to_page(&self, action: &input::Action, focused: TabId) -> bool {
        *action == input::Action::Copy
            && self.palette.is_none()
            && self.open_tabs.is_page(&focused)
            && self.selected_text().is_none()
    }

    pub(crate) fn on_mouse_input(
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
            if (self.menus.open.is_some() || y < self.top_chrome_height())
                && self.handle_tabbar_click(x, y)
            {
                if self.exit_requested {
                    self.exit_requested = false;
                    self.quit(event_loop);
                    return;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
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
                if self.exit_requested {
                    self.exit_requested = false;
                    self.quit(event_loop);
                    return;
                }
                if button == MouseButton::Left {
                    self.pointer.mouse_down = true;
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
        }

        // Ctrl+click on a hyperlinked cell or printed URL/file opens it.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.modifiers.state().control_key()
        {
            let (x, y) = self.pointer.cursor_pos;
            if self.open_file_at_pixel(x, y) {
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
                return;
            }
            if let Some(url) = &self.pointer.hovered_url {
                let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
                if matches!(scheme.as_str(), "http" | "https" | "mailto" | "file") {
                    let _ = open::that(url);
                    return;
                }
            }
        }

        // Plain clicks always drive Winter's own selection, even over a
        // full-screen app (vim, htop) that has turned on mouse reporting.
        // Hold Shift to forward the click to that app's mouse handling instead.
        let mouse_active = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_tracking())
            && self.modifiers.state().shift_key();

        if mouse_active {
            self.forward_mouse_event(state, button, focused);
            return;
        }

        // Right-click: paste if configured, otherwise open the context menu.
        if state == ElementState::Pressed && button == MouseButton::Right {
            if self.config.paste_on_right_click {
                self.paste_from_clipboard();
            } else {
                let (x, y) = self.pointer.cursor_pos;
                self.open_context_menu(x, y);
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return;
        }

        // Any left-press dismisses an open context menu before other handling.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.menus.context_pos.is_some()
        {
            let (x, y) = self.pointer.cursor_pos;
            let surface_w = self.viewport_rect().width;
            if let Some((cw, ch)) = self.renderer.as_ref().map(|r| r.cell_size()) {
                let tabbar = self.build_top_tabbar();
                let hit = winter_render::hit_test(&tabbar, surface_w, cw, ch, x, y);
                if let winter_render::TabbarHit::ContextMenuItem(i) = hit {
                    if let Some(action) = self.menus.context_actions.get(i).cloned() {
                        self.close_context_menu();
                        self.run_context_action(action);
                        if let Some(window) = &self.window {
                            window.request_redraw();
                        }
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
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    break 'scroll;
                                }
                            }
                        }
                    }

                    if let Some((tab_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        self.layout_mut().focus(tab_id);
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        // A page tab owns what is drawn there, so the click is
                        // its to answer before the grid's.
                        let at = PagePoint {
                            col,
                            drag: false,
                            row,
                        };
                        if self.offer_mouse_to_page(tab_id, at) {
                            self.pointer.press_cell = None;
                            self.dirty = true;
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                            return;
                        }
                        // A click parks the traversal cursor under the
                        // pointer, so selecting with the mouse moves the
                        // cursor instead of leaving it wherever the last
                        // keyboard motion stopped.
                        self.track_nav_cursor_to_mouse(tab_id, row, col);
                        // Anchor any drag that follows at the press, not at
                        // the first motion event: see `PointerState::press_cell`.
                        self.pointer.press_cell = self
                            .selection_grid(tab_id)
                            .map(|grid| (tab_id, grid.to_absolute_row(row), col));
                        let now = Instant::now();
                        if let Some((prev_time, prev_x, prev_y)) = self.pointer.last_click {
                            let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
                            if now.duration_since(prev_time) < Duration::from_millis(400)
                                && dist < 5.0
                            {
                                self.select_word_at(tab_id, row, col);
                            }
                        }
                        self.pointer.last_click = Some((now, x, y));
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
                    window.request_redraw();
                }
            }
            (ElementState::Pressed, MouseButton::Middle) => {
                self.paste_from_primary();
            }
            _ => {}
        }
    }
    /// Track the pointer: hover highlights, drag-selection extension, split
    /// dragging, and the resize-edge cursor shape.
    pub(crate) fn on_cursor_moved(&mut self, position: PhysicalPosition<f64>) {
        let x = position.x as f32;
        let y = position.y as f32;
        self.pointer.cursor_pos = (x, y);

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
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
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
                            if let Some(window) = &self.window {
                                window.request_redraw();
                            }
                        }
                    }
                }
                return;
            }
        }

        if self.menus.open.is_some() {
            self.update_menu_hover(x, y);
        }
        if self.menus.context_pos.is_some() {
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
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }
        // The pane strips' hover, which also carries a tab's full title as a
        // tooltip, since a strip may have had to cut it short.
        let strip_before = self.tabs.strip_hover;
        self.update_strip_hover(x, y);
        if self.tabs.strip_hover != strip_before {
            self.tabs.hover_pos = match self.tabs.strip_hover {
                StripHit::Tab(_) => Some(self.pointer.cursor_pos),
                _ => None,
            };
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        let focused = self.layout().focused();
        if self.pointer.mouse_down
            && self
                .open_tabs
                .terminal(&focused)
                .is_some_and(|p| p.mouse_drag_tracking())
            && self.modifiers.state().shift_key()
        {
            self.forward_mouse_motion(focused);
            return;
        }

        let is_tab_dragging = self.tabs.drag_start.is_some_and(|(_, sx, sy)| {
            ((x - sx).powi(2) + (y - sy).powi(2)).sqrt() >= super::strip::DRAG_THRESHOLD_PX
        });
        if self.tabs.drag_start.is_some() {
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }

        if self.pointer.mouse_down && self.tabs.drag_start.is_none() {
            // Resolved against the pane the drag started in, not just the one
            // under the pointer, so running off an edge keeps selecting.
            if let Some((tab_id, row, col)) = self.drag_cell_at(x, y) {
                let at = PagePoint {
                    col,
                    drag: true,
                    row,
                };
                if self.offer_mouse_to_page(tab_id, at) {
                    self.dirty = true;
                    if let Some(window) = &self.window {
                        window.request_redraw();
                    }
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
                } else {
                    let (start_row, start_col) = self.pointer.drag_anchor(tab_id, (abs_row, col));
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
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        // Update hovered hyperlink and cursor icon (divider resize, tab drag, or pointer).
        let new_url = self.hovered_link_at(x, y);
        self.pointer.hovered_url = new_url;
        let vp = self.content_viewport();
        let icon = if let Some(direction) = self.edge_resize_direction(x, y) {
            CursorIcon::from(direction)
        } else if is_tab_dragging {
            CursorIcon::Grabbing
        } else {
            match self.layout().divider_at(x, y, vp) {
                Some(crate::model::layout::Direction::Vertical) => CursorIcon::EwResize,
                Some(crate::model::layout::Direction::Horizontal) => CursorIcon::NsResize,
                None => {
                    if self.pointer.hovered_url.is_some()
                        || matches!(
                            self.tabs.strip_hover,
                            super::strip::StripHit::Tab(_)
                                | super::strip::StripHit::Close(_)
                                | super::strip::StripHit::NewTab(_)
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
    pub(crate) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let scroll_lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                (y * SCROLL_LINES_PER_WHEEL_NOTCH).round() as isize
            }
            MouseScrollDelta::PixelDelta(pos) => (-pos.y / APPROX_CELL_HEIGHT as f64) as isize,
        };

        let (x, y) = self.pointer.cursor_pos;
        let focused = self
            .pane_at_pixel(x, y)
            .map(|(id, _)| id)
            .unwrap_or_else(|| self.layout().focused());
        if scroll_lines != 0 && self.offer_scroll_to_page(focused, scroll_lines) {
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
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
            if let Some(window) = &self.window {
                window.request_redraw();
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::mode::Mode;
    use crate::terminal::session::Terminal;
    use winit::keyboard::{KeyCode as WinitKeyCode, NamedKey};

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
            winter_render::MAX_SCROLLBACK,
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

    /// A pane with a real PTY running `cat`, mirroring how a full-screen app
    /// holds the keyboard: a foreground process (cat) plus the alt screen.
    fn app_pane_and_id() -> (App, crate::model::layout::TabId) {
        let mut app = App::new();
        // Isolate from the developer's own ~/.config keybindings.
        app.window_keymap = crate::model::input::WindowKeymap::default();
        let id = app.layout().shown_tabs()[0];
        let terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("cat"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs.set_mode(&id, Mode::Insert);
        (app, id)
    }

    fn esc() -> (Key, PhysicalKey) {
        (
            Key::Named(NamedKey::Escape),
            PhysicalKey::Code(WinitKeyCode::Escape),
        )
    }

    /// Whether the pane's terminal has echoed an ESC byte the program on it
    /// was sent, waiting up to `wait` for it to arrive.
    fn echoed_escape(app: &mut App, id: crate::model::layout::TabId, wait: Duration) -> bool {
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            let terminal = app.open_tabs.terminal_mut(&id).expect("pane");
            terminal.drain_output();
            if terminal.grid().to_text().contains("^[") {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn test_bare_escape_in_insert_mode_reaches_program_in_full_screen_app() {
        let (mut app, id) = app_pane_and_id();
        app.config.status_bar.enabled = false;
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(
            app.open_tabs.mode(&id).unwrap(),
            Mode::Insert,
            "bare Escape belongs to the program, so Winter stays in Insert"
        );
        assert!(app.notice.is_none());
        assert!(
            echoed_escape(&mut app, id, Duration::from_secs(3)),
            "the program received the Escape"
        );
    }

    #[test]
    fn test_alt_escape_switches_to_normal_mode() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.modifiers = winit::keyboard::ModifiersState::ALT.into();

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
    }

    #[test]
    fn test_escape_in_normal_mode_reaches_the_program_when_winter_has_nothing_to_cancel() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.open_tabs.set_mode(&id, Mode::Normal);

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
        assert!(
            echoed_escape(&mut app, id, Duration::from_secs(3)),
            "the program received the Escape"
        );
    }

    #[test]
    fn test_escape_in_normal_mode_cancels_a_half_typed_command_before_the_program_sees_it() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.open_tabs.set_mode(&id, Mode::Normal);
        app.pending = crate::model::input::PendingPrefix::Count(3);

        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));

        assert_eq!(app.pending, crate::model::input::PendingPrefix::None);
        assert!(
            !echoed_escape(&mut app, id, Duration::from_millis(300)),
            "Winter spent this Escape"
        );
    }

    #[test]
    fn test_mode_switch_is_silent_while_the_status_bar_is_shown() {
        let (mut app, id) = app_pane_and_id();
        app.config.status_bar.enabled = true;
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        // The entry chord switches even inside a full-screen app, and raises
        // no notice.
        app.modifiers = (winit::keyboard::ModifiersState::CONTROL
            | winit::keyboard::ModifiersState::SHIFT)
            .into();
        let (logical, physical) = (
            Key::Named(NamedKey::Space),
            PhysicalKey::Code(WinitKeyCode::Space),
        );
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));
        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
        assert!(app.notice.is_none());
    }

    #[test]
    fn test_a_modifier_going_down_types_nothing_into_the_text_being_edited() {
        // Regression: Ctrl alone decodes to the NUL character, which every
        // text field that took a character wrote in as a stray glyph.
        let (mut app, _id) = app_pane_and_id();
        app.tabs.rename_input = Some("ab".to_string());
        let pressed = app.handle_key(
            ElementState::Pressed,
            Key::Named(NamedKey::Control),
            PhysicalKey::Code(WinitKeyCode::ControlLeft),
        );
        assert!(!pressed);
        assert_eq!(app.tabs.rename_input.as_deref(), Some("ab"));
    }

    #[test]
    fn test_alt_escape_switches_to_normal_at_a_bare_prompt() {
        // A real shell sitting at its prompt: primary screen, no foreground
        // child, Alt-Escape switches modes.
        let mut app = App::new();
        app.window_keymap = crate::model::input::WindowKeymap::default();
        let id = app.layout().shown_tabs()[0];
        let mut terminal = Terminal::with_command(
            40,
            10,
            portable_pty::CommandBuilder::new("bash"),
            winter_render::MAX_SCROLLBACK,
        )
        .expect("test pane spawn");
        for _ in 0..150 {
            terminal.drain_output();
            if !terminal.has_foreground_process() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.open_tabs.insert_terminal(id, terminal);
        app.open_tabs.set_mode(&id, Mode::Insert);
        app.modifiers = winit::keyboard::ModifiersState::ALT.into();
        let (logical, physical) = esc();
        assert!(!app.handle_key(ElementState::Pressed, logical, physical));
        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
    }

    #[test]
    fn test_entry_chord_switches_to_normal_even_inside_a_full_screen_app() {
        let (mut app, id) = app_pane_and_id();
        app.open_tabs
            .terminal_mut(&id)
            .unwrap()
            .grid_mut()
            .enter_alt_screen();
        app.modifiers = (winit::keyboard::ModifiersState::CONTROL
            | winit::keyboard::ModifiersState::SHIFT)
            .into();
        assert!(!app.handle_key(
            ElementState::Pressed,
            Key::Named(NamedKey::Space),
            PhysicalKey::Code(WinitKeyCode::Space)
        ));
        assert_eq!(app.open_tabs.mode(&id).unwrap(), Mode::Normal);
    }
}

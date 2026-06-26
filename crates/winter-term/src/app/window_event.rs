//! Handlers for the winit window events the event loop dispatches.

use std::time::{Duration, Instant};

use winit::dpi::PhysicalPosition;
use winit::event::{ElementState, KeyEvent, MouseButton, MouseScrollDelta};
use winit::event_loop::ActiveEventLoop;
use winit::window::CursorIcon;

use crate::model::input::{self, KeyCode};
use crate::model::mode::Mode;

use super::App;
use super::Selection;
use super::{
    escape_clears_selection, escape_forwarded_to_pty, forwarded_to_pty,
    is_alt_screen_escape_double_tap, winit_key_to_code, ContextAction, APPROX_CELL_HEIGHT,
    CURSOR_BLINK_PERIOD, SCROLLBAR_CLICK_WIDTH, SCROLL_LINES_PER_WHEEL_NOTCH,
};

// ========================================================================
// App: window-event handlers
// ========================================================================

impl App {
    /// Route one key press or release: the chrome overlays first (command
    /// palette, settings page, which-key), then the focused pane's modal
    /// keymap, and finally the PTY.
    pub(crate) fn on_keyboard_input(&mut self, event: KeyEvent, event_loop: &ActiveEventLoop) {
        // winit synthesizes KeyboardInput::Pressed events for every
        // physically-held key on XI_FocusIn (handle_pressed_keys). Swallow
        // those here so e.g. the Tab from Alt+Tab never reaches the PTY.
        if event.state == ElementState::Pressed && self.suppress_synthesized_keys {
            return;
        }

        // Windows-only: drop a key event that raced ahead of this
        // window's own focus-gain notification (see
        // `is_pre_focus_key_leak`'s doc for why this can happen).
        #[cfg(target_os = "windows")]
        if is_pre_focus_key_leak(
            event.state,
            self.window.as_ref().is_some_and(|w| w.has_focus()),
        ) {
            return;
        }

        let focused = self.tab().focused();
        let mods_state = self.modifiers.state();
        let code = winit_key_to_code(&event.logical_key, &event.physical_key);
        let key = input::Key {
            alt: mods_state.alt_key(),
            code,
            ctrl: mods_state.control_key(),
            shift: mods_state.shift_key(),
        };
        if event.state == ElementState::Released {
            let kitty_flags = self
                .panes
                .get(&focused)
                .map(|p| p.kitty_flags())
                .unwrap_or(0);
            let bytes = input::encode_release(&key, kitty_flags);
            if !bytes.is_empty() {
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&bytes);
                }
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
            return;
        }

        // Reset blink to the "visible" phase on every key press so the
        // cursor is immediately shown as feedback for the action.
        if self.config.cursor.blink {
            self.blink_phase = true;
            self.blink_next_flip = Instant::now() + CURSOR_BLINK_PERIOD;
        }

        let mode = self.modes.get(&focused).copied().unwrap_or_default();

        // Taken (not just read) unconditionally so every key other than
        // the matching second Escape clears it - see the field's doc.
        // The bare-Escape branch below is the only place that puts a
        // value back.
        let prev_alt_screen_escape = self.last_alt_screen_escape.take();

        // While a tab rename is in progress, intercept all keyboard
        // input: Enter confirms, Escape cancels, other keys edit the name.
        if self.tabs.rename_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let name = self.tabs.rename_input.take().unwrap();
                    if name.is_empty() {
                        self.tabs.names.remove(&self.tabs.active);
                    } else {
                        self.tabs.names.insert(self.tabs.active, name);
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
            return;
        }

        // While a new-theme name is being entered, intercept all keyboard
        // input the same way: Enter confirms, Escape cancels, other keys
        // edit the name.
        if self.theme_name_input.is_some() {
            match key.code {
                KeyCode::Enter => {
                    let name = self.theme_name_input.take().unwrap();
                    self.create_named_theme(&name);
                }
                KeyCode::Escape => {
                    self.theme_name_input = None;
                }
                KeyCode::Backspace => {
                    if let Some(input) = &mut self.theme_name_input {
                        input.pop();
                    }
                }
                KeyCode::Char(c) if !key.ctrl && !key.alt => {
                    if let Some(input) = &mut self.theme_name_input {
                        input.push(c);
                    }
                }
                _ => {}
            }
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // While the settings page is up it owns all input: edits apply
        // live, Enter/Escape close it, and every key is swallowed so none
        // reaches the PTY.
        if self.settings_page.is_some() {
            let logical = event.logical_key.clone();
            let mut page = self.settings_page.take().unwrap();
            if self.handle_settings_input(&mut page, &logical) {
                self.settings_page = Some(page);
            } else {
                self.on_settings_closed();
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Esc dismisses an open menu before anything else acts on it.
        if self.menus.open.is_some() && key.code == KeyCode::Escape {
            self.close_menu();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Esc clears a mouse-drag selection before anything else acts on it.
        if escape_clears_selection(&key, mode, self.selection.span.is_some()) {
            self.selection.span = None;
            self.dirty = true;
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Global app shortcuts (settings, font size, tab/pane management,
        // palette toggles): configurable via the `window` block in
        // keybindings.kdl, and checked here so they intercept even while
        // an overlay (palette, tab rename, settings) would otherwise own
        // the key. Window-layout chords (split/close/focus/scroll/zoom)
        // are resolved later, once no overlay claims the key.
        if let Some(action) = self.window_keymap.global_action(&key) {
            self.handle_action(action, focused);
            if self.exit_requested {
                self.exit_requested = false;
                self.quit(event_loop);
                return;
            }
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        if self.palette.is_some() {
            let key = event.logical_key.clone();
            let mut palette = self.palette.take().unwrap();
            self.handle_palette_input(&mut palette, &key, &event.physical_key, focused);
            if palette.active {
                self.palette = Some(palette);
            }
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }

        // Escape in Insert mode: forwarded to the PTY if a foreground process
        // is running (a full-screen app, via `is_at_prompt`, or on Linux any
        // other foreground process group leader) or the pane is mid the
        // shell's own tab-completion; otherwise, at a bare shell prompt with
        // no completion in progress, it switches straight to Normal mode.
        // A second bare Escape on the same pane, arriving within
        // `ALT_SCREEN_ESCAPE_DOUBLE_TAP` of one that was forwarded, switches
        // to Normal mode instead of forwarding again - see
        // `last_alt_screen_escape`.
        let bare_esc = mode == Mode::Insert
            && key.code == KeyCode::Escape
            && !key.alt
            && !key.ctrl
            && !key.shift;
        if bare_esc {
            let has_foreground_process = self
                .panes
                .get(&focused)
                .is_some_and(|p| p.has_foreground_process());
            // Cleared unconditionally: a completion this Escape didn't
            // forward to (has_foreground_process was already true) is
            // stale by the time another bare Escape arrives, and one it
            // did forward to is now the shell's problem to resolve, not
            // this app's - either way the next Escape starts fresh.
            let pending_tab_completion = self.pending_tab_completion.remove(&focused);
            let now = Instant::now();
            let double_tap = is_alt_screen_escape_double_tap(prev_alt_screen_escape, focused, now);
            if !double_tap
                && escape_forwarded_to_pty(has_foreground_process, pending_tab_completion)
            {
                self.last_alt_screen_escape = Some((focused, now));
                if let Some(pane) = self.panes.get_mut(&focused) {
                    pane.write(&[0x1b]);
                }
            } else {
                let switch = input::Action::SwitchMode(
                    Mode::Insert.apply(crate::model::mode::ModeEvent::EnterNormal),
                );
                self.handle_action(switch, focused);
            }
            self.update_window_title();
            if let Some(window) = &self.window {
                window.request_redraw();
            }
            return;
        }
        let at_prompt = self.panes.get(&focused).is_some_and(|p| p.is_at_prompt());
        let kitty_flags = self
            .panes
            .get(&focused)
            .map(|p| p.kitty_flags())
            .unwrap_or(0);
        let modify_other_keys = self
            .panes
            .get(&focused)
            .map(|p| p.modify_other_keys())
            .unwrap_or(None);
        let is_alt_screen = self
            .panes
            .get(&focused)
            .map(|p| p.grid().is_alt_screen())
            .unwrap_or(false);
        let prev_pending = self.pending;
        let action = input::resolve_with(
            mode,
            &key,
            &mut self.pending,
            &self.window_keymap,
            kitty_flags,
            modify_other_keys,
            is_alt_screen,
        );
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
        if mode == Mode::Insert && forwarded_to_pty(&action) {
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
            self.quit(event_loop);
            return;
        }
        self.update_window_title();
        if let Some(window) = &self.window {
            window.request_redraw();
        }
    }
    /// Route a mouse button press or release. The tabbar and any open menu
    /// take precedence over the panes, so a click on chrome never reaches a
    /// pane underneath it.
    pub(crate) fn on_mouse_input(
        &mut self,
        state: ElementState,
        button: MouseButton,
        event_loop: &ActiveEventLoop,
    ) {
        let focused = self.tab().focused();

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

        // Ctrl+click on a hyperlinked cell opens the URL in the browser.
        // Double-check the scheme even though `osc_dispatch` already
        // filtered it, as defense in depth.
        if state == ElementState::Pressed
            && button == MouseButton::Left
            && self.modifiers.state().control_key()
        {
            if let Some(url) = &self.pointer.hovered_url {
                let scheme = url.split(':').next().unwrap_or("").to_ascii_lowercase();
                if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                    let _ = open::that(url);
                    return;
                }
            }
        }

        // Plain clicks always drive Winter's own selection, even over a
        // full-screen app (vim, htop) that has turned on mouse reporting.
        // Hold Shift to forward the click to that app's mouse handling instead.
        let mouse_active = self.panes.get(&focused).is_some_and(|p| p.mouse_tracking())
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
                        match action {
                            ContextAction::Copy => self.copy_selection(),
                            ContextAction::Paste => self.paste_from_clipboard(),
                            ContextAction::OpenLink(url) => {
                                let scheme =
                                    url.split(':').next().unwrap_or("").to_ascii_lowercase();
                                if matches!(scheme.as_str(), "http" | "https" | "mailto") {
                                    let _ = open::that(&url);
                                }
                            }
                        }
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
                    self.tab().divider_at(x, y, viewport).is_some()
                };
                if on_divider {
                    self.pointer.divider_drag = Some((x, y));
                } else {
                    // Scrollbar click: right-edge strip of a pane navigates scrollback.
                    let vp = self.viewport_rect();
                    let layout_vp =
                        crate::model::layout::Rect::new(vp.x, vp.y, vp.width, vp.height);
                    'scroll: for (id, rect) in self.tab().rects(layout_vp) {
                        let pr = Self::layout_rect_to_pane(rect);
                        let sb_x = pr.x + pr.width - SCROLLBAR_CLICK_WIDTH;
                        if x >= sb_x && x <= pr.x + pr.width && y >= pr.y && y < pr.y + pr.height {
                            if let Some(pane) = self.panes.get_mut(&id) {
                                let sbl = pane.grid().scrollback_len();
                                if sbl > 0 {
                                    let rows = pane.grid().rows();
                                    let total = (rows + sbl) as f32;
                                    let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                                    let top_virtual = (frac * total) as usize;
                                    let new_offset = sbl.saturating_sub(top_virtual);
                                    pane.grid_mut().set_scroll_offset(new_offset);
                                    self.pointer.scrollbar_drag = Some(id);
                                    if let Some(window) = &self.window {
                                        window.request_redraw();
                                    }
                                    break 'scroll;
                                }
                            }
                        }
                    }

                    if let Some((pane_id, pane_rect)) = self.pane_at_pixel(x, y) {
                        self.tab_mut().focus(pane_id);
                        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                        // A click parks the traversal cursor under the
                        // pointer, so selecting with the mouse moves the
                        // cursor instead of leaving it wherever the last
                        // keyboard motion stopped.
                        self.track_nav_cursor_to_mouse(pane_id, row, col);
                        let now = Instant::now();
                        if let Some((prev_time, prev_x, prev_y)) = self.pointer.last_click {
                            let dist = ((x - prev_x).powi(2) + (y - prev_y).powi(2)).sqrt();
                            if now.duration_since(prev_time) < Duration::from_millis(400)
                                && dist < 5.0
                            {
                                self.select_word_at(pane_id, row, col);
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
                self.pointer.scrollbar_drag = None;
                self.finalize_tab_drag();
                self.copy_selection();
                self.copy_selection_to_primary();
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
                    .tab_mut()
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
                let vp = self.viewport_rect();
                let layout_vp = crate::model::layout::Rect::new(vp.x, vp.y, vp.width, vp.height);
                if let Some((_, rect)) = self
                    .tab()
                    .rects(layout_vp)
                    .into_iter()
                    .find(|(id, _)| *id == sb_pane_id)
                {
                    let pr = Self::layout_rect_to_pane(rect);
                    if let Some(pane) = self.panes.get_mut(&sb_pane_id) {
                        let sbl = pane.grid().scrollback_len();
                        if sbl > 0 {
                            let rows = pane.grid().rows();
                            let total = (rows + sbl) as f32;
                            let frac = ((y - pr.y) / pr.height).clamp(0.0, 1.0);
                            let top_virtual = (frac * total) as usize;
                            pane.grid_mut()
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
                if let winter_render::TabbarHit::Tab(_) = hit {
                    self.tabs.hover_pos = Some(self.pointer.cursor_pos);
                } else {
                    self.tabs.hover_pos = None;
                }
                self.dirty = true;
                self.update_window_title();
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        let focused = self.tab().focused();
        if self.pointer.mouse_down
            && self
                .panes
                .get(&focused)
                .is_some_and(|p| p.mouse_drag_tracking())
            && self.modifiers.state().shift_key()
        {
            self.forward_mouse_motion(focused);
            return;
        }

        if self.pointer.mouse_down {
            if let Some((pane_id, pane_rect)) = self.pane_at_pixel(x, y) {
                let (row, col) = self.pixel_to_cell(x, y, pane_rect);
                // Selection rows are absolute (see `Grid::to_absolute_row`)
                // so they keep naming the same line if auto-scroll (or a
                // wheel scroll) moves the view mid-drag.
                let abs_row = self
                    .panes
                    .get(&pane_id)
                    .map(|p| p.grid().to_absolute_row(row))
                    .unwrap_or(row);
                if let Some(sel) = &mut self.selection.span {
                    sel.end_row = abs_row;
                    sel.end_col = col;
                    sel.pane = pane_id;
                } else {
                    self.selection.span = Some(Selection {
                        block: false,
                        start_row: abs_row,
                        start_col: col,
                        end_row: abs_row,
                        end_col: col,
                        pane: pane_id,
                    });
                }
                // The traversal cursor rides along with the drag's
                // live end, so the block cursor keeps following the
                // mouse while the selection extends.
                self.track_nav_cursor_to_mouse(pane_id, row, col);
                self.dirty = true;
                if let Some(window) = &self.window {
                    window.request_redraw();
                }
            }
        }

        // Update hovered hyperlink and cursor icon (divider resize or pointer).
        let new_url = self.hovered_link_at(x, y);
        self.pointer.hovered_url = new_url;
        let vp = self.content_viewport();
        let icon = if let Some(direction) = self.edge_resize_direction(x, y) {
            CursorIcon::from(direction)
        } else {
            match self.tab().divider_at(x, y, vp) {
                Some(crate::model::layout::Direction::Vertical) => CursorIcon::EwResize,
                Some(crate::model::layout::Direction::Horizontal) => CursorIcon::NsResize,
                None => {
                    if self.pointer.hovered_url.is_some() {
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
    /// Scroll the focused pane, or forward the wheel to the PTY when a
    /// full-screen application has asked for mouse reporting.
    pub(crate) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta) {
        let scroll_lines = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                (y * SCROLL_LINES_PER_WHEEL_NOTCH).round() as isize
            }
            MouseScrollDelta::PixelDelta(pos) => (-pos.y / APPROX_CELL_HEIGHT as f64) as isize,
        };

        let focused = self.tab().focused();
        if self.panes.get(&focused).is_some_and(|p| p.mouse_tracking()) {
            self.forward_mouse_scroll(scroll_lines, focused);
            return;
        }

        if scroll_lines != 0 {
            if let Some(pane) = self.panes.get_mut(&focused) {
                // Alt-screen apps (vim, less, etc.) own their viewport: send
                // arrow keys so they respond to the scroll gesture instead of
                // us scrolling their non-existent scrollback.
                if pane.grid().is_alt_screen() {
                    let arrow = if scroll_lines > 0 {
                        b"\x1b[A" as &[u8]
                    } else {
                        b"\x1b[B"
                    };
                    let count = scroll_lines.unsigned_abs();
                    for _ in 0..count {
                        pane.write(arrow);
                    }
                } else {
                    let grid = pane.grid_mut();
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

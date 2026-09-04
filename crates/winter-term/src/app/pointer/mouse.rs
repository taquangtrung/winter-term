//! SGR / legacy mouse event encoding and forwarding to the PTY.

use winit::event::{ElementState, MouseButton};

use crate::model::layout::PaneId;

use super::App;

// ========================================================================
// Free functions
// ========================================================================

/// SGR (mode 1006) mouse report bytes for `btn_code` at 1-based `(col, row)`.
/// Unlike the legacy protocol, SGR never adds an offset to the button code on
/// release: the trailing `M`/`m` already disambiguates press from release, so
/// the same code identifies the button either way.
fn sgr_mouse_bytes(btn_code: u8, col: usize, row: usize, pressed: bool) -> Vec<u8> {
    let final_char = if pressed { 'M' } else { 'm' };
    format!("\x1b[<{};{};{}{}", btn_code, col + 1, row + 1, final_char).into_bytes()
}

// ========================================================================
// App: PTY mouse forwarding
// ========================================================================

impl App {
    pub(crate) fn forward_mouse_event(
        &mut self,
        state: ElementState,
        button: MouseButton,
        focused: PaneId,
    ) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let btn_code = match button {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Forward => 4,
            MouseButton::Back => 5,
            _ => return,
        };

        let pressed = state == ElementState::Pressed;

        let bytes = if sgr {
            sgr_mouse_bytes(btn_code, col, row, pressed)
        } else {
            let cb = 32 + if pressed { btn_code } else { btn_code + 3 };
            let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
            let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
            format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
        };

        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
    }

    pub(crate) fn forward_mouse_motion(&mut self, focused: PaneId) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let btn_code = 0;
        let cb_code = 32 + btn_code;

        let bytes = if sgr {
            format!("\x1b[<{};{};{}M", cb_code, col + 1, row + 1).into_bytes()
        } else {
            let cb = (32 + cb_code) as u8;
            let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
            let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
            format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
        };

        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
        }
    }

    pub(crate) fn forward_mouse_scroll(&mut self, scroll_lines: isize, focused: PaneId) {
        let (x, y) = self.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        let count = scroll_lines.abs().min(10) as u8;
        let sign: u8 = if scroll_lines > 0 { 0 } else { 1 };

        for _ in 0..count {
            let cb = 64 + sign;
            let bytes = if sgr {
                format!("\x1b[<{};{};{}M", cb, col + 1, row + 1).into_bytes()
            } else {
                let b = 32 + cb;
                let cv = 32u8.saturating_add((col.min(222) + 1) as u8);
                let ch = 32u8.saturating_add((row.min(222) + 1) as u8);
                format!("\x1b[M{}{}{}", b as char, cv as char, ch as char).into_bytes()
            };
            if let Some(pane) = self.panes.get_mut(&focused) {
                pane.write(&bytes);
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

    #[test]
    fn test_sgr_mouse_bytes_keeps_the_button_code_unchanged_on_release() {
        // Regression: SGR release used to add +3 to the button code (a
        // legacy X10 convention where the single shared release code is 3),
        // which SGR doesn't need since the trailing M/m already disambiguates
        // press from release. That made e.g. Middle-release (1+3=4) collide
        // with Forward-press (4), so releasing the middle button read as
        // pressing Forward.
        assert_eq!(sgr_mouse_bytes(1, 5, 10, true), b"\x1b[<1;6;11M");
        assert_eq!(sgr_mouse_bytes(1, 5, 10, false), b"\x1b[<1;6;11m");
    }
}

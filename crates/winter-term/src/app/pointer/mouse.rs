//! SGR / legacy mouse event encoding and forwarding to the PTY.

use winit::event::{ElementState, MouseButton};

use crate::model::layout::PaneId;

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// Highest cell index the legacy (X10) protocol can address: it encodes a
/// coordinate as a single byte biased by [`MOUSE_CB_OFFSET`], so anything past
/// this clamps rather than wrapping into a control character.
const LEGACY_MOUSE_MAX_CELL: usize = 222;
/// Bias the legacy protocol adds to every button code and coordinate byte, to
/// keep them clear of the C0 controls.
const MOUSE_CB_OFFSET: u8 = 32;
/// Button code reported for a wheel notch away from the user.
const WHEEL_UP: u8 = 64;
/// Button code reported for a wheel notch toward the user.
const WHEEL_DOWN: u8 = 65;

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

/// One wheel report at 1-based `(col, row)`: button 64 for up, 65 for down.
///
/// One report per wheel event, never one per line the same notch would scroll
/// Winter's own scrollback. The app on the far side decides how far a notch
/// moves its view, exactly as it does under xterm; scaling by
/// `SCROLL_LINES_PER_WHEEL_NOTCH` here instead made a single notch jump that
/// many times too far in any full-screen app that tracks the mouse.
fn wheel_bytes(sgr: bool, scroll_up: bool, col: usize, row: usize) -> Vec<u8> {
    let btn_code = if scroll_up { WHEEL_UP } else { WHEEL_DOWN };
    if sgr {
        return format!("\x1b[<{};{};{}M", btn_code, col + 1, row + 1).into_bytes();
    }
    let cb = MOUSE_CB_OFFSET.saturating_add(btn_code);
    let cv = MOUSE_CB_OFFSET.saturating_add((col.min(LEGACY_MOUSE_MAX_CELL) + 1) as u8);
    let ch = MOUSE_CB_OFFSET.saturating_add((row.min(LEGACY_MOUSE_MAX_CELL) + 1) as u8);
    format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
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
        let (x, y) = self.pointer.cursor_pos;
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
        let (x, y) = self.pointer.cursor_pos;
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
        if scroll_lines == 0 {
            return;
        }
        let (x, y) = self.pointer.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self.panes.get(&focused).is_some_and(|p| p.mouse_sgr());

        // Only the direction carries: `scroll_lines` is how far *Winter* would
        // scroll its own view for this notch, which is not the app's business.
        let bytes = wheel_bytes(sgr, scroll_lines > 0, col, row);
        if let Some(pane) = self.panes.get_mut(&focused) {
            pane.write(&bytes);
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

    #[test]
    fn test_wheel_bytes_reports_one_notch_per_event() {
        // Regression: a notch used to be forwarded once per line Winter would
        // have scrolled its own view (SCROLL_LINES_PER_WHEEL_NOTCH, three),
        // so a full-screen app that tracks the mouse jumped three times too
        // far per notch and could not be scrolled to a target at all.
        assert_eq!(wheel_bytes(true, true, 5, 10), b"\x1b[<64;6;11M");
        assert_eq!(wheel_bytes(true, false, 5, 10), b"\x1b[<65;6;11M");
    }

    #[test]
    fn test_wheel_bytes_biases_the_legacy_encoding_clear_of_c0() {
        // Legacy (X10) reports are a button byte and two coordinate bytes,
        // each biased by 32: wheel-up is 64+32, and a cell past the encoding's
        // reach clamps instead of wrapping into a control character.
        assert_eq!(wheel_bytes(false, true, 5, 10), b"\x1b[M\x60\x26\x2b");
        assert_eq!(
            wheel_bytes(false, true, 5_000, 10),
            wheel_bytes(false, true, LEGACY_MOUSE_MAX_CELL, 10),
            "a column past the encoding's reach clamps to the last one it can name"
        );
    }
}

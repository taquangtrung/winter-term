//! SGR / legacy mouse event encoding and forwarding to the PTY.

use winit::event::{ElementState, MouseButton};

use crate::model::layout::TabId;

use super::App;
use crate::window::InputState;

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
/// Button codes of the buttons a press or release reports.
const BUTTON_LEFT: u8 = 0;
const BUTTON_MIDDLE: u8 = 1;
const BUTTON_RIGHT: u8 = 2;
const BUTTON_FORWARD: u8 = 4;
const BUTTON_BACK: u8 = 5;
/// The legacy encoding's one release code, shared by every button.
const LEGACY_RELEASE: u8 = 3;
/// Added to a button code to mark a motion report.
const MOTION_FLAG: u8 = 32;
/// Modifier bits a report's button code carries. Shift is never set: in
/// Winter, Shift is the chord that hands the mouse to the program at all, so
/// every report would carry it and it would say nothing.
const MODIFIER_SHIFT: u8 = 4;
const MODIFIER_ALT: u8 = 8;
const MODIFIER_CTRL: u8 = 16;
const MODIFIER_BITS: u8 = MODIFIER_SHIFT | MODIFIER_ALT | MODIFIER_CTRL;
/// The "button" a motion report with no button held names.
const NO_BUTTON: u8 = 3;

// ========================================================================
// Free functions
// ========================================================================

/// One mouse report for button code `code` at the 0-based cell
/// `(col, row)`, in the SGR encoding (mode 1006) or the legacy X10 one.
///
/// SGR keeps the button's own code on release, since its trailing `M`/`m`
/// already says press or release. The legacy encoding has one shared
/// release code instead, [`LEGACY_RELEASE`]: adding 3 to the button (as this
/// once did) turned a right-button release into a different button. Its
/// coordinates are single bytes biased by [`MOUSE_CB_OFFSET`], so a cell past
/// their reach clamps rather than wrapping into a control character.
fn mouse_report(sgr: bool, code: u8, col: usize, row: usize, pressed: bool) -> Vec<u8> {
    if sgr {
        let final_char = if pressed { 'M' } else { 'm' };
        return format!("\x1b[<{};{};{}{}", code, col + 1, row + 1, final_char).into_bytes();
    }
    // The release code replaces the button but keeps the modifier bits.
    let code = if pressed {
        code
    } else {
        LEGACY_RELEASE | (code & MODIFIER_BITS)
    };
    let cb = MOUSE_CB_OFFSET.saturating_add(code);
    let cv = MOUSE_CB_OFFSET.saturating_add((col.min(LEGACY_MOUSE_MAX_CELL) + 1) as u8);
    let ch = MOUSE_CB_OFFSET.saturating_add((row.min(LEGACY_MOUSE_MAX_CELL) + 1) as u8);
    format!("\x1b[M{}{}{}", cb as char, cv as char, ch as char).into_bytes()
}

/// One wheel report at the 0-based cell `(col, row)`: button 64 for up, 65
/// for down.
///
/// One report per wheel event, never one per line the same notch would scroll
/// Winter's own scrollback. The app on the far side decides how far a notch
/// moves its view, exactly as it does under xterm; scaling by
/// `SCROLL_LINES_PER_WHEEL_NOTCH` here instead made a single notch jump that
/// many times too far in any full-screen app that tracks the mouse.
fn wheel_bytes(sgr: bool, scroll_up: bool, col: usize, row: usize, modifiers: u8) -> Vec<u8> {
    let code = if scroll_up { WHEEL_UP } else { WHEEL_DOWN };
    mouse_report(sgr, code | modifiers, col, row, true)
}

/// The button code a motion report carries: the motion flag, the button
/// held (or "no button", for the bare motion any-event tracking reports),
/// and the modifier bits.
fn motion_code(held: Option<u8>, modifiers: u8) -> u8 {
    MOTION_FLAG | held.unwrap_or(NO_BUTTON) | modifiers
}

/// The modifier bits for a report made with Alt and Ctrl as given.
fn modifier_bits(alt: bool, ctrl: bool) -> u8 {
    let alt = if alt { MODIFIER_ALT } else { 0 };
    let ctrl = if ctrl { MODIFIER_CTRL } else { 0 };
    alt | ctrl
}

// ========================================================================
// App: PTY mouse forwarding
// ========================================================================

impl App {

    pub(in crate::window) fn forward_mouse_event(
        &mut self,
        state: ElementState,
        button: MouseButton,
        focused: TabId,
    ) {
        let (x, y) = self.pointer.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_sgr());

        let code = match button {
            MouseButton::Left => BUTTON_LEFT,
            MouseButton::Middle => BUTTON_MIDDLE,
            MouseButton::Right => BUTTON_RIGHT,
            MouseButton::Forward => BUTTON_FORWARD,
            MouseButton::Back => BUTTON_BACK,
            _ => return,
        };
        let pressed = state == ElementState::Pressed;
        let bytes = mouse_report(sgr, code | self.input.mouse_modifier_bits(), col, row, pressed);
        // Motion until the release reports this button as the one held; the
        // wheel's pseudo-buttons are never held.
        if matches!(code, BUTTON_LEFT | BUTTON_MIDDLE | BUTTON_RIGHT) {
            self.pointer.forwarded_button = pressed.then_some(code);
        }
        // A press or release starts the motion de-duplication over.
        self.pointer.last_motion_cell = None;
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
    }

    /// Report pointer motion to the mouse-tracking program in `focused`,
    /// with the button it was last handed a press of still held (drag
    /// tracking) or with none (any-event tracking). Sent only when the
    /// pointer has entered a new cell.
    pub(in crate::window) fn forward_mouse_motion(&mut self, focused: TabId) {
        let (x, y) = self.pointer.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        if self.pointer.last_motion_cell == Some((row, col)) {
            return;
        }
        self.pointer.last_motion_cell = Some((row, col));
        let sgr = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_sgr());
        let code = motion_code(self.pointer.forwarded_button, self.input.mouse_modifier_bits());
        let bytes = mouse_report(sgr, code, col, row, true);
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
    }

    /// Scroll the view of a full-screen app in `focused` by `lines` (positive
    /// is up), for the Normal-mode motions that would otherwise scroll
    /// history the alternate screen does not have. The app owns its viewport,
    /// so it is asked to move it: one wheel notch per line when it tracks the
    /// mouse, else one arrow key per line, as the wheel itself does. Returns
    /// whether the pane is on the alternate screen, i.e. whether the app
    /// took the scroll.
    pub(in crate::window) fn scroll_app_view(&mut self, focused: TabId, lines: isize) -> bool {
        let Some(terminal) = self.open_tabs.terminal_mut(&focused) else {
            return false;
        };
        if !terminal.grid().is_alt_screen() {
            return false;
        }
        let up = lines > 0;
        let bytes = if terminal.mouse_tracking() {
            let (cols, rows) = (terminal.grid().cols(), terminal.grid().rows());
            wheel_bytes(terminal.mouse_sgr(), up, cols / 2, rows / 2, 0)
        } else if up {
            b"\x1b[A".to_vec()
        } else {
            b"\x1b[B".to_vec()
        };
        for _ in 0..lines.unsigned_abs() {
            terminal.write(&bytes);
        }
        true
    }

    pub(in crate::window) fn forward_mouse_scroll(&mut self, scroll_lines: isize, focused: TabId) {
        if scroll_lines == 0 {
            return;
        }
        let (x, y) = self.pointer.cursor_pos;
        let Some((_, pane_rect)) = self.pane_at_pixel(x, y) else {
            return;
        };
        let (row, col) = self.pixel_to_cell(x, y, pane_rect);
        let sgr = self
            .open_tabs
            .terminal(&focused)
            .is_some_and(|p| p.mouse_sgr());

        // Only the direction carries: `scroll_lines` is how far *Winter* would
        // scroll its own view for this notch, which is not the app's business.
        let bytes = wheel_bytes(sgr, scroll_lines > 0, col, row, self.input.mouse_modifier_bits());
        if let Some(terminal) = self.open_tabs.terminal_mut(&focused) {
            terminal.write(&bytes);
        }
    }
}

// ========================================================================
// InputState
// ========================================================================

impl InputState {
    /// The modifier bits for a report made now, from the keys held.
    fn mouse_modifier_bits(&self) -> u8 {
        let held = self.modifiers.state();
        modifier_bits(held.alt_key(), held.control_key())
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
        assert_eq!(mouse_report(true, 1, 5, 10, true), b"\x1b[<1;6;11M");
        assert_eq!(mouse_report(true, 1, 5, 10, false), b"\x1b[<1;6;11m");
    }

    #[test]
    fn test_legacy_release_reports_the_shared_release_code() {
        // Regression: release used to send `button + 3`, so a right release
        // (2 + 3 = 5) read as button 5 instead of "released".
        let release = 32 + LEGACY_RELEASE;
        for button in [BUTTON_LEFT, BUTTON_MIDDLE, BUTTON_RIGHT] {
            assert_eq!(
                mouse_report(false, button, 0, 0, false)[3],
                release,
                "{button}"
            );
        }
        assert_eq!(
            mouse_report(false, BUTTON_RIGHT, 0, 0, true)[3],
            32 + BUTTON_RIGHT
        );
    }

    #[test]
    fn test_motion_reports_name_the_held_button_or_none() {
        let left_drag = motion_code(Some(BUTTON_LEFT), 0);
        assert_eq!(mouse_report(true, left_drag, 1, 2, true), b"\x1b[<32;2;3M");
        let right_drag = motion_code(Some(BUTTON_RIGHT), 0);
        assert_eq!(mouse_report(true, right_drag, 1, 2, true), b"\x1b[<34;2;3M");
        let hover = motion_code(None, 0);
        assert_eq!(mouse_report(true, hover, 1, 2, true), b"\x1b[<35;2;3M");
    }

    #[test]
    fn test_modifiers_ride_on_every_report_kind_release_included() {
        assert_eq!(modifier_bits(true, true), MODIFIER_ALT | MODIFIER_CTRL);
        let click = BUTTON_LEFT | modifier_bits(false, true);
        assert_eq!(mouse_report(true, click, 0, 0, true), b"\x1b[<16;1;1M");
        assert_eq!(
            mouse_report(false, click, 0, 0, false)[3],
            32 + (LEGACY_RELEASE | MODIFIER_CTRL),
            "a legacy release keeps the modifier bits"
        );
        assert_eq!(
            wheel_bytes(true, true, 0, 0, MODIFIER_ALT),
            b"\x1b[<72;1;1M"
        );
        assert_eq!(motion_code(None, MODIFIER_ALT), 43);
    }

    #[test]
    fn test_wheel_bytes_reports_one_notch_per_event() {
        // Regression: a notch used to be forwarded once per line Winter would
        // have scrolled its own view (SCROLL_LINES_PER_WHEEL_NOTCH, three),
        // so a full-screen app that tracks the mouse jumped three times too
        // far per notch and could not be scrolled to a target at all.
        assert_eq!(wheel_bytes(true, true, 5, 10, 0), b"\x1b[<64;6;11M");
        assert_eq!(wheel_bytes(true, false, 5, 10, 0), b"\x1b[<65;6;11M");
    }

    #[test]
    fn test_wheel_bytes_biases_the_legacy_encoding_clear_of_c0() {
        // Legacy (X10) reports are a button byte and two coordinate bytes,
        // each biased by 32: wheel-up is 64+32, and a cell past the encoding's
        // reach clamps instead of wrapping into a control character.
        assert_eq!(wheel_bytes(false, true, 5, 10, 0), b"\x1b[M\x60\x26\x2b");
        assert_eq!(
            wheel_bytes(false, true, 5_000, 10, 0),
            wheel_bytes(false, true, LEGACY_MOUSE_MAX_CELL, 10, 0),
            "a column past the encoding's reach clamps to the last one it can name"
        );
    }
}

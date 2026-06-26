//! The alternate screen buffer and the DEC private modes.

use super::{Cell, Style};
use super::{Cursor, Grid};

// ========================================================================
// Grid: alternate screen and DEC modes
// ========================================================================

impl Grid {
    /// Switch to the alternate screen buffer, saving the current state.
    pub fn enter_alt_screen(&mut self) {
        if self.alt_buffer.is_some() {
            return;
        }
        self.alt_buffer = Some(Box::new(AltBuffer {
            cells: std::mem::take(&mut self.cells),
            cursor: self.cursor,
            saved_cursor: self.saved_cursor,
            style: self.style,
        }));
        self.cells = vec![Cell::default(); self.cols * self.rows];
        self.cursor = Cursor::default();
        self.saved_cursor = None;
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.active_link = 0;
        self.cursor_shape_set = false;
        self.cursor_visible = true;
        self.origin_mode = false;
        self.row_wrapped.fill(false);
        self.row_wrap_indent.fill(0);
    }
    /// Switch back to the primary screen buffer, restoring the saved state.
    pub fn leave_alt_screen(&mut self) {
        let Some(alt) = self.alt_buffer.take() else {
            return;
        };
        self.cells = alt.cells;
        self.cursor = alt.cursor;
        self.saved_cursor = alt.saved_cursor;
        // The primary buffer's cursor was captured when the alt screen was
        // entered, which may have been at a different size. Same hazard as
        // `restore_cursor`.
        self.clamp_cursor();
        self.style = alt.style;
        self.scroll_offset = 0;
        self.scroll_top = 0;
        self.scroll_bottom = self.rows.saturating_sub(1);
        self.active_link = 0;
        self.cursor_shape_set = false;
        self.cursor_visible = true;
        self.origin_mode = false;
        self.row_wrapped.fill(false);
        self.row_wrap_indent.fill(0);
    }
    /// Whether the alternate screen buffer is active, i.e. a fullscreen
    /// application (vim, less, htop) is running.
    pub fn is_alt_screen(&self) -> bool {
        self.alt_buffer.is_some()
    }
    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.bracketed_paste
    }
    /// Whether any mouse tracking mode is active (button or drag).
    pub fn mouse_tracking(&self) -> bool {
        self.mouse_button || self.mouse_drag
    }
    /// Whether drag tracking (CSI ?1002h) specifically is active.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.mouse_drag
    }
    /// Whether SGR extended mouse mode (CSI ?1006h) is active.
    pub fn mouse_sgr(&self) -> bool {
        self.mouse_sgr
    }
    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.focus_event
    }
    /// Handle DECSET/DECRST for a single mode number. Called from screen.rs
    /// which parses the CSI ? sequences.
    pub fn set_private_mode(&mut self, mode: u16, set: bool) {
        match mode {
            MODE_ALT_SCREEN => {
                if set {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            MODE_ALT_SCREEN_47 | MODE_ALT_SCREEN_1047 => {
                if set {
                    self.enter_alt_screen();
                } else {
                    self.leave_alt_screen();
                }
            }
            MODE_SAVE_CURSOR => {
                if set {
                    self.save_cursor();
                } else {
                    self.restore_cursor();
                }
            }
            MODE_BRACKETED_PASTE => self.bracketed_paste = set,
            MODE_CURSOR => self.cursor_visible = set,
            // DECOM: positioning becomes relative to the scroll region and the
            // mode change homes the cursor to that region's top-left.
            MODE_ORIGIN => {
                self.origin_mode = set;
                self.move_to(0, 0);
            }
            MODE_FOCUS_EVENT => self.focus_event = set,
            MODE_MOUSE_BUTTON => self.mouse_button = set,
            MODE_MOUSE_DRAG => self.mouse_drag = set,
            MODE_MOUSE_SGR => self.mouse_sgr = set,
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AltBuffer {
    pub(crate) cells: Vec<Cell>,
    pub(crate) cursor: Cursor,
    pub(crate) saved_cursor: Option<Cursor>,
    pub(crate) style: Style,
}
pub(crate) const MODE_ALT_SCREEN: u16 = 1049;
/// Legacy alternate-screen switch (no cursor save). xterm's older `?47`.
pub(crate) const MODE_ALT_SCREEN_47: u16 = 47;
/// Legacy alternate-screen switch that clears on leave (xterm's `?1047`).
pub(crate) const MODE_ALT_SCREEN_1047: u16 = 1047;
pub(crate) const MODE_BRACKETED_PASTE: u16 = 2004;
pub(crate) const MODE_CURSOR: u16 = 25;
pub(crate) const MODE_MOUSE_BUTTON: u16 = 1000;
pub(crate) const MODE_MOUSE_DRAG: u16 = 1002;
pub(crate) const MODE_MOUSE_SGR: u16 = 1006;
pub(crate) const MODE_FOCUS_EVENT: u16 = 1004;
pub(crate) const MODE_ORIGIN: u16 = 6;
/// Save/restore cursor as a DEC private mode (xterm's `?1048`).
pub(crate) const MODE_SAVE_CURSOR: u16 = 1048;

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::CursorShape;
    use super::*;

    #[test]
    fn test_origin_mode_positions_relative_to_scroll_region() {
        let mut grid = Grid::new(10, 10);
        // Region rows 3..=7 (1-based) -> 0-based top 2, bottom 6.
        grid.set_scroll_region(2, 6);
        grid.set_private_mode(MODE_ORIGIN, true);
        // Enabling origin mode homes the cursor to the region's top-left.
        assert_eq!(grid.cursor(), (2, 0));
        // CUP rows are relative to the region top.
        grid.move_to(3, 4);
        assert_eq!(grid.cursor(), (5, 4));
        // Rows past the region bottom clamp to it.
        grid.move_to(100, 0);
        assert_eq!(grid.cursor(), (6, 0));
        // Disabling returns to absolute, screen-relative positioning.
        grid.set_private_mode(MODE_ORIGIN, false);
        assert_eq!(grid.cursor(), (0, 0));
        grid.move_to(3, 4);
        assert_eq!(grid.cursor(), (3, 4));
    }
    #[test]
    fn test_legacy_alt_screen_mode_47() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.set_private_mode(MODE_ALT_SCREEN_47, true);
        assert!(grid.is_alt_screen());
        grid.set_private_mode(MODE_ALT_SCREEN_47, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }
    #[test]
    fn test_save_cursor_mode_1048() {
        let mut grid = Grid::new(5, 5);
        grid.move_to(2, 3);
        grid.set_private_mode(MODE_SAVE_CURSOR, true);
        grid.move_to(0, 0);
        grid.set_private_mode(MODE_SAVE_CURSOR, false);
        assert_eq!(grid.cursor(), (2, 3));
    }
    #[test]
    fn test_alt_screen_switches_and_restores() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.enter_alt_screen();
        assert!(grid.is_alt_screen());
        assert_eq!(grid.to_text(), "");
        for ch in "xyz".chars() {
            grid.print(ch);
        }
        grid.leave_alt_screen();
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }
    #[test]
    fn test_set_private_mode_alt_screen() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.set_private_mode(MODE_ALT_SCREEN, true);
        assert!(grid.is_alt_screen());
        grid.set_private_mode(MODE_ALT_SCREEN, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }
    #[test]
    fn test_bracketed_paste_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.bracketed_paste());
        grid.set_private_mode(MODE_BRACKETED_PASTE, true);
        assert!(grid.bracketed_paste());
        grid.set_private_mode(MODE_BRACKETED_PASTE, false);
        assert!(!grid.bracketed_paste());
    }
    #[test]
    fn test_mouse_modes() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());

        grid.set_private_mode(MODE_MOUSE_BUTTON, true);
        assert!(grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());

        grid.set_private_mode(MODE_MOUSE_DRAG, true);
        assert!(grid.mouse_tracking());
        assert!(grid.mouse_drag_tracking());

        grid.set_private_mode(MODE_MOUSE_SGR, true);
        assert!(grid.mouse_sgr());

        grid.set_private_mode(MODE_MOUSE_BUTTON, false);
        grid.set_private_mode(MODE_MOUSE_DRAG, false);
        grid.set_private_mode(MODE_MOUSE_SGR, false);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());
    }
    #[test]
    fn test_cursor_visibility_dectcem() {
        let mut grid = Grid::new(3, 2);
        assert!(grid.cursor_visible());
        grid.set_private_mode(MODE_CURSOR, false);
        assert!(!grid.cursor_visible());
        grid.set_private_mode(MODE_CURSOR, true);
        assert!(grid.cursor_visible());
    }
    #[test]
    fn test_alt_screen_resets_reported_shape_and_visibility() {
        let mut grid = Grid::new(3, 2);
        grid.set_cursor_shape(CursorShape::Bar);
        grid.set_private_mode(MODE_CURSOR, false);
        // A full-screen app's cursor state must not leak across the alt-screen
        // boundary: leaving restores an unset shape and a visible cursor.
        grid.set_private_mode(MODE_ALT_SCREEN, true);
        assert_eq!(grid.reported_cursor_shape(), None);
        assert!(grid.cursor_visible());
        grid.set_cursor_shape(CursorShape::Underline);
        grid.set_private_mode(MODE_CURSOR, false);
        grid.set_private_mode(MODE_ALT_SCREEN, false);
        assert_eq!(grid.reported_cursor_shape(), None);
        assert!(grid.cursor_visible());
    }
    #[test]
    fn test_focus_event_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.focus_event());
        grid.set_private_mode(MODE_FOCUS_EVENT, true);
        assert!(grid.focus_event());
        grid.set_private_mode(MODE_FOCUS_EVENT, false);
        assert!(!grid.focus_event());
    }
}

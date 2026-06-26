//! The alternate screen buffer, and read access to the modes programs set
//! (see [`super::modes`] for setting them).

use super::{Cell, RowMeta, Style};
use super::{Cursor, Grid, SavedCursor};

// ========================================================================
// Grid: alternate screen and mode accessors
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
            row_meta: std::mem::replace(&mut self.row_meta, vec![RowMeta::default(); self.rows]),
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
        self.modes.cursor_visible = true;
        self.modes.origin = false;
    }
    /// Switch back to the primary screen buffer, restoring the saved state.
    pub fn leave_alt_screen(&mut self) {
        let Some(alt) = self.alt_buffer.take() else {
            return;
        };
        self.cells = alt.cells;
        // The primary screen's wrap facts come back with its cells, so a
        // wrapped line still copies and reflows as one after a full-screen app.
        self.row_meta = alt.row_meta;
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
        self.modes.cursor_visible = true;
        self.modes.origin = false;
    }
    /// Whether the alternate screen buffer is active, i.e. a fullscreen
    /// application (vim, less, htop) is running.
    pub fn is_alt_screen(&self) -> bool {
        self.alt_buffer.is_some()
    }
    /// Whether bracketed paste mode (CSI ?2004h) is active.
    pub fn bracketed_paste(&self) -> bool {
        self.modes.bracketed_paste
    }
    /// Whether a synchronized update (CSI ?2026h) is in progress, which asks the
    /// terminal not to show what the program draws until it ends the update.
    pub fn synchronized_output(&self) -> bool {
        self.modes.sync_output
    }
    /// Whether any mouse tracking mode is active (button, drag, or any-event).
    pub fn mouse_tracking(&self) -> bool {
        self.modes.mouse_button || self.mouse_drag_tracking()
    }
    /// Whether motion with a button held is reported: drag tracking
    /// (CSI ?1002h), or any-event tracking (CSI ?1003h), which includes it.
    pub fn mouse_drag_tracking(&self) -> bool {
        self.modes.mouse_drag || self.modes.mouse_any_event
    }
    /// Whether every pointer motion is reported, buttons down or not
    /// (CSI ?1003h).
    pub fn mouse_any_event(&self) -> bool {
        self.modes.mouse_any_event
    }
    /// Whether SGR extended mouse mode (CSI ?1006h) is active.
    pub fn mouse_sgr(&self) -> bool {
        self.modes.mouse_sgr
    }
    /// Whether focus event mode (CSI ?1004h) is active.
    pub fn focus_event(&self) -> bool {
        self.modes.focus_event
    }
    /// Whether the cursor keys send their application (`SS3`) forms
    /// (DECCKM, CSI ?1h).
    pub fn app_cursor_keys(&self) -> bool {
        self.modes.app_cursor_keys
    }
    /// Whether the whole screen is drawn in reverse video (DECSCNM, CSI ?5h).
    pub fn reverse_video(&self) -> bool {
        self.modes.reverse_video
    }
}

#[derive(Clone, Debug)]
pub(crate) struct AltBuffer {
    pub(crate) cells: Vec<Cell>,
    pub(crate) cursor: Cursor,
    pub(crate) row_meta: Vec<RowMeta>,
    pub(crate) saved_cursor: Option<SavedCursor>,
    pub(crate) style: Style,
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{CursorShape, DecMode};
    use super::*;

    #[test]
    fn test_origin_mode_positions_relative_to_scroll_region() {
        let mut grid = Grid::new(10, 10);
        // Region rows 3..=7 (1-based) -> 0-based top 2, bottom 6.
        grid.set_scroll_region(2, 6);
        grid.set_dec_mode(DecMode::Origin, true);
        // Enabling origin mode homes the cursor to the region's top-left.
        assert_eq!(grid.cursor(), (2, 0));
        // CUP rows are relative to the region top.
        grid.move_to(3, 4);
        assert_eq!(grid.cursor(), (5, 4));
        // Rows past the region bottom clamp to it.
        grid.move_to(100, 0);
        assert_eq!(grid.cursor(), (6, 0));
        // Disabling returns to absolute, screen-relative positioning.
        grid.set_dec_mode(DecMode::Origin, false);
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
        grid.set_dec_mode(DecMode::AltScreenLegacy, true);
        assert!(grid.is_alt_screen());
        grid.set_dec_mode(DecMode::AltScreenLegacy, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }
    #[test]
    fn test_save_cursor_mode_1048() {
        let mut grid = Grid::new(5, 5);
        grid.move_to(2, 3);
        grid.set_dec_mode(DecMode::SaveCursor, true);
        grid.move_to(0, 0);
        grid.set_dec_mode(DecMode::SaveCursor, false);
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
    fn test_set_dec_mode_alt_screen() {
        let mut grid = Grid::new(3, 2);
        for ch in "abc".chars() {
            grid.print(ch);
        }
        grid.set_dec_mode(DecMode::AltScreen, true);
        assert!(grid.is_alt_screen());
        grid.set_dec_mode(DecMode::AltScreen, false);
        assert!(!grid.is_alt_screen());
        assert_eq!(grid.cell(0, 0).map(|c| c.ch), Some('a'));
    }
    #[test]
    fn test_synchronized_output_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.synchronized_output());
        grid.set_dec_mode(DecMode::SyncOutput, true);
        assert!(grid.synchronized_output());
        grid.set_dec_mode(DecMode::SyncOutput, false);
        assert!(!grid.synchronized_output());
    }
    #[test]
    fn test_bracketed_paste_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.bracketed_paste());
        grid.set_dec_mode(DecMode::BracketedPaste, true);
        assert!(grid.bracketed_paste());
        grid.set_dec_mode(DecMode::BracketedPaste, false);
        assert!(!grid.bracketed_paste());
    }
    #[test]
    fn test_mouse_modes() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());

        grid.set_dec_mode(DecMode::MouseButton, true);
        assert!(grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());

        grid.set_dec_mode(DecMode::MouseDrag, true);
        assert!(grid.mouse_tracking());
        assert!(grid.mouse_drag_tracking());

        grid.set_dec_mode(DecMode::MouseSgr, true);
        assert!(grid.mouse_sgr());

        grid.set_dec_mode(DecMode::MouseButton, false);
        grid.set_dec_mode(DecMode::MouseDrag, false);
        grid.set_dec_mode(DecMode::MouseSgr, false);
        assert!(!grid.mouse_tracking());
        assert!(!grid.mouse_drag_tracking());
        assert!(!grid.mouse_sgr());
    }
    #[test]
    fn test_cursor_visibility_dectcem() {
        let mut grid = Grid::new(3, 2);
        assert!(grid.cursor_visible());
        grid.set_dec_mode(DecMode::CursorVisible, false);
        assert!(!grid.cursor_visible());
        grid.set_dec_mode(DecMode::CursorVisible, true);
        assert!(grid.cursor_visible());
    }
    #[test]
    fn test_alt_screen_resets_reported_shape_and_visibility() {
        let mut grid = Grid::new(3, 2);
        grid.set_cursor_shape(CursorShape::Bar);
        grid.set_dec_mode(DecMode::CursorVisible, false);
        // A full-screen app's cursor state must not leak across the alt-screen
        // boundary: leaving restores an unset shape and a visible cursor.
        grid.set_dec_mode(DecMode::AltScreen, true);
        assert_eq!(grid.reported_cursor_shape(), None);
        assert!(grid.cursor_visible());
        grid.set_cursor_shape(CursorShape::Underline);
        grid.set_dec_mode(DecMode::CursorVisible, false);
        grid.set_dec_mode(DecMode::AltScreen, false);
        assert_eq!(grid.reported_cursor_shape(), None);
        assert!(grid.cursor_visible());
    }
    #[test]
    fn test_focus_event_mode() {
        let mut grid = Grid::new(3, 2);
        assert!(!grid.focus_event());
        grid.set_dec_mode(DecMode::FocusEvent, true);
        assert!(grid.focus_event());
        grid.set_dec_mode(DecMode::FocusEvent, false);
        assert!(!grid.focus_event());
    }
}

//! Terminal resets and the screen-alignment test: RIS (`ESC c`), DECSTR
//! (`CSI ! p`), and DECALN (`ESC # 8`).

use std::mem;

use super::{Cell, Grid, RowMeta, Style};

// ========================================================================
// Constants
// ========================================================================

/// The character DECALN fills the screen with.
const ALIGNMENT_FILL: char = 'E';

// ========================================================================
// Grid: resets
// ========================================================================

impl Grid {
    /// Full reset (RIS, `ESC c`): everything a program can set goes back to
    /// its power-on state and the screen clears, as `reset` and `tput reset`
    /// expect. What the user owns survives: the scrollback history (and the
    /// link table its cells point into) and the host's wrap settings.
    ///
    /// Built as a fresh grid with the survivors moved across, so state added
    /// to the grid later resets by default instead of being forgotten here.
    pub fn full_reset(&mut self) {
        let mut fresh = Grid::new(self.cols, self.rows).with_max_scrollback(self.max_scrollback);
        fresh.erased_spans = mem::take(&mut self.erased_spans);
        fresh.link_ids = mem::take(&mut self.link_ids);
        fresh.link_table = mem::take(&mut self.link_table);
        fresh.next_url_scan = self.next_url_scan;
        fresh.pending_remap = self.pending_remap.take();
        fresh.scroll_to_bottom_on_output = self.scroll_to_bottom_on_output;
        fresh.history_lines = self.history_lines;
        fresh.history_stale = self.history_stale;
        fresh.scrollback = mem::take(&mut self.scrollback);
        fresh.trimmed_rows = self.trimmed_rows;
        fresh.word_wrap = self.word_wrap;
        fresh.wrap_indent = self.wrap_indent;
        // Whatever was anchored to the live screen is gone with it.
        fresh.record_erased_rows((0, fresh.rows));
        *self = fresh;
    }

    /// Soft reset (DECSTR, `CSI ! p`): the modes DECSTR covers (see
    /// `ModeFlags::soft_reset`), the pen, the scroll region, the character
    /// sets, and the saved cursor go back to their defaults. The screen's
    /// contents and the cursor position are kept.
    pub fn soft_reset(&mut self) {
        self.modes.soft_reset();
        self.style = Style::default();
        self.charsets = Default::default();
        self.reset_scroll_region();
        self.saved_cursor = None;
        self.cursor.wrap_pending = false;
    }

    /// Screen alignment test (DECALN, `ESC # 8`): fill the screen with `E`,
    /// reset the margins, and home the cursor.
    pub fn fill_alignment_pattern(&mut self) {
        let fill = Cell {
            ch: ALIGNMENT_FILL,
            ..Cell::default()
        };
        self.cells.fill(fill);
        self.row_meta.fill(RowMeta::default());
        self.record_erased_rows((0, self.rows));
        self.reset_scroll_region();
        self.modes.origin = false;
        self.move_to(0, 0);
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::{Charset, CharsetSlot, DecMode};
    use super::*;

    fn print_str(grid: &mut Grid, text: &str) {
        for ch in text.chars() {
            grid.print(ch);
        }
    }

    #[test]
    fn test_full_reset_clears_the_screen_and_modes_but_keeps_history() {
        let mut grid = Grid::new(5, 2);
        print_str(&mut grid, "aaaaabbbbbccccc");
        let history = grid.scrollback_len();
        assert!(history > 0);
        grid.set_dec_mode(DecMode::AutoWrap, false);
        grid.set_dec_mode(DecMode::CursorVisible, false);
        grid.designate_charset(CharsetSlot::G0, Charset::DecSpecialGraphics);
        grid.set_scroll_region(0, 0);

        grid.full_reset();

        assert_eq!(grid.to_text(), "");
        assert_eq!(grid.cursor(), (0, 0));
        assert!(grid.dec_mode(DecMode::AutoWrap) && grid.dec_mode(DecMode::CursorVisible));
        assert_eq!((grid.scroll_top(), grid.scroll_bottom()), (0, 1));
        assert_eq!(
            grid.scrollback_len(),
            history,
            "the user's history survives"
        );
        grid.print('q');
        assert_eq!(
            grid.cell(0, 0).map(|c| c.ch),
            Some('q'),
            "charsets back to ASCII"
        );
    }

    #[test]
    fn test_full_reset_leaves_the_alt_screen() {
        let mut grid = Grid::new(5, 2);
        grid.set_dec_mode(DecMode::AltScreen, true);
        grid.full_reset();
        assert!(!grid.is_alt_screen());
    }

    #[test]
    fn test_soft_reset_keeps_the_screen_and_cursor() {
        let mut grid = Grid::new(5, 2);
        print_str(&mut grid, "ab");
        grid.set_style(Style {
            reversed: true,
            ..Style::default()
        });
        grid.set_dec_mode(DecMode::CursorVisible, false);
        grid.soft_reset();
        assert_eq!(grid.to_text(), "ab");
        assert_eq!(grid.cursor(), (0, 2));
        assert_eq!(grid.style(), Style::default());
        assert!(grid.cursor_visible());
    }

    #[test]
    fn test_alignment_pattern_fills_the_screen_and_homes_the_cursor() {
        let mut grid = Grid::new(3, 2);
        grid.set_scroll_region(1, 1);
        grid.move_to(1, 2);
        grid.fill_alignment_pattern();
        assert_eq!(grid.to_text(), "EEE\nEEE");
        assert_eq!(grid.cursor(), (0, 0));
        assert_eq!(grid.scroll_top(), 0);
    }
}

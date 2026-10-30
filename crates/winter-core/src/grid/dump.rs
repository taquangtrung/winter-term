//! Writing a grid out as the terminal output that would build it again.
//!
//! A window that replaces its own binary keeps the programs in its panes
//! running, but the new process starts with empty grids. This is the bridge:
//! the old grid is dumped as ordinary escape sequences (text, colors, cursor
//! moves, the modes a program has set) and the new one is built by feeding
//! them through its own parser, exactly as if the program had drawn them.
//! Nothing here needs a serialization format of its own, and what the new
//! binary's parser understands is by definition what it can restore.

use super::cell::{Cell, CellWidth, Style};
use super::charset::Charsets;
use super::{AnsiMode, DecMode};
use super::{Grid, SavedCursor};

// ========================================================================
// Constants
// ========================================================================

/// Control sequences, written out so the dump reads as what it is.
const CSI: &str = "\x1b[";
const OSC_LINK: &str = "\x1b]8;;";
const ST: &str = "\x1b\\";
const RESET: &str = "\x1b[0m";
const NEWLINE: &str = "\r\n";
/// Final bytes of SM/DECSET and RM/DECRST.
const MODE_SET: char = 'h';
const MODE_RESET: char = 'l';

/// DECSC: save the cursor.
const DECSC: &str = "\x1b7";

/// Switches to the alternate screen, saving the cursor it leaves.
const ENTER_ALT_SCREEN: &str = "\x1b[?1049h";

// ========================================================================
// Grid
// ========================================================================

impl Grid {
    /// The terminal output that, fed to a fresh grid of the same size, rebuilds
    /// this one: its scrollback, its screen (and the primary screen under an
    /// alternate one), where the cursor is, and the modes a program set.
    ///
    /// Blocks and other state the host keeps outside the grid are not in it.
    pub fn dump_ansi(&self) -> Vec<u8> {
        let mut out = Dump::default();

        // The primary screen is what the scrollback sits above. With an
        // alternate screen up, the primary cells are the ones it set aside.
        let (primary, primary_cursor) = match &self.alt_buffer {
            Some(alt) => (alt.cells.as_slice(), Some((alt.cursor.row, alt.cursor.col))),
            None => (self.cells.as_slice(), None),
        };
        out.history_and_screen(self, primary);

        if self.alt_buffer.is_some() {
            // The alternate screen saves the cursor it leaves, so the primary
            // cursor is put where it was before entering it.
            // The primary screen's own DECSC save is set aside with it, so it
            // is made before entering the alternate screen.
            if let Some(saved) = self.alt_buffer.as_ref().and_then(|alt| alt.saved_cursor) {
                out.saved_cursor(&saved, 0);
            }
            if let Some((row, col)) = primary_cursor {
                out.cursor_to(row, col);
            }
            out.raw(ENTER_ALT_SCREEN);
            out.screen_at_positions(self, &self.cells);
        }

        if self.scroll_top != 0 || self.scroll_bottom + 1 != self.rows {
            out.raw(&format!(
                "{CSI}{};{}r",
                self.scroll_top + 1,
                self.scroll_bottom + 1
            ));
        }
        if let Some(saved) = self.saved_cursor {
            out.saved_cursor(&saved, self.scroll_top);
        }
        out.modes(self);
        // Under origin mode a CUP row counts from the scroll region's top.
        let row = if self.modes.origin {
            self.cursor.row.saturating_sub(self.scroll_top)
        } else {
            self.cursor.row
        };
        out.cursor_to(row, self.cursor.col);
        out.set_style(&self.style);
        out.close_link();
        out.finish()
    }
}

// ========================================================================
// Dump
// ========================================================================

/// The output being built and the pen state it has left the terminal in, so
/// each cell only writes what differs from the one before.
#[derive(Default)]
struct Dump {
    link: u16,
    out: String,
    style: Style,
}

impl Dump {
    fn raw(&mut self, text: &str) {
        self.out.push_str(text);
    }

    fn finish(mut self) -> Vec<u8> {
        self.raw(RESET);
        self.out.into_bytes()
    }

    /// Every scrollback row, then every row of `screen`, one after another so
    /// that the terminal scrolls the history up exactly as it was.
    fn history_and_screen(&mut self, grid: &Grid, screen: &[Cell]) {
        for row in &grid.scrollback {
            let wrapped = row.meta.wrapped;
            self.row(grid, &row.cells, wrapped);
            if !wrapped {
                self.raw(NEWLINE);
            }
        }
        for row in 0..grid.rows {
            let cells = &screen[row * grid.cols..(row + 1) * grid.cols];
            let wrapped = grid.row_meta.get(row).is_some_and(|meta| meta.wrapped);
            self.row(grid, cells, wrapped);
            // No newline after the last row: that would scroll one too many.
            let last = row + 1 == grid.rows;
            if !last && !wrapped {
                self.raw(NEWLINE);
            }
        }
    }

    /// The cells of one row. A row that ended by running out of width is
    /// written whole, and gets no break after it from the caller, since the
    /// terminal wraps it itself; any other stops at its last real cell.
    fn row(&mut self, grid: &Grid, cells: &[Cell], wrapped: bool) {
        let end = if wrapped {
            cells.len()
        } else {
            content_end(cells)
        };
        for cell in &cells[..end] {
            self.cell(grid, cell);
        }
    }

    /// The rows of an alternate screen, each placed where it belongs rather
    /// than reached by scrolling.
    fn screen_at_positions(&mut self, grid: &Grid, screen: &[Cell]) {
        for row in 0..grid.rows {
            let cells = &screen[row * grid.cols..(row + 1) * grid.cols];
            let end = content_end(cells);
            if end == 0 {
                continue;
            }
            self.cursor_to(row, 0);
            for cell in &cells[..end] {
                self.cell(grid, cell);
            }
        }
    }

    fn cell(&mut self, grid: &Grid, cell: &Cell) {
        if cell.width == CellWidth::Spacer {
            return;
        }
        self.set_style(&cell.style);
        self.set_link(grid, cell.style.link);
        self.out.push(cell.ch);
        if let Some(tail) = &cell.tail {
            self.out.push_str(tail);
        }
    }

    /// Make `saved` the terminal's DECSC save: set up the cursor, pen,
    /// character sets and origin mode it holds, and save them with `ESC 7`.
    /// Leaves the character sets and origin mode at their defaults again, so
    /// the live state can be written after it as onto a fresh terminal. A
    /// pending wrap cannot be recreated by positioning and is not restored.
    fn saved_cursor(&mut self, saved: &SavedCursor, scroll_top: usize) {
        let origin = DecMode::Origin.code();
        let row = if saved.origin {
            self.raw(&format!("{CSI}?{origin}{MODE_SET}"));
            saved.row.saturating_sub(scroll_top)
        } else {
            saved.row
        };
        self.cursor_to(row, saved.col);
        self.set_style(&saved.style);
        self.raw(&saved.charsets.explicit_sequence());
        self.raw(DECSC);
        if saved.origin {
            self.raw(&format!("{CSI}?{origin}{MODE_RESET}"));
        }
        self.raw(&Charsets::default().explicit_sequence());
    }

    fn cursor_to(&mut self, row: usize, col: usize) {
        self.raw(&format!("{CSI}{};{}H", row + 1, col + 1));
    }

    /// The modes, character sets and tab stops a program set, as the
    /// sequences that set them. Written after the screen's cells: those are
    /// already translated through the character sets and laid out under the
    /// wrap and insert modes in force when drawn, so replaying them must not
    /// go through any of these again.
    fn modes(&mut self, grid: &Grid) {
        let fresh = Grid::new(grid.cols, grid.rows);
        for mode in DecMode::ALL.into_iter().filter(|&mode| is_dumped(mode)) {
            let set = grid.dec_mode(mode);
            if set != fresh.dec_mode(mode) {
                self.raw(&format!("{CSI}?{}{}", mode.code(), mode_final(set)));
            }
        }
        let insert = AnsiMode::Insert;
        if grid.ansi_mode(insert) != fresh.ansi_mode(insert) {
            let set = grid.ansi_mode(insert);
            self.raw(&format!("{CSI}{}{}", insert.code(), mode_final(set)));
        }
        self.raw(&grid.charsets.restore_sequence());
        if let Some(tabs) = grid.tabs.restore_sequence() {
            self.raw(&tabs);
        }
    }

    /// Change the pen to `style`, if it is not already it.
    fn set_style(&mut self, style: &Style) {
        let wanted = Style { link: 0, ..*style };
        let current = Style {
            link: 0,
            ..self.style
        };
        if wanted == current {
            return;
        }
        // Start from a clean pen rather than working out what to turn off.
        self.raw(&format!("{CSI}{}m", wanted.sgr_params()));
        self.style = wanted;
    }

    fn set_link(&mut self, grid: &Grid, link: u16) {
        if link == self.link {
            return;
        }
        match grid.link_url(link).filter(|_| link != 0) {
            Some(url) => self.raw(&format!("{OSC_LINK}{url}{ST}")),
            None => self.raw(&format!("{OSC_LINK}{ST}")),
        }
        self.link = link;
    }

    fn close_link(&mut self) {
        if self.link != 0 {
            self.raw(&format!("{OSC_LINK}{ST}"));
            self.link = 0;
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Whether `mode` is written out as a mode. The alternate screen is dumped
/// as the screens themselves, saving the cursor is an action rather than a
/// state, a permanent mode cannot differ, and a synchronized frame in flight
/// is the old process's business.
fn is_dumped(mode: DecMode) -> bool {
    match mode {
        DecMode::AltScreen
        | DecMode::AltScreenClear
        | DecMode::AltScreenLegacy
        | DecMode::GraphemeClusters
        | DecMode::SaveCursor
        | DecMode::SyncOutput => false,
        DecMode::AppCursorKeys
        | DecMode::AppKeypad
        | DecMode::AutoWrap
        | DecMode::BracketedPaste
        | DecMode::CursorVisible
        | DecMode::FocusEvent
        | DecMode::MouseAnyEvent
        | DecMode::MouseButton
        | DecMode::MouseDrag
        | DecMode::MouseSgr
        | DecMode::Origin
        | DecMode::ReverseVideo => true,
    }
}

/// The final byte that sets (`h`) or resets (`l`) a mode.
fn mode_final(set: bool) -> char {
    if set {
        MODE_SET
    } else {
        MODE_RESET
    }
}

/// Where a row stops mattering: past its last cell that is not a plain blank.
fn content_end(cells: &[Cell]) -> usize {
    let blank = Cell::default();
    cells
        .iter()
        .rposition(|cell| *cell != blank)
        .map_or(0, |last| last + 1)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Color;
    use crate::Screen;

    /// Build a screen from `bytes`, dump it, and rebuild it from the dump.
    fn round_trip(cols: usize, rows: usize, bytes: &[u8]) -> (Screen, Screen) {
        let mut original = Screen::new(cols, rows);
        original.feed(bytes);
        let mut rebuilt = Screen::new(cols, rows);
        rebuilt.feed(&original.grid().dump_ansi());
        (original, rebuilt)
    }

    fn same_screen(a: &Grid, b: &Grid) {
        assert_eq!(a.to_text(), b.to_text(), "visible text");
        assert_eq!(a.cursor(), b.cursor(), "cursor");
        for row in 0..a.rows() {
            for col in 0..a.cols() {
                assert_eq!(a.cell(row, col), b.cell(row, col), "cell {row},{col}");
            }
        }
    }

    #[test]
    fn test_text_colors_and_cursor_come_back() {
        let (original, rebuilt) = round_trip(
            20,
            4,
            b"plain \x1b[1;31mred bold\x1b[0m\r\n\x1b[38;2;10;20;30;48;5;200mtrue\x1b[0m \x1b[4mu\x1b[0m\x1b[3;7H",
        );
        same_screen(original.grid(), rebuilt.grid());
    }

    #[test]
    fn test_scrollback_comes_back_above_the_screen() {
        let mut bytes = Vec::new();
        for n in 0..12 {
            bytes.extend_from_slice(format!("line {n}\r\n").as_bytes());
        }
        bytes.extend_from_slice(b"prompt$ ");
        let (original, rebuilt) = round_trip(20, 4, &bytes);
        assert!(original.grid().scrollback_len() > 0);
        assert_eq!(
            original.grid().scrollback_len(),
            rebuilt.grid().scrollback_len(),
            "the same history scrolled off"
        );
        same_screen(original.grid(), rebuilt.grid());
    }

    #[test]
    fn test_a_long_wrapped_line_stays_one_line_in_the_history() {
        let long = "x".repeat(45);
        let mut bytes = long.clone().into_bytes();
        bytes.extend_from_slice(b"\r\nnext\r\nmore\r\nand more\r\nlast");
        let (original, rebuilt) = round_trip(20, 3, &bytes);
        same_screen(original.grid(), rebuilt.grid());
        assert_eq!(
            original.grid().scrollback_len(),
            rebuilt.grid().scrollback_len()
        );
    }

    #[test]
    fn test_wide_characters_survive() {
        let (original, rebuilt) = round_trip(20, 2, "宽字 end".as_bytes());
        same_screen(original.grid(), rebuilt.grid());
    }

    #[test]
    fn test_a_hyperlink_is_written_around_the_cells_it_covers() {
        // Hyperlinks are read by the host's own parser, not by `Screen`, so
        // what is checked is that the dump says them the way a program would.
        let mut grid = Grid::new(20, 2);
        grid.set_active_link(Some("https://example.com"));
        for ch in "link".chars() {
            grid.print(ch);
        }
        grid.set_active_link(None);
        grid.print('!');

        let dump = String::from_utf8(grid.dump_ansi()).expect("text");
        assert!(
            dump.contains("\x1b]8;;https://example.com\x1b\\link\x1b]8;;\x1b\\!"),
            "got {dump:?}"
        );
    }

    #[test]
    fn test_every_mode_and_charset_state_survives_without_redrawing_the_cells() {
        // Line-drawing glyphs are dumped as drawn, and the charset is restored
        // only after them, so they are not translated a second time; autowrap
        // and insert mode likewise come back only after the cells are laid.
        let (original, rebuilt) = round_trip(
            10,
            2,
            b"\x1b(0lqk\x1b[?1h\x1b[?7l\x1b[4h\x1b[?1003h\x1b[3g\x1b[1;4H\x1bH\x1b[1;9H",
        );
        same_screen(original.grid(), rebuilt.grid());
        let mut grids = [original, rebuilt];
        for screen in &mut grids {
            let grid = screen.grid();
            assert!(grid.dec_mode(DecMode::AppCursorKeys));
            assert!(!grid.dec_mode(DecMode::AutoWrap));
            assert!(grid.ansi_mode(AnsiMode::Insert));
            assert!(grid.mouse_any_event());
            screen.feed(b"\x1b[4l\rq\t");
        }
        for screen in &grids {
            assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('─'));
            assert_eq!(screen.grid().cursor(), (0, 3), "the custom tab stop");
        }
    }

    #[test]
    fn test_the_decsc_save_survives_on_both_screens() {
        // A save on the primary screen, then a different one on the
        // alternate screen: after the rebuild each `ESC 8` lands where, and
        // draws how, the program saved it.
        let (original, rebuilt) = round_trip(
            10,
            4,
            b"\x1b[2;3H\x1b[32m\x1b(0\x1b7\x1b[0m\x1b(B\x1b[1;1H\x1b[?1049h\x1b[3;6H\x1b[1m\x1b7\x1b[0m\x1b[1;1H",
        );
        let mut screens = [original, rebuilt];
        for screen in &mut screens {
            screen.feed(b"\x1b8X");
        }
        same_screen(screens[0].grid(), screens[1].grid());
        let alt = screens[1].grid().cell(2, 5).expect("cell");
        assert!(alt.ch == 'X' && alt.style.bold);
        for screen in &mut screens {
            screen.feed(b"\x1b[?1049l\x1b8q");
        }
        same_screen(screens[0].grid(), screens[1].grid());
        let primary = screens[1].grid().cell(1, 2).expect("cell");
        assert_eq!(
            (primary.ch, primary.style.foreground),
            ('─', Color::Indexed(2))
        );
    }

    #[test]
    fn test_modes_a_program_set_are_set_again() {
        let (original, rebuilt) = round_trip(
            10,
            2,
            b"\x1b[?2004h\x1b[?1004h\x1b[?1002h\x1b[?1006h\x1b[?25lhi",
        );
        for grid in [original.grid(), rebuilt.grid()] {
            assert!(grid.bracketed_paste());
            assert!(grid.focus_event());
            assert!(grid.mouse_drag_tracking());
            assert!(grid.mouse_sgr());
        }
        assert_eq!(
            original.grid().cursor_visible(),
            rebuilt.grid().cursor_visible()
        );
    }

    #[test]
    fn test_an_alternate_screen_comes_back_over_the_primary_one() {
        let (original, rebuilt) = round_trip(
            12,
            3,
            b"shell prompt\r\n\x1b[2;3H\x1b[?1049h\x1b[Halt top\x1b[3;1Hbottom row",
        );
        assert!(rebuilt.grid().is_alt_screen());
        same_screen(original.grid(), rebuilt.grid());

        // Leaving the alternate screen gives back the shell as it was.
        let mut original = original;
        let mut rebuilt = rebuilt;
        original.feed(b"\x1b[?1049l");
        rebuilt.feed(b"\x1b[?1049l");
        same_screen(original.grid(), rebuilt.grid());
    }

    #[test]
    fn test_a_scroll_region_is_kept() {
        let (original, rebuilt) = round_trip(10, 5, b"top\x1b[2;4r\x1b[4;1Hx");
        assert_eq!(original.grid().scroll_top(), rebuilt.grid().scroll_top());
        assert_eq!(
            original.grid().scroll_bottom(),
            rebuilt.grid().scroll_bottom()
        );
    }
}

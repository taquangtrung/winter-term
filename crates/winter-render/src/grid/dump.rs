//! Writing a grid out as the terminal output that would build it again.
//!
//! A window that replaces its own binary keeps the programs in its panes
//! running, but the new process starts with empty grids. This is the bridge:
//! the old grid is dumped as ordinary escape sequences (text, colors, cursor
//! moves, the modes a program has set) and the new one is built by feeding
//! them through its own parser, exactly as if the program had drawn them.
//! Nothing here needs a serialization format of its own, and what the new
//! binary's parser understands is by definition what it can restore.

use super::cell::{Cell, CellWidth, Color, Style};
use super::Grid;

// ========================================================================
// Constants
// ========================================================================

/// Control sequences, written out so the dump reads as what it is.
const CSI: &str = "\x1b[";
const OSC_LINK: &str = "\x1b]8;;";
const ST: &str = "\x1b\\";
const RESET: &str = "\x1b[0m";
const NEWLINE: &str = "\r\n";

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
        out.modes(self);
        out.cursor_to(self.cursor.row, self.cursor.col);
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
        for (index, row) in grid.scrollback.iter().enumerate() {
            let wrapped = grid.scrollback_wrapped.get(index).copied().unwrap_or(false);
            self.row(grid, row, wrapped);
            if !wrapped {
                self.raw(NEWLINE);
            }
        }
        for row in 0..grid.rows {
            let cells = &screen[row * grid.cols..(row + 1) * grid.cols];
            let wrapped = grid.row_wrapped.get(row).copied().unwrap_or(false);
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

    fn cursor_to(&mut self, row: usize, col: usize) {
        self.raw(&format!("{CSI}{};{}H", row + 1, col + 1));
    }

    fn modes(&mut self, grid: &Grid) {
        let modes = [
            (grid.bracketed_paste, 2004),
            (grid.focus_event, 1004),
            (grid.mouse_button, 1000),
            (grid.mouse_drag, 1002),
            (grid.mouse_sgr, 1006),
        ];
        for (set, mode) in modes {
            if set {
                self.raw(&format!("{CSI}?{mode}h"));
            }
        }
        if !grid.cursor_visible {
            self.raw(&format!("{CSI}?25l"));
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
        let mut codes = vec!["0".to_string()];
        if wanted.bold {
            codes.push("1".into());
        }
        if wanted.faint {
            codes.push("2".into());
        }
        if wanted.italic {
            codes.push("3".into());
        }
        if wanted.underline {
            codes.push("4".into());
        }
        if wanted.reversed {
            codes.push("7".into());
        }
        if wanted.strikethrough {
            codes.push("9".into());
        }
        push_color(&mut codes, wanted.foreground, 38);
        push_color(&mut codes, wanted.background, 48);
        self.raw(&format!("{CSI}{}m", codes.join(";")));
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

/// Where a row stops mattering: past its last cell that is not a plain blank.
fn content_end(cells: &[Cell]) -> usize {
    let blank = Cell::default();
    cells
        .iter()
        .rposition(|cell| *cell != blank)
        .map_or(0, |last| last + 1)
}

/// The SGR parameters selecting `color` as foreground (`38`) or background
/// (`48`); nothing for the default color, which the reset already is.
fn push_color(codes: &mut Vec<String>, color: Color, base: u8) {
    match color {
        Color::Default => {}
        Color::Indexed(index) => codes.push(format!("{base};5;{index}")),
        Color::Rgb(rgb) => codes.push(format!("{base};2;{};{};{}", rgb.r, rgb.g, rgb.b)),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
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

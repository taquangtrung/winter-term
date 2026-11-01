//! The VT driver: parses a byte stream with `vte` and applies it to a [`Grid`]
//! (printing, cursor motion, erase, and SGR styling).

use vte::{Params, Parser, Perform};

use crate::grid::{
    AnsiMode, Charset, CharsetSlot, Color, CursorShape, DecMode, EraseMode, Grid, RgbColor, Style,
    UnderlineStyle,
};
use crate::osc_guard::OscGuard;

// ========================================================================
// Constants
// ========================================================================

const SGR_EXT_RGB: u16 = 2;
const SGR_EXT_INDEXED: u16 = 5;
const SGR_FOREGROUND_EXT: u16 = 38;
const SGR_BACKGROUND_EXT: u16 = 48;
const SGR_UNDERLINE_COLOR_EXT: u16 = 58;
const SGR_UNDERLINE: u16 = 4;
/// Shortest colon-joined true-color group: `x:2:r:g:b`.
const SGR_RGB_GROUP_LEN: usize = 5;
const BRIGHT_OFFSET: u8 = 8;
const BELL: u8 = 0x07;
const BACKSPACE: u8 = 0x08;
const HORIZONTAL_TAB: u8 = b'\t';
const LINE_FEED: u8 = b'\n';
const VERTICAL_TAB: u8 = 0x0b;
const FORM_FEED: u8 = 0x0c;
const CARRIAGE_RETURN: u8 = b'\r';
/// SO: print from the G1 character set.
const SHIFT_OUT: u8 = 0x0e;
/// SI: print from the G0 character set.
const SHIFT_IN: u8 = 0x0f;
/// ED parameter that clears the scrollback instead of the screen.
const ERASE_SCROLLBACK: usize = 3;
/// DECSCUSR parameter that hands the cursor shape back to the host.
const CURSOR_STYLE_DEFAULT: usize = 0;

// ========================================================================
// Data Structures
// ========================================================================

/// A terminal screen: a `vte` parser feeding a [`Grid`].
pub struct Screen {
    grid: Grid,
    /// Cuts off OSC strings vte would otherwise buffer without limit.
    osc: OscGuard,
    parser: Parser,
    bell: bool,
}

// ========================================================================
// Screen
// ========================================================================

impl Screen {
    /// A blank screen `cols` by `rows` cells, with a fresh VT parser.
    pub fn new(cols: usize, rows: usize) -> Self {
        Self {
            grid: Grid::new(cols, rows),
            osc: OscGuard::default(),
            parser: Parser::new(),
            bell: false,
        }
    }

    /// Consume a chunk of terminal output, updating the grid.
    pub fn feed(&mut self, bytes: &[u8]) {
        let bytes = self.osc.filter(bytes);
        self.parser.advance(&mut self.grid, &bytes);
        if bytes.contains(&BELL) {
            self.bell = true;
        }
    }

    /// The cell grid this screen drives.
    pub fn grid(&self) -> &Grid {
        &self.grid
    }

    /// The cell grid, mutably, for callers that manipulate it outside the
    /// byte stream (selection, URL detection, scroll position).
    pub fn grid_mut(&mut self) -> &mut Grid {
        &mut self.grid
    }

    /// Whether a bell character was received since the last check.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell)
    }

    /// Resize the grid, reflowing its content to the new width.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        self.grid.resize(cols, rows);
    }
}

// ========================================================================
// vte driver
// ========================================================================

impl Perform for Grid {
    fn print(&mut self, c: char) {
        Grid::print(self, c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // VT and FF are line feeds on every terminal since the VT100.
            LINE_FEED | VERTICAL_TAB | FORM_FEED => self.line_feed(),
            CARRIAGE_RETURN => self.carriage_return(),
            BACKSPACE => self.backspace(),
            HORIZONTAL_TAB => self.tab(),
            SHIFT_OUT => self.shift_charset(true),
            SHIFT_IN => self.shift_charset(false),
            _ => {}
        }
    }

    /// Route a control sequence by its intermediate bytes first: the same
    /// final byte means different things behind `?`, `>`, `!` or a space
    /// (`CSI ? 1049 s` saves a mode, `CSI s` saves the cursor), so a sequence
    /// is only acted on under the exact intermediates it was defined with.
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        match (intermediates, action) {
            ([], _) => dispatch_csi(self, params, action),
            ([b'?'], _) => dispatch_private_csi(self, params, action),
            ([b' '], 'q') => set_cursor_style(self, first_param(params, 0)),
            ([b'!'], 'p') => self.soft_reset(),
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}

    fn put(&mut self, _byte: u8) {}

    fn unhook(&mut self) {}

    fn osc_dispatch(&mut self, _params: &[&[u8]], _bell_terminated: bool) {}

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        match (intermediates, byte) {
            ([], b'7') => self.save_cursor(),
            ([], b'8') => self.restore_cursor(),
            // IND (Index): down a row, scrolling the region at the bottom
            // margin. A line feed, without any carriage return.
            ([], b'D') => self.line_feed(),
            // NEL (Next Line): a carriage return and a line feed.
            ([], b'E') => self.next_line(),
            // HTS: set a tab stop at the cursor's column.
            ([], b'H') => self.set_tab_stop(),
            // RI (Reverse Index): up a row, scrolling the region at the top
            // margin. Used by full-screen apps and shell widgets alike to
            // reserve blank rows below the cursor without touching its column.
            ([], b'M') => self.reverse_index(),
            // RIS: full reset.
            ([], b'c') => self.full_reset(),
            // DECKPAM / DECKPNM: application or numeric keypad.
            ([], b'=') => self.set_dec_mode(DecMode::AppKeypad, true),
            ([], b'>') => self.set_dec_mode(DecMode::AppKeypad, false),
            // DECALN: the screen alignment test.
            ([b'#'], b'8') => self.fill_alignment_pattern(),
            // SCS: designate a character set into G0 (`(`) or G1 (`)`).
            ([slot], set) => {
                if let Some(slot) = CharsetSlot::from_intermediate(*slot) {
                    self.designate_charset(slot, Charset::from_designator(set));
                }
            }
            _ => {}
        }
    }
}

// ========================================================================
// CSI dispatch
// ========================================================================

/// A control sequence with no intermediates.
fn dispatch_csi(grid: &mut Grid, params: &Params, action: char) {
    match action {
        '@' => grid.insert_chars(first_param(params, 1)),
        'A' => grid.move_up(first_param(params, 1)),
        // CUD and VPR (`CSI n e`) are the same motion.
        'B' | 'e' => grid.move_down(first_param(params, 1)),
        // CUF and HPR (`CSI n a`) are the same motion.
        'C' | 'a' => grid.move_right(first_param(params, 1)),
        'D' => grid.move_left(first_param(params, 1)),
        'E' => {
            grid.move_down(first_param(params, 1));
            grid.carriage_return();
        }
        'F' => {
            grid.move_up(first_param(params, 1));
            grid.carriage_return();
        }
        // CHA and HPA (`` CSI n ` ``) are the same motion.
        'G' | '`' => grid.move_to_column(first_param(params, 1).saturating_sub(1)),
        'H' | 'f' => {
            let row = first_param(params, 1).saturating_sub(1);
            let col = nth_param(params, 1, 1).saturating_sub(1);
            grid.move_to(row, col);
        }
        'I' => grid.tab_forward(first_param(params, 1)),
        'J' => match first_param(params, 0) {
            // CSI 3 J erases the scrollback (xterm extension used by `clear`).
            ERASE_SCROLLBACK => grid.clear_scrollback(),
            mode => grid.erase_in_display(erase_mode(mode)),
        },
        'K' => grid.erase_in_line(erase_mode(first_param(params, 0))),
        'L' => grid.insert_lines(first_param(params, 1)),
        'M' => grid.delete_lines(first_param(params, 1)),
        'P' => grid.delete_chars(first_param(params, 1)),
        'S' => grid.scroll_up(first_param(params, 1)),
        'T' => grid.scroll_down(first_param(params, 1)),
        'X' => grid.erase_chars(first_param(params, 1)),
        'Z' => grid.tab_backward(first_param(params, 1)),
        'b' => grid.repeat_last_graphic(first_param(params, 1)),
        'd' => grid.move_to_row(first_param(params, 1).saturating_sub(1)),
        'g' => grid.clear_tab_stops(first_param(params, 0)),
        'h' | 'l' => {
            for mode in mode_codes(params).filter_map(AnsiMode::from_code) {
                grid.set_ansi_mode(mode, action == 'h');
            }
        }
        'm' => {
            let mut style = grid.style();
            apply_sgr(params, &mut style);
            grid.set_style(style);
        }
        'r' => {
            let top = first_param(params, 1).saturating_sub(1);
            let bottom = match params.iter().nth(1).and_then(|p| p.first()) {
                Some(&0) | None => grid.rows(),
                Some(&n) => n as usize,
            };
            if top == 0 && bottom == grid.rows() {
                grid.reset_scroll_region();
            } else {
                grid.set_scroll_region(top, bottom.saturating_sub(1));
            }
        }
        's' => grid.save_cursor(),
        'u' => grid.restore_cursor(),
        _ => {}
    }
}

/// A control sequence behind `?`: DEC private modes and their save/restore,
/// and the selective erases (no cell is ever protected here, so they erase
/// exactly as their plain counterparts do).
fn dispatch_private_csi(grid: &mut Grid, params: &Params, action: char) {
    match action {
        'h' | 'l' => {
            for code in mode_codes(params) {
                grid.set_private_mode(code, action == 'h');
            }
        }
        's' => grid.save_dec_modes(mode_codes(params)),
        'r' => grid.restore_dec_modes(mode_codes(params)),
        'J' => grid.erase_in_display(erase_mode(first_param(params, 0))),
        'K' => grid.erase_in_line(erase_mode(first_param(params, 0))),
        _ => {}
    }
}

/// DECSCUSR (`CSI Ps SP q`): 0 hands the shape back to the host; 1-6 pick a
/// blinking or steady block, underline, or bar. Anything else is ignored.
fn set_cursor_style(grid: &mut Grid, style: usize) {
    let shape = match style {
        CURSOR_STYLE_DEFAULT => return grid.reset_cursor_shape(),
        1 | 2 => CursorShape::Block,
        3 | 4 => CursorShape::Underline,
        5 | 6 => CursorShape::Bar,
        _ => return,
    };
    grid.set_cursor_shape(shape);
}

/// The mode numbers in a set/reset/save/restore sequence, one per parameter.
fn mode_codes(params: &Params) -> impl Iterator<Item = u16> + '_ {
    params.iter().filter_map(|sub| sub.first().copied())
}

// ========================================================================
// CSI / SGR helpers
// ========================================================================

fn first_param(params: &Params, default: usize) -> usize {
    nth_param(params, 0, default)
}

fn nth_param(params: &Params, index: usize, default: usize) -> usize {
    match params.iter().nth(index).and_then(|p| p.first()) {
        Some(&0) | None => default,
        Some(&value) => value as usize,
    }
}

fn erase_mode(value: usize) -> EraseMode {
    match value {
        1 => EraseMode::ToStart,
        2 => EraseMode::Whole,
        _ => EraseMode::ToEnd,
    }
}

/// Apply an SGR sequence to `style`. A colon-joined group (`4:3`, `38:2::r:g:b`)
/// is one code with sub-parameters; semicolon-separated codes are independent,
/// and the semicolon form of extended colors (`38;5;n`) spans several groups.
fn apply_sgr(params: &Params, style: &mut Style) {
    let groups: Vec<&[u16]> = params.iter().collect();
    if groups.is_empty() {
        *style = Style::default();
        return;
    }

    let mut index = 0;
    while index < groups.len() {
        let group = groups[index];
        if group.len() > 1 {
            apply_sgr_subparams(group, style);
            index += 1;
        } else {
            let flat: Vec<u16> = groups[index..]
                .iter()
                .filter_map(|g| g.first().copied())
                .collect();
            index += apply_sgr_code(&flat, style);
        }
    }
}

/// Apply one colon-joined SGR group: an underline style (`4:n`) or an extended
/// color whose components arrive as sub-parameters.
fn apply_sgr_subparams(group: &[u16], style: &mut Style) {
    match group[0] {
        SGR_UNDERLINE => style.underline = UnderlineStyle::from_sgr_subparam(group[1]),
        SGR_FOREGROUND_EXT => {
            if let Some(color) = subparam_color(group) {
                style.foreground = color;
            }
        }
        SGR_BACKGROUND_EXT => {
            if let Some(color) = subparam_color(group) {
                style.background = color;
            }
        }
        SGR_UNDERLINE_COLOR_EXT => {
            if let Some(color) = subparam_color(group) {
                style.underline_color = color;
            }
        }
        _ => {}
    }
}

/// The color in a colon-joined `38`/`48` group: `x:5:n` indexed, or `x:2:r:g:b`
/// true color with an optional colorspace slot before the channels
/// (`x:2::r:g:b`).
fn subparam_color(group: &[u16]) -> Option<Color> {
    match group.get(1) {
        Some(&SGR_EXT_INDEXED) => group.get(2).map(|&value| Color::Indexed(value as u8)),
        Some(&SGR_EXT_RGB) if group.len() >= SGR_RGB_GROUP_LEN => {
            let channels = &group[group.len() - 3..];
            Some(Color::Rgb(RgbColor {
                r: channels[0] as u8,
                g: channels[1] as u8,
                b: channels[2] as u8,
            }))
        }
        _ => None,
    }
}

/// Apply one SGR code at the start of `codes`, returning how many codes it
/// consumed (extended colors consume several).
fn apply_sgr_code(codes: &[u16], style: &mut Style) -> usize {
    let code = codes[0];
    match code {
        0 => *style = Style::default(),
        1 => style.bold = true,
        2 => style.faint = true,
        3 => style.italic = true,
        4 => style.underline = UnderlineStyle::Single,
        // Slow and rapid blink blink alike.
        5 | 6 => style.blink = true,
        7 => style.reversed = true,
        8 => style.concealed = true,
        9 => style.strikethrough = true,
        // ECMA-48 double underline. (Some old terminals read 21 as "bold
        // off"; xterm, kitty and VTE all follow ECMA.)
        21 => style.underline = UnderlineStyle::Double,
        22 => {
            style.bold = false;
            style.faint = false;
        }
        23 => style.italic = false,
        24 => style.underline = UnderlineStyle::None,
        25 => style.blink = false,
        27 => style.reversed = false,
        28 => style.concealed = false,
        29 => style.strikethrough = false,
        53 => style.overline = true,
        55 => style.overline = false,
        59 => style.underline_color = Color::Default,
        30..=37 => style.foreground = Color::Indexed((code - 30) as u8),
        90..=97 => style.foreground = Color::Indexed((code - 90) as u8 + BRIGHT_OFFSET),
        39 => style.foreground = Color::Default,
        40..=47 => style.background = Color::Indexed((code - 40) as u8),
        100..=107 => style.background = Color::Indexed((code - 100) as u8 + BRIGHT_OFFSET),
        49 => style.background = Color::Default,
        SGR_FOREGROUND_EXT => return extended_color(codes, |color| style.foreground = color),
        SGR_BACKGROUND_EXT => return extended_color(codes, |color| style.background = color),
        SGR_UNDERLINE_COLOR_EXT => {
            return extended_color(codes, |color| style.underline_color = color)
        }
        _ => {}
    }
    1
}

/// Parse `38`/`48`/`58` extended color (`;5;n` indexed or `;2;r;g;b` true color),
/// returning the number of codes consumed including the leading code.
fn extended_color(codes: &[u16], mut set: impl FnMut(Color)) -> usize {
    match codes.get(1) {
        Some(&SGR_EXT_INDEXED) => match codes.get(2) {
            Some(&value) => {
                set(Color::Indexed(value as u8));
                3
            }
            None => 2,
        },
        Some(&SGR_EXT_RGB) => match (codes.get(2), codes.get(3), codes.get(4)) {
            (Some(&r), Some(&g), Some(&b)) => {
                set(Color::Rgb(RgbColor {
                    r: r as u8,
                    g: g as u8,
                    b: b as u8,
                }));
                5
            }
            _ => 2,
        },
        _ => 1,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::CellWidth;

    fn feed(cols: usize, rows: usize, bytes: &[u8]) -> Screen {
        let mut screen = Screen::new(cols, rows);
        screen.feed(bytes);
        screen
    }

    #[test]
    fn test_plain_text_and_newline() {
        let screen = feed(10, 2, b"hi\r\nthere");
        assert_eq!(screen.grid().to_text(), "hi\nthere");
    }

    #[test]
    fn test_spinner_rotates_in_place() {
        // A classic ASCII spinner: \r then a single rotating glyph each frame.
        let screen = feed(10, 2, b"|\r/\r-\r\\");
        assert_eq!(screen.grid().to_text(), "\\");
        assert_eq!(screen.grid().cursor(), (0, 1));
    }

    #[test]
    fn test_progress_bar_full_width_overwrites() {
        // A progress bar that fills the entire 5-col row, then redraws.
        let screen = feed(5, 2, b"[==] \r[===]");
        assert_eq!(screen.grid().to_text(), "[===]");
        assert_eq!(screen.grid().cursor(), (0, 4));
    }

    #[test]
    fn test_spinner_uses_cha_to_return_to_column_one() {
        // Many spinners return to the start of the line with CSI G (CHA) rather
        // than a bare \r. Each frame must overwrite the glyph in place.
        let screen = feed(10, 2, b"|\x1b[G/\x1b[G-\x1b[G\\");
        assert_eq!(screen.grid().to_text(), "\\");
        assert_eq!(screen.grid().cursor(), (0, 1));
    }

    #[test]
    fn test_cha_moves_to_explicit_column() {
        // CSI 3 G moves to column 3 (1-based), overwriting the third cell.
        let screen = feed(10, 1, b"abcde\x1b[3GX");
        assert_eq!(screen.grid().to_text(), "abXde");
    }

    #[test]
    fn test_ech_clears_field_in_place_for_spinner_redraw() {
        // CSI K erases to end then CSI G returns home, but a fixed-width field
        // is often cleared with ECH (CSI X) without moving the cursor.
        let screen = feed(10, 1, b"working...\x1b[G\x1b[4Xdone");
        assert_eq!(screen.grid().to_text(), "doneing...");
    }

    #[test]
    fn test_vpa_sets_absolute_row() {
        let screen = feed(5, 3, b"\x1b[2dX");
        assert_eq!(screen.grid().cursor(), (1, 1));
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some('X'));
    }

    #[test]
    fn test_cnl_cpl_move_lines_to_column_zero() {
        // CNL (E) drops a line to col 0; CPL (F) climbs back to col 0.
        let screen = feed(5, 3, b"ab\x1b[EX\x1b[FY");
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some('X'));
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('Y'));
    }

    #[test]
    fn test_csi_3j_clears_scrollback() {
        // Fill scrollback by overflowing a tiny screen, then CSI 3 J clears it.
        let mut screen = Screen::new(2, 2);
        screen.feed(b"a\r\nb\r\nc\r\nd\r\n");
        assert!(screen.grid().scrollback_len() > 0);
        screen.feed(b"\x1b[3J");
        assert_eq!(screen.grid().scrollback_len(), 0);
    }

    #[test]
    fn test_cursor_position_then_print_overwrites() {
        // CUP to row 1, col 1, then write: the 'X' lands at the origin.
        let screen = feed(5, 2, b"abc\x1b[1;1HX");
        assert_eq!(screen.grid().to_text(), "Xbc");
    }

    #[test]
    fn test_erase_line_from_cursor() {
        let screen = feed(8, 1, b"hello\x1b[3D\x1b[K");
        assert_eq!(screen.grid().to_text(), "he");
    }

    #[test]
    fn test_sgr_sets_bold_and_truecolor_foreground() {
        let screen = feed(4, 1, b"\x1b[1;38;2;10;20;30mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(cell.style.bold);
        assert_eq!(
            cell.style.foreground,
            Color::Rgb(RgbColor {
                r: 10,
                g: 20,
                b: 30
            })
        );
    }

    #[test]
    fn test_sgr_reset_clears_style() {
        let screen = feed(4, 1, b"\x1b[1m\x1b[0mz");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.bold);
        assert_eq!(cell.style.foreground, Color::Default);
    }

    #[test]
    fn test_sgr_faint_set_and_cleared_by_normal_intensity() {
        let screen = feed(4, 1, b"\x1b[2mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(cell.style.faint);
        assert!(!cell.style.bold);

        let screen = feed(4, 1, b"\x1b[1;2m\x1b[22mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.faint);
        assert!(!cell.style.bold);
    }

    #[test]
    fn test_sgr_strikethrough_set_and_cleared() {
        let screen = feed(4, 1, b"\x1b[9mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(cell.style.strikethrough);

        let screen = feed(4, 1, b"\x1b[9m\x1b[29mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.strikethrough);
    }

    #[test]
    fn test_sgr_underline_color_operands_are_not_read_as_codes() {
        let screen = feed(4, 1, b"\x1b[58;2;100;150;200mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.faint);
        assert!(!cell.style.bold);
        assert!(!cell.style.reversed);
        assert_eq!(cell.style.foreground, Color::Default);

        let screen = feed(4, 1, b"\x1b[58;5;1mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.bold);

        let screen = feed(4, 1, b"\x1b[58:2::1:2:3;1mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(cell.style.bold);
        assert_eq!(cell.style.foreground, Color::Default);
    }

    #[test]
    fn test_sgr_colon_underline_style_does_not_leak_into_other_codes() {
        let screen = feed(4, 1, b"\x1b[4:3mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert_eq!(cell.style.underline, UnderlineStyle::Curly);
        assert!(!cell.style.italic);

        let screen = feed(4, 1, b"\x1b[1m\x1b[4:0mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert_eq!(cell.style.underline, UnderlineStyle::None);
        assert!(cell.style.bold);
    }

    #[test]
    fn test_sgr_colon_extended_colors() {
        let rgb = Color::Rgb(RgbColor {
            r: 10,
            g: 20,
            b: 30,
        });
        for bytes in [&b"\x1b[38:2:10:20:30mZ"[..], b"\x1b[38:2::10:20:30mZ"] {
            let screen = feed(4, 1, bytes);
            let cell = screen.grid().cell(0, 0).expect("cell exists");
            assert_eq!(cell.style.foreground, rgb);
        }

        let screen = feed(4, 1, b"\x1b[48:5:200mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert_eq!(cell.style.background, Color::Indexed(200));
    }

    #[test]
    fn test_sgr_reverse_video_toggles() {
        let screen = feed(4, 1, b"\x1b[7mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(cell.style.reversed);

        let screen = feed(4, 1, b"\x1b[7m\x1b[27mZ");
        let cell = screen.grid().cell(0, 0).expect("cell exists");
        assert!(!cell.style.reversed);
    }

    #[test]
    fn test_csi_z_backtabs_from_a_spinner_to_the_input_line() {
        // agy's frame: redraw the spinner two rows up, drop back down, then
        // CBT to the input column. Mid-stop, on-stop, and past-zero all count.
        let screen = feed(100, 3, b"> abc\x1b[2A\x1b[87G\n\n\x1b[7Z");
        assert_eq!(screen.grid().cursor(), (2, 32));
        let screen = feed(40, 1, b"\x1b[17G\x1b[Z");
        assert_eq!(screen.grid().cursor(), (0, 8), "col 16 is a stop: back one");
        let screen = feed(40, 1, b"\x1b[12G\x1b[9Z");
        assert_eq!(screen.grid().cursor(), (0, 0));
    }

    #[test]
    fn test_csi_i_advances_n_tab_stops_and_clamps() {
        let screen = feed(20, 1, b"\x1b[3G\x1b[2I");
        assert_eq!(screen.grid().cursor(), (0, 16));
        let screen = feed(20, 1, b"\x1b[9I");
        assert_eq!(screen.grid().cursor(), (0, 19));
    }

    #[test]
    fn test_esc_7_8_save_restore_cursor() {
        let screen = feed(5, 3, b"\x1b[2;3H\x1b7\x1b[1;1HX\x1b8Y");
        assert_eq!(screen.grid().cursor(), (1, 3));
    }

    #[test]
    fn test_csi_s_u_save_restore_cursor() {
        let screen = feed(5, 3, b"\x1b[2;3H\x1b[s\x1b[1;1HX\x1b[uY");
        assert_eq!(screen.grid().cursor(), (1, 3));
    }

    #[test]
    fn test_alt_screen_via_csi_1049() {
        let screen = feed(3, 2, b"abc\x1b[?1049hXYZ");
        assert!(screen.grid().is_alt_screen());
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('X'));
    }

    #[test]
    fn test_alt_screen_restore_via_csi_1049() {
        let screen = feed(3, 2, b"abc\x1b[?1049hXYZ\x1b[?1049l");
        assert!(!screen.grid().is_alt_screen());
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('a'));
    }

    #[test]
    fn test_insert_lines_csi_l() {
        let screen = feed(3, 3, b"abcdef\x1b[2;1H\x1b[1L");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(screen.grid().cell(2, 0).map(|c| c.ch), Some('d'));
    }

    #[test]
    fn test_delete_lines_csi_m() {
        let screen = feed(3, 3, b"abcdef\x1b[1;1H\x1b[1M");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('d'));
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(screen.grid().cell(2, 0).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_insert_chars_csi_at() {
        let screen = feed(5, 1, b"hello\x1b[1;2H\x1b[2@");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(screen.grid().cell(0, 1).map(|c| c.ch), Some(' '));
    }

    #[test]
    fn test_delete_chars_csi_p() {
        let screen = feed(5, 1, b"hello\x1b[1;2H\x1b[2P");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('h'));
        assert_eq!(screen.grid().cell(0, 1).map(|c| c.ch), Some('l'));
    }

    #[test]
    fn test_esc_d_index_scrolls_at_the_bottom_margin() {
        // Regression: ESC D (IND) used to be dropped entirely by esc_dispatch,
        // so a program relying on it to advance a row at the scroll margin
        // (rather than `\n`) never actually scrolled.
        let screen = feed(3, 4, b"abcdefghijkl\x1b[2;3r\x1b[3;1H\x1bD");
        assert_eq!(screen.grid().cell(0, 0).map(|c| c.ch), Some('a'));
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some('g'));
    }

    #[test]
    fn test_esc_m_reverse_index_scrolls_at_the_top_margin() {
        // Regression: ESC M (RI) was entirely unimplemented, so nothing could
        // scroll the region down to open a blank row above the cursor.
        let screen = feed(3, 4, b"abcdefghijkl\x1b[2;3r\x1b[2;1H\x1bM");
        assert_eq!(screen.grid().cell(1, 0).map(|c| c.ch), Some(' '));
        assert_eq!(screen.grid().cell(2, 0).map(|c| c.ch), Some('d'));
    }

    #[test]
    fn test_decscusr_cursor_shape() {
        let mut screen = Screen::new(5, 1);
        assert_eq!(screen.grid().reported_cursor_shape(), None);

        screen.feed(b"\x1b[5 q");
        assert_eq!(
            screen.grid().reported_cursor_shape(),
            Some(CursorShape::Bar)
        );

        screen.feed(b"\x1b[3 q");
        assert_eq!(
            screen.grid().reported_cursor_shape(),
            Some(CursorShape::Underline)
        );

        screen.feed(b"\x1b[1 q");
        assert_eq!(
            screen.grid().reported_cursor_shape(),
            Some(CursorShape::Block)
        );

        // 0 hands the shape back to the host's configured one, which is what
        // vim and neovim send on exit to restore the user's cursor.
        screen.feed(b"\x1b[0 q");
        assert_eq!(screen.grid().reported_cursor_shape(), None);
    }

    fn row_text(screen: &Screen, row: usize) -> String {
        let grid = screen.grid();
        let text: String = (0..grid.cols())
            .filter_map(|col| grid.cell(row, col))
            .filter(|cell| cell.width != CellWidth::Spacer)
            .map(|cell| cell.ch)
            .collect();
        text.trim_end().to_string()
    }

    #[test]
    fn test_dec_line_drawing_via_g0_and_via_shift_out() {
        let screen = feed(10, 1, b"\x1b(0lqqk\x1b(Bq");
        assert_eq!(row_text(&screen, 0), "┌──┐q", "ESC ( B returns G0 to ASCII");
        let screen = feed(10, 1, b"\x1b)0x\x0ex\x0fx");
        assert_eq!(
            row_text(&screen, 0),
            "x│x",
            "SO prints from G1, SI back from G0"
        );
    }

    #[test]
    fn test_rep_repeats_the_last_graphic_character() {
        assert_eq!(row_text(&feed(10, 1, b"ab\x1b[3b"), 0), "abbbb");
        // REP repeats what was drawn, not the byte that drew it.
        assert_eq!(row_text(&feed(10, 1, b"\x1b(0q\x1b(B\x1b[2b"), 0), "───");
        assert_eq!(
            row_text(&feed(10, 1, b"\x1b[3b"), 0),
            "",
            "nothing printed yet"
        );
    }

    #[test]
    fn test_hpa_hpr_and_vpr_move_the_cursor() {
        assert_eq!(feed(10, 3, b"\x1b[5`").grid().cursor(), (0, 4));
        assert_eq!(feed(10, 3, b"\x1b[2;2H\x1b[3a").grid().cursor(), (1, 4));
        assert_eq!(feed(10, 3, b"\x1b[1;3H\x1b[2e").grid().cursor(), (2, 2));
    }

    #[test]
    fn test_vertical_tab_and_form_feed_are_line_feeds() {
        let screen = feed(10, 3, b"ab\x0bc\x0cd");
        assert_eq!(row_text(&screen, 0), "ab");
        assert_eq!(row_text(&screen, 1), "  c");
        assert_eq!(row_text(&screen, 2), "   d");
    }

    #[test]
    fn test_nel_returns_to_column_zero_of_the_next_row() {
        let screen = feed(10, 3, b"ab\x1bEc");
        assert_eq!(row_text(&screen, 1), "c");
    }

    #[test]
    fn test_insert_mode_shifts_the_line_right() {
        let screen = feed(10, 1, b"abcd\x1b[1;2H\x1b[4hXY\x1b[4lZ");
        assert_eq!(row_text(&screen, 0), "aXYZcd");
    }

    #[test]
    fn test_autowrap_off_overwrites_the_last_column() {
        let screen = feed(5, 2, b"\x1b[?7labcdefg");
        assert_eq!(row_text(&screen, 0), "abcdg");
        assert_eq!(row_text(&screen, 1), "");
        // Turned back on, the next print past the edge wraps again.
        let screen = feed(5, 2, b"\x1b[?7labcdefg\x1b[?7hxy");
        assert_eq!(row_text(&screen, 1), "y");
    }

    #[test]
    fn test_xtsave_and_xtrestore_save_modes_instead_of_resetting_them() {
        let screen = feed(10, 3, b"\x1b[?1049h\x1b[?1049s");
        assert!(screen.grid().is_alt_screen());
        let screen = feed(10, 3, b"\x1b[?25s\x1b[?25l\x1b[?25r");
        assert!(screen.grid().cursor_visible());
    }

    #[test]
    fn test_sequences_with_foreign_intermediates_are_not_read_as_plain_ones() {
        // XTMODKEYS for another resource is not SGR bold + faint.
        let screen = feed(10, 1, b"\x1b[>1;2mX");
        let style = screen.grid().cell(0, 0).expect("cell").style;
        assert!(!style.bold && !style.faint);
        // DECALN is not DECRC.
        let screen = feed(10, 3, b"\x1b[2;3H\x1b7\x1b[1;1H\x1b#8");
        assert_eq!(screen.grid().cursor(), (0, 0));
        assert_eq!(row_text(&screen, 2), "EEEEEEEEEE");
    }

    #[test]
    fn test_ris_and_decstr_reset_from_the_byte_stream() {
        let screen = feed(10, 2, b"abc\x1b[?25l\x1bc");
        assert_eq!(row_text(&screen, 0), "");
        assert!(screen.grid().cursor_visible());
        let screen = feed(10, 2, b"\x1b[7m\x1b[?25l\x1b[!pX");
        assert!(!screen.grid().cell(0, 0).expect("cell").style.reversed);
        assert!(screen.grid().cursor_visible());
    }

    #[test]
    fn test_selective_erase_erases_like_its_plain_counterpart() {
        assert_eq!(row_text(&feed(10, 1, b"abc\x1b[?2J"), 0), "");
        assert_eq!(row_text(&feed(10, 1, b"abc\x1b[1;2H\x1b[?K"), 0), "a");
    }

    #[test]
    fn test_overwriting_half_a_wide_glyph_blanks_the_other_half() {
        let wide = "\u{4e2d}";
        let right = feed(10, 1, format!("{wide}\x1b[1;2Hx").as_bytes());
        assert_eq!(row_text(&right, 0), " x");
        let left = feed(10, 1, format!("{wide}\x1b[1;1Hx").as_bytes());
        assert_eq!(row_text(&left, 0), "x");
        let cell = left.grid().cell(0, 1).expect("cell");
        assert_eq!(cell.width, CellWidth::Single, "no orphan spacer");
    }

    #[test]
    fn test_cursor_up_and_down_stop_at_the_scroll_margins() {
        // Region rows 2..=3 (1-based); inside it, CUU/CUD stop at its edges.
        assert_eq!(
            feed(10, 4, b"\x1b[2;3r\x1b[3;1H\x1b[5A").grid().cursor(),
            (1, 0)
        );
        assert_eq!(
            feed(10, 4, b"\x1b[2;3r\x1b[2;1H\x1b[5B").grid().cursor(),
            (2, 0)
        );
        // Outside it, they reach the screen's edges.
        assert_eq!(
            feed(10, 4, b"\x1b[2;3r\x1b[4;1H\x1b[5A").grid().cursor(),
            (1, 0)
        );
        assert_eq!(
            feed(10, 4, b"\x1b[2;3r\x1b[1;1H\x1b[A").grid().cursor(),
            (0, 0)
        );
    }

    #[test]
    fn test_hts_and_tbc_from_the_byte_stream() {
        let screen = feed(20, 1, b"\x1b[3g\x1b[1;4H\x1bH\x1b[1;1H\tX");
        assert_eq!(row_text(&screen, 0), "   X");
    }

    #[test]
    fn test_every_sgr_attribute_round_trips_through_its_encoding() {
        // `Style::sgr_params` is what the dump and DECRQSS answer with, so a
        // style must read back as itself.
        let style = Style {
            background: Color::Rgb(RgbColor { r: 1, g: 2, b: 3 }),
            blink: true,
            bold: true,
            concealed: true,
            faint: true,
            foreground: Color::Indexed(196),
            italic: true,
            link: 0,
            overline: true,
            reversed: true,
            strikethrough: true,
            underline: UnderlineStyle::Curly,
            underline_color: Color::Indexed(4),
        };
        let screen = feed(4, 1, format!("\x1b[{}mZ", style.sgr_params()).as_bytes());
        assert_eq!(screen.grid().cell(0, 0).expect("cell").style, style);
        for underline in [UnderlineStyle::Single, UnderlineStyle::Double] {
            let style = Style {
                underline,
                ..Style::default()
            };
            let screen = feed(4, 1, format!("\x1b[{}mZ", style.sgr_params()).as_bytes());
            assert_eq!(screen.grid().cell(0, 0).expect("cell").style, style);
        }
    }

    #[test]
    fn test_sgr_off_codes_clear_the_newer_attributes() {
        let screen = feed(4, 1, b"\x1b[5;8;21;53;58;5;1m\x1b[25;28;24;55;59mZ");
        assert_eq!(
            screen.grid().cell(0, 0).expect("cell").style,
            Style::default()
        );
    }

    #[test]
    fn test_decrc_restores_the_pen_and_character_sets_decsc_saved() {
        // Save in red line drawing, draw something else, restore: the next
        // glyph is a red line again.
        let screen = feed(10, 2, b"\x1b[31m\x1b(0\x1b7\x1b[0m\x1b(B\x1b[2;5Hx\x1b8q");
        let cell = screen.grid().cell(0, 0).expect("cell");
        assert_eq!(cell.ch, '─');
        assert_eq!(cell.style.foreground, Color::Indexed(1));
    }

    #[test]
    fn test_decrc_with_nothing_saved_homes_with_a_fresh_pen() {
        let screen = feed(10, 3, b"\x1b[3;4H\x1b[1m\x1b(0\x1b8q");
        let cell = screen.grid().cell(0, 0).expect("cell");
        assert_eq!((cell.ch, cell.style.bold), ('q', false));
    }

    #[test]
    fn test_decrc_restores_origin_mode() {
        let screen = feed(10, 5, b"\x1b[2;4r\x1b[?6h\x1b7\x1b[?6l\x1b8\x1b[1;1H");
        assert_eq!(
            screen.grid().cursor(),
            (1, 0),
            "CUP is region-relative again"
        );
    }

    #[test]
    fn test_deckpam_and_deckpnm_switch_the_keypad_mode() {
        let screen = feed(10, 1, b"\x1b=");
        assert!(screen.grid().dec_mode(DecMode::AppKeypad));
        let screen = feed(10, 1, b"\x1b=\x1b>");
        assert!(!screen.grid().dec_mode(DecMode::AppKeypad));
    }
}

//! What the terminal reports about itself when a program asks: mode states
//! (DECRQM), the cursor position (DECXCPR), its size (XTWINOPS), its colors
//! (OSC 4 and 10/11/12), its settings (DECRQSS), and its terminfo capabilities
//! (XTGETTCAP). Each reply is built here from the grid and the host's
//! [`Presentation`]; the performer only routes the query.

use winter_render::{Theme, ThemeRgb};

use winter_core::{AnsiMode, CursorShape, DecMode, Grid, RgbColor};

use super::TERM_NAME;
use crate::mux::protocol::CellPixels;

// ========================================================================
// Constants
// ========================================================================

const CSI: &str = "\x1b[";
const OSC: &str = "\x1b]";
const DCS: &str = "\x1bP";
const ST: &str = "\x1b\\";
const BEL: &str = "\x07";
/// DECRQM `Pm`: the mode is set, reset, or not one this terminal knows.
const MODE_SET: u8 = 1;
const MODE_RESET: u8 = 2;
const MODE_UNKNOWN: u8 = 0;
const MODE_PERMANENTLY_SET: u8 = 3;
const MODE_PERMANENTLY_RESET: u8 = 4;
/// XTWINOPS queries this terminal answers, and the codes of their replies.
const WINOPS_TEXT_AREA_PX: u32 = 14;
const WINOPS_CELL_PX: u32 = 16;
const WINOPS_TEXT_AREA_CHARS: u32 = 18;
const WINOPS_SCREEN_CHARS: u32 = 19;
const WINOPS_TEXT_AREA_PX_REPLY: u16 = 4;
const WINOPS_CELL_PX_REPLY: u16 = 6;
const WINOPS_TEXT_AREA_CHARS_REPLY: u16 = 8;
const WINOPS_SCREEN_CHARS_REPLY: u16 = 9;
/// OSC numbers of the dynamic colors, in the order a multi-query walks them.
const OSC_FOREGROUND: u16 = 10;
const OSC_BACKGROUND: u16 = 11;
const OSC_CURSOR: u16 = 12;
/// OSC 4: query or set a palette color.
const OSC_PALETTE: u16 = 4;
/// OSC 104 and 110-112: reset the palette and the dynamic colors.
const OSC_PALETTE_RESET: u16 = 104;
const OSC_DYNAMIC_RESET_FIRST: u16 = 110;
const OSC_DYNAMIC_RESET_LAST: u16 = 112;
/// The field that asks for a color instead of setting one.
const COLOR_QUERY: &[u8] = b"?";
/// Colors in the 256-color palette.
const PALETTE_SIZE: usize = 256;
/// An 8-bit channel scaled to the 16 bits `rgb:` reports carry (0xff ->
/// 0xffff), as xterm does.
const CHANNEL_SCALE: u16 = 0x101;
/// DECRQSS settings this terminal reports.
const DECRQSS_SGR: &[u8] = b"m";
const DECRQSS_CURSOR_STYLE: &[u8] = b" q";
const DECRQSS_SCROLL_REGION: &[u8] = b"r";
/// DECSCUSR codes for the steady shapes, and for "the host's default".
const CURSOR_STYLE_DEFAULT: u8 = 0;
const CURSOR_STYLE_BLOCK: u8 = 2;
const CURSOR_STYLE_UNDERLINE: u8 = 4;
const CURSOR_STYLE_BAR: u8 = 6;
/// The color count XTGETTCAP reports for `Co` / `colors`.
const COLOR_COUNT: &str = "256";
/// Cell size assumed until the window reports the real one: close enough to
/// size an image band or answer a size query.
const DEFAULT_CELL_WIDTH_PX: f32 = 9.0;
const DEFAULT_CELL_HEIGHT_PX: f32 = 20.0;

// ========================================================================
// Data Structures
// ========================================================================

/// How the window shows a terminal, as far as programs may ask about it:
/// the cell size in pixels and the theme's colors.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Presentation {
    /// Height of one cell, in physical pixels.
    pub cell_height_px: f32,
    /// Width of one cell, in physical pixels.
    pub cell_width_px: f32,
    /// The colors the theme draws the terminal in.
    pub colors: ThemeColors,
}

/// The theme's colors as a program sees them: the defaults and the palette.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ThemeColors {
    /// The default background (OSC 11).
    pub background: RgbColor,
    /// The cursor's fill (OSC 12).
    pub cursor: RgbColor,
    /// The default foreground (OSC 10).
    pub foreground: RgbColor,
    /// The 256-color palette (OSC 4).
    pub palette: [RgbColor; PALETTE_SIZE],
}

/// A device control string this terminal acts on, chosen by its introducer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum DcsRequest {
    /// `DCS Pn q`: a Sixel image.
    Sixel,
    /// `DCS $ q Pt ST` (DECRQSS): report a setting.
    StatusString,
    /// `DCS + q Pt ST` (XTGETTCAP): report terminfo capabilities.
    Termcap,
}

// ========================================================================
// Presentation
// ========================================================================

impl Presentation {
    /// The cell size in whole pixels, as a PTY carries it.
    pub fn cell_pixels(&self) -> CellPixels {
        CellPixels::from_px(self.cell_width_px, self.cell_height_px)
    }
}

impl Default for Presentation {
    fn default() -> Self {
        Self {
            cell_height_px: DEFAULT_CELL_HEIGHT_PX,
            cell_width_px: DEFAULT_CELL_WIDTH_PX,
            colors: ThemeColors::from_theme(&Theme::default()),
        }
    }
}

// ========================================================================
// ThemeColors
// ========================================================================

impl ThemeColors {
    /// The colors `theme` draws a terminal in.
    pub fn from_theme(theme: &Theme) -> Self {
        let rgb = |color: ThemeRgb| RgbColor {
            r: color.r,
            g: color.g,
            b: color.b,
        };
        let palette = std::array::from_fn(|index| {
            let (r, g, b) = theme.palette_color(index as u8);
            RgbColor { r, g, b }
        });
        Self {
            background: rgb(theme.background),
            cursor: rgb(theme.cursor_bg),
            foreground: rgb(theme.foreground),
            palette,
        }
    }
}

// ========================================================================
// DcsRequest
// ========================================================================

impl DcsRequest {
    /// The request a DCS hook with `intermediates` and final byte `action`
    /// introduces, or `None` for one this terminal ignores.
    pub(super) fn from_hook(intermediates: &[u8], action: char) -> Option<Self> {
        match (intermediates, action) {
            ([], 'q') => Some(Self::Sixel),
            ([b'$'], 'q') => Some(Self::StatusString),
            ([b'+'], 'q') => Some(Self::Termcap),
            _ => None,
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The DECRQM reply for DEC private mode `code`.
pub(super) fn private_mode_report(grid: &Grid, code: u32) -> String {
    let state = match u16::try_from(code).ok().and_then(DecMode::from_code) {
        Some(mode) if mode.is_permanent() => match grid.dec_mode(mode) {
            true => MODE_PERMANENTLY_SET,
            false => MODE_PERMANENTLY_RESET,
        },
        Some(mode) => mode_state(grid.dec_mode(mode)),
        None => MODE_UNKNOWN,
    };
    format!("{CSI}?{code};{state}$y")
}

/// The DECRQM reply for ANSI mode `code`.
pub(super) fn ansi_mode_report(grid: &Grid, code: u32) -> String {
    let state = match u16::try_from(code).ok().and_then(AnsiMode::from_code) {
        Some(mode) => mode_state(grid.ansi_mode(mode)),
        None => MODE_UNKNOWN,
    };
    format!("{CSI}{code};{state}$y")
}

/// The cursor position report: CPR (`CSI r ; c R`) for `CSI 6 n`, or the
/// DEC form (`CSI ? r ; c R`) for `CSI ? 6 n`. Both 1-based.
pub(super) fn cursor_position_report(grid: &Grid, private: bool) -> String {
    let (row, col) = grid.cursor();
    let marker = if private { "?" } else { "" };
    format!("{CSI}{marker}{};{}R", row + 1, col + 1)
}

/// The XTWINOPS (`CSI Ps t`) reply for size query `op`, or `None` for an
/// operation this terminal does not answer. Window moves, resizes and the
/// title reports are deliberately left alone: a program has no business
/// driving the window, and echoing the title back is a known injection path.
pub(super) fn window_ops_report(
    grid: &Grid,
    presentation: &Presentation,
    op: u32,
) -> Option<String> {
    let (rows, cols) = (grid.rows(), grid.cols());
    let cell_h = presentation.cell_height_px.round() as usize;
    let cell_w = presentation.cell_width_px.round() as usize;
    let (code, first, second) = match op {
        WINOPS_TEXT_AREA_PX => (WINOPS_TEXT_AREA_PX_REPLY, rows * cell_h, cols * cell_w),
        WINOPS_CELL_PX => (WINOPS_CELL_PX_REPLY, cell_h, cell_w),
        WINOPS_TEXT_AREA_CHARS => (WINOPS_TEXT_AREA_CHARS_REPLY, rows, cols),
        WINOPS_SCREEN_CHARS => (WINOPS_SCREEN_CHARS_REPLY, rows, cols),
        _ => return None,
    };
    Some(format!("{CSI}{code};{first};{second}t"))
}

/// The replies to a color OSC, or `None` when `params` is not one: OSC 4
/// palette queries, OSC 10/11/12 dynamic-color queries, and the resets
/// (OSC 104, 110-112), which need no reply. Each reply ends the way the
/// query did. Fields that set a color instead of asking are ignored, and the
/// resets with them have nothing to undo: the theme is the user's.
pub(super) fn color_reports(
    params: &[&[u8]],
    colors: &ThemeColors,
    bell_terminated: bool,
) -> Option<Vec<String>> {
    let code = std::str::from_utf8(params.first()?)
        .ok()?
        .parse::<u16>()
        .ok()?;
    let terminator = if bell_terminated { BEL } else { ST };
    let fields = &params[1..];
    match code {
        OSC_PALETTE => Some(palette_reports(fields, colors, terminator)),
        OSC_FOREGROUND..=OSC_CURSOR => {
            Some(dynamic_color_reports(code, fields, colors, terminator))
        }
        OSC_PALETTE_RESET | OSC_DYNAMIC_RESET_FIRST..=OSC_DYNAMIC_RESET_LAST => Some(Vec::new()),
        _ => None,
    }
}

/// OSC 4's `index ; spec` pairs: one reply for each pair whose spec is `?`.
fn palette_reports(fields: &[&[u8]], colors: &ThemeColors, terminator: &str) -> Vec<String> {
    fields
        .chunks(2)
        .filter_map(|pair| match pair {
            [index, spec] if *spec == COLOR_QUERY => {
                let index = std::str::from_utf8(index).ok()?.parse::<u8>().ok()?;
                let color = colors.palette[usize::from(index)];
                Some(format!(
                    "{OSC}{OSC_PALETTE};{index};{}{terminator}",
                    rgb_spec(color)
                ))
            }
            _ => None,
        })
        .collect()
}

/// OSC 10/11/12 starting at `first`: each field asks for the next dynamic
/// color in turn (`OSC 10 ; ? ; ? ST` asks for the foreground and the
/// background), and the walk stops at the cursor color, the last of them.
fn dynamic_color_reports(
    first: u16,
    fields: &[&[u8]],
    colors: &ThemeColors,
    terminator: &str,
) -> Vec<String> {
    fields
        .iter()
        .zip(first..=OSC_CURSOR)
        .filter(|(field, _)| **field == COLOR_QUERY)
        .map(|(_, code)| {
            let color = match code {
                OSC_FOREGROUND => colors.foreground,
                OSC_BACKGROUND => colors.background,
                _ => colors.cursor,
            };
            format!("{OSC}{code};{}{terminator}", rgb_spec(color))
        })
        .collect()
}

/// The DECRQSS reply for the setting named by `request`: `DCS 1 $ r <value>
/// ST` for one this terminal reports, `DCS 0 $ r ST` otherwise.
pub(super) fn status_string_report(grid: &Grid, request: &[u8]) -> String {
    let value = match request {
        DECRQSS_SGR => Some(format!("{}m", grid.style().sgr_params())),
        DECRQSS_CURSOR_STYLE => Some(format!("{} q", cursor_style_code(grid))),
        DECRQSS_SCROLL_REGION => Some(format!(
            "{};{}r",
            grid.scroll_top() + 1,
            grid.scroll_bottom() + 1
        )),
        _ => None,
    };
    match value {
        Some(value) => format!("{DCS}1$r{value}{ST}"),
        None => format!("{DCS}0$r{ST}"),
    }
}

/// The XTGETTCAP replies to `request`, its hex-encoded capability names
/// separated by `;`: one `DCS 1 + r name=value ST` per capability this
/// terminal knows, one `DCS 0 + r name ST` per one it does not, so a program
/// waiting on each answer is never left hanging.
pub(super) fn termcap_reports(request: &[u8]) -> Vec<String> {
    request
        .split(|&byte| byte == b';')
        .filter(|name| !name.is_empty())
        .map(|hex_name| {
            // Only a name that decodes is ever written back. The request is
            // whatever the program printed, and the reply goes to the shell's
            // input: echoing undecodable bytes would let a file being `cat`ed
            // type a carriage return and a command (xterm's old DECRQSS bug).
            let Some(decoded) = hex_decode(hex_name) else {
                return format!("{DCS}0+r{ST}");
            };
            let hex_name = String::from_utf8_lossy(hex_name);
            match String::from_utf8(decoded)
                .ok()
                .as_deref()
                .and_then(termcap_value)
            {
                Some(value) => format!("{DCS}1+r{hex_name}={}{ST}", hex_encode(value)),
                None => format!("{DCS}0+r{hex_name}{ST}"),
            }
        })
        .collect()
}

fn mode_state(set: bool) -> u8 {
    if set {
        MODE_SET
    } else {
        MODE_RESET
    }
}

/// The DECSCUSR code for the cursor style the program set, or 0 when the
/// host's own applies.
fn cursor_style_code(grid: &Grid) -> u8 {
    match grid.reported_cursor_shape() {
        None => CURSOR_STYLE_DEFAULT,
        Some(CursorShape::Block) => CURSOR_STYLE_BLOCK,
        Some(CursorShape::Underline) => CURSOR_STYLE_UNDERLINE,
        Some(CursorShape::Bar) => CURSOR_STYLE_BAR,
    }
}

/// The terminfo value of capability `name` for the terminal Winter
/// advertises.
fn termcap_value(name: &str) -> Option<&'static str> {
    match name {
        "TN" | "name" => Some(TERM_NAME),
        "Co" | "colors" => Some(COLOR_COUNT),
        _ => None,
    }
}

/// `color` in the `rgb:rrrr/gggg/bbbb` form X11 color queries answer with.
fn rgb_spec(color: RgbColor) -> String {
    let scale = |channel: u8| u16::from(channel) * CHANNEL_SCALE;
    format!(
        "rgb:{:04x}/{:04x}/{:04x}",
        scale(color.r),
        scale(color.g),
        scale(color.b)
    )
}

fn hex_encode(text: &str) -> String {
    text.bytes().map(|byte| format!("{byte:02X}")).collect()
}

fn hex_decode(hex: &[u8]) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    hex.chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn colors() -> ThemeColors {
        ThemeColors {
            background: RgbColor {
                r: 0,
                g: 0x10,
                b: 0xff,
            },
            cursor: RgbColor { r: 1, g: 2, b: 3 },
            foreground: RgbColor {
                r: 0xff,
                g: 0xff,
                b: 0xff,
            },
            palette: [RgbColor { r: 0, g: 0, b: 0 }; PALETTE_SIZE],
        }
    }

    #[test]
    fn test_decrqm_reports_set_reset_and_unknown() {
        let mut grid = Grid::new(10, 3);
        assert_eq!(private_mode_report(&grid, 25), "\x1b[?25;1$y");
        grid.set_dec_mode(DecMode::AppCursorKeys, true);
        assert_eq!(private_mode_report(&grid, 1), "\x1b[?1;1$y");
        assert_eq!(private_mode_report(&grid, 1000), "\x1b[?1000;2$y");
        assert_eq!(private_mode_report(&grid, 9999), "\x1b[?9999;0$y");
        assert_eq!(
            private_mode_report(&grid, 2027),
            "\x1b[?2027;3$y",
            "permanently set"
        );
        assert_eq!(ansi_mode_report(&grid, 4), "\x1b[4;2$y");
        assert_eq!(ansi_mode_report(&grid, 20), "\x1b[20;0$y");
    }

    #[test]
    fn test_cursor_position_report_in_both_forms() {
        let mut grid = Grid::new(10, 5);
        grid.move_to(2, 4);
        assert_eq!(cursor_position_report(&grid, false), "\x1b[3;5R");
        assert_eq!(cursor_position_report(&grid, true), "\x1b[?3;5R");
    }

    #[test]
    fn test_window_ops_report_sizes_and_ignores_the_rest() {
        let grid = Grid::new(80, 24);
        let presentation = Presentation {
            cell_height_px: 18.4,
            cell_width_px: 9.0,
            colors: colors(),
        };
        assert_eq!(
            window_ops_report(&grid, &presentation, 16).as_deref(),
            Some("\x1b[6;18;9t")
        );
        assert_eq!(
            window_ops_report(&grid, &presentation, 14).as_deref(),
            Some("\x1b[4;432;720t")
        );
        assert_eq!(
            window_ops_report(&grid, &presentation, 18).as_deref(),
            Some("\x1b[8;24;80t")
        );
        assert_eq!(
            window_ops_report(&grid, &presentation, 21),
            None,
            "no title echo"
        );
    }

    #[test]
    fn test_color_queries_answer_in_kind_and_walk_consecutive_colors() {
        let colors = colors();
        assert_eq!(
            color_reports(&[b"11", b"?"], &colors, true),
            Some(vec!["\x1b]11;rgb:0000/1010/ffff\x07".to_string()])
        );
        assert_eq!(
            color_reports(&[b"10", b"?", b"?"], &colors, false),
            Some(vec![
                "\x1b]10;rgb:ffff/ffff/ffff\x1b\\".to_string(),
                "\x1b]11;rgb:0000/1010/ffff\x1b\\".to_string(),
            ])
        );
        let many = vec![b"?".as_slice(); 70_000];
        let mut flood = vec![b"12".as_slice()];
        flood.extend(many);
        assert_eq!(
            color_reports(&flood, &colors, true).map(|r| r.len()),
            Some(1),
            "the walk stops at the cursor color instead of counting past it"
        );
        assert_eq!(
            color_reports(&[b"11", b"#123456"], &colors, true),
            Some(Vec::new())
        );
        assert_eq!(
            color_reports(&[b"52", b"c", b"?"], &colors, true),
            None,
            "not a color OSC"
        );
    }

    #[test]
    fn test_palette_queries_answer_each_asked_index() {
        let mut colors = colors();
        colors.palette[1] = RgbColor {
            r: 0xff,
            g: 0,
            b: 0,
        };
        assert_eq!(
            color_reports(
                &[b"4", b"1", b"?", b"2", b"#00ff00", b"300", b"?"],
                &colors,
                true
            ),
            Some(vec!["\x1b]4;1;rgb:ffff/0000/0000\x07".to_string()]),
            "a set pair and an out-of-range index are skipped"
        );
        assert_eq!(color_reports(&[b"104"], &colors, true), Some(Vec::new()));
    }

    #[test]
    fn test_decrqss_reports_known_settings_and_rejects_others() {
        let mut grid = Grid::new(10, 5);
        grid.set_scroll_region(1, 3);
        assert_eq!(status_string_report(&grid, b"r"), "\x1bP1$r2;4r\x1b\\");
        assert_eq!(status_string_report(&grid, b" q"), "\x1bP1$r0 q\x1b\\");
        grid.set_cursor_shape(CursorShape::Bar);
        assert_eq!(status_string_report(&grid, b" q"), "\x1bP1$r6 q\x1b\\");
        assert_eq!(status_string_report(&grid, b"m"), "\x1bP1$r0m\x1b\\");
        assert_eq!(status_string_report(&grid, b"t"), "\x1bP0$r\x1b\\");
    }

    #[test]
    fn test_xtgettcap_answers_every_name_known_or_not() {
        // "TN" = 544E, "Co" = 436F, "zz" = 7A7A.
        let reports = termcap_reports(b"544E;436F;7A7A");
        assert_eq!(
            reports[0],
            format!("\x1bP1+r544E={}\x1b\\", hex_encode(TERM_NAME))
        );
        assert_eq!(reports[1], "\x1bP1+r436F=323536\x1b\\");
        assert_eq!(reports[2], "\x1bP0+r7A7A\x1b\\");
        // Malformed hex is answered without the name: it is never echoed.
        assert_eq!(
            termcap_reports(b"XYZ"),
            vec!["\x1bP0+r\x1b\\"],
            "malformed hex"
        );
    }

    #[test]
    fn test_dcs_requests_are_told_apart_by_their_intermediates() {
        // Regression: DECRQSS and XTGETTCAP share Sixel's final byte and were
        // decoded as images.
        assert_eq!(DcsRequest::from_hook(b"", 'q'), Some(DcsRequest::Sixel));
        assert_eq!(
            DcsRequest::from_hook(b"$", 'q'),
            Some(DcsRequest::StatusString)
        );
        assert_eq!(DcsRequest::from_hook(b"+", 'q'), Some(DcsRequest::Termcap));
        assert_eq!(DcsRequest::from_hook(b"", 'p'), None);
    }

    #[test]
    fn test_an_undecodable_termcap_request_is_never_echoed_into_the_input() {
        let replies = termcap_reports(b"\r\x15touch /tmp/pwned\r");
        assert_eq!(replies, vec![format!("{DCS}0+r{ST}")]);
        // A well-formed unknown name is still answered by name.
        assert_eq!(termcap_reports(b"7a7a"), vec![format!("{DCS}0+r7a7a{ST}")]);
    }
}

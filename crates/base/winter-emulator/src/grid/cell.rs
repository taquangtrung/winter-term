//! Cell contents and the value types describing how a cell is drawn.

// ========================================================================
// Constants
// ========================================================================

/// The SGR codes [`Style::sgr_params`] writes.
const SGR_RESET: u16 = 0;
const SGR_BOLD: u16 = 1;
const SGR_FAINT: u16 = 2;
const SGR_ITALIC: u16 = 3;
const SGR_UNDERLINE: u16 = 4;
const SGR_BLINK: u16 = 5;
const SGR_REVERSE: u16 = 7;
const SGR_CONCEAL: u16 = 8;
const SGR_STRIKETHROUGH: u16 = 9;
const SGR_FOREGROUND: u16 = 38;
const SGR_BACKGROUND: u16 = 48;
const SGR_OVERLINE: u16 = 53;
const SGR_UNDERLINE_COLOR: u16 = 58;
/// Extended-color selectors after 38/48/58.
const SGR_EXT_INDEXED: u16 = 5;
const SGR_EXT_RGB: u16 = 2;
/// The `4:n` sub-parameter for a single underline.
const SINGLE_UNDERLINE: u16 = 1;

// ========================================================================
// Data Structures
// ========================================================================

/// One screen cell: a character, its styling, and its display width role.
#[derive(Clone, Debug, PartialEq)]
pub struct Cell {
    /// The cell's base character. A blank cell holds `' '`.
    pub ch: char,
    /// Trailing codepoints that combine onto `ch` but can't be composed into a
    /// single `char`: emoji ZWJ sequences, variation selectors, skin-tone
    /// modifiers, and a paired second regional-indicator flag half. `None` in
    /// the common case.
    pub tail: Option<Box<str>>,
    /// Colors and attributes applied when this cell is drawn.
    pub style: Style,
    /// This cell's role in double-width layout.
    pub width: CellWidth,
}
/// A cell's role in double-width (CJK/emoji) layout.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CellWidth {
    /// A normal single-column cell.
    #[default]
    Single,
    /// The left half of a double-width character; `ch` holds the character.
    Wide,
    /// The right half of a double-width character: a placeholder with no glyph,
    /// skipped at render time so the wide glyph spans both columns.
    Spacer,
}
/// A cell's visual attributes.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Style {
    /// Cell background color.
    pub background: Color,
    /// SGR 5/6 (blink): the glyph blinks on the cursor's timer.
    pub blink: bool,
    /// SGR 1.
    pub bold: bool,
    /// SGR 8 (conceal): the glyph is not drawn, though the cell keeps it for
    /// copying, as a terminal shows a typed password.
    pub concealed: bool,
    /// SGR 2 (faint/dim): the glyph is blended toward its background at render time.
    pub faint: bool,
    /// Glyph color.
    pub foreground: Color,
    /// SGR 3.
    pub italic: bool,
    /// Intern ID into the grid's link table; 0 means no hyperlink.
    pub link: u16,
    /// SGR 53: a line along the cell's top edge.
    pub overline: bool,
    /// SGR 7 (reverse video): foreground and background are swapped at render time.
    pub reversed: bool,
    /// SGR 9.
    pub strikethrough: bool,
    /// SGR 4, `4:n` and 21: whether and how the cell is underlined.
    pub underline: UnderlineStyle,
    /// SGR 58: the underline's color. `Default` draws it in the glyph's own.
    pub underline_color: Color,
}
/// How a cell is underlined (SGR `4:n`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UnderlineStyle {
    /// A wavy line (`4:3`), which spell checkers and LSP clients use.
    Curly,
    /// A dashed line (`4:5`).
    Dashed,
    /// A dotted line (`4:4`).
    Dotted,
    /// Two lines (`4:2`, or SGR 21).
    Double,
    /// Not underlined (`4:0`, SGR 24).
    #[default]
    None,
    /// One line (SGR 4, `4:1`).
    Single,
}
/// A foreground or background color.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Color {
    /// The terminal's default fg/bg.
    #[default]
    Default,
    /// A 256-color palette index.
    Indexed(u8),
    /// A 24-bit true color.
    Rgb(RgbColor),
}
/// A 24-bit color.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RgbColor {
    /// Red channel, 0-255.
    pub r: u8,
    /// Green channel, 0-255.
    pub g: u8,
    /// Blue channel, 0-255.
    pub b: u8,
}
/// Which region an erase affects, relative to the cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EraseMode {
    /// From the start of the region up to and including the cursor.
    ToStart,
    /// From the cursor to the end of the region.
    ToEnd,
    /// The whole region.
    Whole,
}
/// How the cursor is drawn. Configurable per interaction mode.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum CursorShape {
    /// A filled cell-sized rectangle.
    #[default]
    Block,
    /// A line along the cell's bottom edge.
    Underline,
    /// A vertical line at the cell's left edge.
    Bar,
}
// ========================================================================
// Style
// ========================================================================

impl Style {
    /// The SGR parameters that turn a reset pen into this one, starting with
    /// the reset itself (`0;1;4:3;38;5;196`). The hyperlink is not a pen
    /// attribute and is left out.
    pub fn sgr_params(&self) -> String {
        let mut codes = vec![SGR_RESET.to_string()];
        let flags = [
            (self.bold, SGR_BOLD),
            (self.faint, SGR_FAINT),
            (self.italic, SGR_ITALIC),
            (self.blink, SGR_BLINK),
            (self.reversed, SGR_REVERSE),
            (self.concealed, SGR_CONCEAL),
            (self.strikethrough, SGR_STRIKETHROUGH),
            (self.overline, SGR_OVERLINE),
        ];
        codes.extend(
            flags
                .iter()
                .filter(|(on, _)| *on)
                .map(|(_, code)| code.to_string()),
        );
        // Plain `4` for a single underline, understood everywhere; the
        // colon form only for the styles that need it.
        match self.underline.sgr_subparam() {
            None => {}
            Some(SINGLE_UNDERLINE) => codes.push(SGR_UNDERLINE.to_string()),
            Some(style) => codes.push(format!("{SGR_UNDERLINE}:{style}")),
        }
        push_color(&mut codes, self.foreground, SGR_FOREGROUND);
        push_color(&mut codes, self.background, SGR_BACKGROUND);
        push_color(&mut codes, self.underline_color, SGR_UNDERLINE_COLOR);
        codes.join(";")
    }
}

// ========================================================================
// UnderlineStyle
// ========================================================================

impl UnderlineStyle {
    /// The style a `4:n` sub-parameter selects. An unknown style still
    /// underlines, singly, rather than dropping the program's request.
    pub fn from_sgr_subparam(style: u16) -> Self {
        match style {
            0 => Self::None,
            2 => Self::Double,
            3 => Self::Curly,
            4 => Self::Dotted,
            5 => Self::Dashed,
            _ => Self::Single,
        }
    }

    /// The `4:n` sub-parameter that selects this style, or `None` for no
    /// underline.
    fn sgr_subparam(self) -> Option<u16> {
        match self {
            Self::Curly => Some(3),
            Self::Dashed => Some(5),
            Self::Dotted => Some(4),
            Self::Double => Some(2),
            Self::None => None,
            Self::Single => Some(SINGLE_UNDERLINE),
        }
    }

    /// Whether any underline is drawn.
    pub fn is_drawn(self) -> bool {
        self != Self::None
    }
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            ch: ' ',
            tail: None,
            style: Style::default(),
            width: CellWidth::Single,
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Append extended color `color` under `base` (38, 48, 58); the default
/// color needs no code, the reset already chose it.
fn push_color(codes: &mut Vec<String>, color: Color, base: u16) {
    match color {
        Color::Default => {}
        Color::Indexed(index) => codes.push(format!("{base};{SGR_EXT_INDEXED};{index}")),
        Color::Rgb(rgb) => codes.push(format!(
            "{base};{SGR_EXT_RGB};{};{};{}",
            rgb.r, rgb.g, rgb.b
        )),
    }
}

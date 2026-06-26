//! Cell contents and the value types describing how a cell is drawn.

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
    /// SGR 1.
    pub bold: bool,
    /// Glyph color.
    pub foreground: Color,
    /// SGR 3.
    pub italic: bool,
    /// Intern ID into the grid's link table; 0 means no hyperlink.
    pub link: u16,
    /// SGR 7 (reverse video): foreground and background are swapped at render time.
    pub reversed: bool,
    /// SGR 4.
    pub underline: bool,
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
impl CursorShape {
    /// Interpret a `cursor` config value (`"block"`/`"bar"`/`"underline"`).
    /// Common synonyms (`"beam"`, `"underscore"`) are accepted; unknown values
    /// fall back to `Block` so a typo never produces a missing cursor.
    pub fn from_value(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "bar" | "beam" | "line" => Self::Bar,
            "underline" | "underscore" => Self::Underline,
            _ => Self::Block,
        }
    }

    /// The canonical config value for this shape (round-trips through
    /// [`Self::from_value`]).
    pub fn as_value(&self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bar => "bar",
            Self::Underline => "underline",
        }
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

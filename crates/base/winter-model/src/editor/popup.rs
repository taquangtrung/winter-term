//! What an editor draws over its rows: completion, info cards, toolbars.

use crate::paint::Style;

// ========================================================================
// Data Structures
// ========================================================================

/// What an editor has open over its rows, for the window to draw there.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Popups {
    /// The card over the word the pointer or the cursor is on.
    pub card: Option<InfoCard>,
    /// The completion popup at the cursor.
    pub completion: Option<CompletionPopup>,
    /// The drop-down list over the pane.
    pub drop_list: Option<DropList>,
}

/// The completion popup an editor has open at its cursor, for the window to
/// draw under it.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionPopup {
    /// What it offers, the best first.
    pub items: Vec<CompletionItem>,
    /// Which entry is chosen.
    pub selected: usize,
    /// How many characters before the cursor the entries would replace, so
    /// the popup lines up under the word being completed.
    pub typed: usize,
}

/// One entry of a completion popup.
#[derive(Clone, Debug, PartialEq)]
pub struct CompletionItem {
    /// More about it, drawn dim beside it.
    pub detail: Option<String>,
    /// The letter its kind is marked with.
    pub kind: char,
    /// What it reads as.
    pub label: String,
    /// The back-end that offered it.
    pub source: &'static str,
}

/// A card of text an editor shows over its rows: what a name is, or the
/// problem a server found there.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct InfoCard {
    /// The cell it hangs from, as a row and a column of the pane, for one
    /// the pointer asked for; `None` hangs it from the cursor.
    pub cell: Option<(usize, usize)>,
    /// What kind of thing it is about, said apart at its foot: the problems'
    /// kinds and what found them. `None` for a card with no foot.
    pub footer: Option<String>,
    /// Per line of `text`, the style of each char, for a card showing code
    /// in its syntax colors: empty for plain text, and a char past its
    /// line's styles is drawn plain.
    pub styles: Vec<Vec<Style>>,
    /// What it says, one line per line.
    pub text: String,
}

/// A bar an editor has drawn over its top rows in the window's own font, as
/// the status bar is, rather than painted into the rows themselves: the rows
/// it takes, which the editor leaves blank under it, and its items.
#[derive(Clone, Debug, PartialEq)]
pub struct Toolbar {
    /// The items from the left edge.
    pub left: Vec<crate::view::ToolbarItem>,
    /// The items against the right edge.
    pub right: Vec<crate::view::ToolbarItem>,
    /// How many of the pane's top rows it covers.
    pub rows: usize,
}

/// A drop-down list an editor has open over its pane, lying on its cells:
/// the PDF viewer's outline under its toolbar. A click on it comes to the
/// editor as one on the cell under it, as every click does.
#[derive(Clone, Debug, PartialEq)]
pub struct DropList {
    /// The pane cell its top-left corner lies on.
    pub cell: (usize, usize),
    /// How many cells wide it is.
    pub cols: usize,
    /// Its rows, one to a cell row.
    pub rows: Vec<crate::view::DropListRow>,
    /// Which row is chosen, if any shows.
    pub selected: Option<usize>,
}

/// Token inspection details under the cursor.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenInspectData {
    /// The character under the cursor and its codepoint.
    pub character: String,
    /// The scope selector rule that matched the token's color.
    pub color_rule: Option<String>,
    /// The hex color string if defined by the matching rule.
    pub foreground_hex: Option<String>,
    /// The syntax grammar name.
    pub language: String,
    /// Human-readable 1-based line and column in the file.
    pub position: String,
    /// The TextMate scope stack from root to leaf.
    pub scopes: Vec<String>,
    /// The font style (e.g. bold, italic).
    pub style: Option<String>,
}

impl TokenInspectData {
    /// Format a character with its codepoint (e.g. `'a' (U+0061)`).
    pub fn format_char(ch: char) -> String {
        format!("'{}' (U+{:04X})", ch.escape_debug(), ch as u32)
    }

    /// Format a character with optional combining tail codepoints.
    pub fn format_char_with_tail(ch: char, tail: Option<&str>) -> String {
        if ch == ' ' || ch == '\0' {
            format!("' ' (U+{:04X})", if ch == '\0' { 0 } else { 0x20 })
        } else if let Some(tail) = tail {
            format!(
                "'{}{}' (U+{:04X})",
                ch.escape_debug(),
                tail.escape_debug(),
                ch as u32
            )
        } else {
            Self::format_char(ch)
        }
    }

    /// Format a character, or newline if absent.
    pub fn format_char_or_newline(ch: Option<char>) -> String {
        match ch {
            Some(c) => Self::format_char(c),
            None => "\\n (U+000A)".to_string(),
        }
    }

    /// Human-readable 1-based line and column in the file or listing.
    pub fn format_position(row: usize, col: usize) -> String {
        format!("ln {}, col {}", row + 1, col + 1)
    }
}

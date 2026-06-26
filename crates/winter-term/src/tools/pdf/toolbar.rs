//! The row over the pages: the section being read, which opens the outline,
//! then the zoom, the page, and the color inversion, each a button.
//!
//! Painted as text on the pane's top row, with every button's columns kept
//! so a click on the row can be told which one it landed on.

use std::ops::Range;

use crate::model::paint::{Row, Span, Style};

// ========================================================================
// Constants
// ========================================================================

/// The marks the buttons are drawn with: plain text, so any font has them.
const OUTLINE_MARK: &str = "≡";
const OPEN_MARK: &str = "▾";
const ZOOM_OUT_MARK: &str = "−";
const ZOOM_IN_MARK: &str = "+";
const PREVIOUS_MARK: &str = "‹";
const NEXT_MARK: &str = "›";
const INVERT_MARK: &str = "◐";

/// What the outline button says when the reading spot is ahead of every
/// section, and when the document lists none.
const NO_SECTION: &str = "Outline";
const NO_OUTLINE: &str = "No outline";

/// Between two groups of buttons.
const SEPARATOR: &str = "│";

/// What a title cut short to fit ends with.
const ELLIPSIS: char = '…';

// ========================================================================
// Data Structures
// ========================================================================

/// A button on the bar.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Button {
    /// Invert the pages' colors, or put them back.
    Invert,
    /// The page after the one being read.
    NextPage,
    /// Open or close the outline.
    Outline,
    /// Ask which page to go to.
    Page,
    /// The page before the one being read.
    PreviousPage,
    /// Ask for a zoom.
    Zoom,
    ZoomIn,
    ZoomOut,
}

/// What the bar shows.
#[derive(Clone, Debug, PartialEq)]
pub struct State<'a> {
    /// Whether the document lists any sections.
    pub has_outline: bool,
    /// Whether the pages are drawn inverted.
    pub inverted: bool,
    /// Whether the outline is open under the bar.
    pub outline_open: bool,
    /// The page being read, from one, and how many there are: none while
    /// the document is still opening.
    pub page: Option<(usize, usize)>,
    /// The section the reading spot is in.
    pub section: Option<&'a str>,
    /// The zoom, in percent of a pixel a point.
    pub zoom: u32,
}

/// The bar, painted: its spans, and the columns each button takes.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Bar {
    buttons: Vec<(Range<usize>, Button)>,
    pub row: Row,
}

/// A run of the bar while it is laid out: its text, how it is styled, and
/// the button it is part of, if any.
struct Piece {
    button: Option<Button>,
    style: Style,
    text: String,
}

// ========================================================================
// Bar
// ========================================================================

impl Bar {
    /// The bar for `state`, `cols` wide: the buttons on the right, and the
    /// section on the left in what room they leave, cut short to fit.
    pub fn new(state: &State, cols: usize) -> Self {
        let right = right_pieces(state);
        let right_width: usize = right.iter().map(|piece| width(&piece.text)).sum();
        let room = cols.saturating_sub(right_width);
        let mut pieces = left_pieces(state, room);
        let left_width: usize = pieces.iter().map(|piece| width(&piece.text)).sum();
        pieces.push(Piece {
            button: None,
            style: Style::Normal,
            text: " ".repeat(room.saturating_sub(left_width)),
        });
        pieces.extend(right);
        let mut bar = Self::default();
        let mut col = 0;
        for piece in pieces {
            let end = col + width(&piece.text);
            if let Some(button) = piece.button {
                bar.buttons.push((col..end, button));
            }
            bar.row.push(Span::new(piece.style, piece.text));
            col = end;
        }
        bar
    }

    /// The button at column `col`, if one is drawn there.
    pub fn button_at(&self, col: usize) -> Option<Button> {
        self.buttons
            .iter()
            .find(|(cols, _)| cols.contains(&col))
            .map(|(_, button)| *button)
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The outline button: the section being read, cut short to `room` less
/// the marks around it.
fn left_pieces(state: &State, room: usize) -> Vec<Piece> {
    let title = match (state.has_outline, state.section) {
        (false, _) => NO_OUTLINE,
        (true, Some(section)) => section,
        (true, None) => NO_SECTION,
    };
    let style = match (state.has_outline, state.outline_open) {
        (false, _) => Style::Dim,
        (true, true) => Style::HeaderAccent,
        (true, false) => Style::Header,
    };
    let frame = format!(" {OUTLINE_MARK}  {OPEN_MARK} ");
    let title = truncate(title, room.saturating_sub(width(&frame) + 1));
    let button = state.has_outline.then_some(Button::Outline);
    vec![Piece {
        button,
        style,
        text: format!(" {OUTLINE_MARK} {title} {OPEN_MARK} "),
    }]
}

/// The zoom, the page, and the inversion, each group set off by a line.
fn right_pieces(state: &State) -> Vec<Piece> {
    let button = |button: Button, style: Style, text: String| Piece {
        button: Some(button),
        style,
        text,
    };
    let separator = || Piece {
        button: None,
        style: Style::Dim,
        text: SEPARATOR.to_string(),
    };
    let mut pieces = vec![
        button(Button::ZoomOut, Style::Header, format!(" {ZOOM_OUT_MARK} ")),
        button(Button::Zoom, Style::Normal, format!("{}%", state.zoom)),
        button(Button::ZoomIn, Style::Header, format!(" {ZOOM_IN_MARK} ")),
        separator(),
    ];
    if let Some((page, pages)) = state.page {
        pieces.extend([
            button(Button::PreviousPage, Style::Header, format!(" {PREVIOUS_MARK} ")),
            button(Button::Page, Style::Normal, format!("{page} / {pages}")),
            button(Button::NextPage, Style::Header, format!(" {NEXT_MARK} ")),
            separator(),
        ]);
    }
    let invert_style = match state.inverted {
        true => Style::Accent,
        false => Style::Header,
    };
    pieces.push(button(Button::Invert, invert_style, format!(" {INVERT_MARK} ")));
    pieces
}

/// How many columns `text` takes, one a character.
fn width(text: &str) -> usize {
    text.chars().count()
}

/// `text`, cut short with an ellipsis to fit in `room` columns.
pub fn truncate(text: &str, room: usize) -> String {
    if width(text) <= room {
        return text.to_string();
    }
    if room == 0 {
        return String::new();
    }
    let mut cut: String = text.chars().take(room - 1).collect();
    cut.push(ELLIPSIS);
    cut
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> State<'static> {
        State {
            has_outline: true,
            inverted: false,
            outline_open: false,
            page: Some((3, 12)),
            section: Some("2.1 Ethereum Virtual Machine"),
            zoom: 125,
        }
    }

    fn text(bar: &Bar) -> String {
        bar.row.iter().map(|span| span.text.as_str()).collect()
    }

    /// The column `needle` starts at in the bar's text.
    fn col_of(bar: &Bar, needle: &str) -> usize {
        let text = text(bar);
        let byte = text.find(needle).unwrap();
        text[..byte].chars().count()
    }

    #[test]
    fn test_the_bar_fills_the_row_exactly() {
        let bar = Bar::new(&state(), 80);
        assert_eq!(width(&text(&bar)), 80);
        assert!(text(&bar).ends_with("3 / 12 › │ ◐ "), "{}", text(&bar));
    }

    #[test]
    fn test_each_button_is_found_under_its_own_columns() {
        let bar = Bar::new(&state(), 80);
        assert_eq!(bar.button_at(col_of(&bar, "−")), Some(Button::ZoomOut));
        assert_eq!(bar.button_at(col_of(&bar, "125%")), Some(Button::Zoom));
        assert_eq!(bar.button_at(col_of(&bar, "+")), Some(Button::ZoomIn));
        assert_eq!(bar.button_at(col_of(&bar, "‹")), Some(Button::PreviousPage));
        assert_eq!(bar.button_at(col_of(&bar, "3 / 12")), Some(Button::Page));
        assert_eq!(bar.button_at(col_of(&bar, "›")), Some(Button::NextPage));
        assert_eq!(bar.button_at(col_of(&bar, "◐")), Some(Button::Invert));
        assert_eq!(bar.button_at(col_of(&bar, "Ethereum")), Some(Button::Outline));
        assert_eq!(bar.button_at(col_of(&bar, "│")), None, "a separator is no button");
    }

    #[test]
    fn test_a_long_section_is_cut_short_to_leave_the_buttons_whole() {
        let bar = Bar::new(&state(), 40);
        let text = text(&bar);
        assert_eq!(width(&text), 40);
        assert!(text.contains('…'), "{text}");
        assert!(text.ends_with("3 / 12 › │ ◐ "), "{text}");
    }

    #[test]
    fn test_a_document_with_no_outline_has_no_outline_button() {
        let bar = Bar::new(
            &State {
                has_outline: false,
                section: None,
                ..state()
            },
            80,
        );
        assert!(text(&bar).contains(NO_OUTLINE));
        assert_eq!(bar.button_at(col_of(&bar, NO_OUTLINE)), None);
    }
}

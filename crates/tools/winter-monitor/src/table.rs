//! The table every list monitor draws: fixed-width cells and the last one
//! taking the room left, a heading cell that sorts on a click, and text cut
//! to fit a cell.

use misty::layout::Align;
use misty::view::{Extent, Space, Text, column, row, text};
use misty::{Element, ThemeToken};

// ========================================================================
// Constants
// ========================================================================

/// The mark on text too long for its cell.
const ELLIPSIS: char = '\u{2026}';

/// The text columns at each side of a row, inside the mark the cursor's
/// row takes; what lines up with the table's text pads by as much.
pub const ROW_PADDING_COLS: f32 = 1.0;

// ========================================================================
// Functions
// ========================================================================

/// A cell holding `content`: `width` text columns wide and a column more for
/// the gap after it, or the room the others leave.
pub fn cell<'a, A: 'static>(
    content: Text<'a>,
    width: Option<usize>,
    right: bool,
) -> Element<'a, A> {
    let shown = match right {
        true => column([content.into()]).align(Align::End),
        false => column([content.into()]).align(Align::Start),
    };
    match width {
        Some(width) => shown.width(Extent::Cells((width + 1) as f32)).shrink(0.0),
        None => shown.grow(1.0),
    }
}

/// A row of the table: its cells side by side, a text row tall.
pub fn table_row<A: 'static>(cells: Vec<Element<'_, A>>) -> Element<'_, A> {
    row(cells)
        .gap(Space::Cells(0.0))
        .align(Align::Start)
        .padding(ROW_PADDING_COLS, 0.0)
}

/// A column heading, `label` under `arrow` where the table is sorted by it,
/// saying `about` and sorting by it on a click through `sort`.
pub fn heading<'a, A: 'static>(
    label: &str,
    about: &str,
    arrow: Option<char>,
    width: Option<usize>,
    right: bool,
    sort: impl Fn(&mut A) + 'static,
) -> Element<'a, A> {
    let shown = match arrow {
        Some(arrow) => text(format!("{arrow}{label}"))
            .color(ThemeToken::Accent)
            .proportional(),
        None => text(label.to_string()).muted().proportional(),
    };
    cell(shown, width, right)
        .tooltip(format!("{about}; click to sort by it"))
        .on_press(move |editor: &mut A, _| sort(editor))
}

/// A table row the cursor is on, marked, or another, lit under the pointer.
pub fn marked<A: 'static>(line: Element<'_, A>, selected: bool) -> Element<'_, A> {
    match selected {
        true => line
            .background(ThemeToken::CursorLine)
            .border(ThemeToken::Accent)
            .corners(misty::view::Corners::Control),
        false => line.hover_background(ThemeToken::CursorLine),
    }
}

/// `text` cut to `width` cells, with a mark where it was cut.
pub fn fit(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    match width {
        0 => String::new(),
        _ => format!("{kept}{ELLIPSIS}"),
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_text_too_long_for_its_cell_is_cut_with_a_mark() {
        assert_eq!(fit("firefox", 7), "firefox");
        assert_eq!(fit("firefox-bin", 7), "firefo\u{2026}");
        assert_eq!(fit("firefox", 0), "");
    }
}

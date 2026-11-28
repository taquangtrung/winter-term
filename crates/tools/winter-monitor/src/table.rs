//! The table every list monitor draws: fixed-width cells and the last one
//! taking the room left, a heading cell that sorts on a click, and text cut
//! to fit a cell.

use misty::layout::Align;
use misty::view::{Extent, Page, Space, Text, column, row, text, untitled_page};
use misty::{Element, ThemeToken};
use winter_model::editor::{Header, HeaderPart, HeaderSegment, header_view};
use winter_model::view::ToolbarTone;

// ========================================================================
// Constants
// ========================================================================

/// The mark on text too long for its cell.
const ELLIPSIS: char = '\u{2026}';

/// The text columns at each side of a row, inside the mark the cursor's
/// row takes; what lines up with the table's text pads by as much.
pub const ROW_PADDING_COLS: f32 = 1.0;

/// The rows above and below a footer's text, which a [`summary_page`]
/// leaves to the footer.
pub const FOOTER_PADDING_ROWS: f32 = 0.5;

// ========================================================================
// Data Structures
// ========================================================================

/// One part of a monitor's summary, on its header: what it says and in
/// what tone, and for a part a press does something with, what it runs and
/// the tooltip saying so.
#[derive(Clone, Debug, PartialEq)]
pub struct SummaryPart<A> {
    /// What a press runs, and the tooltip saying what the part means and
    /// which key runs it too; `None` for a part with nothing to press.
    pub press: Option<(A, String)>,
    /// What it says.
    pub text: String,
    /// How it is drawn.
    pub tone: ToolbarTone,
}

// ========================================================================
// SummaryPart
// ========================================================================

impl<A> SummaryPart<A> {
    /// A part saying `text` in `tone`, with nothing to press.
    pub fn label(text: impl Into<String>, tone: ToolbarTone) -> Self {
        Self {
            press: None,
            text: text.into(),
            tone,
        }
    }

    /// A part saying `text` in `tone` that runs `act` on a press, its
    /// tooltip `about`.
    pub fn button(text: impl Into<String>, tone: ToolbarTone, act: A, about: String) -> Self {
        Self {
            press: Some((act, about)),
            text: text.into(),
            tone,
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// An untitled page for `monitor`, its header the summary `summary` gives,
/// a press on a part of it running `run` with what the part runs: no header
/// while the summary is empty. The header is as tall as Winter's, so a
/// footer given it pads itself by [`FOOTER_PADDING_ROWS`].
pub fn summary_page<'a, A: 'static, C: 'static>(
    monitor: &A,
    summary: fn(&A) -> Vec<SummaryPart<C>>,
    run: fn(&mut A, C),
) -> Page<'a, A> {
    let parts = summary(monitor);
    let shown = untitled_page()
        .content_padding(Space::Cells(0.0))
        .band_padding(Space::Cells(0.0));
    if parts.is_empty() {
        return shown;
    }
    let pressed = move |monitor: &mut A, action: u16| {
        if let Some(act) = summary_act(summary(monitor), action) {
            run(monitor, act);
        }
    };
    let header = summary_header(&parts);
    shown.header(header_view(&header, |_: &mut A, _| {}, pressed))
}

/// `parts` as a header's summary, each part with a press told back as its
/// place in `parts`.
fn summary_header<A>(parts: &[SummaryPart<A>]) -> Header {
    let shown = parts
        .iter()
        .enumerate()
        .map(|(at, part)| match (&part.press, u16::try_from(at)) {
            (Some((_, about)), Ok(action)) => {
                HeaderPart::button(part.text.clone(), part.tone, action, about.clone())
            }
            _ => HeaderPart::label(part.text.clone(), part.tone),
        })
        .collect();
    Header::new(vec![HeaderSegment::Summary(shown)])
}

/// What the part of `parts` told back as `action` runs.
fn summary_act<A>(parts: Vec<SummaryPart<A>>, action: u16) -> Option<A> {
    let (act, _) = parts.into_iter().nth(usize::from(action))?.press?;
    Some(act)
}

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

/// A row of the table: its cells side by side, a text row tall with the
/// theme's row padding above and below, as the rows of misty's own tables
/// have.
pub fn table_row<A: 'static>(cells: Vec<Element<'_, A>>) -> Element<'_, A> {
    row(cells)
        .gap(Space::Cells(0.0))
        .align(Align::Start)
        .padding(ROW_PADDING_COLS, 0.0)
        .row_padding()
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
        Some(arrow) => text(format!("{arrow}{label}")).color(ThemeToken::Accent),
        None => text(label.to_string()).muted(),
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
    fn test_a_summary_part_is_pressed_by_the_action_it_is_drawn_with() {
        let parts = vec![
            SummaryPart::label("12 processes", ToolbarTone::Normal),
            SummaryPart::button(
                "tree",
                ToolbarTone::Dim,
                'v',
                "Listed as a tree".to_string(),
            ),
        ];
        let header = summary_header(&parts);
        let actions: Vec<Option<u16>> = header.summary().map(|part| part.action).collect();
        assert_eq!(actions, [None, Some(1)], "a label is no button");
        assert_eq!(
            header.part(1).and_then(|part| part.tooltip.as_deref()),
            Some("Listed as a tree")
        );
        assert_eq!(summary_act(parts.clone(), 1), Some('v'));
        assert_eq!(summary_act(parts.clone(), 0), None);
        assert_eq!(summary_act(parts, 9), None);
    }

    #[test]
    fn test_text_too_long_for_its_cell_is_cut_with_a_mark() {
        assert_eq!(fit("firefox", 7), "firefox");
        assert_eq!(fit("firefox-bin", 7), "firefo\u{2026}");
        assert_eq!(fit("firefox", 0), "");
    }
}

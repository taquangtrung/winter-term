//! The popups an editor opens over its pane, drawn by misty: the completion
//! list under the word being completed, the info card over the word it is
//! about, and the drop list under a breadcrumb. The frame works out what
//! each shows and where; misty draws them over the window.

use misty::color::Rgba;
use misty::text::{Span, StyledText, TextColor};
use misty::{CompletionList, CompletionRow, DropList, DropRow, Frame, InfoCard, Rect, Widget};
use winter_render::{CompletionView, DropListView, InfoSpan, InfoView};

use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Constants
// ========================================================================

/// The cells one level of a drop list's indent takes, as misty indents.
const INDENT_CELLS: usize = 2;

// ========================================================================
// Data Structures
// ========================================================================

/// One pane's popups, as the last frame worked them out.
#[derive(Clone, Debug, Default)]
pub(in crate::window) struct PanePopups {
    /// The completion list, in the pane the keys go to.
    pub(in crate::window) completion: Option<CompletionView>,
    /// The drop list.
    pub(in crate::window) drop_list: Option<DropListView>,
    /// The info card.
    pub(in crate::window) info: Option<InfoView>,
}

// ========================================================================
// App: the popups misty draws
// ========================================================================

impl App {
    /// Draw each pane's popups over `frame`.
    pub(in crate::window) fn draw_pane_popups(&mut self, frame: &mut Frame) {
        if self.chrome.pane_popups.is_empty() {
            return;
        }
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&winter_render::Theme::dark()),
        };
        let bounds = frame.bounds();
        for popups in &self.chrome.pane_popups {
            if let Some(view) = &popups.drop_list {
                let (left, top) = view.origin;
                let (cell_width, cell_height) = view.cell;
                let area = Rect::new(
                    left,
                    top,
                    view.cols as f32 * cell_width,
                    view.rows.len() as f32 * cell_height,
                );
                let mut list = DropList::new(view.rows.iter().map(drop_row).collect());
                list.set_selected(view.selected);
                list.draw(frame, area, &theme);
            }
            if let Some(view) = &popups.info {
                let lines = match view.spans.is_empty() {
                    true => view.text.lines().map(plain_line).collect(),
                    false => view.spans.iter().map(|line| styled_line(line)).collect(),
                };
                let mut card = InfoCard::new(lines, anchor(view.anchor));
                if let Some(footer) = &view.footer {
                    card = card.with_footer(footer.as_str());
                }
                card.draw(frame, bounds, &theme);
            }
            if let Some(view) = &popups.completion {
                let rows = view.rows.iter().map(completion_row).collect();
                let mut list = CompletionList::new(rows, anchor(view.anchor));
                list.set_selected(Some(view.selected));
                list.draw(frame, bounds, &theme);
            }
        }
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Misty's anchor for Winter's `(x, y, height)` cell box: a cell there.
fn anchor((x, y, height): (f32, f32, f32)) -> Rect {
    Rect::new(x, y, 1.0, height)
}

/// Misty's completion row for Winter's.
fn completion_row(row: &winter_render::CompletionRow) -> CompletionRow {
    CompletionRow {
        detail: row.detail.clone(),
        kind: Some(row.kind).filter(|kind| *kind != ' '),
        label: row.label.clone(),
        source: Some(row.source.clone()).filter(|source| !source.is_empty()),
    }
}

/// Misty's drop list row for Winter's: its indent from where its fold mark
/// sits.
fn drop_row(row: &winter_render::DropListRow) -> DropRow {
    let mut shown = DropRow::new(row.label.as_str()).with_depth(row.mark_col / INDENT_CELLS);
    if let Some(open) = row.fold {
        shown = shown.with_fold(open);
    }
    if !row.note.is_empty() {
        shown = shown.with_note(row.note.as_str());
    }
    match row.dim {
        true => shown.dimmed(),
        false => shown,
    }
}

/// A line of an info card in the foreground color.
fn plain_line(line: &str) -> StyledText {
    StyledText::plain(line, TextColor::Token(misty::theme::ThemeToken::Foreground))
}

/// A line of an info card in its runs' colors and styles.
fn styled_line(runs: &[InfoSpan]) -> StyledText {
    let mut line = StyledText::new();
    for run in runs {
        let color = TextColor::Rgba(Rgba::new(run.color.r, run.color.g, run.color.b, u8::MAX));
        let mut span = Span::new(run.text.as_str(), color);
        if run.bold {
            span = span.bold();
        }
        if run.italic {
            span = span.italic();
        }
        line.push(span);
    }
    line
}

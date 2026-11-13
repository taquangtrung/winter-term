//! The completion popup of an [`EditorView`](crate::EditorView): Cherry's
//! open completion menu drawn as a popup under the text being completed,
//! in the menu's colors: the match typed now on the selection color,
//! each match's detail muted at the right.

use cherry::Completion;
use misty::geometry::{Point, Rect};
use misty::text::{self, StyledText};
use misty::theme::{Theme, ThemeToken};
use misty::{CellMetrics, Frame, Popup, PopupSide};

// ========================================================================
// Constants
// ========================================================================

/// The most matches the popup shows at once; it scrolls to keep the
/// selected one in sight.
const MAX_ROWS: usize = 8;

/// The fewest cells of text a row is wide.
const MIN_TEXT_CELLS: usize = 12;

/// The cells between a match's label and its detail.
const DETAIL_GAP_CELLS: usize = 2;

// ========================================================================
// Functions
// ========================================================================

/// Draws `completion` in the frame's overlay below `anchor` (the cell
/// where the completed text starts, in physical pixels), or above it when
/// there is no room below, in `theme`'s colors.
pub(crate) fn draw_completion(
    frame: &mut Frame,
    completion: &Completion,
    anchor: Rect,
    theme: &Theme,
) {
    let items = completion.items();
    if items.is_empty() {
        return;
    }
    let metrics = frame.cell_metrics();
    let scale_factor = frame.scale_factor();
    let inset = Popup::inset(&theme.shape, scale_factor);
    let padding = metrics.width.round();
    let text_cells = items
        .iter()
        .map(|item| {
            let detail = item
                .detail
                .as_deref()
                .map_or(0, |detail| DETAIL_GAP_CELLS + text::width(detail));
            text::width(&item.label) + detail
        })
        .max()
        .unwrap_or(0)
        .max(MIN_TEXT_CELLS);
    let rows = items.len().min(MAX_ROWS);
    let width = text_cells as f32 * metrics.width + 2.0 * (padding + inset);
    let height = rows as f32 * metrics.height + 2.0 * inset;
    let popup = Popup::new(anchor, width, height, PopupSide::Below, frame.bounds());
    popup.draw(frame, theme);
    let content = popup.content(&theme.shape, scale_factor);
    let first = first_shown(completion.selected(), items.len(), rows);
    frame.overlay(|frame| {
        for (slot, (index, item)) in items.iter().enumerate().skip(first).take(rows).enumerate() {
            let row = row_rect(content, slot, metrics);
            if completion.selected() == Some(index) {
                frame.draw_box(
                    row,
                    &theme.shape.highlight(theme.selection, frame.scale_factor()),
                );
            }
            let text_x = row.x + padding;
            let label = StyledText::plain(item.label.as_str(), ThemeToken::Foreground.into());
            frame.draw_text(&label, Point::new(text_x, row.y), row, theme);
            if let Some(detail) = &item.detail {
                let detail_width = text::width(detail) as f32 * metrics.width;
                let x = (row.x + row.width - padding - detail_width).max(text_x);
                let detail = StyledText::plain(detail.as_str(), ThemeToken::Muted.into());
                frame.draw_text(&detail, Point::new(x, row.y), row, theme);
            }
        }
    });
}

/// The index of the first match shown when `rows` of `count` matches fit
/// and `selected` is typed: from the top until the selected one would
/// fall below the last row, then with it on the last row.
fn first_shown(selected: Option<usize>, count: usize, rows: usize) -> usize {
    let last_first = count.saturating_sub(rows);
    selected
        .map_or(0, |selected| (selected + 1).saturating_sub(rows))
        .min(last_first)
}

/// The `slot`th row of the popup's `content`, a cell tall.
fn row_rect(content: Rect, slot: usize, metrics: CellMetrics) -> Rect {
    Rect::new(
        content.x,
        content.y + slot as f32 * metrics.height,
        content.width,
        metrics.height,
    )
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_shown_rows_scroll_only_to_keep_the_selected_match_in_sight() {
        assert_eq!(first_shown(None, 20, 8), 0);
        assert_eq!(first_shown(Some(7), 20, 8), 0, "the last row");
        assert_eq!(first_shown(Some(8), 20, 8), 1);
        assert_eq!(first_shown(Some(19), 20, 8), 12);
        assert_eq!(first_shown(Some(2), 3, 8), 0, "all fit");
    }
}

//! A pane's bands: an editor's breadcrumb row over its rows (where it is,
//! and the names of its columns), and the strip a pane's gutter draws in beside
//! the grid its content draws in, laid out in pixels so that each stands where it reads
//! best rather than on the content's grid: the gutter a fraction of a column
//! off the text, the breadcrumbs in the window's own font.

use winter_model::layout::{Rect, TabId};
use winter_model::paint::{Gutter, GutterSide};
use winter_render::{ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView, PANE_H_PAD};

use crate::window::terminal::command_marks;
use crate::window::{App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH};

// ========================================================================
// Constants
// ========================================================================

/// How tall the breadcrumb row is, in rows of cells: a little taller than
/// a row, so it reads as the pane's furniture rather than a line of it.
const CRUMB_ROW_CELLS: f32 = 1.2;

/// The first toolbar button a crumb is told back as, past every button an
/// editor's own toolbar numbers, so a press on the breadcrumb row is told
/// apart from one on the toolbar.
const CRUMB_BUTTON_BASE: u16 = 0x8000;

/// What stands between two crumbs.
const CRUMB_SEPARATOR: &str = "\u{203a}";

// ========================================================================
// Data Structures
// ========================================================================

/// Where the parts of a pane draw, in surface pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) struct PaneBands {
    /// The whole of the pane below its strip, the breadcrumb row and the
    /// gutter included.
    pub(in crate::window) bounds: Rect,
    /// The grid the pane's content is painted into: what the breadcrumb row
    /// and the gutter leave.
    pub(in crate::window) content: Rect,
    /// The row an editor's breadcrumbs and column names are drawn in, across
    /// the top of the pane; `None` for one with neither.
    pub(in crate::window) crumbs: Option<Rect>,
    /// The strip the pane's gutter is painted into; `None` for a tab that is
    /// neither an editor nor a terminal.
    pub(in crate::window) gutter: Option<Rect>,
}

/// What a pane holds besides its content, which decides its bands.
#[derive(Clone, Debug, PartialEq)]
struct Parts {
    /// Whether it names its columns.
    columns: bool,
    /// Whether it has breadcrumbs.
    crumbs: bool,
    /// Its gutter.
    gutter: Gutter,
}

// ========================================================================
// App
// ========================================================================

impl App {
    /// Each shown tab paired with the whole of its pane below the strip,
    /// its breadcrumb row and gutter included: what the pointer is over.
    pub(in crate::window) fn pane_bounds(&self) -> Vec<(TabId, Rect)> {
        self.pane_bounds_in(self.content_viewport())
    }

    /// Where the parts of tab `tab` draw within `bounds`, its pane below
    /// the strip.
    pub(in crate::window) fn pane_bands(&self, tab: TabId, bounds: Rect) -> PaneBands {
        let side = self.config.gutter_side;
        let parts = match self.open_tabs.page(&tab) {
            Some(slot) => Parts {
                columns: !slot.page.column_header().is_empty(),
                crumbs: !slot.page.crumbs().is_empty(),
                gutter: Gutter::new(slot.page.gutter_lanes(), side),
            },
            // A terminal's gutter holds its commands' marks, while it is
            // shown; it has no breadcrumbs, its sticky header being drawn
            // over its rows.
            None if self.config.terminal_gutter && self.open_tabs.terminal(&tab).is_some() => {
                Parts {
                    columns: false,
                    crumbs: false,
                    gutter: Gutter::new(command_marks::gutter_lanes(), side),
                }
            }
            None => {
                return PaneBands {
                    bounds,
                    content: bounds,
                    crumbs: None,
                    gutter: None,
                };
            }
        };
        let cell = self.renderer.as_ref().map_or(
            (APPROX_CELL_WIDTH as f32, APPROX_CELL_HEIGHT as f32),
            |renderer| renderer.cell_size(),
        );
        bands(&parts, bounds, cell)
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The breadcrumb row of tab `owner` over `rect`, drawn as a toolbar: each
/// of `crumbs` a button, the last (where the editor is) the heaviest, a
/// separator between two, and the names of its `columns` over them.
pub(in crate::window) fn crumb_bar(
    owner: TabId,
    crumbs: &[String],
    columns: Vec<ToolbarColumn>,
    rect: Rect,
) -> ToolbarView {
    let last = crumbs.len().saturating_sub(1);
    let mut left = Vec::with_capacity(crumbs.len() * 2);
    for (index, crumb) in crumbs.iter().enumerate() {
        if index > 0 {
            left.push(ToolbarItem {
                button: None,
                shrinks: false,
                text: CRUMB_SEPARATOR.to_string(),
                tone: ToolbarTone::Dim,
            });
        }
        left.push(ToolbarItem {
            button: u16::try_from(index)
                .ok()
                .map(|index| CRUMB_BUTTON_BASE.saturating_add(index)),
            shrinks: true,
            text: crumb.clone(),
            tone: match index == last {
                true => ToolbarTone::Strong,
                false => ToolbarTone::Normal,
            },
        });
    }
    ToolbarView {
        columns,
        left,
        owner: owner.0,
        rect: (rect.x, rect.y, rect.width, rect.height),
        right: Vec::new(),
    }
}

/// Which crumb toolbar button `button` is, or `None` for a button of the
/// editor's own toolbar.
pub(in crate::window) fn crumb_of(button: u16) -> Option<usize> {
    button.checked_sub(CRUMB_BUTTON_BASE).map(usize::from)
}

/// `bounds` split between the parts of an editor pane, for cells `cell`
/// wide and tall: the breadcrumb row across the top, for an editor with
/// breadcrumbs or columns to name, then the content beside the gutter.
fn bands(parts: &Parts, bounds: Rect, (cell_width, cell_height): (f32, f32)) -> PaneBands {
    let crumbs = (parts.crumbs || parts.columns).then(|| {
        let height = (cell_height * CRUMB_ROW_CELLS).round().min(bounds.height);
        Rect::new(bounds.x, bounds.y, bounds.width, height)
    });
    let top = crumbs.map_or(bounds.y, |row| row.y + row.height);
    let below = Rect::new(bounds.x, top, bounds.width, bounds.y + bounds.height - top);
    // A gutter with no lanes takes no room: the rows run to the pane's edge.
    let (content, gutter) = match parts.gutter.is_empty() {
        true => (below, None),
        false => {
            let (content, gutter) = split_gutter(&parts.gutter, below, cell_width);
            (content, Some(gutter))
        }
    };
    PaneBands {
        bounds,
        content,
        crumbs,
        gutter,
    }
}

/// `rect` split between `gutter` and the content, for cells `cell_width`
/// wide: the content's rect and the gutter's. The renderer insets the text of
/// each by its side padding, so each rect takes that padding on both sides of
/// its text, and the two overlap where the gutter's padding meets the
/// content's: the gutter's last column ends its gap short of the content's
/// first. The split is on whole pixels, rounded away from the lanes so they
/// are never cut.
fn split_gutter(gutter: &Gutter, rect: Rect, cell_width: f32) -> (Rect, Rect) {
    let lanes = gutter.width() * cell_width;
    let gap = gutter.gap() * cell_width;
    let right = rect.x + rect.width;
    match gutter.side() {
        GutterSide::Left => {
            let start = (rect.x + lanes + gap).ceil().min(right);
            let strip = Rect::new(
                rect.x,
                rect.y,
                start - rect.x + 2.0 * PANE_H_PAD,
                rect.height,
            );
            let content = Rect::new(start, rect.y, right - start, rect.height);
            (content, strip)
        }
        GutterSide::Right => {
            let start = (right - lanes - 2.0 * PANE_H_PAD).floor().max(rect.x);
            let strip = Rect::new(start, rect.y, right - start, rect.height);
            let width = (start + 2.0 * PANE_H_PAD - gap - rect.x)
                .round()
                .clamp(0.0, rect.width);
            let content = Rect::new(rect.x, rect.y, width, rect.height);
            (content, strip)
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::paint::GutterLane;

    use super::*;

    const CELL: (f32, f32) = (10.0, 20.0);

    fn numbers(side: GutterSide) -> Parts {
        Parts {
            columns: false,
            crumbs: false,
            gutter: Gutter::new(vec![GutterLane::Text { width: 3 }], side),
        }
    }

    /// Where the renderer starts and ends the text of `rect`.
    fn text_edges(rect: Rect) -> (f32, f32) {
        (rect.x + PANE_H_PAD, rect.x + rect.width - PANE_H_PAD)
    }

    #[test]
    fn test_a_leading_gutter_ends_its_gap_short_of_the_content_s_text() {
        let bounds = Rect::new(100.0, 0.0, 400.0, 200.0);
        let PaneBands {
            content, gutter, ..
        } = bands(&numbers(GutterSide::Left), bounds, CELL);
        let gutter = gutter.expect("an editor has a gutter");
        let lanes_end = text_edges(gutter).0 + 3.0 * CELL.0;
        assert_eq!(text_edges(content).0 - lanes_end, 10.0, "a cell");
        assert!(text_edges(gutter).1 >= lanes_end, "the numbers are not cut");
        assert_eq!(
            content.x + content.width,
            500.0,
            "the content runs to the edge"
        );
    }

    #[test]
    fn test_a_trailing_gutter_starts_its_gap_past_the_content_s_text() {
        let bounds = Rect::new(100.0, 0.0, 400.0, 200.0);
        let PaneBands {
            content, gutter, ..
        } = bands(&numbers(GutterSide::Right), bounds, CELL);
        let gutter = gutter.expect("an editor has a gutter");
        assert_eq!(text_edges(gutter), (468.0, 498.0), "the lanes at the edge");
        assert_eq!(text_edges(gutter).0 - text_edges(content).1, 10.0);
        assert_eq!(content.x, 100.0);
    }

    #[test]
    fn test_an_editor_with_nothing_beside_its_rows_has_no_gutter() {
        let parts = Parts {
            gutter: Gutter::new(Vec::new(), GutterSide::Left),
            ..numbers(GutterSide::Left)
        };
        let bounds = Rect::new(0.0, 0.0, 400.0, 200.0);
        let PaneBands {
            content, gutter, ..
        } = bands(&parts, bounds, CELL);
        assert_eq!(
            (gutter, content),
            (None, bounds),
            "the rows to the pane's edge"
        );
    }

    #[test]
    fn test_an_editor_naming_columns_gets_a_breadcrumb_row_above_its_rows() {
        let parts = Parts {
            columns: true,
            ..numbers(GutterSide::Left)
        };
        let bounds = Rect::new(0.0, 0.0, 400.0, 210.0);
        let PaneBands {
            content,
            crumbs,
            gutter,
            ..
        } = bands(&parts, bounds, CELL);
        let crumbs = crumbs.expect("a row for the names");
        assert_eq!(
            (crumbs.y, crumbs.height),
            (0.0, 24.0),
            "a fifth taller than a row"
        );
        assert_eq!(content.y, 24.0, "the rows start under it");
        assert_eq!(
            gutter.map(|strip| strip.y),
            Some(content.y),
            "level with the rows"
        );
    }

    #[test]
    fn test_a_press_on_a_crumb_is_told_apart_from_the_toolbar_s_buttons() {
        let crumbs = ["/".to_string(), "tmp".to_string(), "vt".to_string()];
        let bar = crumb_bar(
            TabId(1),
            &crumbs,
            Vec::new(),
            Rect::new(0.0, 0.0, 400.0, 24.0),
        );
        let pressed: Vec<Option<usize>> = bar
            .left
            .iter()
            .filter_map(|item| item.button)
            .map(crumb_of)
            .collect();
        assert_eq!(pressed, [Some(0), Some(1), Some(2)]);
        assert_eq!(crumb_of(3), None, "the editor's own button");
    }

    #[test]
    fn test_an_editor_with_no_header_keeps_the_whole_height() {
        let bounds = Rect::new(0.0, 0.0, 400.0, 210.0);
        let PaneBands {
            content, crumbs, ..
        } = bands(&numbers(GutterSide::Left), bounds, CELL);
        assert_eq!(crumbs, None);
        assert_eq!((content.y, content.height), (0.0, 210.0));
    }

    #[test]
    fn test_a_terminal_has_no_gutter_until_it_is_shown() {
        let mut app = crate::window::App::new();
        let tab = app.layout().focused();
        app.open_tabs
            .insert_terminal(tab, crate::window::test_support::idle_terminal());
        let bounds = Rect::new(0.0, 0.0, 400.0, 200.0);
        let hidden = app.pane_bands(tab, bounds);
        assert_eq!(
            (hidden.gutter, hidden.content),
            (None, bounds),
            "hidden by default"
        );
        app.config.terminal_gutter = true;
        let shown = app.pane_bands(tab, bounds);
        assert!(shown.gutter.is_some(), "its column, once shown");
        assert!(
            shown.content.width < bounds.width,
            "taken from the terminal"
        );
    }
}

//! A pane's bands: an editor's header over its rows (where it is, the names
//! of its columns, and a summary of what it shows), and the strip a pane's
//! gutter draws in beside the grid its content draws in, laid out in pixels
//! so that each stands where it reads best rather than on the content's
//! grid: the gutter a fraction of a column off the text, the header in the
//! window's own font.

use crate::render::{ToolbarColumn, ToolbarItem, ToolbarTone, ToolbarView};
use misty::Rect;
use winter_model::editor::{HEADER_ROWS, Header, HeaderPart, PATH_SEPARATOR};
use winter_model::layout::TabId;
use winter_model::paint::{Gutter, GutterSide, IconKind};

use crate::window::terminal::command_marks;
use crate::window::{APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH, App};

// ========================================================================
// Constants
// ========================================================================

/// The first toolbar button a part of a header's summary is told back as,
/// past every button an editor's own toolbar numbers.
const PART_BUTTON_BASE: u16 = 0x4000;

/// The first toolbar button a part of a header's path is told back as, past
/// every part of its summary.
const PATH_BUTTON_BASE: u16 = PART_BUTTON_BASE + HeaderPart::MAX_ACTION + 1;

// ========================================================================
// Data Structures
// ========================================================================

/// Where the parts of a pane draw, in surface pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) struct PaneBands {
    /// The whole of the pane below its strip, the header and the gutter
    /// included.
    pub(in crate::window) bounds: Rect,
    /// The grid the pane's content is painted into: what the header and the
    /// gutter leave.
    pub(in crate::window) content: Rect,
    /// The editor's header, across the top of the pane; `None` for one with
    /// nothing to show there.
    pub(in crate::window) header: Option<Rect>,
    /// The strip the pane's gutter is painted into; `None` for a tab that is
    /// neither an editor nor a terminal.
    pub(in crate::window) gutter: Option<Rect>,
}

/// What a pane holds besides its content, which decides its bands.
#[derive(Clone, Debug, PartialEq)]
struct Parts {
    /// Its gutter.
    gutter: Gutter,
    /// Whether it has a header.
    header: bool,
}

/// What a press on a header's button is.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(in crate::window) enum HeaderButton {
    /// A part of its summary, told back as this action.
    Part(u16),
    /// The part of its path at this index.
    Path(usize),
}

// ========================================================================
// App
// ========================================================================

impl App {
    /// Each shown tab paired with the whole of its pane below the strip,
    /// its header and gutter included: what the pointer is over.
    pub(in crate::window) fn pane_bounds(&self) -> Vec<(TabId, Rect)> {
        self.pane_bounds_in(self.viewport_rect())
    }

    /// Where the parts of tab `tab` draw within `bounds`, its pane below
    /// the strip.
    pub(in crate::window) fn pane_bands(&self, tab: TabId, bounds: Rect) -> PaneBands {
        let side = self.config.gutter_side;
        let parts = match self.open_tabs.page(&tab) {
            Some(slot) => Parts {
                gutter: Gutter::new(slot.page.gutter_lanes(), side),
                header: !slot.page.header().is_empty(),
            },
            // A terminal's gutter holds its commands' marks, while it is
            // shown; it has no header, its sticky header being drawn over
            // its rows.
            None if self.config.terminal_gutter && self.open_tabs.terminal(&tab).is_some() => {
                Parts {
                    gutter: Gutter::new(command_marks::gutter_lanes(), side),
                    header: false,
                }
            }
            None => {
                return PaneBands {
                    bounds,
                    content: bounds,
                    gutter: None,
                    header: None,
                };
            }
        };
        let cell = self.renderer.as_ref().map_or(
            (APPROX_CELL_WIDTH as f32, APPROX_CELL_HEIGHT as f32),
            |renderer| renderer.cell_size(),
        );
        bands(&parts, bounds, cell, self.pane_pad())
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// The header `header` of tab `owner` over `rect`, drawn as a toolbar:
/// each part of its path a button, ancestors muted and the last (where the
/// editor is) in normal foreground with `icon`, a separator between two, the
/// names of its columns over them as `columns` places them, and its summary
/// at the right.
pub(in crate::window) fn header_bar(
    owner: TabId,
    header: &Header,
    icon: Option<IconKind>,
    columns: Vec<ToolbarColumn>,
    rect: Rect,
) -> ToolbarView {
    let path = header.path();
    let last = path.len().saturating_sub(1);
    let mut left = Vec::with_capacity(path.len() * 2);
    for (index, part) in path.iter().enumerate() {
        if index > 0 {
            left.push(ToolbarItem {
                button: None,
                icon: None,
                shrinks: false,
                text: PATH_SEPARATOR.to_string(),
                tone: ToolbarTone::Dim,
            });
        }
        let is_last = index == last;
        left.push(ToolbarItem {
            button: u16::try_from(index)
                .ok()
                .map(|index| PATH_BUTTON_BASE.saturating_add(index)),
            icon: match is_last {
                true => icon.clone(),
                false => None,
            },
            shrinks: true,
            text: part.clone(),
            tone: match is_last {
                true => ToolbarTone::Normal,
                false => ToolbarTone::Dim,
            },
        });
    }
    let right = header
        .summary()
        .map(|part| ToolbarItem {
            button: part
                .action
                .filter(|action| *action <= HeaderPart::MAX_ACTION)
                .map(|action| PART_BUTTON_BASE + action),
            icon: None,
            shrinks: false,
            text: part.text.clone(),
            tone: part.tone,
        })
        .collect();
    ToolbarView {
        columns,
        left,
        owner: owner.0,
        rect: (rect.x, rect.y, rect.width, rect.height),
        right,
    }
}

/// Which part of a header toolbar button `button` is, or `None` for a
/// button of the editor's own toolbar.
pub(in crate::window) fn header_button(button: u16) -> Option<HeaderButton> {
    match button {
        PATH_BUTTON_BASE.. => Some(HeaderButton::Path(usize::from(button - PATH_BUTTON_BASE))),
        PART_BUTTON_BASE.. => Some(HeaderButton::Part(button - PART_BUTTON_BASE)),
        _ => None,
    }
}

/// `bounds` split between the parts of an editor pane, for cells `cell`
/// wide and tall and side padding `pad`: the header across the top, for an
/// editor with one, then the content beside the gutter.
fn bands(
    parts: &Parts,
    bounds: Rect,
    (cell_width, cell_height): (f32, f32),
    pad: f32,
) -> PaneBands {
    let header = parts.header.then(|| {
        let height = (cell_height * HEADER_ROWS).round().min(bounds.height);
        Rect::new(bounds.x, bounds.y, bounds.width, height)
    });
    let top = header.map_or(bounds.y, |line| line.y + line.height);
    let below = Rect::new(bounds.x, top, bounds.width, bounds.y + bounds.height - top);
    // A gutter with no lanes takes no room: the rows run to the pane's edge.
    let (content, gutter) = match parts.gutter.is_empty() {
        true => (below, None),
        false => {
            let (content, gutter) = split_gutter(&parts.gutter, below, cell_width, pad);
            (content, Some(gutter))
        }
    };
    PaneBands {
        bounds,
        content,
        gutter,
        header,
    }
}

/// `rect` split between `gutter` and the content, for cells `cell_width`
/// wide: the content's rect and the gutter's. The renderer insets the text of
/// each by its side padding `pad`, so each rect takes that padding on both sides of
/// its text, and the two overlap where the gutter's padding meets the
/// content's: the gutter's last column ends its gap short of the content's
/// first. The split is on whole pixels, rounded away from the lanes so they
/// are never cut.
fn split_gutter(gutter: &Gutter, rect: Rect, cell_width: f32, pad: f32) -> (Rect, Rect) {
    let lanes = gutter.width() * cell_width;
    let gap = gutter.gap() * cell_width;
    let right = rect.x + rect.width;
    match gutter.side() {
        GutterSide::Left => {
            let start = (rect.x + lanes + gap).ceil().min(right);
            let strip = Rect::new(rect.x, rect.y, start - rect.x + 2.0 * pad, rect.height);
            let content = Rect::new(start, rect.y, right - start, rect.height);
            (content, strip)
        }
        GutterSide::Right => {
            let start = (right - lanes - 2.0 * pad).floor().max(rect.x);
            let strip = Rect::new(start, rect.y, right - start, rect.height);
            let width = (start + 2.0 * pad - gap - rect.x)
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
    use crate::render::PANE_H_PAD;
    use winter_model::editor::HeaderSegment;
    use winter_model::paint::GutterLane;

    use super::*;

    const CELL: (f32, f32) = (10.0, 20.0);

    fn numbers(side: GutterSide) -> Parts {
        Parts {
            gutter: Gutter::new(vec![GutterLane::Text { width: 3 }], side),
            header: false,
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
        } = bands(&numbers(GutterSide::Left), bounds, CELL, PANE_H_PAD);
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
        } = bands(&numbers(GutterSide::Right), bounds, CELL, PANE_H_PAD);
        let gutter = gutter.expect("an editor has a gutter");
        let right_edge = bounds.x + bounds.width - PANE_H_PAD;
        assert_eq!(
            text_edges(gutter),
            (right_edge - 30.0, right_edge),
            "the lanes at the edge"
        );
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
        } = bands(&parts, bounds, CELL, PANE_H_PAD);
        assert_eq!(
            (gutter, content),
            (None, bounds),
            "the rows to the pane's edge"
        );
    }

    #[test]
    fn test_an_editor_with_a_header_gets_it_above_its_rows() {
        let parts = Parts {
            header: true,
            ..numbers(GutterSide::Left)
        };
        let bounds = Rect::new(0.0, 0.0, 400.0, 210.0);
        let PaneBands {
            content,
            gutter,
            header,
            ..
        } = bands(&parts, bounds, CELL, PANE_H_PAD);
        let line = header.expect("a header");
        assert_eq!(
            (line.y, line.height),
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
    fn test_a_header_s_summary_goes_at_the_right_each_press_told_apart() {
        let header = Header::new(vec![
            HeaderSegment::Path(vec!["home".to_string()]),
            HeaderSegment::Summary(vec![
                HeaderPart::label("12 processes", ToolbarTone::Normal),
                HeaderPart::button("tree", ToolbarTone::Dim, 3, "Listed as a tree"),
            ]),
        ]);
        let bar = header_bar(
            TabId(1),
            &header,
            None,
            Vec::new(),
            Rect::new(0.0, 0.0, 400.0, 24.0),
        );
        let texts: Vec<&str> = bar.right.iter().map(|item| item.text.as_str()).collect();
        assert_eq!(texts, ["12 processes", "tree"]);
        let pressed: Vec<Option<HeaderButton>> = bar
            .left
            .iter()
            .chain(&bar.right)
            .filter_map(|item| item.button)
            .map(header_button)
            .collect();
        assert_eq!(
            pressed,
            [Some(HeaderButton::Path(0)), Some(HeaderButton::Part(3))]
        );
    }

    #[test]
    fn test_a_press_on_a_path_part_is_told_apart_from_the_toolbar_s_buttons() {
        let path = vec!["/".to_string(), "tmp".to_string(), "vt".to_string()];
        let header = Header::new(vec![HeaderSegment::Path(path)]);
        let bar = header_bar(
            TabId(1),
            &header,
            None,
            Vec::new(),
            Rect::new(0.0, 0.0, 400.0, 24.0),
        );
        let pressed: Vec<Option<HeaderButton>> = bar
            .left
            .iter()
            .filter_map(|item| item.button)
            .map(header_button)
            .collect();
        let parts = [0, 1, 2].map(|at| Some(HeaderButton::Path(at)));
        assert_eq!(pressed, parts);
        assert_eq!(header_button(3), None, "the editor's own button");
    }

    #[test]
    fn test_header_bar_dims_ancestors_and_highlights_where_it_is() {
        let path = vec!["/".to_string(), "tmp".to_string(), "vt".to_string()];
        let header = Header::new(vec![HeaderSegment::Path(path)]);
        let file_icon = IconKind::File {
            name: "vt".to_string(),
        };
        let bar = header_bar(
            TabId(1),
            &header,
            Some(file_icon.clone()),
            Vec::new(),
            Rect::new(0.0, 0.0, 400.0, 24.0),
        );
        let tones: Vec<ToolbarTone> = bar
            .left
            .iter()
            .filter(|item| item.button.is_some())
            .map(|item| item.tone)
            .collect();
        assert_eq!(
            tones,
            [ToolbarTone::Dim, ToolbarTone::Dim, ToolbarTone::Normal]
        );
        let icons: Vec<Option<&IconKind>> = bar
            .left
            .iter()
            .filter(|item| item.button.is_some())
            .map(|item| item.icon.as_ref())
            .collect();
        assert_eq!(icons, [None, None, Some(&file_icon)]);
    }

    #[test]
    fn test_an_editor_with_no_header_keeps_the_whole_height() {
        let bounds = Rect::new(0.0, 0.0, 400.0, 210.0);
        let PaneBands {
            content, header, ..
        } = bands(&numbers(GutterSide::Left), bounds, CELL, PANE_H_PAD);
        assert_eq!(header, None);
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

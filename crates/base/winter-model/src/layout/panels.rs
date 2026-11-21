//! The window's panels: a left sidebar, a right sidebar, and a bottom panel
//! around the main area. Each is one pane outside the split tree, shown or
//! hidden, sized in cells rather than as a share of the window.
//!
//! The sidebars run the window's full height and the bottom panel spans the
//! main area between them. When the window is too small for the panels at
//! their sizes, they shrink toward their minimums, the right sidebar first,
//! then the left, then the bottom panel; past that the right sidebar is left
//! out, then the left, then the bottom panel. Being left out is worked out
//! each time the window is laid out and never stored: a panel the user
//! shows stays shown, and comes back as the window grows.

use misty::Rect;

use super::tree::Pane;
use super::{PaneTree, TabId};

// ========================================================================
// Constants
// ========================================================================

/// A sidebar's width, in cells, until the user sets one.
pub const SIDEBAR_COLS: u16 = 32;

/// The bottom panel's height, in cells, until the user sets one.
pub const BOTTOM_ROWS: u16 = 14;

/// The narrowest a sidebar shrinks to, in cells, before it is left out.
pub const MIN_SIDEBAR_COLS: u16 = 20;

/// The lowest the bottom panel shrinks to, in cells, before it is left out.
pub const MIN_BOTTOM_ROWS: u16 = 4;

/// The narrowest the main area is let become, in cells, for the panels.
const MAIN_MIN_COLS: f32 = 40.0;

/// The lowest the main area is let become, in cells, for the bottom panel.
const MAIN_MIN_ROWS: f32 = 6.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Which panel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Side {
    /// The bottom panel, across the main area.
    Bottom,
    /// The left sidebar.
    Left,
    /// The right sidebar.
    Right,
}

/// Where a tab sits: in the main area's split tree, or in a panel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Region {
    /// The main area, where the work happens.
    Main,
    /// A panel.
    Panel(Side),
}

/// Whether a panel holds tabs, and whether it is shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelState {
    /// No tabs: the panel is there for the next thing sent to it.
    Empty,
    /// Tabs, out of sight; their programs keep running.
    Hidden,
    /// Tabs, in sight (unless the window is too small for it).
    Shown,
}

/// A snapshot of one panel, for sessions and handovers.
#[derive(Clone, Debug, PartialEq)]
pub struct PanelTree {
    /// Its tabs, `None` for an empty panel.
    pub pane: Option<PaneTree>,
    /// Whether the user has it shown.
    pub shown: bool,
    /// Which panel.
    pub side: Side,
    /// Its width (a sidebar) or height (the bottom panel), in cells.
    pub size: u16,
}

/// One panel: its tabs, whether it is shown, and its size.
#[derive(Clone, Debug)]
pub(super) struct Panel {
    /// Its tabs, `None` while it has none.
    pub(super) pane: Option<Pane>,
    /// Whether the user has it shown.
    pub(super) shown: bool,
    /// Its width (a sidebar) or height (the bottom panel), in cells.
    pub(super) size: u16,
}

/// The three panels of a window.
#[derive(Clone, Debug)]
pub(super) struct Panels {
    bottom: Panel,
    left: Panel,
    right: Panel,
}

/// Where the panels that fit go, and what is left for the main area.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Fitted {
    /// The main area.
    pub(super) main: Rect,
    /// Each panel laid out, left, right, then bottom.
    pub(super) panels: Vec<(Side, Rect)>,
}

// ========================================================================
// Side Implementations
// ========================================================================

impl Side {
    /// Every panel, in the order they are laid out and listed.
    pub const ALL: [Side; 3] = [Side::Left, Side::Right, Side::Bottom];

    /// The size a panel on this side starts at, in cells.
    pub fn default_size(self) -> u16 {
        match self {
            Side::Bottom => BOTTOM_ROWS,
            Side::Left | Side::Right => SIDEBAR_COLS,
        }
    }

    /// The smallest a panel on this side shrinks to, in cells.
    pub fn min_size(self) -> u16 {
        match self {
            Side::Bottom => MIN_BOTTOM_ROWS,
            Side::Left | Side::Right => MIN_SIDEBAR_COLS,
        }
    }
}

// ========================================================================
// Panel Implementations
// ========================================================================

impl Panel {
    /// An empty, hidden panel on `side`, at its default size.
    fn new(side: Side) -> Self {
        Self {
            pane: None,
            shown: false,
            size: side.default_size(),
        }
    }

    /// Whether it has tabs, and whether it is shown.
    pub(super) fn state(&self) -> PanelState {
        match (&self.pane, self.shown) {
            (None, _) => PanelState::Empty,
            (Some(_), false) => PanelState::Hidden,
            (Some(_), true) => PanelState::Shown,
        }
    }

    /// Whether it asks for room: shown, with tabs.
    fn wants_room(&self) -> bool {
        self.state() == PanelState::Shown
    }
}

// ========================================================================
// Panels Implementations
// ========================================================================

impl Panels {
    /// Three empty, hidden panels.
    pub(super) fn new() -> Self {
        Self {
            bottom: Panel::new(Side::Bottom),
            left: Panel::new(Side::Left),
            right: Panel::new(Side::Right),
        }
    }

    /// The panel on `side`.
    pub(super) fn get(&self, side: Side) -> &Panel {
        match side {
            Side::Bottom => &self.bottom,
            Side::Left => &self.left,
            Side::Right => &self.right,
        }
    }

    /// The panel on `side`, mutably.
    pub(super) fn get_mut(&mut self, side: Side) -> &mut Panel {
        match side {
            Side::Bottom => &mut self.bottom,
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    /// The panel holding `tab`, if one does.
    pub(super) fn side_of(&self, tab: TabId) -> Option<Side> {
        Side::ALL.into_iter().find(|&side| {
            self.get(side)
                .pane
                .as_ref()
                .is_some_and(|pane| pane.members.contains(&tab))
        })
    }

    /// Every panel's pane, left, right, then bottom, leaving out empty ones.
    pub(super) fn panes(&self) -> impl Iterator<Item = (Side, &Pane)> {
        Side::ALL
            .into_iter()
            .filter_map(|side| self.get(side).pane.as_ref().map(|pane| (side, pane)))
    }

    /// Where the panels that ask for room go within `viewport`, cells
    /// `cell` (width, height) in pixels, and the main area what is left.
    pub(super) fn fit(&self, viewport: Rect, cell: (f32, f32)) -> Fitted {
        let (cell_w, cell_h) = cell;
        let room = |side: Side| self.get(side).wants_room();
        let size = |side: Side| f32::from(self.get(side).size);
        let min = |side: Side| f32::from(side.min_size());
        // Across: the sidebars against the main area's least width.
        let spare_w = (viewport.width - MAIN_MIN_COLS * cell_w).max(0.0);
        let full_left = room(Side::Left).then(|| size(Side::Left) * cell_w);
        let mut right = room(Side::Right).then(|| size(Side::Right) * cell_w);
        let mut left = full_left;
        loop {
            // Shrink the right first, then the left, each to its least.
            for side in [Side::Right, Side::Left] {
                let over = left.unwrap_or(0.0) + right.unwrap_or(0.0) - spare_w;
                let width = match side {
                    Side::Right => &mut right,
                    _ => &mut left,
                };
                if over > 0.0
                    && let Some(width) = width
                {
                    *width = (*width - over).max(min(side) * cell_w);
                }
            }
            if left.unwrap_or(0.0) + right.unwrap_or(0.0) <= spare_w {
                break;
            }
            // Still too wide: leave out the right, the left back at its own
            // size to shrink again; then the left.
            match right {
                Some(_) => {
                    right = None;
                    left = full_left;
                }
                None => {
                    left = None;
                    break;
                }
            }
        }
        // Down: the bottom panel against the main area's least height.
        let spare_h = (viewport.height - MAIN_MIN_ROWS * cell_h).max(0.0);
        let bottom = room(Side::Bottom)
            .then(|| (size(Side::Bottom) * cell_h).min(spare_h))
            .filter(|height| *height >= min(Side::Bottom) * cell_h);
        let left_w = left.map_or(0.0, f32::round);
        let right_w = right.map_or(0.0, f32::round);
        let bottom_h = bottom.map_or(0.0, f32::round);
        let middle_x = viewport.x + left_w;
        let middle_w = viewport.width - left_w - right_w;
        let mut panels = Vec::new();
        if left.is_some() {
            panels.push((
                Side::Left,
                Rect::new(viewport.x, viewport.y, left_w, viewport.height),
            ));
        }
        if right.is_some() {
            panels.push((
                Side::Right,
                Rect::new(
                    viewport.x + viewport.width - right_w,
                    viewport.y,
                    right_w,
                    viewport.height,
                ),
            ));
        }
        if bottom.is_some() {
            panels.push((
                Side::Bottom,
                Rect::new(
                    middle_x,
                    viewport.y + viewport.height - bottom_h,
                    middle_w,
                    bottom_h,
                ),
            ));
        }
        Fitted {
            main: Rect::new(middle_x, viewport.y, middle_w, viewport.height - bottom_h),
            panels,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Panels with `sides` shown, each holding one tab.
    fn shown(sides: &[Side]) -> Panels {
        let mut panels = Panels::new();
        for (index, &side) in sides.iter().enumerate() {
            let panel = panels.get_mut(side);
            panel.pane = Some(Pane::new(TabId(10 + index as u64)));
            panel.shown = true;
        }
        panels
    }

    const CELL: (f32, f32) = (10.0, 20.0);

    #[test]
    fn test_the_sidebars_run_full_height_and_the_bottom_panel_spans_between() {
        let panels = shown(&[Side::Left, Side::Right, Side::Bottom]);
        let fitted = panels.fit(Rect::new(0.0, 0.0, 1600.0, 800.0), CELL);
        assert_eq!(
            fitted.panels,
            [
                (Side::Left, Rect::new(0.0, 0.0, 320.0, 800.0)),
                (Side::Right, Rect::new(1280.0, 0.0, 320.0, 800.0)),
                (Side::Bottom, Rect::new(320.0, 520.0, 960.0, 280.0)),
            ]
        );
        assert_eq!(fitted.main, Rect::new(320.0, 0.0, 960.0, 520.0));
    }

    #[test]
    fn test_a_hidden_or_empty_panel_takes_no_room() {
        let mut panels = shown(&[Side::Left]);
        panels.get_mut(Side::Left).shown = false;
        panels.get_mut(Side::Right).shown = true;
        let viewport = Rect::new(0.0, 0.0, 1000.0, 600.0);
        let fitted = panels.fit(viewport, CELL);
        assert!(fitted.panels.is_empty());
        assert_eq!(fitted.main, viewport);
    }

    #[test]
    fn test_a_small_window_shrinks_the_right_sidebar_first_then_leaves_it_out() {
        let panels = shown(&[Side::Left, Side::Right]);
        let widths = |width: f32| {
            panels
                .fit(Rect::new(0.0, 0.0, width, 600.0), CELL)
                .panels
                .into_iter()
                .map(|(side, rect)| (side, rect.width))
                .collect::<Vec<_>>()
        };
        // The main area keeps 40 cells (400px); the sidebars want 320px each.
        assert_eq!(
            widths(1000.0),
            [(Side::Left, 320.0), (Side::Right, 280.0)],
            "the right shrinks first"
        );
        assert_eq!(widths(620.0), [(Side::Left, 220.0)], "then is left out");
        assert_eq!(widths(500.0), [], "then the left");
    }

    #[test]
    fn test_the_bottom_panel_shrinks_to_its_least_then_is_left_out() {
        let panels = shown(&[Side::Bottom]);
        let fitted = panels.fit(Rect::new(0.0, 0.0, 800.0, 300.0), CELL);
        assert_eq!(
            fitted.panels[0].1.height, 180.0,
            "the main area keeps 6 rows"
        );
        let short = panels.fit(Rect::new(0.0, 0.0, 800.0, 190.0), CELL);
        assert!(short.panels.is_empty(), "under 4 rows, left out");
    }
}

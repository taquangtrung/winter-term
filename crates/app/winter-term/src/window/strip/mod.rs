//! The strip of tabs along the top of every pane: where each strip sits, the
//! pane area left below it, and where a tab let go of goes. Misty draws the
//! strips and takes the pointer over them ([`view`]); which tabs are busy,
//! and so wear a spinner, lives in [`busy`].

pub(in crate::window) mod busy;
pub(in crate::window) mod view;

use misty::{CellMetrics, TabStrip};

use winter_model::layout::{FocusDir, PaneArea, Rect, TabId};
use winter_model::paint::IconKind;

use crate::window::{App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH};

// ========================================================================
// Constants
// ========================================================================

/// What stands at the caret of a tab name being typed into the strip.
const RENAME_CARET: char = '\u{2502}';

/// Fraction of a pane's width/height from an edge that defines a split drop zone.
pub(in crate::window) const SPLIT_DROP_EDGE_RATIO: f32 = 0.25;

// ========================================================================
// Data Structures
// ========================================================================

/// What a point on a pane strip lands on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(in crate::window) enum StripHit {
    /// The close mark of this tab.
    Close(TabId),
    /// The new-tab button of the strip whose shown tab is this one.
    NewTab(TabId),
    /// No strip, or the blank end of one.
    #[default]
    None,
    /// This tab's label.
    Tab(TabId),
}

/// Where a dragged tab is let go.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::window) enum TabDrop {
    /// Anywhere off the strips, at this point: by the pane under it.
    At(f32, f32),
    /// Onto a strip, at `index` among the tabs of the pane holding `pane`.
    Strip { index: usize, pane: TabId },
}

// ========================================================================
// Functions
// ========================================================================

/// Which split direction corresponds to a point within a pane rectangle.
fn pane_drop_split_dir(x: f32, y: f32, pr: &winter_render::renderer::PaneRect) -> FocusDir {
    let w = pr.width.max(1.0);
    let h = pr.height.max(1.0);
    let norm_x = ((x - pr.x) / w).clamp(0.0, 1.0);
    let norm_y = ((y - pr.y) / h).clamp(0.0, 1.0);
    let dist_left = norm_x;
    let dist_right = 1.0 - norm_x;
    let dist_top = norm_y;
    let dist_bottom = 1.0 - norm_y;
    let min_dist = dist_left.min(dist_right).min(dist_top.min(dist_bottom));
    if min_dist == dist_top {
        FocusDir::Up
    } else if min_dist == dist_bottom {
        FocusDir::Down
    } else if min_dist == dist_left {
        FocusDir::Left
    } else {
        FocusDir::Right
    }
}

/// Whether a point within a pane rectangle falls into an outer edge zone.
fn is_pane_edge_zone(x: f32, y: f32, pr: &winter_render::renderer::PaneRect) -> bool {
    let w = pr.width.max(1.0);
    let h = pr.height.max(1.0);
    let norm_x = ((x - pr.x) / w).clamp(0.0, 1.0);
    let norm_y = ((y - pr.y) / h).clamp(0.0, 1.0);
    !(SPLIT_DROP_EDGE_RATIO..=1.0 - SPLIT_DROP_EDGE_RATIO).contains(&norm_x)
        || !(SPLIT_DROP_EDGE_RATIO..=1.0 - SPLIT_DROP_EDGE_RATIO).contains(&norm_y)
}

// ========================================================================
// App: pane strips
// ========================================================================

impl App {
    /// Pixel height of a pane strip, as misty draws it in the font of the
    /// last frame (Winter's own cells before the first).
    pub(in crate::window) fn strip_height(&self) -> f32 {
        let metrics = self
            .window
            .as_ref()
            .and_then(|window| window.cell_metrics());
        let metrics = metrics.unwrap_or_else(|| {
            let (width, height) = self
                .renderer
                .as_ref()
                .map(|r| r.cell_size())
                .unwrap_or((APPROX_CELL_WIDTH as f32, APPROX_CELL_HEIGHT as f32));
            CellMetrics {
                baseline: height,
                height,
                width,
            }
        });
        TabStrip::height(metrics)
    }

    /// Every pane of the layout, with its strip's area.
    pub(in crate::window) fn strip_areas(&self) -> Vec<(PaneArea, Rect)> {
        self.strip_areas_in(self.content_viewport())
    }

    /// The same, laid out within `viewport`.
    pub(in crate::window) fn strip_areas_in(&self, viewport: Rect) -> Vec<(PaneArea, Rect)> {
        let height = self.strip_height();
        self.layout()
            .panes(viewport)
            .into_iter()
            .map(|area| {
                let rect = area.rect;
                let strip = Rect::new(rect.x, rect.y, rect.width, height.min(rect.height));
                (area, strip)
            })
            .collect()
    }

    /// Each shown tab paired with the area it draws its content in: its
    /// pane's area below the strip, less an editor's gutter (see
    /// [`App::pane_bands`]).
    pub(in crate::window) fn pane_rects(&self) -> Vec<(TabId, Rect)> {
        self.pane_rects_in(self.content_viewport())
    }

    /// The same, laid out within `viewport`.
    pub(in crate::window) fn pane_rects_in(&self, viewport: Rect) -> Vec<(TabId, Rect)> {
        self.pane_bounds_in(viewport)
            .into_iter()
            .map(|(id, bounds)| (id, self.pane_bands(id, bounds).content))
            .collect()
    }

    /// Each shown tab paired with its pane's whole area below the strip,
    /// laid out within `viewport`.
    pub(in crate::window) fn pane_bounds_in(&self, viewport: Rect) -> Vec<(TabId, Rect)> {
        let height = self.strip_height();
        self.layout()
            .rects(viewport)
            .into_iter()
            .map(|(id, rect)| {
                let top = height.min(rect.height);
                (
                    id,
                    Rect::new(rect.x, rect.y + top, rect.width, rect.height - top),
                )
            })
            .collect()
    }

    /// What `tab` holds, as its icon depicts it: its editor, or a terminal
    /// by the program running in it.
    pub(in crate::window) fn tab_icon_kind(&self, tab: TabId) -> IconKind {
        if let Some(slot) = self.open_tabs.page(&tab) {
            return slot.tab_icon();
        }
        let program = self
            .open_tabs
            .terminal(&tab)
            .and_then(|terminal| terminal.title_facts().program);
        match program {
            Some(name) => IconKind::Program { name },
            None => IconKind::Terminal,
        }
    }

    /// How a strip names `pane`: the rename being typed while it is the
    /// focused tab, else its title.
    pub(in crate::window) fn strip_title(&self, tab: TabId) -> String {
        match self.chrome.rename_input() {
            Some(input) if input.tab == tab => {
                let text = input.field.text();
                let (before, after) = text.split_at(
                    text.char_indices()
                        .nth(input.field.col())
                        .map_or(text.len(), |(at, _)| at),
                );
                format!("{before}{RENAME_CARET}{after}")
            }
            _ => self.tab_title(tab),
        }
    }

    /// Track what the pointer is over on the pane strips, for hover marks.
    pub(in crate::window) fn update_strip_hover(&mut self, x: f32, y: f32) {
        let hit = self.strip_hit(x, y);
        if hit != self.tabs.strip_hover {
            self.tabs.strip_hover = hit;
            self.dirty = true;
        }
    }

    /// Move `tab` to where it was let go: among a strip's tabs, or by the
    /// pane under the point, splitting that pane when the point is near one
    /// of its edges (or in the tab's own pane), joining its tabs otherwise.
    /// A point past the window's edge counts as on the pane nearest it.
    pub(in crate::window) fn drop_tab(&mut self, tab: TabId, drop: TabDrop) {
        let (x, y) = match drop {
            TabDrop::Strip { index, pane } => {
                let members = self.layout().tabs_in_pane(pane);
                match members.get(index) {
                    Some(&target) if target == tab => {}
                    Some(&target) => self.move_tab_to(tab, target),
                    None if members.contains(&tab) => {
                        if let Some(&last) = members.last() {
                            self.move_tab_to(tab, last);
                        }
                    }
                    None => self.move_tab_to_pane(tab, pane),
                }
                return;
            }
            TabDrop::At(x, y) => (x, y),
        };
        let target = self.pane_at_pixel(x, y).or_else(|| {
            let vp = self.content_viewport();
            let clamped_x = x.clamp(vp.x, vp.x + vp.width.max(1.0) - 1.0);
            let clamped_y = y.clamp(vp.y, vp.y + vp.height.max(1.0) - 1.0);
            self.pane_at_pixel(clamped_x, clamped_y)
        });
        let Some((target_pane, pr)) = target else {
            return;
        };
        let same = self.layout().same_pane(tab, target_pane);
        if same || is_pane_edge_zone(x, y, &pr) {
            let direction = pane_drop_split_dir(x, y, &pr);
            self.split_tab_to_new_pane(tab, target_pane, direction);
        } else {
            self.move_tab_to_pane(tab, target_pane);
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dropping_a_tab_reorders_in_same_strip() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().members(), vec![tab0, tab1]);

        // Drop tab1 at tab0's place on the strip.
        app.drop_tab(
            tab1,
            TabDrop::Strip {
                index: 0,
                pane: tab0,
            },
        );
        assert_eq!(app.layout().members(), vec![tab1, tab0]);
    }

    #[test]
    fn test_dropping_a_tab_across_panes_into_other_strip() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        app.split_pane_with_terminal(winter_model::layout::Direction::Horizontal);
        let tab2 = app.layout().focused();

        // Pane 1 has [tab0, tab1], Pane 2 has [tab2]
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0, tab1]);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab2]);

        // Drop tab1 on pane 2's strip, at tab2's place.
        app.drop_tab(
            tab1,
            TabDrop::Strip {
                index: 0,
                pane: tab2,
            },
        );
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0]);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab1, tab2]);
        assert_eq!(app.layout().focused(), tab1);
    }

    #[test]
    fn test_dropping_a_tab_into_other_pane_body_collapses_empty_source_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane_with_terminal(winter_model::layout::Direction::Horizontal);
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 2);

        // Drag tab0 into tab1's pane content area
        let pane1_rect = app.pane_rect(tab1).expect("pane 1 rect");
        let drop_x = pane1_rect.x + pane1_rect.width / 2.0;
        let drop_y = pane1_rect.y + pane1_rect.height / 2.0;

        let (x, y) = (drop_x, drop_y);
        app.drop_tab(tab0, TabDrop::At(x, y));
        // Since tab0 was the only tab in its pane, that pane collapses!
        assert_eq!(app.layout().shown_tabs().len(), 1);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1, tab0]);
        assert_eq!(app.layout().focused(), tab0);
    }

    #[test]
    fn test_move_tab_in_direction_moves_tab_to_adjacent_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        // Split vertically (tab0 on left, tab1 on right)
        app.split_pane_with_terminal(winter_model::layout::Direction::Vertical);
        let tab1 = app.layout().focused();
        assert_ne!(tab0, tab1);
        assert_eq!(app.layout().shown_tabs().len(), 2);

        // Focus left pane (tab0) and add another tab (tab2) to tab0's pane
        app.layout_mut().focus(tab0);
        app.new_tab();
        let tab2 = app.layout().focused();
        assert_ne!(tab2, tab0);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab0, tab2]);

        // Move current tab (tab2) to the right pane
        app.move_tab_in_direction(winter_model::layout::FocusDir::Right);
        // tab2 is now in tab1's pane
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab1, tab2]);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0]);
        assert_eq!(app.layout().focused(), tab2);

        // Move tab2 back to the left pane
        app.move_tab_in_direction(winter_model::layout::FocusDir::Left);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab0, tab2]);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);

        // Move tab2 left again: there is no pane to the left, so tab2 becomes a new pane on the left
        app.move_tab_in_direction(winter_model::layout::FocusDir::Left);
        assert_eq!(app.layout().shown_tabs().len(), 3);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab2]);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0]);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);
    }

    #[test]
    fn test_dropping_a_tab_into_own_pane_splits_into_new_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 1);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0, tab1]);

        let pane_rect = app.pane_rect(tab1).expect("pane rect");
        // Drag tab1 to the bottom 10% of the pane body (split Down)
        let (x, y) = (
            pane_rect.x + pane_rect.width / 2.0,
            pane_rect.y + pane_rect.height * 0.9,
        );
        app.drop_tab(tab1, TabDrop::At(x, y));
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().focused(), tab1);
        let vp = Rect::new(0.0, 0.0, 800.0, 600.0);
        assert_eq!(app.layout().pane_in_direction(FocusDir::Up, vp), Some(tab0));

        // Move tab1 back into tab0's pane
        app.move_tab_to_pane(tab1, tab0);
        assert_eq!(app.layout().shown_tabs().len(), 1);

        let pane_rect = app.pane_rect(tab1).expect("pane rect");
        // Drag tab1 to the right 10% of the pane body (split Right)
        let (x, y) = (
            pane_rect.x + pane_rect.width * 0.9,
            pane_rect.y + pane_rect.height / 2.0,
        );
        app.drop_tab(tab1, TabDrop::At(x, y));
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().focused(), tab1);
        assert_eq!(
            app.layout().pane_in_direction(FocusDir::Left, vp),
            Some(tab0)
        );
    }

    #[test]
    fn test_dropping_a_tab_into_other_pane_edge_splits_target_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane_with_terminal(winter_model::layout::Direction::Vertical);
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 2);

        let pane0_rect = app.pane_rect(tab0).expect("pane 0 rect");
        // Drag tab1 to the left 10% of pane0 (split Left of pane0)
        let (x, y) = (
            pane0_rect.x + pane0_rect.width * 0.1,
            pane0_rect.y + pane0_rect.height / 2.0,
        );
        app.drop_tab(tab1, TabDrop::At(x, y));
        // tab1 was the lone tab in its pane, so old pane collapsed and new pane created to the left of tab0
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().focused(), tab1);
        let vp = Rect::new(0.0, 0.0, 800.0, 600.0);
        assert_eq!(
            app.layout().pane_in_direction(FocusDir::Right, vp),
            Some(tab0)
        );

        // Add tab2 to tab1's pane
        app.new_tab();
        let tab2 = app.layout().focused();
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1, tab2]);

        // Drag tab2 to the bottom 10% of tab0's pane (split Down of tab0)
        let pane0_rect_updated = app.pane_rect(tab0).expect("pane 0 rect");
        let (x, y) = (
            pane0_rect_updated.x + pane0_rect_updated.width / 2.0,
            pane0_rect_updated.y + pane0_rect_updated.height * 0.9,
        );
        app.drop_tab(tab2, TabDrop::At(x, y));
        assert_eq!(app.layout().shown_tabs().len(), 3);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);
        assert_eq!(app.layout().pane_in_direction(FocusDir::Up, vp), Some(tab0));
    }

    #[test]
    fn test_dropping_a_tab_past_window_edge_clamps_and_splits() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 1);

        // Drag tab1 far past the right edge of the window
        let (x, y) = (999.0, 300.0);
        app.drop_tab(tab1, TabDrop::At(x, y));
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().focused(), tab1);
        let vp = Rect::new(0.0, 0.0, 800.0, 600.0);
        assert_eq!(
            app.layout().pane_in_direction(FocusDir::Left, vp),
            Some(tab0)
        );
    }

    #[test]
    fn test_move_tab_in_direction_creates_new_pane_when_no_overlapping_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        // Split vertically: tab0 left, tab2 right
        app.split_pane_with_terminal(winter_model::layout::Direction::Vertical);
        let tab2 = app.layout().focused();
        // Split left pane horizontally: tab0 top, tab1 bottom
        app.layout_mut().focus(tab0);
        app.split_pane_with_terminal(winter_model::layout::Direction::Horizontal);
        let tab1 = app.layout().focused();

        // Focus pane 3 (tab2) and add another tab to it
        app.layout_mut().focus(tab2);
        app.new_tab();
        let tab3 = app.layout().focused();
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab2, tab3]);
        assert_eq!(app.layout().shown_tabs().len(), 3);

        // Move tab3 up: should create a new pane above tab2, NOT move into tab0 or tab1
        app.move_tab_in_direction(FocusDir::Up);
        assert_eq!(app.layout().shown_tabs().len(), 4, "created a 4th pane");
        assert_eq!(app.layout().focused(), tab3);
        assert!(!app.layout().tabs_in_pane(tab0).contains(&tab3));
        assert!(!app.layout().tabs_in_pane(tab1).contains(&tab3));
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab2]);
    }
}

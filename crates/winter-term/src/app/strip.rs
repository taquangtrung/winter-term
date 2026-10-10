//! The strip of tabs along the top of every pane: where each tab sits on it,
//! what a click there lands on, and the pane area left below it.

use crate::model::layout::{GroupArea, PaneId, Rect};

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How far, in pixels, a tab has to be dragged before letting go of it moves
/// it rather than counting as a click.
const DRAG_THRESHOLD_PX: f32 = 10.0;

/// How far apart, in pixels, two group edges can sit and still count as
/// shared, covering a rounding gap between neighbors.
const EDGE_TOLERANCE_PX: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// What a point on a pane strip lands on.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) enum StripHit {
    /// The close mark of this tab.
    Close(PaneId),
    /// The new-tab button of the strip whose shown tab is this one.
    NewTab(PaneId),
    /// No strip, or the blank end of one.
    #[default]
    None,
    /// This tab's label.
    Tab(PaneId),
}

/// One pane strip, ready to rasterize: its tabs, their pixel layout, the
/// area it covers, and what the pointer is over on it.
pub(crate) struct StripFrame {
    pub(crate) edges: winter_render::PaneStripEdges,
    pub(crate) hover: winter_render::PaneStripHit,
    pub(crate) layout: winter_render::PaneStripLayout,
    pub(crate) rect: winter_render::renderer::PaneRect,
    pub(crate) tabs: Vec<winter_render::PaneTab>,
}

// ========================================================================
// Functions
// ========================================================================

/// Which sides of the group at `rect` carry a divider across its strip. The
/// right side does whenever another of `groups` sits against it. The left side
/// does only when no group there has its strip at the same height, since such
/// a neighbor draws that divider as its own right side.
fn strip_edges(rect: Rect, groups: &[Rect]) -> winter_render::PaneStripEdges {
    let touches = |a: f32, b: f32| (a - b).abs() <= EDGE_TOLERANCE_PX;
    let overlaps = |a0: f32, a1: f32, b0: f32, b1: f32| a0.max(b0) < a1.min(b1);
    let mut edges = winter_render::PaneStripEdges::default();
    let mut left_aligned = false;
    for other in groups {
        let rows = overlaps(rect.y, rect.y + rect.height, other.y, other.y + other.height);
        let cols = overlaps(rect.x, rect.x + rect.width, other.x, other.x + other.width);
        let on_left = rows && touches(other.x + other.width, rect.x);
        edges.left |= on_left;
        left_aligned |= on_left && touches(other.y, rect.y);
        edges.right |= rows && touches(rect.x + rect.width, other.x);
        edges.top |= cols && touches(other.y + other.height, rect.y);
    }
    edges.left &= !left_aligned;
    edges
}

// ========================================================================
// App: pane strips
// ========================================================================

impl App {
    /// Pixel height of a pane strip.
    pub(crate) fn strip_height(&self) -> f32 {
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(super::APPROX_CELL_HEIGHT as f32);
        winter_render::pane_strip_height_px(ch)
    }

    /// Every tab group of the layout, with its strip's area.
    pub(crate) fn strip_areas(&self) -> Vec<(GroupArea, Rect)> {
        self.strip_areas_in(self.content_viewport())
    }

    /// The same, laid out within `viewport`.
    pub(crate) fn strip_areas_in(&self, viewport: Rect) -> Vec<(GroupArea, Rect)> {
        let height = self.strip_height();
        self.layout()
            .groups(viewport)
            .into_iter()
            .map(|area| {
                let rect = area.rect;
                let strip = Rect::new(rect.x, rect.y, rect.width, height.min(rect.height));
                (area, strip)
            })
            .collect()
    }

    /// Each shown tab paired with the area it draws in: its group's area
    /// below the strip.
    pub(crate) fn pane_rects(&self) -> Vec<(PaneId, Rect)> {
        self.pane_rects_in(self.content_viewport())
    }

    /// The same, laid out within `viewport`.
    pub(crate) fn pane_rects_in(&self, viewport: Rect) -> Vec<(PaneId, Rect)> {
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

    /// Every strip of the layout within `viewport`, laid out for drawing.
    pub(crate) fn strip_frames(&self, viewport: Rect, (cw, ch): (f32, f32)) -> Vec<StripFrame> {
        let areas = self.strip_areas_in(viewport);
        let rects: Vec<Rect> = areas.iter().map(|(area, _)| area.rect).collect();
        areas
            .into_iter()
            .map(|(area, rect)| {
                let tabs = self.pane_tabs(&area);
                let active = tabs.iter().position(|t| t.active).unwrap_or(0);
                let layout =
                    winter_render::layout_pane_strip(&tabs, rect.width, rect.height, cw, ch, active);
                let hover = match self.tabs.strip_hover {
                    StripHit::Tab(id) => winter_render::PaneStripHit::Tab(id.0),
                    StripHit::Close(id) => winter_render::PaneStripHit::CloseTab(id.0),
                    StripHit::NewTab(shown) if shown == area.active => {
                        winter_render::PaneStripHit::NewTab
                    }
                    _ => winter_render::PaneStripHit::None,
                };
                let mut edges = strip_edges(area.rect, &rects);
                // A strip against the top of the content has the title bar
                // above it, which is divided from it like any other neighbor.
                edges.top |= viewport.y > 0.0
                    && (area.rect.y - viewport.y).abs() <= EDGE_TOLERANCE_PX;
                StripFrame {
                    edges,
                    hover,
                    layout,
                    rect: Self::layout_rect_to_pane(rect),
                    tabs,
                }
            })
            .collect()
    }

    /// Build the GPU `PaneTab`s for group `area`.
    pub(crate) fn pane_tabs(&self, area: &GroupArea) -> Vec<winter_render::PaneTab> {
        area.members
            .iter()
            .map(|&pane| winter_render::PaneTab {
                active: pane == area.active,
                icon: self.tab_icon(pane),
                pane_id: pane.0,
                title: self.strip_title(pane),
            })
            .collect()
    }

    /// The SVG naming what `pane` holds: a terminal's, or its page's.
    fn tab_icon(&self, pane: PaneId) -> Option<Vec<u8>> {
        let name = match self.pages.get(&pane) {
            Some(slot) => slot.tab_icon(),
            None => crate::icons::terminal_icon(),
        };
        crate::icons::svg(&name)
    }

    /// How a strip names `pane`: the rename being typed while it is the
    /// focused tab, else its title.
    fn strip_title(&self, pane: PaneId) -> String {
        match &self.tabs.rename_input {
            Some(input) if pane == self.layout().focused() => format!("{input}\u{2502}"),
            _ => self.tab_title(pane),
        }
    }

    /// The strip under the point `(x, y)`, with its group.
    fn strip_at(&self, x: f32, y: f32) -> Option<(GroupArea, Rect)> {
        self.strip_areas().into_iter().find(|(_, rect)| {
            let r = Self::layout_rect_to_pane(*rect);
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        })
    }

    /// What the point `(x, y)` lands on among the pane strips.
    pub(crate) fn strip_hit(&self, x: f32, y: f32) -> StripHit {
        let Some((area, rect)) = self.strip_at(x, y) else {
            return StripHit::None;
        };
        let (cw, ch) = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size())
            .unwrap_or((super::APPROX_CELL_WIDTH as f32, super::APPROX_CELL_HEIGHT as f32));
        let tabs = self.pane_tabs(&area);
        let active_idx = tabs.iter().position(|t| t.active).unwrap_or(0);
        let layout = winter_render::layout_pane_strip(
            &tabs,
            rect.width,
            rect.height,
            cw,
            ch,
            active_idx,
        );
        let rel_x = x - rect.x;
        let rel_y = y - rect.y;
        match winter_render::hit_test_pane_strip(&layout, rel_x, rel_y, &tabs) {
            winter_render::PaneStripHit::Tab(id) => StripHit::Tab(PaneId(id)),
            winter_render::PaneStripHit::CloseTab(id) => StripHit::Close(PaneId(id)),
            winter_render::PaneStripHit::NewTab => StripHit::NewTab(area.active),
            _ => StripHit::None,
        }
    }

    /// Act on a press at `(x, y)` over a pane strip: show, close, or open a
    /// tab, or close one with the middle button. Returns whether the press
    /// landed on a strip at all, so the caller does not also treat it as a
    /// press on a pane.
    pub(crate) fn handle_strip_press(&mut self, x: f32, y: f32, middle: bool) -> bool {
        let Some((area, _)) = self.strip_at(x, y) else {
            return false;
        };
        match (self.strip_hit(x, y), middle) {
            (StripHit::Close(pane), _) | (StripHit::Tab(pane), true) => self.close_tab(pane),
            (StripHit::Tab(pane), false) => {
                self.show_tab(pane);
                self.tabs.drag_start = Some((pane, x));
            }
            (StripHit::NewTab(shown), false) => {
                self.layout_mut().focus(shown);
                self.new_tab();
            }
            // The blank end of a strip focuses its pane, as a press on the
            // pane would, without reaching what is drawn there.
            (StripHit::NewTab(_), true) | (StripHit::None, _) => self.show_tab(area.active),
        }
        self.dirty = true;
        true
    }

    /// Track what the pointer is over on the pane strips, for hover marks.
    pub(crate) fn update_strip_hover(&mut self, x: f32, y: f32) {
        let hit = self.strip_hit(x, y);
        if hit != self.tabs.strip_hover {
            self.tabs.strip_hover = hit;
            self.dirty = true;
        }
    }

    /// On release, move a dragged tab to where it was let go, when that is
    /// another tab of the same strip and the pointer moved far enough to mean
    /// a drag rather than a click.
    pub(crate) fn finalize_tab_drag(&mut self) {
        let Some((pane, start_x)) = self.tabs.drag_start.take() else {
            return;
        };
        let (x, y) = self.pointer.cursor_pos;
        if (x - start_x).abs() < DRAG_THRESHOLD_PX {
            return;
        }
        if let StripHit::Tab(target) | StripHit::Close(target) = self.strip_hit(x, y) {
            self.move_tab_to(pane, target);
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
    fn test_a_strip_beside_a_strip_at_another_height_draws_its_own_left_divider() {
        // Left group full height; right column split into top and bottom.
        let left = Rect::new(0.0, 0.0, 100.0, 200.0);
        let top_right = Rect::new(100.0, 0.0, 100.0, 100.0);
        let bottom_right = Rect::new(100.0, 100.0, 100.0, 100.0);
        let groups = [left, top_right, bottom_right];

        let left_edges = strip_edges(left, &groups);
        let top_edges = strip_edges(top_right, &groups);
        let bottom_edges = strip_edges(bottom_right, &groups);

        assert!(left_edges.right && !left_edges.left);
        assert!(!top_edges.left, "the left strip draws this one");
        assert!(bottom_edges.left, "no strip at its height to the left");
        assert!(bottom_edges.top && !top_edges.top);
    }
}

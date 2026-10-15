//! The strip of tabs along the top of every pane: where each tab sits on it,
//! what a click there lands on, and the pane area left below it.

use crate::model::layout::{GroupArea, Rect, TabId};

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// How far, in pixels, a tab has to be dragged before letting go of it moves
/// it rather than counting as a click.
pub(crate) const DRAG_THRESHOLD_PX: f32 = 10.0;

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
    Close(TabId),
    /// The new-tab button of the strip whose shown tab is this one.
    NewTab(TabId),
    /// No strip, or the blank end of one.
    #[default]
    None,
    /// This tab's label.
    Tab(TabId),
}

/// One pane strip, ready to rasterize: its tabs, their pixel layout, the
/// area it covers, and what the pointer is over on it.
pub(crate) struct StripFrame {
    pub(crate) drop_target: winter_render::PaneStripDropTarget,
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
        let rows = overlaps(
            rect.y,
            rect.y + rect.height,
            other.y,
            other.y + other.height,
        );
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
    pub(crate) fn pane_rects(&self) -> Vec<(TabId, Rect)> {
        self.pane_rects_in(self.content_viewport())
    }

    /// The same, laid out within `viewport`.
    pub(crate) fn pane_rects_in(&self, viewport: Rect) -> Vec<(TabId, Rect)> {
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
        let (cx, cy) = self.pointer.cursor_pos;
        let drag_info = self.tabs.drag_start.filter(|(_, sx, sy)| {
            ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt() >= DRAG_THRESHOLD_PX
        });
        areas
            .into_iter()
            .map(|(area, rect)| {
                let tabs = self.pane_tabs(&area);
                let active = tabs.iter().position(|t| t.active).unwrap_or(0);
                let layout = winter_render::layout_pane_strip(
                    &tabs,
                    rect.width,
                    rect.height,
                    cw,
                    ch,
                    active,
                );
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
                edges.top |=
                    viewport.y > 0.0 && (area.rect.y - viewport.y).abs() <= EDGE_TOLERANCE_PX;

                let drop_target = if let Some((drag_tab, _, _)) = drag_info {
                    let r = Self::layout_rect_to_pane(rect);
                    let over_strip =
                        cx >= r.x && cx < r.x + r.width && cy >= r.y && cy < r.y + r.height;
                    if over_strip {
                        let rel_x = cx - r.x;
                        let mut found_ix = None;
                        for tab_reg in &layout.tabs {
                            if tab_reg.w <= 0.0 {
                                continue;
                            }
                            if rel_x < tab_reg.x + tab_reg.w {
                                if rel_x < tab_reg.x + tab_reg.w / 2.0 {
                                    found_ix = Some(tab_reg.x);
                                } else {
                                    found_ix = Some(tab_reg.x + tab_reg.w);
                                }
                                break;
                            }
                        }
                        let ix = found_ix.unwrap_or_else(|| {
                            layout.tabs.last().map(|t| t.x + t.w).unwrap_or(0.0)
                        });
                        winter_render::PaneStripDropTarget {
                            indicator_x: Some(ix),
                            is_target_pane: false,
                        }
                    } else if let Some((target_pane, _)) = self.pane_at_pixel(cx, cy) {
                        if area.members.contains(&target_pane) && !area.members.contains(&drag_tab)
                        {
                            winter_render::PaneStripDropTarget {
                                indicator_x: Some(
                                    layout.tabs.last().map(|t| t.x + t.w).unwrap_or(0.0),
                                ),
                                is_target_pane: true,
                            }
                        } else {
                            winter_render::PaneStripDropTarget::default()
                        }
                    } else {
                        winter_render::PaneStripDropTarget::default()
                    }
                } else {
                    winter_render::PaneStripDropTarget::default()
                };

                StripFrame {
                    drop_target,
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
        let focused = self.layout().focused();
        let (cx, cy) = self.pointer.cursor_pos;
        let dragging_tab = self.tabs.drag_start.and_then(|(pane, sx, sy)| {
            let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
            if dist >= DRAG_THRESHOLD_PX {
                Some(pane)
            } else {
                None
            }
        });
        area.members
            .iter()
            .map(|&pane| winter_render::PaneTab {
                active: pane == area.active,
                dragging: dragging_tab == Some(pane),
                focused: pane == focused,
                icon: self.tab_icon(pane),
                tab_id: pane.0,
                title: self.strip_title(pane),
            })
            .collect()
    }

    /// The SVG naming what `pane` holds: a terminal's, or its page's.
    fn tab_icon(&self, tab: TabId) -> Option<Vec<u8>> {
        let name = match self.pages.get(&tab) {
            Some(slot) => slot.tab_icon(),
            None => crate::icons::terminal_icon(),
        };
        crate::icons::svg(&name)
    }

    /// How a strip names `pane`: the rename being typed while it is the
    /// focused tab, else its title.
    pub(super) fn strip_title(&self, tab: TabId) -> String {
        let title = match &self.tabs.rename_input {
            Some(input) if tab == self.layout().focused() => format!("{input}\u{2502}"),
            _ => self.tab_title(tab),
        };
        match self.spinning.contains(&tab) {
            true => format!("{} {title}", super::busy::spinner_glyph(self.spinner_frame)),
            false => title,
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
        let (cw, ch) = self.renderer.as_ref().map(|r| r.cell_size()).unwrap_or((
            super::APPROX_CELL_WIDTH as f32,
            super::APPROX_CELL_HEIGHT as f32,
        ));
        let tabs = self.pane_tabs(&area);
        let active_idx = tabs.iter().position(|t| t.active).unwrap_or(0);
        let layout =
            winter_render::layout_pane_strip(&tabs, rect.width, rect.height, cw, ch, active_idx);
        let rel_x = x - rect.x;
        let rel_y = y - rect.y;
        match winter_render::hit_test_pane_strip(&layout, rel_x, rel_y, &tabs) {
            winter_render::PaneStripHit::Tab(id) => StripHit::Tab(TabId(id)),
            winter_render::PaneStripHit::CloseTab(id) => StripHit::Close(TabId(id)),
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
                self.tabs.drag_start = Some((pane, x, y));
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
    /// another tab of the same strip or across panes, and the pointer moved
    /// far enough to mean a drag rather than a click.
    pub(crate) fn finalize_tab_drag(&mut self) {
        let Some((pane, start_x, start_y)) = self.tabs.drag_start.take() else {
            return;
        };
        let (x, y) = self.pointer.cursor_pos;
        let dist = ((x - start_x).powi(2) + (y - start_y).powi(2)).sqrt();
        if dist < DRAG_THRESHOLD_PX {
            return;
        }
        match self.strip_hit(x, y) {
            StripHit::Tab(target) | StripHit::Close(target) if target == pane => {}
            StripHit::Tab(target) | StripHit::Close(target) => {
                self.move_tab_to(pane, target);
            }
            _ => {
                if let Some((area, _)) = self.strip_at(x, y) {
                    self.move_tab_to_group(pane, area.active);
                } else if let Some((target_pane, _)) = self.pane_at_pixel(x, y) {
                    if !self.layout().same_group(pane, target_pane) {
                        self.move_tab_to_group(pane, target_pane);
                    }
                }
            }
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

    #[test]
    fn test_pane_tabs_marks_active_tab_in_unfocused_group_as_not_focused() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.split_pane(crate::model::layout::Direction::Horizontal);
        let second = app.layout().focused();
        assert_ne!(first, second);

        let first_area = GroupArea {
            active: first,
            members: vec![first],
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
        };
        let first_tabs = app.pane_tabs(&first_area);
        assert_eq!(first_tabs.len(), 1);
        assert!(first_tabs[0].active, "tab is active in its group");
        assert!(
            !first_tabs[0].focused,
            "tab in unfocused group should not be marked focused"
        );

        let second_area = GroupArea {
            active: second,
            members: vec![second],
            rect: Rect::new(0.0, 50.0, 100.0, 50.0),
        };
        let second_tabs = app.pane_tabs(&second_area);
        assert_eq!(second_tabs.len(), 1);
        assert!(second_tabs[0].active, "tab is active in its group");
        assert!(
            second_tabs[0].focused,
            "tab in focused group should be marked focused"
        );
    }

    #[test]
    fn test_finalize_tab_drag_reorders_in_same_strip() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().members(), vec![tab0, tab1]);

        // Drag tab1 back to tab0's position
        let strip = app.strip_areas()[0].1;
        let strip_y = strip.y + strip.height / 2.0;
        app.tabs.drag_start = Some((tab1, 100.0, strip_y));
        app.pointer.cursor_pos = (20.0, strip_y);
        app.finalize_tab_drag();
        assert_eq!(app.layout().members(), vec![tab1, tab0]);
    }

    #[test]
    fn test_finalize_tab_drag_across_panes_into_other_strip() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        app.split_pane(crate::model::layout::Direction::Horizontal);
        let tab2 = app.layout().focused();

        // Pane 1 has [tab0, tab1], Pane 2 has [tab2]
        assert_eq!(app.layout().panes().len(), 2);
        assert_eq!(app.layout().group_members(tab0), vec![tab0, tab1]);
        assert_eq!(app.layout().group_members(tab2), vec![tab2]);

        // Drag tab1 into pane 2's strip over tab2
        let pane2_rect = app.pane_rect(tab2).expect("pane 2 rect");
        let strip2_y = pane2_rect.y - 10.0;
        app.tabs.drag_start = Some((tab1, 20.0, 5.0));
        app.pointer.cursor_pos = (pane2_rect.x + 20.0, strip2_y);

        app.finalize_tab_drag();
        assert_eq!(app.layout().panes().len(), 2);
        assert_eq!(app.layout().group_members(tab0), vec![tab0]);
        assert_eq!(app.layout().group_members(tab2), vec![tab1, tab2]);
        assert_eq!(app.layout().focused(), tab1);
    }

    #[test]
    fn test_finalize_tab_drag_into_other_pane_body_collapses_empty_source_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane(crate::model::layout::Direction::Horizontal);
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().panes().len(), 2);

        // Drag tab0 into tab1's pane content area
        let pane1_rect = app.pane_rect(tab1).expect("pane 1 rect");
        let drop_x = pane1_rect.x + pane1_rect.width / 2.0;
        let drop_y = pane1_rect.y + pane1_rect.height / 2.0;

        app.tabs.drag_start = Some((tab0, 20.0, 5.0));
        app.pointer.cursor_pos = (drop_x, drop_y);

        app.finalize_tab_drag();
        // Since tab0 was the only tab in its pane, that pane collapses!
        assert_eq!(app.layout().panes().len(), 1);
        assert_eq!(app.layout().group_members(tab1), vec![tab1, tab0]);
        assert_eq!(app.layout().focused(), tab0);
    }

    #[test]
    fn test_strip_frames_drop_target_and_dragging_feedback() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane(crate::model::layout::Direction::Horizontal);
        let tab1 = app.layout().focused();
        let viewport = Rect::new(0.0, 0.0, 800.0, 600.0);

        // 1. Idle state: no dragging, no drop indicator
        let frames = app.strip_frames(viewport, (10.0, 20.0));
        assert_eq!(frames.len(), 2);
        assert!(!frames[0].tabs[0].dragging);
        assert_eq!(frames[0].drop_target.indicator_x, None);
        assert!(!frames[0].drop_target.is_target_pane);

        // 2. Active drag of tab0 over its own strip
        let strip0 = app.strip_areas_in(viewport)[0].1;
        app.tabs.drag_start = Some((tab0, 10.0, 10.0));
        app.pointer.cursor_pos = (strip0.x + 30.0, strip0.y + 10.0);
        let frames = app.strip_frames(viewport, (10.0, 20.0));
        let frame0 = frames.iter().find(|f| f.tabs[0].tab_id == tab0.0).unwrap();
        assert!(
            frame0.tabs[0].dragging,
            "dragged tab should be marked dragging"
        );
        assert!(
            frame0.drop_target.indicator_x.is_some(),
            "strip being hovered should show insertion indicator"
        );
        assert!(!frame0.drop_target.is_target_pane);

        // 3. Active drag of tab0 over tab1's pane content body
        let pane1_rect = app.pane_rect(tab1).expect("pane 1 rect");
        app.pointer.cursor_pos = (
            pane1_rect.x + pane1_rect.width / 2.0,
            pane1_rect.y + pane1_rect.height / 2.0,
        );
        let frames = app.strip_frames(viewport, (10.0, 20.0));
        let frame1 = frames.iter().find(|f| f.tabs[0].tab_id == tab1.0).unwrap();
        assert!(
            frame1.drop_target.is_target_pane,
            "hovered target pane body should trigger target pane feedback"
        );
        assert!(
            frame1.drop_target.indicator_x.is_some(),
            "target pane strip should show insertion indicator at the end"
        );
    }

    #[test]
    fn test_move_tab_in_direction_moves_tab_to_adjacent_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        // Split vertically (tab0 on left, tab1 on right)
        app.split_pane(crate::model::layout::Direction::Vertical);
        let tab1 = app.layout().focused();
        assert_ne!(tab0, tab1);
        assert_eq!(app.layout().panes().len(), 2);

        // Focus left pane (tab0) and add another tab (tab2) to tab0's pane
        app.layout_mut().focus(tab0);
        app.new_tab();
        let tab2 = app.layout().focused();
        assert_ne!(tab2, tab0);
        assert_eq!(app.layout().group_members(tab2), vec![tab0, tab2]);

        // Move current tab (tab2) to the right pane
        app.move_tab_in_direction(crate::model::layout::FocusDir::Right);
        // tab2 is now in tab1's group
        assert_eq!(app.layout().group_members(tab2), vec![tab1, tab2]);
        assert_eq!(app.layout().group_members(tab0), vec![tab0]);
        assert_eq!(app.layout().focused(), tab2);

        // Move tab2 back to the left pane
        app.move_tab_in_direction(crate::model::layout::FocusDir::Left);
        assert_eq!(app.layout().group_members(tab2), vec![tab0, tab2]);
        assert_eq!(app.layout().group_members(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);
    }
}

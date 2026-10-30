//! The strip of tabs along the top of every pane: where each tab sits on it,
//! what a click there lands on, and the pane area left below it. Which tabs
//! are busy, and the spinner their titles wear, lives in [`busy`].

pub(in crate::window) mod busy;

use std::sync::Arc;

use winter_render::{PaneStripLayout, PaneTab, Region};

use crate::model::layout::{FocusDir, PaneArea, Rect, TabId};
use crate::model::paint::IconKind;
use crate::model::spinner;

use crate::window::{App, APPROX_CELL_HEIGHT, APPROX_CELL_WIDTH};

// ========================================================================
// Constants
// ========================================================================

/// How far, in pixels, a tab has to be dragged before letting go of it moves
/// it rather than counting as a click.
pub(in crate::window) const DRAG_THRESHOLD_PX: f32 = 10.0;

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

/// One pane strip, ready to rasterize: its tabs, their pixel layout, the
/// area it covers, and what the pointer is over on it.
pub(in crate::window) struct StripFrame {
    pub(in crate::window) drop_target: winter_render::PaneStripDropTarget,
    pub(in crate::window) hover: winter_render::PaneStripHit,
    pub(in crate::window) layout: winter_render::PaneStripLayout,
    pub(in crate::window) rect: winter_render::renderer::PaneRect,
    pub(in crate::window) tabs: Vec<winter_render::PaneTab>,
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
    /// Pixel height of a pane strip.
    pub(in crate::window) fn strip_height(&self) -> f32 {
        let ch = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size().1)
            .unwrap_or(APPROX_CELL_HEIGHT as f32);
        winter_render::pane_strip_height_px(ch)
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

    /// Each shown tab paired with the area it draws in: its pane's area
    /// below the strip.
    pub(in crate::window) fn pane_rects(&self) -> Vec<(TabId, Rect)> {
        self.pane_rects_in(self.content_viewport())
    }

    /// The same, laid out within `viewport`.
    pub(in crate::window) fn pane_rects_in(&self, viewport: Rect) -> Vec<(TabId, Rect)> {
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
    pub(in crate::window) fn strip_frames(
        &self,
        viewport: Rect,
        (cw, ch): (f32, f32),
    ) -> Vec<StripFrame> {
        let areas = self.strip_areas_in(viewport);
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
                    } else if let Some((target_pane, pr)) = self.pane_at_pixel(cx, cy) {
                        if area.members.contains(&target_pane)
                            && !area.members.contains(&drag_tab)
                            && !is_pane_edge_zone(cx, cy, &pr)
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
                    hover,
                    layout,
                    rect: Self::layout_rect_to_pane(rect),
                    tabs,
                }
            })
            .collect()
    }

    /// Build the GPU `PaneTab`s for pane `area`.
    pub(in crate::window) fn pane_tabs(&self, area: &PaneArea) -> Vec<winter_render::PaneTab> {
        let active = self.active_pane();
        let (cx, cy) = self.pointer.cursor_pos;
        let dragging_tab = self.tabs.drag_start.and_then(|(tab, sx, sy)| {
            let dist = ((cx - sx).powi(2) + (cy - sy).powi(2)).sqrt();
            if dist >= DRAG_THRESHOLD_PX {
                Some(tab)
            } else {
                None
            }
        });
        area.members
            .iter()
            .map(|&tab| winter_render::PaneTab {
                active: tab == area.active,
                dragging: dragging_tab == Some(tab),
                focused: active == Some(tab),
                icon: self.tab_icon(tab),
                modified: self
                    .open_tabs
                    .editor(&tab)
                    .is_some_and(|slot| slot.editor.is_dirty()),
                tab_id: tab.0,
                title: self.strip_title(tab),
            })
            .collect()
    }

    /// The SVG naming what `tab` holds.
    fn tab_icon(&self, tab: TabId) -> Option<Arc<[u8]>> {
        crate::icons::shared_svg(&self.tab_icon_kind(tab))
    }

    /// What `tab` holds, as its icon depicts it: its editor, or a terminal
    /// by the program running in it.
    pub(in crate::window) fn tab_icon_kind(&self, tab: TabId) -> IconKind {
        if let Some(slot) = self.open_tabs.editor(&tab) {
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
        let title = match self.chrome.rename_input() {
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
        };
        match self.busy.spinning.contains(&tab) {
            true => format!("{} {title}", spinner::glyph(self.busy.frame)),
            false => title,
        }
    }

    /// The strip under the point `(x, y)`, with its pane.
    fn strip_at(&self, x: f32, y: f32) -> Option<(PaneArea, Rect)> {
        self.strip_areas().into_iter().find(|(_, rect)| {
            let r = Self::layout_rect_to_pane(*rect);
            x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
        })
    }

    /// What the point `(x, y)` lands on among the pane strips.
    pub(in crate::window) fn strip_hit(&self, x: f32, y: f32) -> StripHit {
        let Some((area, rect, tabs, layout)) = self.strip_laid_out(x, y) else {
            return StripHit::None;
        };
        let rel_x = x - rect.x;
        let rel_y = y - rect.y;
        match winter_render::hit_test_pane_strip(&layout, rel_x, rel_y, &tabs) {
            winter_render::PaneStripHit::Tab(id) => StripHit::Tab(TabId(id)),
            winter_render::PaneStripHit::CloseTab(id) => StripHit::Close(TabId(id)),
            winter_render::PaneStripHit::NewTab => StripHit::NewTab(area.active),
            _ => StripHit::None,
        }
    }

    /// Where tab `tab` is drawn, on the strip under `(x, y)`, in window
    /// pixels.
    pub(in crate::window) fn strip_tab_region(&self, tab: TabId, x: f32, y: f32) -> Option<Region> {
        let (_, rect, tabs, layout) = self.strip_laid_out(x, y)?;
        let at = tabs.iter().position(|t| t.tab_id == tab.0)?;
        let region = layout.tabs.get(at)?;
        Some(Region {
            x: rect.x + region.x,
            y: rect.y + region.y,
            ..*region
        })
    }

    /// The strip under `(x, y)`: its pane, where it is, its tabs, and how
    /// they are laid out on it.
    fn strip_laid_out(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(PaneArea, Rect, Vec<PaneTab>, PaneStripLayout)> {
        let (area, rect) = self.strip_at(x, y)?;
        let (cw, ch) = self
            .renderer
            .as_ref()
            .map(|r| r.cell_size())
            .unwrap_or((APPROX_CELL_WIDTH as f32, APPROX_CELL_HEIGHT as f32));
        let tabs = self.pane_tabs(&area);
        let active_idx = tabs.iter().position(|t| t.active).unwrap_or(0);
        let layout =
            winter_render::layout_pane_strip(&tabs, rect.width, rect.height, cw, ch, active_idx);
        Some((area, rect, tabs, layout))
    }

    /// Act on a press at `(x, y)` over a pane strip: show, close, or open a
    /// tab, or close one with the middle button. Returns whether the press
    /// landed on a strip at all, so the caller does not also treat it as a
    /// press on a pane.
    pub(in crate::window) fn handle_strip_press(&mut self, x: f32, y: f32, middle: bool) -> bool {
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
                self.set_focus(shown);
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
    pub(in crate::window) fn update_strip_hover(&mut self, x: f32, y: f32) {
        let hit = self.strip_hit(x, y);
        if hit != self.tabs.strip_hover {
            self.tabs.strip_hover = hit;
            self.dirty = true;
        }
    }

    /// On release, move a dragged tab to where it was let go, when that is
    /// another tab of the same strip or across panes, and the pointer moved
    /// far enough to mean a drag rather than a click. Dropping onto an edge
    /// of a pane (or into the body of the same pane) splits that pane to create
    /// a new pane holding the dragged tab.
    pub(in crate::window) fn finalize_tab_drag(&mut self) {
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
                    self.move_tab_to_pane(pane, area.active);
                } else {
                    let target_info = self.pane_at_pixel(x, y).or_else(|| {
                        let vp = self.content_viewport();
                        let clamped_x = x.clamp(vp.x, vp.x + vp.width.max(1.0) - 1.0);
                        let clamped_y = y.clamp(vp.y, vp.y + vp.height.max(1.0) - 1.0);
                        self.pane_at_pixel(clamped_x, clamped_y)
                    });
                    if let Some((target_pane, pr)) = target_info {
                        let same = self.layout().same_pane(pane, target_pane);
                        let edge = is_pane_edge_zone(x, y, &pr);
                        let direction = pane_drop_split_dir(x, y, &pr);
                        if edge || same {
                            self.split_tab_to_new_pane(pane, target_pane, direction);
                        } else {
                            self.move_tab_to_pane(pane, target_pane);
                        }
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
    use crate::window::input::overlay::{Overlay, RenameInput};

    #[test]
    fn test_pane_tabs_marks_active_tab_in_unfocused_group_as_not_focused() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.split_pane_with_terminal(crate::model::layout::Direction::Horizontal);
        let second = app.layout().focused();
        assert_ne!(first, second);

        let first_area = PaneArea {
            active: first,
            members: vec![first],
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
        };
        let first_tabs = app.pane_tabs(&first_area);
        assert_eq!(first_tabs.len(), 1);
        assert!(first_tabs[0].active, "tab is active in its pane");
        assert!(
            !first_tabs[0].focused,
            "tab in unfocused pane should not be marked focused"
        );

        let second_area = PaneArea {
            active: second,
            members: vec![second],
            rect: Rect::new(0.0, 50.0, 100.0, 50.0),
        };
        let second_tabs = app.pane_tabs(&second_area);
        assert_eq!(second_tabs.len(), 1);
        assert!(second_tabs[0].active, "tab is active in its pane");
        assert!(
            second_tabs[0].focused,
            "tab in focused pane should be marked focused"
        );
    }

    #[test]
    fn test_pane_tabs_mark_no_tab_focused_while_a_popup_is_open() {
        let mut app = App::new();
        let tab = app.layout().focused();
        app.open_overlay(Overlay::Rename(RenameInput::new(tab, "")));
        let area = PaneArea {
            active: tab,
            members: vec![tab],
            rect: Rect::new(0.0, 0.0, 100.0, 50.0),
        };

        let tabs = app.pane_tabs(&area);

        assert!(tabs[0].active, "the tab stays active in its pane");
        assert!(!tabs[0].focused, "the popup has the keyboard, not the pane");
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
        app.split_pane_with_terminal(crate::model::layout::Direction::Horizontal);
        let tab2 = app.layout().focused();

        // Pane 1 has [tab0, tab1], Pane 2 has [tab2]
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0, tab1]);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab2]);

        // Drag tab1 into pane 2's strip over tab2
        let pane2_rect = app.pane_rect(tab2).expect("pane 2 rect");
        let strip2_y = pane2_rect.y - 10.0;
        app.tabs.drag_start = Some((tab1, 20.0, 5.0));
        app.pointer.cursor_pos = (pane2_rect.x + 20.0, strip2_y);

        app.finalize_tab_drag();
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0]);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab1, tab2]);
        assert_eq!(app.layout().focused(), tab1);
    }

    #[test]
    fn test_finalize_tab_drag_into_other_pane_body_collapses_empty_source_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane_with_terminal(crate::model::layout::Direction::Horizontal);
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 2);

        // Drag tab0 into tab1's pane content area
        let pane1_rect = app.pane_rect(tab1).expect("pane 1 rect");
        let drop_x = pane1_rect.x + pane1_rect.width / 2.0;
        let drop_y = pane1_rect.y + pane1_rect.height / 2.0;

        app.tabs.drag_start = Some((tab0, 20.0, 5.0));
        app.pointer.cursor_pos = (drop_x, drop_y);

        app.finalize_tab_drag();
        // Since tab0 was the only tab in its pane, that pane collapses!
        assert_eq!(app.layout().shown_tabs().len(), 1);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1, tab0]);
        assert_eq!(app.layout().focused(), tab0);
    }

    #[test]
    fn test_strip_frames_drop_target_and_dragging_feedback() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane_with_terminal(crate::model::layout::Direction::Horizontal);
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
        app.split_pane_with_terminal(crate::model::layout::Direction::Vertical);
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
        app.move_tab_in_direction(crate::model::layout::FocusDir::Right);
        // tab2 is now in tab1's pane
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab1, tab2]);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0]);
        assert_eq!(app.layout().focused(), tab2);

        // Move tab2 back to the left pane
        app.move_tab_in_direction(crate::model::layout::FocusDir::Left);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab0, tab2]);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);

        // Move tab2 left again: there is no pane to the left, so tab2 becomes a new pane on the left
        app.move_tab_in_direction(crate::model::layout::FocusDir::Left);
        assert_eq!(app.layout().shown_tabs().len(), 3);
        assert_eq!(app.layout().tabs_in_pane(tab2), vec![tab2]);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0]);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);
    }

    #[test]
    fn test_finalize_tab_drag_into_own_pane_splits_into_new_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 1);
        assert_eq!(app.layout().tabs_in_pane(tab0), vec![tab0, tab1]);

        let pane_rect = app.pane_rect(tab1).expect("pane rect");
        // Drag tab1 to the bottom 10% of the pane body (split Down)
        app.tabs.drag_start = Some((tab1, 20.0, 5.0));
        app.pointer.cursor_pos = (
            pane_rect.x + pane_rect.width / 2.0,
            pane_rect.y + pane_rect.height * 0.9,
        );

        app.finalize_tab_drag();
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().focused(), tab1);
        let vp = Rect::new(0.0, 0.0, 800.0, 600.0);
        assert_eq!(app.layout().pane_in_direction(FocusDir::Up, vp), Some(tab0));

        // Move tab1 back into tab0's pane
        app.move_tab_to_pane(tab1, tab0);
        assert_eq!(app.layout().shown_tabs().len(), 1);

        let pane_rect = app.pane_rect(tab1).expect("pane rect");
        // Drag tab1 to the right 10% of the pane body (split Right)
        app.tabs.drag_start = Some((tab1, 20.0, 5.0));
        app.pointer.cursor_pos = (
            pane_rect.x + pane_rect.width * 0.9,
            pane_rect.y + pane_rect.height / 2.0,
        );

        app.finalize_tab_drag();
        assert_eq!(app.layout().shown_tabs().len(), 2);
        assert_eq!(app.layout().focused(), tab1);
        assert_eq!(
            app.layout().pane_in_direction(FocusDir::Left, vp),
            Some(tab0)
        );
    }

    #[test]
    fn test_finalize_tab_drag_into_other_pane_edge_splits_target_pane() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.split_pane_with_terminal(crate::model::layout::Direction::Vertical);
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 2);

        let pane0_rect = app.pane_rect(tab0).expect("pane 0 rect");
        // Drag tab1 to the left 10% of pane0 (split Left of pane0)
        app.tabs.drag_start = Some((tab1, 500.0, 5.0));
        app.pointer.cursor_pos = (
            pane0_rect.x + pane0_rect.width * 0.1,
            pane0_rect.y + pane0_rect.height / 2.0,
        );

        app.finalize_tab_drag();
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
        app.tabs.drag_start = Some((tab2, 50.0, 5.0));
        app.pointer.cursor_pos = (
            pane0_rect_updated.x + pane0_rect_updated.width / 2.0,
            pane0_rect_updated.y + pane0_rect_updated.height * 0.9,
        );

        app.finalize_tab_drag();
        assert_eq!(app.layout().shown_tabs().len(), 3);
        assert_eq!(app.layout().tabs_in_pane(tab1), vec![tab1]);
        assert_eq!(app.layout().focused(), tab2);
        assert_eq!(app.layout().pane_in_direction(FocusDir::Up, vp), Some(tab0));
    }

    #[test]
    fn test_finalize_tab_drag_past_window_edge_clamps_and_splits() {
        let mut app = App::new();
        let tab0 = app.layout().focused();
        app.new_tab();
        let tab1 = app.layout().focused();
        assert_eq!(app.layout().shown_tabs().len(), 1);

        // Drag tab1 far past the right edge of the window
        app.tabs.drag_start = Some((tab1, 20.0, 5.0));
        app.pointer.cursor_pos = (999.0, 300.0);

        app.finalize_tab_drag();
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
        app.split_pane_with_terminal(crate::model::layout::Direction::Vertical);
        let tab2 = app.layout().focused();
        // Split left pane horizontally: tab0 top, tab1 bottom
        app.layout_mut().focus(tab0);
        app.split_pane_with_terminal(crate::model::layout::Direction::Horizontal);
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

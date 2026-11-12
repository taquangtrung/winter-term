//! The pane strips, drawn by misty: one [`TabStrip`] for each pane, kept in
//! step with the layout before each frame (its tabs, their order, the one
//! shown, titles, icons, unsaved and busy marks), and the pointer handed to
//! them. A click shows a tab, its close mark or a middle click closes it,
//! the new-tab button opens one, and a tab dragged along its strip moves
//! there; dragged off it, the tab goes to the strip or pane it is let go
//! over (see [`App::drop_tab`]).

use std::time::Instant;

use misty::{
    draw_drop_zone, DropZone, EventCx, Frame, Input, Point, TabDrag, TabId as MistyTab, TabStrip,
    TabStripHit, TabStripResponse, Widget,
};

use winter_model::layout::{FocusDir, Rect, TabId};

use super::{StripHit, TabDrop};
use crate::window::misty_theme::misty_theme;
use crate::window::App;

// ========================================================================
// Data Structures
// ========================================================================

/// One pane's strip: misty's widget, the Winter tab behind each of its
/// tabs, and where it was laid out.
pub(in crate::window) struct PaneStrip {
    /// Where the strip sits, along its pane's top.
    rect: Rect,
    /// Misty's strip.
    strip: TabStrip,
    /// Each Winter tab on the strip and misty's tab for it, in strip order.
    tabs: Vec<(TabId, MistyTab)>,
}

/// What a strip shows of one tab.
struct StripTab {
    /// Whether it is busy long enough to show a spinner.
    busy: bool,
    /// The bundled icon naming what it holds.
    icon: Option<String>,
    /// Whether it holds unsaved changes.
    modified: bool,
    /// Winter's tab.
    tab: TabId,
    /// Its place and title, with a rename's caret while one is typed.
    title: String,
}

/// A tab dragged off its strip, and where the pointer is.
#[derive(Clone, Copy, Debug)]
pub(in crate::window) struct TabDragging {
    /// Where the pointer is.
    point: Point,
    /// The tab dragged.
    tab: TabId,
}

// ========================================================================
// PaneStrip
// ========================================================================

impl PaneStrip {
    /// An empty strip.
    fn new() -> Self {
        Self {
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            strip: TabStrip::new(),
            tabs: Vec::new(),
        }
    }

    /// Whether Winter's `tab` is on the strip.
    fn has(&self, tab: TabId) -> bool {
        self.tabs.iter().any(|(winter, _)| *winter == tab)
    }

    /// Winter's tab behind misty's `id`.
    fn winter(&self, id: MistyTab) -> Option<TabId> {
        self.tabs
            .iter()
            .find(|(_, misty)| *misty == id)
            .map(|(winter, _)| *winter)
    }

    /// Misty's tab for Winter's `tab`.
    fn misty(&self, tab: TabId) -> Option<MistyTab> {
        self.tabs
            .iter()
            .find(|(winter, _)| *winter == tab)
            .map(|(_, misty)| *misty)
    }

    /// Whether `(x, y)` is on the strip.
    fn contains(&self, x: f32, y: f32) -> bool {
        let rect = self.rect;
        x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
    }

    /// Bring the strip in step with `shown`, in order, `active` the tab
    /// shown and the strip lit as the focused pane's when `focused`.
    fn sync(&mut self, shown: &[StripTab], active: TabId, focused: bool) {
        let gone: Vec<MistyTab> = self
            .tabs
            .iter()
            .filter(|(winter, _)| !shown.iter().any(|tab| tab.tab == *winter))
            .map(|(_, misty)| *misty)
            .collect();
        for id in gone {
            self.strip.remove(id);
        }
        self.tabs
            .retain(|(winter, _)| shown.iter().any(|tab| tab.tab == *winter));
        for (index, tab) in shown.iter().enumerate() {
            let id = match self.misty(tab.tab) {
                Some(id) => {
                    self.strip.move_tab(id, index);
                    id
                }
                None => {
                    let id = self.strip.insert(index, tab.title.as_str());
                    self.tabs.push((tab.tab, id));
                    id
                }
            };
            self.strip.set_title(id, tab.title.as_str());
            self.strip.set_icon(id, tab.icon.clone());
            self.strip.set_modified(id, tab.modified);
            self.strip.set_busy(id, tab.busy);
        }
        let names: Vec<String> = shown.iter().filter_map(|tab| tab.icon.clone()).collect();
        crate::icons::add_to(self.strip.icons_mut(), &names);
        self.tabs.sort_by_key(|(winter, _)| {
            shown
                .iter()
                .position(|tab| tab.tab == *winter)
                .unwrap_or(usize::MAX)
        });
        if let Some(id) = self.misty(active) {
            if self.strip.active() != Some(id) {
                self.strip.activate(id);
            }
        }
        self.strip.set_focused(focused);
    }
}

// ========================================================================
// App: the strips misty draws
// ========================================================================

impl App {
    /// Bring a strip for each pane in step with the layout, reusing the
    /// strip that held the pane's tabs last frame, so hover, scroll, and a
    /// drag under way carry over.
    pub(in crate::window) fn sync_strips(&mut self) {
        let areas = self.strip_areas();
        let focused_pane = self.active_pane();
        let mut old = std::mem::take(&mut self.chrome.strips);
        let mut strips = Vec::with_capacity(areas.len());
        for (area, rect) in areas {
            let reuse = old
                .iter()
                .position(|strip| strip.has(area.active))
                .or_else(|| {
                    old.iter()
                        .position(|strip| area.members.iter().any(|tab| strip.has(*tab)))
                });
            let mut strip = match reuse {
                Some(index) => old.swap_remove(index),
                None => PaneStrip::new(),
            };
            let shown: Vec<StripTab> = area
                .members
                .iter()
                .enumerate()
                .map(|(index, &tab)| self.strip_tab(tab, index))
                .collect();
            strip.sync(&shown, area.active, focused_pane == Some(area.active));
            strip.rect = rect;
            strips.push(strip);
        }
        self.chrome.strips = strips;
    }

    /// Draw every pane's strip over `frame`, as [`Self::sync_strips`] last
    /// left them, and where a tab dragged off its strip would go.
    pub(in crate::window) fn draw_strips(&mut self, frame: &mut Frame) {
        let theme = match &self.renderer {
            Some(renderer) => misty_theme(renderer.theme()),
            None => misty_theme(&crate::render::Theme::dark()),
        };
        let dragging = self.chrome.tab_drag;
        for pane in &mut self.chrome.strips {
            let marker = dragging
                .filter(|drag| !pane.has(drag.tab) && pane.contains(drag.point.x, drag.point.y))
                .map(|drag| drag.point);
            pane.strip.show_drop_marker(marker);
            let rect = pane.rect;
            let area = misty::Rect::new(rect.x, rect.y, rect.width, rect.height);
            pane.strip.draw(frame, area, &theme);
        }
        let zone = dragging.and_then(|drag| self.drop_zone(drag));
        if let Some((pane, zone)) = zone {
            draw_drop_zone(frame, pane, zone, &theme);
        }
    }

    /// Hand pointer `input` to the strips; whether one took it. A press on
    /// a split divider is the divider's, and the pointer moving is still
    /// Winter's too, unless it drags a tab.
    pub(in crate::window) fn offer_to_strips(
        &mut self,
        input: &Input,
        cx: &mut EventCx<'_>,
    ) -> bool {
        if let Input::PointerButton(button) = input {
            let (x, y) = (button.position.x, button.position.y);
            let on_divider = self
                .layout()
                .divider_at(x, y, self.content_viewport())
                .is_some();
            if button.pressed && (on_divider || self.chrome.dialog_open()) {
                return false;
            }
        }
        let mut answer = None;
        for (index, pane) in self.chrome.strips.iter_mut().enumerate() {
            let response = pane.strip.handle(input, cx);
            if response != TabStripResponse::Ignored && answer.is_none() {
                answer = Some((index, response));
            }
        }
        let Some((index, response)) = answer else {
            return false;
        };
        // What the answer names, read off the strip before the layout
        // changes under it.
        let pane = &self.chrome.strips[index];
        let shown = pane.strip.active().and_then(|id| pane.winter(id));
        let named = match response {
            TabStripResponse::Activated(id)
            | TabStripResponse::Close(id)
            | TabStripResponse::Moved(id) => pane.winter(id),
            _ => None,
        };
        let place = match response {
            TabStripResponse::Moved(id) => pane.strip.ids().position(|other| other == id),
            _ => None,
        };
        let moved = matches!(input, Input::PointerMoved(_));
        match response {
            TabStripResponse::Activated(_) => {
                if let Some(tab) = named {
                    self.show_tab(tab);
                }
            }
            TabStripResponse::Close(_) => {
                if let Some(tab) = named {
                    self.close_tab(tab);
                }
            }
            TabStripResponse::NewTab => {
                if let Some(tab) = shown {
                    self.set_focus(tab);
                }
                self.new_tab();
            }
            TabStripResponse::Moved(_) => {
                if let (Some(tab), Some(index), Some(pane_tab)) = (named, place, shown) {
                    self.drop_tab(
                        tab,
                        TabDrop::Strip {
                            index,
                            pane: pane_tab,
                        },
                    );
                }
            }
            TabStripResponse::Dragging(drag) => {
                self.chrome.tab_drag = self.tab_dragging(index, drag);
                self.dirty = true;
                return true;
            }
            TabStripResponse::DroppedOutside(drag) => {
                self.chrome.tab_drag = None;
                if let Some(dragging) = self.tab_dragging(index, drag) {
                    let drop = self.drop_at(dragging.point);
                    self.drop_tab(dragging.tab, drop);
                }
            }
            TabStripResponse::Handled => {
                if moved {
                    return false;
                }
            }
            TabStripResponse::Ignored => return false,
        }
        self.dirty = true;
        !moved
    }

    /// When the strips next need drawing for a spinner's turn.
    pub(in crate::window) fn strips_deadline(&self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.chrome
            .strips
            .iter()
            .filter_map(|pane| pane.strip.about_to_wait(cx))
            .min()
    }

    /// What the strips have at `(x, y)` as last drawn.
    pub(in crate::window) fn strip_hit(&self, x: f32, y: f32) -> StripHit {
        let point = Point::new(x, y);
        for pane in &self.chrome.strips {
            match pane.strip.hit_test(point) {
                Some(TabStripHit::Close(id)) => {
                    return pane.winter(id).map_or(StripHit::None, StripHit::Close);
                }
                Some(TabStripHit::NewTab) => {
                    let shown = pane.strip.active().and_then(|id| pane.winter(id));
                    return shown.map_or(StripHit::None, StripHit::NewTab);
                }
                Some(TabStripHit::Tab(id)) => {
                    return pane.winter(id).map_or(StripHit::None, StripHit::Tab);
                }
                None => {}
            }
        }
        StripHit::None
    }

    /// Where `tab` was last drawn on its strip, in window pixels.
    pub(in crate::window) fn strip_tab_region(&self, tab: TabId) -> Option<crate::render::Region> {
        let pane = self.chrome.strips.iter().find(|pane| pane.has(tab))?;
        let rect = pane.strip.tab_rect(pane.misty(tab)?)?;
        Some(crate::render::Region {
            h: rect.height,
            w: rect.width,
            x: rect.x,
            y: rect.y,
        })
    }

    /// What the strip shows of `tab`, the `index`th on it: its title led by
    /// its place, the number that switches to it.
    fn strip_tab(&self, tab: TabId, index: usize) -> StripTab {
        StripTab {
            busy: self.busy.spinning.contains(&tab),
            icon: crate::icons::name_for(&self.tab_icon_kind(tab)),
            modified: self.open_tabs.page(&tab).is_some_and(|slot| {
                slot.page
                    .as_editor()
                    .is_some_and(|editor| editor.is_dirty())
            }),
            tab,
            title: format!("{}: {}", index + 1, self.strip_title(tab)),
        }
    }

    /// The tab misty says is dragged off strip `index`.
    fn tab_dragging(&self, index: usize, drag: TabDrag) -> Option<TabDragging> {
        let tab = self.chrome.strips.get(index)?.winter(drag.tab)?;
        Some(TabDragging {
            point: drag.position,
            tab,
        })
    }

    /// Where a tab let go at `point` goes: into the strip under it, at the
    /// gap there, or by the pane under it.
    fn drop_at(&self, point: Point) -> TabDrop {
        let strip = self
            .chrome
            .strips
            .iter()
            .find(|pane| pane.contains(point.x, point.y));
        let target = strip.and_then(|pane| {
            let index = pane.strip.drop_index(point)?;
            let shown = pane.tabs.first()?.0;
            Some(TabDrop::Strip { index, pane: shown })
        });
        target.unwrap_or(TabDrop::At(point.x, point.y))
    }

    /// The pane a tab dragged off its strip would go by, and how: a side
    /// to split it toward, or its center to join its tabs.
    fn drop_zone(&self, drag: TabDragging) -> Option<(misty::Rect, DropZone)> {
        let (x, y) = (drag.point.x, drag.point.y);
        if self.chrome.strips.iter().any(|pane| pane.contains(x, y)) {
            return None;
        }
        let (target, rect) = self.pane_at_pixel(x, y)?;
        let pane = misty::Rect::new(rect.x, rect.y, rect.width, rect.height);
        let splits =
            self.layout().same_pane(drag.tab, target) || super::is_pane_edge_zone(x, y, &rect);
        let zone = match (splits, super::pane_drop_split_dir(x, y, &rect)) {
            (false, _) => DropZone::Center,
            (true, FocusDir::Up) => DropZone::Top,
            (true, FocusDir::Down) => DropZone::Bottom,
            (true, FocusDir::Left) => DropZone::Left,
            (true, FocusDir::Right) => DropZone::Right,
        };
        Some((pane, zone))
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// The Winter tabs on the first strip, in order, and the one shown.
    fn first_strip(app: &App) -> (Vec<TabId>, Option<TabId>) {
        let pane = &app.chrome.strips[0];
        let order = pane.strip.ids().filter_map(|id| pane.winter(id)).collect();
        let shown = pane.strip.active().and_then(|id| pane.winter(id));
        (order, shown)
    }

    #[test]
    fn test_a_strip_follows_its_pane_s_tabs_their_order_and_the_one_shown() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.new_tab();
        let second = app.layout().focused();
        app.sync_strips();
        assert_eq!(first_strip(&app), (vec![first, second], Some(second)));

        app.move_tab_to(second, first);
        app.show_tab(first);
        app.sync_strips();
        assert_eq!(first_strip(&app), (vec![second, first], Some(first)));

        app.close_tab(first);
        app.sync_strips();
        assert_eq!(first_strip(&app), (vec![second], Some(second)));
    }

    #[test]
    fn test_a_split_gives_each_pane_a_strip_of_its_own_tabs() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.split_pane_with_terminal(winter_model::layout::Direction::Vertical);
        let second = app.layout().focused();
        app.sync_strips();
        let owners: Vec<Vec<TabId>> = app
            .chrome
            .strips
            .iter()
            .map(|pane| pane.tabs.iter().map(|(tab, _)| *tab).collect())
            .collect();
        assert_eq!(owners, [vec![first], vec![second]]);
    }

    #[test]
    fn test_a_tab_dragged_over_another_pane_joins_it_in_the_middle_or_splits_it_at_an_edge() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.split_pane_with_terminal(winter_model::layout::Direction::Vertical);
        let second = app.layout().focused();
        let pane = app.pane_rect(second).expect("shown");
        let over = |x: f32, y: f32| TabDragging {
            point: Point::new(x, y),
            tab: first,
        };
        let middle = over(pane.x + pane.width / 2.0, pane.y + pane.height / 2.0);
        let edge = over(pane.x + pane.width * 0.95, pane.y + pane.height / 2.0);
        assert_eq!(
            app.drop_zone(middle).map(|(_, zone)| zone),
            Some(DropZone::Center)
        );
        assert_eq!(
            app.drop_zone(edge).map(|(_, zone)| zone),
            Some(DropZone::Right)
        );
    }
}

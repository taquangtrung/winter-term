//! The pane strips, drawn by misty: one [`TabStrip`] for each pane, kept in
//! step with the layout before each frame (its tabs, their order, the one
//! shown, titles, icons, unsaved, waiting, and busy marks), and the pointer handed to
//! them. A click shows a tab, its close mark or a middle click closes it,
//! a pane's new-tab button opens one, and a tab dragged along its strip moves
//! there; dragged off it, the tab goes to the strip or pane it is let go
//! over (see [`App::drop_tab`]).

use std::time::Instant;

use misty::{AccessAction, EventCx, FocusId, Frame, Input, Point, Rect, SolidFill, Widget};
use misty::widgets::{DropZone, TabClose, TabDrag, TabId as MistyTab, TabStrip, TabStripHit, TabStripResponse, TabStyle, TabWidth, draw_drop_target};

use winter_model::layout::{FocusDir, Panel, Region, TabId};

use super::sidebar::{SidebarBar, SidebarResponse};
use super::{StripHit, TabDrop};
use crate::control::AgentState;
use crate::window::App;
use crate::window::appearance::ui_theme;

// ========================================================================
// Constants
// ========================================================================

/// The fewest logical pixels wide a tab is kept when only a few are open.
const PREFERRED_MIN_TAB_WIDTH: f32 = 240.0;

/// The hard minimum width of any tab, in logical pixels, even with many open.
const HARD_MIN_TAB_WIDTH: f32 = 140.0;

/// How many tabs keep the full preferred minimum width before shrinking.
const FULL_WIDTH_TAB_LIMIT: usize = 3;

/// How many logical pixels the minimum tab width drops per extra tab.
const TAB_WIDTH_STEP: f32 = 30.0;

/// Top corner radius for the active tab, matching `Shape::MODERN`.
const ACTIVE_TAB_RADIUS: f32 = 6.0;

/// How tall a pane strip is, in cells: the height Winter's strips have had,
/// kept whatever misty's own default becomes.
pub(in crate::window) const STRIP_HEIGHT_RATIO: f32 = 1.6;

// ========================================================================
// Data Structures
// ========================================================================

/// One pane's strip: misty's tab strip or a sidebar's flat icon switcher,
/// the Winter tab behind each entry, and where it was laid out.
pub(in crate::window) struct PaneStrip {
    /// How this strip is drawn and interacted with.
    kind: StripKind,
    /// Where the strip sits, along its pane's top.
    rect: Rect,
    /// Whether it is a pane's strip of tabs or a panel's switcher of views.
    region: Region,
    /// Each Winter tab on the strip and misty's tab for it, in strip order.
    tabs: Vec<(TabId, MistyTab)>,
}

/// Which widget backs a [`PaneStrip`].
enum StripKind {
    /// Flat icon switcher for a sidebar panel.
    Sidebar(SidebarBar),
    /// Standard tab strip for main panes and the bottom panel.
    Tabs(Box<TabStrip>),
}

/// What a strip shows of one tab.
pub(super) struct StripTab {
    /// Whether an agent in it waits for the user, unseen since it started.
    pub(super) attention: bool,
    /// Whether it is busy long enough to show a spinner.
    pub(super) busy: bool,
    /// The bundled icon naming what it holds.
    pub(super) icon: Option<String>,
    /// Whether it holds unsaved changes.
    pub(super) modified: bool,
    /// Winter's tab.
    pub(super) tab: TabId,
    /// Its place and title, with a rename's caret while one is typed.
    pub(super) title: String,
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
    /// An empty strip for `region`: a sidebar gets a flat icon switcher,
    /// while main panes and the bottom panel use misty's tab strip.
    fn new(region: Region) -> Self {
        let kind = match region {
            Region::Panel(Panel::Sidebar(_)) => StripKind::Sidebar(SidebarBar::new()),
            Region::Main | Region::Panel(Panel::Bottom) => {
                // Pill tabs of one width, the widest title's, so none stands out,
                // with a rounded active tab and no vertical cell borders, with close
                // buttons shown under the pointer only.
                let strip = TabStrip::new()
                    .with_tab_style(TabStyle::Filled)
                    .with_tab_width(TabWidth::Equal)
                    .with_min_tab_width(PREFERRED_MIN_TAB_WIDTH)
                    .with_active_tab_radius(ACTIVE_TAB_RADIUS)
                    .with_active_tab_edges(false)
                    .with_tab_borders(false);
                let mut strip = match region {
                    Region::Main => strip.with_close_buttons(TabClose::Hovered),
                    Region::Panel(_) => strip
                        .with_close_buttons(TabClose::Never)
                        .with_new_tab_button(false),
                };
                strip.set_height_ratio(Some(STRIP_HEIGHT_RATIO));
                StripKind::Tabs(Box::new(strip))
            }
        };
        Self {
            kind,
            rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            region,
            tabs: Vec::new(),
        }
    }

    /// Whether Winter's `tab` is on the strip.
    fn has(&self, tab: TabId) -> bool {
        match &self.kind {
            StripKind::Sidebar(bar) => bar.has(tab),
            StripKind::Tabs(_) => self.tabs.iter().any(|(winter, _)| *winter == tab),
        }
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

    /// Where the active tab's accent line was drawn, over the border along
    /// the strip's top; `None` when the strip is not the focused pane's or
    /// uses a bottom indicator.
    pub(in crate::window) fn accent_rect(&self) -> Option<Rect> {
        match &self.kind {
            StripKind::Sidebar(_) => None,
            StripKind::Tabs(strip) => strip.accent_rect(),
        }
    }

    /// Bring the strip in step with `shown`, in order, `active` the tab
    /// shown, the strip lit as the focused pane's when `focused`, and its
    /// accent dimmed unless `window_focused`.
    fn sync(&mut self, shown: &[StripTab], active: TabId, focused: bool, window_focused: bool) {
        let strip = match &mut self.kind {
            StripKind::Sidebar(bar) => {
                bar.sync(shown, active, focused, window_focused);
                return;
            }
            StripKind::Tabs(strip) => strip,
        };
        let gone: Vec<MistyTab> = self
            .tabs
            .iter()
            .filter(|(winter, _)| !shown.iter().any(|tab| tab.tab == *winter))
            .map(|(_, misty)| *misty)
            .collect();
        for id in gone {
            strip.remove(id);
        }
        self.tabs
            .retain(|(winter, _)| shown.iter().any(|tab| tab.tab == *winter));
        for (index, tab) in shown.iter().enumerate() {
            let existing = self
                .tabs
                .iter()
                .find(|(winter, _)| *winter == tab.tab)
                .map(|(_, misty)| *misty);
            let id = match existing {
                Some(id) => {
                    strip.move_tab(id, index);
                    id
                }
                None => {
                    let id = strip.insert(index, tab.title.as_str());
                    self.tabs.push((tab.tab, id));
                    id
                }
            };
            strip.set_title(id, tab.title.as_str());
            strip.set_icon(id, tab.icon.clone());
            strip.set_modified(id, tab.modified);
            strip.set_attention(id, tab.attention);
            strip.set_busy(id, tab.busy);
        }
        let names: Vec<String> = shown.iter().filter_map(|tab| tab.icon.clone()).collect();
        crate::icons::add_to(strip.icons_mut(), &names);
        self.tabs.sort_by_key(|(winter, _)| {
            shown
                .iter()
                .position(|tab| tab.tab == *winter)
                .unwrap_or(usize::MAX)
        });
        let active_id = self
            .tabs
            .iter()
            .find(|(winter, _)| *winter == active)
            .map(|(_, misty)| *misty);
        if let Some(id) = active_id
            && strip.active() != Some(id)
        {
            strip.activate(id);
        }
        strip.set_min_tab_width(min_tab_width(shown.len()));
        strip.set_focused(focused);
        strip.set_window_active(window_focused);
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
        let mut old = std::mem::take(&mut self.parts.strips);
        let mut strips = Vec::with_capacity(areas.len());
        for (area, rect) in areas {
            let reuse = old
                .iter()
                .position(|strip| strip.region == area.region && strip.has(area.active))
                .or_else(|| {
                    old.iter().position(|strip| {
                        strip.region == area.region
                            && area.members.iter().any(|tab| strip.has(*tab))
                    })
                });
            let mut strip = match reuse {
                Some(index) => old.swap_remove(index),
                None => PaneStrip::new(area.region),
            };
            let shown: Vec<StripTab> = area
                .members
                .iter()
                .enumerate()
                .map(|(index, &tab)| self.strip_tab(tab, index, area.region))
                .collect();
            strip.sync(
                &shown,
                area.active,
                focused_pane == Some(area.active),
                self.input.focus.window,
            );
            // The accent takes over the border along the strip's top.
            if let StripKind::Tabs(tabs) = &mut strip.kind {
                tabs.set_accent_overhang(border_above(rect, &self.parts.drawn_separators));
            }
            strip.rect = rect;
            strips.push(strip);
        }
        self.parts.strips = strips;
    }

    /// Draw every pane's strip over `frame`, as [`Self::sync_strips`] last
    /// left them, and where a tab dragged off its strip would go.
    pub(in crate::window) fn draw_strips(&mut self, frame: &mut Frame) {
        let theme = ui_theme(self.renderer.as_ref());
        let dragging = self.parts.tab_drag;
        for (index, pane) in self.parts.strips.iter_mut().enumerate() {
            // Nothing is dropped into a panel.
            let marker = dragging
                .filter(|_| pane.region == Region::Main)
                .filter(|drag| !pane.has(drag.tab) && pane.contains(drag.point.x, drag.point.y))
                .map(|drag| drag.point);
            match &mut pane.kind {
                StripKind::Sidebar(bar) => {
                    bar.draw(frame, pane.rect, theme);
                    bar.access(frame, strip_id(index));
                }
                StripKind::Tabs(strip) => {
                    strip.show_drop_marker(marker);
                    strip.draw(frame, pane.rect, theme);
                    // Screen readers hear each strip's tabs and buttons, and click
                    // them through `strips_access_action`.
                    strip.access(frame, strip_id(index));
                }
            }
        }
        let zone = dragging.and_then(|drag| self.drop_zone(drag));
        if let Some((pane, zone)) = zone {
            draw_drop_target(frame, zone.rect(pane), true, theme);
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
        if self.input.pointer.divider_drag.is_some() {
            return false;
        }
        if let Input::PointerButton(button) = input {
            let (x, y) = (button.position.x, button.position.y);
            let on_divider = self
                .layout()
                .divider_at(x, y, self.viewport_rect())
                .is_some();
            if button.pressed && (on_divider || self.parts.dialog_open()) {
                return false;
            }
        }
        let moved = matches!(input, Input::PointerMoved(_) | Input::PointerLeft);
        let mut sidebar_answer = None;
        let mut tab_answer = None;
        for (index, pane) in self.parts.strips.iter_mut().enumerate() {
            match &mut pane.kind {
                StripKind::Sidebar(bar) => {
                    let response = bar.handle(input, cx);
                    if response != SidebarResponse::Ignored && sidebar_answer.is_none() {
                        sidebar_answer = Some(response);
                    }
                }
                StripKind::Tabs(strip) => {
                    let response = strip.handle(input, cx);
                    if response != TabStripResponse::Ignored && tab_answer.is_none() {
                        tab_answer = Some((index, response));
                    }
                }
            }
        }
        if let Some(response) = sidebar_answer {
            return self.act_on_sidebar(response, moved);
        }
        let Some((index, response)) = tab_answer else {
            return false;
        };
        self.act_on_strip(index, response, moved)
    }

    /// Do what a screen reader's `action` on a strip asks, as a click there
    /// would; whether a strip took it.
    pub(in crate::window) fn strips_access_action(&mut self, action: AccessAction) -> bool {
        for (index, pane) in self.parts.strips.iter_mut().enumerate() {
            match &mut pane.kind {
                StripKind::Sidebar(bar) => {
                    let response = bar.access_action(strip_id(index), action);
                    if response != SidebarResponse::Ignored {
                        return self.act_on_sidebar(response, false);
                    }
                }
                StripKind::Tabs(strip) => {
                    let response = strip.access_action(strip_id(index), action);
                    if response != TabStripResponse::Ignored {
                        return self.act_on_strip(index, response, false);
                    }
                }
            }
        }
        false
    }

    /// Carry out what a sidebar icon switcher answered.
    fn act_on_sidebar(&mut self, response: SidebarResponse, moved: bool) -> bool {
        match response {
            SidebarResponse::Activated(view) => {
                self.layout_mut().focus(view);
                self.after_tab_change();
            }
            SidebarResponse::Handled => {
                if moved {
                    return false;
                }
            }
            SidebarResponse::Ignored => return false,
        }
        self.dirty = true;
        !moved
    }

    /// Carry out what strip `index` answered, the pointer having `moved`
    /// for it; whether the answer was the strip's to take.
    fn act_on_strip(&mut self, index: usize, response: TabStripResponse, moved: bool) -> bool {
        // What the answer names, read off the strip before the layout
        // changes under it.
        let pane = &self.parts.strips[index];
        if let Region::Panel(_) = pane.region {
            return self.act_on_switcher(index, response, moved);
        }
        let StripKind::Tabs(strip) = &pane.kind else {
            return false;
        };
        let shown = strip.active().and_then(|id| pane.winter(id));
        let named = match response {
            TabStripResponse::Activated(id)
            | TabStripResponse::Close(id)
            | TabStripResponse::Moved(id) => pane.winter(id),
            _ => None,
        };
        let place = match response {
            TabStripResponse::Moved(id) => strip.ids().position(|other| other == id),
            _ => None,
        };
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
                self.parts.tab_drag = self.tab_dragging(index, drag);
                self.dirty = true;
                return true;
            }
            TabStripResponse::DroppedOutside(drag) => {
                self.parts.tab_drag = None;
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
            // A response misty adds later is none the strip acts on.
            _ => return false,
        }
        self.dirty = true;
        !moved
    }

    /// Carry out what a panel's switcher, strip `index`, answered: a view
    /// chosen. Its views never close or move, so a drag only snaps back.
    fn act_on_switcher(&mut self, index: usize, response: TabStripResponse, moved: bool) -> bool {
        let pane = &self.parts.strips[index];
        match response {
            TabStripResponse::Activated(id) => {
                if let Some(view) = pane.winter(id) {
                    self.layout_mut().focus(view);
                    self.after_tab_change();
                }
            }
            TabStripResponse::Dragging(_) | TabStripResponse::DroppedOutside(_) => {
                self.parts.tab_drag = None;
            }
            TabStripResponse::Close(_) | TabStripResponse::Moved(_) => {}
            TabStripResponse::Handled => {
                if moved {
                    return false;
                }
            }
            TabStripResponse::Ignored => return false,
            // A response misty adds later is none the switcher acts on.
            _ => return false,
        }
        self.dirty = true;
        !moved
    }

    /// When the strips next need drawing for a spinner's turn.
    pub(in crate::window) fn strips_deadline(&mut self, cx: &mut EventCx<'_>) -> Option<Instant> {
        self.parts
            .strips
            .iter_mut()
            .filter_map(|pane| match &mut pane.kind {
                StripKind::Sidebar(_) => None,
                StripKind::Tabs(strip) => strip.about_to_wait(cx),
            })
            .min()
    }

    /// What the strips have at `(x, y)` as last drawn.
    pub(in crate::window) fn strip_hit(&self, x: f32, y: f32) -> StripHit {
        let point = Point::new(x, y);
        for pane in &self.parts.strips {
            match &pane.kind {
                StripKind::Sidebar(bar) => {
                    if let Some(tab) = bar.hit_tab(point) {
                        return StripHit::Tab(tab);
                    }
                }
                StripKind::Tabs(strip) => match strip.hit_test(point) {
                    Some(TabStripHit::Close(id)) => {
                        return pane.winter(id).map_or(StripHit::None, StripHit::Close);
                    }
                    Some(TabStripHit::NewTab) => {
                        let shown = strip.active().and_then(|id| pane.winter(id));
                        return shown.map_or(StripHit::None, StripHit::NewTab);
                    }
                    Some(TabStripHit::NextTab) => return StripHit::NextTab,
                    Some(TabStripHit::PrevTab) => return StripHit::PrevTab,
                    Some(TabStripHit::Tab(id)) => {
                        return pane.winter(id).map_or(StripHit::None, StripHit::Tab);
                    }
                    None => {}
                    // A part misty adds later is no part Winter knows.
                    Some(_) => {}
                },
            }
        }
        StripHit::None
    }

    /// Where the part of a strip at `(x, y)` was last drawn: a tab, its
    /// close button, or a button.
    pub(in crate::window) fn strip_part_rect(&self, x: f32, y: f32) -> Option<Rect> {
        let point = Point::new(x, y);
        self.parts.strips.iter().find_map(|pane| match &pane.kind {
            StripKind::Sidebar(bar) => bar.part_rect(point),
            StripKind::Tabs(strip) => strip.part_rect(point),
        })
    }

    /// Whether `tab`'s strip cut its title short in the last frame.
    pub(in crate::window) fn strip_title_cut(&self, tab: TabId) -> bool {
        self.parts
            .strips
            .iter()
            .find(|pane| pane.has(tab))
            .and_then(|pane| match &pane.kind {
                StripKind::Sidebar(_) => Some(true),
                StripKind::Tabs(strip) => Some(strip.title_cut(pane.misty(tab)?)),
            })
            .unwrap_or(false)
    }

    /// What the strip shows of `tab`, the `index`th on it in `region`: its
    /// title, led in the main area by its place, the number that switches to
    /// it. A panel's views go unnumbered, the numbers being the panes'.
    fn strip_tab(&self, tab: TabId, index: usize, region: Region) -> StripTab {
        // An agent at work spins; one waiting for you out of sight carries
        // the accent dot until you see it.
        let agent = self.tabs.agents.of(tab);
        StripTab {
            attention: self.tabs.agents.unseen(tab),
            busy: self.busy.spinning.contains(&tab) || agent == Some(AgentState::Working),
            icon: crate::icons::name_for(&self.tab_icon_kind(tab)),
            modified: self.open_tabs.page(&tab).is_some_and(|slot| {
                slot.page
                    .as_editor()
                    .is_some_and(|editor| editor.is_dirty())
            }),
            tab,
            title: match region {
                Region::Main => format!("{}: {}", index + 1, self.strip_title(tab)),
                Region::Panel(_) => self.strip_title(tab),
            },
        }
    }

    /// The tab misty says is dragged off strip `index`.
    fn tab_dragging(&self, index: usize, drag: TabDrag) -> Option<TabDragging> {
        let tab = self.parts.strips.get(index)?.winter(drag.tab)?;
        Some(TabDragging {
            point: drag.position,
            tab,
        })
    }

    /// Where a tab let go at `point` goes: into the strip under it, at the
    /// gap there, or by the pane under it.
    fn drop_at(&self, point: Point) -> TabDrop {
        let strip = self
            .parts
            .strips
            .iter()
            .find(|pane| pane.contains(point.x, point.y));
        let target = strip.and_then(|pane| {
            let StripKind::Tabs(tabs) = &pane.kind else {
                return None;
            };
            let index = tabs.drop_index(point)?;
            let shown = pane.tabs.first()?.0;
            Some(TabDrop::Strip { index, pane: shown })
        });
        target.unwrap_or(TabDrop::At(point.x, point.y))
    }

    /// The pane a tab dragged off its strip would go by, and how: a side
    /// to split it toward, or its center to join its tabs.
    fn drop_zone(&self, drag: TabDragging) -> Option<(misty::Rect, DropZone)> {
        let (x, y) = (drag.point.x, drag.point.y);
        if self.parts.strips.iter().any(|pane| pane.contains(x, y)) {
            return None;
        }
        let (target, rect) = self.pane_at_pixel(x, y)?;
        if self.layout().region_of(target) != Some(Region::Main) {
            return None;
        }
        let splits =
            self.layout().same_pane(drag.tab, target) || super::is_pane_edge_zone(x, y, &rect);
        let zone = match (splits, super::pane_drop_split_dir(x, y, &rect)) {
            (false, _) => DropZone::Center,
            (true, FocusDir::Up) => DropZone::Top,
            (true, FocusDir::Down) => DropZone::Bottom,
            (true, FocusDir::Left) => DropZone::Left,
            (true, FocusDir::Right) => DropZone::Right,
        };
        Some((rect, zone))
    }
}

// ========================================================================
// Functions
// ========================================================================

/// What screen readers know strip `index` by.
fn strip_id(index: usize) -> FocusId {
    FocusId::new("winter.strip").child(index as u64)
}

/// Minimum tab width for `tab_count` tabs: stepping down from the preferred
/// width until reaching the hard floor. A sidebar's square icons ignore it.
fn min_tab_width(tab_count: usize) -> f32 {
    let extra = tab_count.saturating_sub(FULL_WIDTH_TAB_LIMIT) as f32;
    (PREFERRED_MIN_TAB_WIDTH - extra * TAB_WIDTH_STEP).max(HARD_MIN_TAB_WIDTH)
}

/// How many pixels of a border running across `strip`'s top lie above it,
/// among `separators`: the rows its active tab's accent rises over; none
/// where nothing borders it, at the window's top.
fn border_above(strip: Rect, separators: &[SolidFill]) -> f32 {
    separators
        .iter()
        .map(|fill| fill.rect)
        .filter(|line| {
            line.width > line.height
                && line.y < strip.y
                && line.y + line.height >= strip.y
                && line.x < strip.x + strip.width
                && line.x + line.width > strip.x
        })
        .map(|line| strip.y - line.y)
        .fold(0.0, f32::max)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use winter_model::layout::Edge;

    use super::*;

    /// The Winter tabs on the first strip, in order, and the one shown.
    fn first_strip(app: &App) -> (Vec<TabId>, Option<TabId>) {
        let pane = &app.parts.strips[0];
        let StripKind::Tabs(strip) = &pane.kind else {
            panic!("expected tab strip");
        };
        let order = strip.ids().filter_map(|id| pane.winter(id)).collect();
        let shown = strip.active().and_then(|id| pane.winter(id));
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
    fn test_a_panel_s_views_go_unnumbered_and_the_main_area_s_tabs_keep_their_number() {
        let mut app = App::new();
        let main = app.layout().focused();
        app.toggle_panel(Panel::Sidebar(Edge::Left));
        let panel = app.layout().focused();
        assert_eq!(
            app.strip_tab(main, 0, Region::Main).title,
            format!("1: {}", app.strip_title(main))
        );
        let region = app.layout().region_of(panel).expect("held");
        assert_eq!(
            app.strip_tab(panel, 0, region).title,
            app.strip_title(panel),
            "no number"
        );
    }

    #[test]
    fn test_an_agent_waiting_for_you_wears_the_attention_dot_not_the_unsaved_one() {
        // A terminal holds no unsaved text, so its dot must not read as such.
        let mut app = App::new();
        let tab = app.layout().focused();
        app.input.focus.window = false;
        app.take_hook_state(tab, AgentState::Waiting);
        let waiting = app.strip_tab(tab, 0, Region::Main);
        assert!(waiting.attention && !waiting.modified);
        app.take_hook_state(tab, AgentState::Working);
        let working = app.strip_tab(tab, 0, Region::Main);
        assert!(!working.attention && working.busy);
    }

    #[test]
    fn test_a_split_gives_each_pane_a_strip_of_its_own_tabs() {
        let mut app = App::new();
        let first = app.layout().focused();
        app.split_pane_with_terminal(winter_model::layout::Direction::Vertical);
        let second = app.layout().focused();
        app.sync_strips();
        let owners: Vec<Vec<TabId>> = app
            .parts
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

    #[test]
    fn test_the_accent_rises_over_the_border_across_the_strip_s_top_not_a_divider_ending_there() {
        let strip = Rect::new(100.0, 40.0, 200.0, 24.0);
        let fill = |rect| SolidFill {
            color: misty::Rgba::new(0, 0, 0, u8::MAX),
            rect,
        };
        let across = fill(Rect::new(0.0, 38.0, 400.0, 3.0));
        let divider_above = fill(Rect::new(150.0, 0.0, 1.0, 40.0));
        let elsewhere = fill(Rect::new(400.0, 39.0, 100.0, 1.0));
        assert_eq!(border_above(strip, &[divider_above, elsewhere]), 0.0);
        assert_eq!(border_above(strip, &[across, divider_above]), 2.0);
    }

    #[test]
    fn test_min_tab_width_shrinks_with_more_tabs_down_to_hard_floor() {
        assert_eq!(min_tab_width(1), PREFERRED_MIN_TAB_WIDTH);
        assert_eq!(min_tab_width(FULL_WIDTH_TAB_LIMIT), PREFERRED_MIN_TAB_WIDTH);
        assert!(min_tab_width(FULL_WIDTH_TAB_LIMIT + 1) < PREFERRED_MIN_TAB_WIDTH);
        assert!(min_tab_width(FULL_WIDTH_TAB_LIMIT + 2) < min_tab_width(FULL_WIDTH_TAB_LIMIT + 1));
        assert_eq!(min_tab_width(50), HARD_MIN_TAB_WIDTH);
    }
}

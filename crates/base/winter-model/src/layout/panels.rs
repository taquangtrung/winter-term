//! The window's panels: a left sidebar, a right sidebar, and a bottom panel
//! around the main area. A panel is a [`Dock`] of views, one shown at a
//! time, chosen by kind: a sidebar's are [`SidebarKind`]s, the bottom
//! panel's [`BottomPanelKind`]s. A view is no tab: it never leaves its
//! panel, and no tab enters one. Each view is named by a [`TabId`] the app
//! allocates, so focus stays one id for the whole window.
//!
//! Sizes are in logical pixels, so a panel follows the UI scale and not the
//! terminal font. The sidebars run the window's full height and the bottom
//! panel spans the main area between them. When the window is too small for
//! the panels at their sizes, they shrink toward their minimums, the right
//! sidebar first, then the left, then the bottom panel; past that the right
//! sidebar is left out, then the left, then the bottom panel. Being left out
//! is worked out each time the window is laid out and never stored.

use misty::Rect;

use super::TabId;

// ========================================================================
// Constants
// ========================================================================

/// A sidebar's width, in logical pixels, until the user sets one.
pub const SIDEBAR_WIDTH: f32 = 260.0;

/// The bottom panel's height, in logical pixels, until the user sets one.
pub const BOTTOM_HEIGHT: f32 = 220.0;

/// The narrowest a sidebar shrinks to, in logical pixels, before it is left
/// out.
pub const MIN_SIDEBAR_WIDTH: f32 = 160.0;

/// The lowest the bottom panel shrinks to, in logical pixels, before it is
/// left out.
pub const MIN_BOTTOM_HEIGHT: f32 = 80.0;

/// The largest a panel is let grow to, in logical pixels.
pub const MAX_PANEL_SIZE: f32 = 4000.0;

/// The narrowest the main area is let become for the panels, in logical
/// pixels.
const MAIN_MIN_WIDTH: f32 = 320.0;

/// The lowest the main area is let become for the bottom panel, in logical
/// pixels.
const MAIN_MIN_HEIGHT: f32 = 96.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The three panels of a window, which belong to the window rather than a
/// workspace: [`Layout::take_panels`](super::Layout::take_panels) hands them
/// from one workspace's layout to the next.
#[derive(Clone, Debug)]
pub struct Panels {
    pub(super) bottom: Dock<BottomPanelKind>,
    pub(super) left: Dock<SidebarKind>,
    pub(super) right: Dock<SidebarKind>,
}

/// One panel: the views it holds, by kind, the one shown, whether the user
/// has it shown, and its size.
#[derive(Clone, Debug)]
pub struct Dock<K> {
    /// What every panel has, whatever its kinds.
    pub(super) base: DockBase,
    /// Each view's kind, in the order of `base.views`.
    kinds: Vec<K>,
}

/// The part of a panel that does not depend on its kinds of view.
#[derive(Clone, Debug)]
pub(super) struct DockBase {
    /// The view shown, `None` while it holds none.
    pub(super) active: Option<TabId>,
    /// Whether the user has it shown.
    pub(super) shown: bool,
    /// Its width (a sidebar) or height (the bottom panel), in logical
    /// pixels.
    pub(super) size: f32,
    /// Its views, in the switcher's order.
    pub(super) views: Vec<TabId>,
}

/// A snapshot of one panel, for sessions and handovers: its views by kind,
/// the one shown, whether it is shown, and its size.
#[derive(Clone, Debug, PartialEq)]
pub struct DockSnapshot<K> {
    /// The view shown.
    pub active: Option<K>,
    /// Its views' kinds, in the switcher's order.
    pub kinds: Vec<K>,
    /// Whether the user has it shown.
    pub shown: bool,
    /// Its width or height, in logical pixels.
    pub size: f32,
}

/// What a sidebar shows.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum SidebarKind {
    /// The project's folders as a tree.
    Files,
    /// The project's git status.
    Git,
    /// The headings and symbols of the main tab last worked in.
    Outline,
    /// The project's TODO comments.
    Todo,
}

/// What the bottom panel shows.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum BottomPanelKind {
    /// The builds run, one shown.
    Output,
    /// The errors and warnings the builds found.
    Problems,
    /// Saved lists and project searches, one shown.
    Search,
}

/// Which sidebar.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Edge {
    /// The left sidebar.
    Left,
    /// The right sidebar.
    Right,
}

/// Which panel.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum Panel {
    /// The bottom panel, across the main area.
    Bottom,
    /// A sidebar.
    Sidebar(Edge),
}

/// Where a tab or a view sits: in the main area's split tree, or in a
/// panel.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum Region {
    /// The main area, where every tab lives.
    Main,
    /// A panel, holding views.
    Panel(Panel),
}

/// Whether a panel holds views, and whether it is shown.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelState {
    /// No views yet: the app fills it when it is first shown.
    Empty,
    /// Views, out of sight.
    Hidden,
    /// Views, in sight (unless the window is too small for it).
    Shown,
}

/// Where the panels that fit go, and what is left for the main area.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct Fitted {
    /// The main area.
    pub(super) main: Rect,
    /// Each panel laid out, left, right, then bottom.
    pub(super) panels: Vec<(Panel, Rect)>,
}

// ========================================================================
// Edge Implementations
// ========================================================================

impl Edge {
    /// Both sidebars, left first.
    pub const ALL: [Edge; 2] = [Edge::Left, Edge::Right];
}

// ========================================================================
// Panel Implementations
// ========================================================================

impl Panel {
    /// Every panel, in the order they are laid out and listed.
    pub const ALL: [Panel; 3] = [
        Panel::Sidebar(Edge::Left),
        Panel::Sidebar(Edge::Right),
        Panel::Bottom,
    ];

    /// The size a panel starts at, in logical pixels.
    pub fn default_size(self) -> f32 {
        match self {
            Panel::Bottom => BOTTOM_HEIGHT,
            Panel::Sidebar(_) => SIDEBAR_WIDTH,
        }
    }

    /// The smallest a panel shrinks to, in logical pixels.
    pub fn min_size(self) -> f32 {
        match self {
            Panel::Bottom => MIN_BOTTOM_HEIGHT,
            Panel::Sidebar(_) => MIN_SIDEBAR_WIDTH,
        }
    }
}

// ========================================================================
// Dock Implementations
// ========================================================================

impl<K: Copy + PartialEq> Dock<K> {
    /// An empty, hidden panel at `size`.
    fn new(size: f32) -> Self {
        Self {
            base: DockBase {
                active: None,
                shown: false,
                size,
                views: Vec::new(),
            },
            kinds: Vec::new(),
        }
    }

    /// The kind of the view shown, while it holds any.
    pub fn active_kind(&self) -> Option<K> {
        self.base.active.and_then(|tab| self.kind_of(tab))
    }

    /// The view shown, while it holds any.
    pub fn active_tab(&self) -> Option<TabId> {
        self.base.active
    }

    /// Whether the user has it shown.
    pub fn is_shown(&self) -> bool {
        self.base.shown
    }

    /// The kind of the view `tab`, if it holds it.
    pub fn kind_of(&self, tab: TabId) -> Option<K> {
        let at = self.base.views.iter().position(|view| *view == tab)?;
        Some(self.kinds[at])
    }

    /// Its views' kinds, in the switcher's order.
    pub fn kinds(&self) -> &[K] {
        &self.kinds
    }

    /// Its width (a sidebar) or height, in logical pixels.
    pub fn size(&self) -> f32 {
        self.base.size
    }

    /// A snapshot of it, for sessions and handovers.
    pub fn snapshot(&self) -> DockSnapshot<K> {
        DockSnapshot {
            active: self.active_kind(),
            kinds: self.kinds.clone(),
            shown: self.base.shown,
            size: self.base.size,
        }
    }

    /// The view of `kind`, if it holds one.
    pub fn tab_of(&self, kind: K) -> Option<TabId> {
        let at = self.kinds.iter().position(|held| *held == kind)?;
        Some(self.base.views[at])
    }

    /// Its views, in the switcher's order.
    pub fn views(&self) -> &[TabId] {
        &self.base.views
    }

    /// Add the view `tab` of `kind` at the end of the switcher and show it,
    /// the panel shown too. Returns `false`, changing nothing, when it holds
    /// a view of `kind` or `tab` already.
    pub(super) fn add(&mut self, kind: K, tab: TabId) -> bool {
        if self.tab_of(kind).is_some() || self.base.views.contains(&tab) {
            return false;
        }
        self.kinds.push(kind);
        self.base.views.push(tab);
        self.base.active = Some(tab);
        self.base.shown = true;
        true
    }

    /// Take out the view of `kind`, showing the one before it (or the first)
    /// where it was shown. Returns its id, `None` when it holds none.
    pub(super) fn remove(&mut self, kind: K) -> Option<TabId> {
        let at = self.kinds.iter().position(|held| *held == kind)?;
        self.kinds.remove(at);
        let tab = self.base.views.remove(at);
        if self.base.active == Some(tab) {
            self.base.active = self.base.views.get(at.saturating_sub(1)).copied();
        }
        if self.base.views.is_empty() {
            self.base.shown = false;
        }
        Some(tab)
    }
}

// ========================================================================
// DockBase Implementations
// ========================================================================

impl DockBase {
    /// Whether it holds views, and whether it is shown.
    pub(super) fn state(&self) -> PanelState {
        match (self.active, self.shown) {
            (None, _) => PanelState::Empty,
            (Some(_), false) => PanelState::Hidden,
            (Some(_), true) => PanelState::Shown,
        }
    }

    /// The view `step` places along the switcher from the one shown, stopping
    /// at the ends: `None` past them.
    pub(super) fn step(&self, forward: bool) -> Option<TabId> {
        let at = self
            .views
            .iter()
            .position(|view| Some(*view) == self.active)?;
        match forward {
            true => self.views.get(at + 1).copied(),
            false => at.checked_sub(1).map(|before| self.views[before]),
        }
    }
}

// ========================================================================
// Panels Implementations
// ========================================================================

impl Panels {
    /// Three empty, hidden panels at their default sizes.
    pub(super) fn new() -> Self {
        Self {
            bottom: Dock::new(BOTTOM_HEIGHT),
            left: Dock::new(SIDEBAR_WIDTH),
            right: Dock::new(SIDEBAR_WIDTH),
        }
    }

    /// Every panel's views, left, right, then bottom.
    pub fn views(&self) -> Vec<TabId> {
        Panel::ALL
            .into_iter()
            .flat_map(|panel| self.base(panel).views.iter().copied())
            .collect()
    }

    /// The sidebar on `edge`.
    pub(super) fn sidebar(&self, edge: Edge) -> &Dock<SidebarKind> {
        match edge {
            Edge::Left => &self.left,
            Edge::Right => &self.right,
        }
    }

    /// The sidebar on `edge`, mutably.
    pub(super) fn sidebar_mut(&mut self, edge: Edge) -> &mut Dock<SidebarKind> {
        match edge {
            Edge::Left => &mut self.left,
            Edge::Right => &mut self.right,
        }
    }

    /// What `panel` has whatever its kinds of view.
    pub(super) fn base(&self, panel: Panel) -> &DockBase {
        match panel {
            Panel::Bottom => &self.bottom.base,
            Panel::Sidebar(edge) => &self.sidebar(edge).base,
        }
    }

    /// The same, mutably.
    pub(super) fn base_mut(&mut self, panel: Panel) -> &mut DockBase {
        match panel {
            Panel::Bottom => &mut self.bottom.base,
            Panel::Sidebar(edge) => &mut self.sidebar_mut(edge).base,
        }
    }

    /// The panel holding the view `tab`, if one does.
    pub(super) fn panel_of(&self, tab: TabId) -> Option<Panel> {
        Panel::ALL
            .into_iter()
            .find(|&panel| self.base(panel).views.contains(&tab))
    }

    /// The sidebar holding a view of `kind`, if one does.
    pub(super) fn sidebar_of(&self, kind: SidebarKind) -> Option<Edge> {
        Edge::ALL
            .into_iter()
            .find(|&edge| self.sidebar(edge).tab_of(kind).is_some())
    }

    /// Where the panels that ask for room go within `viewport`, at `scale`
    /// physical pixels to a logical one, and the main area what is left.
    pub(super) fn fit(&self, viewport: Rect, scale: f32) -> Fitted {
        let room = |panel: Panel| self.base(panel).state() == PanelState::Shown;
        let size = |panel: Panel| self.base(panel).size * scale;
        let min = |panel: Panel| panel.min_size() * scale;
        let (left_panel, right_panel) = (Panel::Sidebar(Edge::Left), Panel::Sidebar(Edge::Right));
        // Across: the sidebars against the main area's least width.
        let spare_w = (viewport.width - MAIN_MIN_WIDTH * scale).max(0.0);
        let full_left = room(left_panel).then(|| size(left_panel));
        let mut right = room(right_panel).then(|| size(right_panel));
        let mut left = full_left;
        loop {
            // Shrink the right first, then the left, each to its least.
            for panel in [right_panel, left_panel] {
                let over = left.unwrap_or(0.0) + right.unwrap_or(0.0) - spare_w;
                let width = match panel == right_panel {
                    true => &mut right,
                    false => &mut left,
                };
                if over > 0.0
                    && let Some(width) = width
                {
                    *width = (*width - over).max(min(panel));
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
        let spare_h = (viewport.height - MAIN_MIN_HEIGHT * scale).max(0.0);
        let bottom = room(Panel::Bottom)
            .then(|| size(Panel::Bottom).min(spare_h))
            .filter(|height| *height >= min(Panel::Bottom));
        let left_w = left.map_or(0.0, f32::round);
        let right_w = right.map_or(0.0, f32::round);
        let bottom_h = bottom.map_or(0.0, f32::round);
        let middle_x = viewport.x + left_w;
        let middle_w = viewport.width - left_w - right_w;
        let mut panels = Vec::new();
        if left.is_some() {
            panels.push((
                left_panel,
                Rect::new(viewport.x, viewport.y, left_w, viewport.height),
            ));
        }
        if right.is_some() {
            panels.push((
                right_panel,
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
                Panel::Bottom,
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

    const LEFT: Panel = Panel::Sidebar(Edge::Left);
    const RIGHT: Panel = Panel::Sidebar(Edge::Right);

    /// Panels with `panels` shown, each holding one view, at scale 1.
    fn shown(panels: &[Panel]) -> Panels {
        let mut out = Panels::new();
        for (index, &panel) in panels.iter().enumerate() {
            let tab = TabId(10 + index as u64);
            match panel {
                Panel::Bottom => out.bottom.add(BottomPanelKind::Output, tab),
                Panel::Sidebar(edge) => out.sidebar_mut(edge).add(SidebarKind::Files, tab),
            };
        }
        out
    }

    #[test]
    fn test_the_sidebars_run_full_height_and_the_bottom_panel_spans_between() {
        let panels = shown(&[LEFT, RIGHT, Panel::Bottom]);
        let fitted = panels.fit(Rect::new(0.0, 0.0, 1600.0, 800.0), 1.0);
        assert_eq!(
            fitted.panels,
            [
                (LEFT, Rect::new(0.0, 0.0, 260.0, 800.0)),
                (RIGHT, Rect::new(1340.0, 0.0, 260.0, 800.0)),
                (Panel::Bottom, Rect::new(260.0, 580.0, 1080.0, 220.0)),
            ]
        );
        assert_eq!(fitted.main, Rect::new(260.0, 0.0, 1080.0, 580.0));
    }

    #[test]
    fn test_sizes_follow_the_scale_factor() {
        let panels = shown(&[LEFT]);
        let fitted = panels.fit(Rect::new(0.0, 0.0, 3200.0, 1600.0), 2.0);
        assert_eq!(fitted.panels, [(LEFT, Rect::new(0.0, 0.0, 520.0, 1600.0))]);
    }

    #[test]
    fn test_a_hidden_or_empty_panel_takes_no_room() {
        let mut panels = shown(&[LEFT]);
        panels.left.base.shown = false;
        panels.right.base.shown = true;
        let viewport = Rect::new(0.0, 0.0, 1000.0, 600.0);
        let fitted = panels.fit(viewport, 1.0);
        assert!(fitted.panels.is_empty());
        assert_eq!(fitted.main, viewport);
    }

    #[test]
    fn test_a_small_window_shrinks_the_right_sidebar_first_then_leaves_it_out() {
        let panels = shown(&[LEFT, RIGHT]);
        let widths = |width: f32| {
            panels
                .fit(Rect::new(0.0, 0.0, width, 600.0), 1.0)
                .panels
                .into_iter()
                .map(|(panel, rect)| (panel, rect.width))
                .collect::<Vec<_>>()
        };
        // The main area keeps 320px; the sidebars want 260px each.
        assert_eq!(
            widths(800.0),
            [(LEFT, 260.0), (RIGHT, 220.0)],
            "the right shrinks first"
        );
        assert_eq!(widths(560.0), [(LEFT, 240.0)], "then is left out");
        assert_eq!(widths(440.0), [], "then the left");
    }

    #[test]
    fn test_the_bottom_panel_shrinks_to_its_least_then_is_left_out() {
        let panels = shown(&[Panel::Bottom]);
        let fitted = panels.fit(Rect::new(0.0, 0.0, 800.0, 250.0), 1.0);
        assert_eq!(fitted.panels[0].1.height, 154.0, "the main area keeps 96px");
        let short = panels.fit(Rect::new(0.0, 0.0, 800.0, 170.0), 1.0);
        assert!(short.panels.is_empty(), "under 80px, left out");
    }

    #[test]
    fn test_a_dock_holds_one_view_per_kind_and_removing_the_shown_one_shows_the_one_before() {
        let mut dock = Dock::new(SIDEBAR_WIDTH);
        assert!(dock.add(SidebarKind::Files, TabId(1)));
        assert!(!dock.add(SidebarKind::Files, TabId(2)), "one Files view");
        assert!(dock.add(SidebarKind::Git, TabId(3)));
        assert!(dock.add(SidebarKind::Todo, TabId(4)));
        assert_eq!(dock.active_kind(), Some(SidebarKind::Todo));
        assert_eq!(dock.remove(SidebarKind::Todo), Some(TabId(4)));
        assert_eq!(dock.active_kind(), Some(SidebarKind::Git));
        dock.remove(SidebarKind::Git);
        dock.remove(SidebarKind::Files);
        assert_eq!(dock.base.state(), PanelState::Empty);
    }

    #[test]
    fn test_stepping_through_views_stops_at_the_ends() {
        let mut dock = Dock::new(BOTTOM_HEIGHT);
        dock.add(BottomPanelKind::Output, TabId(1));
        dock.add(BottomPanelKind::Problems, TabId(2));
        dock.base.active = Some(TabId(1));
        assert_eq!(dock.base.step(true), Some(TabId(2)));
        assert_eq!(dock.base.step(false), None);
        dock.base.active = Some(TabId(2));
        assert_eq!(dock.base.step(true), None);
    }
}

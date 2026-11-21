//! The window's layout: a main area holding a binary tree of splits whose
//! leaves are panes, each a strip of tabs with one of them shown, and three
//! panels around it, each showing views rather than tabs (see [`panels`]).
//! Pure geometry and tree surgery, independent of any renderer.

use misty::Rect;

pub mod panels;
mod tree;

use panels::Fitted;
pub use panels::{
    BOTTOM_HEIGHT, BottomPanelKind, Dock, DockSnapshot, Edge, MAX_PANEL_SIZE, Panel, PanelState,
    Panels, Region, SIDEBAR_WIDTH, SidebarKind,
};

use tree::{
    Node, Pane, balance_node, close_in, collect_pane_refs, collect_panes, distance, divider_hit_in,
    drag_in, find_pane, find_pane_mut, is_toward, layout_tree_to_node, node_to_layout_tree,
    split_at, split_off_tab, split_pane_with_tab,
};

// ========================================================================
// Constants
// ========================================================================

/// Pixel half-width of the invisible hit zone on each side of a split divider.
/// A pointer within this distance triggers the resize cursor and starts a drag.
pub const DIVIDER_HIT_MARGIN: f32 = 1.5;

/// Physical pixels to a logical one until the window says (see
/// [`Layout::set_scale`]).
const DEFAULT_SCALE: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The window's layout: the main area's split tree of panes, the panels
/// around it, and which tab or view has focus.
///
/// Every tab is a `TabId`, a terminal or an editor alike, allocated by the owner
/// so ids stay unique for the life of the window; a panel's views are named
/// the same way. Only the shown tab of each pane is laid out; the rest wait
/// in their pane's strip.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The shown tab of the focused pane, or the shown view of the focused
    /// panel.
    focused: TabId,
    /// The main-area tab that last had focus, for focus to come back to
    /// from a panel.
    main_return: TabId,
    /// The left and right sidebars and the bottom panel.
    panels: Panels,
    /// The main area's split tree.
    root: Node,
    /// Physical pixels to a logical one, which panels are measured in.
    scale: f32,
    /// When true, `rects()` returns only the focused pane at the full viewport;
    /// cleared when the user calls `toggle_zoom()` again.
    zoomed: bool,
}

/// A pane or a panel as laid out: where it sits and what its strip holds.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneArea {
    /// The shown tab, or a panel's shown view.
    pub active: TabId,
    /// Every tab in strip order, or a panel's views in its switcher's.
    pub members: Vec<TabId>,
    /// The whole pane, strip included.
    pub rect: Rect,
    /// Whether it is a pane of the main area or a panel.
    pub region: Region,
}

/// Which way a split's divider runs.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Direction {
    /// A horizontal divider: first child on top, second below.
    Horizontal,
    /// A vertical divider: first child on the left, second on the right.
    Vertical,
}

/// A directional focus move.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FocusDir {
    /// Toward the bottom of the screen.
    Down,
    /// Toward the left of the screen.
    Left,
    /// Toward the right of the screen.
    Right,
    /// Toward the top of the screen.
    Up,
}

/// Identifies one tab: a terminal or an editor.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct TabId(pub u64);

/// A serializable snapshot of a [`Layout`]'s split tree. Used by the session
/// module to persist and restore the layout across restarts, and by the
/// closed-pane stash to put a merged-away split back.
#[derive(Clone, Debug)]
pub enum LayoutTree {
    /// A leaf holding one pane.
    Pane(PaneTree),
    /// A split of two child trees.
    Split {
        /// Whether the children sit side by side or stacked.
        direction: Direction,
        /// Fraction of the space given to the first child.
        ratio: f32,
        /// The child above or to the left.
        first: Box<LayoutTree>,
        /// The child below or to the right.
        second: Box<LayoutTree>,
    },
}

/// A snapshot of one pane.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneTree {
    /// The shown tab; the first member when it names none of them.
    pub active: TabId,
    /// Every tab, in strip order.
    pub members: Vec<TabId>,
}

// ========================================================================
// Layout
// ========================================================================

impl Layout {
    // --------------------------------------------------------------------
    // Construction and snapshots
    // --------------------------------------------------------------------

    /// A layout whose single full-area pane holds `TabId(0)`, focused.
    pub fn new() -> Self {
        Self::with_root(TabId(0))
    }

    /// A layout with a single full-area pane holding `root`, focused, and
    /// its panels empty.
    pub fn with_root(root: TabId) -> Self {
        Self {
            focused: root,
            main_return: root,
            panels: Panels::new(),
            root: Node::Leaf(Pane::new(root)),
            scale: DEFAULT_SCALE,
            zoomed: false,
        }
    }

    /// Rebuild a `Layout`'s main area from a [`LayoutTree`] snapshot, e.g.
    /// on session restore, its panels empty. Panes the snapshot left empty
    /// are dropped; `None` when that leaves nothing at all.
    pub fn from_tree(tree: LayoutTree, focused: TabId) -> Option<Self> {
        let root = layout_tree_to_node(tree)?;
        let mut first = Vec::new();
        collect_pane_refs(&root, &mut first);
        let first = first[0].active;
        let mut layout = Self {
            focused: first,
            main_return: first,
            panels: Panels::new(),
            root,
            scale: DEFAULT_SCALE,
            zoomed: false,
        };
        layout.focus(focused);
        Some(layout)
    }

    /// Export the main area's split tree as a [`LayoutTree`] for session
    /// persistence; the panels go by [`Dock::snapshot`].
    pub fn export_tree(&self) -> LayoutTree {
        node_to_layout_tree(&self.root)
    }

    /// Take the panels out, leaving three empty ones: they belong to the
    /// window, and go from one workspace's layout to the next. With a view
    /// focused, focus goes to the main area.
    pub fn take_panels(&mut self) -> Panels {
        if self.panels.panel_of(self.focused).is_some() {
            let back = self.main_return();
            self.set_focus(back);
        }
        std::mem::replace(&mut self.panels, Panels::new())
    }

    /// Put `panels`, taken from another layout, in place of these.
    pub fn put_panels(&mut self, panels: Panels) {
        self.panels = panels;
    }

    /// Measure panels at `scale` physical pixels to a logical one: the
    /// window's scale factor.
    pub fn set_scale(&mut self, scale: f32) {
        if scale > 0.0 {
            self.scale = scale;
        }
    }

    // --------------------------------------------------------------------
    // Queries
    // --------------------------------------------------------------------

    /// What has focus: the shown tab of a pane, or a panel's shown view.
    pub fn focused(&self) -> TabId {
        self.focused
    }

    /// The shown tab of every pane of the main area, in tree order, then the
    /// shown view of every panel the user has shown, left, right, then
    /// bottom: what a frame draws, unless the window is too small for a
    /// panel.
    pub fn shown_tabs(&self) -> Vec<TabId> {
        let mut out = self.main_shown_tabs();
        out.extend(Panel::ALL.into_iter().filter_map(|panel| {
            let base = self.panels.base(panel);
            base.shown.then_some(base.active).flatten()
        }));
        out
    }

    /// Every tab, shown or not, in tree then strip order. A panel's views
    /// are no tabs, and are left out.
    pub fn members(&self) -> Vec<TabId> {
        self.main_members()
    }

    /// Every pane's tabs in strip order, one list per pane in tree order.
    pub fn pane_members(&self) -> Vec<Vec<TabId>> {
        let mut panes = Vec::new();
        collect_pane_refs(&self.root, &mut panes);
        panes.iter().map(|pane| pane.members.clone()).collect()
    }

    /// Every tab of the main area, in tree then strip order.
    pub fn main_members(&self) -> Vec<TabId> {
        let mut panes = Vec::new();
        collect_pane_refs(&self.root, &mut panes);
        panes
            .iter()
            .flat_map(|pane| pane.members.iter().copied())
            .collect()
    }

    /// Whether a pane holds the tab `tab`, or a panel the view `tab`.
    pub fn contains(&self, tab: TabId) -> bool {
        self.region_of(tab).is_some()
    }

    /// Where `tab` sits, if anywhere: a tab in the main area, a view in a
    /// panel.
    pub fn region_of(&self, tab: TabId) -> Option<Region> {
        if find_pane(&self.root, tab).is_some() {
            return Some(Region::Main);
        }
        self.panels.panel_of(tab).map(Region::Panel)
    }

    /// The panel holding the view `tab`, if one does.
    pub fn panel_of(&self, tab: TabId) -> Option<Panel> {
        self.panels.panel_of(tab)
    }

    /// Every tab of the pane holding `tab`, in strip order; empty for a
    /// view, which is no tab.
    pub fn tabs_in_pane(&self, tab: TabId) -> Vec<TabId> {
        find_pane(&self.root, tab)
            .map(|pane| pane.members.clone())
            .unwrap_or_default()
    }

    /// Whether `tab` is the tab its pane shows, or the view its panel shows,
    /// whether or not it is in sight (a zoom or a hidden panel hides some).
    pub fn is_pane_front(&self, tab: TabId) -> bool {
        match self.panels.panel_of(tab) {
            Some(panel) => self.panels.base(panel).active == Some(tab),
            None => find_pane(&self.root, tab).is_some_and(|pane| pane.active == tab),
        }
    }

    /// The tabs of the pane holding `tab`, most recently shown first; empty
    /// for a view.
    pub fn recent_in_pane(&self, tab: TabId) -> Vec<TabId> {
        find_pane(&self.root, tab)
            .map(|pane| pane.recent.clone())
            .unwrap_or_default()
    }

    /// The main-area tab focus goes back to when it leaves a panel: the one
    /// that last had focus there.
    pub fn main_return(&self) -> TabId {
        match find_pane(&self.root, self.main_return) {
            Some(_) => self.main_return,
            None => self.main_shown_tabs()[0],
        }
    }

    /// Each laid-out pane's shown tab, and each panel's shown view, paired
    /// with its whole area within `viewport`. When zoomed, only the focused
    /// tab, at the full viewport.
    pub fn rects(&self, viewport: Rect) -> Vec<(TabId, Rect)> {
        self.panes(viewport)
            .into_iter()
            .map(|area| (area.active, area.rect))
            .collect()
    }

    /// Every pane laid out within `viewport`: the main area's in tree order,
    /// then the panels that fit (see [`Panels::fit`]). When zoomed, only the
    /// focused tab's, at the full viewport.
    pub fn panes(&self, viewport: Rect) -> Vec<PaneArea> {
        let fitted = self.panels.fit(viewport, self.scale);
        let mut out = Vec::new();
        collect_panes(&self.root, fitted.main, &mut out);
        for (panel, rect) in fitted.panels {
            let base = self.panels.base(panel);
            if let Some(active) = base.active {
                out.push(PaneArea {
                    active,
                    members: base.views.clone(),
                    rect,
                    region: Region::Panel(panel),
                });
            }
        }
        if self.zoomed {
            out.retain(|area| area.active == self.focused);
            for area in &mut out {
                area.rect = viewport;
            }
        }
        out
    }

    /// The `Direction` of the divider, between panes or at a panel's edge,
    /// that `(px, py)` is within [`DIVIDER_HIT_MARGIN`] pixels of; `None`
    /// over none, or while zoomed.
    pub fn divider_at(&self, px: f32, py: f32, viewport: Rect) -> Option<Direction> {
        if self.zoomed {
            return None;
        }
        let fitted = self.panels.fit(viewport, self.scale);
        if let Some(panel) = panel_edge_at(&fitted, px, py, DIVIDER_HIT_MARGIN) {
            return Some(match panel {
                Panel::Bottom => Direction::Horizontal,
                Panel::Sidebar(_) => Direction::Vertical,
            });
        }
        if self.main_shown_tabs().len() <= 1 {
            return None;
        }
        divider_hit_in(&self.root, fitted.main, px, py)
    }

    /// Whether the focused pane is currently expanded to fill the full viewport.
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    // --------------------------------------------------------------------
    // Splits
    // --------------------------------------------------------------------

    /// Find the divider at `(start_x, start_y)` and move it by `(dx, dy)`:
    /// a split's ratio, or a panel's size, which follows the pointer to the
    /// nearest whole logical pixel. Call once per mouse-move event with the
    /// delta from the previous cursor position. Returns `true` when a
    /// divider was found and moved. No-op when zoomed.
    pub fn drag_divider(
        &mut self,
        start_x: f32,
        start_y: f32,
        dx: f32,
        dy: f32,
        viewport: Rect,
    ) -> bool {
        if self.zoomed {
            return false;
        }
        let fitted = self.panels.fit(viewport, self.scale);
        // A panel's edge snaps to a logical pixel, so the pointer may be up
        // to half of one from it between two moves.
        let reach = DIVIDER_HIT_MARGIN.max(self.scale / 2.0 + 1.0);
        if let Some(panel) = panel_edge_at(&fitted, start_x, start_y, reach) {
            let (x, y) = (start_x + dx, start_y + dy);
            let physical = match panel {
                Panel::Sidebar(Edge::Left) => x - viewport.x,
                Panel::Sidebar(Edge::Right) => viewport.x + viewport.width - x,
                Panel::Bottom => viewport.y + viewport.height - y,
            };
            let size = (physical / self.scale)
                .round()
                .clamp(panel.min_size(), MAX_PANEL_SIZE);
            self.panels.base_mut(panel).size = size;
            return true;
        }
        drag_in(&mut self.root, fitted.main, start_x, start_y, dx, dy)
    }

    /// Toggle the focused tab between filling the whole window and the
    /// normal layout. A panel's view does not zoom; nor does a zoom stay on
    /// once focus is in a panel.
    pub fn toggle_zoom(&mut self) {
        self.zoomed = !self.zoomed && self.panels.panel_of(self.focused).is_none();
    }

    /// Split the focused pane in two, placing a new pane holding only the
    /// caller-allocated `new_id` beside it and focusing it. A panel does not
    /// split: with a panel focused, the main pane that last had focus splits.
    /// A zoom ends: a split asks for both panes in sight.
    pub fn split(&mut self, direction: Direction, ratio: f32, new_id: TabId) {
        let target = match self.region_of(self.focused) {
            Some(Region::Main) => self.focused,
            _ => self.main_return(),
        };
        split_at(
            &mut self.root,
            target,
            direction,
            ratio.clamp(0.0, 1.0),
            new_id,
        );
        self.set_focus(new_id);
        self.zoomed = false;
    }

    /// Recompute every split's ratio so its two children evenly share that
    /// split's own axis. Panels keep their sizes.
    ///
    /// Each split's first child gets a share proportional to its weight along
    /// the split's own direction (see `axis_weight`): a chain of splits along
    /// the same direction telescopes into equal slots (three same-direction
    /// splits give thirds, four give quarters, ...), matching the staircase of
    /// halves a fixed 0.5 ratio would otherwise produce. Splits are only
    /// weighed against siblings on their own axis, so splitting or closing a
    /// pane inside one row/column never resizes a sibling row/column on a
    /// different axis elsewhere in the tree. No-op for a single pane.
    pub fn balance(&mut self) {
        balance_node(&mut self.root);
    }

    // --------------------------------------------------------------------
    // Tabs
    // --------------------------------------------------------------------

    /// Add `new_id` to the focused pane, right after the tab it is showing,
    /// and show and focus it. With a panel focused, it goes to the main pane
    /// that last had focus.
    pub fn add_tab(&mut self, new_id: TabId) {
        let target = match self.panels.panel_of(self.focused) {
            Some(_) => self.main_return(),
            None => self.focused,
        };
        if let Some(pane) = self.pane_of_mut(target) {
            let at = pane.position(pane.active) + 1;
            pane.members.insert(at, new_id);
            pane.show(new_id);
        }
        self.set_focus(new_id);
    }

    /// Add `new_id` to the pane holding `beside`, after the tab it shows,
    /// without showing it or moving the focus: a tab started in the
    /// background, there in the strip to go to.
    pub fn add_background_tab(&mut self, beside: TabId, new_id: TabId) {
        if let Some(pane) = self.pane_of_mut(beside) {
            let at = pane.position(pane.active) + 1;
            pane.members.insert(at, new_id);
            pane.recent.push(new_id);
        }
    }

    /// Remove one tab. A pane left empty collapses its parent split into the
    /// sibling, and the main area's last tab cannot be closed; a panel's
    /// view never closes. Returns whether anything changed.
    ///
    /// A pane that loses its shown tab shows the one shown before it, and
    /// focus follows when the closed tab had it.
    pub fn close(&mut self, tab: TabId) -> bool {
        let Some(pane) = find_pane_mut(&mut self.root, tab) else {
            return false;
        };
        if pane.members.len() > 1 {
            pane.remove(tab);
            let active = pane.active;
            if self.focused == tab {
                self.set_focus(active);
            }
            return true;
        }
        if !close_in(&mut self.root, tab) {
            return false;
        }
        if self.focused == tab {
            let next = self.main_shown_tabs()[0];
            self.set_focus(next);
        }
        true
    }

    /// Show and focus `tab`, wherever it is: a tab in its pane, a view in
    /// its panel, the panel shown and a zoom ended. Returns whether it
    /// exists.
    pub fn focus(&mut self, tab: TabId) -> bool {
        if let Some(panel) = self.panels.panel_of(tab) {
            let base = self.panels.base_mut(panel);
            base.active = Some(tab);
            base.shown = true;
            self.zoomed = false;
            self.set_focus(tab);
            return true;
        }
        let Some(pane) = self.pane_of_mut(tab) else {
            return false;
        };
        pane.show(tab);
        self.set_focus(tab);
        true
    }

    /// Add `new_id` to the pane holding `beside`, and optionally show and
    /// focus it.
    pub fn add_tab_beside(&mut self, beside: TabId, new_id: TabId, show: bool) {
        if let Some(pane) = self.pane_of_mut(beside) {
            let at = pane.position(pane.active) + 1;
            pane.members.insert(at, new_id);
            if show {
                pane.show(new_id);
            } else {
                pane.recent.push(new_id);
            }
        }
        if show {
            self.set_focus(new_id);
        }
    }

    /// Show the next (`forward`) or previous tab of the focused pane by strip
    /// position, or the next or previous view of the focused panel, without
    /// wrapping around. Returns whether the shown tab or view changed.
    pub fn cycle_tab(&mut self, forward: bool) -> bool {
        if let Some(panel) = self.panels.panel_of(self.focused) {
            return match self.panels.base(panel).step(forward) {
                Some(view) => self.focus(view),
                None => false,
            };
        }
        let members = self.tabs_in_pane(self.focused);
        let count = members.len();
        if count <= 1 {
            return false;
        }
        let index = members.iter().position(|&p| p == self.focused).unwrap_or(0);
        let next = match forward {
            true if index + 1 < count => index + 1,
            false if index > 0 => index - 1,
            _ => return false,
        };
        self.focus(members[next])
    }

    /// Move the focused tab one place along its strip, toward the end when
    /// `forward`. Returns whether it moved.
    pub fn move_tab(&mut self, forward: bool) -> bool {
        let focused = self.focused;
        let Some(pane) = self.pane_of_mut(focused) else {
            return false;
        };
        let at = pane.position(focused);
        let to = match forward {
            true if at + 1 < pane.members.len() => at + 1,
            false if at > 0 => at - 1,
            _ => return false,
        };
        pane.members.swap(at, to);
        true
    }

    /// Whether `a` and `b` belong to the same pane.
    pub fn same_pane(&self, a: TabId, b: TabId) -> bool {
        self.pane_of(a)
            .is_some_and(|pane| pane.members.contains(&b))
    }

    /// Move `tab` to where `target` sits in its strip. If `target` is in the
    /// same pane, reorders within the strip. If `target` is in another pane,
    /// moves `tab` there, to `target`'s position. If `tab` was
    /// the sole member of a main pane, that pane's split collapses; the main
    /// area's last tab stays.
    pub fn move_tab_to(&mut self, tab: TabId, target: TabId) -> bool {
        if tab == target {
            return false;
        }
        let Some(to_pane) = self.pane_of(target) else {
            return false;
        };
        if to_pane.members.contains(&tab) {
            let pane = self.pane_of_mut(tab).expect("found above");
            let from = pane.position(tab);
            let to = pane.position(target);
            let moved = pane.members.remove(from);
            pane.members.insert(to, moved);
            return true;
        }
        let to = to_pane.position(target);
        self.transfer_tab_to_pane(tab, target, Some(to))
    }

    /// Move `tab` into the pane holding `target_pane_tab`, placing
    /// it at the end of that strip.
    pub fn move_tab_to_pane(&mut self, tab: TabId, target_pane_tab: TabId) -> bool {
        if self.pane_of(tab).is_none() || self.pane_of(target_pane_tab).is_none() {
            return false;
        }
        if self.same_pane(tab, target_pane_tab) {
            let pane = self.pane_of_mut(tab).expect("found above");
            let from = pane.position(tab);
            let to = pane.members.len().saturating_sub(1);
            if from == to {
                return false;
            }
            let moved = pane.members.remove(from);
            pane.members.push(moved);
            return true;
        }
        self.transfer_tab_to_pane(tab, target_pane_tab, None)
    }

    /// Move `tab` out of wherever it is into the strip holding `target_tab`,
    /// at `insert_at` (its end when `None`), showing and focusing it.
    fn transfer_tab_to_pane(
        &mut self,
        tab: TabId,
        target_tab: TabId,
        insert_at: Option<usize>,
    ) -> bool {
        if self.pane_of(target_tab).is_none() {
            return false;
        }
        let collapses = find_pane(&self.root, tab).is_some_and(|pane| pane.members.len() == 1);
        if !self.take_tab(tab) {
            return false;
        }
        let Some(to_pane) = self.pane_of_mut(target_tab) else {
            return false;
        };
        let pos = insert_at
            .unwrap_or(to_pane.members.len())
            .min(to_pane.members.len());
        to_pane.members.insert(pos, tab);
        to_pane.show(tab);
        self.set_focus(tab);
        if collapses {
            self.balance();
        }
        true
    }

    /// Split `target_pane_tab`'s pane to make `tab` a new pane in `direction`.
    /// Returns `true` if a new pane was created; `false` if either tab was
    /// not found, if the target is a panel's view, or if
    /// `tab` was the only tab of the target pane.
    pub fn split_tab_to_new_pane(
        &mut self,
        tab: TabId,
        target_pane_tab: TabId,
        direction: FocusDir,
    ) -> bool {
        if self.region_of(target_pane_tab) != Some(Region::Main) {
            return false;
        }
        let Some(from_len) = self.pane_of(tab).map(|pane| pane.members.len()) else {
            return false;
        };
        if self.same_pane(tab, target_pane_tab) {
            if from_len <= 1 || !split_off_tab(&mut self.root, tab, direction) {
                return false;
            }
        } else {
            if !self.take_tab(tab) {
                return false;
            }
            if !split_pane_with_tab(&mut self.root, target_pane_tab, tab, direction) {
                return false;
            }
        }
        self.set_focus(tab);
        self.balance();
        true
    }

    /// Move `tab` into a new pane in `direction` beside its current pane.
    /// Returns `true` if a new pane was created, `false` if `tab` was not found
    /// or was the only tab in its pane.
    pub fn move_tab_to_new_pane(&mut self, tab: TabId, direction: FocusDir) -> bool {
        self.split_tab_to_new_pane(tab, tab, direction)
    }

    // --------------------------------------------------------------------
    // Panels
    // --------------------------------------------------------------------

    /// Whether `panel` holds views, and whether it is shown.
    pub fn panel_state(&self, panel: Panel) -> PanelState {
        self.panels.base(panel).state()
    }

    /// The sidebar on `edge`.
    pub fn sidebar(&self, edge: Edge) -> &Dock<SidebarKind> {
        self.panels.sidebar(edge)
    }

    /// The bottom panel.
    pub fn bottom_panel(&self) -> &Dock<BottomPanelKind> {
        &self.panels.bottom
    }

    /// The sidebar holding a view of `kind`, if one does.
    pub fn sidebar_of(&self, kind: SidebarKind) -> Option<Edge> {
        self.panels.sidebar_of(kind)
    }

    /// Show `panel` and focus the view it shows; a zoom ends. Returns
    /// `false`, doing nothing, for an empty panel.
    pub fn show_panel(&mut self, panel: Panel) -> bool {
        match self.panels.base(panel).active {
            Some(view) => self.focus(view),
            None => false,
        }
    }

    /// Hide `panel`, its views kept; with focus in it, focus goes back to the
    /// main pane it came from.
    pub fn hide_panel(&mut self, panel: Panel) {
        self.panels.base_mut(panel).shown = false;
        if self.panels.panel_of(self.focused) == Some(panel) {
            let back = self.main_return();
            self.set_focus(back);
        }
    }

    /// Show or hide `panel`, leaving the focus where it is unless it was in
    /// the panel hidden. An empty panel stays hidden.
    pub fn set_panel_shown(&mut self, panel: Panel, shown: bool) {
        match shown {
            true => {
                let base = self.panels.base_mut(panel);
                base.shown = base.active.is_some();
            }
            false => self.hide_panel(panel),
        }
    }

    /// Set `panel`'s width (a sidebar) or height, in logical pixels, within
    /// its least and [`MAX_PANEL_SIZE`].
    pub fn set_panel_size(&mut self, panel: Panel, size: f32) {
        self.panels.base_mut(panel).size = size.clamp(panel.min_size(), MAX_PANEL_SIZE);
    }

    /// Add the view `tab` of `kind` to the sidebar on `edge` and show it
    /// there, focusing it when `focus`. Returns `false`, changing nothing,
    /// when either sidebar holds a view of `kind` already, a view sitting in
    /// one sidebar at a time.
    pub fn add_sidebar_view(
        &mut self,
        edge: Edge,
        kind: SidebarKind,
        tab: TabId,
        focus: bool,
    ) -> bool {
        if self.panels.sidebar_of(kind).is_some() || self.contains(tab) {
            return false;
        }
        self.panels.sidebar_mut(edge).add(kind, tab);
        if focus {
            self.focus(tab);
        }
        true
    }

    /// Move the view of `kind` to the sidebar on `edge`, showing and
    /// focusing it there. Returns `false` when no sidebar holds one, or it
    /// is there already.
    pub fn move_sidebar_view(&mut self, kind: SidebarKind, edge: Edge) -> bool {
        let Some(from) = self.panels.sidebar_of(kind) else {
            return false;
        };
        if from == edge {
            return false;
        }
        let Some(tab) = self.panels.sidebar_mut(from).remove(kind) else {
            return false;
        };
        self.panels.sidebar_mut(edge).add(kind, tab);
        self.focus(tab);
        true
    }

    /// Add the view `tab` of `kind` to the bottom panel and show it there,
    /// focusing it when `focus`. Returns `false`, changing nothing, when the
    /// bottom panel holds a view of `kind` already.
    pub fn add_bottom_view(&mut self, kind: BottomPanelKind, tab: TabId, focus: bool) -> bool {
        if self.contains(tab) || !self.panels.bottom.add(kind, tab) {
            return false;
        }
        if focus {
            self.focus(tab);
        }
        true
    }

    /// Bring `tab` to the front of its pane, or the view `tab` to the front
    /// of its panel, without moving the focus. Returns whether it is held.
    pub fn bring_to_front(&mut self, tab: TabId) -> bool {
        if let Some(panel) = self.panels.panel_of(tab) {
            self.panels.base_mut(panel).active = Some(tab);
            return true;
        }
        match self.pane_of_mut(tab) {
            Some(pane) => {
                pane.show(tab);
                true
            }
            None => false,
        }
    }

    // --------------------------------------------------------------------
    // Focus between panes
    // --------------------------------------------------------------------

    /// Focus the main-area pane at position `index` in tree order (0-based);
    /// panels are reached by their own keys. Returns whether the index was
    /// in range.
    pub fn focus_by_index(&mut self, index: usize) -> bool {
        match self.main_shown_tabs().get(index) {
            Some(&id) => {
                self.set_focus(id);
                true
            }
            None => false,
        }
    }

    /// Focus the next shown pane or panel in order, wrapping around.
    pub fn focus_next(&mut self) {
        let panes = self.shown_tabs();
        if let Some(index) = panes.iter().position(|&p| p == self.focused) {
            self.set_focus(panes[(index + 1) % panes.len()]);
        }
    }

    /// Find the nearest pane or panel in the given direction within
    /// `viewport`, by the distance between their centers, or `None` if there
    /// is none that way. Out of a panel into the main area, the main pane
    /// that last had focus wins where it lies that way.
    pub fn pane_in_direction(&self, direction: FocusDir, viewport: Rect) -> Option<TabId> {
        let areas = self.panes(viewport);
        let current = areas.iter().find(|area| area.active == self.focused)?;
        let from = center_of(current.rect);
        let toward = |area: &&PaneArea| {
            area.active != self.focused
                && match direction {
                    FocusDir::Up | FocusDir::Down => overlap_x(current.rect, area.rect) > 1.0,
                    FocusDir::Left | FocusDir::Right => overlap_y(current.rect, area.rect) > 1.0,
                }
                && is_toward(direction, from, center_of(area.rect))
        };
        let nearest = areas.iter().filter(toward).min_by(|a, b| {
            distance(from, center_of(a.rect)).total_cmp(&distance(from, center_of(b.rect)))
        })?;
        if current.region != Region::Main && nearest.region == Region::Main {
            let back = self.main_return();
            if let Some(area) = areas.iter().filter(toward).find(|area| area.active == back) {
                return Some(area.active);
            }
        }
        Some(nearest.active)
    }

    /// Focus the nearest pane or panel in the given direction within
    /// `viewport` (see [`Layout::pane_in_direction`]). Returns whether
    /// focus moved.
    pub fn focus_in_direction(&mut self, direction: FocusDir, viewport: Rect) -> bool {
        if let Some(target) = self.pane_in_direction(direction, viewport) {
            self.set_focus(target);
            true
        } else {
            false
        }
    }

    /// Focus the pane beside the focused one in the same row: one whose height
    /// overlaps it, to its right first, else to its left, the nearest of
    /// either. Returns whether focus moved, which is false when nothing sits
    /// beside it.
    pub fn focus_side_by_side(&mut self, viewport: Rect) -> bool {
        let rects = self.rects(viewport);
        let Some(&(_, current)) = rects.iter().find(|(id, _)| *id == self.focused) else {
            return false;
        };
        let from = center_of(current);
        let in_row =
            |rect: &Rect| rect.y < current.y + current.height && current.y < rect.y + rect.height;
        let nearest = |toward: FocusDir| {
            rects
                .iter()
                .filter(|(id, rect)| *id != self.focused && in_row(rect))
                .filter(|(_, rect)| is_toward(toward, from, center_of(*rect)))
                .min_by(|a, b| {
                    distance(from, center_of(a.1)).total_cmp(&distance(from, center_of(b.1)))
                })
                .map(|(id, _)| *id)
        };
        match nearest(FocusDir::Right).or_else(|| nearest(FocusDir::Left)) {
            Some(id) => {
                self.set_focus(id);
                true
            }
            None => false,
        }
    }

    // --------------------------------------------------------------------
    // Helpers
    // --------------------------------------------------------------------

    /// The shown tab of every pane of the main area, in tree order.
    pub fn main_shown_tabs(&self) -> Vec<TabId> {
        let mut panes = Vec::new();
        collect_pane_refs(&self.root, &mut panes);
        panes.iter().map(|pane| pane.active).collect()
    }

    /// The pane holding `tab`.
    fn pane_of(&self, tab: TabId) -> Option<&Pane> {
        find_pane(&self.root, tab)
    }

    /// The pane holding `tab`, mutably.
    fn pane_of_mut(&mut self, tab: TabId) -> Option<&mut Pane> {
        find_pane_mut(&mut self.root, tab)
    }

    /// Take `tab` out of its pane: a pane left empty collapses. Returns
    /// `false`, changing nothing, for a tab not held or the main area's last
    /// tab.
    fn take_tab(&mut self, tab: TabId) -> bool {
        match find_pane_mut(&mut self.root, tab) {
            Some(pane) if pane.members.len() > 1 => {
                pane.remove(tab);
                true
            }
            Some(_) => close_in(&mut self.root, tab),
            None => false,
        }
    }

    /// Give `tab` focus, remembering it when it is in the main area, for
    /// focus to come back to from a panel.
    fn set_focus(&mut self, tab: TabId) {
        if find_pane(&self.root, tab).is_some() {
            self.main_return = tab;
        }
        self.focused = tab;
    }
}

impl Default for Layout {
    fn default() -> Self {
        Self::new()
    }
}

// ========================================================================
// LayoutTree
// ========================================================================

impl LayoutTree {
    /// A leaf holding one pane of a single tab.
    pub fn single(tab: TabId) -> Self {
        LayoutTree::Pane(PaneTree {
            active: tab,
            members: vec![tab],
        })
    }

    /// Every tab the snapshot names, in tree then strip order.
    pub fn members(&self) -> Vec<TabId> {
        match self {
            LayoutTree::Pane(pane) => pane.members.clone(),
            LayoutTree::Split { first, second, .. } => {
                let mut out = first.members();
                out.extend(second.members());
                out
            }
        }
    }

    /// The same tree holding only the tabs `keep` accepts.
    pub fn retain(self, keep: &impl Fn(TabId) -> bool) -> Self {
        match self {
            LayoutTree::Pane(mut pane) => {
                pane.members.retain(|&p| keep(p));
                LayoutTree::Pane(pane)
            }
            LayoutTree::Split {
                direction,
                ratio,
                first,
                second,
            } => LayoutTree::Split {
                direction,
                ratio,
                first: Box::new(first.retain(keep)),
                second: Box::new(second.retain(keep)),
            },
        }
    }
}

// ========================================================================
// Rect helpers
// ========================================================================

fn center_of(rect: Rect) -> (f32, f32) {
    (rect.x + rect.width / 2.0, rect.y + rect.height / 2.0)
}

fn overlap_x(a: Rect, b: Rect) -> f32 {
    let start = a.x.max(b.x);
    let end = (a.x + a.width).min(b.x + b.width);
    (end - start).max(0.0)
}

fn overlap_y(a: Rect, b: Rect) -> f32 {
    let start = a.y.max(b.y);
    let end = (a.y + a.height).min(b.y + b.height);
    (end - start).max(0.0)
}

/// The panel of `fitted` whose edge toward the main area `(px, py)` is
/// within `reach` pixels of, along that edge.
fn panel_edge_at(fitted: &Fitted, px: f32, py: f32, reach: f32) -> Option<Panel> {
    fitted.panels.iter().find_map(|&(panel, rect)| {
        let along_y = py >= rect.y && py <= rect.y + rect.height;
        let along_x = px >= rect.x && px <= rect.x + rect.width;
        let near = match panel {
            Panel::Sidebar(Edge::Left) => along_y && (px - (rect.x + rect.width)).abs() <= reach,
            Panel::Sidebar(Edge::Right) => along_y && (px - rect.x).abs() <= reach,
            Panel::Bottom => along_x && (py - rect.y).abs() <= reach,
        };
        near.then_some(panel)
    })
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::tree::split_area;
    use super::*;

    /// A window wide enough for every panel, in cells of 10 by 20 pixels.
    const WIDE: Rect = Rect {
        height: 800.0,
        width: 1600.0,
        x: 0.0,
        y: 0.0,
    };

    /// A layout of one main tab, `TabId(0)`, at scale 1.
    fn with_panels() -> Layout {
        Layout::with_root(TabId(0))
    }

    const LEFT: Panel = Panel::Sidebar(Edge::Left);
    const RIGHT: Panel = Panel::Sidebar(Edge::Right);

    /// Where `tab` is laid out in [`WIDE`], and in which region.
    fn placed(layout: &Layout, tab: TabId) -> Option<(Region, Rect)> {
        layout
            .panes(WIDE)
            .into_iter()
            .find(|area| area.active == tab)
            .map(|area| (area.region, area.rect))
    }

    const VIEWPORT: Rect = Rect {
        height: 100.0,
        width: 200.0,
        x: 0.0,
        y: 0.0,
    };

    #[test]
    fn test_a_background_tab_joins_the_strip_without_showing_or_taking_focus() {
        let mut layout = Layout::with_root(TabId(0));
        layout.add_background_tab(TabId(0), TabId(1));
        assert_eq!(layout.members(), vec![TabId(0), TabId(1)]);
        assert_eq!(layout.shown_tabs(), vec![TabId(0)]);
        assert_eq!(layout.focused(), TabId(0));
    }

    #[test]
    fn test_panels_sit_around_the_main_area_and_keep_their_size_through_a_balance() {
        let mut layout = with_panels();
        layout.split(Direction::Vertical, 0.3, TabId(1));
        layout.add_bottom_view(BottomPanelKind::Output, TabId(2), true);
        layout.add_sidebar_view(Edge::Left, SidebarKind::Files, TabId(3), false);
        assert_eq!(layout.focused(), TabId(2));
        let (region, bottom) = placed(&layout, TabId(2)).expect("laid out");
        assert_eq!(region, Region::Panel(Panel::Bottom));
        let (_, left) = placed(&layout, TabId(3)).expect("laid out");
        assert_eq!(left, Rect::new(0.0, 0.0, 260.0, 800.0), "full height");
        assert_eq!(bottom.x, 260.0, "between the sidebars");
        assert_eq!(bottom.height, BOTTOM_HEIGHT);
        layout.balance();
        let (_, main_a) = placed(&layout, TabId(0)).expect("laid out");
        let (_, main_b) = placed(&layout, TabId(1)).expect("laid out");
        assert_eq!(main_a.width, main_b.width, "the main panes balanced");
        assert_eq!(
            placed(&layout, TabId(2)).map(|(_, rect)| rect),
            Some(bottom)
        );
    }

    #[test]
    fn test_views_are_no_tabs_new_tabs_go_to_the_main_area_and_views_never_close() {
        let mut layout = with_panels();
        layout.add_bottom_view(BottomPanelKind::Output, TabId(1), true);
        layout.add_tab(TabId(2));
        assert_eq!(
            layout.region_of(TabId(2)),
            Some(Region::Main),
            "beside the main tab"
        );
        assert_eq!(
            layout.members(),
            [TabId(0), TabId(2)],
            "the view is no member"
        );
        assert!(layout.tabs_in_pane(TabId(1)).is_empty());
        assert!(!layout.close(TabId(1)), "a view never closes");
        assert!(layout.contains(TabId(1)));
        assert!(
            !layout.move_tab_to(TabId(2), TabId(1)),
            "nothing moves into a panel"
        );
    }

    #[test]
    fn test_a_hidden_panel_keeps_its_views_and_comes_back_when_one_is_focused() {
        let mut layout = with_panels();
        layout.add_bottom_view(BottomPanelKind::Output, TabId(1), true);
        layout.hide_panel(Panel::Bottom);
        assert_eq!(layout.panel_state(Panel::Bottom), PanelState::Hidden);
        assert_eq!(layout.focused(), TabId(0), "back where focus was");
        assert_eq!(
            layout.rects(WIDE),
            [(TabId(0), WIDE)],
            "the main area takes it all"
        );
        layout.focus(TabId(1));
        assert_eq!(layout.panel_state(Panel::Bottom), PanelState::Shown);
        layout.hide_panel(Panel::Bottom);
        assert!(layout.show_panel(Panel::Bottom));
        assert_eq!(layout.focused(), TabId(1), "the view it shows");
        assert!(
            !layout.show_panel(RIGHT),
            "an empty panel has nothing to show"
        );
    }

    #[test]
    fn test_a_sidebar_view_sits_in_one_sidebar_and_moves_between_them() {
        let mut layout = with_panels();
        assert!(layout.add_sidebar_view(Edge::Left, SidebarKind::Files, TabId(1), true));
        assert!(
            !layout.add_sidebar_view(Edge::Right, SidebarKind::Files, TabId(2), true),
            "one Files view"
        );
        assert!(layout.add_sidebar_view(Edge::Left, SidebarKind::Git, TabId(3), true));
        assert!(layout.move_sidebar_view(SidebarKind::Files, Edge::Right));
        assert_eq!(layout.sidebar_of(SidebarKind::Files), Some(Edge::Right));
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(layout.sidebar(Edge::Left).kinds(), [SidebarKind::Git]);
        assert_eq!(
            layout.panel_state(LEFT),
            PanelState::Shown,
            "Git still there"
        );
    }

    #[test]
    fn test_next_and_previous_tab_step_through_a_panel_s_views_without_wrapping() {
        let mut layout = with_panels();
        layout.add_bottom_view(BottomPanelKind::Output, TabId(1), false);
        layout.add_bottom_view(BottomPanelKind::Problems, TabId(2), false);
        layout.focus(TabId(1));
        assert!(!layout.cycle_tab(false), "stops at the first");
        assert!(layout.cycle_tab(true));
        assert_eq!(layout.focused(), TabId(2));
        assert_eq!(
            layout.bottom_panel().active_kind(),
            Some(BottomPanelKind::Problems)
        );
        assert!(!layout.cycle_tab(true), "stops at the last");
    }

    #[test]
    fn test_a_split_with_a_panel_focused_splits_the_main_pane_that_last_had_focus() {
        let mut layout = with_panels();
        layout.split(Direction::Vertical, 0.5, TabId(1));
        layout.focus(TabId(0));
        layout.add_bottom_view(BottomPanelKind::Output, TabId(2), true);
        layout.split(Direction::Horizontal, 0.5, TabId(3));
        assert_eq!(layout.region_of(TabId(3)), Some(Region::Main));
        let (_, beside) = placed(&layout, TabId(3)).expect("laid out");
        let (_, split_from) = placed(&layout, TabId(0)).expect("laid out");
        assert_eq!(beside.x, split_from.x, "under the pane split from");
    }

    #[test]
    fn test_leaving_a_panel_goes_to_the_main_pane_that_last_had_focus() {
        let mut layout = with_panels();
        layout.split(Direction::Horizontal, 0.5, TabId(1));
        layout.focus(TabId(0));
        layout.add_sidebar_view(Edge::Left, SidebarKind::Files, TabId(2), true);
        // From the left sidebar, both main panes lie to the right; the top
        // one had focus last.
        assert!(layout.focus_in_direction(FocusDir::Right, WIDE));
        assert_eq!(layout.focused(), TabId(0));
        layout.focus(TabId(1));
        layout.focus(TabId(2));
        assert!(layout.focus_in_direction(FocusDir::Right, WIDE));
        assert_eq!(layout.focused(), TabId(1));
    }

    #[test]
    fn test_pane_numbers_count_the_main_area_only() {
        let mut layout = with_panels();
        layout.add_sidebar_view(Edge::Left, SidebarKind::Files, TabId(1), true);
        assert!(!layout.focus_by_index(1), "the panel is no pane number");
        assert!(layout.focus_by_index(0));
        assert_eq!(layout.focused(), TabId(0));
    }

    #[test]
    fn test_a_view_does_not_zoom() {
        let mut layout = with_panels();
        layout.add_bottom_view(BottomPanelKind::Output, TabId(1), true);
        layout.toggle_zoom();
        assert!(!layout.is_zoomed());
    }

    #[test]
    fn test_panels_go_from_one_layout_to_the_next_and_focus_stays_in_a_main_area() {
        let mut layout = with_panels();
        layout.add_sidebar_view(Edge::Right, SidebarKind::Outline, TabId(1), true);
        layout.set_panel_size(RIGHT, 300.0);
        let panels = layout.take_panels();
        assert_eq!(layout.focused(), TabId(0), "focus left the panel taken");
        assert_eq!(layout.panel_state(RIGHT), PanelState::Empty);
        let mut next = Layout::with_root(TabId(5));
        next.put_panels(panels);
        assert_eq!(
            next.sidebar(Edge::Right).tab_of(SidebarKind::Outline),
            Some(TabId(1))
        );
        assert_eq!(next.sidebar(Edge::Right).size(), 300.0);
    }

    #[test]
    fn test_dragging_a_panel_s_edge_sets_its_size_in_logical_pixels() {
        let mut layout = with_panels();
        layout.set_scale(2.0);
        layout.add_sidebar_view(Edge::Left, SidebarKind::Files, TabId(1), false);
        assert_eq!(
            layout.divider_at(520.0, 400.0, WIDE),
            Some(Direction::Vertical)
        );
        assert!(layout.drag_divider(520.0, 400.0, 47.0, 0.0, WIDE));
        assert_eq!(
            layout.sidebar(Edge::Left).size(),
            284.0,
            "567px at scale 2 is 283.5"
        );
        // The edge is now at 568px, 1px from the pointer: still held.
        assert!(layout.drag_divider(567.0, 400.0, -500.0, 0.0, WIDE));
        assert_eq!(
            layout.sidebar(Edge::Left).size(),
            LEFT.min_size(),
            "no smaller than its least"
        );
    }

    #[test]
    fn test_side_by_side_focus_picks_the_pane_in_the_same_row_not_the_next_in_tree_order() {
        // a over b, then c across the whole right half.
        let (a, b, c) = (TabId(0), TabId(1), TabId(2));
        let mut tab = Layout::with_root(a);
        tab.split(Direction::Vertical, 0.5, c);
        tab.focus(a);
        tab.split(Direction::Horizontal, 0.5, b);
        assert_eq!(tab.shown_tabs(), vec![a, b, c], "tree order would go to b");

        tab.focus(a);
        assert!(tab.focus_side_by_side(VIEWPORT));
        assert_eq!(tab.focused(), c, "c is beside a; b is under it");

        tab.focus(b);
        assert!(tab.focus_side_by_side(VIEWPORT));
        assert_eq!(tab.focused(), c);
    }

    #[test]
    fn test_side_by_side_focus_goes_left_when_nothing_is_to_the_right() {
        let (a, c) = (TabId(0), TabId(2));
        let mut tab = Layout::with_root(a);
        tab.split(Direction::Vertical, 0.5, c);
        assert!(tab.focus_side_by_side(VIEWPORT));
        assert_eq!(tab.focused(), a);
    }

    #[test]
    fn test_side_by_side_focus_finds_nothing_in_a_stack() {
        let (a, b) = (TabId(0), TabId(1));
        let mut tab = Layout::with_root(a);
        tab.split(Direction::Horizontal, 0.5, b);
        assert!(!tab.focus_side_by_side(VIEWPORT));
        assert_eq!(tab.focused(), b, "focus did not move");
    }

    #[test]
    fn test_new_tab_has_one_focused_pane() {
        let tab = Layout::new();
        assert_eq!(tab.shown_tabs(), vec![TabId(0)]);
        assert_eq!(tab.focused(), TabId(0));
    }

    #[test]
    fn test_from_tree_clamps_an_out_of_range_ratio() {
        // Regression: in-app split mutators always clamp their ratio to
        // [0, 1], but a deserialized `session.json` isn't guaranteed to; an
        // out-of-range ratio produced a negative-width/height rect that
        // silently misrendered instead of erroring.
        let too_big = LayoutTree::Split {
            direction: Direction::Vertical,
            ratio: 5.0,
            first: Box::new(LayoutTree::single(TabId(0))),
            second: Box::new(LayoutTree::single(TabId(1))),
        };
        match Layout::from_tree(too_big, TabId(0)).unwrap().export_tree() {
            LayoutTree::Split { ratio, .. } => assert_eq!(ratio, 1.0),
            LayoutTree::Pane(_) => panic!("expected a split"),
        }

        let too_small = LayoutTree::Split {
            direction: Direction::Vertical,
            ratio: -3.0,
            first: Box::new(LayoutTree::single(TabId(0))),
            second: Box::new(LayoutTree::single(TabId(1))),
        };
        match Layout::from_tree(too_small, TabId(0))
            .unwrap()
            .export_tree()
        {
            LayoutTree::Split { ratio, .. } => assert_eq!(ratio, 0.0),
            LayoutTree::Pane(_) => panic!("expected a split"),
        }
    }

    #[test]
    fn test_split_adds_a_focused_pane_and_divides_the_area() {
        let mut tab = Layout::new();
        let right = TabId(1);
        tab.split(Direction::Vertical, 0.5, right);
        assert_eq!(tab.focused(), right);
        assert_eq!(tab.shown_tabs(), vec![TabId(0), right]);

        let rects = tab.rects(VIEWPORT);
        assert_eq!(rects[0], (TabId(0), Rect::new(0.0, 0.0, 100.0, 100.0)));
        assert_eq!(rects[1], (right, Rect::new(100.0, 0.0, 100.0, 100.0)));
    }

    #[test]
    fn test_close_collapses_split_into_sibling() {
        let mut tab = Layout::new();
        let right = TabId(1);
        tab.split(Direction::Vertical, 0.5, right);
        assert!(tab.close(right));
        assert_eq!(tab.shown_tabs(), vec![TabId(0)]);
        assert_eq!(tab.focused(), TabId(0));
        assert_eq!(tab.rects(VIEWPORT), vec![(TabId(0), VIEWPORT)]);
    }

    #[test]
    fn test_last_pane_cannot_be_closed() {
        let mut tab = Layout::new();
        assert!(!tab.close(TabId(0)));
        assert_eq!(tab.shown_tabs(), vec![TabId(0)]);
    }

    #[test]
    fn test_focus_next_wraps_around() {
        let mut tab = Layout::new();
        let right = TabId(1);
        tab.split(Direction::Vertical, 0.5, right);
        tab.focus(TabId(0));
        tab.focus_next();
        assert_eq!(tab.focused(), right);
        tab.focus_next();
        assert_eq!(tab.focused(), TabId(0));
    }

    #[test]
    fn test_splitting_a_zoomed_pane_shows_both_halves() {
        // The zoom was kept, so only the new pane showed and the one split
        // from was hidden until the user unzoomed.
        let mut tab = Layout::new();
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.toggle_zoom();
        tab.split(Direction::Horizontal, 0.5, TabId(2));
        assert!(!tab.is_zoomed());
        assert_eq!(tab.rects(VIEWPORT).len(), 3);
    }

    #[test]
    fn test_zoom_returns_full_viewport_for_focused_pane() {
        let mut tab = Layout::new();
        let right = TabId(1);
        tab.split(Direction::Vertical, 0.5, right);
        tab.focus(TabId(0));
        assert!(!tab.is_zoomed());
        tab.toggle_zoom();
        assert!(tab.is_zoomed());
        let rects = tab.rects(VIEWPORT);
        assert_eq!(rects.len(), 1, "only focused pane when zoomed");
        assert_eq!(rects[0], (TabId(0), VIEWPORT));
        tab.toggle_zoom();
        assert!(!tab.is_zoomed());
        let rects = tab.rects(VIEWPORT);
        assert_eq!(rects.len(), 2, "both panes restored after unzoom");
    }

    #[test]
    fn test_focus_in_direction_moves_to_the_adjacent_pane() {
        let mut tab = Layout::new();
        let right = TabId(1);
        tab.split(Direction::Vertical, 0.5, right);
        tab.focus(TabId(0));
        assert!(tab.focus_in_direction(FocusDir::Right, VIEWPORT));
        assert_eq!(tab.focused(), right);
        assert!(!tab.focus_in_direction(FocusDir::Right, VIEWPORT));
        assert!(tab.focus_in_direction(FocusDir::Left, VIEWPORT));
        assert_eq!(tab.focused(), TabId(0));
    }

    fn area_of(rects: &[(TabId, Rect)], id: TabId) -> f32 {
        rects
            .iter()
            .find(|(pid, _)| *pid == id)
            .map(|(_, r)| r.width * r.height)
            .unwrap_or(f32::NAN)
    }

    #[test]
    fn test_balance_is_noop_for_a_single_pane() {
        let mut tab = Layout::new();
        tab.balance();
        assert_eq!(tab.rects(VIEWPORT), vec![(TabId(0), VIEWPORT)]);
    }

    #[test]
    fn test_a_split_of_an_odd_size_cuts_on_a_whole_pixel_and_leaves_no_gap() {
        let odd = Rect::new(10.0, 30.0, 721.0, 481.0);
        for direction in [Direction::Vertical, Direction::Horizontal] {
            let (first, second) = split_area(odd, direction, 0.5);
            let (cut, next, end) = match direction {
                Direction::Vertical => (first.x + first.width, second.x, second.x + second.width),
                Direction::Horizontal => {
                    (first.y + first.height, second.y, second.y + second.height)
                }
            };
            assert_eq!(cut, cut.round(), "{direction:?}");
            assert_eq!(cut, next, "{direction:?}");
            let odd_end = match direction {
                Direction::Vertical => odd.x + odd.width,
                Direction::Horizontal => odd.y + odd.height,
            };
            assert_eq!(end, odd_end, "{direction:?}");
        }
    }

    #[test]
    fn test_balance_equalizes_three_same_direction_splits() {
        let mut tab = Layout::new();
        // Two successive vertical splits of the focused pane build a staircase
        // (50% / 25% / 25%) without balancing.
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.split(Direction::Vertical, 0.5, TabId(2));
        tab.balance();

        let rects = tab.rects(VIEWPORT);
        let third = VIEWPORT.width * VIEWPORT.height / 3.0;
        for id in [TabId(0), TabId(1), TabId(2)] {
            assert!(
                // Within a column of pixels: splits fall on whole pixels.
                (area_of(&rects, id) - third).abs() <= VIEWPORT.height,
                "pane {id:?} should occupy a third after balance"
            );
        }
    }

    #[test]
    fn test_balance_keeps_mixed_directions_local_to_their_own_axis() {
        let mut tab = Layout::new();
        tab.split(Direction::Vertical, 0.5, TabId(1)); // 0 | 1
        tab.split(Direction::Horizontal, 0.5, TabId(2)); // 0 | (1 over 2)
        tab.balance();

        // The horizontal split of 1 shares column space with 0 (its own axis is
        // vertical, unaffected), so 0 keeps half the width; 1 and 2 split that
        // remaining column into equal-height quarters.
        let rects = tab.rects(VIEWPORT);
        let total = VIEWPORT.width * VIEWPORT.height;
        assert!((area_of(&rects, TabId(0)) - total / 2.0).abs() < 0.01);
        assert!((area_of(&rects, TabId(1)) - total / 4.0).abs() < 0.01);
        assert!((area_of(&rects, TabId(2)) - total / 4.0).abs() < 0.01);
    }

    #[test]
    fn test_balance_restores_equality_after_closing_a_pane() {
        let mut tab = Layout::new();
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.split(Direction::Vertical, 0.5, TabId(2));
        tab.balance();
        // Close the middle pane; without rebalancing the survivor of that split
        // would inherit an oversized share.
        assert!(tab.close(TabId(1)));
        tab.balance();

        let rects = tab.rects(VIEWPORT);
        let half = VIEWPORT.width * VIEWPORT.height / 2.0;
        for id in [TabId(0), TabId(2)] {
            assert!(
                (area_of(&rects, id) - half).abs() < 0.01,
                "pane {id:?} should occupy half after close + balance"
            );
        }
    }

    #[test]
    fn test_balance_leaves_unrelated_columns_untouched_by_a_cross_axis_split() {
        let mut tab = Layout::new();
        // Three equal vertical columns: 0 | 1 | 2.
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.split(Direction::Vertical, 0.5, TabId(2));
        tab.balance();

        // Split column 2 horizontally into 2 (top) and 3 (bottom).
        tab.split(Direction::Horizontal, 0.5, TabId(3));
        tab.balance();

        let rects = tab.rects(VIEWPORT);
        let third = VIEWPORT.width / 3.0;
        for id in [TabId(0), TabId(1)] {
            assert!(
                (rects.iter().find(|(p, _)| *p == id).unwrap().1.width - third).abs() <= 1.0,
                "pane {id:?} width must be untouched by a split on a different axis"
            );
        }
        let col2_width = rects.iter().find(|(p, _)| *p == TabId(2)).unwrap().1.width;
        assert!((col2_width - third).abs() <= 1.0);
        assert_eq!(
            col2_width,
            rects.iter().find(|(p, _)| *p == TabId(3)).unwrap().1.width
        );
    }

    #[test]
    fn test_balance_equalizes_a_same_axis_split_into_an_existing_column() {
        let mut tab = Layout::new();
        // Three equal vertical columns: 0 | 1 | 2.
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.split(Direction::Vertical, 0.5, TabId(2));
        tab.balance();

        // Split column 2 on the same (vertical) axis into 2 | 3.
        tab.split(Direction::Vertical, 0.5, TabId(3));
        tab.balance();

        let rects = tab.rects(VIEWPORT);
        let quarter = VIEWPORT.width / 4.0;
        for id in [TabId(0), TabId(1), TabId(2), TabId(3)] {
            assert!(
                (rects.iter().find(|(p, _)| *p == id).unwrap().1.width - quarter).abs() < 0.01,
                "pane {id:?} should occupy a quarter width: a same-axis split still equalizes the whole row"
            );
        }
    }

    #[test]
    fn test_a_tab_added_to_a_pane_shows_in_its_place_without_a_new_split() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(
            layout.shown_tabs(),
            vec![TabId(1)],
            "one pane, showing the new tab"
        );
        assert_eq!(layout.members(), vec![TabId(0), TabId(1)]);
        assert_eq!(layout.rects(VIEWPORT), vec![(TabId(1), VIEWPORT)]);
    }

    #[test]
    fn test_a_new_tab_goes_right_after_the_one_shown() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.focus(TabId(0));
        layout.add_tab(TabId(2));
        assert_eq!(
            layout.tabs_in_pane(TabId(0)),
            vec![TabId(0), TabId(2), TabId(1)]
        );
    }

    #[test]
    fn test_closing_the_shown_tab_goes_back_to_the_one_shown_before_it() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.add_tab(TabId(2));
        layout.focus(TabId(0));
        layout.focus(TabId(2));
        assert!(layout.close(TabId(2)));
        assert_eq!(
            layout.focused(),
            TabId(0),
            "not the strip neighbor, TabId(1)"
        );
    }

    #[test]
    fn test_closing_a_hidden_tab_leaves_the_shown_one_alone() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        assert!(layout.close(TabId(0)));
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(layout.members(), vec![TabId(1)]);
    }

    #[test]
    fn test_closing_the_last_tab_of_a_pane_collapses_its_split() {
        let mut layout = Layout::new();
        layout.split(Direction::Vertical, 0.5, TabId(1));
        layout.add_tab(TabId(2));
        assert!(layout.close(TabId(2)));
        assert_eq!(
            layout.shown_tabs(),
            vec![TabId(0), TabId(1)],
            "the pane stays"
        );
        assert!(layout.close(TabId(1)));
        assert_eq!(layout.rects(VIEWPORT), vec![(TabId(0), VIEWPORT)]);
        assert_eq!(layout.focused(), TabId(0));
    }

    #[test]
    fn test_focusing_a_hidden_tab_shows_it_in_its_own_pane() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.split(Direction::Vertical, 0.5, TabId(2));
        assert!(layout.focus(TabId(0)));
        assert_eq!(layout.shown_tabs(), vec![TabId(0), TabId(2)]);
        assert_eq!(layout.focused(), TabId(0));
    }

    #[test]
    fn test_cycling_tabs_stays_inside_the_focused_pane() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.split(Direction::Vertical, 0.5, TabId(2));
        layout.focus(TabId(1));
        assert!(!layout.cycle_tab(true), "does not wrap past the end");
        assert_eq!(layout.focused(), TabId(1));
        assert!(layout.cycle_tab(false));
        assert_eq!(layout.focused(), TabId(0));
        assert!(!layout.cycle_tab(false), "does not wrap before the start");
        layout.focus(TabId(2));
        assert!(!layout.cycle_tab(true), "a lone tab has nowhere to go");
        assert!(!layout.cycle_tab(false), "a lone tab has nowhere to go");
    }

    #[test]
    fn test_moving_a_tab_reorders_only_its_own_strip() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.add_tab(TabId(2));
        assert!(layout.move_tab_to(TabId(2), TabId(0)));
        assert_eq!(layout.members(), vec![TabId(2), TabId(0), TabId(1)]);
        assert!(!layout.move_tab(false), "already first");
        assert!(layout.move_tab(true));
        assert_eq!(layout.members(), vec![TabId(0), TabId(2), TabId(1)]);
    }

    #[test]
    fn test_moving_a_tab_across_panes_when_source_has_multiple_tabs() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.split(Direction::Horizontal, 0.5, TabId(2));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.tabs_in_pane(TabId(0)), vec![TabId(0), TabId(1)]);
        assert_eq!(layout.tabs_in_pane(TabId(2)), vec![TabId(2)]);

        // Move TabId(1) from first pane to TabId(2)'s pane
        assert!(layout.move_tab_to(TabId(1), TabId(2)));
        assert_eq!(layout.shown_tabs().len(), 2, "both panes still exist");
        assert_eq!(layout.tabs_in_pane(TabId(0)), vec![TabId(0)]);
        assert_eq!(layout.tabs_in_pane(TabId(2)), vec![TabId(1), TabId(2)]);
        assert_eq!(layout.focused(), TabId(1));
    }

    #[test]
    fn test_moving_a_tab_across_panes_when_source_is_lone_tab_collapses_split() {
        let mut layout = Layout::new();
        layout.split(Direction::Horizontal, 0.5, TabId(1));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.tabs_in_pane(TabId(0)), vec![TabId(0)]);
        assert_eq!(layout.tabs_in_pane(TabId(1)), vec![TabId(1)]);

        // Move TabId(0) (the sole tab in pane 0) into TabId(1)'s pane
        assert!(layout.move_tab_to(TabId(0), TabId(1)));
        assert_eq!(
            layout.shown_tabs().len(),
            1,
            "empty pane collapsed the split"
        );
        assert_eq!(layout.tabs_in_pane(TabId(1)), vec![TabId(0), TabId(1)]);
        assert_eq!(layout.focused(), TabId(0));
    }

    #[test]
    fn test_moving_a_tab_to_end_of_pane_same_and_different_panes() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.add_tab(TabId(2));
        // Members: [TabId(0), TabId(1), TabId(2)]
        // Move TabId(0) to the end of its own pane
        assert!(layout.move_tab_to_pane(TabId(0), TabId(1)));
        assert_eq!(layout.members(), vec![TabId(1), TabId(2), TabId(0)]);

        // Split into another pane
        layout.split(Direction::Vertical, 0.5, TabId(3));
        assert_eq!(layout.shown_tabs().len(), 2);
        // Move TabId(1) from first pane to the end of the second pane (holding TabId(3))
        assert!(layout.move_tab_to_pane(TabId(1), TabId(3)));
        assert_eq!(layout.tabs_in_pane(TabId(0)), vec![TabId(2), TabId(0)]);
        assert_eq!(layout.tabs_in_pane(TabId(3)), vec![TabId(3), TabId(1)]);
        assert_eq!(layout.focused(), TabId(1));
    }

    #[test]
    fn test_a_snapshot_round_trips_panes_and_their_shown_tabs() {
        let mut layout = Layout::new();
        layout.add_tab(TabId(1));
        layout.split(Direction::Horizontal, 0.5, TabId(2));
        layout.focus(TabId(0));
        let restored = Layout::from_tree(layout.export_tree(), TabId(2)).unwrap();
        assert_eq!(restored.members(), vec![TabId(0), TabId(1), TabId(2)]);
        assert_eq!(restored.shown_tabs(), vec![TabId(0), TabId(2)]);
        assert_eq!(restored.focused(), TabId(2));
    }

    #[test]
    fn test_a_snapshot_with_an_emptied_pane_collapses_its_split() {
        let mut layout = Layout::new();
        layout.split(Direction::Vertical, 0.5, TabId(1));
        let tree = layout.export_tree().retain(&|p| p != TabId(1));
        let restored = Layout::from_tree(tree, TabId(1)).unwrap();
        assert_eq!(restored.rects(VIEWPORT), vec![(TabId(0), VIEWPORT)]);
        assert_eq!(
            restored.focused(),
            TabId(0),
            "a focus naming nothing falls back"
        );
        let nothing = LayoutTree::single(TabId(0)).retain(&|_| false);
        assert!(Layout::from_tree(nothing, TabId(0)).is_none());
    }

    #[test]
    fn test_pane_in_direction_locates_adjacent_panes() {
        let mut layout = Layout::new();
        // Initially single pane (TabId(0))
        assert_eq!(layout.pane_in_direction(FocusDir::Right, VIEWPORT), None);
        assert_eq!(layout.pane_in_direction(FocusDir::Down, VIEWPORT), None);

        // Split vertically (TabId(0) left, TabId(1) right)
        layout.split(Direction::Vertical, 0.5, TabId(1));
        layout.focus(TabId(0));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Right, VIEWPORT),
            Some(TabId(1))
        );
        assert_eq!(layout.pane_in_direction(FocusDir::Left, VIEWPORT), None);

        // From TabId(1), left is TabId(0)
        layout.focus(TabId(1));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Left, VIEWPORT),
            Some(TabId(0))
        );
        assert_eq!(layout.pane_in_direction(FocusDir::Right, VIEWPORT), None);
    }

    #[test]
    fn test_move_tab_to_new_pane_in_all_directions() {
        // Sole tab cannot be split off into a new pane
        let mut layout = Layout::new();
        assert!(!layout.move_tab_to_new_pane(TabId(0), FocusDir::Right));

        // Add TabId(1) to the same pane
        layout.add_tab(TabId(1));
        assert_eq!(layout.tabs_in_pane(TabId(0)), vec![TabId(0), TabId(1)]);

        // Move TabId(1) right: creates a new pane on the right
        assert!(layout.move_tab_to_new_pane(TabId(1), FocusDir::Right));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Left, VIEWPORT),
            Some(TabId(0))
        );

        // Move TabId(1) back into TabId(0)'s pane
        layout.move_tab_to_pane(TabId(1), TabId(0));
        assert_eq!(layout.shown_tabs().len(), 1);

        // Move TabId(1) left: creates a new pane on the left
        assert!(layout.move_tab_to_new_pane(TabId(1), FocusDir::Left));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Right, VIEWPORT),
            Some(TabId(0))
        );

        // Move back
        layout.move_tab_to_pane(TabId(1), TabId(0));
        assert_eq!(layout.shown_tabs().len(), 1);

        // Move TabId(1) down: creates a new pane below
        assert!(layout.move_tab_to_new_pane(TabId(1), FocusDir::Down));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Up, VIEWPORT),
            Some(TabId(0))
        );

        // Move back
        layout.move_tab_to_pane(TabId(1), TabId(0));
        assert_eq!(layout.shown_tabs().len(), 1);

        // Move TabId(1) up: creates a new pane above
        assert!(layout.move_tab_to_new_pane(TabId(1), FocusDir::Up));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Down, VIEWPORT),
            Some(TabId(0))
        );
    }

    #[test]
    fn test_split_tab_to_new_pane_across_panes() {
        let mut layout = Layout::new();
        // Pane 0: [TabId(0)]
        // Split to make Pane 1: [TabId(1)]
        layout.split(Direction::Vertical, 0.5, TabId(1));
        assert_eq!(layout.shown_tabs(), vec![TabId(0), TabId(1)]);

        // Move TabId(1) into a new pane to the Left of TabId(0).
        // Since TabId(1) was the only tab in its pane, its original pane collapses.
        assert!(layout.split_tab_to_new_pane(TabId(1), TabId(0), FocusDir::Left));
        assert_eq!(layout.shown_tabs().len(), 2);
        assert_eq!(layout.focused(), TabId(1));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Right, VIEWPORT),
            Some(TabId(0))
        );

        // Add TabId(2) to TabId(1)'s pane
        layout.add_tab(TabId(2));
        assert_eq!(layout.tabs_in_pane(TabId(1)), vec![TabId(1), TabId(2)]);

        // Move TabId(2) into a new pane Below TabId(0).
        // TabId(1)'s pane remains with TabId(1), TabId(0)'s pane splits vertically/horizontally.
        assert!(layout.split_tab_to_new_pane(TabId(2), TabId(0), FocusDir::Down));
        assert_eq!(layout.shown_tabs().len(), 3);
        assert_eq!(layout.tabs_in_pane(TabId(1)), vec![TabId(1)]);
        assert_eq!(layout.focused(), TabId(2));
        assert_eq!(
            layout.pane_in_direction(FocusDir::Up, VIEWPORT),
            Some(TabId(0))
        );
    }

    #[test]
    fn test_pane_in_direction_and_move_tab_with_side_split_column() {
        // Layout:
        // 1 |
        // - | 3
        // 2 |
        // TabId(0) is pane 1, TabId(1) is pane 2, TabId(2) is pane 3.
        let mut layout = Layout::new();
        // Split vertically: left (TabId(0)), right (TabId(2))
        layout.split(Direction::Vertical, 0.5, TabId(2));
        // Split left pane horizontally: top (TabId(0)), bottom (TabId(1))
        layout.focus(TabId(0));
        layout.split(Direction::Horizontal, 0.5, TabId(1));

        // From pane 3 (TabId(2)), there are no panes directly above or below
        layout.focus(TabId(2));
        assert_eq!(layout.pane_in_direction(FocusDir::Up, VIEWPORT), None);
        assert_eq!(layout.pane_in_direction(FocusDir::Down, VIEWPORT), None);
        assert_eq!(layout.pane_in_direction(FocusDir::Right, VIEWPORT), None);
        // Left is either 0 or 1
        assert!(
            layout.pane_in_direction(FocusDir::Left, VIEWPORT) == Some(TabId(0))
                || layout.pane_in_direction(FocusDir::Left, VIEWPORT) == Some(TabId(1))
        );

        // Add TabId(3) to pane 3
        layout.add_tab(TabId(3));
        assert_eq!(layout.tabs_in_pane(TabId(2)), vec![TabId(2), TabId(3)]);

        // Move TabId(3) up: should create a new pane above TabId(2), not move into pane 1 or 2
        assert!(layout.move_tab_to_new_pane(TabId(3), FocusDir::Up));
        assert_eq!(layout.shown_tabs().len(), 4);
        assert_eq!(layout.focused(), TabId(3));
        // TabId(3) is now above TabId(2)
        assert_eq!(
            layout.pane_in_direction(FocusDir::Down, VIEWPORT),
            Some(TabId(2))
        );
    }
}

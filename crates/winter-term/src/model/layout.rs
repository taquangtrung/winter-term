//! The split-tree pane layout (§2.1): the window holds a binary tree of splits
//! whose leaves are panes, each a strip of tabs with one of them shown.
//! Pure geometry and tree surgery, independent of any renderer.

// ========================================================================
// Constants
// ========================================================================

/// Pixel half-width of the invisible hit zone on each side of a split divider.
/// A pointer within this distance triggers the resize cursor and starts a drag.
pub const DIVIDER_HIT_MARGIN: f32 = 4.0;

/// Minimum and maximum split ratio, preventing a pane from being squeezed to zero.
const RATIO_MIN: f32 = 0.1;
const RATIO_MAX: f32 = 0.9;

// ========================================================================
// Data Structures
// ========================================================================

/// The window's pane layout: a binary split tree of panes plus which tab
/// has focus.
///
/// Every tab is a `TabId`, a terminal or an editor alike, allocated by the owner
/// so ids stay unique for the life of the window. Only the shown tab of each
/// pane is laid out; the rest wait in their pane's strip.
#[derive(Clone, Debug)]
pub struct Layout {
    /// The shown tab of the focused pane.
    focused: TabId,
    root: Node,
    /// When true, `rects()` returns only the focused pane at the full viewport;
    /// cleared when the user calls `toggle_zoom()` again.
    zoomed: bool,
}

/// A node in the split tree: a pane leaf or a binary split.
#[derive(Clone, Debug)]
enum Node {
    Leaf(Pane),
    Split(SplitNode),
}

/// One leaf of the split tree: its tabs in strip order and the one shown.
#[derive(Clone, Debug)]
struct Pane {
    /// The shown tab.
    active: TabId,
    /// Every tab, in the order the strip draws them.
    members: Vec<TabId>,
    /// Every tab, most recently shown first, so closing the shown tab goes
    /// back to the one the reader came from rather than to a neighbor.
    recent: Vec<TabId>,
}

/// An internal split dividing its area between two child nodes.
#[derive(Clone, Debug)]
struct SplitNode {
    direction: Direction,
    first: Box<Node>,
    ratio: f32,
    second: Box<Node>,
}

/// A pane as laid out: where it sits and what its strip holds.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneArea {
    /// The shown tab.
    pub active: TabId,
    /// Every tab, in strip order.
    pub members: Vec<TabId>,
    /// The whole pane, strip included.
    pub rect: Rect,
}

/// A rectangular area, in the renderer's coordinate space (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    /// Height in physical pixels.
    pub height: f32,
    /// Width in physical pixels.
    pub width: f32,
    /// Distance from the left edge, in physical pixels.
    pub x: f32,
    /// Distance from the top edge, in physical pixels.
    pub y: f32,
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
// Pane
// ========================================================================

impl Pane {
    /// A pane holding only `tab`.
    fn new(tab: TabId) -> Self {
        Self {
            active: tab,
            members: vec![tab],
            recent: vec![tab],
        }
    }

    /// A pane rebuilt from a snapshot, or `None` for one with no tabs.
    fn from_tree(tree: PaneTree) -> Option<Self> {
        let first = *tree.members.first()?;
        let active = match tree.members.contains(&tree.active) {
            true => tree.active,
            false => first,
        };
        let mut recent = tree.members.clone();
        recent.retain(|&p| p != active);
        recent.insert(0, active);
        Some(Self {
            active,
            members: tree.members,
            recent,
        })
    }

    /// Where `tab` sits in the strip; the end when it is not there.
    fn position(&self, tab: TabId) -> usize {
        self.members
            .iter()
            .position(|&p| p == tab)
            .unwrap_or(self.members.len())
    }

    /// Show `tab`, marking it the most recently shown.
    fn show(&mut self, tab: TabId) {
        self.active = tab;
        self.recent.retain(|&p| p != tab);
        self.recent.insert(0, tab);
    }

    /// Drop `tab`, showing the most recently shown tab left when it was the
    /// one shown. Never leaves the pane empty.
    fn remove(&mut self, tab: TabId) {
        self.members.retain(|&p| p != tab);
        self.recent.retain(|&p| p != tab);
        if self.active == tab {
            if let Some(&next) = self.recent.first() {
                self.active = next;
            }
        }
    }
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

    /// A layout with a single full-area pane holding `root`, focused.
    pub fn with_root(root: TabId) -> Self {
        Self {
            focused: root,
            root: Node::Leaf(Pane::new(root)),
            zoomed: false,
        }
    }

    /// Rebuild a `Layout` from a [`LayoutTree`] snapshot, e.g. on session
    /// restore. Panes the snapshot left empty are dropped; `None` when that
    /// leaves nothing at all.
    pub fn from_tree(tree: LayoutTree, focused: TabId) -> Option<Self> {
        let root = layout_tree_to_node(tree)?;
        let mut layout = Self {
            focused,
            root,
            zoomed: false,
        };
        if !layout.focus(focused) {
            layout.focused = layout.shown_tabs()[0];
        }
        Some(layout)
    }

    /// Export the split tree as a [`LayoutTree`] for session persistence.
    pub fn export_tree(&self) -> LayoutTree {
        node_to_layout_tree(&self.root)
    }

    // --------------------------------------------------------------------
    // Queries
    // --------------------------------------------------------------------

    /// The shown tab of the focused pane.
    pub fn focused(&self) -> TabId {
        self.focused
    }

    /// The shown tab of every pane, left-to-right / top-to-bottom in tree
    /// order: the tabs a frame draws.
    pub fn shown_tabs(&self) -> Vec<TabId> {
        self.pane_list().iter().map(|pane| pane.active).collect()
    }

    /// Every tab of every pane, shown or not, in tree then strip order.
    pub fn members(&self) -> Vec<TabId> {
        self.pane_list()
            .iter()
            .flat_map(|pane| pane.members.iter().copied())
            .collect()
    }

    /// Whether any pane holds `tab`.
    pub fn contains(&self, tab: TabId) -> bool {
        find_pane(&self.root, tab).is_some()
    }

    /// Every tab of the pane holding `tab`, in strip order.
    pub fn tabs_in_pane(&self, tab: TabId) -> Vec<TabId> {
        find_pane(&self.root, tab)
            .map(|pane| pane.members.clone())
            .unwrap_or_default()
    }

    /// The pane holding `tab`'s tabs, most recently shown first.
    pub fn recent_in_pane(&self, tab: TabId) -> Vec<TabId> {
        find_pane(&self.root, tab)
            .map(|pane| pane.recent.clone())
            .unwrap_or_default()
    }

    /// Each pane's shown tab paired with the pane's whole area within
    /// `viewport`. When zoomed, only the focused pane is returned, and it
    /// occupies the entire viewport.
    pub fn rects(&self, viewport: Rect) -> Vec<(TabId, Rect)> {
        self.panes(viewport)
            .into_iter()
            .map(|area| (area.active, area.rect))
            .collect()
    }

    /// Every pane laid out within `viewport`, in tree order. When zoomed,
    /// only the focused pane, at the full viewport.
    pub fn panes(&self, viewport: Rect) -> Vec<PaneArea> {
        let mut out = Vec::new();
        collect_panes(&self.root, viewport, &mut out);
        if self.zoomed {
            out.retain(|area| area.active == self.focused);
            for area in &mut out {
                area.rect = viewport;
            }
        }
        out
    }

    /// Return the `Direction` of any split divider that (px, py) is within
    /// [`DIVIDER_HIT_MARGIN`] pixels of, or `None`. Used to choose the resize
    /// cursor icon. Returns `None` when zoomed (no dividers are visible).
    pub fn divider_at(&self, px: f32, py: f32, viewport: Rect) -> Option<Direction> {
        if self.zoomed || self.shown_tabs().len() <= 1 {
            return None;
        }
        divider_hit_in(&self.root, viewport, px, py)
    }

    /// Whether the focused pane is currently expanded to fill the full viewport.
    pub fn is_zoomed(&self) -> bool {
        self.zoomed
    }

    // --------------------------------------------------------------------
    // Splits
    // --------------------------------------------------------------------

    /// Find the split divider that contains `(start_x, start_y)` and shift its
    /// ratio by `(dx, dy)`. Call once per mouse-move event with the delta from
    /// the previous cursor position. Returns `true` when a divider was found and
    /// adjusted. No-op when zoomed.
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
        drag_in(&mut self.root, viewport, start_x, start_y, dx, dy)
    }

    /// Toggle the focused pane between full-viewport zoom and normal split layout.
    pub fn toggle_zoom(&mut self) {
        self.zoomed = !self.zoomed;
    }

    /// Split the focused pane in two, placing a new pane holding only the
    /// caller-allocated `new_id` beside it and focusing it.
    pub fn split(&mut self, direction: Direction, ratio: f32, new_id: TabId) {
        split_at(
            &mut self.root,
            self.focused,
            direction,
            ratio.clamp(0.0, 1.0),
            new_id,
        );
        self.focused = new_id;
    }

    /// Recompute every split's ratio so its two children evenly share that
    /// split's own axis.
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
    /// and show and focus it.
    pub fn add_tab(&mut self, new_id: TabId) {
        if let Some(pane) = find_pane_mut(&mut self.root, self.focused) {
            let at = pane.position(pane.active) + 1;
            pane.members.insert(at, new_id);
            pane.show(new_id);
        }
        self.focused = new_id;
    }

    /// Remove one tab. A pane left empty collapses its parent split into the
    /// sibling; the last tab of the last pane cannot be closed. Returns
    /// whether anything changed.
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
                self.focused = active;
            }
            return true;
        }
        if !close_in(&mut self.root, tab) {
            return false;
        }
        if self.focused == tab {
            self.focused = self.shown_tabs()[0];
        }
        true
    }

    /// Show and focus `tab`, wherever it is. Returns whether it exists.
    pub fn focus(&mut self, tab: TabId) -> bool {
        let Some(pane) = find_pane_mut(&mut self.root, tab) else {
            return false;
        };
        pane.show(tab);
        self.focused = tab;
        true
    }

    /// Show the next (`forward`) or previous tab of the focused pane by strip
    /// position, wrapping around. Returns whether the shown tab changed.
    pub fn cycle_tab(&mut self, forward: bool) -> bool {
        let members = self.tabs_in_pane(self.focused);
        let count = members.len();
        if count <= 1 {
            return false;
        }
        let index = members.iter().position(|&p| p == self.focused).unwrap_or(0);
        let next = match forward {
            true => (index + 1) % count,
            false => (index + count - 1) % count,
        };
        self.focus(members[next])
    }

    /// Move the focused tab one place along its strip, toward the end when
    /// `forward`. Returns whether it moved.
    pub fn move_tab(&mut self, forward: bool) -> bool {
        let focused = self.focused;
        let Some(pane) = find_pane_mut(&mut self.root, focused) else {
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
        find_pane(&self.root, a)
            .map(|p| p.members.contains(&b))
            .unwrap_or(false)
    }

    /// Move `tab` to where `target` sits in its strip. If `target` is in the
    /// same pane, reorders within the strip. If `target` is in another pane,
    /// moves `tab` across panes to `target`'s position. If `tab` was the sole
    /// member of its pane, that pane's split collapses.
    pub fn move_tab_to(&mut self, tab: TabId, target: TabId) -> bool {
        if tab == target {
            return false;
        }
        let from_pane = match find_pane(&self.root, tab) {
            Some(p) => p,
            None => return false,
        };
        let to_pane = match find_pane(&self.root, target) {
            Some(p) => p,
            None => return false,
        };
        if from_pane.members.contains(&target) {
            let pane = find_pane_mut(&mut self.root, tab).unwrap();
            let from = pane.position(tab);
            let to = pane.position(target);
            let moved = pane.members.remove(from);
            pane.members.insert(to, moved);
            return true;
        }
        let to = to_pane.position(target);
        self.transfer_tab_to_pane(tab, target, Some(to))
    }

    /// Move `tab` into the pane holding `target_pane_tab`, placing it at the
    /// end of that pane's strip.
    pub fn move_tab_to_pane(&mut self, tab: TabId, target_pane_tab: TabId) -> bool {
        let from_pane = match find_pane(&self.root, tab) {
            Some(p) => p,
            None => return false,
        };
        if !self.contains(target_pane_tab) {
            return false;
        }
        if from_pane.members.contains(&target_pane_tab) {
            let pane = find_pane_mut(&mut self.root, tab).unwrap();
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

    fn transfer_tab_to_pane(
        &mut self,
        tab: TabId,
        target_tab: TabId,
        insert_at: Option<usize>,
    ) -> bool {
        let from_len = match find_pane(&self.root, tab) {
            Some(p) => p.members.len(),
            None => return false,
        };
        if from_len > 1 {
            let from_pane = find_pane_mut(&mut self.root, tab).unwrap();
            from_pane.remove(tab);
            let to_pane = find_pane_mut(&mut self.root, target_tab).unwrap();
            let pos = insert_at
                .unwrap_or(to_pane.members.len())
                .min(to_pane.members.len());
            to_pane.members.insert(pos, tab);
            to_pane.show(tab);
            self.focused = tab;
            true
        } else {
            let to_pane_len = match find_pane(&self.root, target_tab) {
                Some(p) => p.members.len(),
                None => return false,
            };
            let pos = insert_at.unwrap_or(to_pane_len).min(to_pane_len);
            if !close_in(&mut self.root, tab) {
                return false;
            }
            let to_pane = find_pane_mut(&mut self.root, target_tab).unwrap();
            to_pane.members.insert(pos.min(to_pane.members.len()), tab);
            to_pane.show(tab);
            self.focused = tab;
            self.balance();
            true
        }
    }

    /// Split `target_pane_tab`'s pane to make `tab` a new pane in `direction`.
    /// Returns `true` if a new pane was created, `false` if `tab` or `target_pane_tab`
    /// was not found or if `tab` was the only tab in its pane and the target was the same pane.
    pub fn split_tab_to_new_pane(
        &mut self,
        tab: TabId,
        target_pane_tab: TabId,
        direction: FocusDir,
    ) -> bool {
        let from_len = match find_pane(&self.root, tab) {
            Some(p) => p.members.len(),
            None => return false,
        };
        let same = match find_pane(&self.root, tab) {
            Some(p) => p.members.contains(&target_pane_tab),
            None => false,
        };
        if same {
            if from_len <= 1 {
                return false;
            }
            if !split_off_tab(&mut self.root, tab, direction) {
                return false;
            }
        } else {
            if !self.contains(target_pane_tab) {
                return false;
            }
            if from_len > 1 {
                let from_pane = find_pane_mut(&mut self.root, tab).unwrap();
                from_pane.remove(tab);
            } else if !close_in(&mut self.root, tab) {
                return false;
            }
            if !split_pane_with_tab(&mut self.root, target_pane_tab, tab, direction) {
                return false;
            }
        }
        self.focused = tab;
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
    // Focus between panes
    // --------------------------------------------------------------------

    /// Focus the pane at position `index` in tree order (0-based). Returns
    /// whether the index was in range.
    pub fn focus_by_index(&mut self, index: usize) -> bool {
        match self.shown_tabs().get(index) {
            Some(&id) => {
                self.focused = id;
                true
            }
            None => false,
        }
    }

    /// Focus the next pane in tree order, wrapping around.
    pub fn focus_next(&mut self) {
        let panes = self.shown_tabs();
        if let Some(index) = panes.iter().position(|&p| p == self.focused) {
            self.focused = panes[(index + 1) % panes.len()];
        }
    }

    /// Find the nearest pane in the given direction within `viewport`, by the
    /// distance between pane centers, or `None` if there is no pane in that
    /// direction.
    pub fn pane_in_direction(&self, direction: FocusDir, viewport: Rect) -> Option<TabId> {
        let rects = self.rects(viewport);
        let current = rects.iter().find(|(id, _)| *id == self.focused)?;
        let from = current.1.center();

        rects
            .iter()
            .filter(|(id, _)| *id != self.focused)
            .filter(|(_, rect)| is_toward(direction, from, rect.center()))
            .min_by(|a, b| distance(from, a.1.center()).total_cmp(&distance(from, b.1.center())))
            .map(|(id, _)| *id)
    }

    /// Focus the nearest pane in the given direction within `viewport`, by the
    /// distance between pane centers. Returns whether focus moved.
    pub fn focus_in_direction(&mut self, direction: FocusDir, viewport: Rect) -> bool {
        if let Some(target) = self.pane_in_direction(direction, viewport) {
            self.focused = target;
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
        let from = current.center();
        let in_row =
            |rect: &Rect| rect.y < current.y + current.height && current.y < rect.y + rect.height;
        let nearest = |toward: FocusDir| {
            rects
                .iter()
                .filter(|(id, rect)| *id != self.focused && in_row(rect))
                .filter(|(_, rect)| is_toward(toward, from, rect.center()))
                .min_by(|a, b| {
                    distance(from, a.1.center()).total_cmp(&distance(from, b.1.center()))
                })
                .map(|(id, _)| *id)
        };
        match nearest(FocusDir::Right).or_else(|| nearest(FocusDir::Left)) {
            Some(id) => {
                self.focused = id;
                true
            }
            None => false,
        }
    }

    /// Every pane in tree order.
    fn pane_list(&self) -> Vec<&Pane> {
        let mut out = Vec::new();
        collect_pane_refs(&self.root, &mut out);
        out
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
// Rect
// ========================================================================

impl Rect {
    /// A rectangle in physical pixels, measured from the top-left corner.
    pub fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            height,
            width,
            x,
            y,
        }
    }

    fn center(self) -> (f32, f32) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }

    fn split(self, direction: Direction, ratio: f32) -> (Rect, Rect) {
        match direction {
            Direction::Vertical => {
                let width = self.width * ratio;
                (
                    Rect::new(self.x, self.y, width, self.height),
                    Rect::new(self.x + width, self.y, self.width - width, self.height),
                )
            }
            Direction::Horizontal => {
                let height = self.height * ratio;
                (
                    Rect::new(self.x, self.y, self.width, height),
                    Rect::new(self.x, self.y + height, self.width, self.height - height),
                )
            }
        }
    }
}

// ========================================================================
// Tree helpers
// ========================================================================

fn collect_pane_refs<'a>(node: &'a Node, out: &mut Vec<&'a Pane>) {
    match node {
        Node::Leaf(pane) => out.push(pane),
        Node::Split(split) => {
            collect_pane_refs(&split.first, out);
            collect_pane_refs(&split.second, out);
        }
    }
}

fn collect_panes(node: &Node, area: Rect, out: &mut Vec<PaneArea>) {
    match node {
        Node::Leaf(pane) => out.push(PaneArea {
            active: pane.active,
            members: pane.members.clone(),
            rect: area,
        }),
        Node::Split(split) => {
            let (first, second) = area.split(split.direction, split.ratio);
            collect_panes(&split.first, first, out);
            collect_panes(&split.second, second, out);
        }
    }
}

fn find_pane(node: &Node, tab: TabId) -> Option<&Pane> {
    match node {
        Node::Leaf(pane) => pane.members.contains(&tab).then_some(pane),
        Node::Split(split) => {
            find_pane(&split.first, tab).or_else(|| find_pane(&split.second, tab))
        }
    }
}

fn find_pane_mut(node: &mut Node, tab: TabId) -> Option<&mut Pane> {
    match node {
        Node::Leaf(pane) => pane.members.contains(&tab).then_some(pane),
        Node::Split(split) => match find_pane(&split.first, tab) {
            Some(_) => find_pane_mut(&mut split.first, tab),
            None => find_pane_mut(&mut split.second, tab),
        },
    }
}

fn split_at(
    node: &mut Node,
    target: TabId,
    direction: Direction,
    ratio: f32,
    new_id: TabId,
) -> bool {
    match node {
        Node::Leaf(pane) if pane.members.contains(&target) => {
            let existing = std::mem::replace(pane, Pane::new(target));
            *node = Node::Split(SplitNode {
                direction,
                first: Box::new(Node::Leaf(existing)),
                ratio,
                second: Box::new(Node::Leaf(Pane::new(new_id))),
            });
            true
        }
        Node::Leaf(_) => false,
        Node::Split(split) => {
            split_at(&mut split.first, target, direction, ratio, new_id)
                || split_at(&mut split.second, target, direction, ratio, new_id)
        }
    }
}

fn split_off_tab(node: &mut Node, tab: TabId, direction: FocusDir) -> bool {
    match node {
        Node::Leaf(pane) if pane.members.contains(&tab) => {
            let mut remaining = std::mem::replace(pane, Pane::new(tab));
            remaining.remove(tab);
            let (dir, first, second) = match direction {
                FocusDir::Left => (
                    Direction::Vertical,
                    Box::new(Node::Leaf(Pane::new(tab))),
                    Box::new(Node::Leaf(remaining)),
                ),
                FocusDir::Right => (
                    Direction::Vertical,
                    Box::new(Node::Leaf(remaining)),
                    Box::new(Node::Leaf(Pane::new(tab))),
                ),
                FocusDir::Up => (
                    Direction::Horizontal,
                    Box::new(Node::Leaf(Pane::new(tab))),
                    Box::new(Node::Leaf(remaining)),
                ),
                FocusDir::Down => (
                    Direction::Horizontal,
                    Box::new(Node::Leaf(remaining)),
                    Box::new(Node::Leaf(Pane::new(tab))),
                ),
            };
            *node = Node::Split(SplitNode {
                direction: dir,
                first,
                ratio: 0.5,
                second,
            });
            true
        }
        Node::Leaf(_) => false,
        Node::Split(split) => {
            split_off_tab(&mut split.first, tab, direction)
                || split_off_tab(&mut split.second, tab, direction)
        }
    }
}

fn split_pane_with_tab(
    node: &mut Node,
    target: TabId,
    new_tab: TabId,
    direction: FocusDir,
) -> bool {
    match node {
        Node::Leaf(pane) if pane.members.contains(&target) => {
            let target_leaf = Box::new(Node::Leaf(std::mem::replace(pane, Pane::new(new_tab))));
            let new_leaf = Box::new(Node::Leaf(Pane::new(new_tab)));
            let (dir, first, second) = match direction {
                FocusDir::Left => (Direction::Vertical, new_leaf, target_leaf),
                FocusDir::Right => (Direction::Vertical, target_leaf, new_leaf),
                FocusDir::Up => (Direction::Horizontal, new_leaf, target_leaf),
                FocusDir::Down => (Direction::Horizontal, target_leaf, new_leaf),
            };
            *node = Node::Split(SplitNode {
                direction: dir,
                first,
                ratio: 0.5,
                second,
            });
            true
        }
        Node::Leaf(_) => false,
        Node::Split(split) => {
            split_pane_with_tab(&mut split.first, target, new_tab, direction)
                || split_pane_with_tab(&mut split.second, target, new_tab, direction)
        }
    }
}

/// Remove the pane holding `target`, collapsing its parent split into the
/// sibling. Returns `false` for the root pane, which has no sibling.
fn close_in(node: &mut Node, target: TabId) -> bool {
    let replacement = match node {
        Node::Leaf(_) => return false,
        Node::Split(split) if leaf_holds(&split.first, target) => {
            std::mem::replace(split.second.as_mut(), Node::Leaf(Pane::new(target)))
        }
        Node::Split(split) if leaf_holds(&split.second, target) => {
            std::mem::replace(split.first.as_mut(), Node::Leaf(Pane::new(target)))
        }
        Node::Split(split) => {
            return close_in(&mut split.first, target) || close_in(&mut split.second, target);
        }
    };
    *node = replacement;
    true
}

/// Return the direction of the first split divider within `DIVIDER_HIT_MARGIN`
/// of `(px, py)` inside `area`, or `None`.
fn divider_hit_in(node: &Node, area: Rect, px: f32, py: f32) -> Option<Direction> {
    let Node::Split(split) = node else {
        return None;
    };
    let (first_area, second_area) = area.split(split.direction, split.ratio);
    let on_divider = match split.direction {
        Direction::Vertical => {
            let div_x = area.x + first_area.width;
            px >= div_x - DIVIDER_HIT_MARGIN
                && px <= div_x + DIVIDER_HIT_MARGIN
                && py >= area.y
                && py < area.y + area.height
        }
        Direction::Horizontal => {
            let div_y = area.y + first_area.height;
            py >= div_y - DIVIDER_HIT_MARGIN
                && py <= div_y + DIVIDER_HIT_MARGIN
                && px >= area.x
                && px < area.x + area.width
        }
    };
    if on_divider {
        return Some(split.direction);
    }
    divider_hit_in(&split.first, first_area, px, py)
        .or_else(|| divider_hit_in(&split.second, second_area, px, py))
}

/// Find the split containing `(start_x, start_y)` and adjust its ratio by
/// `dx/dy` relative to the node's pixel area. Returns `true` when found.
fn drag_in(node: &mut Node, area: Rect, start_x: f32, start_y: f32, dx: f32, dy: f32) -> bool {
    let Node::Split(split) = node else {
        return false;
    };
    let (first_area, second_area) = area.split(split.direction, split.ratio);
    let on_divider = match split.direction {
        Direction::Vertical => {
            let div_x = area.x + first_area.width;
            start_x >= div_x - DIVIDER_HIT_MARGIN
                && start_x <= div_x + DIVIDER_HIT_MARGIN
                && start_y >= area.y
                && start_y < area.y + area.height
        }
        Direction::Horizontal => {
            let div_y = area.y + first_area.height;
            start_y >= div_y - DIVIDER_HIT_MARGIN
                && start_y <= div_y + DIVIDER_HIT_MARGIN
                && start_x >= area.x
                && start_x < area.x + area.width
        }
    };
    if on_divider {
        let delta = match split.direction {
            Direction::Vertical => {
                if area.width > 0.0 {
                    dx / area.width
                } else {
                    0.0
                }
            }
            Direction::Horizontal => {
                if area.height > 0.0 {
                    dy / area.height
                } else {
                    0.0
                }
            }
        };
        split.ratio = (split.ratio + delta).clamp(RATIO_MIN, RATIO_MAX);
        return true;
    }
    drag_in(&mut split.first, first_area, start_x, start_y, dx, dy)
        || drag_in(&mut split.second, second_area, start_x, start_y, dx, dy)
}

fn leaf_holds(node: &Node, target: TabId) -> bool {
    matches!(node, Node::Leaf(pane) if pane.members.contains(&target))
}

/// Weight of `node` along `axis`: how many equal-sized slots it should claim
/// when splits on that axis are balanced (see [`Layout::balance`]).
///
/// A split whose own direction matches `axis` lays its children out *along*
/// `axis`, so each child is a separate slot and their weights add. A split
/// whose direction differs stacks its children *across* `axis` (they share
/// the same slot on `axis`, e.g. one on top of the other for a horizontal
/// split when `axis` is vertical), so the pair claims only the larger child's
/// weight, not the sum.
fn axis_weight(node: &Node, axis: Direction) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Split(split) if split.direction == axis => {
            axis_weight(&split.first, axis) + axis_weight(&split.second, axis)
        }
        Node::Split(split) => axis_weight(&split.first, axis).max(axis_weight(&split.second, axis)),
    }
}

/// Set every split's ratio so its first child gets a share of the split's own
/// axis proportional to [`axis_weight`] (see [`Layout::balance`]). Descend first
/// so a subtree's ratios are final before its parent's ratio is set, though
/// `axis_weight` itself only reads leaf/direction shape and is unaffected by
/// ratios.
fn balance_node(node: &mut Node) {
    if let Node::Split(split) = node {
        balance_node(&mut split.first);
        balance_node(&mut split.second);
        let first = axis_weight(&split.first, split.direction) as f32;
        let second = axis_weight(&split.second, split.direction) as f32;
        split.ratio = first / (first + second);
    }
}

fn is_toward(direction: FocusDir, from: (f32, f32), to: (f32, f32)) -> bool {
    match direction {
        FocusDir::Down => to.1 > from.1,
        FocusDir::Left => to.0 < from.0,
        FocusDir::Right => to.0 > from.0,
        FocusDir::Up => to.1 < from.1,
    }
}

fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// Rebuild a node from a snapshot, dropping empty panes and collapsing any
/// split left with one side. `None` when nothing is left.
fn layout_tree_to_node(tree: LayoutTree) -> Option<Node> {
    match tree {
        LayoutTree::Pane(pane) => Pane::from_tree(pane).map(Node::Leaf),
        LayoutTree::Split {
            direction,
            ratio,
            first,
            second,
        } => match (layout_tree_to_node(*first), layout_tree_to_node(*second)) {
            (Some(first), Some(second)) => Some(Node::Split(SplitNode {
                direction,
                // In-app mutators (e.g. `Layout::split`) always clamp to
                // [0, 1]; a deserialized `session.json` isn't guaranteed to,
                // and an out-of-range ratio produces a negative-width/height
                // rect that silently misrenders instead of erroring.
                ratio: ratio.clamp(0.0, 1.0),
                first: Box::new(first),
                second: Box::new(second),
            })),
            (Some(only), None) | (None, Some(only)) => Some(only),
            (None, None) => None,
        },
    }
}

fn node_to_layout_tree(node: &Node) -> LayoutTree {
    match node {
        Node::Leaf(pane) => LayoutTree::Pane(PaneTree {
            active: pane.active,
            members: pane.members.clone(),
        }),
        Node::Split(s) => LayoutTree::Split {
            direction: s.direction,
            ratio: s.ratio,
            first: Box::new(node_to_layout_tree(&s.first)),
            second: Box::new(node_to_layout_tree(&s.second)),
        },
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const VIEWPORT: Rect = Rect {
        height: 100.0,
        width: 200.0,
        x: 0.0,
        y: 0.0,
    };

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
                (area_of(&rects, id) - third).abs() < 0.01,
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
    fn test_balance_equalizes_four_panes() {
        let mut tab = Layout::new();
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.split(Direction::Vertical, 0.5, TabId(2));
        tab.split(Direction::Vertical, 0.5, TabId(3));
        tab.balance();

        let rects = tab.rects(VIEWPORT);
        let quarter = VIEWPORT.width * VIEWPORT.height / 4.0;
        for id in [TabId(0), TabId(1), TabId(2), TabId(3)] {
            assert!(
                (area_of(&rects, id) - quarter).abs() < 0.01,
                "pane {id:?} should occupy a quarter after balance"
            );
        }
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

    /// Count `(horizontal, vertical)` split nodes under `node`.
    fn direction_counts(node: &Node) -> (usize, usize) {
        match node {
            Node::Leaf(_) => (0, 0),
            Node::Split(s) => {
                let (mut h, mut v) = direction_counts(&s.first);
                let (hf, vf) = direction_counts(&s.second);
                h += hf;
                v += vf;
                match s.direction {
                    Direction::Horizontal => h += 1,
                    Direction::Vertical => v += 1,
                }
                (h, v)
            }
        }
    }

    #[test]
    fn test_balance_preserves_mixed_split_directions() {
        let mut tab = Layout::new();
        // Build a mixed tree: V(0, H(1, 2)).
        tab.split(Direction::Vertical, 0.5, TabId(1));
        tab.split(Direction::Horizontal, 0.5, TabId(2));
        let before = direction_counts(&tab.root);
        assert!(before.0 > 0 && before.1 > 0, "sanity: tree starts mixed");

        tab.balance();

        assert_eq!(
            direction_counts(&tab.root),
            before,
            "balance must not reshape the split tree, only its ratios"
        );
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
                (rects.iter().find(|(p, _)| *p == id).unwrap().1.width - third).abs() < 0.01,
                "pane {id:?} width must be untouched by a split on a different axis"
            );
        }
        let col2_width = rects.iter().find(|(p, _)| *p == TabId(2)).unwrap().1.width;
        assert!((col2_width - third).abs() < 0.01);
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
        assert!(layout.cycle_tab(true));
        assert_eq!(layout.focused(), TabId(0), "wraps within the pane");
        layout.focus(TabId(2));
        assert!(!layout.cycle_tab(true), "a lone tab has nowhere to go");
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
}

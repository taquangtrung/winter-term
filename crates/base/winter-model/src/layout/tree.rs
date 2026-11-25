//! The split tree of the main area: its nodes, its panes, and the surgery
//! and geometry done on them. [`Layout`](super::Layout) holds one.

use misty::Rect;

use super::{
    Direction, FocusDir, LayoutTree, PaneArea, PaneTree, Region, TabId,
};

// ========================================================================
// Constants
// ========================================================================

/// Minimum and maximum split ratio, preventing a pane from being squeezed to zero.
pub(super) const RATIO_MIN: f32 = 0.1;
pub(super) const RATIO_MAX: f32 = 0.9;

// ========================================================================
// Data Structures
// ========================================================================

/// A node in the split tree: a pane leaf or a binary split.
#[derive(Clone, Debug)]
pub(super) enum Node {
    Leaf(Pane),
    Split(SplitNode),
}

/// One leaf of the split tree: its tabs in strip order and the one shown.
#[derive(Clone, Debug)]
pub(super) struct Pane {
    /// The shown tab.
    pub(super) active: TabId,
    /// Every tab, in the order the strip draws them.
    pub(super) members: Vec<TabId>,
    /// Every tab, most recently shown first, so closing the shown tab goes
    /// back to the one the reader came from rather than to a neighbor.
    pub(super) recent: Vec<TabId>,
}

/// An internal split dividing its area between two child nodes.
#[derive(Clone, Debug)]
pub(super) struct SplitNode {
    pub(super) direction: Direction,
    pub(super) first: Box<Node>,
    pub(super) ratio: f32,
    pub(super) second: Box<Node>,
}

// ========================================================================
// Pane
// ========================================================================

impl Pane {
    /// A pane holding only `tab`.
    pub(super) fn new(tab: TabId) -> Self {
        Self {
            active: tab,
            members: vec![tab],
            recent: vec![tab],
        }
    }

    /// A pane rebuilt from a snapshot, or `None` for one with no tabs.
    pub(super) fn from_tree(tree: PaneTree) -> Option<Self> {
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
    pub(super) fn position(&self, tab: TabId) -> usize {
        self.members
            .iter()
            .position(|&p| p == tab)
            .unwrap_or(self.members.len())
    }

    /// Show `tab`, marking it the most recently shown.
    pub(super) fn show(&mut self, tab: TabId) {
        self.active = tab;
        self.recent.retain(|&p| p != tab);
        self.recent.insert(0, tab);
    }

    /// Drop `tab`, showing the most recently shown tab left when it was the
    /// one shown. Never leaves the pane empty.
    pub(super) fn remove(&mut self, tab: TabId) {
        self.members.retain(|&p| p != tab);
        self.recent.retain(|&p| p != tab);
        if self.active == tab
            && let Some(&next) = self.recent.first()
        {
            self.active = next;
        }
    }
}

// ========================================================================
// Geometry
// ========================================================================

/// `area` cut in two at `ratio` along `direction`, on a whole pixel: the
/// line between the two halves is drawn on the pixels before the cut, and
/// a cut between pixels spreads that line over two at half strength.
pub(super) fn split_area(area: Rect, direction: Direction, ratio: f32) -> (Rect, Rect) {
    match direction {
        Direction::Vertical => {
            let width = (area.x + area.width * ratio).round() - area.x;
            (
                Rect::new(area.x, area.y, width, area.height),
                Rect::new(area.x + width, area.y, area.width - width, area.height),
            )
        }
        Direction::Horizontal => {
            let height = (area.y + area.height * ratio).round() - area.y;
            (
                Rect::new(area.x, area.y, area.width, height),
                Rect::new(area.x, area.y + height, area.width, area.height - height),
            )
        }
    }
}

// ========================================================================
// Tree helpers
// ========================================================================

pub(super) fn collect_pane_refs<'a>(node: &'a Node, out: &mut Vec<&'a Pane>) {
    match node {
        Node::Leaf(pane) => out.push(pane),
        Node::Split(split) => {
            collect_pane_refs(&split.first, out);
            collect_pane_refs(&split.second, out);
        }
    }
}

pub(super) fn collect_panes(node: &Node, area: Rect, out: &mut Vec<PaneArea>) {
    match node {
        Node::Leaf(pane) => out.push(PaneArea {
            active: pane.active,
            members: pane.members.clone(),
            rect: area,
            region: Region::Main,
        }),
        Node::Split(split) => {
            let (first, second) = split_area(area, split.direction, split.ratio);
            collect_panes(&split.first, first, out);
            collect_panes(&split.second, second, out);
        }
    }
}

pub(super) fn find_pane(node: &Node, tab: TabId) -> Option<&Pane> {
    match node {
        Node::Leaf(pane) => pane.members.contains(&tab).then_some(pane),
        Node::Split(split) => {
            find_pane(&split.first, tab).or_else(|| find_pane(&split.second, tab))
        }
    }
}

pub(super) fn find_pane_mut(node: &mut Node, tab: TabId) -> Option<&mut Pane> {
    match node {
        Node::Leaf(pane) => pane.members.contains(&tab).then_some(pane),
        Node::Split(split) => match find_pane(&split.first, tab) {
            Some(_) => find_pane_mut(&mut split.first, tab),
            None => find_pane_mut(&mut split.second, tab),
        },
    }
}

pub(super) fn split_at(
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

pub(super) fn split_off_tab(node: &mut Node, tab: TabId, direction: FocusDir) -> bool {
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

pub(super) fn split_pane_with_tab(
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
pub(super) fn close_in(node: &mut Node, target: TabId) -> bool {
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

/// Return the direction of the first split divider within `reach` physical
/// pixels of `(px, py)` inside `area`, or `None`.
pub(super) fn divider_hit_in(
    node: &Node,
    area: Rect,
    px: f32,
    py: f32,
    reach: f32,
) -> Option<Direction> {
    let Node::Split(split) = node else {
        return None;
    };
    let (first_area, second_area) = split_area(area, split.direction, split.ratio);
    let on_divider = match split.direction {
        Direction::Vertical => {
            let div_x = area.x + first_area.width;
            px >= div_x - reach
                && px <= div_x + reach
                && py >= area.y
                && py < area.y + area.height
        }
        Direction::Horizontal => {
            let div_y = area.y + first_area.height;
            py >= div_y - reach
                && py <= div_y + reach
                && px >= area.x
                && px < area.x + area.width
        }
    };
    if on_divider {
        return Some(split.direction);
    }
    divider_hit_in(&split.first, first_area, px, py, reach)
        .or_else(|| divider_hit_in(&split.second, second_area, px, py, reach))
}

/// Find the split containing `(start_x, start_y)` within `reach` physical
/// pixels and adjust its ratio by `dx/dy` relative to the node's pixel area.
/// Returns `true` when found.
pub(super) fn drag_in(
    node: &mut Node,
    area: Rect,
    start_x: f32,
    start_y: f32,
    dx: f32,
    dy: f32,
    reach: f32,
) -> bool {
    let Node::Split(split) = node else {
        return false;
    };
    let (first_area, second_area) = split_area(area, split.direction, split.ratio);
    let on_divider = match split.direction {
        Direction::Vertical => {
            let div_x = area.x + first_area.width;
            start_x >= div_x - reach
                && start_x <= div_x + reach
                && start_y >= area.y
                && start_y < area.y + area.height
        }
        Direction::Horizontal => {
            let div_y = area.y + first_area.height;
            start_y >= div_y - reach
                && start_y <= div_y + reach
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
    drag_in(&mut split.first, first_area, start_x, start_y, dx, dy, reach)
        || drag_in(&mut split.second, second_area, start_x, start_y, dx, dy, reach)
}

pub(super) fn leaf_holds(node: &Node, target: TabId) -> bool {
    matches!(node, Node::Leaf(pane) if pane.members.contains(&target))
}

/// Weight of `node` along `axis`: how many equal-sized slots it should claim
/// when splits on that axis are balanced (see [`Layout::balance`](super::Layout::balance)).
///
/// A split whose own direction matches `axis` lays its children out *along*
/// `axis`, so each child is a separate slot and their weights add. A split
/// whose direction differs stacks its children *across* `axis` (they share
/// the same slot on `axis`, e.g. one on top of the other for a horizontal
/// split when `axis` is vertical), so the pair claims only the larger child's
/// weight, not the sum.
pub(super) fn axis_weight(node: &Node, axis: Direction) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Split(split) if split.direction == axis => {
            axis_weight(&split.first, axis) + axis_weight(&split.second, axis)
        }
        Node::Split(split) => axis_weight(&split.first, axis).max(axis_weight(&split.second, axis)),
    }
}

/// Set every split's ratio so its first child gets a share of the split's own
/// axis proportional to [`axis_weight`] (see
/// [`Layout::balance`](super::Layout::balance)). Descend first so a subtree's
/// ratios are final before its parent's ratio is set, though
/// `axis_weight` itself only reads leaf/direction shape and is unaffected by
/// ratios.
pub(super) fn balance_node(node: &mut Node) {
    if let Node::Split(split) = node {
        balance_node(&mut split.first);
        balance_node(&mut split.second);
        let first = axis_weight(&split.first, split.direction) as f32;
        let second = axis_weight(&split.second, split.direction) as f32;
        split.ratio = first / (first + second);
    }
}

pub(super) fn is_toward(direction: FocusDir, from: (f32, f32), to: (f32, f32)) -> bool {
    match direction {
        FocusDir::Down => to.1 > from.1,
        FocusDir::Left => to.0 < from.0,
        FocusDir::Right => to.0 > from.0,
        FocusDir::Up => to.1 < from.1,
    }
}

pub(super) fn distance(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

/// Rebuild a node from a snapshot, dropping empty panes and collapsing any
/// split left with one side. `None` when nothing is left.
pub(super) fn layout_tree_to_node(tree: LayoutTree) -> Option<Node> {
    match tree {
        LayoutTree::Pane(pane) => Pane::from_tree(pane).map(Node::Leaf),
        LayoutTree::Split {
            direction,
            first,
            ratio,
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

pub(super) fn node_to_layout_tree(node: &Node) -> LayoutTree {
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

//! The separators between panes. Where two panes meet, the stretch of the
//! edge they share is one separator, and it is a border of each: the bottom
//! border of the pane above and the top border of the pane below, the right
//! border of the pane on its left and the left border of the pane on its
//! right. The chrome around the content is a neighbor the same way: a pane
//! against the title bar has its top border there, and one resting on the
//! status bar its bottom border on the bar's top edge.
//!
//! Each separator is drawn once, over the panes' strips of tabs: between two
//! panes and against the title bar on the pixels before the shared edge (the
//! last row or column of what is above or to the left), against the status
//! bar on the bar's own top row. The focused pane's bottom border takes the
//! focus color, wherever below it is another pane or the status bar.

use super::super::PaneRect;
use super::{quad_vertices, BgVertex};

// ========================================================================
// Constants
// ========================================================================

/// Thickness of a pane divider, in pixels.
pub(crate) const DIVIDER_THICKNESS: f32 = 1.0;

/// Thickness of the status bar's top edge, in pixels.
const STATUS_EDGE_PX: f32 = 1.0;

/// How far apart two pane edges may be and still count as the one edge they
/// share: pane rects are pixel-snapped, and may round 1 px apart.
const EDGE_SLACK_PX: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// The stretch of an edge two panes share, or a pane and the chrome around
/// the content, as drawn.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Separator {
    /// What is after it: below one that runs across, right of one that runs
    /// down. It is that one's top or left border.
    pub(crate) after: Side,
    /// What is before it: above one that runs across, left of one that runs
    /// down. It is that one's bottom or right border.
    pub(crate) before: Side,
    /// Its pixels.
    pub(crate) bounds: PaneRect,
    /// Whether it runs across, rather than down.
    pub(crate) horizontal: bool,
}

/// What lies on one side of a separator.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Side {
    /// A pane, by its index.
    Pane(usize),
    /// The status bar, below the content.
    StatusBar,
    /// The title bar, above the content.
    TitleBar,
}

/// How the separators of a frame are colored, and how thick they are.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SeparatorStyle {
    /// The focused pane's bottom border.
    pub(crate) focus: (f32, f32, f32),
    /// Every other separator between two panes, or against the title bar.
    pub(crate) plain: (f32, f32, f32),
    /// The status bar's top edge, where it is no focused pane's border.
    pub(crate) status: (f32, f32, f32),
    /// The thickness of one between two panes, in pixels.
    pub(crate) width: f32,
}

// ========================================================================
// Functions
// ========================================================================

/// Every separator around the panes whose whole areas (their strips of tabs
/// included) are `areas`, `width` pixels thick, the status bar's top edge at
/// `status_top` where the bar is shown.
pub(crate) fn separators(areas: &[PaneRect], width: f32, status_top: Option<f32>) -> Vec<Separator> {
    let mut found = Vec::new();
    for (i, a) in areas.iter().enumerate() {
        for (j, b) in areas.iter().enumerate().skip(i + 1) {
            found.extend(shared_edge(a, b, width).map(|(bounds, horizontal, a_first)| {
                let (before, after) = match a_first {
                    true => (Side::Pane(i), Side::Pane(j)),
                    false => (Side::Pane(j), Side::Pane(i)),
                };
                Separator { after, before, bounds, horizontal }
            }));
        }
    }
    // The chrome above the content (the title bar) is a neighbor too.
    let top = areas.iter().map(|area| area.y).fold(f32::INFINITY, f32::min);
    for (i, area) in areas.iter().enumerate() {
        if top.is_finite() && top > EDGE_SLACK_PX && (area.y - top).abs() <= EDGE_SLACK_PX {
            found.push(Separator {
                after: Side::Pane(i),
                before: Side::TitleBar,
                bounds: across(area.y, area.x, area.x + area.width, width),
                horizontal: true,
            });
        }
        // And the status bar below it, along the bar's own top row.
        if let Some(edge) = status_top.filter(|edge| (area.y + area.height - edge).abs() <= EDGE_SLACK_PX) {
            found.push(Separator {
                after: Side::StatusBar,
                before: Side::Pane(i),
                bounds: PaneRect { height: STATUS_EDGE_PX, width: area.width, x: area.x, y: edge },
                horizontal: true,
            });
        }
    }
    found
}

/// The vertices of `separators`, colored as `style` says for a frame whose
/// focused pane is `focused`: its bottom border in the focus color, drawn
/// after the rest so it is whole where a separator meets it.
pub(crate) fn separator_vertices(
    separators: &[Separator],
    focused: Option<usize>,
    style: SeparatorStyle,
    surface: (f32, f32),
) -> Vec<BgVertex> {
    let is_focus = |separator: &Separator| {
        separator.horizontal && focused.is_some_and(|pane| separator.before == Side::Pane(pane))
    };
    let color = |separator: &Separator| match (is_focus(separator), separator.after) {
        (true, _) => style.focus,
        (false, Side::StatusBar) => style.status,
        (false, Side::Pane(_) | Side::TitleBar) => style.plain,
    };
    let quad = |separator: &Separator| {
        let bounds = separator.bounds;
        quad_vertices(
            bounds.x,
            bounds.y,
            bounds.x + bounds.width,
            bounds.y + bounds.height,
            color(separator),
            surface.0,
            surface.1,
        )
    };
    let rest = separators.iter().filter(|separator| !is_focus(separator));
    let focus = separators.iter().filter(|separator| is_focus(separator));
    rest.chain(focus).flat_map(quad).collect()
}

/// The edge panes `a` and `b` share, as a separator `width` thick, whether
/// it runs across, and whether `a` is before it (above or left); `None`
/// where they do not meet. The edges may be [`EDGE_SLACK_PX`] apart and
/// still meet.
fn shared_edge(a: &PaneRect, b: &PaneRect, width: f32) -> Option<(PaneRect, bool, bool)> {
    let meets = |first: f32, second: f32| (first - second).abs() <= EDGE_SLACK_PX;
    let (a_right, b_right) = (a.x + a.width, b.x + b.width);
    let (a_bottom, b_bottom) = (a.y + a.height, b.y + b.height);
    if meets(a_right, b.x) || meets(b_right, a.x) {
        let (top, bottom) = (a.y.max(b.y), a_bottom.min(b_bottom));
        let a_first = a.x < b.x;
        let edge = if a_first { a_right } else { b_right };
        return (top < bottom).then(|| (down(edge, top, bottom, width), false, a_first));
    }
    if meets(a_bottom, b.y) || meets(b_bottom, a.y) {
        let (left, right) = (a.x.max(b.x), a_right.min(b_right));
        let a_first = a.y < b.y;
        let edge = if a_first { a_bottom } else { b_bottom };
        return (left < right).then(|| (across(edge, left, right, width), true, a_first));
    }
    None
}

/// A separator `width` thick across from `left` to `right`, on the rows
/// before row `edge`: as many before it as after, the odd one before.
fn across(edge: f32, left: f32, right: f32, width: f32) -> PaneRect {
    PaneRect {
        height: width,
        width: right - left,
        x: left,
        y: edge - (width / 2.0).ceil(),
    }
}

/// A separator `width` thick down from `top` to `bottom`, on the columns
/// before column `edge`.
fn down(edge: f32, top: f32, bottom: f32, width: f32) -> PaneRect {
    PaneRect {
        height: bottom - top,
        width,
        x: edge - (width / 2.0).ceil(),
        y: top,
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn pane(x: f32, y: f32, width: f32, height: f32) -> PaneRect {
        PaneRect { height, width, x, y }
    }

    /// 1 | 2 over 3, under a title bar 30 px tall, over a status bar at 330.
    fn layout() -> [PaneRect; 3] {
        [pane(0.0, 30.0, 200.0, 300.0), pane(200.0, 30.0, 200.0, 100.0), pane(200.0, 130.0, 200.0, 200.0)]
    }

    #[test]
    fn test_a_separator_is_a_border_of_what_is_on_either_side_of_it() {
        let found = separators(&layout(), 1.0, Some(330.0));
        let between = |before, after| found.iter().find(|s| s.before == before && s.after == after).map(|s| s.bounds);
        let (one, two, three) = (Side::Pane(0), Side::Pane(1), Side::Pane(2));
        assert_eq!(between(one, two), Some(pane(199.0, 30.0, 1.0, 100.0)), "the right border of 1, the left of 2");
        assert_eq!(between(two, three), Some(pane(200.0, 129.0, 200.0, 1.0)), "the bottom border of 2, the top of 3");
        assert_eq!(between(Side::TitleBar, one), Some(pane(0.0, 29.0, 200.0, 1.0)), "1 against the title bar");
        assert_eq!(between(one, Side::StatusBar), Some(pane(0.0, 330.0, 200.0, 1.0)), "on the status bar's own row");
        assert_eq!(between(three, Side::StatusBar), Some(pane(200.0, 330.0, 200.0, 1.0)));
        assert!(separators(&layout(), 1.0, None).iter().all(|s| s.after != Side::StatusBar), "no bar, no edge");
    }

    #[test]
    fn test_edges_one_pixel_apart_still_meet_and_far_ones_do_not() {
        // Pixel snapping may round two neighbors' shared edge 1 px apart.
        let found = separators(&[pane(0.0, 0.0, 800.0, 411.0), pane(0.0, 410.0, 800.0, 356.0)], 1.0, None);
        assert_eq!(found.len(), 1, "drawn despite the 1 px gap");
        assert!(separators(&[pane(0.0, 0.0, 100.0, 100.0), pane(0.0, 120.0, 100.0, 100.0)], 1.0, None).is_empty());
    }

    #[test]
    fn test_only_the_focused_pane_s_bottom_border_takes_the_focus_color() {
        let style = SeparatorStyle { focus: (1.0, 0.0, 0.0), plain: (0.5, 0.5, 0.5), status: (0.2, 0.2, 0.2), width: 1.0 };
        let found = separators(&layout(), 1.0, Some(330.0));
        let colored = |focused| {
            let verts = separator_vertices(&found, focused, style, (400.0, 400.0));
            let colors: Vec<(f32, f32, f32)> = verts.chunks(6).map(|quad| (quad[0].r, quad[0].g, quad[0].b)).collect();
            (colors.iter().filter(|c| **c == style.focus).count(), colors.last().copied())
        };
        // Pane 2: its bottom border, over 3, is colored, drawn last; its
        // left border and its top against the title bar are not.
        assert_eq!(colored(Some(1)), (1, Some(style.focus)));
        // Pane 3 rests on the status bar: the bar's edge is its bottom border.
        assert_eq!(colored(Some(2)), (1, Some(style.focus)));
        let plain = separator_vertices(&found, None, style, (400.0, 400.0));
        let status = plain.chunks(6).filter(|quad| (quad[0].r, quad[0].g, quad[0].b) == style.status).count();
        assert_eq!(status, 2, "the status bar's edge keeps its own color");
    }
}

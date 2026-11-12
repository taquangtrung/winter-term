//! Pane tab strips: the model each pane's strip of tabs is drawn from, its
//! pixel geometry, and its hit testing, in one place so the renderer and the
//! app compute the same rectangles. Also the title bar's styles, which the
//! app's settings name; the title bar itself is misty's.

use std::sync::Arc;

// ========================================================================
// Constants
// ========================================================================

/// Minimum tab width, in cells. Tabs never shrink below this; when even
/// minimum-width tabs overflow the strip, it paginates with scroll arrows.
const MIN_TAB_CELLS: f32 = 18.0;
/// Maximum tab width, in cells. With only a few tabs they grow to share the
/// available width up to this cap, rather than staying pinned at the minimum.
const MAX_TAB_CELLS: f32 = 30.0;
/// Width of each tab-strip scroll arrow (`‹` / `›`) target, in cells.
const TAB_SCROLL_CELLS: f32 = 2.5;
/// Width of the close (`×`) target inside a tab, in cells.
const CLOSE_CELLS: f32 = 3.0;
/// Horizontal padding inset from each edge of a tab pill, in cells. Applied to
/// both edges alike: the icon (or the title, with none) clears the left, and
/// the close button clears the right.
pub(crate) const TAB_H_PAD_CELLS: f32 = 0.8;
/// Flat horizontal gap between adjacent tab pills, in pixels, split evenly
/// (half on each side) so neighbors each contribute half the gap. Purely a
/// rendering inset on the pill's own background: the tab's hit-test
/// `Region`, title, and close-button positions are unaffected, so click
/// targets stay exactly as wide as before.
pub(crate) const TAB_GAP_PX: f32 = 0.5;
/// Width of a pane strip's new-tab (`+`) button, in cells.
const NEW_TAB_CELLS: f32 = 4.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Which menubar presentation to draw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuStyle {
    /// A `File / Edit / View / …` menubar in the title bar; each title opens its menu.
    Classic,
    /// A single `☰` button in the title bar that opens one dropdown of commands.
    #[default]
    Modern,
}

/// Which edge of the title bar carries the minimize/maximize/close buttons.
/// The hamburger button (modern style only) sits on the opposite edge.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum ControlsSide {
    /// Window controls hug the left edge; hamburger (if any) on the right.
    #[default]
    Left,
    /// Window controls hug the right edge; hamburger (if any) on the left.
    Right,
}

/// A pixel rectangle in surface coordinates (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Height in pixels.
    pub h: f32,
    /// Width in pixels.
    pub w: f32,
    /// X origin in pixels.
    pub x: f32,
    /// Y origin in pixels.
    pub y: f32,
}

/// One tab's data for pane-level tab strip layout.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneTab {
    /// The pane's identifier.
    pub tab_id: u64,
    /// The tab's title as drawn.
    pub title: String,
    /// Whether this tab is currently the active tab in its pane group.
    pub active: bool,
    /// Whether this tab is in the focused pane group (not dimmed).
    pub focused: bool,
    /// SVG document of the icon drawn before the title, or `None` for no icon.
    /// Shared, so comparing strips frame to frame is a pointer check rather
    /// than a walk over every icon's bytes.
    pub icon: Option<Arc<[u8]>>,
    /// Whether this tab is currently being dragged.
    pub dragging: bool,
    /// Whether the tab holds changes not yet written, drawn as a dot where
    /// the close button is until the pointer comes over the tab.
    pub modified: bool,
}

/// Drop target feedback for a pane strip during tab drag and drop.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PaneStripDropTarget {
    /// Pixel x position within the strip where the vertical insertion line is drawn.
    pub indicator_x: Option<f32>,
    /// Whether this pane's content area below the strip is the drop target.
    pub is_target_pane: bool,
}

/// What a point on a pane tab strip lands on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PaneStripHit {
    /// Nothing hit: blank tab strip space or outside.
    None,
    /// A tab's body, by pane ID.
    Tab(u64),
    /// A tab's close (`×`) button, by pane ID.
    CloseTab(u64),
    /// The new-tab (`+`) button.
    NewTab,
    /// Left scroll arrow (`‹`).
    ScrollLeft,
    /// Right scroll arrow (`›`).
    ScrollRight,
}

/// Pixel layout of a pane-level tab strip.
#[derive(Clone, Debug, PartialEq)]
pub struct PaneStripLayout {
    /// Per-tab hit/draw regions, parallel to the input `tabs`.
    pub tabs: Vec<Region>,
    /// Per-tab close (`×`) button regions, parallel to the input `tabs`.
    pub closes: Vec<Region>,
    /// The new-tab (`+`) button region.
    pub new_tab: Option<Region>,
    /// Left scroll arrow (`‹`), present when paginated.
    pub scroll_left: Option<Region>,
    /// Right scroll arrow (`›`), present when paginated.
    pub scroll_right: Option<Region>,
}

// ========================================================================
// Region
// ========================================================================

impl Region {
    /// Whether the point `(px, py)` is inside this region.
    pub fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

// ========================================================================
// Public API
// ========================================================================

/// Compute the pixel geometry of a pane tab strip.
pub fn layout_pane_strip(
    tabs: &[PaneTab],
    width: f32,
    height: f32,
    cw: f32,
    ch: f32,
    active_idx: usize,
) -> PaneStripLayout {
    let _ = ch;
    let n = tabs.len();
    let min_tab_w = MIN_TAB_CELLS * cw;
    let max_tab_w = MAX_TAB_CELLS * cw;
    let close_w = CLOSE_CELLS * cw;
    let right_pad = TAB_H_PAD_CELLS * cw;
    let new_tab_w = NEW_TAB_CELLS * cw;
    let scroll_w = TAB_SCROLL_CELLS * cw;

    let avail_no_nav = (width - new_tab_w).max(0.0);
    let paginated = n > 1 && (n as f32) * min_tab_w > avail_no_nav;

    let off = Region {
        h: height,
        w: 0.0,
        x: 0.0,
        y: 0.0,
    };
    let mut out_tabs = vec![off; n];
    let mut out_closes = vec![off; n];
    let mut scroll_left = None;
    let mut scroll_right = None;

    let (tabs_origin, tab_w, visible_start, visible_count) = if !paginated {
        let tab_w = if n == 0 {
            min_tab_w
        } else {
            (avail_no_nav / n as f32).clamp(min_tab_w, max_tab_w)
        };
        (0.0, tab_w, 0, n)
    } else {
        let avail = (width - 2.0 * scroll_w - new_tab_w).max(min_tab_w);
        let count = ((avail / min_tab_w).floor() as usize).clamp(1, n);
        let tab_w = (avail / count as f32).clamp(min_tab_w, max_tab_w);
        let start = (active_idx / count * count).min(n - count);
        scroll_left = Some(Region {
            h: height,
            w: scroll_w,
            x: 0.0,
            y: 0.0,
        });
        let origin = scroll_w;
        scroll_right = Some(Region {
            h: height,
            w: scroll_w,
            x: origin + count as f32 * tab_w,
            y: 0.0,
        });
        (origin, tab_w, start, count)
    };

    for slot in 0..visible_count {
        let i = visible_start + slot;
        if i >= n {
            break;
        }
        let x = tabs_origin + slot as f32 * tab_w;
        out_tabs[i] = Region {
            h: height,
            w: tab_w,
            x,
            y: 0.0,
        };
        out_closes[i] = Region {
            h: height,
            w: close_w,
            x: x + tab_w - close_w - right_pad,
            y: 0.0,
        };
    }

    let new_tab_x = if paginated {
        scroll_right.as_ref().map_or(0.0, |r| r.x + r.w)
    } else {
        n as f32 * tab_w
    };
    let new_tab = if new_tab_x + new_tab_w <= width || n == 0 {
        Some(Region {
            h: height,
            w: new_tab_w,
            x: new_tab_x,
            y: 0.0,
        })
    } else {
        None
    };

    PaneStripLayout {
        tabs: out_tabs,
        closes: out_closes,
        new_tab,
        scroll_left,
        scroll_right,
    }
}

/// Map a relative click at `(x, y)` to the pane tab strip element under it.
pub fn hit_test_pane_strip(
    layout: &PaneStripLayout,
    x: f32,
    y: f32,
    tabs: &[PaneTab],
) -> PaneStripHit {
    if layout.scroll_left.is_some_and(|r| r.contains(x, y)) {
        return PaneStripHit::ScrollLeft;
    }
    if layout.scroll_right.is_some_and(|r| r.contains(x, y)) {
        return PaneStripHit::ScrollRight;
    }
    for (i, region) in layout.closes.iter().enumerate() {
        if region.contains(x, y) {
            return PaneStripHit::CloseTab(tabs[i].tab_id);
        }
    }
    for (i, region) in layout.tabs.iter().enumerate() {
        if region.contains(x, y) {
            return PaneStripHit::Tab(tabs[i].tab_id);
        }
    }
    if layout.new_tab.is_some_and(|r| r.contains(x, y)) {
        return PaneStripHit::NewTab;
    }
    PaneStripHit::None
}

//! Top window tabbar: the tabbar and menubar model plus their pixel geometry.
//!
//! Geometry lives here, in one place, so the GPU renderer (which draws the
//! tabbar) and the app (which hit-tests mouse clicks against it) compute the
//! exact same rectangles and never drift. The app supplies a [`TopTabbar`]
//! describing the tabs and menus; [`layout`] turns it into concrete pixel
//! [`Region`]s, and [`hit_test`] maps a click to a [`TabbarHit`].

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
/// Width of the zoom/restore icon target shown left of the close button when a
/// tab's pane is zoomed, in cells.
pub(crate) const ZOOM_CELLS: f32 = 2.0;
/// Horizontal padding inset from each edge of a tab pill, in cells. Applied to
/// both edges: the title clears the left, and the close button clears the right.
pub(crate) const TAB_H_PAD_CELLS: f32 = 0.8;
/// Vertical padding above a tab pill, in pixels: shared between
/// `renderer::rasterize_tabbar_strip` (the pill's own background geometry, via
/// `tab_top_inset_px`) and `renderer::draw_tabbar` (text centering), so a
/// label and its pill never drift apart. A tab pill floats with a small
/// top/bottom margin and all four corners rounded (Brave-style), rather than
/// sitting flush against the strip's bottom edge.
pub(crate) const TAB_TOP_VPAD_PX: f32 = 1.0;
/// Vertical padding below a tab pill, in pixels. See `TAB_TOP_VPAD_PX`.
pub(crate) const TAB_BOTTOM_VPAD_PX: f32 = 1.0;
/// Flat horizontal gap between adjacent tab pills, in pixels, split evenly
/// (half on each side) so neighbors each contribute half the gap. Purely a
/// rendering inset on the pill's own background (`renderer::rasterize_tabbar_strip`)
///: the tab's hit-test `Region`, title, and close-button positions are
/// unaffected, so click targets stay exactly as wide as before.
pub(crate) const TAB_GAP_PX: f32 = 0.5;
/// Extra vertical inset the new-tab button's hover pill carries on its own
/// bottom edge (on top of `TAB_TOP_VPAD_PX`'s top inset), as a fraction of
/// the cell height, making it read shorter than the other titlebar buttons'
/// hover pills. The `+` glyph itself centers on the plain tab shape instead
/// (see `renderer::draw_tabbar`), not on this pill.
pub(crate) const NEW_TAB_BOTTOM_INSET_RATIO: f32 = 0.15;
/// Extra padding cleared on the first tab's own left edge, in cells: keeps
/// it off whatever's immediately to its left (window controls, or the
/// hamburger's open space), on top of `TAB_H_PAD_CELLS`'s inner title inset.
const FIRST_TAB_LEFT_PAD_CELLS: f32 = 0.0;
/// Width of the new-tab (`+`) button, in cells. Matches `CLOSE_CONTROL_CELLS`
/// so the two outermost titlebar-edge buttons read as the same size.
const NEW_TAB_CELLS: f32 = 4.0;
/// Horizontal padding used to size every titlebar button's shared hover-pill
/// width (see `renderer::rasterize_tabbar_strip`): the pill is
/// `NEW_TAB_CELLS - 2 * this` wide, centered in whichever button is hovered.
/// `layout` below reserves extra spacing around narrower buttons (minimize,
/// maximize, close) so that shared pill never bleeds into a neighbor.
pub(crate) const HOVER_PILL_H_PAD_CELLS: f32 = 0.3;
/// Width of the modern hamburger (`☰`) button, in cells. Kept just wide
/// enough to clear the shared hover-pill width (`HOVER_PILL_H_PAD_CELLS`) plus
/// `HAMBURGER_RIGHT_PAD_CELLS`, so the box neither leaves visible empty
/// tabbar band around the pill nor lets it overflow past the box's own edge.
const HAMBURGER_CELLS: f32 = 4.0;
/// Padding cleared on the hamburger's own right edge, in cells: keeps it off
/// the window edge or tab strip it faces, whichever that button-side meets.
const HAMBURGER_RIGHT_PAD_CELLS: f32 = 0.6;
/// Width of the minimize/maximize window-control buttons, in cells. Narrower
/// than `CLOSE_CONTROL_CELLS` so close reads as the more deliberate action.
const CONTROL_CELLS: f32 = 3.0;
/// Padding cleared on the "maximize" control's own right edge, in cells,
/// keeps it off whichever the button-side meets (close, or open tabbar space).
const MAXIMIZE_RIGHT_PAD_CELLS: f32 = 0.8;
/// Width of the "close" window-control button, in cells.
const CLOSE_CONTROL_CELLS: f32 = 4.0;
/// Padding cleared on the "close" control's own left edge, in cells: keeps
/// it off whichever the button-side meets (the window edge or maximize).
const CLOSE_CONTROL_LEFT_PAD_CELLS: f32 = 0.4;
/// Horizontal padding around a classic menu title, in cells (one each side).
const MENU_TITLE_PAD_CELLS: f32 = 2.0;
/// Minimum dropdown panel width, in cells.
const DROPDOWN_MIN_CELLS: f32 = 22.0;
/// Padding added to the widest `label` + `shortcut` when sizing a dropdown, in
/// cells (leading indent, gap between label and shortcut, trailing margin).
const DROPDOWN_PAD_CELLS: f32 = 4.0;
/// Height of one dropdown item row as a multiple of the cell height. Taller than
/// a terminal row so the menu reads as an app menu, not a packed text grid.
const DROPDOWN_ITEM_RATIO: f32 = 1.9;
/// Padding above the first and below the last dropdown item, in cells.
const DROPDOWN_PAD_Y_CELLS: f32 = 0.4;

// ========================================================================
// Data Structures
// ========================================================================

/// Which menubar presentation to draw.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum MenuStyle {
    /// A `File / Edit / View / …` row above the tabbar; each title opens its menu.
    Classic,
    /// A single `☰` button on the tabbar that opens one dropdown of commands.
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

/// One tab's label in the tabbar.
#[derive(Clone, Debug)]
pub struct TabLabel {
    /// The tab's resolved title, as drawn.
    pub title: String,
    /// When true, the focused pane in this tab is zoomed to fill the viewport.
    pub zoomed: bool,
}

/// One dropdown menu: a `title` (shown only in the classic menubar) and its
/// items. Item order matches the app's parallel command list.
#[derive(Clone, Debug)]
pub struct Menu {
    /// The dropdown's lines, top to bottom.
    pub items: Vec<MenuItem>,
    /// Menubar title, drawn only in the classic style.
    pub title: String,
}

/// One selectable line in a dropdown. Purely presentational; the app maps the
/// same index to a command name. An item with `children` is a submenu parent:
/// hovering it opens a child panel to the right instead of running a command.
#[derive(Clone, Debug)]
pub struct MenuItem {
    /// Submenu entries. Empty for a plain command item.
    pub children: Vec<MenuItem>,
    /// The item text.
    pub label: String,
    /// Keyboard shortcut hint drawn right-aligned; empty when unbound.
    pub shortcut: String,
}

impl MenuItem {
    /// Whether this item opens a submenu rather than dispatching a command.
    pub fn has_children(&self) -> bool {
        !self.children.is_empty()
    }
}

/// A right-click context menu anchored at a pixel position.
#[derive(Clone, Debug)]
pub struct ContextMenu {
    /// The menu's lines, top to bottom.
    pub items: Vec<MenuItem>,
    /// Index of the hovered item, or `None`.
    pub selected: Option<usize>,
    /// Surface-pixel position of the top-left corner of the panel.
    pub x: f32,
    /// Surface-pixel top edge of the panel.
    pub y: f32,
}

/// The full top-tabbar model the app hands the renderer each frame.
#[derive(Clone, Debug)]
pub struct TopTabbar {
    /// Index into `tabs` of the focused tab.
    pub active_tab: usize,
    /// Which tabbar element the cursor is currently over; drives hover highlights.
    pub tabbar_hover: TabbarHit,
    /// A right-click context menu, or `None` when closed.
    pub context_menu: Option<ContextMenu>,
    /// Which edge the window controls occupy; the hamburger button (modern style
    /// only) is placed on the opposite edge.
    pub controls_side: ControlsSide,
    /// Modern hamburger dropdown or classic menubar.
    pub menu_style: MenuStyle,
    /// The app's menus, in bar order.
    pub menus: Vec<Menu>,
    /// Index into `menus` of the open dropdown, or `None` when closed.
    pub open_menu: Option<usize>,
    /// Index (into the open menu's `items`) of the parent whose submenu is shown,
    /// or `None` when no submenu is open.
    pub open_submenu: Option<usize>,
    /// The highlighted dropdown item (mouse hover), if any.
    pub selected_item: Option<usize>,
    /// The highlighted submenu child (mouse hover), if any.
    pub selected_subitem: Option<usize>,
    /// The open tabs, left to right.
    pub tabs: Vec<TabLabel>,
    /// URL of the hyperlink under the cursor, with the cursor position (surface
    /// coordinates). When `Some`, the renderer draws a tooltip below the cursor.
    pub url_tooltip: Option<(String, f32, f32)>,
    /// Whether to draw custom minimize/maximize/close controls (the borderless
    /// "modern" title bar); `false` lets the OS draw them.
    pub window_controls: bool,
}

/// A pixel rectangle in surface coordinates (origin top-left).
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Region {
    pub h: f32,
    pub w: f32,
    pub x: f32,
    pub y: f32,
}

/// Geometry of an open dropdown panel and its item rows.
#[derive(Clone, Copy, Debug)]
pub(crate) struct DropdownLayout {
    pub item_h: f32,
    pub items: usize,
    pub origin_x: f32,
    /// Vertical padding inside the panel, above the first and below the last row.
    pub pad: f32,
    pub top: f32,
    pub width: f32,
}

/// Concrete pixel geometry of the whole tabbar for one frame.
pub(crate) struct TabbarLayout {
    /// Per-tab close (`×`) targets, parallel to `tabs`.
    pub closes: Vec<Region>,
    /// The `[minimize, maximize, close]` window-control targets at the right edge,
    /// or `None` when the OS draws the decorations.
    pub controls: Option<[Region; 3]>,
    /// The right-click context menu panel, or `None` when closed.
    pub context_menu: Option<DropdownLayout>,
    pub dropdown: Option<DropdownLayout>,
    /// The modern hamburger button, or `None` in classic style.
    pub hamburger: Option<Region>,
    /// Classic menubar band top (`y`), or `None` in modern style.
    pub menubar_top: Option<f32>,
    /// Classic menu-title targets, parallel to `menus`; empty in modern style.
    pub menu_titles: Vec<Region>,
    pub new_tab: Region,
    /// Left scroll arrow (`‹`), present only when the tab strip is paginated.
    pub scroll_left: Option<Region>,
    /// Right scroll arrow (`›`), present only when the tab strip is paginated.
    pub scroll_right: Option<Region>,
    /// The child panel of the open submenu parent, to the right of `dropdown`.
    pub submenu: Option<DropdownLayout>,
    /// Top (`y`) of the tabbar row.
    pub tab_row_top: f32,
    /// Per-tab hit/draw regions, parallel to `tabbar.tabs`. Tabs scrolled out of
    /// the visible window when paginated have a zero-width region.
    pub tabs: Vec<Region>,
}

/// What a click landed on.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabbarHit {
    /// The window close (`✕`) control.
    Close,
    /// A tab's close (`✕`) glyph, by tab index.
    CloseTab(usize),
    /// An item in the right-click context menu, by index.
    ContextMenuItem(usize),
    /// An item in the open dropdown, by index into that menu's `items`.
    DropdownItem(usize),
    /// The modern-style hamburger button that opens the menu dropdown.
    Hamburger,
    /// The window maximize/restore (`□`) control.
    Maximize,
    /// A classic menu title, by index into `menus`.
    MenuTitle(usize),
    /// The window minimize (`—`) control.
    Minimize,
    /// The `+` button that opens a new tab.
    NewTab,
    /// Nothing hit: empty tabbar space, or a point outside it.
    None,
    /// The left tab-strip scroll arrow: navigate to the previous tab.
    ScrollTabsLeft,
    /// The right tab-strip scroll arrow: navigate to the next tab.
    ScrollTabsRight,
    /// A child of the open submenu, by index into that submenu's items. The
    /// parent is the tabbar's `open_submenu`.
    SubmenuItem(usize),
    /// A tab's body, by index; selects that tab.
    Tab(usize),
}

// ========================================================================
// Region
// ========================================================================

impl Region {
    fn contains(&self, px: f32, py: f32) -> bool {
        px >= self.x && px < self.x + self.w && py >= self.y && py < self.y + self.h
    }
}

// ========================================================================
// Public API
// ========================================================================

/// Number of top cell rows the tabbar occupies: the tabbar, plus the menubar row
/// in classic style.
pub fn tabbar_rows(style: MenuStyle) -> usize {
    match style {
        // Classic stacks a menubar over the tabbar. Modern is a single bar, two
        // cells tall so the title bar has room to breathe (VS Code-ish height).
        MenuStyle::Classic => 2,
        MenuStyle::Modern => 2,
    }
}

/// Map a click at `(x, y)` to the tabbar element under it. The open submenu wins
/// first (it overlays the parent), then the parent dropdown, then per-tab close
/// buttons, tabs, the new-tab button, and finally the menu triggers.
pub fn hit_test(
    tabbar: &TopTabbar,
    surface_w: f32,
    cell_w: f32,
    cell_h: f32,
    x: f32,
    y: f32,
) -> TabbarHit {
    let layout = layout(tabbar, surface_w, cell_w, cell_h);

    // The context menu overlays everything else when open.
    if let Some(cm_layout) = &layout.context_menu {
        for i in 0..cm_layout.items {
            if dropdown_item_region(cm_layout, i).contains(x, y) {
                return TabbarHit::ContextMenuItem(i);
            }
        }
    }

    if let Some(submenu) = &layout.submenu {
        for i in 0..submenu.items {
            if dropdown_item_region(submenu, i).contains(x, y) {
                return TabbarHit::SubmenuItem(i);
            }
        }
    }
    if let Some(dropdown) = &layout.dropdown {
        for i in 0..dropdown.items {
            if dropdown_item_region(dropdown, i).contains(x, y) {
                return TabbarHit::DropdownItem(i);
            }
        }
    }

    if let Some([minimize, maximize, close]) = &layout.controls {
        if close.contains(x, y) {
            return TabbarHit::Close;
        }
        if maximize.contains(x, y) {
            return TabbarHit::Maximize;
        }
        if minimize.contains(x, y) {
            return TabbarHit::Minimize;
        }
    }

    if layout.scroll_left.is_some_and(|r| r.contains(x, y)) {
        return TabbarHit::ScrollTabsLeft;
    }
    if layout.scroll_right.is_some_and(|r| r.contains(x, y)) {
        return TabbarHit::ScrollTabsRight;
    }
    for (i, region) in layout.closes.iter().enumerate() {
        if region.contains(x, y) {
            return TabbarHit::CloseTab(i);
        }
    }
    for (i, region) in layout.tabs.iter().enumerate() {
        if region.contains(x, y) {
            return TabbarHit::Tab(i);
        }
    }
    if layout.new_tab.contains(x, y) {
        return TabbarHit::NewTab;
    }
    if let Some(hamburger) = &layout.hamburger {
        if hamburger.contains(x, y) {
            return TabbarHit::Hamburger;
        }
    }
    for (i, region) in layout.menu_titles.iter().enumerate() {
        if region.contains(x, y) {
            return TabbarHit::MenuTitle(i);
        }
    }
    TabbarHit::None
}

// ========================================================================
// Layout
// ========================================================================

/// Pixel top-inset for a tab pill's own background/highlight: `TAB_TOP_VPAD_PX`
/// plus the same flat top-up `modern_tabbar_height_px` adds to the Modern
/// tabbar's total height (0 in Classic style, which doesn't use that flat
/// top-up). A pill's rendered height is `tab.h - tab_top_inset_px(..) -
/// TAB_BOTTOM_VPAD_PX` (`renderer::rasterize_tabbar_strip` insets the bottom
/// edge by `TAB_BOTTOM_VPAD_PX`, for a floating, fully-rounded pill), so
/// growing the top-up grows only the empty space above the tabs: it cancels
/// out of the pill's own height.
pub(crate) fn tab_top_inset_px(menu_style: MenuStyle) -> f32 {
    let extra = if menu_style == MenuStyle::Modern {
        crate::TABBAR_EXTRA_HEIGHT_PX
    } else {
        0.0
    };
    TAB_TOP_VPAD_PX + extra
}

pub(crate) fn layout(tabbar: &TopTabbar, surface_w: f32, cw: f32, ch: f32) -> TabbarLayout {
    let classic = tabbar.menu_style == MenuStyle::Classic;
    let tabbar_h = if tabbar.menu_style == MenuStyle::Modern {
        crate::modern_tabbar_height_px(ch)
    } else {
        tabbar_rows(tabbar.menu_style) as f32 * ch
    };
    let menubar_top = if classic { Some(0.0) } else { None };
    let tab_row_top = if classic { ch } else { 0.0 };
    // Interactive elements are one cell tall in classic (one row each) but span
    // the whole taller bar in modern, so clicks land anywhere in the band.
    let bar_h = if classic { ch } else { tabbar_h };

    // The modern hamburger sits on the edge opposite the window controls; the
    // tabs begin after whichever element is on the left. Classic style has no
    // hamburger, but window controls (if enabled) still reserve their edge.
    let controls_left = tabbar.controls_side == ControlsSide::Left;
    let hamburger_w = HAMBURGER_CELLS * cw;
    // The shared hover-pill width (matched to the new-tab button, see
    // `HOVER_PILL_H_PAD_CELLS`) is wider than minimize/maximize's own box and
    // close's (post-left-pad) box, so centering it there would bleed into a
    // neighboring button. `control_margin`/`close_margin` are exactly the
    // overflow on each side of those boxes: the extra spacing below inserts
    // just enough room to absorb it, wherever the two-sided need doubles.
    let hover_pill_cells = NEW_TAB_CELLS - 2.0 * HOVER_PILL_H_PAD_CELLS;
    let control_margin = ((hover_pill_cells - CONTROL_CELLS) * 0.5).max(0.0) * cw;
    let close_box_cells = CLOSE_CONTROL_CELLS - CLOSE_CONTROL_LEFT_PAD_CELLS;
    let close_margin = ((hover_pill_cells - close_box_cells) * 0.5).max(0.0) * cw;
    // Close is wider than minimize/maximize, so the group's reserved width is
    // one close-width plus two regular control-widths, plus the empty gap
    // after maximize (a real gap, not an inset of any one button, so its own
    // hover pill stays full-sized instead of shrinking), plus the hover-pill
    // margins above (one on each of the group's four internal boundaries: its
    // outer edge, close-to-maximize, minimize-to-maximize, and its inner edge).
    let controls_group_w = CLOSE_CONTROL_CELLS * cw
        + 2.0 * CONTROL_CELLS * cw
        + MAXIMIZE_RIGHT_PAD_CELLS * cw
        + 3.0 * control_margin
        + close_margin;
    let left_reserve = if controls_left {
        if tabbar.window_controls {
            controls_group_w
        } else {
            0.0
        }
    } else {
        hamburger_w
    };

    let hamburger = (!classic).then_some(Region {
        h: bar_h,
        w: hamburger_w - HAMBURGER_RIGHT_PAD_CELLS * cw,
        x: if controls_left {
            surface_w - hamburger_w
        } else {
            0.0
        },
        y: tab_row_top,
    });
    let tabs_left = left_reserve;

    // Right-edge elements that bound the tab strip: window controls (when on the
    // right) and the modern hamburger (when controls sit on the left edge).
    let right_reserve = (if !controls_left && tabbar.window_controls {
        controls_group_w
    } else {
        0.0
    }) + (if controls_left && !classic {
        hamburger_w
    } else {
        0.0
    });

    let close_w = CLOSE_CELLS * cw;
    let right_pad = TAB_H_PAD_CELLS * cw;
    let new_tab_w = NEW_TAB_CELLS * cw;
    let min_tab_w = MIN_TAB_CELLS * cw;
    let max_tab_w = MAX_TAB_CELLS * cw;
    let scroll_w = TAB_SCROLL_CELLS * cw;

    let n = tabbar.tabs.len();
    let strip_left = tabs_left;
    let strip_right = (surface_w - right_reserve).max(strip_left);
    let strip_w = strip_right - strip_left;

    // Tabs grow to share the available width up to MAX, never below MIN. When
    // even MIN-width tabs overflow, paginate: clamp to MIN-ish, reserve arrows,
    // and show only the page containing the active tab.
    let avail_no_nav = (strip_w - new_tab_w).max(0.0);
    let paginated = n > 1 && (n as f32) * min_tab_w > avail_no_nav;

    let off = Region {
        h: bar_h,
        w: 0.0,
        x: strip_left,
        y: tab_row_top,
    };
    let mut tabs = vec![off; n];
    let mut closes = vec![off; n];
    let mut scroll_left = None;
    let mut scroll_right = None;

    let (tabs_origin, tab_w, visible_start, visible_count) = if !paginated {
        let tab_w = if n == 0 {
            min_tab_w
        } else {
            (avail_no_nav / n as f32).clamp(min_tab_w, max_tab_w)
        };
        (strip_left, tab_w, 0, n)
    } else {
        // Reserve both arrows and the new-tab button, then fit as many tabs as
        // the remaining width allows and divide it evenly among them.
        let avail = (strip_w - 2.0 * scroll_w - new_tab_w).max(min_tab_w);
        let count = ((avail / min_tab_w).floor() as usize).clamp(1, n);
        let tab_w = (avail / count as f32).clamp(min_tab_w, max_tab_w);
        // Page-based window keyed on the active tab so it is always visible.
        let start = (tabbar.active_tab / count * count).min(n - count);
        scroll_left = Some(Region {
            h: bar_h,
            w: scroll_w,
            x: strip_left,
            y: tab_row_top,
        });
        let origin = strip_left + scroll_w;
        scroll_right = Some(Region {
            h: bar_h,
            w: scroll_w,
            x: origin + count as f32 * tab_w,
            y: tab_row_top,
        });
        (origin, tab_w, start, count)
    };

    for slot in 0..visible_count {
        let i = visible_start + slot;
        if i >= n {
            break;
        }
        let x = tabs_origin + slot as f32 * tab_w;
        // The first tab clears its own left edge by an extra
        // `FIRST_TAB_LEFT_PAD_CELLS`, inset within its slot like every other
        // titlebar-edge button's own padding: its right edge (and every
        // other tab's position) is unaffected.
        let first_tab_pad = if i == 0 {
            FIRST_TAB_LEFT_PAD_CELLS * cw
        } else {
            0.0
        };
        tabs[i] = Region {
            h: bar_h,
            w: tab_w - first_tab_pad,
            x: x + first_tab_pad,
            y: tab_row_top,
        };
        // Close button sits inset from the right edge by `TAB_H_PAD_CELLS` so it
        // clears the tab pill on both sides. The title (left-padded by the same
        // amount) fills the remainder; a zoom icon may sit just to its left.
        closes[i] = Region {
            h: bar_h,
            w: close_w,
            x: x + tab_w - close_w - right_pad,
            y: tab_row_top,
        };
    }

    // The new-tab button follows the visible tabs (after the right arrow when
    // paginated, so it stays on screen regardless of the scroll position).
    let new_tab_x = if paginated {
        scroll_right.as_ref().map_or(strip_left, |r| r.x + r.w)
    } else {
        strip_left + n as f32 * tab_w
    };
    let new_tab = Region {
        h: bar_h,
        w: new_tab_w,
        x: new_tab_x,
        y: tab_row_top,
    };

    // Window controls hug the edge chosen by `controls_side`. Close is always
    // the outermost button (nearest the edge that side hugs) and is wider
    // than minimize/maximize, with its own left edge padded so it clears
    // whatever that side meets (the window edge or maximize). Maximize gets a
    // real empty gap after it instead: its own box (and hover pill) stays
    // full width, unlike close's inset. `control_margin`/`close_margin` widen
    // every remaining internal boundary just enough that the shared, wider
    // hover pill (see `HOVER_PILL_H_PAD_CELLS`) never bleeds past it.
    let controls = tabbar.window_controls.then(|| {
        let w = CONTROL_CELLS * cw;
        let close_w = CLOSE_CONTROL_CELLS * cw;
        let close_pad = CLOSE_CONTROL_LEFT_PAD_CELLS * cw;
        let maximize_pad = MAXIMIZE_RIGHT_PAD_CELLS * cw;
        if controls_left {
            let close = Region {
                h: bar_h,
                w: close_w - close_pad,
                x: close_pad,
                y: tab_row_top,
            };
            let minimize = Region {
                h: bar_h,
                w,
                x: close_w + close_margin + control_margin,
                y: tab_row_top,
            };
            let maximize = Region {
                h: bar_h,
                w,
                x: minimize.x + w + 2.0 * control_margin,
                y: tab_row_top,
            };
            // Maximize is the outermost-right slot here, so the extra width
            // baked into `controls_group_w` opens its own gap to the tab
            // strip (which starts at `controls_group_w`) on its own.
            [minimize, maximize, close]
        } else {
            // Close is the outermost-right slot here, so `close_margin` is
            // spent on its gap to the true window edge instead.
            let strip_right = surface_w - controls_group_w;
            let minimize = Region {
                h: bar_h,
                w,
                x: strip_right + control_margin,
                y: tab_row_top,
            };
            let maximize = Region {
                h: bar_h,
                w,
                x: minimize.x + w + 2.0 * control_margin,
                y: tab_row_top,
            };
            let close_x = maximize.x + w + maximize_pad;
            let close = Region {
                h: bar_h,
                w: close_w - close_pad,
                x: close_x + close_pad,
                y: tab_row_top,
            };
            [minimize, maximize, close]
        }
    });

    let menu_titles = if classic {
        let mut titles = Vec::with_capacity(tabbar.menus.len());
        let mut x = 0.0;
        for menu in &tabbar.menus {
            let w = (menu.title.chars().count() as f32 + MENU_TITLE_PAD_CELLS) * cw;
            titles.push(Region {
                h: ch,
                w,
                x,
                y: 0.0,
            });
            x += w;
        }
        titles
    } else {
        Vec::new()
    };

    let dropdown = tabbar.open_menu.and_then(|open| {
        let menu = tabbar.menus.get(open)?;
        let width = panel_width(&menu.items, cw).min(surface_w);
        let (origin_x, top) = if classic {
            let title = menu_titles.get(open)?;
            (title.x, ch)
        } else {
            // Anchor to the hamburger: when it is on the left (controls right,
            // the default) the dropdown opens flush with its left edge growing
            // right; when it is on the right (controls left) the dropdown right-
            // aligns with the hamburger so it stays on screen.
            let hb = hamburger.as_ref()?;
            let x = if controls_left {
                (hb.x + hb.w - width).max(0.0)
            } else {
                hb.x
            };
            (x, tabbar_h)
        };
        Some(DropdownLayout {
            item_h: ch * DROPDOWN_ITEM_RATIO,
            items: menu.items.len(),
            origin_x,
            pad: ch * DROPDOWN_PAD_Y_CELLS,
            top,
            width,
        })
    });

    // The submenu opens beside the parent panel, to the right by default, or
    // to the left when controls are on the left edge (so it does not run off the
    // right side of the window). Its first child row aligns with the hovered
    // parent item; the parent panel stays put.
    let submenu = dropdown.and_then(|parent| {
        let open = tabbar.open_menu?;
        let parent_idx = tabbar.open_submenu?;
        let item = tabbar.menus.get(open)?.items.get(parent_idx)?;
        if item.children.is_empty() {
            return None;
        }
        let child_width = panel_width(&item.children, cw).min(surface_w);
        let origin_x = if controls_left {
            (parent.origin_x - child_width).max(0.0)
        } else {
            parent.origin_x + parent.width
        };
        Some(DropdownLayout {
            item_h: parent.item_h,
            items: item.children.len(),
            origin_x,
            pad: parent.pad,
            top: parent.top + parent_idx as f32 * parent.item_h,
            width: child_width,
        })
    });

    let context_menu = tabbar.context_menu.as_ref().map(|cm| {
        let width = panel_width(&cm.items, cw).min(surface_w);
        let origin_x = cm.x.min(surface_w - width).max(0.0);
        DropdownLayout {
            item_h: ch * DROPDOWN_ITEM_RATIO,
            items: cm.items.len(),
            origin_x,
            pad: ch * DROPDOWN_PAD_Y_CELLS,
            top: cm.y,
            width,
        }
    });

    TabbarLayout {
        closes,
        context_menu,
        controls,
        dropdown,
        hamburger,
        menubar_top,
        menu_titles,
        new_tab,
        scroll_left,
        scroll_right,
        submenu,
        tab_row_top,
        tabs,
    }
}

/// The pixel rect of dropdown item `i`.
pub(crate) fn dropdown_item_region(dropdown: &DropdownLayout, i: usize) -> Region {
    Region {
        h: dropdown.item_h,
        w: dropdown.width,
        x: dropdown.origin_x,
        y: dropdown.top + dropdown.pad + i as f32 * dropdown.item_h,
    }
}

/// Panel width sized to the widest `label` + `shortcut`, floored at a minimum.
fn panel_width(items: &[MenuItem], cw: f32) -> f32 {
    let widest = items
        .iter()
        .map(|item| item.label.chars().count() + item.shortcut.chars().count())
        .max()
        .unwrap_or(0) as f32;
    (widest + DROPDOWN_PAD_CELLS).max(DROPDOWN_MIN_CELLS) * cw
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    const CW: f32 = 10.0;
    const CH: f32 = 20.0;
    const SURFACE_W: f32 = 1200.0;

    fn leaf(label: &str, shortcut: &str) -> MenuItem {
        MenuItem {
            children: Vec::new(),
            label: label.into(),
            shortcut: shortcut.into(),
        }
    }

    fn tabbar(style: MenuStyle, tabs: usize, open: Option<usize>) -> TopTabbar {
        let menus = match style {
            MenuStyle::Modern => vec![Menu {
                title: "Menu".into(),
                items: vec![
                    leaf("New Tab", "Ctrl-Shift-T"),
                    // A submenu parent: hovering it opens its children.
                    MenuItem {
                        children: vec![leaf("Vertical", ""), leaf("Horizontal", "")],
                        label: "Split".into(),
                        shortcut: String::new(),
                    },
                ],
            }],
            MenuStyle::Classic => vec![
                Menu {
                    title: "File".into(),
                    items: vec![leaf("New Tab", "")],
                },
                Menu {
                    title: "View".into(),
                    items: vec![leaf("Theme", "")],
                },
            ],
        };
        TopTabbar {
            active_tab: 0,
            context_menu: None,
            controls_side: ControlsSide::Right,
            menu_style: style,
            menus,
            open_menu: open,
            open_submenu: None,
            selected_item: None,
            selected_subitem: None,
            tabs: (0..tabs)
                .map(|i| TabLabel {
                    title: format!("Tab {i}"),
                    zoomed: false,
                })
                .collect(),
            tabbar_hover: TabbarHit::None,
            url_tooltip: None,
            window_controls: false,
        }
    }

    #[test]
    fn test_tabbar_is_two_rows_in_both_styles() {
        assert_eq!(tabbar_rows(MenuStyle::Modern), 2);
        assert_eq!(tabbar_rows(MenuStyle::Classic), 2);
    }

    #[test]
    fn test_modern_tabbar_is_top_row_classic_is_second() {
        let modern = layout(&tabbar(MenuStyle::Modern, 2, None), SURFACE_W, CW, CH);
        assert_eq!(modern.tab_row_top, 0.0);
        assert_eq!(modern.menubar_top, None);

        let classic = layout(&tabbar(MenuStyle::Classic, 2, None), SURFACE_W, CW, CH);
        assert_eq!(classic.tab_row_top, CH);
        assert_eq!(classic.menubar_top, Some(0.0));
    }

    #[test]
    fn test_extra_tabbar_height_grows_the_band_without_growing_the_pill() {
        // `modern_tabbar_height_px` tops up the Modern band by
        // `TABBAR_EXTRA_HEIGHT_PX`, and `tab_top_inset_px` tops up the tab
        // pill's own top inset by the exact same amount, while the bottom
        // inset (`TAB_BOTTOM_VPAD_PX`) never carries that top-up. So that
        // part of the extra space lands entirely above the tabs: it must
        // not, on its own, grow the pill/active-highlight height.
        let modern = layout(&tabbar(MenuStyle::Modern, 2, None), SURFACE_W, CW, CH);
        let band_h = modern.tabs[0].h;
        assert_eq!(
            band_h,
            crate::MODERN_TABBAR_HEIGHT * CH + crate::TABBAR_EXTRA_HEIGHT_PX
        );

        let pill_h = band_h - tab_top_inset_px(MenuStyle::Modern) - TAB_BOTTOM_VPAD_PX;
        assert_eq!(
            pill_h,
            crate::MODERN_TABBAR_HEIGHT * CH - TAB_TOP_VPAD_PX - TAB_BOTTOM_VPAD_PX
        );

        // Classic style has no flat band top-up (only Modern's ratio-based
        // height gets one), so its top inset is exactly `TAB_TOP_VPAD_PX`.
        assert_eq!(tab_top_inset_px(MenuStyle::Classic), TAB_TOP_VPAD_PX);
    }

    #[test]
    fn test_hit_test_picks_tab_then_new_tab() {
        let c = tabbar(MenuStyle::Modern, 2, None);
        let l = layout(&c, SURFACE_W, CW, CH);
        // A point in each tab's left title area, clear of the close button.
        let title_pt = |t: Region| t.x + TAB_H_PAD_CELLS * CW + CW;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, title_pt(l.tabs[0]), 5.0),
            TabbarHit::Tab(0)
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, title_pt(l.tabs[1]), 5.0),
            TabbarHit::Tab(1)
        );
        // The new-tab button sits just past the last tab.
        let nt = l.new_tab;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, nt.x + nt.w / 2.0, 5.0),
            TabbarHit::NewTab
        );
    }

    #[test]
    fn test_close_target_at_right_edge_of_tab() {
        let c = tabbar(MenuStyle::Modern, 1, None);
        let l = layout(&c, SURFACE_W, CW, CH);
        // The center of the close region hits the close button.
        let close = l.closes[0];
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, close.x + close.w / 2.0, 5.0),
            TabbarHit::CloseTab(0)
        );
        // The strip right of the close button is the right-padding area, which
        // still counts as the tab (not the close button).
        let tab = l.tabs[0];
        let right_pad = tab.x + tab.w - (CW / 2.0);
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, right_pad, 5.0),
            TabbarHit::Tab(0)
        );
        // The far left of the tab is the title area, not the close button.
        let left_of_tab = tab.x + CW;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, left_of_tab, 5.0),
            TabbarHit::Tab(0)
        );
    }

    #[test]
    fn test_few_tabs_grow_to_max_cap() {
        // With only two tabs and a wide surface, each grows to the MAX cap
        // instead of staying at the old fixed 18-cell width, and no scroll
        // arrows appear.
        let l = layout(&tabbar(MenuStyle::Modern, 2, None), SURFACE_W, CW, CH);
        // Tab 0 is inset by its own extra left pad, so it's narrower by that
        // amount; every other tab (here, tab 1) reaches the cap in full.
        assert_eq!(
            l.tabs[0].w,
            MAX_TAB_CELLS * CW - FIRST_TAB_LEFT_PAD_CELLS * CW
        );
        assert_eq!(l.tabs[1].w, MAX_TAB_CELLS * CW);
        assert!(l.scroll_left.is_none() && l.scroll_right.is_none());
    }

    #[test]
    fn test_many_tabs_paginate_with_min_width_and_arrows() {
        // Enough tabs that even MIN-width tabs overflow: the strip paginates.
        let c = tabbar(MenuStyle::Modern, 12, None);
        let l = layout(&c, SURFACE_W, CW, CH);
        assert!(l.scroll_left.is_some() && l.scroll_right.is_some());
        // Visible tabs keep at least the minimum width; off-window tabs are
        // collapsed to zero width and not all 12 fit.
        let visible: Vec<usize> = (0..12).filter(|&i| l.tabs[i].w > 0.0).collect();
        assert!(!visible.is_empty() && visible.len() < 12);
        for &i in &visible {
            assert!(l.tabs[i].w >= MIN_TAB_CELLS * CW);
        }
        // The active tab (0) is within the visible window.
        assert!(l.tabs[0].w > 0.0);
        // The scroll arrows are hittable.
        let sl = l.scroll_left.unwrap();
        let sr = l.scroll_right.unwrap();
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, sl.x + sl.w / 2.0, sl.y + sl.h / 2.0),
            TabbarHit::ScrollTabsLeft
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, sr.x + sr.w / 2.0, sr.y + sr.h / 2.0),
            TabbarHit::ScrollTabsRight
        );
    }

    #[test]
    fn test_pagination_keeps_active_tab_visible() {
        // A high active-tab index scrolls the window so that tab stays visible
        // while earlier tabs collapse out of view.
        let mut c = tabbar(MenuStyle::Modern, 12, None);
        c.active_tab = 10;
        let l = layout(&c, SURFACE_W, CW, CH);
        assert!(l.tabs[10].w > 0.0, "active tab is visible");
        assert_eq!(l.tabs[0].w, 0.0, "earlier tab scrolled out");
    }

    #[test]
    fn test_hamburger_only_in_modern() {
        let modern = tabbar(MenuStyle::Modern, 1, None);
        // The hamburger now sits at the left edge.
        assert_eq!(
            hit_test(&modern, SURFACE_W, CW, CH, 5.0, 5.0),
            TabbarHit::Hamburger
        );
        // Classic has menu titles on the top row instead.
        let classic = tabbar(MenuStyle::Classic, 1, None);
        assert_eq!(
            hit_test(&classic, SURFACE_W, CW, CH, 2.0, 2.0),
            TabbarHit::MenuTitle(0)
        );
    }

    #[test]
    fn test_open_dropdown_items_are_hit_first() {
        let c = tabbar(MenuStyle::Modern, 1, Some(0));
        let layout = layout(&c, SURFACE_W, CW, CH);
        let dropdown = layout.dropdown.expect("dropdown open");
        let first = dropdown_item_region(&dropdown, 0);
        let hit = hit_test(
            &c,
            SURFACE_W,
            CW,
            CH,
            first.x + 2.0,
            first.y + first.h / 2.0,
        );
        assert_eq!(hit, TabbarHit::DropdownItem(0));
        let second = dropdown_item_region(&dropdown, 1);
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, second.x + 2.0, second.y + 2.0),
            TabbarHit::DropdownItem(1)
        );
    }

    #[test]
    fn test_minimize_maximize_hover_pills_do_not_overlap() {
        // Minimize and maximize sit flush against each other with no
        // button-level gap. The shared hover-pill width (matched to the
        // wider new-tab button) exceeds their own box width, so without
        // `control_margin` reserving extra spacing, their centered hover
        // pills would bleed into each other.
        let mut c = tabbar(MenuStyle::Modern, 1, None);
        c.window_controls = true;
        let hover_pill_w = NEW_TAB_CELLS * CW - 2.0 * HOVER_PILL_H_PAD_CELLS * CW;

        for side in [ControlsSide::Left, ControlsSide::Right] {
            c.controls_side = side;
            let layout = layout(&c, SURFACE_W, CW, CH);
            let [minimize, maximize, _close] = layout.controls.expect("controls present");
            let min_pill_end = minimize.x + (minimize.w + hover_pill_w) / 2.0;
            let max_pill_start = maximize.x + (maximize.w - hover_pill_w) / 2.0;
            assert!(
                min_pill_end <= max_pill_start,
                "minimize/maximize hover pills overlap under {side:?}: \
                 minimize ends at {min_pill_end}, maximize starts at {max_pill_start}"
            );
        }
    }

    #[test]
    fn test_window_controls_hit_only_when_enabled() {
        let mut c = tabbar(MenuStyle::Modern, 1, None);
        // Disabled by default: the far-right edge is empty title-bar space.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W - 5.0, 5.0),
            TabbarHit::None
        );

        c.window_controls = true;
        let w = CONTROL_CELLS * CW;

        // Rightmost is the wider close button, then maximize, then minimize
        // moving left; click each one's center to stay clear of the margins
        // now reserved between them for the shared hover pill.
        let layout = layout(&c, SURFACE_W, CW, CH);
        let [minimize, maximize, close] = layout.controls.expect("controls present");
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, close.x + close.w / 2.0, 5.0),
            TabbarHit::Close
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, maximize.x + maximize.w / 2.0, 5.0),
            TabbarHit::Maximize
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, minimize.x + minimize.w / 2.0, 5.0),
            TabbarHit::Minimize
        );
        assert_eq!(minimize.w, w);
        assert_eq!(maximize.w, w);
        assert!(
            close.w > minimize.w && close.w > maximize.w,
            "close ({}) should be wider than minimize ({}) and maximize ({})",
            close.w,
            minimize.w,
            maximize.w
        );
    }

    #[test]
    fn test_close_control_left_pad_clears_maximize() {
        // Close sits flush against maximize; it gets a small pad on its own
        // left edge so a click right at the old, unpadded boundary now lands
        // in the gap instead of the button.
        let mut c = tabbar(MenuStyle::Modern, 1, None);
        c.window_controls = true;
        let close_w = CLOSE_CONTROL_CELLS * CW;
        let close_x = SURFACE_W - close_w;
        let close_pad = CLOSE_CONTROL_LEFT_PAD_CELLS * CW;
        // Close's left edge (bordering maximize) is now inset by `close_pad`;
        // a click just inside the old boundary misses both buttons.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, close_x + close_pad / 2.0, 5.0),
            TabbarHit::None
        );
    }

    #[test]
    fn test_hamburger_right_pad_clears_the_first_tab() {
        // `FIRST_TAB_LEFT_PAD_CELLS` is zero, so the small gap between the
        // hamburger and the first tab comes entirely from the hamburger's
        // own right-edge pad, not from the tab's side.
        let mut c = tabbar(MenuStyle::Modern, 1, None);
        c.window_controls = true;
        let layout = layout(&c, SURFACE_W, CW, CH);
        let hamburger = layout.hamburger.expect("hamburger present");
        assert_eq!(
            hamburger.w,
            HAMBURGER_CELLS * CW - HAMBURGER_RIGHT_PAD_CELLS * CW
        );
        assert_eq!(
            hamburger.x + hamburger.w + HAMBURGER_RIGHT_PAD_CELLS * CW,
            layout.tabs[0].x
        );
    }

    #[test]
    fn test_click_in_empty_space_is_none() {
        let c = tabbar(MenuStyle::Modern, 1, None);
        // Middle of the tabbar row, past the new-tab button, before the hamburger.
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, SURFACE_W / 2.0, 5.0),
            TabbarHit::None
        );
    }

    #[test]
    fn test_controls_left_puts_hamburger_right_and_mirrors_hits() {
        let mut c = tabbar(MenuStyle::Modern, 1, None);
        c.window_controls = true;
        c.controls_side = ControlsSide::Left;

        // Left edge is now Close (wider, inset by its left pad), then
        // Minimize, then Maximize; click each one's center to stay clear of
        // the margins reserved between them for the shared hover pill.
        let layout = layout(&c, SURFACE_W, CW, CH);
        let [minimize, maximize, close] = layout.controls.expect("controls present");
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, close.x + close.w / 2.0, 5.0),
            TabbarHit::Close
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, minimize.x + minimize.w / 2.0, 5.0),
            TabbarHit::Minimize
        );
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, maximize.x + maximize.w / 2.0, 5.0),
            TabbarHit::Maximize
        );

        // Hamburger moved to the far right.
        assert_eq!(
            hit_test(
                &c,
                SURFACE_W,
                CW,
                CH,
                SURFACE_W - HAMBURGER_CELLS * CW + 1.0,
                5.0
            ),
            TabbarHit::Hamburger
        );
        let hamburger = layout.hamburger.expect("hamburger present");
        assert_eq!(
            hamburger.x + hamburger.w,
            SURFACE_W - HAMBURGER_RIGHT_PAD_CELLS * CW
        );

        // Tabs start after the three controls (and their margins), not at the
        // hamburger's old left slot. Close button is at the right of each
        // tab; the title area begins after the left padding.
        let tabs_left = layout.tabs[0].x;
        let title_area = tabs_left + TAB_H_PAD_CELLS * CW + CW;
        assert_eq!(
            hit_test(&c, SURFACE_W, CW, CH, title_area, 5.0),
            TabbarHit::Tab(0)
        );
        // The first cell is no longer the hamburger (it became Close).
        assert_ne!(
            hit_test(&c, SURFACE_W, CW, CH, 1.0, 5.0),
            TabbarHit::Hamburger
        );
    }

    #[test]
    fn test_submenu_opens_right_of_parent_and_is_hit_first() {
        let mut c = tabbar(MenuStyle::Modern, 1, Some(0));
        c.open_submenu = Some(1); // "Split" carries children.
        let layout = layout(&c, SURFACE_W, CW, CH);
        let parent = layout.dropdown.expect("parent open");
        let submenu = layout.submenu.expect("submenu open");
        // The child panel sits immediately right of the parent panel.
        assert_eq!(submenu.origin_x, parent.origin_x + parent.width);
        assert_eq!(submenu.items, 2);
        // A click on a child resolves to the submenu, which overlays the parent.
        let child = dropdown_item_region(&submenu, 0);
        assert_eq!(
            hit_test(
                &c,
                SURFACE_W,
                CW,
                CH,
                child.x + 2.0,
                child.y + child.h / 2.0
            ),
            TabbarHit::SubmenuItem(0)
        );
    }

    #[test]
    fn test_no_submenu_without_an_open_parent() {
        // open_submenu set but no menu open: no submenu geometry.
        let mut c = tabbar(MenuStyle::Modern, 1, None);
        c.open_submenu = Some(1);
        assert!(layout(&c, SURFACE_W, CW, CH).submenu.is_none());
    }

    #[test]
    fn test_classic_dropdown_anchors_under_its_title() {
        let c = tabbar(MenuStyle::Classic, 1, Some(1));
        let layout = layout(&c, SURFACE_W, CW, CH);
        let dropdown = layout.dropdown.expect("dropdown open");
        // Opens directly below the menubar row.
        assert_eq!(dropdown.top, CH);
        // Left edge aligns with the second menu title.
        assert_eq!(dropdown.origin_x, layout.menu_titles[1].x);
    }
}

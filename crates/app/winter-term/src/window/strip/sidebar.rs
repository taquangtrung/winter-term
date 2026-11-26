//! Flat icon switcher for a sidebar panel: clean view icons on the header
//! surface with rounded hover/active pills and a bottom accent indicator.

use misty::accesskit::{Action, Node, Role};
use misty::{
    AccessAction, BoxStyle, EventCx, FocusId, Frame, IconSet, Input, Point, PointerButton, Rect,
    Rgba, Theme,
};

use winter_model::layout::TabId;

use super::view::StripTab;

// ========================================================================
// Constants
// ========================================================================

/// Thickness of the active view's bottom accent bar, in logical pixels.
const ACCENT_HEIGHT: f32 = 2.0;

/// Share of the icon pill width covered by the bottom accent indicator.
const ACCENT_WIDTH_SHARE: f32 = 0.65;

/// Mix share of `theme.hover` into `theme.surface` for the active pill at rest.
const ACTIVE_PILL_SHARE: f32 = 0.75;

/// Diameter of the unseen-attention indicator dot, in logical pixels.
const ATTENTION_DOT_SIZE: f32 = 5.0;

/// Extra horizontal room added to the button height for a balanced pill shape.
const BUTTON_EXTRA_WIDTH: f32 = 4.0;

/// Reduced gap between icon buttons when the sidebar is narrow, in logical pixels.
const COMPACT_GAP: f32 = 2.0;

/// Corner radius of each icon button's hover/active pill, in logical pixels.
const CORNER_RADIUS: f32 = 5.0;

/// Gap between icon buttons at comfortable widths, in logical pixels.
const DEFAULT_GAP: f32 = 4.0;

/// Thickness of the bottom border separating the switcher from the view below.
const HAIRLINE: f32 = 1.0;

/// Icon side length as a share of the cell height.
const ICON_HEIGHT_SHARE: f32 = 0.78;

/// Mix share of `theme.surface` into `theme.accent` when unfocused or inactive.
const INACTIVE_ACCENT_SHARE: f32 = 0.45;

/// Minimum width of the bottom accent indicator, in logical pixels.
const MIN_ACCENT_WIDTH: f32 = 8.0;

/// Horizontal padding at the left and right ends of the icon row, in logical pixels.
const OUTER_PAD: f32 = 6.0;

/// Vertical inset of each icon pill from the strip's top and bottom border.
const VERTICAL_INSET: f32 = 4.0;

// ========================================================================
// Data Structures
// ========================================================================

/// A sidebar panel's flat icon switcher.
#[derive(Debug, Default)]
pub(super) struct SidebarBar {
    /// Currently shown view in this sidebar.
    active: Option<TabId>,
    /// Last drawn strip bounds, for hit testing and accessibility.
    bar: Rect,
    /// Whether this sidebar currently holds keyboard focus.
    focused: bool,
    /// View currently under the pointer, if any.
    hovered: Option<TabId>,
    /// Bundled icons used by this sidebar's views.
    icons: IconSet,
    /// Views shown left to right.
    items: Vec<SidebarItem>,
    /// Last laid-out button geometry, in sync with `items`.
    slots: Vec<SidebarSlot>,
    /// Whether the OS window currently holds focus.
    window_focused: bool,
}

/// What became of an input offered to a [`SidebarBar`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SidebarResponse {
    /// This view was clicked and should be shown and focused.
    Activated(TabId),
    /// The switcher consumed the event (e.g. hover update).
    Handled,
    /// The event was outside the switcher.
    Ignored,
}

/// One view entry in a [`SidebarBar`].
#[derive(Clone, Debug)]
struct SidebarItem {
    /// Whether an agent in this view is waiting unseen.
    attention: bool,
    /// Bundled icon name for this view.
    icon: Option<String>,
    /// Winter tab backing this view.
    tab: TabId,
    /// Accessible name and tooltip label.
    title: String,
}

/// Laid-out geometry for one icon button.
#[derive(Clone, Copy, Debug)]
struct SidebarSlot {
    /// Full hit target from strip top to strip bottom.
    hit: Rect,
    /// Rounded highlight pill bounds.
    pill: Rect,
    /// Backing view id.
    tab: TabId,
}

// ========================================================================
// SidebarBar
// ========================================================================

impl SidebarBar {
    /// An empty sidebar switcher.
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Whether `tab` is currently among this sidebar's views.
    pub(super) fn has(&self, tab: TabId) -> bool {
        self.items.iter().any(|item| item.tab == tab)
    }

    /// Bring the switcher in step with the sidebar's views and focus state.
    pub(super) fn sync(
        &mut self,
        shown: &[StripTab],
        active: TabId,
        focused: bool,
        window_focused: bool,
    ) {
        self.items = shown
            .iter()
            .map(|tab| SidebarItem {
                attention: tab.attention,
                icon: tab.icon.clone(),
                tab: tab.tab,
                title: tab.title.clone(),
            })
            .collect();
        let names: Vec<String> = shown.iter().filter_map(|tab| tab.icon.clone()).collect();
        crate::icons::add_to(&mut self.icons, &names);
        self.active = Some(active);
        self.focused = focused;
        self.window_focused = window_focused;
        if let Some(hovered) = self.hovered
            && !self.items.iter().any(|item| item.tab == hovered)
        {
            self.hovered = None;
        }
    }

    /// Draw the flat icon bar into `bar` and record its slot geometry.
    pub(super) fn draw(&mut self, frame: &mut Frame, bar: Rect, theme: &Theme) {
        let scale = frame.scale_factor() as f32;
        let stroke = scale_px(HAIRLINE, scale).max(1.0);
        let radius = scale_px(CORNER_RADIUS, scale);
        let cell_h = frame.cell_metrics().height;

        frame.fill_rect(bar, theme.surface);
        frame.fill_rect(
            Rect::new(bar.x, bar.y + bar.height - stroke, bar.width, stroke),
            theme.border,
        );

        let tabs: Vec<TabId> = self.items.iter().map(|item| item.tab).collect();
        self.slots = layout_slots(bar, &tabs, scale);
        self.bar = bar;

        let active_fill = theme.surface.mix(theme.hover, ACTIVE_PILL_SHARE);
        let accent_color = match self.focused && self.window_focused {
            true => theme.accent,
            false => theme.accent.mix(theme.surface, INACTIVE_ACCENT_SHARE),
        };
        let accent_h = scale_px(ACCENT_HEIGHT, scale).max(1.0);
        let dot_size = scale_px(ATTENTION_DOT_SIZE, scale).max(2.0);
        let icon_inset = scale_px(2.0, scale);

        for (item, slot) in self.items.iter().zip(&self.slots) {
            let is_active = self.active == Some(item.tab);
            let is_hovered = self.hovered == Some(item.tab);

            let pill_fill = match (is_active, is_hovered) {
                (_, true) => Some(theme.hover),
                (true, false) => Some(active_fill),
                (false, false) => None,
            };
            if let Some(fill) = pill_fill {
                let style = BoxStyle {
                    border: None,
                    corner_radius: radius,
                    fill: Rgba::from(fill),
                    shadow: None,
                };
                frame.draw_box(slot.pill, &style);
            }

            let icon_color = match is_active || is_hovered {
                true => theme.foreground,
                false => theme.muted,
            };
            if let Some(name) = item.icon.as_deref() {
                let side = (cell_h * ICON_HEIGHT_SHARE)
                    .round()
                    .min(slot.pill.height - icon_inset)
                    .min(slot.pill.width - icon_inset)
                    .max(1.0);
                let icon_rect = Rect::new(
                    slot.pill.x + ((slot.pill.width - side) / 2.0).round(),
                    slot.pill.y + ((slot.pill.height - side) / 2.0).round(),
                    side,
                    side,
                );
                frame.draw_named_icon(&self.icons, name, icon_rect, icon_color);
            }

            if item.attention {
                let dot = Rect::new(
                    (slot.pill.x + slot.pill.width - dot_size - scale_px(2.0, scale)).round(),
                    (slot.pill.y + scale_px(2.0, scale)).round(),
                    dot_size,
                    dot_size,
                );
                let dot_style = BoxStyle {
                    border: None,
                    corner_radius: dot_size / 2.0,
                    fill: Rgba::from(theme.accent),
                    shadow: None,
                };
                frame.draw_box(dot, &dot_style);
            }

            if is_active {
                let accent_w = (slot.pill.width * ACCENT_WIDTH_SHARE)
                    .round()
                    .max(scale_px(MIN_ACCENT_WIDTH, scale))
                    .min(slot.pill.width);
                let accent_rect = Rect::new(
                    slot.pill.x + ((slot.pill.width - accent_w) / 2.0).round(),
                    bar.y + bar.height - accent_h,
                    accent_w,
                    accent_h,
                );
                let accent_style = BoxStyle {
                    border: None,
                    corner_radius: accent_h / 2.0,
                    fill: Rgba::from(accent_color),
                    shadow: None,
                };
                frame.draw_box(accent_rect, &accent_style);
            }
        }
    }

    /// Describe the switcher and its view buttons to screen readers.
    pub(super) fn access(&self, frame: &mut Frame, id: FocusId) {
        if self.slots.is_empty() {
            return;
        }
        frame.enter_access(id, self.bar, Node::new(Role::TabList));
        for (item, slot) in self.items.iter().zip(&self.slots) {
            let mut node = Node::new(Role::Tab);
            node.set_label(item.title.as_str());
            node.set_selected(self.active == Some(item.tab));
            node.add_action(Action::Click);
            frame.access(id.child(item.tab.0), slot.pill, node);
        }
        frame.leave_access();
    }

    /// Carry out a screen-reader click on one of this switcher's view nodes.
    pub(super) fn access_action(&self, id: FocusId, action: AccessAction) -> SidebarResponse {
        let AccessAction::Click(target) = action else {
            return SidebarResponse::Ignored;
        };
        let found = self
            .items
            .iter()
            .find(|item| id.child(item.tab.0) == target)
            .map(|item| item.tab);
        match found {
            Some(tab) => SidebarResponse::Activated(tab),
            None => SidebarResponse::Ignored,
        }
    }

    /// Handle pointer movement, leaving, and left clicks on the switcher.
    pub(super) fn handle(&mut self, input: &Input, cx: &mut EventCx<'_>) -> SidebarResponse {
        match input {
            Input::PointerMoved(point) => {
                let hit = self.hit_tab(*point);
                if self.hovered != hit {
                    self.hovered = hit;
                    cx.request_redraw();
                    return SidebarResponse::Handled;
                }
                match hit.is_some() {
                    true => SidebarResponse::Handled,
                    false => SidebarResponse::Ignored,
                }
            }
            Input::PointerLeft => {
                if self.hovered.take().is_some() {
                    cx.request_redraw();
                }
                SidebarResponse::Ignored
            }
            Input::PointerButton(button) => {
                if !button.pressed || button.button != PointerButton::Left {
                    return SidebarResponse::Ignored;
                }
                match self.hit_tab(button.position) {
                    Some(tab) => {
                        self.active = Some(tab);
                        cx.request_redraw();
                        SidebarResponse::Activated(tab)
                    }
                    None if self.bar.contains(button.position) => SidebarResponse::Handled,
                    None => SidebarResponse::Ignored,
                }
            }
            _ => SidebarResponse::Ignored,
        }
    }

    /// View at `point`, if any.
    pub(super) fn hit_tab(&self, point: Point) -> Option<TabId> {
        self.slots
            .iter()
            .find(|slot| slot.hit.contains(point))
            .map(|slot| slot.tab)
    }

    /// Pill bounds for the icon at `point`, for anchoring tooltips.
    pub(super) fn part_rect(&self, point: Point) -> Option<Rect> {
        self.slots
            .iter()
            .find(|slot| slot.hit.contains(point))
            .map(|slot| slot.pill)
    }
}

// ========================================================================
// Functions
// ========================================================================

/// Scale logical pixels `px` by `scale`, rounded to whole physical pixels.
fn scale_px(px: f32, scale: f32) -> f32 {
    (px * scale).round()
}

/// Lay out `tabs` left-to-right inside `bar`, shrinking gap and width when
/// needed so every icon fits inside narrow sidebars.
fn layout_slots(bar: Rect, tabs: &[TabId], scale: f32) -> Vec<SidebarSlot> {
    let count = tabs.len();
    if count == 0 || bar.width <= 0.0 || bar.height <= 0.0 {
        return Vec::new();
    }
    let stroke = scale_px(HAIRLINE, scale).max(1.0);
    let h_pad = scale_px(OUTER_PAD, scale);
    let v_inset = scale_px(VERTICAL_INSET, scale);
    let pill_h = (bar.height - stroke - 2.0 * v_inset).max(1.0);
    let ideal_w = (pill_h + scale_px(BUTTON_EXTRA_WIDTH, scale)).round();
    let default_gap = scale_px(DEFAULT_GAP, scale);
    let available = (bar.width - 2.0 * h_pad).max(1.0);

    let total_ideal = count as f32 * ideal_w + (count - 1) as f32 * default_gap;
    let (pill_w, gap) = if total_ideal <= available {
        (ideal_w, default_gap)
    } else {
        let gap = scale_px(COMPACT_GAP, scale);
        let width = ((available - (count - 1) as f32 * gap) / count as f32)
            .floor()
            .max(1.0);
        (width, gap)
    };

    let mut x = bar.x + h_pad;
    let pill_y = bar.y + v_inset;
    let mut slots = Vec::with_capacity(count);
    for &tab in tabs {
        let pill = Rect::new(x, pill_y, pill_w, pill_h);
        let hit = Rect::new(x, bar.y, pill_w, bar.height);
        slots.push(SidebarSlot { hit, pill, tab });
        x += pill_w + gap;
    }
    slots
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layout_fits_all_icons_without_overlap_in_narrow_sidebar() {
        let tabs = [TabId(1), TabId(2), TabId(3), TabId(4), TabId(5)];
        let bar = Rect::new(0.0, 0.0, 100.0, 30.0);
        let slots = layout_slots(bar, &tabs, 1.0);
        assert_eq!(slots.len(), tabs.len());
        for pair in slots.windows(2) {
            assert!(
                pair[0].pill.x + pair[0].pill.width < pair[1].pill.x,
                "adjacent pills keep a gap"
            );
        }
        let last = slots.last().expect("non-empty");
        assert!(
            last.pill.x + last.pill.width <= bar.x + bar.width,
            "all icons fit within narrow sidebar width"
        );
    }

    #[test]
    fn test_hit_tab_and_access_action_target_matching_view() {
        let mut bar = SidebarBar::new();
        let first = TabId(10);
        let second = TabId(20);
        bar.items = vec![
            SidebarItem {
                attention: false,
                icon: None,
                tab: first,
                title: "Files".to_string(),
            },
            SidebarItem {
                attention: false,
                icon: None,
                tab: second,
                title: "Git".to_string(),
            },
        ];
        bar.bar = Rect::new(0.0, 0.0, 200.0, 32.0);
        bar.slots = layout_slots(bar.bar, &[first, second], 1.0);

        let second_center = Point::new(
            bar.slots[1].pill.x + bar.slots[1].pill.width / 2.0,
            bar.slots[1].pill.y + bar.slots[1].pill.height / 2.0,
        );
        assert_eq!(bar.hit_tab(second_center), Some(second));
        assert_eq!(bar.part_rect(second_center), Some(bar.slots[1].pill));

        let id = FocusId::new("winter.strip").child(0);
        assert_eq!(
            bar.access_action(id, AccessAction::Click(id.child(second.0))),
            SidebarResponse::Activated(second)
        );
        assert_eq!(
            bar.access_action(id, AccessAction::Click(id.child(999))),
            SidebarResponse::Ignored
        );
    }
}

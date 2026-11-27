//! Shared card-based UI building blocks for sidebar views with small item
//! counts (Sessions): rounded cards, section headers with count badges, and
//! compact icon action buttons.

use misty::layout::Align;
use misty::view::{Corners, Extent, Press, Space, column, row, text};
use misty::{Element, IconSet, ThemeToken};

use winter_model::paint::IconKind;

use super::{
    CLOSED_MARK, FOLD_PX, FOLD_TIP, OPEN_MARK, PanelView, RowIcon, UNFOLD_TIP, art_name, icon_view,
};

// ========================================================================
// Constants
// ========================================================================

/// Horizontal and vertical padding inside an icon action button, in logical
/// pixels.
const ACTION_PAD_PX: (f32, f32) = (5.0, 3.0);

/// Horizontal and vertical padding inside a card, in logical pixels.
const CARD_PAD_PX: (f32, f32) = (10.0, 8.0);

/// Vertical gap between elements stacked inside a card, in logical pixels.
const CARD_ROW_GAP_PX: f32 = 4.0;

/// Horizontal and vertical padding inside a section count chip, in logical
/// pixels.
const COUNT_PAD_PX: (f32, f32) = (5.0, 1.0);

/// Padding around an empty-state placeholder, in logical pixels.
const EMPTY_PAD_PX: (f32, f32) = (12.0, 16.0);

/// Horizontal gap between elements on a card's header or detail line.
const INLINE_GAP_PX: f32 = 6.0;

/// Vertical gap between cards and sections in a card list, in logical pixels.
const LIST_GAP_PX: f32 = 8.0;

/// Outer padding around a scrolling card list, in logical pixels.
const LIST_PAD_PX: (f32, f32) = (8.0, 8.0);

/// Horizontal gap between a section label and its count chip, in logical
/// pixels.
const SECTION_GAP_PX: f32 = 6.0;

/// Relative font scale for section header labels and counts.
const SECTION_LABEL_SCALE: f32 = 0.85;

/// Horizontal and vertical padding around a section header, in logical pixels.
const SECTION_PAD_PX: (f32, f32) = (2.0, 0.0);

/// Extra vertical space above a following section header, in logical pixels.
const SECTION_SPACE_ABOVE_PX: f32 = 8.0;

// ========================================================================
// Functions
// ========================================================================

/// A scrolling vertical container for card-based sidebar views.
pub(super) fn card_list<'a, S: PanelView>(items: Vec<Element<'a, S>>) -> Element<'a, S> {
    column(items)
        .gap(Space::Px(LIST_GAP_PX))
        .padding_px(LIST_PAD_PX.0, LIST_PAD_PX.1)
        .scroll()
}

/// An empty-state message with a leading icon.
pub(super) fn empty_state<'a, S: PanelView>(
    icons: &'a IconSet,
    icon_name: &'static str,
    message: &'a str,
) -> Element<'a, S> {
    let mut parts: Vec<Element<'a, S>> = Vec::new();
    if let Some(art) = art_name(icons, &IconKind::Symbol { name: icon_name }) {
        parts.push(
            icon_view(icons, art, ThemeToken::Muted, message).id(format!("empty/{icon_name}")),
        );
    }
    parts.push(Element::from(text(message).muted()));
    row(parts)
        .gap(Space::Px(INLINE_GAP_PX))
        .padding_px(EMPTY_PAD_PX.0, EMPTY_PAD_PX.1)
}

/// A flush section header with a muted label, soft count chip, and clean
/// spacing above subsequent sections.
pub(super) fn section_bar<'a, S: PanelView>(
    label: &'a str,
    count: usize,
    separated: bool,
) -> Element<'a, S> {
    let title = Element::from(text(label).muted().emphasized().scale(SECTION_LABEL_SCALE));
    let badge = row([Element::from(
        text(count.to_string()).muted().scale(SECTION_LABEL_SCALE),
    )])
    .width(Extent::Auto)
    .padding_px(COUNT_PAD_PX.0, COUNT_PAD_PX.1)
    .corners(Corners::Control)
    .background(ThemeToken::Surface);
    let bar = row([title, badge])
        .align(Align::Center)
        .gap(Space::Px(SECTION_GAP_PX))
        .padding_px(SECTION_PAD_PX.0, SECTION_PAD_PX.1);
    let header = match separated {
        true => {
            let spacer =
                column(Vec::<Element<'a, S>>::new()).height(Extent::Px(SECTION_SPACE_ABOVE_PX));
            column([spacer, bar]).gap(Space::Px(0.0))
        }
        false => bar,
    };
    header.id(format!("section/{label}"))
}

/// A rounded card container with selection highlight, hover feedback, and
/// scroll-into-view tracking.
pub(super) fn card_shell<'a, S: PanelView>(
    id: String,
    selected: bool,
    tooltip: Option<&'a str>,
    lines: Vec<Element<'a, S>>,
    on_press: impl Fn(&mut S, Press) + 'static,
) -> Element<'a, S> {
    let mut card = column(lines)
        .gap(Space::Px(CARD_ROW_GAP_PX))
        .padding_px(CARD_PAD_PX.0, CARD_PAD_PX.1)
        .corners(Corners::Box)
        .on_pointer_press(on_press)
        .id(id);
    card = match selected {
        true => card
            .background(ThemeToken::SelectedRow)
            .border(ThemeToken::Accent)
            .keep_in_view(),
        false => card
            .background(ThemeToken::Surface)
            .border(ThemeToken::Border)
            .hover_border(ThemeToken::Accent),
    };
    match tooltip {
        Some(tip) => card.tooltip(tip),
        None => card,
    }
}

/// A leading icon for a card title or detail row, if its artwork is loaded.
pub(super) fn leading_icon<'a, S: PanelView>(
    icons: &'a IconSet,
    icon: &'a RowIcon,
) -> Option<Element<'a, S>> {
    let art = art_name(icons, &icon.kind)?;
    Some(icon_view(icons, art, icon.color, &icon.name))
}

/// A compact fold chevron button for expanding or collapsing a card's details.
pub(super) fn card_fold_button<'a, S: PanelView>(
    open: bool,
    on_press: impl Fn(&mut S, Press) + 'static,
) -> Element<'a, S> {
    let (glyph, tip) = match open {
        true => (OPEN_MARK, FOLD_TIP),
        false => (CLOSED_MARK, UNFOLD_TIP),
    };
    row([Element::from(text(glyph).muted())])
        .width(Extent::Px(FOLD_PX))
        .corners(Corners::Control)
        .hover_background(ThemeToken::Hover)
        .hover_border(ThemeToken::Border)
        .tooltip(tip)
        .on_pointer_press(on_press)
}

/// A compact icon button for an inline card action, with a key-hint tooltip.
pub(super) fn action_button<'a, S: PanelView>(
    icons: &'a IconSet,
    icon_name: &'static str,
    tooltip: &'static str,
    color: ThemeToken,
    on_press: impl Fn(&mut S, Press) + 'static,
) -> Option<Element<'a, S>> {
    let art = art_name(icons, &IconKind::Symbol { name: icon_name })?;
    let icon = icon_view(icons, art, color, tooltip);
    Some(
        row([icon])
            .width(Extent::Auto)
            .padding_px(ACTION_PAD_PX.0, ACTION_PAD_PX.1)
            .corners(Corners::Control)
            .hover_background(ThemeToken::Hover)
            .hover_border(ThemeToken::Border)
            .tooltip(tooltip)
            .on_pointer_press(on_press),
    )
}

/// A muted metadata line inside an expanded card, led by a symbol icon.
pub(super) fn detail_line<'a, S: PanelView>(
    icons: &'a IconSet,
    icon_name: &'static str,
    icon_label: &'static str,
    value: &'a str,
    tooltip: &'static str,
) -> Element<'a, S> {
    let mut parts: Vec<Element<'a, S>> = Vec::new();
    if let Some(art) = art_name(icons, &IconKind::Symbol { name: icon_name }) {
        parts.push(icon_view(icons, art, ThemeToken::Muted, icon_label));
    }
    parts.push(Element::from(text(value).muted()).grow(1.0));
    row(parts).gap(Space::Px(INLINE_GAP_PX)).tooltip(tooltip)
}

/// Inline gap for card header lines.
pub(super) fn inline_gap() -> Space {
    Space::Px(INLINE_GAP_PX)
}

//! The picker: a query line over a filtered list, centred under a decorated
//! title bar, or along the bottom of the pane a live picker is about. An
//! entry takes a line, or two when it has a detail; a side panel can show
//! the selected entry beside or under the list, and a line of key hints
//! closes it.

use super::view::{PanelView, PickerItem, PickerPlacement, PickerView};
use super::MENU_HOVER_INSET;
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::chrome::{
    bottom_in_rect, centered, centered_in_rect, draw_caret, draw_divider, shape_fitted,
    title_bar_height, Chrome, TitleBar, TITLE_BAR_TOP_PAD,
};
use crate::renderer::raster::paint::{
    blit_svg, buffer_width, draw_highlighted_label, shape_chrome_line,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::{
    editor_card_size, font_scale, DIALOG_MIN_WIDTH, DROPDOWN_SHADOW, ICON_GAP_CELLS,
    ICON_HEIGHT_RATIO, PALETTE_ITEM_PAD_Y, POPUP_EDGE_ROOM,
};
use crate::theme::Theme;
use glyphon::{Color, FontSystem, SwashCache};

// ========================================================================
// Constants
// ========================================================================

/// The palette's width and height as fractions of the surface.
///
/// A fixed size, as the key-hint card is: the list narrows as the query is
/// typed, and a panel that shrank with it would walk its own rows out from
/// under the eye reading them. How many results are visible follows from the
/// height rather than from a count of its own, so a taller window shows more
/// of them.
const PALETTE_WIDTH_RATIO: f32 = 0.7;

const PALETTE_HEIGHT_RATIO: f32 = 0.6;

/// Padding around the input field box inside the picker.
const PICKER_FIELD_PAD_X: f32 = 8.0;
const PICKER_FIELD_PAD_Y: f32 = 4.0;
const PICKER_FIELD_RADIUS: f32 = 4.0;

/// How wide the query's caret is drawn, in pixels.
const CARET_WIDTH: f32 = 2.0;

/// Extra horizontal padding inside each palette result row, in pixels, applied
/// to both the left label and the right hint.
const PALETTE_ITEM_PAD_X: f32 = 5.0;

/// How wide, in cells, a picker must be for its side panel to sit beside the
/// list rather than under it.
const PANEL_BESIDE_MIN_CELLS: f32 = 100.0;

/// The share of the width the list keeps beside a side panel, and of the
/// height it keeps over one.
const PANEL_LIST_SHARE: f32 = 0.45;

/// The gap between a row's buttons, and around them, in pixels.
const BUTTON_GAP: f32 = 6.0;

/// How strongly the side panel's focused line and its marks are tinted.
const PANEL_FOCUS_TINT: f32 = 0.15;
const PANEL_MARK_TINT: f32 = 0.35;

// ========================================================================
// Data Structures
// ========================================================================

/// The picker panel's size and the heights its parts take, in pixels, and
/// how many lines of entries fit in it: what drawing it and paging through it
/// both go by.
pub(in crate::renderer) struct PickerPanel {
    /// The key hints' line, or nothing without hints.
    hints_h: f32,
    /// The query line's band, its field and the padding around it.
    input_h: f32,
    /// One line of an entry.
    item_h: f32,
    panel_h: f32,
    panel_w: f32,
    /// How many lines of entries fit, at least one.
    pub(in crate::renderer) rows: usize,
    /// The title bar, or nothing for a picker that names nothing.
    title_h: f32,
    /// The padding above the title bar, or above the query line.
    top_pad: f32,
}

/// Where the parts of the picker sit inside its card, in pixels from the
/// card's top-left corner, the shadow's margin included.
struct PickerLayout {
    /// The list's left edge and width.
    list_x: f32,
    list_w: f32,
    /// The side panel's rectangle, when it shows.
    panel: Option<(f32, f32, f32, f32)>,
    /// The top and height of each entry drawn, in order; entries past the
    /// list's foot are not drawn.
    rows: Vec<(f32, f32)>,
    /// The top of the list, under the divider.
    results_top: f32,
}

/// A button of an entry, where it is drawn on the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickerButtonHit {
    /// Which of the entry's buttons.
    pub button: usize,
    /// Which entry, by its index in the view.
    pub item: usize,
    /// The button's rectangle on the surface.
    pub rect: (f32, f32, f32, f32),
}

/// An entry of the picker, or of the action menu over it, where it is drawn
/// on the surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickerRowHit {
    /// Which entry, by its index in the list's view.
    pub at: usize,
    /// Whether the entry is the action menu's rather than the list's.
    pub menu: bool,
    /// The entry's rectangle on the surface.
    pub rect: (f32, f32, f32, f32),
}

// ========================================================================
// Free functions
// ========================================================================

/// The panel `view` is drawn in, inside a `surface_w` by `surface_h`
/// window: a fixed fraction of the window by default (or the size it was
/// resized to), with as many lines of entries as its height holds. It does
/// not resize as the query narrows the list, which would walk its own rows
/// out from under the eye.
pub(in crate::renderer) fn picker_panel(
    ctx: &FontCtx,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> PickerPanel {
    let inner_pad = ctx.cell_h * 0.5;
    let item_h = (ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0).round();
    let input_h = ctx.field_h + PICKER_FIELD_PAD_Y * 2.0;
    let hints_h = match view.hints.is_empty() {
        true => 0.0,
        false => item_h,
    };
    // A titled list is headed by what it is a list of, above the line the
    // filter is typed on; a palette that names nothing keeps its own height.
    let (top_pad, title_h) = match view.title.is_empty() {
        true => (inner_pad, 0.0),
        false => (TITLE_BAR_TOP_PAD, title_bar_height(ctx)),
    };
    let min_h = top_pad + inner_pad + title_h + input_h + 1.0 + item_h + hints_h;
    let min_w = DIALOG_MIN_WIDTH * font_scale(ctx);
    let (base_w, base_h) = match view.size {
        Some((w, h)) => (w, h),
        None => match view.placement {
            PickerPlacement::WindowCenter => (
                (surface_w * PALETTE_WIDTH_RATIO)
                    .min(surface_w - POPUP_EDGE_ROOM)
                    .floor()
                    .max(min_w),
                (surface_h * PALETTE_HEIGHT_RATIO)
                    .min(surface_h - POPUP_EDGE_ROOM)
                    .floor()
                    .max(min_h),
            ),
            PickerPlacement::EditorBottom((_rx, _ry, rw, rh)) => (
                rw.floor(),
                (rh * 0.40).floor().max(min_h).min((rh - 10.0).max(min_h)),
            ),
            PickerPlacement::Pane((_, _, rw, rh)) => editor_card_size(ctx, (rw, rh)),
            PickerPlacement::Transient((_, _, rw, rh)) => (rw, rh),
        },
    };
    let panel_w = match view.placement {
        PickerPlacement::WindowCenter => base_w
            .clamp(min_w, (surface_w - POPUP_EDGE_ROOM).max(min_w))
            .floor(),
        PickerPlacement::EditorBottom((_, _, rw, _)) => match view.size {
            Some(_) => base_w.clamp(100.0, surface_w).floor(),
            None => rw.floor(),
        },
        PickerPlacement::Pane(_) | PickerPlacement::Transient(_) => {
            base_w.clamp(100.0, surface_w.max(100.0)).floor()
        }
    };
    let panel_h = match view.placement {
        PickerPlacement::WindowCenter => base_h
            .clamp(min_h, (surface_h - POPUP_EDGE_ROOM).max(min_h))
            .floor(),
        PickerPlacement::EditorBottom((_, _, _, rh)) => {
            base_h.clamp(min_h, (rh - 10.0).max(min_h)).floor()
        }
        PickerPlacement::Pane(_) | PickerPlacement::Transient(_) => {
            base_h.clamp(min_h, surface_h.max(min_h)).floor()
        }
    };
    let mut list_h = panel_h - top_pad - inner_pad - title_h - input_h - 1.0 - hints_h;
    if view.panel.is_some() && !is_panel_beside(ctx, panel_w) {
        list_h *= PANEL_LIST_SHARE;
    }
    let rows = ((list_h / item_h).floor() as usize).max(1);
    PickerPanel {
        hints_h,
        input_h,
        item_h,
        panel_h,
        panel_w,
        rows,
        title_h,
        top_pad,
    }
}

/// Whether a picker `panel_w` wide is wide enough for its side panel to sit
/// beside the list.
fn is_panel_beside(ctx: &FontCtx, panel_w: f32) -> bool {
    panel_w >= PANEL_BESIDE_MIN_CELLS * ctx.cell_w
}

/// Where the list, its entries, and the side panel sit in `panel`.
fn picker_layout(ctx: &FontCtx, view: &PickerView, panel: &PickerPanel) -> PickerLayout {
    let margin = DROPDOWN_SHADOW;
    let results_top = margin + panel.top_pad + panel.title_h + panel.input_h + 1.0;
    let results_h = margin + panel.panel_h - panel.hints_h - ctx.cell_h * 0.5 - results_top;
    let beside = is_panel_beside(ctx, panel.panel_w);
    let (list_w, list_h, panel_rect) = match (&view.panel, beside) {
        (None, _) => (panel.panel_w, results_h, None),
        (Some(_), true) => {
            let list_w = (panel.panel_w * PANEL_LIST_SHARE).floor();
            let rect = (
                margin + list_w,
                results_top,
                panel.panel_w - list_w,
                results_h,
            );
            (list_w, results_h, Some(rect))
        }
        (Some(_), false) => {
            let list_h = (results_h * PANEL_LIST_SHARE).floor();
            let rect = (
                margin,
                results_top + list_h,
                panel.panel_w,
                results_h - list_h,
            );
            (panel.panel_w, list_h, Some(rect))
        }
    };
    let detail_h = panel.item_h + ctx.line_height;
    let mut rows = Vec::new();
    let mut top = results_top;
    for item in &view.items {
        let height = match item.detail.is_empty() {
            true => panel.item_h,
            false => detail_h,
        };
        if top + height > results_top + list_h + 0.5 {
            break;
        }
        rows.push((top, height));
        top += height;
    }
    PickerLayout {
        list_w,
        list_x: margin,
        panel: panel_rect,
        results_top,
        rows,
    }
}

/// Where the picker's card goes on the surface: in the middle of the window,
/// or along the bottom of the pane, moved by any drag.
/// Where the picker drawn for `view` puts its caret, in surface pixels, as
/// `(left, top, width, height)`: drawn apart from the picker, so a blink
/// never paints the picker again.
pub(in crate::renderer) fn picker_caret_rect(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> (f32, f32, f32, f32) {
    let panel = picker_panel(ctx, view, surface_w, surface_h);
    let (query_x, text_y) = query_text_origin(ctx, &panel);
    let advance = caret_advance(font_system, ctx, view);
    let (x, y) = picker_origin(view, &panel, (surface_w, surface_h));
    // As the picker's image is placed: its canvas starts a shadow's width
    // before the panel, on a whole pixel.
    let (left, top) = ((x - DROPDOWN_SHADOW).round(), (y - DROPDOWN_SHADOW).round());
    (
        left + (query_x + advance) as f32 + 1.0,
        top + text_y as f32 + 1.0,
        CARET_WIDTH,
        (ctx.line_height - 2.0).max(1.0),
    )
}

/// Where the query's text starts on the picker's canvas: its left edge, and
/// the top of its line.
fn query_text_origin(ctx: &FontCtx, panel: &PickerPanel) -> (i32, i32) {
    let field_x = DROPDOWN_SHADOW + PICKER_FIELD_PAD_X;
    let field_y = DROPDOWN_SHADOW + panel.top_pad + panel.title_h + PICKER_FIELD_PAD_Y;
    let text_dy = ((ctx.field_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let text_pad = (ctx.cell_w * 0.7).round().max(6.0) as i32;
    (field_x as i32 + text_pad, field_y as i32 + text_dy)
}

/// How far after the query's start the caret sits: the width of the
/// characters before it.
fn caret_advance(font_system: &mut FontSystem, ctx: &FontCtx, view: &PickerView) -> i32 {
    let before: String = view.query.chars().take(view.caret).collect();
    let color = Color::rgb(0, 0, 0);
    let buffer = shape_chrome_line(font_system, ctx, &before, color, true, true);
    buffer_width(&buffer).ceil() as i32
}

fn picker_origin(view: &PickerView, panel: &PickerPanel, surface: (f32, f32)) -> (f32, f32) {
    let (surface_w, surface_h) = surface;
    let (panel_w, panel_h) = (panel.panel_w, panel.panel_h);
    let (mut x, mut y) = picker_base_origin(view.placement, (panel_w, panel_h), surface);
    if let Some((dx, dy)) = view.offset {
        x = (x + dx).clamp(0.0, (surface_w - panel_w).max(0.0)).floor();
        y = (y + dy).clamp(0.0, (surface_h - panel_h).max(0.0)).floor();
    }
    (x, y)
}

/// Where a `size` card goes under `placement` before any drag, in a
/// `surface` window: what a drag offset is measured from.
pub fn picker_base_origin(
    placement: PickerPlacement,
    size: (f32, f32),
    surface: (f32, f32),
) -> (f32, f32) {
    let ((panel_w, panel_h), (surface_w, surface_h)) = (size, surface);
    match placement {
        PickerPlacement::WindowCenter => centered(surface_w, surface_h, panel_w, panel_h),
        PickerPlacement::EditorBottom(rect) => {
            bottom_in_rect(surface_w, surface_h, panel_w, panel_h, rect, 0.0)
        }
        PickerPlacement::Pane(rect) => {
            centered_in_rect(surface_w, surface_h, panel_w, panel_h, rect)
        }
        PickerPlacement::Transient((rx, ry, _, _)) => (
            rx.clamp(0.0, (surface_w - panel_w).max(0.0)).floor(),
            ry.clamp(0.0, (surface_h - panel_h).max(0.0)).floor(),
        ),
    }
}

/// The rectangles of the selected entry's buttons, in the card, right to
/// left from the entry's right edge, before its hint.
fn button_rects(
    ctx: &FontCtx,
    item: &PickerItem,
    row: (f32, f32),
    list: (f32, f32),
    hint_w: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let size = (ctx.cell_h * ICON_HEIGHT_RATIO).round().max(1.0);
    let (top, _) = row;
    let (list_x, list_w) = list;
    let mut right = list_x + list_w - ctx.cell_w - PALETTE_ITEM_PAD_X - hint_w - BUTTON_GAP;
    let y = top + (PALETTE_ITEM_PAD_Y * 2.0 + ctx.line_height - size) / 2.0;
    item.buttons
        .iter()
        .map(|_| {
            right -= size;
            let rect = (right, y, size, size);
            right -= BUTTON_GAP;
            rect
        })
        .collect()
}

/// Where the selected entry's buttons are on the surface, for a click to find.
pub(in crate::renderer) fn picker_button_hits(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> Vec<PickerButtonHit> {
    let panel = picker_panel(ctx, view, surface_w, surface_h);
    let layout = picker_layout(ctx, view, &panel);
    let (x, y) = picker_origin(view, &panel, (surface_w, surface_h));
    let Some((item, row)) = view
        .items
        .get(view.selected)
        .zip(layout.rows.get(view.selected))
    else {
        return Vec::new();
    };
    let hint_w = hint_width(font_system, ctx, &item.hint);
    button_rects(ctx, item, *row, (layout.list_x, layout.list_w), hint_w)
        .into_iter()
        .enumerate()
        .map(|(button, (bx, by, bw, bh))| PickerButtonHit {
            button,
            item: view.selected,
            rect: (x + bx, y + by, bw, bh),
        })
        .collect()
}

/// Where the selected entry is on the surface, as the left edge of its text
/// and its top and bottom edges: what the action menu hangs from.
pub(in crate::renderer) fn selected_row_anchor(
    ctx: &FontCtx,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> Option<(f32, f32, f32)> {
    let rows = picker_row_rects(ctx, view, surface_w, surface_h);
    let &(left, top, _, height) = rows.get(view.selected)?;
    Some((left + ctx.cell_w + PALETTE_ITEM_PAD_X, top, top + height))
}

/// Where each entry drawn is on the surface, in the order of `view.items`:
/// what a click on an entry lands on.
pub(in crate::renderer) fn picker_row_rects(
    ctx: &FontCtx,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> Vec<(f32, f32, f32, f32)> {
    let panel = picker_panel(ctx, view, surface_w, surface_h);
    let layout = picker_layout(ctx, view, &panel);
    let (x, y) = picker_origin(view, &panel, (surface_w, surface_h));
    // The layout is in canvas pixels, and the canvas starts a shadow's width
    // before the card.
    let (left, top) = (x - DROPDOWN_SHADOW, y - DROPDOWN_SHADOW);
    layout
        .rows
        .iter()
        .map(|&(row_top, height)| (left + layout.list_x, top + row_top, layout.list_w, height))
        .collect()
}

/// How wide an entry's hint is drawn, with the gap before it.
fn hint_width(font_system: &mut FontSystem, ctx: &FontCtx, hint: &str) -> f32 {
    match hint.is_empty() {
        true => 0.0,
        false => {
            let color = Color::rgb(0, 0, 0);
            buffer_width(&shape_chrome_line(
                font_system,
                ctx,
                hint,
                color,
                false,
                true,
            ))
            .ceil()
                + BUTTON_GAP
        }
    }
}

/// Rasterize the picker: a floating panel with a query line at the top, the
/// filtered entries below, the side panel when it shows, and the key hints
/// at its foot.
pub(in crate::renderer) fn picker_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PickerView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let field_pad_y = PICKER_FIELD_PAD_Y;
    let field_h = ctx.field_h;
    let panel = picker_panel(ctx, view, surface_w, surface_h);
    let layout = picker_layout(ctx, view, &panel);
    let (panel_w, panel_h) = (panel.panel_w, panel.panel_h);
    let (top_pad, title_h, input_h) = (panel.top_pad, panel.title_h, panel.input_h);

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);

    // Hover highlight on the selected entry, filling its row.
    if let Some(&(top, height)) = layout.rows.get(view.selected) {
        let hover_v_inset = 1.0;
        let hover_rect = (
            layout.list_x + MENU_HOVER_INSET,
            top + hover_v_inset,
            layout.list_w - 2.0 * MENU_HOVER_INSET,
            height - 2.0 * hover_v_inset,
        );
        frame.fill(hover_rect, 2.0, theme.menu_hover_bg, 1.0);
    }

    // The title bar, above the input line: what the list is of, which the key
    // that opened it is too far back to say by the time it is read.
    if !view.title.is_empty() {
        frame.draw_title(
            font_system,
            swash_cache,
            ctx,
            theme,
            TitleBar::Decorated,
            &view.title,
            &view.title_count,
        );
    }

    // A simple 1px border around the input field well.
    let field_x = margin + PICKER_FIELD_PAD_X;
    let field_y = margin + top_pad + title_h + field_pad_y;
    let field_w = panel_w - 2.0 * PICKER_FIELD_PAD_X;
    let well = mix_rgb(theme.menu_bg, theme.background, 0.6);
    frame.fill(
        (field_x, field_y, field_w, field_h),
        PICKER_FIELD_RADIUS,
        theme.active_indicator,
        1.0,
    );
    frame.fill(
        (field_x + 1.0, field_y + 1.0, field_w - 2.0, field_h - 2.0),
        (PICKER_FIELD_RADIUS - 1.0).max(0.0),
        well,
        1.0,
    );

    // Hairline divider separating the input area from the results list below.
    let divider_y = margin + top_pad + title_h + input_h;
    draw_divider(&mut frame, theme, divider_y, panel_w);

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let match_color = theme.ansi[4].to_glyphon();
    let pad_x = (ctx.cell_w * 1.0) as i32;
    let item_text_dy = ((panel.item_h - ctx.line_height) / 2.0).max(0.0) as i32;

    // Query text, inside the bordered field; before anything is typed, what
    // to type, dimmed, cut to the field.
    let (query_x, input_text_y) = query_text_origin(ctx, &panel);
    let (mut query_buf, color) = match view.query.is_empty() {
        true => {
            let text_pad = query_x as f32 - field_x;
            let room = (field_w - 2.0 * text_pad).max(0.0);
            let shaped = shape_fitted(font_system, ctx, &view.placeholder, muted, true, true, room);
            (shaped, muted)
        }
        false => (
            shape_chrome_line(font_system, ctx, &view.query, foreground, true, true),
            foreground,
        ),
    };
    frame.draw_text(
        font_system,
        swash_cache,
        &mut query_buf,
        (query_x, input_text_y),
        color,
    );

    // Caret: a thin bar after the characters before it, placed by shaping
    // them alone when it is inside the query.
    if view.caret_visible {
        let caret_x = (query_x + caret_advance(font_system, ctx, view)) as f32 + 1.0;
        let caret_y = input_text_y as f32 + 1.0;
        draw_caret(&mut frame, ctx, theme, (caret_x, caret_y));
    }

    // Icons get a column of their own when any entry has one, so every label
    // starts on one line whether it has an icon or not.
    let icon_px = (ctx.cell_h * ICON_HEIGHT_RATIO).round().max(1.0) as u32;
    let label_indent = match view.items.iter().any(|item| item.icon.is_some()) {
        true => icon_px as i32 + (ctx.cell_w * ICON_GAP_CELLS).round() as i32,
        false => 0,
    };
    let underline = view.match_underline;

    if layout.rows.is_empty() {
        let mut no_match =
            shape_chrome_line(font_system, ctx, &view.empty_message, muted, false, true);
        frame.draw_text(
            font_system,
            swash_cache,
            &mut no_match,
            (
                layout.list_x as i32 + pad_x,
                layout.results_top as i32 + item_text_dy,
            ),
            muted,
        );
    }
    let item_pad = PALETTE_ITEM_PAD_X as i32;
    for (at, (item, &(top, _))) in view.items.iter().zip(&layout.rows).enumerate() {
        let row_y = top as i32 + item_text_dy;
        let icon_x = layout.list_x as i32 + pad_x + item_pad;
        let label_x = icon_x + label_indent;
        if let Some(svg) = &item.icon {
            let icon_y = (top + (panel.item_h - icon_px as f32) / 2.0).round() as i32;
            let (rgba, canvas) = frame.pixels();
            blit_svg(rgba, canvas, svg, (icon_x, icon_y), icon_px);
        }
        let label_w = draw_run(
            font_system,
            swash_cache,
            ctx,
            &mut frame,
            Run {
                accent: match_color,
                base: foreground,
                matches: &item.match_positions,
                text: &item.label,
                underline,
            },
            (label_x, row_y),
        );
        if !item.description.is_empty() {
            let gap = (ctx.cell_w * 1.0).round() as i32;
            draw_run(
                font_system,
                swash_cache,
                ctx,
                &mut frame,
                Run {
                    accent: match_color,
                    base: muted,
                    matches: &item.description_matches,
                    text: &item.description,
                    underline,
                },
                (label_x + label_w + gap, row_y),
            );
        }
        if !item.detail.is_empty() {
            draw_run(
                font_system,
                swash_cache,
                ctx,
                &mut frame,
                Run {
                    accent: match_color,
                    base: foreground,
                    matches: &item.detail_matches,
                    text: &item.detail,
                    underline,
                },
                (label_x, row_y + ctx.line_height as i32),
            );
        }
        let right = (layout.list_x + layout.list_w) as i32 - pad_x - item_pad;
        let hint_w = hint_width(font_system, ctx, &item.hint);
        if !item.hint.is_empty() {
            let mut hint_buf = shape_chrome_line(font_system, ctx, &item.hint, muted, false, true);
            let width = buffer_width(&hint_buf).ceil() as i32;
            frame.draw_text(
                font_system,
                swash_cache,
                &mut hint_buf,
                (right - width, row_y),
                muted,
            );
        }
        if at == view.selected {
            let list = (layout.list_x, layout.list_w);
            let rects = button_rects(ctx, item, (top, 0.0), list, hint_w);
            for (svg, (bx, by, size, _)) in item.buttons.iter().zip(rects) {
                let (rgba, canvas) = frame.pixels();
                blit_svg(rgba, canvas, svg, (bx as i32, by as i32), size as u32);
            }
        }
    }

    if let (Some(preview), Some(rect)) = (&view.panel, layout.panel) {
        draw_panel(
            font_system,
            swash_cache,
            ctx,
            theme,
            &mut frame,
            preview,
            rect,
        );
    }

    if !view.hints.is_empty() {
        let hints_y = margin + panel_h - panel.hints_h;
        let mut hints = shape_chrome_line(font_system, ctx, &view.hints, muted, false, true);
        frame.draw_text(
            font_system,
            swash_cache,
            &mut hints,
            (margin as i32 + pad_x, hints_y as i32 + item_text_dy),
            muted,
        );
    }

    let (x, y) = picker_origin(view, &panel, (surface_w, surface_h));
    frame.into_image(x, y)
}

/// A run of text drawn with its matched letters lit.
struct Run<'a> {
    accent: Color,
    base: Color,
    matches: &'a [usize],
    text: &'a str,
    underline: bool,
}

/// Draw `run` at `at`; how wide it is drawn.
fn draw_run(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    frame: &mut Chrome,
    run: Run,
    at: (i32, i32),
) -> i32 {
    let mut buffer = shape_chrome_line(font_system, ctx, run.text, run.base, true, true);
    let width = buffer_width(&buffer).ceil() as i32;
    match run.matches.is_empty() {
        true => frame.draw_text(font_system, swash_cache, &mut buffer, at, run.base),
        false => {
            let (rgba, canvas) = frame.pixels();
            draw_highlighted_label(
                font_system,
                swash_cache,
                ctx,
                rgba,
                canvas,
                run.text,
                run.matches,
                run.base,
                run.accent,
                run.underline,
                true,
                at,
            );
        }
    }
    width
}

/// Draw the side panel's lines in `rect`: the focused line tinted, the
/// query's marks behind their letters, each run in its colour.
fn draw_panel(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    frame: &mut Chrome,
    preview: &PanelView,
    rect: (f32, f32, f32, f32),
) {
    let (x, y, w, h) = rect;
    frame.fill(
        (x, y, w, h),
        0.0,
        mix_rgb(theme.menu_bg, theme.background, 0.5),
        1.0,
    );
    let pad = ctx.cell_w;
    let shown = ((h / ctx.line_height).floor() as usize).max(1);
    let first = preview
        .focus
        .map_or(0, |focus| focus.saturating_sub(shown / 2))
        .min(preview.lines.len().saturating_sub(shown));
    for (n, line) in preview.lines.iter().enumerate().skip(first).take(shown) {
        let line_y = y + (n - first) as f32 * ctx.line_height;
        if preview.focus == Some(n) {
            let tint = mix_rgb(theme.menu_bg, theme.ansi[4], PANEL_FOCUS_TINT);
            frame.fill((x, line_y, w, ctx.line_height), 0.0, tint, 1.0);
        }
        let text: String = line.spans.iter().map(|span| span.text.as_str()).collect();
        for mark in &line.marks {
            let before: String = text.chars().take(mark.start).collect();
            let marked: String = text.chars().skip(mark.start).take(mark.len()).collect();
            let color = theme.foreground.to_glyphon();
            let from = buffer_width(&shape_chrome_line(
                font_system,
                ctx,
                &before,
                color,
                false,
                false,
            ));
            let width = buffer_width(&shape_chrome_line(
                font_system,
                ctx,
                &marked,
                color,
                false,
                false,
            ));
            let tint = mix_rgb(theme.menu_bg, theme.ansi[4], PANEL_MARK_TINT);
            frame.fill(
                (x + pad + from, line_y, width, ctx.line_height),
                2.0,
                tint,
                1.0,
            );
        }
        let mut span_x = x + pad;
        for span in &line.spans {
            let color = span.color.to_glyphon();
            let mut buffer = shape_chrome_line(font_system, ctx, &span.text, color, false, false);
            let width = buffer_width(&buffer);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut buffer,
                (span_x as i32, line_y as i32),
                color,
            );
            span_x += width;
            if span_x > x + w {
                break;
            }
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_the_picker_is_one_size_however_much_the_query_matches() {
        // Typing narrows the list, and a panel sized to what is left would
        // shrink under every keystroke, moving the rows out from under the
        // eye reading them.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let mut card = |matches: usize| {
            let view = PickerView {
                anchor: None,
                caret: 0,
                caret_visible: true,
                empty_message: "Nothing matches".to_string(),
                items: (0..matches)
                    .map(|i| crate::renderer::widgets::PickerItem {
                        action: i.to_string(),
                        hint: String::new(),
                        icon: None,
                        label: format!("entry {i}"),
                        match_positions: Vec::new(),
                        ..Default::default()
                    })
                    .collect(),
                match_underline: false,
                query: String::new(),
                selected: 0,
                title: String::new(),
                offset: None,
                size: None,
                placement: PickerPlacement::default(),
                ..Default::default()
            };
            let image = picker_rgba(
                &mut font_system,
                &mut swash,
                &ctx,
                &theme,
                &view,
                surface_w,
                surface_h,
            );
            (image.width, image.height, image.y)
        };

        let full = card(40);
        assert_eq!(card(3), full, "three matches draw the box a full list does");
        assert_eq!(card(0), full, "and so does none");
        assert!(
            (full.0 as f32 - (surface_w * PALETTE_WIDTH_RATIO + 2.0 * DROPDOWN_SHADOW)).abs() < 2.0,
            "and it is sized against the window, got {full:?}"
        );
    }

    #[test]
    fn test_window_wise_picker_is_centered_in_window() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let view = PickerView {
            anchor: None,
            caret: 0,
            caret_visible: true,
            empty_message: "No targets".to_string(),
            items: (0..20)
                .map(|i| crate::renderer::widgets::PickerItem {
                    action: i.to_string(),
                    hint: String::new(),
                    icon: None,
                    label: format!("line {i}"),
                    match_positions: Vec::new(),
                    ..Default::default()
                })
                .collect(),
            match_underline: false,
            query: String::new(),
            selected: 0,
            title: "Commands".to_string(),
            offset: None,
            size: None,
            placement: PickerPlacement::WindowCenter,
            ..Default::default()
        };
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        let expected_panel_w = (surface_w * PALETTE_WIDTH_RATIO).floor();
        let expected_panel_h = (surface_h * PALETTE_HEIGHT_RATIO).floor();
        let expected_total_w = (expected_panel_w + 2.0 * DROPDOWN_SHADOW) as u32;
        let expected_total_h = (expected_panel_h + 2.0 * DROPDOWN_SHADOW) as u32;
        assert_eq!(image.width, expected_total_w);
        assert_eq!(image.height, expected_total_h);
        let expected_x = ((surface_w - expected_panel_w) / 2.0).floor() - DROPDOWN_SHADOW;
        let expected_y = ((surface_h - expected_panel_h) / 2.0).floor() - DROPDOWN_SHADOW;
        assert_eq!(image.x, expected_x);
        assert_eq!(image.y, expected_y);
    }

    #[test]
    fn test_editor_wise_picker_is_anchored_at_bottom_of_editor() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let editor_rect = (100.0, 50.0, 800.0, 700.0);
        let view = sample_picker("Jump: Buffer", true)
            .with_placement(PickerPlacement::EditorBottom(editor_rect));
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        let expected_panel_w = 800.0_f32;
        let expected_panel_h = (700.0_f32 * 0.40_f32).floor();
        let expected_total_w = (expected_panel_w + 2.0 * DROPDOWN_SHADOW) as u32;
        let expected_total_h = (expected_panel_h + 2.0 * DROPDOWN_SHADOW) as u32;
        assert_eq!(image.width, expected_total_w);
        assert_eq!(image.height, expected_total_h);
        let expected_x = 100.0 - DROPDOWN_SHADOW;
        let expected_y = 50.0 + 700.0 - expected_panel_h - DROPDOWN_SHADOW;
        assert_eq!(image.x, expected_x);
        assert_eq!(image.y, expected_y);
    }

    #[test]
    fn test_the_placeholder_shows_only_while_nothing_is_typed() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let mut draw = |query: &str, placeholder: &str| {
            let view = PickerView {
                caret: 0,
                caret_visible: false,
                placeholder: placeholder.to_string(),
                query: query.to_string(),
                ..sample_picker("Checkout", false)
            };
            picker_rgba(
                &mut font_system,
                &mut swash,
                &ctx,
                &Theme::dark(),
                &view,
                1000.0,
                800.0,
            )
            .rgba
        };
        assert_ne!(
            draw("", "Branch name"),
            draw("", ""),
            "an empty query line says what to type"
        );
        assert_eq!(
            draw("ma", "Branch name"),
            draw("ma", ""),
            "and says nothing once something is typed"
        );
    }

    #[test]
    fn test_row_hits_start_at_the_drawn_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let view = sample_picker("Checkout", true);
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &view,
            surface_w,
            surface_h,
        );
        let rows = picker_row_rects(&ctx, &view, surface_w, surface_h);
        assert_eq!(rows.len(), 3, "one per entry");
        let (card_x, card_y) = (image.x + DROPDOWN_SHADOW, image.y + DROPDOWN_SHADOW);
        assert_eq!(rows[0].0, card_x, "flush with the card, not a shadow in");
        assert!(rows[0].1 > card_y && rows[1].1 > rows[0].1);
    }

    #[test]
    fn test_a_card_offset_from_its_base_lands_where_asked() {
        // What a resize stores: the offset from where a card that size is
        // placed, so the card is drawn where the pointer left it.
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let surface = (1000.0, 800.0);
        let (size, want) = ((500.0, 400.0), (120.0, 90.0));
        let base = picker_base_origin(PickerPlacement::WindowCenter, size, surface);
        let view = PickerView {
            offset: Some((want.0 - base.0, want.1 - base.1)),
            size: Some(size),
            ..sample_picker("Checkout", true)
        };
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &view,
            surface.0,
            surface.1,
        );
        assert_eq!((image.x + DROPDOWN_SHADOW, image.y + DROPDOWN_SHADOW), want);
    }

    #[test]
    fn test_transient_picker_takes_the_transient_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let card = (260.0, 180.0, 480.0, 500.0);
        let view = sample_picker("Checkout", true).with_placement(PickerPlacement::Transient(card));
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        assert_eq!(image.width, (480.0 + 2.0 * DROPDOWN_SHADOW) as u32);
        assert_eq!(image.height, (500.0 + 2.0 * DROPDOWN_SHADOW) as u32);
        assert_eq!(image.x, 260.0 - DROPDOWN_SHADOW);
        assert_eq!(image.y, 180.0 - DROPDOWN_SHADOW);
    }

    #[test]
    fn test_a_list_an_editor_asks_for_is_its_card_in_its_pane() {
        // The same card the editor's menus are drawn on, so a list asked
        // with or without a menu first is one size.
        let ctx = sample_font_ctx();
        let pane = (500.0, 100.0, 500.0, 700.0);
        let view = sample_picker("Checkout", true).with_placement(PickerPlacement::Pane(pane));
        let panel = picker_panel(&ctx, &view, 1000.0, 800.0);
        let card = editor_card_size(&ctx, (500.0, 700.0));
        assert_eq!((panel.panel_w, panel.panel_h), card);
        let (x, y) = picker_origin(&view, &panel, (1000.0, 800.0));
        assert_eq!(x + card.0 / 2.0, 750.0, "centred across the pane");
        assert_eq!(y + card.1 / 2.0, 450.0, "and down it");
    }

    #[test]
    fn test_transient_picker_stays_on_the_surface() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let card = (700.0, 600.0, 480.0, 500.0);
        let view = sample_picker("Checkout", true).with_placement(PickerPlacement::Transient(card));
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        assert_eq!(image.x, surface_w - 480.0 - DROPDOWN_SHADOW);
        assert_eq!(image.y, surface_h - 500.0 - DROPDOWN_SHADOW);
    }

    #[test]
    fn test_palette_custom_size() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let mut view = sample_picker("Resize", true);
        view.size = Some((600.0, 450.0));
        let image = picker_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );
        let expected_total_w = (600.0 + 2.0 * DROPDOWN_SHADOW) as u32;
        let expected_total_h = (450.0 + 2.0 * DROPDOWN_SHADOW) as u32;
        assert_eq!(image.width, expected_total_w);
        assert_eq!(image.height, expected_total_h);
    }

    /// A picker with a few rows, titled `title`, its caret shown or not.
    fn sample_picker(title: &str, caret_visible: bool) -> PickerView {
        PickerView {
            anchor: None,
            caret: 0,
            caret_visible,
            empty_message: String::new(),
            items: (0..3)
                .map(|i| crate::renderer::widgets::PickerItem {
                    action: i.to_string(),
                    hint: String::new(),
                    icon: None,
                    label: format!("entry {i}"),
                    match_positions: Vec::new(),
                    ..Default::default()
                })
                .collect(),
            match_underline: false,
            query: "ent".to_string(),
            selected: 0,
            title: title.to_string(),
            offset: None,
            size: None,
            placement: PickerPlacement::default(),
            ..Default::default()
        }
    }

    fn item(label: &str) -> PickerItem {
        PickerItem {
            action: label.to_string(),
            label: label.to_string(),
            ..Default::default()
        }
    }

    #[test]
    fn test_an_entry_with_a_detail_takes_its_second_line_under_the_label() {
        let ctx = sample_font_ctx();
        let mut view = sample_picker("Search", true);
        view.items = vec![
            PickerItem {
                detail: "let x = 1;".to_string(),
                ..item("a.rs:3:5")
            },
            item("b.rs"),
        ];
        let panel = picker_panel(&ctx, &view, 1000.0, 800.0);
        let layout = picker_layout(&ctx, &view, &panel);
        let (first_top, first_h) = layout.rows[0];
        assert_eq!(first_h, panel.item_h + ctx.line_height);
        assert_eq!(
            layout.rows[1].0,
            first_top + first_h,
            "the next entry starts under both lines"
        );
    }

    #[test]
    fn test_the_key_hints_take_a_line_from_the_entries() {
        let ctx = sample_font_ctx();
        let bare = sample_picker("Files", true);
        let hinted = PickerView {
            hints: "F4 actions".to_string(),
            ..bare.clone()
        };
        let rows = |view: &PickerView| picker_panel(&ctx, view, 1000.0, 800.0).rows;
        assert_eq!(rows(&hinted), rows(&bare) - 1);
    }

    #[test]
    fn test_the_side_panel_sits_beside_a_wide_list_and_under_a_narrow_one() {
        let ctx = sample_font_ctx();
        let view = PickerView {
            panel: Some(PanelView::default()),
            ..sample_picker("Files", true)
        };
        let wide = picker_panel(&ctx, &view, 2000.0, 800.0);
        let (x, y, _, _) = picker_layout(&ctx, &view, &wide).panel.unwrap();
        assert!(x > DROPDOWN_SHADOW, "beside: it starts right of the list");
        assert_eq!(y, picker_layout(&ctx, &view, &wide).results_top);
        let narrow = picker_panel(&ctx, &view, 400.0, 800.0);
        let (x, y, _, _) = picker_layout(&ctx, &view, &narrow).panel.unwrap();
        assert_eq!(x, DROPDOWN_SHADOW, "under: it spans the card");
        assert!(y > picker_layout(&ctx, &view, &narrow).results_top);
    }

    #[test]
    fn test_only_the_selected_entrys_buttons_are_hit_areas_inside_the_card() {
        let mut font_system = FontSystem::new();
        let ctx = sample_font_ctx();
        let icon: std::sync::Arc<[u8]> = std::sync::Arc::from(&b"<svg/>"[..]);
        let mut view = sample_picker("Buffers", true);
        view.items[1].buttons = vec![icon.clone()];
        view.items[0].buttons = vec![icon.clone(), icon];
        view.selected = 0;
        let hits = picker_button_hits(&mut font_system, &ctx, &view, 1000.0, 800.0);
        assert_eq!(hits.len(), 2);
        assert!(hits.iter().all(|hit| hit.item == 0));
        let image = render_picker(&view);
        let (x, _, w, _) = hits[0].rect;
        assert!(x > image.x && x + w < image.x + image.width as f32);
        assert!(hits[1].rect.0 < hits[0].rect.0, "buttons run right to left");
    }

    #[test]
    fn test_the_action_menu_hangs_from_the_selected_entry() {
        let ctx = sample_font_ctx();
        let mut view = sample_picker("Files", true);
        let first = selected_row_anchor(&ctx, &view, 1000.0, 800.0).expect("a row");
        view.selected = 2;
        let third = selected_row_anchor(&ctx, &view, 1000.0, 800.0).expect("a row");
        assert!(third.1 > first.1, "lower for a lower entry");
        assert!(third.2 > third.1, "the bottom under the top");
        view.selected = 99;
        assert!(selected_row_anchor(&ctx, &view, 1000.0, 800.0).is_none());
    }

    fn render_picker(view: &PickerView) -> DropdownImage {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        picker_rgba(
            &mut font_system,
            &mut swash,
            &sample_font_ctx(),
            &Theme::dark(),
            view,
            1000.0,
            800.0,
        )
    }

    #[test]
    fn test_the_palette_caret_is_drawn_only_in_its_shown_phase() {
        let shown = render_picker(&sample_picker("", true));
        let hidden = render_picker(&sample_picker("", false));
        assert_eq!((shown.width, shown.height), (hidden.width, hidden.height));
        assert_ne!(shown.rgba, hidden.rgba, "the caret blinks off");
    }

    #[test]
    fn test_the_caret_drawn_apart_covers_where_the_picker_drew_it() {
        let shown = render_picker(&sample_picker("Files", true));
        let hidden = render_picker(&sample_picker("Files", false));
        let (left, top, width, height) = picker_caret_rect(
            &mut FontSystem::new(),
            &sample_font_ctx(),
            &sample_picker("Files", true),
            1000.0,
            800.0,
        );
        // Every pixel the caret changed, in surface pixels, lies in the rect
        // the caret is drawn apart at.
        let changed: Vec<(f32, f32)> = (0..shown.height)
            .flat_map(|y| (0..shown.width).map(move |x| (x, y)))
            .filter(|&(x, y)| {
                let i = ((y * shown.width + x) * 4) as usize;
                shown.rgba[i..i + 4] != hidden.rgba[i..i + 4]
            })
            .map(|(x, y)| (shown.x + x as f32, shown.y + y as f32))
            .collect();
        assert!(!changed.is_empty(), "the caret was drawn");
        assert!(
            changed.iter().all(|&(x, y)| {
                x >= left - 1.0
                    && x < left + width + 1.0
                    && y >= top - 1.0
                    && y < top + height + 1.0
            }),
            "the caret at {:?} lies in {:?}",
            changed.first(),
            (left, top, width, height)
        );
    }

    #[test]
    fn test_a_titled_palette_draws_a_divider_between_its_title_and_its_input() {
        let ctx = sample_font_ctx();
        let image = render_picker(&sample_picker("Files", true));
        let y = (DROPDOWN_SHADOW + TITLE_BAR_TOP_PAD + title_bar_height(&ctx)).round() as u32;
        // Sampled near the right edge, clear of the title and the input text.
        let x = image.width - DROPDOWN_SHADOW as u32 - 4;
        let pixel = |y: u32| {
            let i = ((y * image.width + x) * 4) as usize;
            image.rgba[i..i + 3].to_vec()
        };
        assert_ne!(pixel(y), pixel(y + 3), "a hairline at the title's foot");
    }

    #[test]
    fn test_picker_drag_offset_and_clamping() {
        let base_view = sample_picker("Files", true);
        let base_image = render_picker(&base_view);

        // Positive delta (moves down and right)
        let mut moved_view = base_view.clone();
        moved_view.offset = Some((50.0, 30.0));
        let moved_image = render_picker(&moved_view);
        assert_eq!(moved_image.x, base_image.x + 50.0);
        assert_eq!(moved_image.y, base_image.y + 30.0);

        // Excessive negative delta clamps to 0
        let mut clamped_view = base_view;
        clamped_view.offset = Some((-5000.0, -5000.0));
        let clamped_image = render_picker(&clamped_view);
        assert_eq!(clamped_image.x, -DROPDOWN_SHADOW);
        assert_eq!(clamped_image.y, -DROPDOWN_SHADOW);
    }
}

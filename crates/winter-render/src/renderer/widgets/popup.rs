//! The popup of what can be done at the cursor, after VS Code's action list:
//! a list floating there (the fixes and refactorings, how a word might be
//! spelled), hung under the cursor's cell or over it where the window has no
//! room below. No title bar: what the list is of heads it as a quiet label,
//! and each row is its icon, a lightbulb for a fix, then what it does.
//! Typing filters it, and what is typed shows on a line under the label
//! only once something is, so a list not yet typed into is its rows.

use glyphon::{FontSystem, SwashCache};

use super::completion::shown_rows;
use super::view::{PickerItem, PickerView};
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::chrome::{anchored, draw_caret, draw_divider, shape_fitted, Chrome};
use crate::renderer::raster::paint::{
    blit_svg, buffer_width, draw_highlighted_label, shape_chrome_line,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::{DROPDOWN_SHADOW, ICON_GAP_CELLS, ICON_HEIGHT_RATIO};
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// The room above and below a row's text, and at the card's sides, in
/// pixels.
const ROW_PAD_Y: f32 = 4.0;
const PAD_X: f32 = 8.0;

/// The room above and below the label heading the list, in pixels.
const HEADER_PAD_Y: f32 = 3.0;

/// How far the band over the chosen row stands in from the card's sides,
/// and how round its corners are, in pixels.
const BAND_INSET: f32 = 4.0;
const BAND_RADIUS: f32 = 3.0;

/// The narrowest the card is, in cells, so a short list is still a target.
const MIN_WIDTH_CELLS: f32 = 24.0;

/// The widest the card grows, as a share of the window.
const MAX_WIDTH_SHARE: f32 = 0.6;

/// The border above the first row and below the last, in pixels.
const EDGE: f32 = 1.0;

// ========================================================================
// Data Structures
// ========================================================================

/// Where the parts of the card go, in canvas pixels.
struct Layout {
    /// How tall the label heading the list is; 0 without one.
    header_h: f32,
    /// How large a row's icon is, and how far its label stands from the
    /// row's start: past the icon where any row has one.
    icon_px: u32,
    label_indent: f32,
    /// How tall the line of what is typed is, its divider included; 0
    /// before anything is typed.
    query_h: f32,
    /// How tall each row is.
    row_h: f32,
    /// Where the text starts across, and how wide it may run.
    room: f32,
    text_x: f32,
}

// ========================================================================
// Free functions
// ========================================================================

/// Rasterize the list `view` describes, hung from `anchor` (the cursor's
/// cell as its left edge, top edge, and bottom edge in surface pixels)
/// inside a `surface_w` by `surface_h` window.
#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn popup_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PickerView,
    anchor: (f32, f32, f32),
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let (first, shown) = match view.items.is_empty() {
        true => (0, 0),
        false => shown_rows(view.items.len(), view.selected),
    };
    let icon_px = (ctx.cell_h * ICON_HEIGHT_RATIO).round().max(1.0) as u32;
    let label_indent = match view.items.iter().any(|item| item.icon.is_some()) {
        true => icon_px as f32 + (ctx.cell_w * ICON_GAP_CELLS).round(),
        false => 0.0,
    };
    let foreground = theme.foreground.to_glyphon();
    let mut width_of = |text: &str| {
        buffer_width(&shape_chrome_line(
            font_system,
            ctx,
            text,
            foreground,
            false,
            true,
        ))
        .ceil()
    };
    let others = [
        view.title.as_str(),
        view.query.as_str(),
        view.empty_message.as_str(),
    ]
    .map(&mut width_of);
    let widest = view
        .items
        .iter()
        .map(|item| width_of(&item.label) + label_indent)
        .chain(others)
        .fold(0.0, f32::max);
    let panel_w = (widest + 2.0 * PAD_X)
        .max(MIN_WIDTH_CELLS * ctx.cell_w)
        .min(surface_w * MAX_WIDTH_SHARE)
        .ceil();
    let row_h = (ctx.line_height + 2.0 * ROW_PAD_Y).round();
    let layout = Layout {
        header_h: match view.title.is_empty() {
            true => 0.0,
            false => (ctx.line_height + 2.0 * HEADER_PAD_Y).round(),
        },
        icon_px,
        label_indent,
        query_h: match view.query.is_empty() {
            true => 0.0,
            false => row_h + 1.0,
        },
        row_h,
        room: panel_w - 2.0 * PAD_X,
        text_x: margin + PAD_X,
    };
    let panel_h = layout.header_h + layout.query_h + row_h * shown.max(1) as f32 + 2.0 * EDGE;
    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);

    let mut top = margin + EDGE;
    if layout.header_h > 0.0 {
        draw_header(
            &mut frame,
            font_system,
            swash_cache,
            ctx,
            theme,
            &view.title,
            &layout,
            top,
        );
        top += layout.header_h;
    }
    if layout.query_h > 0.0 {
        draw_query(
            &mut frame,
            font_system,
            swash_cache,
            ctx,
            theme,
            view,
            &layout,
            top,
            panel_w,
        );
        top += layout.query_h;
    }
    if shown == 0 {
        let muted = theme.ansi[8].to_glyphon();
        let mut empty = shape_fitted(
            font_system,
            ctx,
            &view.empty_message,
            muted,
            false,
            true,
            layout.room,
        );
        let at = (layout.text_x as i32, (top + ROW_PAD_Y).round() as i32);
        frame.draw_text(font_system, swash_cache, &mut empty, at, muted);
    }
    for (at, item) in view.items[first..first + shown].iter().enumerate() {
        let row_top = top + row_h * at as f32;
        if first + at == view.selected {
            let band = (
                margin + BAND_INSET,
                row_top,
                panel_w - 2.0 * BAND_INSET,
                row_h,
            );
            frame.fill(band, BAND_RADIUS, theme.menu_hover_bg, 1.0);
        }
        draw_row(
            &mut frame,
            font_system,
            swash_cache,
            ctx,
            theme,
            view,
            item,
            &layout,
            row_top,
        );
    }

    let (x, y) = anchored(anchor, (panel_w, panel_h), margin, (surface_w, surface_h));
    frame.into_image(x, y)
}

/// The quiet label heading the list, what it is of, with its top at `top`.
#[allow(clippy::too_many_arguments)]
fn draw_header(
    frame: &mut Chrome,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    title: &str,
    layout: &Layout,
    top: f32,
) {
    let muted = theme.ansi[8].to_glyphon();
    let mut label = shape_fitted(font_system, ctx, title, muted, true, true, layout.room);
    let at = (layout.text_x as i32, (top + HEADER_PAD_Y).round() as i32);
    frame.draw_text(font_system, swash_cache, &mut label, at, muted);
}

/// What is typed into the list and its caret, over a hairline, with its
/// top at `top`.
#[allow(clippy::too_many_arguments)]
fn draw_query(
    frame: &mut Chrome,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PickerView,
    layout: &Layout,
    top: f32,
    panel_w: f32,
) {
    let foreground = theme.foreground.to_glyphon();
    let text_y = top + ROW_PAD_Y;
    let mut query = shape_fitted(
        font_system,
        ctx,
        &view.query,
        foreground,
        false,
        true,
        layout.room,
    );
    frame.draw_text(
        font_system,
        swash_cache,
        &mut query,
        (layout.text_x as i32, text_y as i32),
        foreground,
    );
    if view.caret_visible {
        let before: String = view.query.chars().take(view.caret).collect();
        let before = shape_chrome_line(font_system, ctx, &before, foreground, false, true);
        let caret_x = layout.text_x + buffer_width(&before).ceil() + 1.0;
        draw_caret(frame, ctx, theme, (caret_x, text_y + 1.0));
    }
    draw_divider(frame, theme, top + layout.query_h - 1.0, panel_w);
}

/// One row with its top at `top`: its icon, then what it does, the letters
/// typed lit where something is typed.
#[allow(clippy::too_many_arguments)]
fn draw_row(
    frame: &mut Chrome,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PickerView,
    item: &PickerItem,
    layout: &Layout,
    top: f32,
) {
    let foreground = theme.foreground.to_glyphon();
    if let Some(svg) = &item.icon {
        let icon_y = (top + (layout.row_h - layout.icon_px as f32) / 2.0).round() as i32;
        let (rgba, canvas) = frame.pixels();
        blit_svg(
            rgba,
            canvas,
            svg,
            (layout.text_x as i32, icon_y),
            layout.icon_px,
        );
    }
    let label_x = (layout.text_x + layout.label_indent) as i32;
    let text_y = (top + ROW_PAD_Y).round() as i32;
    let room = (layout.room - layout.label_indent).max(0.0);
    match !view.query.is_empty() && !item.match_positions.is_empty() {
        true => {
            let match_color = theme.ansi[4].to_glyphon();
            let (rgba, canvas) = frame.pixels();
            draw_highlighted_label(
                font_system,
                swash_cache,
                ctx,
                rgba,
                canvas,
                &item.label,
                &item.match_positions,
                foreground,
                match_color,
                view.match_underline,
                false,
                (label_x, text_y),
            );
        }
        false => {
            let mut label =
                shape_fitted(font_system, ctx, &item.label, foreground, false, true, room);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut label,
                (label_x, text_y),
                foreground,
            );
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
    use crate::renderer::widgets::view::PickerItem;

    /// The list of `labels`, `query` typed into it.
    fn list(labels: &[&str], query: &str) -> PickerView {
        PickerView {
            anchor: None,
            caret: query.chars().count(),
            caret_visible: true,
            empty_message: "Nothing matches".to_string(),
            items: labels
                .iter()
                .map(|label| PickerItem {
                    action: label.to_string(),
                    icon: None,
                    label: label.to_string(),
                    match_positions: Vec::new(),
                    shortcut: String::new(),
                })
                .collect(),
            match_underline: false,
            query: query.to_string(),
            selected: 0,
            title: "Quick Fix".to_string(),
        }
    }

    /// `view` drawn hung from a cell at (100, 200)..(100, 220).
    fn drawn(view: &PickerView) -> DropdownImage {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let anchor = (100.0, 200.0, 220.0);
        popup_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            view,
            anchor,
            1000.0,
            800.0,
        )
    }

    #[test]
    fn test_the_list_hangs_from_the_cursor_not_the_window_centre() {
        let image = drawn(&list(&["Add import", "Spell as word"], ""));
        let panel = (image.x + DROPDOWN_SHADOW, image.y + DROPDOWN_SHADOW);
        assert_eq!(panel.0, 100.0, "starts at the cursor's column");
        assert!(
            panel.1 >= 220.0 && panel.1 < 230.0,
            "just under the cursor, at {}",
            panel.1
        );
    }

    #[test]
    fn test_what_is_typed_takes_a_line_only_once_something_is() {
        let labels = ["Add import", "Spell as word"];
        let untouched = drawn(&list(&labels, ""));
        let typed = drawn(&list(&labels, "imp"));
        assert!(
            typed.height > untouched.height,
            "the typed line is added over the rows"
        );
    }

    #[test]
    fn test_the_label_heading_the_list_takes_a_line_only_when_there_is_one() {
        let labels = ["Add import", "Spell as word"];
        let headed = drawn(&list(&labels, ""));
        let mut bare = list(&labels, "");
        bare.title.clear();
        assert!(
            drawn(&bare).height < headed.height,
            "no label, no line for it"
        );
    }

    #[test]
    fn test_rows_with_icons_leave_room_for_them() {
        let label = "Change to \"reaper\" and more besides, a long one";
        let plain = drawn(&list(&[label], ""));
        let mut iconed = list(&[label], "");
        iconed.items[0].icon = Some(std::sync::Arc::from(&b"<svg/>"[..]));
        assert!(
            drawn(&iconed).width > plain.width,
            "the label stands past the icon"
        );
    }
}

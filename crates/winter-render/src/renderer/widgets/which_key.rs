//! The key-hint card shown when a key that starts a sequence is pressed and
//! the rest is waited for: what each next key does, in columns.

use super::view::WhichKeyView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{buffer_width, shape_chrome_line, truncate_label};
use crate::renderer::raster::popup::{centered, PopupFrame};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::Theme;
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Constants
// ========================================================================

/// The font size, in pixels, magic-vscode's key-hint popup is laid out at.
/// Every `HINT_*` length below is in those pixels, scaled to Winter's own
/// font size when drawn, so the popup keeps its proportions at any zoom.
const HINT_BASE_FONT_PX: f32 = 13.0;
/// The panel's share of the window's width and height.
const HINT_WIDTH_RATIO: f32 = 0.80;
const HINT_HEIGHT_RATIO: f32 = 0.75;
/// The panel's smallest and largest size.
const HINT_MIN_W: f32 = 480.0;
const HINT_MIN_H: f32 = 240.0;
const HINT_MAX_W: f32 = 1500.0;
const HINT_MAX_H: f32 = 800.0;
/// The most of the window the panel's floor may take, so a small window
/// still shows a margin around it.
const HINT_FLOOR_CAP_RATIO: f32 = 0.92;
/// Space above the title, and the title's own padding around its text.
const HINT_PANEL_PAD_TOP: f32 = 3.0;
const HINT_TITLE_PAD_TOP: f32 = 2.0;
const HINT_TITLE_PAD_BOTTOM: f32 = 6.0;
/// The title sits a little left of the rows, as in magic-vscode.
const HINT_TITLE_PAD_X: f32 = 10.0;
/// Space between the divider under the title and the first row.
const HINT_TITLE_GAP: f32 = 2.0;
/// The title's opacity: a heading, quieter than the rows it heads.
const HINT_TITLE_ALPHA: f32 = 0.85;
/// Space below the last row the panel can hold.
const HINT_PANEL_PAD_BOTTOM: f32 = 10.0;
/// Space between the rows and the panel's sides.
const HINT_BODY_PAD_X: f32 = 16.0;
/// Space above and below each row's text.
const HINT_ROW_PAD_Y: f32 = 4.0;
/// Space between a row's key and what it does.
const HINT_KEY_GAP: f32 = 10.0;
/// The narrowest the key column is, so short keys still line up.
const HINT_MIN_KEY_W: f32 = 32.0;
/// The narrowest a column is, and the space between columns.
const HINT_MIN_COL_W: f32 = 160.0;
const HINT_COL_GAP: f32 = 24.0;
/// Slack kept at a column's right edge, so its widest row never touches the
/// next column.
const HINT_COL_SLACK: f32 = 4.0;
/// The most columns the rows are spread across.
const HINT_MAX_COLUMNS: usize = 5;
/// The longest a row's description is, in characters, before it is cut short.
const HINT_MAX_LABEL_CHARS: usize = 60;

/// One column of the key-hint popup: which rows it holds, how wide it is, and
/// how wide its key column is, all in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HintColumn {
    end: usize,
    key_w: f32,
    start: usize,
    width: f32,
}

/// Lay `widths` (each row's key and description width, in order) out in
/// columns, after magic-vscode: the fewest columns whose rows fit `avail_h`,
/// filled top to bottom, each as wide as its widest row needs but never more
/// than an equal share of `avail_w`, so the columns always fit across. `s`
/// scales the `HINT_*` lengths to the font.
fn hint_columns(
    widths: &[(f32, f32)],
    avail_w: f32,
    avail_h: f32,
    row_h: f32,
    s: f32,
) -> Vec<HintColumn> {
    let count = widths.len();
    if count == 0 {
        return Vec::new();
    }
    let (min_col_w, col_gap) = (HINT_MIN_COL_W * s, HINT_COL_GAP * s);
    let width_cap = (((avail_w + col_gap) / (min_col_w + col_gap)).floor() as usize).max(1);
    let max_columns = HINT_MAX_COLUMNS.min(count).min(width_cap);
    let columns = (1..=max_columns)
        .find(|&n| count.div_ceil(n) as f32 * row_h <= avail_h)
        .unwrap_or(max_columns);
    let fair = ((avail_w - col_gap * (columns - 1) as f32) / columns as f32).floor();
    let per_column = count.div_ceil(columns);
    (0..columns)
        .map(|column| {
            let start = column * per_column;
            let end = (start + per_column).min(count);
            let rows = &widths[start.min(end)..end];
            let widest_key = rows.iter().map(|(key, _)| *key).fold(0.0, f32::max);
            let widest_label = rows.iter().map(|(_, label)| *label).fold(0.0, f32::max);
            let key_w = (HINT_MIN_KEY_W * s).max(widest_key.ceil());
            let natural =
                min_col_w.max(key_w + HINT_KEY_GAP * s + widest_label + HINT_COL_SLACK * s);
            HintColumn {
                end,
                key_w,
                start: start.min(end),
                width: natural.min(fair),
            }
        })
        .collect()
}

/// `low`..`high` scaled, with the floor capped at `HINT_FLOOR_CAP_RATIO` of
/// `surface` and `ratio` of it in between: CSS's
/// `clamp(min(low, cap), ratio, high)`.
fn hint_panel_length(surface: f32, ratio: f32, low: f32, high: f32) -> f32 {
    let floor = low.min(surface * HINT_FLOOR_CAP_RATIO);
    (surface * ratio).clamp(floor, high.max(floor)).floor()
}

/// Shape `text` in `color`, cut short with an ellipsis until it fits
/// `max_w` pixels.
fn shape_fitted(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    text: &str,
    color: glyphon::Color,
    proportional: bool,
    max_w: f32,
) -> glyphon::Buffer {
    let mut chars = text.chars().count();
    loop {
        let fitted = truncate_label(text, chars);
        let buffer = shape_chrome_line(font_system, ctx, &fitted, color, false, proportional);
        if chars <= 1 || buffer_width(&buffer) <= max_w {
            return buffer;
        }
        chars -= 1;
    }
}

/// Rasterize the key-hint popup after magic-vscode's: a large panel centred
/// on the window, a quiet left-aligned title over a divider, then plain rows
/// of a key in the link color beside what it does, flowing down columns.
pub(in crate::renderer) fn which_key_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &WhichKeyView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let s = ctx.font_size / HINT_BASE_FONT_PX;
    let margin = DROPDOWN_SHADOW;
    let panel_w = hint_panel_length(surface_w, HINT_WIDTH_RATIO, HINT_MIN_W * s, HINT_MAX_W * s);
    let panel_h = hint_panel_length(surface_h, HINT_HEIGHT_RATIO, HINT_MIN_H * s, HINT_MAX_H * s);
    let mut frame = PopupFrame::new(theme, panel_w, panel_h, margin);

    // Title, then the divider under it, across the whole panel.
    let foreground = theme.foreground.to_glyphon();
    let title_color = glyphon::Color::rgba(
        foreground.r(),
        foreground.g(),
        foreground.b(),
        (HINT_TITLE_ALPHA * 255.0).round() as u8,
    );
    let title_x = margin + HINT_TITLE_PAD_X * s;
    let title_y = margin + (HINT_PANEL_PAD_TOP + HINT_TITLE_PAD_TOP) * s;
    let title_room = panel_w - (HINT_TITLE_PAD_X + HINT_BODY_PAD_X) * s;
    let mut title = shape_fitted(font_system, ctx, &view.title, title_color, true, title_room);
    frame.draw_text(
        font_system,
        swash_cache,
        &mut title,
        (title_x.round() as i32, title_y.round() as i32),
        title_color,
    );
    let divider_y = (title_y + ctx.line_height + HINT_TITLE_PAD_BOTTOM * s).round();
    frame.fill((margin, divider_y, panel_w, 1.0), 0.0, theme.divider, 1.0);

    // The rows: measured first, so the columns can be sized to them.
    let key_color = theme.ansi[4].to_glyphon();
    let labels: Vec<String> = view
        .items
        .iter()
        .map(|(_, label)| truncate_label(label, HINT_MAX_LABEL_CHARS))
        .collect();
    let widths: Vec<(f32, f32)> = view
        .items
        .iter()
        .zip(&labels)
        .map(|((key, _), label)| {
            let key = shape_chrome_line(font_system, ctx, key, key_color, false, false);
            let label = shape_chrome_line(font_system, ctx, label, foreground, false, true);
            (buffer_width(&key), buffer_width(&label))
        })
        .collect();
    let row_h = (ctx.line_height + 2.0 * HINT_ROW_PAD_Y * s).round();
    let body_top = divider_y + 1.0 + HINT_TITLE_GAP * s;
    let body_bottom = margin + panel_h - HINT_PANEL_PAD_BOTTOM * s;
    let avail_w = panel_w - 2.0 * HINT_BODY_PAD_X * s;
    let columns = hint_columns(&widths, avail_w, body_bottom - body_top, row_h, s);

    let mut column_x = margin + HINT_BODY_PAD_X * s;
    for column in &columns {
        let label_x = column_x + column.key_w + HINT_KEY_GAP * s;
        let label_room = (column_x + column.width - label_x).max(0.0);
        for (row, index) in (column.start..column.end).enumerate() {
            let row_y = body_top + row as f32 * row_h;
            // Rows past the panel's foot are cut off, as overflow is in
            // magic-vscode's: the column count already did its best.
            if row_y + row_h > body_bottom {
                break;
            }
            let text_y = (row_y + HINT_ROW_PAD_Y * s).round() as i32;
            let (key, _) = &view.items[index];
            let mut key_buf = shape_fitted(font_system, ctx, key, key_color, false, column.key_w);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut key_buf,
                (column_x.round() as i32, text_y),
                key_color,
            );
            let mut label_buf = shape_fitted(
                font_system,
                ctx,
                &labels[index],
                foreground,
                true,
                label_room,
            );
            frame.draw_text(
                font_system,
                swash_cache,
                &mut label_buf,
                (label_x.round() as i32, text_y),
                foreground,
            );
        }
        column_x += column.width + HINT_COL_GAP * s;
    }

    let (panel_x, panel_y) = centered(surface_w, surface_h, panel_w, panel_h);
    frame.into_image(panel_x, panel_y)
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_which_key_rgba_is_centered_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1000.0;
        let surface_h = 800.0;

        let view = WhichKeyView {
            title: "Key hints: (g) was pressed, wait for the next key stroke...".to_string(),
            items: vec![
                ("g".to_string(), "top of buffer".to_string()),
                ("v".to_string(), "restore visual".to_string()),
            ],
        };

        let image = which_key_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        // Centered horizontally: left margin plus card width / 2 is close to surface_w / 2
        let card_center_x = image.x + image.width as f32 / 2.0;
        assert!(
            (card_center_x - surface_w / 2.0).abs() < 50.0,
            "which-key card is roughly centered"
        );
        // Body is non-empty and opaque
        assert!(image.width > 100);
        assert!(image.height > 50);
        let center_idx = ((image.height / 2 * image.width + image.width / 2) * 4) as usize;
        assert_eq!(image.rgba[center_idx + 3], 255, "card interior is opaque");
    }
    #[test]
    fn test_hint_columns_use_the_fewest_columns_whose_rows_fit_the_height() {
        let rows = vec![(10.0, 50.0); 10];
        // Five rows fit 100px at 20px a row: two columns, filled top down.
        let columns = hint_columns(&rows, 1000.0, 100.0, 20.0, 1.0);
        assert_eq!(columns.len(), 2);
        assert_eq!((columns[0].start, columns[0].end), (0, 5));
        assert_eq!((columns[1].start, columns[1].end), (5, 10));
        // With room for all ten, one column, however wide the panel.
        assert_eq!(hint_columns(&rows, 1000.0, 200.0, 20.0, 1.0).len(), 1);
    }

    #[test]
    fn test_hint_columns_never_overflow_the_panel_width() {
        // Long descriptions want far more than an equal share: each column is
        // held to it, so the row of columns still fits across.
        let rows = vec![(10.0, 900.0); 6];
        let columns = hint_columns(&rows, 600.0, 40.0, 20.0, 1.0);
        let total: f32 = columns.iter().map(|c| c.width).sum::<f32>()
            + HINT_COL_GAP * (columns.len() - 1) as f32;
        assert!(total <= 600.0, "{total} > 600");
        // A narrow panel holds as many columns as fit at their narrowest.
        let narrow = hint_columns(&[(10.0, 50.0); 20], 400.0, 20.0, 20.0, 1.0);
        assert_eq!(narrow.len(), 2);
    }

    #[test]
    fn test_hint_panel_is_a_share_of_the_window_within_its_bounds() {
        assert_eq!(hint_panel_length(1000.0, 0.8, 480.0, 1500.0), 800.0);
        assert_eq!(hint_panel_length(4000.0, 0.8, 480.0, 1500.0), 1500.0);
        // A small window: the floor gives way to keep a margin around it.
        assert_eq!(hint_panel_length(400.0, 0.8, 480.0, 1500.0), 368.0);
    }

    #[test]
    fn test_the_key_hint_panel_is_one_size_whatever_it_holds() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);
        let mut card = |count: usize| {
            let view = WhichKeyView {
                title: "Branch".to_string(),
                items: (0..count)
                    .map(|i| (i.to_string(), format!("choice {i}")))
                    .collect(),
            };
            let image = which_key_rgba(
                &mut font_system,
                &mut swash,
                &ctx,
                &theme,
                &view,
                surface_w,
                surface_h,
            );
            (image.width, image.height)
        };

        // As magic-vscode's: a share of the window, not of the content, so
        // the panel does not jump about from one prefix to the next.
        let small = card(2);
        let large = card(15);
        assert_eq!(small, large);
        // It stays bounded within the surface margins
        assert!(large.0 as f32 <= surface_w + 2.0 * DROPDOWN_SHADOW);
        assert!(large.1 as f32 <= surface_h + 2.0 * DROPDOWN_SHADOW);
    }
}

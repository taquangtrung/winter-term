//! The key-hint HUD shown when a key that starts a sequence is pressed and
//! the rest is waited for: what each next key does, in columns. A HUD only
//! shows: it takes no keys, and goes once the next key is pressed.

use super::view::WhichKeyView;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::chrome::{
    centered, centered_in_rect, shape_fitted, Chrome, TitleBar, TITLE_BAR_TOP_PAD,
};
use crate::renderer::raster::paint::{
    buffer_width, fill_line_segment, shape_chrome_line, truncate_label,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::{DROPDOWN_SHADOW, POPUP_EDGE_ROOM};
use crate::theme::{Rgb, Theme};
use glyphon::{FontSystem, SwashCache};

// ========================================================================
// Constants
// ========================================================================

/// The size of a graphical checkbox, in base pixels before zoom scaling.
const CHECKBOX_BASE_SIZE: f32 = 13.0;
/// Space between a checkbox and its label text.
const CHECKBOX_LABEL_GAP: f32 = 8.0;

/// The font size, in pixels, the key-hint popup is laid out at.
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
/// Space between the divider under the title and the first row.
const HINT_TITLE_GAP: f32 = 2.0;
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

// ========================================================================
// Data Structures
// ========================================================================

/// One interactive item row of the key-hint popup HUD.
#[derive(Clone, Debug, PartialEq)]
pub struct WhichKeyHit {
    /// Item index in the view's items list.
    pub index: usize,
    /// Whether this item represents a toggleable checkbox parameter.
    pub is_checkbox: bool,
    /// Key string (e.g. `"-a"`, `"c"`, `"f"`).
    pub key: String,
    /// Surface pixel bounds `(x, y, w, h)`.
    pub rect: (f32, f32, f32, f32),
}

/// What an item's label is and whether it is a checkbox parameter.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::renderer) struct ParsedItem {
    pub(in crate::renderer) checkbox: Option<bool>,
    pub(in crate::renderer) label: String,
}

/// One column of the key-hint popup: which rows it holds, how wide it is, and
/// how wide its key column is, all in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
struct HintColumn {
    end: usize,
    key_w: f32,
    start: usize,
    width: f32,
}

// ========================================================================
// Functions
// ========================================================================

/// Parse a plain-text checkbox prefix or suffix (`[x] `, `[ ] `, `[on]`, `[off]`)
/// out of `label`, returning whether it is checked and the clean label.
pub(in crate::renderer) fn parse_item_checkbox(label: &str) -> ParsedItem {
    if let Some(rest) = label
        .strip_prefix("[x] ")
        .or_else(|| label.strip_prefix("[X] "))
    {
        let clean = rest
            .strip_suffix("  [on]")
            .or_else(|| rest.strip_suffix("[on]"))
            .unwrap_or(rest)
            .trim_end();
        ParsedItem {
            checkbox: Some(true),
            label: clean.to_string(),
        }
    } else if let Some(rest) = label.strip_prefix("[ ] ") {
        let clean = rest
            .strip_suffix("  [off]")
            .or_else(|| rest.strip_suffix("[off]"))
            .unwrap_or(rest)
            .trim_end();
        ParsedItem {
            checkbox: Some(false),
            label: clean.to_string(),
        }
    } else if let Some(rest) = label
        .strip_suffix("  [on]")
        .or_else(|| label.strip_suffix("[on]"))
    {
        ParsedItem {
            checkbox: Some(true),
            label: rest.trim_end().to_string(),
        }
    } else if let Some(rest) = label
        .strip_suffix("  [off]")
        .or_else(|| label.strip_suffix("[off]"))
    {
        ParsedItem {
            checkbox: Some(false),
            label: rest.trim_end().to_string(),
        }
    } else {
        ParsedItem {
            checkbox: None,
            label: label.to_string(),
        }
    }
}

/// Draw an anti-aliased graphical checkbox (rounded rectangle, and checkmark when checked).
fn draw_checkbox(
    frame: &mut Chrome,
    (x, y, w, h): (f32, f32, f32, f32),
    checked: bool,
    theme: &Theme,
    s: f32,
) {
    let radius = (3.0 * s).round().max(2.0);
    if checked {
        // Checked: filled with active indicator accent
        frame.fill((x, y, w, h), radius, theme.active_indicator, 1.0);

        // Crisp vector checkmark inside the box
        let check_color = match theme.active_indicator.luminance() > 0.65 {
            true => theme.background,
            false => Rgb::new(255, 255, 255),
        };
        let p0 = (x + w * 0.22, y + h * 0.52);
        let p1 = (x + w * 0.42, y + h * 0.72);
        let p2 = (x + w * 0.78, y + h * 0.28);
        let thickness = (1.8 * s).max(1.5);
        let (rgba, canvas) = frame.pixels();
        fill_line_segment(rgba, canvas, p0, p1, thickness, check_color, 1.0);
        fill_line_segment(rgba, canvas, p1, p2, thickness, check_color, 1.0);
    } else {
        // Unchecked: subtle rounded border with card background interior
        let border_thickness = (1.2 * s).round().max(1.0);
        frame.fill((x, y, w, h), radius, theme.divider, 0.9);
        let inner_x = x + border_thickness;
        let inner_y = y + border_thickness;
        let inner_w = (w - 2.0 * border_thickness).max(0.0);
        let inner_h = (h - 2.0 * border_thickness).max(0.0);
        let inner_r = (radius - border_thickness).max(1.0);
        frame.fill(
            (inner_x, inner_y, inner_w, inner_h),
            inner_r,
            theme.menu_bg,
            1.0,
        );
    }
}

/// Lay `widths` (each row's key and description width, in order) out in
/// columns: the fewest columns whose rows fit `avail_h`,
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

/// `low`..`high` scaled, with the floor capped [`POPUP_EDGE_ROOM`] short of
/// `surface`, so a small pane still shows a margin around it, and `ratio` of
/// it in between: CSS's `clamp(min(low, cap), ratio, high)`.
fn hint_panel_length(surface: f32, ratio: f32, low: f32, high: f32) -> f32 {
    let floor = low.min((surface - POPUP_EDGE_ROOM).max(0.0));
    (surface * ratio).clamp(floor, high.max(floor)).floor()
}

/// Rasterize the key-hint popup: a large panel centred
/// on the window, an undecorated title bar ([`TitleBar::Undecorated`]), then plain rows
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
    // Sized to, and centred in, the pane it serves, as a transient is.
    let (area_w, area_h) = match view.anchor_rect {
        Some((_, _, w, h)) => (w, h),
        None => (surface_w, surface_h),
    };
    let panel_w = hint_panel_length(area_w, HINT_WIDTH_RATIO, HINT_MIN_W * s, HINT_MAX_W * s);
    let panel_h = hint_panel_length(area_h, HINT_HEIGHT_RATIO, HINT_MIN_H * s, HINT_MAX_H * s);
    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);

    // A HUD's undecorated title bar: the key hints only show, they ask nothing.
    let foreground = theme.foreground.to_glyphon();
    let divider_y = frame.draw_title(
        font_system,
        swash_cache,
        ctx,
        theme,
        TitleBar::Undecorated,
        &view.title,
        "",
    );

    // The rows: measured first, so the columns can be sized to them.
    let box_size = (CHECKBOX_BASE_SIZE * s).round().max(12.0);
    let box_gap = (CHECKBOX_LABEL_GAP * s).round();
    let key_color = theme.ansi[4].to_glyphon();

    let parsed_items: Vec<ParsedItem> = view
        .items
        .iter()
        .map(|(_, label)| parse_item_checkbox(label))
        .collect();

    let widths: Vec<(f32, f32)> = view
        .items
        .iter()
        .zip(&parsed_items)
        .map(|((key, _), parsed)| {
            let truncated = truncate_label(&parsed.label, HINT_MAX_LABEL_CHARS);
            let key_buf = shape_chrome_line(font_system, ctx, key, key_color, false, false);
            let label_buf =
                shape_chrome_line(font_system, ctx, &truncated, foreground, false, true);
            let extra_w = if parsed.checkbox.is_some() {
                box_size + box_gap
            } else {
                0.0
            };
            (buffer_width(&key_buf), buffer_width(&label_buf) + extra_w)
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
            // Rows past the panel's foot are cut off: the column count
            // already did its best.
            if row_y + row_h > body_bottom {
                break;
            }
            let text_y = (row_y + HINT_ROW_PAD_Y * s).round() as i32;
            let (key, _) = &view.items[index];
            let parsed = &parsed_items[index];

            let mut key_buf =
                shape_fitted(font_system, ctx, key, key_color, false, false, column.key_w);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut key_buf,
                (column_x.round() as i32, text_y),
                key_color,
            );

            let (text_x, text_room) = match parsed.checkbox {
                Some(checked) => {
                    let box_x = label_x;
                    let box_y = (row_y + (row_h - box_size) / 2.0).round();
                    draw_checkbox(
                        &mut frame,
                        (box_x, box_y, box_size, box_size),
                        checked,
                        theme,
                        s,
                    );
                    (
                        box_x + box_size + box_gap,
                        (label_room - box_size - box_gap).max(0.0),
                    )
                }
                None => (label_x, label_room),
            };

            let truncated = truncate_label(&parsed.label, HINT_MAX_LABEL_CHARS);
            let mut label_buf = shape_fitted(
                font_system,
                ctx,
                &truncated,
                foreground,
                false,
                true,
                text_room,
            );
            frame.draw_text(
                font_system,
                swash_cache,
                &mut label_buf,
                (text_x.round() as i32, text_y),
                foreground,
            );
        }
        column_x += column.width + HINT_COL_GAP * s;
    }

    let (panel_x, panel_y) = match view.anchor_rect {
        Some(rect) => centered_in_rect(surface_w, surface_h, panel_w, panel_h, rect),
        None => centered(surface_w, surface_h, panel_w, panel_h),
    };
    frame.into_image(panel_x, panel_y)
}

/// Compute the surface hit areas for each visible item in the which-key popup HUD.
pub(in crate::renderer) fn which_key_item_hits(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    view: &WhichKeyView,
    card_rect: (f32, f32, f32, f32),
) -> Vec<WhichKeyHit> {
    let (panel_x, panel_y, panel_w, panel_h) = card_rect;
    let s = ctx.font_size / HINT_BASE_FONT_PX;
    let box_size = (CHECKBOX_BASE_SIZE * s).round().max(12.0);
    let box_gap = (CHECKBOX_LABEL_GAP * s).round();
    let margin = DROPDOWN_SHADOW;

    let key_color = glyphon::Color::rgb(0, 0, 0);
    let foreground = glyphon::Color::rgb(0, 0, 0);

    let parsed_items: Vec<ParsedItem> = view
        .items
        .iter()
        .map(|(_, label)| parse_item_checkbox(label))
        .collect();

    let widths: Vec<(f32, f32)> = view
        .items
        .iter()
        .zip(&parsed_items)
        .map(|((key, _), parsed)| {
            let truncated = truncate_label(&parsed.label, HINT_MAX_LABEL_CHARS);
            let key_buf = shape_chrome_line(font_system, ctx, key, key_color, false, false);
            let label_buf =
                shape_chrome_line(font_system, ctx, &truncated, foreground, false, true);
            let extra_w = if parsed.checkbox.is_some() {
                box_size + box_gap
            } else {
                0.0
            };
            (buffer_width(&key_buf), buffer_width(&label_buf) + extra_w)
        })
        .collect();

    let row_h = (ctx.line_height + 2.0 * HINT_ROW_PAD_Y * s).round();
    let title_h = crate::renderer::raster::chrome::title_bar_height(ctx);
    // As drawn: the title bar's top pad does not scale with the font.
    let body_top = margin + TITLE_BAR_TOP_PAD + title_h + 1.0 + HINT_TITLE_GAP * s;
    let body_bottom = margin + panel_h - HINT_PANEL_PAD_BOTTOM * s;
    let avail_w = panel_w - 2.0 * HINT_BODY_PAD_X * s;
    let columns = hint_columns(&widths, avail_w, body_bottom - body_top, row_h, s);

    let mut hits = Vec::new();
    let mut column_x = margin + HINT_BODY_PAD_X * s;
    for column in &columns {
        for (row, index) in (column.start..column.end).enumerate() {
            let row_y = body_top + row as f32 * row_h;
            if row_y + row_h > body_bottom {
                break;
            }
            let (key, _) = &view.items[index];
            hits.push(WhichKeyHit {
                index,
                is_checkbox: parsed_items[index].checkbox.is_some(),
                key: key.clone(),
                rect: (
                    panel_x + (column_x - margin),
                    panel_y + (row_y - margin),
                    column.width,
                    row_h,
                ),
            });
        }
        column_x += column.width + HINT_COL_GAP * s;
    }
    hits
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::*;

    #[test]
    fn test_which_key_is_sized_to_and_centred_in_its_pane() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let ctx = sample_font_ctx();
        let pane = (500.0, 100.0, 500.0, 600.0);
        let view = WhichKeyView {
            anchor_rect: Some(pane),
            items: vec![("g".to_string(), "top of buffer".to_string())],
            title: "g".to_string(),
        };
        let image = which_key_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &Theme::dark(),
            &view,
            1000.0,
            800.0,
        );
        let card_w = image.width as f32 - 2.0 * DROPDOWN_SHADOW;
        let center_x = image.x + image.width as f32 / 2.0;
        assert!(card_w <= 500.0, "no wider than its pane: {card_w}");
        assert!(
            (center_x - 750.0).abs() < 2.0,
            "centred on the pane: {center_x}"
        );
    }

    #[test]
    fn test_which_key_rgba_is_centered_card() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1000.0;
        let surface_h = 800.0;

        let view = WhichKeyView {
            anchor_rect: None,
            title: "Key hints: (g) was pressed, wait for the next key stroke…".to_string(),
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
        assert_eq!(hint_panel_length(400.0, 0.8, 480.0, 1500.0), 360.0);
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
                anchor_rect: None,
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

        // A share of the window, not of the content, so
        // the panel does not jump about from one prefix to the next.
        let small = card(2);
        let large = card(15);
        assert_eq!(small, large);
        // It stays bounded within the surface margins
        assert!(large.0 as f32 <= surface_w + 2.0 * DROPDOWN_SHADOW);
        assert!(large.1 as f32 <= surface_h + 2.0 * DROPDOWN_SHADOW);
    }

    #[test]
    fn test_parse_item_checkbox_extracts_marks_and_cleans_label() {
        assert_eq!(
            parse_item_checkbox("[x] Force with lease (--force-with-lease)"),
            ParsedItem {
                checkbox: Some(true),
                label: "Force with lease (--force-with-lease)".to_string(),
            }
        );
        assert_eq!(
            parse_item_checkbox("[ ] Allow empty (--allow-empty)"),
            ParsedItem {
                checkbox: Some(false),
                label: "Allow empty (--allow-empty)".to_string(),
            }
        );
        assert_eq!(
            parse_item_checkbox("Stage all (--all)  [on]"),
            ParsedItem {
                checkbox: Some(true),
                label: "Stage all (--all)".to_string(),
            }
        );
        assert_eq!(
            parse_item_checkbox("commit, in an editor"),
            ParsedItem {
                checkbox: None,
                label: "commit, in an editor".to_string(),
            }
        );
    }

    #[test]
    fn test_which_key_renders_checkbox_items_and_produces_hits() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let (surface_w, surface_h) = (1000.0, 800.0);

        let view = WhichKeyView {
            anchor_rect: None,
            title: "Commit".to_string(),
            items: vec![
                ("c".to_string(), "commit, in an editor".to_string()),
                ("-a".to_string(), "[x] Stage all files (--all)".to_string()),
                (
                    "-e".to_string(),
                    "[ ] Allow empty commit (--allow-empty)".to_string(),
                ),
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
        assert!(image.width > 0 && image.height > 0);

        let hits = which_key_item_hits(
            &mut font_system,
            &ctx,
            &view,
            (
                image.x + DROPDOWN_SHADOW,
                image.y + DROPDOWN_SHADOW,
                image.width as f32 - 2.0 * DROPDOWN_SHADOW,
                image.height as f32 - 2.0 * DROPDOWN_SHADOW,
            ),
        );
        assert_eq!(hits.len(), 3);
        assert!(!hits[0].is_checkbox);
        assert_eq!(hits[0].key, "c");
        assert!(hits[1].is_checkbox);
        assert_eq!(hits[1].key, "-a");
        assert!(hits[2].is_checkbox);
        assert_eq!(hits[2].key, "-e");
    }
}

//! The transient dialog window for commands that accept parameters and switches.
//! Unlike a passive which-key HUD, a transient dialog is content-fitted and
//! interactive: switches have graphical vector checkboxes that can be toggled
//! directly via keyboard or mouse clicks, and commands execute when selected.

use glyphon::{FontSystem, SwashCache};

use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::chrome::{
    centered_in_rect, shape_fitted, title_bar_height, top_anchored, Chrome, TitleBar,
    TITLE_BAR_TOP_PAD,
};
use crate::renderer::raster::paint::{
    buffer_width, fill_line_segment, shape_chrome_line, truncate_label,
};
use crate::renderer::raster::view::DropdownImage;
use crate::renderer::raster::DROPDOWN_SHADOW;
use crate::theme::{Rgb, Theme};

use super::view::{SwitchKind, TransientView};

// ========================================================================
// Constants
// ========================================================================

/// The base pixel size of a graphical vector checkbox before zoom scaling.
const CHECKBOX_BASE_SIZE: f32 = 13.0;

/// Space between a checkbox and its label text.
const CHECKBOX_LABEL_GAP: f32 = 8.0;

/// Reference font size for layout scaling.
const BASE_FONT_PX: f32 = 13.0;

/// Min and max panel width for the content-fitted window.
const MIN_PANEL_W: f32 = 480.0;
const MAX_PANEL_W: f32 = 720.0;

/// Minimum key column width.
const MIN_KEY_W: f32 = 28.0;

/// Space between the key column and the item content.
const KEY_GAP: f32 = 12.0;

/// Horizontal padding on left and right inside the panel card.
const BODY_PAD_X: f32 = 16.0;

/// Vertical padding above and below each row.
const ROW_PAD_Y: f32 = 4.0;

/// Padding below the last item in the panel.
const PANEL_PAD_BOTTOM: f32 = 12.0;

/// Vertical gap between sections when both switches and commands are present.
const SECTION_GAP_Y: f32 = 8.0;

/// Maximum characters in a label before truncation.
const MAX_LABEL_CHARS: usize = 60;

// ========================================================================
// Data Structures
// ========================================================================

/// An interactive hit region in the transient window.
#[derive(Clone, Debug, PartialEq)]
pub struct TransientHit {
    /// Item index within its list (switches or commands).
    pub index: usize,
    /// Whether this hit targets a parameter switch rather than a command.
    pub is_switch: bool,
    /// Key string to dispatch upon activation (e.g. `"-a"` or `"c"`).
    pub key: String,
    /// Surface bounds `(x, y, w, h)`.
    pub rect: (f32, f32, f32, f32),
}

// ========================================================================
// Functions
// ========================================================================

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
        frame.fill((x, y, w, h), radius, theme.active_indicator, 1.0);

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

/// Draw an anti-aliased circular radio / bullet button.
fn draw_bullet_radio(
    frame: &mut Chrome,
    (x, y, w, h): (f32, f32, f32, f32),
    checked: bool,
    theme: &Theme,
    s: f32,
) {
    let radius = w / 2.0;
    if checked {
        // Outer colored ring with card background interior
        let border_thickness = (1.5 * s).round().max(1.0);
        frame.fill((x, y, w, h), radius, theme.active_indicator, 1.0);
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

        // Center filled bullet dot
        let dot_inset = (3.5 * s).round().max(3.0);
        let dot_w = (w - 2.0 * dot_inset).max(2.0);
        let dot_h = (h - 2.0 * dot_inset).max(2.0);
        let dot_x = x + dot_inset;
        let dot_y = y + dot_inset;
        frame.fill(
            (dot_x, dot_y, dot_w, dot_h),
            dot_w / 2.0,
            theme.active_indicator,
            1.0,
        );
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

/// Compute layout metrics for transient window sizing and placement.
#[allow(clippy::too_many_arguments)]
fn compute_transient_layout(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    view: &TransientView,
    surface_w: f32,
    surface_h: f32,
    s: f32,
    box_size: f32,
    box_gap: f32,
) -> (f32, f32, f32, f32, f32, f32, f32) {
    let dummy_color = glyphon::Color::rgb(0, 0, 0);

    // Measure keys to find widest key column
    let mut widest_key = MIN_KEY_W * s;
    for cmd in &view.commands {
        let buf = shape_chrome_line(font_system, ctx, &cmd.key, dummy_color, false, false);
        widest_key = widest_key.max(buffer_width(&buf));
    }
    for sw in &view.switches {
        let buf = shape_chrome_line(font_system, ctx, &sw.key, dummy_color, false, false);
        widest_key = widest_key.max(buffer_width(&buf));
    }
    let key_w = (widest_key + 2.0 * s).ceil();

    // Measure widest content row
    let mut widest_content = 0.0f32;
    for cmd in &view.commands {
        let trunc = truncate_label(&cmd.label, MAX_LABEL_CHARS);
        let buf = shape_chrome_line(font_system, ctx, &trunc, dummy_color, false, false);
        let row_w = key_w + KEY_GAP * s + buffer_width(&buf);
        widest_content = widest_content.max(row_w);
    }
    for sw in &view.switches {
        let trunc = truncate_label(&sw.label, MAX_LABEL_CHARS);
        let l_buf = shape_chrome_line(font_system, ctx, &trunc, dummy_color, false, false);
        let f_buf = shape_chrome_line(font_system, ctx, &sw.flag, dummy_color, false, false);
        let flag_w = if sw.flag.is_empty() {
            0.0
        } else {
            buffer_width(&f_buf) + 12.0 * s
        };
        let row_w = key_w + KEY_GAP * s + box_size + box_gap + buffer_width(&l_buf) + flag_w;
        widest_content = widest_content.max(row_w);
    }

    let title_buf = shape_chrome_line(font_system, ctx, &view.title, dummy_color, true, false);
    let title_w = buffer_width(&title_buf) + 48.0 * s;

    let max_w = (surface_w * 0.95).max(100.0).min(MAX_PANEL_W * s);
    let min_w = (MIN_PANEL_W * s).min(max_w);
    let panel_w = (widest_content + 2.0 * BODY_PAD_X * s)
        .max(title_w)
        .clamp(min_w, max_w)
        .floor();

    let header_h = if view.title.is_empty() {
        0.0
    } else {
        TITLE_BAR_TOP_PAD + title_bar_height(ctx)
    };
    let row_h = (ctx.line_height + 2.0 * ROW_PAD_Y * s).round();
    let section_hdr_h = (ctx.line_height + 4.0 * s).round();

    let has_sections = !view.switches.is_empty() && !view.commands.is_empty();
    let switches_h = if view.switches.is_empty() {
        0.0
    } else {
        (if has_sections { section_hdr_h } else { 0.0 }) + view.switches.len() as f32 * row_h
    };
    let divider_h = if has_sections {
        SECTION_GAP_Y * s + 1.0 + SECTION_GAP_Y * s
    } else {
        0.0
    };
    let commands_h = if view.commands.is_empty() {
        0.0
    } else {
        (if has_sections { section_hdr_h } else { 0.0 }) + view.commands.len() as f32 * row_h
    };

    let total_h = header_h + 8.0 * s + switches_h + divider_h + commands_h + PANEL_PAD_BOTTOM * s;
    let panel_h = total_h.min((surface_h * 0.95).max(100.0)).floor();

    (
        panel_w,
        panel_h,
        key_w,
        row_h,
        header_h,
        section_hdr_h,
        divider_h,
    )
}

/// Rasterize the content-fitted transient window and return the positioned dropdown image.
pub(in crate::renderer) fn transient_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &TransientView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let s = ctx.font_size / BASE_FONT_PX;
    let margin = DROPDOWN_SHADOW;
    let box_size = (CHECKBOX_BASE_SIZE * s).round().max(12.0);
    let box_gap = (CHECKBOX_LABEL_GAP * s).round().max(6.0);

    let (panel_w, panel_h, key_w, row_h, header_h, section_hdr_h, _) = compute_transient_layout(
        font_system,
        ctx,
        view,
        surface_w,
        surface_h,
        s,
        box_size,
        box_gap,
    );

    let mut frame = Chrome::new(theme, panel_w, panel_h, margin);

    let body_top = if !view.title.is_empty() {
        frame.draw_title(
            font_system,
            swash_cache,
            ctx,
            theme,
            TitleBar::Decorated,
            &view.title,
            "",
        ) + 4.0 * s
    } else {
        margin + header_h + 4.0 * s
    };

    let body_bottom = margin + panel_h - PANEL_PAD_BOTTOM * s;
    let foreground = theme.foreground.to_glyphon();
    let key_color = theme.ansi[4].to_glyphon();
    let muted_color = theme.ansi[8].to_glyphon();
    let flag_color = theme.ansi[4].to_glyphon();

    let has_sections = !view.switches.is_empty() && !view.commands.is_empty();
    let content_x = margin + BODY_PAD_X * s;
    let row_w = panel_w - 2.0 * BODY_PAD_X * s;

    let mut cur_y = body_top;

    // 1. Switches Section
    if !view.switches.is_empty() {
        if has_sections {
            let mut sec_buf =
                shape_chrome_line(font_system, ctx, "Parameters", muted_color, true, false);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut sec_buf,
                (content_x.round() as i32, (cur_y + 2.0 * s).round() as i32),
                muted_color,
            );
            cur_y += section_hdr_h;
        }

        for (i, sw) in view.switches.iter().enumerate() {
            if cur_y + row_h > body_bottom {
                break;
            }
            if i == view.selected {
                let hover_rect = (
                    margin + 4.0 * s,
                    cur_y + 1.0,
                    panel_w - 8.0 * s,
                    row_h - 2.0,
                );
                frame.fill(hover_rect, 2.0, theme.menu_hover_bg, 1.0);
            }
            let text_y = (cur_y + ROW_PAD_Y * s).round() as i32;

            // Key column
            let mut key_buf =
                shape_fitted(font_system, ctx, &sw.key, key_color, false, false, key_w);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut key_buf,
                (content_x.round() as i32, text_y),
                key_color,
            );

            // Vector indicator: square checkbox or circular radio bullet
            let cb_x = content_x + key_w + KEY_GAP * s;
            let cb_y = (cur_y + (row_h - box_size) / 2.0).round();
            match sw.kind {
                SwitchKind::Checkbox => {
                    draw_checkbox(
                        &mut frame,
                        (cb_x, cb_y, box_size, box_size),
                        sw.checked,
                        theme,
                        s,
                    );
                }
                SwitchKind::Radio => {
                    draw_bullet_radio(
                        &mut frame,
                        (cb_x, cb_y, box_size, box_size),
                        sw.checked,
                        theme,
                        s,
                    );
                }
            }

            // Label
            let label_x = cb_x + box_size + box_gap;
            let flag_w = if sw.flag.is_empty() {
                0.0
            } else {
                let f_buf = shape_chrome_line(font_system, ctx, &sw.flag, flag_color, false, false);
                buffer_width(&f_buf) + 12.0 * s
            };
            let max_label_w = (content_x + row_w - label_x - flag_w).max(0.0);

            let trunc = truncate_label(&sw.label, MAX_LABEL_CHARS);
            let mut label_buf = shape_fitted(
                font_system,
                ctx,
                &trunc,
                foreground,
                false,
                true,
                max_label_w,
            );
            frame.draw_text(
                font_system,
                swash_cache,
                &mut label_buf,
                (label_x.round() as i32, text_y),
                foreground,
            );

            // Flag
            if !sw.flag.is_empty() {
                let actual_lw = buffer_width(&label_buf);
                let flag_x = label_x + actual_lw + 12.0 * s;
                let room_for_flag = (content_x + row_w - flag_x).max(0.0);
                let mut flag_buf = shape_fitted(
                    font_system,
                    ctx,
                    &sw.flag,
                    flag_color,
                    false,
                    false,
                    room_for_flag,
                );
                frame.draw_text(
                    font_system,
                    swash_cache,
                    &mut flag_buf,
                    (flag_x.round() as i32, text_y),
                    flag_color,
                );
            }

            cur_y += row_h;
        }
    }

    // Divider between sections
    if has_sections {
        cur_y += SECTION_GAP_Y * s;
        let (rgba, canvas) = frame.pixels();
        fill_line_segment(
            rgba,
            canvas,
            (content_x, cur_y.round()),
            (content_x + row_w, cur_y.round()),
            1.0,
            theme.divider,
            0.8,
        );
        cur_y += 1.0 + SECTION_GAP_Y * s;
    }

    // 2. Commands Section
    if !view.commands.is_empty() {
        if has_sections {
            let mut sec_buf =
                shape_chrome_line(font_system, ctx, "Commands", muted_color, true, false);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut sec_buf,
                (content_x.round() as i32, (cur_y + 2.0 * s).round() as i32),
                muted_color,
            );
            cur_y += section_hdr_h;
        }

        for (j, cmd) in view.commands.iter().enumerate() {
            if cur_y + row_h > body_bottom {
                break;
            }
            let global_idx = view.switches.len() + j;
            if global_idx == view.selected {
                let hover_rect = (
                    margin + 4.0 * s,
                    cur_y + 1.0,
                    panel_w - 8.0 * s,
                    row_h - 2.0,
                );
                frame.fill(hover_rect, 2.0, theme.menu_hover_bg, 1.0);
            }
            let text_y = (cur_y + ROW_PAD_Y * s).round() as i32;

            // Key column
            let mut key_buf =
                shape_fitted(font_system, ctx, &cmd.key, key_color, false, false, key_w);
            frame.draw_text(
                font_system,
                swash_cache,
                &mut key_buf,
                (content_x.round() as i32, text_y),
                key_color,
            );

            // Label
            let label_x = content_x + key_w + KEY_GAP * s;
            let max_label_w = (content_x + row_w - label_x).max(0.0);
            let trunc = truncate_label(&cmd.label, MAX_LABEL_CHARS);
            let mut label_buf = shape_fitted(
                font_system,
                ctx,
                &trunc,
                foreground,
                false,
                true,
                max_label_w,
            );
            frame.draw_text(
                font_system,
                swash_cache,
                &mut label_buf,
                (label_x.round() as i32, text_y),
                foreground,
            );

            cur_y += row_h;
        }
    }

    // Placed at the center of the active editor pane by default, falling back to top_anchored:
    let (mut panel_x, mut panel_y) = match view.anchor_rect {
        Some(rect) => centered_in_rect(surface_w, surface_h, panel_w, panel_h, rect),
        None => top_anchored(surface_w, surface_h, panel_w, panel_h, 0.0),
    };
    if let Some((dx, dy)) = view.offset {
        panel_x = (panel_x + dx)
            .clamp(0.0, (surface_w - panel_w).max(0.0))
            .floor();
        panel_y = (panel_y + dy)
            .clamp(0.0, (surface_h - panel_h).max(0.0))
            .floor();
    }

    frame.into_image(panel_x, panel_y)
}

/// Compute surface hit regions for all interactive rows in the transient window.
pub(in crate::renderer) fn transient_item_hits(
    _font_system: &mut FontSystem,
    ctx: &FontCtx,
    view: &TransientView,
    card_rect: (f32, f32, f32, f32),
) -> Vec<TransientHit> {
    let (panel_x, panel_y, panel_w, panel_h) = card_rect;
    let s = ctx.font_size / BASE_FONT_PX;
    let margin = DROPDOWN_SHADOW;

    let header_h = if view.title.is_empty() {
        0.0
    } else {
        TITLE_BAR_TOP_PAD + title_bar_height(ctx)
    };
    let row_h = (ctx.line_height + 2.0 * ROW_PAD_Y * s).round();
    let section_hdr_h = (ctx.line_height + 4.0 * s).round();

    let body_top = margin + header_h + 4.0 * s;
    let body_bottom = margin + panel_h - PANEL_PAD_BOTTOM * s;
    let has_sections = !view.switches.is_empty() && !view.commands.is_empty();
    let row_w = panel_w - 2.0 * BODY_PAD_X * s;

    let mut hits = Vec::new();
    let mut cur_y = body_top;

    // Switches hits
    if !view.switches.is_empty() {
        if has_sections {
            cur_y += section_hdr_h;
        }
        for (index, sw) in view.switches.iter().enumerate() {
            if cur_y + row_h > body_bottom {
                break;
            }
            hits.push(TransientHit {
                index,
                is_switch: true,
                key: sw.key.clone(),
                rect: (
                    panel_x + BODY_PAD_X * s,
                    panel_y + (cur_y - margin),
                    row_w,
                    row_h,
                ),
            });
            cur_y += row_h;
        }
    }

    // Divider gap
    if has_sections {
        cur_y += SECTION_GAP_Y * s + 1.0 + SECTION_GAP_Y * s;
    }

    // Commands hits
    if !view.commands.is_empty() {
        if has_sections {
            cur_y += section_hdr_h;
        }
        for (index, cmd) in view.commands.iter().enumerate() {
            if cur_y + row_h > body_bottom {
                break;
            }
            hits.push(TransientHit {
                index,
                is_switch: false,
                key: cmd.key.clone(),
                rect: (
                    panel_x + BODY_PAD_X * s,
                    panel_y + (cur_y - margin),
                    row_w,
                    row_h,
                ),
            });
            cur_y += row_h;
        }
    }

    hits
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::super::view::{TransientCommand, TransientSwitch};
    use super::*;

    fn sample_font_ctx<'a>() -> FontCtx<'a> {
        FontCtx {
            bold_weight: None,
            cell_h: 20.0,
            cell_w: 10.0,
            family: None,
            field_h: 24.0,
            font_has_bold: true,
            font_size: 13.0,
            line_height: 18.0,
            normal_weight: None,
        }
    }

    #[test]
    fn test_transient_fits_content_width_and_height() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 1200.0;
        let surface_h = 800.0;

        let view = TransientView::new(
            "Commit",
            vec![
                TransientCommand::new("c", "commit, in an editor"),
                TransientCommand::new("a", "amend, in an editor"),
            ],
            vec![
                TransientSwitch::new("-a", "Stage all modified files", "--all", true),
                TransientSwitch::new("-e", "Allow empty commit", "--allow-empty", false),
            ],
        );

        let image = transient_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );

        // Content-fitted: width and height should be reasonable, far smaller than 80% screen
        let card_w = image.width as f32 - 2.0 * DROPDOWN_SHADOW;
        let card_h = image.height as f32 - 2.0 * DROPDOWN_SHADOW;
        assert!(card_w >= MIN_PANEL_W, "meets minimum width");
        assert!(
            card_w <= MAX_PANEL_W,
            "fitted nicely, not blown out: {card_w}"
        );
        assert!(card_h < 350.0, "fitted nicely in height: {card_h}");

        // Centered horizontally, positioned at top of window like VS Code
        let center_x = image.x + image.width as f32 / 2.0;
        assert!((center_x - surface_w / 2.0).abs() < 2.0);
        let panel_y = image.y + DROPDOWN_SHADOW;
        assert_eq!(panel_y, 0.0);
    }

    #[test]
    fn test_transient_item_hits_produces_hits_for_switches_and_commands() {
        let mut font_system = FontSystem::new();
        let ctx = sample_font_ctx();
        let view = TransientView::new(
            "Log",
            vec![
                TransientCommand::new("l", "current branch"),
                TransientCommand::new("a", "all branches"),
            ],
            vec![
                TransientSwitch::new("-f", "Follow only first parent", "--first-parent", false),
                TransientSwitch::new("-r", "Reverse", "--reverse", true),
            ],
        );

        let card_rect = (200.0, 150.0, 400.0, 250.0);
        let hits = transient_item_hits(&mut font_system, &ctx, &view, card_rect);

        assert_eq!(hits.len(), 4);
        assert!(hits[0].is_switch);
        assert_eq!(hits[0].key, "-f");
        assert!(hits[1].is_switch);
        assert_eq!(hits[1].key, "-r");
        assert!(!hits[2].is_switch);
        assert_eq!(hits[2].key, "l");
        assert!(!hits[3].is_switch);
        assert_eq!(hits[3].key, "a");

        // Rects are within card bounds
        for hit in &hits {
            assert!(hit.rect.0 >= card_rect.0);
            assert!(hit.rect.1 >= card_rect.1);
            assert!(hit.rect.2 > 0.0);
            assert!(hit.rect.3 > 0.0);
        }
    }

    #[test]
    fn test_rebase_transient_rgba_and_hits() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 800.0;
        let surface_h = 600.0;

        let view = TransientView::new(
            "Rebase",
            vec![
                TransientCommand::new("i", "interactive, from the commit at point"),
                TransientCommand::new("u", "onto the upstream"),
                TransientCommand::new("c", "continue"),
                TransientCommand::new("s", "skip"),
                TransientCommand::new("x", "abort"),
            ],
            vec![],
        );

        let image = transient_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );
        let card_rect = (
            image.x + DROPDOWN_SHADOW,
            image.y + DROPDOWN_SHADOW,
            image.width as f32 - 2.0 * DROPDOWN_SHADOW,
            image.height as f32 - 2.0 * DROPDOWN_SHADOW,
        );
        let hits = transient_item_hits(&mut font_system, &ctx, &view, card_rect);
        assert_eq!(hits.len(), 5);
    }

    #[test]
    fn test_radio_bullet_switches_render_correctly() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let surface_w = 800.0;
        let surface_h = 600.0;

        let view = TransientView::new(
            "Merge",
            vec![TransientCommand::new("m", "merge commit")],
            vec![
                TransientSwitch::radio("-f", "Fast-forward only", "--ff-only", true),
                TransientSwitch::radio("-n", "No fast-forward", "--no-ff", false),
            ],
        );

        let image = transient_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view,
            surface_w,
            surface_h,
        );
        assert!(image.width > 0 && image.height > 0);

        let card_rect = (
            image.x + DROPDOWN_SHADOW,
            image.y + DROPDOWN_SHADOW,
            image.width as f32 - 2.0 * DROPDOWN_SHADOW,
            image.height as f32 - 2.0 * DROPDOWN_SHADOW,
        );
        let hits = transient_item_hits(&mut font_system, &ctx, &view, card_rect);
        assert_eq!(hits.len(), 3);
        assert!(hits[0].is_switch);
        assert_eq!(hits[0].key, "-f");
    }

    #[test]
    fn test_transient_matrix_boundary_resolutions_never_panic() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();

        let variations = vec![
            // Empty
            TransientView::new("", vec![], vec![]),
            // Commands only
            TransientView::new(
                "Commands Only",
                vec![
                    TransientCommand::new("a", "alpha"),
                    TransientCommand::new("b", "beta"),
                ],
                vec![],
            ),
            // Switches only
            TransientView::new(
                "Switches Only",
                vec![],
                vec![
                    TransientSwitch::new("-x", "extra", "--extra", false),
                    TransientSwitch::radio("-r", "radio", "--radio", true),
                ],
            ),
            // Mixed
            TransientView::new(
                "Mixed Very Long Title That May Exceed Normal Boundaries",
                vec![TransientCommand::new(
                    "c",
                    "commit with very long description that might need truncating",
                )],
                vec![
                    TransientSwitch::new(
                        "-a",
                        "stage all files including untracked and modified",
                        "--all",
                        true,
                    ),
                    TransientSwitch::radio("-f", "force with lease", "--force-with-lease", false),
                ],
            ),
        ];

        let surface_sizes = [
            (1920.0, 1080.0),
            (800.0, 600.0),
            (480.0, 320.0),
            (300.0, 200.0),
            (150.0, 100.0),
        ];

        for view in &variations {
            for &(w, h) in &surface_sizes {
                let image = transient_rgba(&mut font_system, &mut swash, &ctx, &theme, view, w, h);
                assert!(image.width > 0);
                assert!(image.height > 0);
                let card_rect = (
                    image.x + DROPDOWN_SHADOW,
                    image.y + DROPDOWN_SHADOW,
                    image.width as f32 - 2.0 * DROPDOWN_SHADOW,
                    image.height as f32 - 2.0 * DROPDOWN_SHADOW,
                );
                let hits = transient_item_hits(&mut font_system, &ctx, view, card_rect);
                if h >= 320.0 {
                    assert_eq!(hits.len(), view.switches.len() + view.commands.len());
                } else {
                    assert!(hits.len() <= view.switches.len() + view.commands.len());
                }
                for hit in &hits {
                    assert!(hit.rect.0 >= card_rect.0);
                    assert!(hit.rect.1 >= card_rect.1);
                }
            }
        }
    }

    #[test]
    fn test_transient_selection_rendering_and_top_anchoring() {
        let mut font_system = FontSystem::new();
        let mut swash = SwashCache::new();
        let theme = Theme::dark();
        let ctx = sample_font_ctx();

        let view_sel0 = TransientView::new(
            "Log",
            vec![
                TransientCommand::new("l", "current branch"),
                TransientCommand::new("a", "all branches"),
            ],
            vec![TransientSwitch::new(
                "-f",
                "Follow only first parent",
                "--first-parent",
                false,
            )],
        )
        .with_selected(0);

        let view_sel1 = view_sel0.clone().with_selected(1);

        let img0 = transient_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view_sel0,
            800.0,
            600.0,
        );
        let img1 = transient_rgba(
            &mut font_system,
            &mut swash,
            &ctx,
            &theme,
            &view_sel1,
            800.0,
            600.0,
        );

        // Anchored at top (y = 0 relative to card, so img.y = -DROPDOWN_SHADOW)
        assert_eq!(img0.y, -DROPDOWN_SHADOW);
        assert_eq!(img1.y, -DROPDOWN_SHADOW);

        // Different selection produces different RGBA pixels (hover band drawn)
        assert_ne!(img0.rgba, img1.rgba);
    }
}

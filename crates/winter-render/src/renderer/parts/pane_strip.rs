//! A pane's own tab strip: its tab pills, icons and buttons painted into an
//! RGBA buffer, and the split dividers drawn around it.

use glyphon::{Color, FontSystem, SwashCache};

use super::{SQUARE_CORNER, TAB_CLOSE_SHIFT_RIGHT_PX, TAB_LABEL_SHIFT_UP_PX};
use crate::image::ImagePlacement;
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{
    blit_svg, button_pill, composite_buffer, fill_line_segment, fill_rounded_rect, pill_center,
    shape_chrome_line, truncate_label,
};
use crate::renderer::raster::upload::PaneStripKey;
use crate::renderer::raster::{DIM_FACTOR, ICON_GAP_CELLS, ICON_HEIGHT_RATIO};
use crate::renderer::GpuRenderer;
use crate::tabbar::{
    self, PaneStripDropTarget, PaneStripEdges, PaneStripHit, PaneStripLayout, PaneTab, Region,
    TAB_H_PAD_CELLS,
};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// Active tab pill background color (#2a2f31): the pane's own background, so
/// the active tab reads as open into its pane.
const PANE_STRIP_ACTIVE_BG: Rgb = Rgb::new(0x2a, 0x2f, 0x31);

/// Non-occupied band background color (#1f2527).
const PANE_STRIP_BAND_BG: Rgb = Rgb::new(0x1f, 0x25, 0x27);

/// Height of the top accent indicator bar on selected tab pills, in pixels.
pub(crate) const PANE_STRIP_ACTIVE_INDICATOR_PX: f32 = 2.0;

/// Border color mix strength for unfocused active tab outline.
const UNFOCUSED_ACTIVE_BORDER_MIX: f32 = 0.15;

/// How far the divider between two inactive tabs leans from their color to
/// the bright text color: enough to read as an edge, short of a border.
const INACTIVE_SEPARATOR_MIX: f32 = 0.18;

/// The modified dot's diameter as a fraction of the cell height: a little
/// smaller than the close button's cross, so it reads as a mark, not a key.
const MODIFIED_DOT_RATIO: f32 = 0.3;

// ========================================================================
// Pane strip painting
// ========================================================================

/// Rasterize a pane-level tab strip into an RGBA buffer.
#[allow(clippy::too_many_arguments)]
pub(in crate::renderer) fn rasterize_pane_strip_rgba(
    tabs: &[PaneTab],
    layout: &PaneStripLayout,
    width: u32,
    height: u32,
    cw: f32,
    ch: f32,
    theme: &Theme,
    hover: PaneStripHit,
    separator: Rgb,
    drop_target: PaneStripDropTarget,
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
) -> Vec<u8> {
    let canvas = (width, height);
    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // 1. Fill recessed non-occupied band with lighter color #282d30.
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (0.0, 0.0, width as f32, height as f32),
        0.0,
        PANE_STRIP_BAND_BG,
        1.0,
    );

    let tab_vpad_top = crate::PANE_STRIP_TOP_PAD_PX;
    let tab_vpad_bottom = crate::PANE_STRIP_SEPARATOR_PX + crate::PANE_STRIP_SEPARATOR_INSET_PX;
    let inactive_bg = mix_rgb(PANE_STRIP_BAND_BG, PANE_STRIP_ACTIVE_BG, 0.4);
    let unfocused_active_bg = mix_rgb(PANE_STRIP_ACTIVE_BG, PANE_STRIP_BAND_BG, DIM_FACTOR);
    // Every button's icon and square hover pill is centered vertically in the
    // strip, between its top edge and the separator along its bottom, which is
    // also where the tab labels center, so the close, new-tab and scroll
    // buttons line up with each other and with the text.
    let pill_mid_y = |r: Region| r.y + (r.h - tab_vpad_bottom) / 2.0;

    let dark_theme = {
        let bg = theme.tabbar_bg;
        (bg.r as f32 + bg.g as f32 + bg.b as f32) / (3.0 * 255.0) < 0.5
    };
    let bright_target = if dark_theme {
        Rgb::new(255, 255, 255)
    } else {
        theme.foreground
    };
    let active_text_color_rgb = mix_rgb(theme.tab_active_fg, bright_target, 0.35);
    let active_text_color = active_text_color_rgb.to_glyphon();
    let unfocused_active_text_color =
        mix_rgb(active_text_color_rgb, PANE_STRIP_BAND_BG, DIM_FACTOR).to_glyphon();
    let inactive_text_color = mix_rgb(theme.ansi[8], bright_target, 0.62).to_glyphon();

    // 2. Tab pills and their contents (text + close icon).
    for (i, tab) in tabs.iter().enumerate() {
        let tab_reg = layout.tabs[i];
        if tab_reg.w <= 0.0 {
            continue;
        }

        let is_hovered =
            matches!(hover, PaneStripHit::Tab(id) | PaneStripHit::CloseTab(id) if id == tab.tab_id);
        let pill_color = if tab.dragging {
            mix_rgb(PANE_STRIP_BAND_BG, PANE_STRIP_ACTIVE_BG, 0.25)
        } else if tab.active {
            if tab.focused {
                PANE_STRIP_ACTIVE_BG
            } else if is_hovered {
                mix_rgb(unfocused_active_bg, PANE_STRIP_ACTIVE_BG, 0.4)
            } else {
                unfocused_active_bg
            }
        } else if is_hovered {
            mix_rgb(inactive_bg, PANE_STRIP_ACTIVE_BG, 0.55)
        } else {
            inactive_bg
        };

        let pill_alpha = if tab.dragging { 0.45 } else { 1.0 };
        let pill_bounds = (
            tab_reg.x + tabbar::TAB_GAP_PX / 2.0,
            tab_reg.y + tab_vpad_top,
            tab_reg.w - tabbar::TAB_GAP_PX,
            tab_reg.h - tab_vpad_top - tab_vpad_bottom,
        );
        fill_rounded_rect(
            &mut rgba,
            canvas,
            pill_bounds,
            SQUARE_CORNER,
            pill_color,
            pill_alpha,
        );

        // Decorate selected tab: an accent bar along the top edge of the active tab.
        // In the focused pane, it uses a darker and more saturated royal blue accent.
        // In the non-focused pane, it uses a dimmed accent color and a subtle hairline border
        // so the selected tab remains immediately recognizable without competing for focus.
        if tab.active && !tab.dragging {
            let active_indicator = theme.active_indicator;
            // The bar starts at the strip's top edge rather than the pill's,
            // so it meets the border above (the title bar's, or a split's)
            // with no band-colored row between them, and keeps its own
            // thickness: it covers the pill's top rows rather than adding to
            // them.
            let indicator = (
                pill_bounds.0,
                tab_reg.y,
                pill_bounds.2,
                PANE_STRIP_ACTIVE_INDICATOR_PX,
            );
            if tab.focused {
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    indicator,
                    SQUARE_CORNER,
                    active_indicator,
                    1.0,
                );
            } else {
                let unfocused_border = mix_rgb(
                    unfocused_active_bg,
                    bright_target,
                    UNFOCUSED_ACTIVE_BORDER_MIX,
                );
                let unfocused_indicator = mix_rgb(active_indicator, PANE_STRIP_BAND_BG, DIM_FACTOR);
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    indicator,
                    SQUARE_CORNER,
                    unfocused_indicator,
                    1.0,
                );
                // 1px left and right borders to crisply delineate the unfocused active tab from neighbors
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (
                        pill_bounds.0,
                        tab_reg.y + PANE_STRIP_ACTIVE_INDICATOR_PX,
                        1.0,
                        pill_bounds.1 + pill_bounds.3 - tab_reg.y - PANE_STRIP_ACTIVE_INDICATOR_PX,
                    ),
                    SQUARE_CORNER,
                    unfocused_border,
                    1.0,
                );
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (
                        pill_bounds.0 + pill_bounds.2 - 1.0,
                        tab_reg.y + PANE_STRIP_ACTIVE_INDICATOR_PX,
                        1.0,
                        pill_bounds.1 + pill_bounds.3 - tab_reg.y - PANE_STRIP_ACTIVE_INDICATOR_PX,
                    ),
                    SQUARE_CORNER,
                    unfocused_border,
                    1.0,
                );
            }
        }

        // Close button hover highlight
        let close_reg = layout.closes[i];
        let close_pill = button_pill(
            close_reg.x + close_reg.w / 2.0 + TAB_CLOSE_SHIFT_RIGHT_PX,
            pill_mid_y(close_reg),
            ch,
        );
        let is_close_hovered = hover == PaneStripHit::CloseTab(tab.tab_id);
        if is_close_hovered && !tab.dragging {
            let close_hover_color = mix_rgb(pill_color, theme.foreground, 0.18);
            fill_rounded_rect(
                &mut rgba,
                canvas,
                close_pill,
                SQUARE_CORNER,
                close_hover_color,
                1.0,
            );
        }

        // Close button vector 'X'
        {
            let close_color = if tab.dragging {
                mix_rgb(PANE_STRIP_BAND_BG, theme.ansi[8], 0.4)
            } else if is_close_hovered || is_hovered || (tab.active && tab.focused) {
                theme.foreground
            } else if tab.active {
                mix_rgb(theme.foreground, PANE_STRIP_BAND_BG, DIM_FACTOR)
            } else {
                theme.ansi[8]
            };
            let close_alpha = if tab.dragging { 0.45 } else { 1.0 };
            let (cx, cy) = pill_center(close_pill);
            // Unwritten changes are a dot in the close button's place, which
            // turns back into the button once the pointer is over the tab, so
            // the one click that would lose them is never where the eye
            // expected a mark.
            if tab.modified && !is_hovered && !is_close_hovered {
                let side = (ch * MODIFIED_DOT_RATIO).round().max(2.0);
                let dot = (cx - side / 2.0, cy - side / 2.0, side, side);
                fill_rounded_rect(&mut rgba, canvas, dot, side / 2.0, close_color, close_alpha);
            } else {
                let size = (ch * 0.35).round();
                let half = size / 2.0;
                let thickness = 1.25;
                fill_line_segment(
                    &mut rgba,
                    canvas,
                    (cx - half, cy - half),
                    (cx + half, cy + half),
                    thickness,
                    close_color,
                    close_alpha,
                );
                fill_line_segment(
                    &mut rgba,
                    canvas,
                    (cx - half, cy + half),
                    (cx + half, cy - half),
                    thickness,
                    close_color,
                    close_alpha,
                );
            }
        }

        // Tab label
        let pad = cw * 0.4;
        let title_x = tab_reg.x + TAB_H_PAD_CELLS * cw;
        let title_end = close_reg.x;
        let mut text_x = title_x + pad;
        let text_color = if tab.dragging {
            let dimmed_rgb = mix_rgb(PANE_STRIP_BAND_BG, theme.ansi[8], 0.5);
            Color::rgba(dimmed_rgb.r, dimmed_rgb.g, dimmed_rgb.b, 115)
        } else if tab.active {
            if tab.focused {
                active_text_color
            } else {
                unfocused_active_text_color
            }
        } else {
            inactive_text_color
        };
        if let Some(svg) = &tab.icon {
            let icon_px = (ch * ICON_HEIGHT_RATIO).round().max(1.0) as u32;
            let icon_y = tab_reg.y
                + tab_vpad_top
                + (tab_reg.h - tab_vpad_top - tab_vpad_bottom - icon_px as f32) / 2.0
                - TAB_LABEL_SHIFT_UP_PX;
            blit_svg(
                &mut rgba,
                canvas,
                svg,
                (text_x.round() as i32, icon_y.round() as i32),
                icon_px,
            );
            text_x += icon_px as f32 + cw * ICON_GAP_CELLS;
        }
        let avail = (title_end - text_x).max(cw);
        let max_chars = (avail / cw).floor() as usize;
        let labeled = format!("{}: {}", i + 1, tab.title);
        let title = truncate_label(&labeled, max_chars);
        let mut buffer = shape_chrome_line(font_system, ctx, &title, text_color, tab.active, true);
        let vcenter_y = tab_reg.y
            + tab_vpad_top
            + (tab_reg.h - tab_vpad_top - tab_vpad_bottom - ctx.line_height) / 2.0
            - TAB_LABEL_SHIFT_UP_PX;
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &mut buffer,
            (text_x.round() as i32, vcenter_y.round() as i32),
            text_color,
        );
    }

    // Between two inactive tabs, a hairline down the tabs' full height:
    // their pills share one color, and the half-pixel gap between them alone
    // leaves the strip reading as one long tab. The active tab draws its own
    // edges.
    let tab_divider = mix_rgb(inactive_bg, bright_target, INACTIVE_SEPARATOR_MIX);
    for (pair, regions) in tabs.windows(2).zip(layout.tabs.windows(2)) {
        let quiet = |tab: &PaneTab| !tab.active && !tab.dragging;
        let (left, right) = (regions[0], regions[1]);
        if !quiet(&pair[0]) || !quiet(&pair[1]) || left.w <= 0.0 || right.w <= 0.0 {
            continue;
        }
        let top = left.y + tab_vpad_top;
        let height = left.h - tab_vpad_top - tab_vpad_bottom;
        fill_rounded_rect(
            &mut rgba,
            canvas,
            ((right.x - 0.5).round(), top, 1.0, height),
            SQUARE_CORNER,
            tab_divider,
            1.0,
        );
    }

    // 3. New-tab button (+)
    if let Some(nt) = layout.new_tab {
        let pill = button_pill(nt.x + nt.w / 2.0, pill_mid_y(nt), ch);
        if hover == PaneStripHit::NewTab {
            let new_tab_hover = mix_rgb(PANE_STRIP_BAND_BG, theme.foreground, 0.12);
            fill_rounded_rect(&mut rgba, canvas, pill, SQUARE_CORNER, new_tab_hover, 1.0);
        }

        // The '+' is two pixel-aligned bars centered exactly on the pill, so
        // its edges are crisp and its arms are even: a stroke straddling a
        // pixel boundary would leave a half-lit row on one side. The bars'
        // thickness and length share the pill's parity so that holds for an
        // odd pill size as well.
        let (_, _, side, _) = pill;
        let (cx, cy) = pill_center(pill);
        let odd = (side as u32 % 2) as f32;
        let thickness = 2.0 - odd;
        let len = ((ch * 0.35).round() + 2.0).max(2.0) + odd;
        let plus_color = if hover == PaneStripHit::NewTab {
            theme.foreground
        } else {
            theme.ansi[8]
        };
        for bar in [
            (cx - len / 2.0, cy - thickness / 2.0, len, thickness),
            (cx - thickness / 2.0, cy - len / 2.0, thickness, len),
        ] {
            fill_rounded_rect(&mut rgba, canvas, bar, SQUARE_CORNER, plus_color, 1.0);
        }
    }

    // 4. Scroll chevrons if paginated
    let tab_count = tabs.len();
    let active_idx = tabs.iter().position(|t| t.active).unwrap_or(0);
    for (region, points_left, hit) in [
        (layout.scroll_left, true, PaneStripHit::ScrollLeft),
        (layout.scroll_right, false, PaneStripHit::ScrollRight),
    ] {
        let Some(r) = region else { continue };
        let active = if points_left {
            active_idx > 0
        } else {
            active_idx + 1 < tab_count
        };
        let hovered = active && hover == hit;
        let pill = button_pill(r.x + r.w / 2.0, pill_mid_y(r), ch);
        if hovered {
            fill_rounded_rect(
                &mut rgba,
                canvas,
                pill,
                SQUARE_CORNER,
                mix_rgb(PANE_STRIP_BAND_BG, theme.foreground, 0.12),
                1.0,
            );
        }
        let color = if !active {
            mix_rgb(PANE_STRIP_BAND_BG, theme.ansi[8], 0.4)
        } else if hovered {
            theme.foreground
        } else {
            theme.ansi[8]
        };
        let (cx, cy) = pill_center(pill);
        let half = (ch * 0.2).round();
        let reach = half * 0.6;
        let thickness = 1.5;
        let tip_x = if points_left { cx - reach } else { cx + reach };
        let back_x = if points_left { cx + reach } else { cx - reach };
        fill_line_segment(
            &mut rgba,
            canvas,
            (back_x, cy - half),
            (tip_x, cy),
            thickness,
            color,
            1.0,
        );
        fill_line_segment(
            &mut rgba,
            canvas,
            (tip_x, cy),
            (back_x, cy + half),
            thickness,
            color,
            1.0,
        );
    }

    // 5. The line between the strip and the pane below it, with a row of the
    // pane's own color under it.
    let inset = crate::PANE_STRIP_SEPARATOR_INSET_PX;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (0.0, height as f32 - inset, width as f32, inset),
        0.0,
        theme.background,
        1.0,
    );
    let sep_color = if drop_target.is_target_pane {
        theme.active_indicator
    } else {
        separator
    };
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (
            0.0,
            height as f32 - inset - crate::PANE_STRIP_SEPARATOR_PX,
            width as f32,
            crate::PANE_STRIP_SEPARATOR_PX,
        ),
        0.0,
        sep_color,
        1.0,
    );
    if drop_target.is_target_pane {
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (
                0.0,
                height as f32 - inset - crate::PANE_STRIP_SEPARATOR_PX,
                width as f32,
                crate::PANE_STRIP_SEPARATOR_PX + inset,
            ),
            0.0,
            theme.active_indicator,
            1.0,
        );
    }

    // 6. Drop insertion indicator bar with serif caps at top and bottom.
    if let Some(ix) = drop_target.indicator_x {
        let bar_w = 3.0;
        let bar_x = (ix - bar_w / 2.0).clamp(0.0, (width as f32 - bar_w).max(0.0));
        let bar_top = tab_vpad_top;
        let bar_h = (height as f32 - tab_vpad_top - tab_vpad_bottom).max(0.0);
        let indicator_color = theme.active_indicator;

        fill_rounded_rect(
            &mut rgba,
            canvas,
            (bar_x, bar_top, bar_w, bar_h),
            SQUARE_CORNER,
            indicator_color,
            1.0,
        );

        let cap_w = 7.0;
        let cap_h = 2.0;
        let cap_x = (ix - cap_w / 2.0).clamp(0.0, (width as f32 - cap_w).max(0.0));
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (cap_x, bar_top, cap_w, cap_h),
            SQUARE_CORNER,
            indicator_color,
            1.0,
        );
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (cap_x, (bar_top + bar_h - cap_h).max(bar_top), cap_w, cap_h),
            SQUARE_CORNER,
            indicator_color,
            1.0,
        );
    }

    rgba
}

/// A rasterized pane strip with the split dividers around it, and how far
/// the result reaches past the strip's own area.
struct DividedStrip {
    height: u32,
    /// Columns added on the left, where the strip's left divider lies.
    left: u32,
    rgba: Vec<u8>,
    /// Rows added on top, where the strip's top divider lies.
    top: u32,
    width: u32,
}

/// Put the dividers `edges` names around the `width` x `height` strip `rgba`.
///
/// A divider is centered on the edge a strip shares with its neighbor, so the
/// part of it before that edge (`ceil(divider_width / 2)` pixels) lies outside
/// the strip's own area on the left and top. The result grows by that much
/// there, which puts the divider on the same pixels as the ones drawn between
/// the panes below. Each divider is drawn whole by one of the two strips that
/// share it; the caller unsets the side the neighbor draws.
fn add_strip_dividers(
    rgba: &[u8],
    width: u32,
    height: u32,
    edges: PaneStripEdges,
    divider_width: f32,
    color: Rgb,
) -> DividedStrip {
    let before = (divider_width / 2.0).ceil() as u32;
    let after = (divider_width / 2.0).floor() as u32;
    let left = if edges.left { before } else { 0 };
    let top = if edges.top { before } else { 0 };
    let (out_w, out_h) = (width + left, height + top);
    let mut out = vec![0u8; (out_w * out_h * 4) as usize];
    let row_bytes = (width * 4) as usize;
    for row in 0..height {
        let src = row as usize * row_bytes;
        let dst = (((row + top) * out_w + left) * 4) as usize;
        out[dst..dst + row_bytes].copy_from_slice(&rgba[src..src + row_bytes]);
    }
    let canvas = (out_w, out_h);
    let (w, h) = (out_w as f32, out_h as f32);
    let mut fill = |bounds: (f32, f32, f32, f32)| {
        fill_rounded_rect(&mut out, canvas, bounds, 0.0, color, 1.0);
    };
    if edges.left {
        fill((0.0, 0.0, (left + after) as f32, h));
    }
    if edges.right {
        fill((w - before as f32, 0.0, before as f32, h));
    }
    if edges.top {
        fill((0.0, 0.0, w, (top + after) as f32));
    }
    DividedStrip {
        height: out_h,
        left,
        rgba: out,
        top,
        width: out_w,
    }
}

// ========================================================================
// GpuRenderer: pane strips
// ========================================================================

impl GpuRenderer {
    /// Rasterize a pane tab strip and upload it to the GPU texture with ID `texture_id`.
    /// Returns the corresponding `ImagePlacement`.
    #[allow(clippy::too_many_arguments)]
    pub fn rasterize_pane_strip(
        &mut self,
        texture_id: u64,
        tabs: &[PaneTab],
        layout: &PaneStripLayout,
        width: u32,
        height: u32,
        hover: PaneStripHit,
        edges: PaneStripEdges,
        drop_target: PaneStripDropTarget,
        x: f32,
        y: f32,
    ) -> ImagePlacement {
        let context = self.raster_context(width as f32, height as f32);
        let matches = |key: &PaneStripKey| {
            key.tabs == tabs
                && key.layout == *layout
                && (key.width, key.height) == (width, height)
                && key.hover == hover
                && key.edges == edges
                && key.drop_target == drop_target
                && key.origin == (x, y)
        };
        if let Some(cached) = self
            .overlay_caches
            .pane_strips
            .get(&texture_id)
            .and_then(|cache| cache.hit(&context, matches))
        {
            if let Some(&placement) = cached.first() {
                return placement;
            }
        }
        let divider_color = self.split_divider_color();
        let (cell_w, cell_h) = (self.cell_width, self.cell_height);
        let painter = self.chrome_painter();
        let rgba = rasterize_pane_strip_rgba(
            tabs,
            layout,
            width,
            height,
            cell_w,
            cell_h,
            painter.theme,
            hover,
            divider_color,
            drop_target,
            painter.font_system,
            painter.swash_cache,
            &painter.ctx,
        );
        let strip = add_strip_dividers(
            &rgba,
            width,
            height,
            edges,
            self.divider_width,
            divider_color,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            texture_id,
            &strip.rgba,
            strip.width,
            strip.height,
        );
        let placement = ImagePlacement {
            alpha: 1.0,
            height: strip.height as f32,
            id: texture_id,
            u_max: 1.0,
            u_min: 0.0,
            v_max: 1.0,
            v_min: 0.0,
            width: strip.width as f32,
            x: x - strip.left as f32,
            y: y - strip.top as f32,
        };
        let key = PaneStripKey {
            drop_target,
            edges,
            height,
            hover,
            layout: layout.clone(),
            origin: (x, y),
            tabs: tabs.to_vec(),
            width,
        };
        self.overlay_caches
            .pane_strips
            .entry(texture_id)
            .or_default()
            .store(context, key, vec![placement]);
        placement
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::raster::test_support::*;

    #[test]
    fn test_a_one_pixel_divider_lies_on_the_pixel_before_the_shared_edge() {
        // A 4x3 strip of opaque white; the divider red. Its left divider must
        // sit one column left of the strip, where the pane divider's pixel is,
        // and its top divider one row above.
        let base = vec![255u8; 4 * 3 * 4];
        let red = Rgb::new(255, 0, 0);
        let edges = PaneStripEdges {
            left: true,
            right: false,
            top: true,
        };
        let strip = add_strip_dividers(&base, 4, 3, edges, 1.0, red);
        assert_eq!((strip.width, strip.height), (5, 4));
        assert_eq!((strip.left, strip.top), (1, 1));
        let px = |x: usize, y: usize| {
            let i = (y * 5 + x) * 4;
            [strip.rgba[i], strip.rgba[i + 1], strip.rgba[i + 2]]
        };
        assert_eq!(px(0, 2), [255, 0, 0], "left divider column");
        assert_eq!(px(3, 0), [255, 0, 0], "top divider row");
        assert_eq!(px(1, 1), [255, 255, 255], "strip content starts after both");
    }

    #[test]
    fn test_pane_strip_button_hover_pills_are_equal_squares_centered_in_the_strip() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 700;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![
            PaneTab {
                active: true,
                dragging: false,
                focused: true,
                icon: None,
                modified: false,
                tab_id: 1,
                title: "zsh".into(),
            },
            PaneTab {
                active: false,
                dragging: false,
                focused: false,
                icon: None,
                modified: false,
                tab_id: 2,
                title: "a".into(),
            },
        ];
        // Narrow enough to paginate, so the scroll arrows are there to hover.
        let narrow = 300;
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let paged = layout_pane_strip(&tabs, narrow as f32, h as f32, CELL_W, CELL_H, 0);
        assert!(paged.scroll_right.is_some(), "the strip should paginate");
        let mut paint = |layout: &PaneStripLayout, width, hover| {
            rasterize_pane_strip_rgba(
                &tabs,
                layout,
                width,
                h,
                CELL_W,
                CELL_H,
                &theme,
                hover,
                Rgb::new(60, 60, 60),
                PaneStripDropTarget::default(),
                &mut fonts,
                &mut swash,
                &ctx,
            )
        };
        let idle = paint(&layout, w, PaneStripHit::None);
        let idle_paged = paint(&paged, narrow, PaneStripHit::None);
        let boxes = [
            changed_box(&idle, &paint(&layout, w, PaneStripHit::CloseTab(1)), w),
            changed_box(&idle, &paint(&layout, w, PaneStripHit::NewTab), w),
            changed_box(
                &idle_paged,
                &paint(&paged, narrow, PaneStripHit::ScrollRight),
                narrow,
            ),
        ];
        // The strip's content ends at the separator, two rows above the
        // bottom, so its middle (in edge coordinates) is the pills' middle.
        let pill_mid = (h - 2) as f32 / 2.0;
        for (x, y, bw, bh) in boxes {
            assert_eq!(bw, bh, "hover pill at ({x}, {y}) is {bw}x{bh}, not square");
            assert!(
                ((y as f32 + bh as f32 / 2.0) - pill_mid).abs() <= 0.5,
                "hover pill at y {y} is off the strip center {pill_mid}"
            );
            assert!(bw as f32 <= CELL_H, "hover pill {bw}px is over a cell tall");
        }
        assert_eq!(
            boxes[0].2, boxes[1].2,
            "close and new-tab pills differ in size"
        );
        assert_eq!(
            boxes[0].2, boxes[2].2,
            "close and scroll pills differ in size"
        );
        assert_eq!(boxes[0].2 as f32, CELL_H, "pills are not one cell tall");
    }

    #[test]
    fn test_two_inactive_tabs_have_a_divider_and_the_active_tab_needs_none() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 600;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tab = |id: u64, active: bool| PaneTab {
            active,
            dragging: false,
            focused: true,
            icon: None,
            modified: false,
            tab_id: id,
            title: format!("t{id}"),
        };
        let tabs = vec![tab(1, true), tab(2, false), tab(3, false)];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let rgba = rasterize_pane_strip_rgba(
            &tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );
        let at = |x: f32| {
            let y = (crate::PANE_STRIP_TOP_PAD_PX + h as f32) as u32 / 2;
            let i = ((y * w + (x - 0.5).round() as u32) * 4) as usize;
            Rgb::new(rgba[i], rgba[i + 1], rgba[i + 2])
        };
        let inactive_bg = mix_rgb(PANE_STRIP_BAND_BG, PANE_STRIP_ACTIVE_BG, 0.4);
        let divider = mix_rgb(inactive_bg, Rgb::new(255, 255, 255), INACTIVE_SEPARATOR_MIX);
        assert_eq!(
            at(layout.tabs[2].x),
            divider,
            "between the two inactive tabs"
        );
        let top = crate::PANE_STRIP_TOP_PAD_PX as u32;
        let i = ((top * w + (layout.tabs[2].x - 0.5).round() as u32) * 4) as usize;
        assert_eq!(
            Rgb::new(rgba[i], rgba[i + 1], rgba[i + 2]),
            divider,
            "from the tabs' top edge"
        );
        assert_ne!(
            at(layout.tabs[1].x),
            divider,
            "the active tab has its own edge"
        );
    }

    #[test]
    fn test_a_modified_tab_shows_a_dot_until_the_pointer_comes_over_it() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let mut raster = |modified: bool, hover: PaneStripHit| {
            let tabs = vec![PaneTab {
                active: true,
                dragging: false,
                focused: true,
                icon: None,
                modified,
                tab_id: 1,
                title: "notes.md".into(),
            }];
            let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
            rasterize_pane_strip_rgba(
                &tabs,
                &layout,
                w,
                h,
                CELL_W,
                CELL_H,
                &theme,
                hover,
                Rgb::new(60, 60, 60),
                PaneStripDropTarget::default(),
                &mut fonts,
                &mut swash,
                &ctx,
            )
        };
        assert_ne!(
            raster(true, PaneStripHit::None),
            raster(false, PaneStripHit::None),
            "the dot stands where the cross was"
        );
        assert_eq!(
            raster(true, PaneStripHit::Tab(1)),
            raster(false, PaneStripHit::Tab(1)),
            "over the tab, the close button is back"
        );
    }

    #[test]
    fn test_pane_strip_tab_pills_have_square_corners() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![PaneTab {
            active: true,
            dragging: false,
            focused: true,
            icon: None,
            modified: false,
            tab_id: 1,
            title: "zsh".into(),
        }];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let rgba = rasterize_pane_strip_rgba(
            &tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 3].to_vec()
        };
        // An unadorned tab pill's top-left pixel is as filled as its interior;
        // a rounded corner would leave it partly band-colored.
        assert_eq!(at(1, 3), at(150, 12));
    }

    #[test]
    fn test_new_tab_plus_is_a_crisp_symmetric_cross_centered_on_its_pill() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![PaneTab {
            active: true,
            dragging: false,
            focused: true,
            icon: None,
            modified: false,
            tab_id: 1,
            title: "zsh".into(),
        }];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let nt = layout.new_tab.expect("the button fits");
        let rgba = rasterize_pane_strip_rgba(
            &tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );
        let px = |x: u32, y: u32| rgba[((y * w + x) * 4) as usize..][..3].to_vec();
        let band = px(nt.x as u32 + 1, 1);
        let ink: Vec<(u32, u32)> = (0..h - 2)
            .flat_map(|y| (nt.x as u32..(nt.x + nt.w) as u32).map(move |x| (x, y)))
            .filter(|&(x, y)| px(x, y) != band)
            .collect();
        let (x0, x1) = (
            ink.iter().map(|p| p.0).min().unwrap(),
            ink.iter().map(|p| p.0).max().unwrap(),
        );
        let (y0, y1) = (
            ink.iter().map(|p| p.1).min().unwrap(),
            ink.iter().map(|p| p.1).max().unwrap(),
        );
        assert_eq!(x1 - x0, y1 - y0, "the '+' is not as wide as it is tall");
        let full = px(x0 + (x1 - x0) / 2, y0 + (y1 - y0) / 2);
        // Crisp: every pixel it touches is the same full-strength color, with
        // no half-lit edge.
        assert!(
            ink.iter().all(|&(x, y)| px(x, y) == full),
            "the '+' has soft edges"
        );
        let pill = button_pill(nt.x + nt.w / 2.0, (h - 2) as f32 / 2.0, CELL_H);
        let (cx, cy) = ((x0 + x1 + 1) as f32 / 2.0, (y0 + y1 + 1) as f32 / 2.0);
        assert_eq!(
            (cx, cy),
            (pill.0 + pill.2 / 2.0, pill.1 + pill.3 / 2.0),
            "off its pill"
        );
    }

    #[test]
    fn test_unfocused_active_tab_is_dimmed_compared_to_focused_active_tab() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let focused_tabs = vec![PaneTab {
            active: true,
            dragging: false,
            focused: true,
            icon: None,
            modified: false,
            tab_id: 1,
            title: "zsh".into(),
        }];
        let unfocused_tabs = vec![PaneTab {
            active: true,
            dragging: false,
            focused: false,
            icon: None,
            modified: false,
            tab_id: 1,
            title: "zsh".into(),
        }];
        let layout = layout_pane_strip(&focused_tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let focused_rgba = rasterize_pane_strip_rgba(
            &focused_tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );
        let unfocused_rgba = rasterize_pane_strip_rgba(
            &unfocused_tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );
        let tab_mid_x = (layout.tabs[0].x + layout.tabs[0].w / 2.0) as usize;
        let tab_mid_y = 4;
        let focused_pixel = &focused_rgba[((tab_mid_y * w as usize + tab_mid_x) * 4)..][..3];
        let unfocused_pixel = &unfocused_rgba[((tab_mid_y * w as usize + tab_mid_x) * 4)..][..3];
        assert_ne!(
            focused_pixel, unfocused_pixel,
            "unfocused tab pill should have distinct dimmer color"
        );
    }

    #[test]
    fn test_unfocused_active_tab_has_indicator_and_border_distinct_from_inactive_tab() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![
            PaneTab {
                active: true,
                dragging: false,
                focused: false,
                icon: None,
                modified: false,
                tab_id: 1,
                title: "zsh".into(),
            },
            PaneTab {
                active: false,
                dragging: false,
                focused: false,
                icon: None,
                modified: false,
                tab_id: 2,
                title: "vim".into(),
            },
        ];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let rgba = rasterize_pane_strip_rgba(
            &tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );

        let active_top_x = (layout.tabs[0].x + layout.tabs[0].w / 2.0) as usize;
        let active_top_y = (crate::PANE_STRIP_TOP_PAD_PX + 0.5) as usize;
        let inactive_top_x = (layout.tabs[1].x + layout.tabs[1].w / 2.0) as usize;
        let inactive_top_y = active_top_y;

        let active_top_pixel = &rgba[((active_top_y * w as usize + active_top_x) * 4)..][..3];
        let inactive_top_pixel = &rgba[((inactive_top_y * w as usize + inactive_top_x) * 4)..][..3];

        assert_ne!(
            active_top_pixel, inactive_top_pixel,
            "unfocused active tab should have distinct accent indicator at the top"
        );
        let expected_unfocused_indicator =
            mix_rgb(theme.active_indicator, PANE_STRIP_BAND_BG, DIM_FACTOR);
        assert_eq!(
            active_top_pixel,
            &[
                expected_unfocused_indicator.r,
                expected_unfocused_indicator.g,
                expected_unfocused_indicator.b
            ],
            "unfocused active tab should use dimmed royal blue accent for the top border"
        );
    }

    #[test]
    fn test_focused_active_tab_has_saturated_royal_blue_indicator() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![PaneTab {
            active: true,
            dragging: false,
            focused: true,
            icon: None,
            modified: false,
            tab_id: 1,
            title: "zsh".into(),
        }];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let rgba = rasterize_pane_strip_rgba(
            &tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            PaneStripDropTarget::default(),
            &mut fonts,
            &mut swash,
            &ctx,
        );

        let active_top_x = (layout.tabs[0].x + layout.tabs[0].w / 2.0) as usize;
        let active_top_y = (crate::PANE_STRIP_TOP_PAD_PX + 0.5) as usize;

        let active_top_pixel = &rgba[((active_top_y * w as usize + active_top_x) * 4)..][..3];
        assert_eq!(
            active_top_pixel,
            &[
                theme.active_indicator.r,
                theme.active_indicator.g,
                theme.active_indicator.b
            ],
            "focused active tab should use the deep saturated royal blue color for the top border"
        );
        let edge_pixel = &rgba[(active_top_x * 4)..][..3];
        assert_eq!(
            edge_pixel, active_top_pixel,
            "the bar reaches the strip's top edge, against the border above"
        );
        let below = PANE_STRIP_ACTIVE_INDICATOR_PX as usize;
        let below_pixel = &rgba[((below * w as usize + active_top_x) * 4)..][..3];
        assert_ne!(
            below_pixel, active_top_pixel,
            "and is no thicker than its own width"
        );
    }

    #[test]
    fn test_pane_strip_drop_indicator_and_dragging_tab() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripDropTarget, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![
            PaneTab {
                active: true,
                dragging: true,
                focused: true,
                icon: None,
                modified: false,
                tab_id: 1,
                title: "zsh".into(),
            },
            PaneTab {
                active: false,
                dragging: false,
                focused: false,
                icon: None,
                modified: false,
                tab_id: 2,
                title: "vim".into(),
            },
        ];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let drop_target = PaneStripDropTarget {
            indicator_x: Some(150.0),
            is_target_pane: true,
        };
        let rgba = rasterize_pane_strip_rgba(
            &tabs,
            &layout,
            w,
            h,
            CELL_W,
            CELL_H,
            &theme,
            PaneStripHit::None,
            Rgb::new(60, 60, 60),
            drop_target,
            &mut fonts,
            &mut swash,
            &ctx,
        );

        // 1. Dragging tab suppresses the top active accent line
        let tab0_top_x = (layout.tabs[0].x + layout.tabs[0].w / 2.0) as usize;
        let tab0_top_y = (crate::PANE_STRIP_TOP_PAD_PX + 0.5) as usize;
        let tab0_top_pixel = &rgba[((tab0_top_y * w as usize + tab0_top_x) * 4)..][..3];
        assert_ne!(
            tab0_top_pixel,
            &[
                theme.active_indicator.r,
                theme.active_indicator.g,
                theme.active_indicator.b
            ],
            "dragging active tab should suppress the top active accent indicator"
        );

        // 2. Drop target insertion line renders royal blue at indicator_x
        let ind_x = 150usize;
        let ind_y = (crate::PANE_STRIP_TOP_PAD_PX + 4.0) as usize;
        let ind_pixel = &rgba[((ind_y * w as usize + ind_x) * 4)..][..3];
        assert_eq!(
            ind_pixel,
            &[
                theme.active_indicator.r,
                theme.active_indicator.g,
                theme.active_indicator.b
            ],
            "drop insertion bar should render with royal blue accent at indicator_x"
        );

        // 3. Target pane separator line renders with royal blue accent
        let sep_y = (h - 1) as usize;
        let sep_x = (w / 2) as usize;
        let sep_pixel = &rgba[((sep_y * w as usize + sep_x) * 4)..][..3];
        assert_eq!(
            sep_pixel,
            &[
                theme.active_indicator.r,
                theme.active_indicator.g,
                theme.active_indicator.b
            ],
            "target pane bottom line should highlight with royal blue accent"
        );
    }
}

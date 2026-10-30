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
    self, PaneStripDropTarget, PaneStripHit, PaneStripLayout, PaneTab, Region,
    TAB_H_PAD_CELLS,
};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// How far an inactive tab is lifted from the band toward the pane's
/// background: a tab, but plainly not the one open into the pane.
const INACTIVE_TAB_LIFT: f32 = 0.3;

/// Height of the top accent indicator bar on selected tab pills, in pixels.
pub(crate) const PANE_STRIP_ACTIVE_INDICATOR_PX: f32 = 2.0;

/// The same bar's height in a pane without the focus: thinner as well as
/// dimmer, so the focused pane's reads first.
const PANE_STRIP_INACTIVE_INDICATOR_PX: f32 = 1.0;

/// How far the divider between two inactive tabs leans from their color to
/// the bright text color: enough to read as an edge, short of a border.
const INACTIVE_SEPARATOR_MIX: f32 = 0.18;

/// How far, from 0 to 1, the line under a strip is eased from the split
/// divider's color toward the background, so it never reads as a divider.
const STRIP_SEPARATOR_DIM: f32 = 0.5;

/// How far the line under the strip fades into the active tab where it runs
/// under it: the tab still opens into its pane, and the strip still reads
/// as ending along the line.
const ACTIVE_LINE_FADE: f32 = 0.5;

/// How far the active tab's side edges are eased from the split divider's
/// color toward the background: less than the line under the strip, so the
/// tab's outline reads plainly against the tabs beside it.
const ACTIVE_EDGE_DIM: f32 = 0.2;

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

    // The active tab is the pane's own background, so it reads as open into
    // its pane; the band and the other tabs are shaded darker, so the strips
    // frame the panes and the line between two panes needs no weight of its
    // own.
    let active_bg = theme.background;
    let band_bg = band_color(theme);

    // 1. Fill the recessed band the tabs sit on.
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (0.0, 0.0, width as f32, height as f32),
        0.0,
        band_bg,
        1.0,
    );

    let tab_vpad_top = crate::PANE_STRIP_TOP_PAD_PX;
    let tab_vpad_bottom = crate::PANE_STRIP_SEPARATOR_PX + crate::PANE_STRIP_SEPARATOR_INSET_PX;
    let inactive_bg = inactive_tab_color(theme);
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
        mix_rgb(active_text_color_rgb, band_bg, DIM_FACTOR).to_glyphon();
    let inactive_text_color = mix_rgb(theme.ansi[8], bright_target, 0.62).to_glyphon();

    // 2. Tab pills and their contents (text + close icon).
    for (i, tab) in tabs.iter().enumerate() {
        let tab_reg = layout.tabs[i];
        if tab_reg.w <= 0.0 {
            continue;
        }

        let is_hovered =
            matches!(hover, PaneStripHit::Tab(id) | PaneStripHit::CloseTab(id) if id == tab.tab_id);
        // An active tab is its pane's background whether or not the pane has
        // the focus, so it always joins its pane; its bar and text tell focus.
        let pill_color = if tab.dragging {
            mix_rgb(band_bg, active_bg, 0.25)
        } else if tab.active {
            active_bg
        } else if is_hovered {
            mix_rgb(inactive_bg, active_bg, 0.55)
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

        // Decorate selected tab: an accent bar along the top edge of the active tab,
        // in the bright accent in the focused pane and dimmed toward the band in
        // the others, so the selected tab stays recognizable without competing
        // for focus. It has no side borders: a full-height line in the strip
        // reads as the divider between two panes.
        if tab.active && !tab.dragging {
            // The bar starts at the strip's top edge rather than the pill's,
            // so it meets the border above (the title bar's, or a split's)
            // with no band-colored row between them, and keeps its own
            // thickness: it covers the pill's top rows rather than adding to
            // them.
            let (indicator_color, indicator_px) = match tab.focused {
                true => (theme.active_indicator, PANE_STRIP_ACTIVE_INDICATOR_PX),
                false => (
                    mix_rgb(theme.active_indicator, band_bg, DIM_FACTOR),
                    PANE_STRIP_INACTIVE_INDICATOR_PX,
                ),
            };
            let indicator = (pill_bounds.0, tab_reg.y, pill_bounds.2, indicator_px);
            fill_rounded_rect(
                &mut rgba,
                canvas,
                indicator,
                SQUARE_CORNER,
                indicator_color,
                1.0,
            );
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
                mix_rgb(band_bg, theme.ansi[8], 0.4)
            } else if is_close_hovered || is_hovered || (tab.active && tab.focused) {
                theme.foreground
            } else if tab.active {
                mix_rgb(theme.foreground, band_bg, DIM_FACTOR)
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
        let title_end = close_reg.x;
        let mut text_x = tab_reg.x + TAB_H_PAD_CELLS * cw;
        let text_color = if tab.dragging {
            let dimmed_rgb = mix_rgb(band_bg, theme.ansi[8], 0.5);
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
            let new_tab_hover = mix_rgb(band_bg, theme.foreground, 0.12);
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
                mix_rgb(band_bg, theme.foreground, 0.12),
                1.0,
            );
        }
        let color = if !active {
            mix_rgb(band_bg, theme.ansi[8], 0.4)
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
        mix_rgb(separator, theme.background, STRIP_SEPARATOR_DIM)
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
    // The active tab runs on down into its pane, as a folder's tab joins its
    // folder, the line going on under it faded into its color and its sides
    // edged down to the line, so it stands apart from the tabs beside it;
    // while a tab is dragged over the pane, the line is the drop mark and
    // stays whole.
    if !drop_target.is_target_pane {
        let edge_color = mix_rgb(separator, theme.background, ACTIVE_EDGE_DIM);
        let faded_line = mix_rgb(sep_color, active_bg, ACTIVE_LINE_FADE);
        let line_y = height as f32 - inset - crate::PANE_STRIP_SEPARATOR_PX;
        for (i, tab) in tabs.iter().enumerate() {
            let tab_reg = layout.tabs[i];
            if !tab.active || tab.dragging || tab_reg.w <= 0.0 {
                continue;
            }
            // On a whole pixel: the strip's height is not, and a fill
            // starting partway into a row blends into whatever is under it.
            let bottom = (tab_reg.y + tab_reg.h - tab_vpad_bottom).round();
            let left = tab_reg.x + tabbar::TAB_GAP_PX / 2.0;
            let right = tab_reg.x + tab_reg.w - tabbar::TAB_GAP_PX / 2.0;
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (left, bottom, right - left, height as f32 - bottom),
                0.0,
                active_bg,
                1.0,
            );
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (left, line_y, right - left, crate::PANE_STRIP_SEPARATOR_PX),
                0.0,
                faded_line,
                1.0,
            );
            // The tab's own outermost column on each side, from the strip's
            // top to the bottom of the line the tab cuts through, so the
            // edges never paint over what is beside it.
            // They start under the accent bar along the tab's top, which
            // keeps its whole width.
            let bar = match tab.focused {
                true => PANE_STRIP_ACTIVE_INDICATOR_PX,
                false => PANE_STRIP_INACTIVE_INDICATOR_PX,
            };
            let (edge_top, edge_bottom) = (tab_reg.y + bar, height as f32 - inset);
            for x in [left.round(), right.round() - 1.0] {
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (x, edge_top, 1.0, edge_bottom - edge_top),
                    0.0,
                    edge_color,
                    1.0,
                );
            }
        }
    }
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

/// The band a pane's tabs sit on.
fn band_color(theme: &Theme) -> Rgb {
    theme.tab_strip_bg()
}

/// An inactive tab: the band lifted a little toward the pane's background.
fn inactive_tab_color(theme: &Theme) -> Rgb {
    mix_rgb(band_color(theme), theme.background, INACTIVE_TAB_LIFT)
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
                && key.drop_target == drop_target
                && key.origin == (x, y)
        };
        if let Some(cached) = self
            .overlay_caches
            .pane_strips
            .get(&texture_id)
            .and_then(|cache| cache.hit(&context, matches))
        {
            if let Some(&placement) = cached.first().filter(|placement| self.image_pass.has(placement.id)) {
                return placement;
            }
        }
        let divider_color = self.split_divider_color();
        let (cell_w, cell_h) = (self.fonts.cell_width, self.fonts.cell_height);
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
        self.image_pass.upload(&self.device, &self.queue, texture_id, &rgba, width, height);
        let placement = ImagePlacement {
            alpha: 1.0,
            height: height as f32,
            id: texture_id,
            u_max: 1.0,
            u_min: 0.0,
            v_max: 1.0,
            v_min: 0.0,
            width: width as f32,
            x,
            y,
        };
        let key = PaneStripKey {
            drop_target,
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
        let inactive_bg = inactive_tab_color(&theme);
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
    fn test_an_active_tab_joins_its_pane_and_only_its_bar_tells_focus() {
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
        let pixel = |rgba: &[u8], x: usize, y: usize| {
            let i = (y * w as usize + x) * 4;
            Rgb::new(rgba[i], rgba[i + 1], rgba[i + 2])
        };
        let tab_mid_x = (layout.tabs[0].x + layout.tabs[0].w / 2.0) as usize;
        assert_eq!(pixel(&focused_rgba, tab_mid_x, 0), theme.active_indicator);
        assert_ne!(
            pixel(&unfocused_rgba, tab_mid_x, 0),
            theme.active_indicator,
            "the unfocused pane's bar is dimmed"
        );
        let pill_y = 4;
        assert_eq!(pixel(&focused_rgba, tab_mid_x, pill_y), theme.background);
        assert_eq!(
            pixel(&unfocused_rgba, tab_mid_x, pill_y),
            theme.background,
            "an active tab joins its pane, focused or not"
        );
        let line_y = (h as f32
            - crate::PANE_STRIP_SEPARATOR_INSET_PX
            - crate::PANE_STRIP_SEPARATOR_PX) as usize;
        // Regression: the fill opening the tab into its pane began partway
        // into the line's row, so the line under it showed half-faded in
        // some panes and not at all in others. It is faded on purpose now.
        let under_tab = pixel(&unfocused_rgba, tab_mid_x, line_y);
        let beside = pixel(&unfocused_rgba, w as usize - 1, line_y);
        assert_ne!(under_tab, theme.background, "the line runs on under the active tab");
        assert_ne!(under_tab, beside, "faded into the tab, not as strong as beside it");
        assert_ne!(beside, theme.background, "and runs on whole under the band");
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
        // Thinner than the focused pane's: one row of bar, then the pill.
        let bar = &rgba[(active_top_x * 4)..][..3];
        let below = PANE_STRIP_INACTIVE_INDICATOR_PX as usize;
        let under_bar = &rgba[((below * w as usize + active_top_x) * 4)..][..3];
        assert_ne!(bar, under_bar, "the unfocused pane's bar is one pixel thick");
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

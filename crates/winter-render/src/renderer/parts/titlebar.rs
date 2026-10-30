//! The top title bar: its tab pills, menu titles and working path as text
//! runs, and the strip behind them (the band, the pills, the new-tab and
//! hamburger buttons, the scroll chevrons and the window controls) painted
//! into an RGBA buffer.

use glyphon::Color;

use super::{SQUARE_CORNER, TAB_CLOSE_SHIFT_RIGHT_PX, TAB_LABEL_SHIFT_UP_PX};
use crate::renderer::colors::mix_rgb;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::raster::paint::{
    buffer_width, button_pill, fill_line_segment, fill_rounded_rect, pill_center,
    shape_chrome_line, text_bounds, truncate_label,
};
use crate::renderer::raster::view::{StripImage, TabbarText};
use crate::renderer::GpuRenderer;
use crate::tabbar::{
    self, layout as tabbar_layout, TabbarHit, TabbarLayout, TopTabbar, TAB_H_PAD_CELLS, ZOOM_CELLS,
};
use crate::theme::{Rgb, Theme};

// ========================================================================
// Constants
// ========================================================================

/// Flat pixels the per-tab close button (× glyph and its hover pill) is
/// shifted up from its vertically-centered position within its close slot.
const TAB_CLOSE_SHIFT_UP_PX: f32 = 1.0;

/// The narrowest stretch of title bar, in cells, worth drawing the working
/// path into; below it the path is left out rather than reduced to an ellipsis.
const PATH_MIN_CELLS: f32 = 8.0;

/// The fewest characters a shortened working path keeps, so it never ends up
/// as a lone ellipsis.
const PATH_MIN_KEPT_CHARS: usize = 4;

/// How much larger the title bar's button icons (tab close, hamburger and
/// window controls) are drawn than their base size, to suit the taller bar.
const TITLEBAR_ICON_SCALE: f32 = 1.2;

/// How far the active tab's background is mixed toward `theme.foreground`
/// from `theme.tab_active_bg`, so it reads as clearly highlighted rather
/// than blending into the content view below it.
const TAB_ACTIVE_HIGHLIGHT: f32 = 0.06;

/// Opacity of the hairline separating the tab band from the content below.
const TABBAR_BORDER_ALPHA: f32 = 0.5;

/// Font-size multiplier for the tab close (`×`) and zoom glyphs, so they read
/// larger than the title text against the taller tab band.
const TAB_BUTTON_GLYPH_RATIO: f32 = 1.25;

/// Glyph drawn left of the close button when a tab's pane is zoomed to fill the
/// viewport. Matches the window-control maximize glyph (U+25A1).
const TAB_ZOOM_GLYPH: &str = "\u{25a1}";

// ========================================================================
// Data Structures
// ========================================================================

/// The derived metrics and colors every tabbar-strip painting pass shares.
struct StripStyle<'a> {
    /// Fill of the active tab's pill.
    active_bg: Rgb,
    /// Fill of the recessed band across the whole strip.
    band: Rgb,
    /// Pixel dimensions of the RGBA buffer being painted into.
    canvas: (u32, u32),
    cell_h: f32,
    cell_w: f32,
    /// Fill of an inactive tab's pill.
    inactive_bg: Rgb,
    tab_vpad_bottom: f32,
    tab_vpad_top: f32,
    /// The palette every pass resolves its own colors from.
    theme: &'a Theme,
}

// ========================================================================
// GpuRenderer: title bar text
// ========================================================================

impl GpuRenderer {
    /// Append the top tabbar's background quads to `verts` and return its text
    /// runs (tab titles, close glyphs, the new-tab button, and either the modern
    /// hamburger or the classic menu titles). The dropdown is drawn separately.
    pub(in crate::renderer) fn draw_tabbar(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
    ) -> Vec<TabbarText> {
        let cw = self.fonts.cell_width;
        let ch = self.fonts.cell_height;
        let layout = tabbar_layout(tabbar, surface_w, cw, ch);
        let foreground = self.theme.foreground.to_glyphon();
        // Brighten tab titles so they read clearly against the tab band. On a
        // dark theme that means blending toward white; on a light theme the
        // readable direction is toward the dark foreground, so blending toward
        // white would wash the text out.
        let dark_theme = {
            let bg = self.theme.tabbar_bg;
            (bg.r as f32 + bg.g as f32 + bg.b as f32) / (3.0 * 255.0) < 0.5
        };
        let bright_target = if dark_theme {
            Rgb::new(255, 255, 255)
        } else {
            self.theme.foreground
        };
        // Active tab: the theme's active color pushed further toward the bright
        // target for a crisp, luminous label.
        let active_tab = mix_rgb(self.theme.tab_active_fg, bright_target, 0.35).to_glyphon();
        // Inactive tab: the muted bright-black lifted well toward the bright
        // target so background tabs stay legible, just dimmer than the active one.
        let inactive_tab = mix_rgb(self.theme.ansi[8], bright_target, 0.62).to_glyphon();
        let mut texts = Vec::new();
        // The taller modern bar is two cells high, so center a single text line
        // vertically within each element's region instead of top-aligning it.
        let line_h = self.fonts.line_height;
        let vcenter = |y: f32, h: f32| y + (h - line_h) / 2.0;
        // Tabs render inset top and bottom by the same amount (see
        // `rasterize_tabbar_strip`), so their visible "tab shape" is shorter
        // than the full element region. Center the label, zoom icon, and
        // close button on that shape, not the taller region, or they'd sit
        // off-center inside the pill.
        let tab_top_inset = tabbar::tab_top_inset_px(tabbar.menu_style);
        let tab_bottom_inset = tabbar::TAB_BOTTOM_VPAD_PX;

        // The band, rounded tab cards, and tabbar border are rasterized into a
        // texture by `rasterize_tabbar_strip` and composited under this text.

        // Tab titles plus their close button (and, when zoomed, a zoom icon just
        // left of it). The close button sits on the right, inset from the edge.
        for (i, tab) in layout.tabs.iter().enumerate() {
            // Tabs scrolled out of the paginated window carry a zero-width region.
            if tab.w <= 0.0 {
                continue;
            }
            let close = layout.closes[i];
            let title_x = tab.x + TAB_H_PAD_CELLS * cw;
            // The zoom icon reserves a slot immediately left of the close button
            // when the tab's pane is zoomed; the title must clear whichever comes
            // first.
            let zoom_w = ZOOM_CELLS * cw;
            let zoom = tabbar.tabs[i].zoomed.then_some(close.x - zoom_w);
            let title_end = zoom.unwrap_or(close.x);
            let avail = (title_end - title_x).max(cw);
            let max_chars = (avail / cw).floor() as usize;
            // Prefix every tab title with its 1-based tab number so tabs are
            // always identifiable (and scrolled tabs stay so).
            let labeled = format!("{}: {}", i + 1, tabbar.tabs[i].title);
            let title = truncate_label(&labeled, max_chars);
            let active = i == tabbar.active_tab;
            let color = if active { active_tab } else { inactive_tab };
            // Tab titles use the desktop environment's interface font; the active
            // tab is bolded and distinguished by its color and pill.
            let buffer = self.tabbar_line_buffer(&title, color, active, true);
            texts.push(TabbarText {
                bounds: text_bounds(title_x, tab.y, title_end, tab.y + tab.h),
                buffer,
                color,
                left: title_x,
                top: vcenter(
                    tab.y + tab_top_inset,
                    tab.h - tab_top_inset - tab_bottom_inset,
                ) - TAB_LABEL_SHIFT_UP_PX,
            });

            // Zoom/restore icon, drawn only while the pane is zoomed.
            if let Some(zoom_x) = zoom {
                let glyph_cw = cw * TAB_BUTTON_GLYPH_RATIO;
                let glyph_ch = self.fonts.line_height * TAB_BUTTON_GLYPH_RATIO;
                let zoom_buf =
                    self.tabbar_button_buffer(TAB_ZOOM_GLYPH, color, TAB_BUTTON_GLYPH_RATIO);
                texts.push(TabbarText {
                    bounds: text_bounds(zoom_x, close.y, zoom_x + zoom_w, close.y + close.h),
                    buffer: zoom_buf,
                    color,
                    left: zoom_x + (zoom_w - glyph_cw) / 2.0,
                    top: close.y
                        + tab_top_inset
                        + (close.h - tab_top_inset - tab_bottom_inset - glyph_ch) / 2.0,
                });
            }
        }

        for (i, region) in layout.menu_titles.iter().enumerate() {
            let open = tabbar.open_menu == Some(i);
            let buffer = self.tabbar_line_buffer(&tabbar.menus[i].title, foreground, open, false);
            texts.push(TabbarText {
                bounds: text_bounds(region.x, region.y, region.x + region.w, region.y + region.h),
                buffer,
                color: foreground,
                left: region.x + cw,
                top: vcenter(region.y, region.h),
            });
        }

        // The focused pane's working path, centered in the free stretch of the
        // row between the hamburger and the window controls, muted like an
        // inactive tab's label.
        let path = layout.path;
        let avail = path.w - 2.0 * cw;
        if !tabbar.path.is_empty() && avail >= PATH_MIN_CELLS * cw {
            let (buffer, width) = self.fitted_path_buffer(&tabbar.path, inactive_tab, avail);
            texts.push(TabbarText {
                bounds: text_bounds(path.x, path.y, path.x + path.w, path.y + path.h),
                buffer,
                color: inactive_tab,
                left: path.x + (path.w - width) / 2.0,
                top: vcenter(path.y, path.h) - TAB_LABEL_SHIFT_UP_PX,
            });
        }

        texts
    }

    /// Shape `path` as one line no wider than `avail` pixels, with its front
    /// cut off (behind an ellipsis) when it does not fit: the end of a path is
    /// the part that says where you are. Returns the buffer and its width.
    fn fitted_path_buffer(
        &mut self,
        path: &str,
        color: Color,
        avail: f32,
    ) -> (glyphon::Buffer, f32) {
        let mut text = path.to_string();
        loop {
            let buffer = self.tabbar_line_buffer(&text, color, false, true);
            let width = buffer_width(&buffer);
            let kept = text.chars().count();
            if width <= avail || kept <= PATH_MIN_KEPT_CHARS {
                return (buffer, width);
            }
            // Scale the kept length by how far over it is, and always drop at
            // least one character so the loop ends.
            let target = ((kept as f32 * avail / width).floor() as usize).min(kept - 1);
            let tail: String = path
                .chars()
                .skip(path.chars().count() - target.saturating_sub(1))
                .collect();
            text = format!("\u{2026}{tail}");
        }
    }

    /// Shape a single line of tabbar text into a reusable buffer. With
    /// `proportional` set, it uses a sans-serif UI font (for the dropdown menu)
    /// rather than the terminal's monospace family.
    fn tabbar_line_buffer(
        &mut self,
        text: &str,
        color: Color,
        bold: bool,
        proportional: bool,
    ) -> glyphon::Buffer {
        let painter = self.chrome_painter();
        shape_chrome_line(
            painter.font_system,
            &painter.ctx,
            text,
            color,
            bold,
            proportional,
        )
    }

    /// Shape a single tab-button glyph (close `×`, zoom icon) at a scaled-up
    /// font size so it reads as a control rather than body text. The terminal's
    /// monospace family is used (matching the window-control glyphs) so each
    /// glyph's advance is exactly one scaled cell, keeping the cw-based
    /// centering exact and preventing overflow into a neighbour's clip rect.
    fn tabbar_button_buffer(&mut self, glyph: &str, color: Color, ratio: f32) -> glyphon::Buffer {
        let painter = self.chrome_painter();
        let ctx = FontCtx {
            font_size: painter.ctx.font_size * ratio,
            line_height: (painter.ctx.line_height * ratio).round(),
            ..painter.ctx
        };
        shape_chrome_line(painter.font_system, &ctx, glyph, color, false, false)
    }
}

// ========================================================================
// Tabbar strip painting
// ========================================================================

/// Paint the whole top-tabbar strip into an RGBA buffer: the recessed band,
/// every tab pill and its close icon, the new-tab and hamburger buttons, the
/// scroll chevrons, and the window controls.
///
/// Free of the GPU on purpose, like the other `*_rgba` painters in this module,
/// so the pixel work can be exercised without a device; the caller only uploads
/// the result.
pub(in crate::renderer) fn tabbar_strip_rgba(
    tabbar: &TopTabbar,
    surface_w: f32,
    cell_w: f32,
    cell_h: f32,
    theme: &Theme,
) -> StripImage {
    let cw = cell_w;
    let ch = cell_h;
    let layout = tabbar_layout(tabbar, surface_w, cw, ch);
    let tabbar_h = if tabbar.menu_style == crate::tabbar::MenuStyle::Modern {
        crate::modern_titlebar_height_px(ch)
    } else {
        tabbar::tabbar_rows(tabbar.menu_style) as f32 * ch
    };
    let width = surface_w.ceil().max(1.0) as u32;
    let height = tabbar_h.ceil().max(1.0) as u32;
    let canvas = (width, height);
    let band = theme.tabbar_bg;
    // Mixed toward the foreground (a neutral shift, not a color tint)
    // rather than left equal to the content background: a floating tab
    // pill in exactly the background color would read as chrome, not as
    // "this is the active tab".
    let active_bg = mix_rgb(theme.tab_active_bg, theme.foreground, TAB_ACTIVE_HIGHLIGHT);
    let inactive_bg = mix_rgb(theme.tabbar_bg, theme.tab_active_bg, 0.5);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Recessed band across the whole strip.
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (0.0, 0.0, width as f32, height as f32),
        0.0,
        band,
        1.0,
    );
    // In classic style, separate the menubar row from the tabbar row.
    if layout.menubar_top.is_some() {
        fill_rounded_rect(
            &mut rgba,
            canvas,
            (0.0, layout.tab_row_top - 1.0, width as f32, 1.0),
            0.0,
            theme.divider,
            TABBAR_BORDER_ALPHA,
        );
    }

    // The hamburger, close, and new-tab hover pills are inset on all sides
    // so they read as floating buttons. Tabs themselves now match: inset
    // by the same amount on top and bottom (Brave-style floating pills,
    // all four corners rounded) instead of sitting flush against the
    // strip's bottom edge. `TAB_GAP_PX` still opens a hairline gap between
    // neighbors: the hit-test `Region`s stay flush, only the rendered
    // pill shrinks.
    let style = StripStyle {
        active_bg,
        band,
        canvas,
        cell_h: ch,
        cell_w: cw,
        inactive_bg,
        theme,
        // New-tab's own horizontal hover padding: the reference width every
        // other titlebar button's hover pill matches. Shared with
        // `tabbar::layout`, which reserves extra spacing around the narrower
        // buttons so this wider pill never bleeds into a neighbor.
        tab_vpad_bottom: tabbar::TAB_BOTTOM_VPAD_PX,
        tab_vpad_top: tabbar::tab_top_inset_px(tabbar.menu_style),
    };
    // Every titlebar-edge button's hover pill is the same fixed width, is
    // centered horizontally in whichever button is hovered, and shares one
    // top inset, so the pills line up no matter how wide their own boxes
    // are. Only the background color varies between buttons.

    paint_tab_pills(&mut rgba, &style, &layout, tabbar);
    paint_scroll_chevrons(&mut rgba, &style, &layout, tabbar);
    paint_hamburger(&mut rgba, &style, &layout, tabbar);
    paint_window_controls(&mut rgba, &style, &layout, tabbar);

    StripImage {
        height,
        rgba,
        width,
    }
}

fn paint_tab_pills(
    rgba: &mut [u8],
    style: &StripStyle<'_>,
    layout: &TabbarLayout,
    tabbar: &TopTabbar,
) {
    let hover_tab = match tabbar.tabbar_hover {
        TabbarHit::Tab(i) | TabbarHit::CloseTab(i) => Some(i),
        _ => None,
    };
    for (i, tab) in layout.tabs.iter().enumerate() {
        if tab.w <= 0.0 {
            continue;
        }
        let is_active = i == tabbar.active_tab;
        let base = if is_active {
            style.active_bg
        } else {
            style.inactive_bg
        };
        let color = if hover_tab == Some(i) && !is_active {
            mix_rgb(style.inactive_bg, style.active_bg, 0.55)
        } else {
            base
        };
        let bounds = (
            tab.x + tabbar::TAB_GAP_PX / 2.0,
            tab.y + style.tab_vpad_top,
            tab.w - tabbar::TAB_GAP_PX,
            tab.h - style.tab_vpad_top - style.tab_vpad_bottom,
        );
        fill_rounded_rect(rgba, style.canvas, bounds, SQUARE_CORNER, color, 1.0);
        // Close button hover: rounded rectangle highlight over the × region.
        // Centered on the tab shape (inset top and bottom), like the ×
        // glyph itself below, not the taller full element region.
        if let TabbarHit::CloseTab(hi) = tabbar.tabbar_hover {
            if hi == i {
                let close = layout.closes[i];
                let hpad = style.cell_w * 0.15;
                let vpad = style.cell_h * 0.2;
                let close_hover = mix_rgb(color, style.theme.foreground, 0.18);
                fill_rounded_rect(
                    rgba,
                    style.canvas,
                    (
                        close.x + hpad + TAB_CLOSE_SHIFT_RIGHT_PX,
                        close.y + style.tab_vpad_top + vpad - TAB_CLOSE_SHIFT_UP_PX,
                        close.w - hpad * 2.0,
                        close.h - style.tab_vpad_top - style.tab_vpad_bottom - vpad * 2.0,
                    ),
                    SQUARE_CORNER,
                    close_hover,
                    1.0,
                );
            }
        }
        // Draw close tab button cross icon (X) vector graphic, centered on
        // the tab shape (inset top and bottom) so it lines up with the
        // label instead of the taller full element region.
        {
            let close = layout.closes[i];
            let is_close_hovered = tabbar.tabbar_hover == TabbarHit::CloseTab(i);
            let is_tab_hovered = hover_tab == Some(i);
            let is_active = i == tabbar.active_tab;
            let close_color = if is_close_hovered || is_tab_hovered || is_active {
                style.theme.foreground
            } else {
                style.theme.ansi[8]
            };
            let cx = close.x + close.w / 2.0 + TAB_CLOSE_SHIFT_RIGHT_PX;
            let cy = close.y
                + style.tab_vpad_top
                + (close.h - style.tab_vpad_top - style.tab_vpad_bottom) / 2.0
                - TAB_CLOSE_SHIFT_UP_PX;
            let size = (style.cell_h * 0.35 * TITLEBAR_ICON_SCALE).round();
            let half = size / 2.0;
            let thickness = 1.25;
            fill_line_segment(
                rgba,
                style.canvas,
                (cx - half, cy - half),
                (cx + half, cy + half),
                thickness,
                close_color,
                1.0,
            );
            fill_line_segment(
                rgba,
                style.canvas,
                (cx - half, cy + half),
                (cx + half, cy - half),
                thickness,
                close_color,
                1.0,
            );
        }
    }
}
/// Paint the tab-strip scroll arrows, dimmed and non-interactive at the ends
/// of the tab list.
fn paint_scroll_chevrons(
    rgba: &mut [u8],
    style: &StripStyle<'_>,
    layout: &TabbarLayout,
    tabbar: &TopTabbar,
) {
    // Tab-strip scroll arrows (chevrons), drawn as vector glyphs like the
    // close icon. Each gets a hover pill whose top aligns with a tab's own
    // top inset (`tab_vpad_top`), like every other titlebar button's hover
    // pill; dimmed and non-interactive at the ends of the tab list.
    let tab_count = tabbar.tabs.len();
    for (region, points_left, hit) in [
        (layout.scroll_left, true, TabbarHit::ScrollTabsLeft),
        (layout.scroll_right, false, TabbarHit::ScrollTabsRight),
    ] {
        let Some(r) = region else { continue };
        // The left arrow is inactive on the first tab, the right on the last.
        let active = if points_left {
            tabbar.active_tab > 0
        } else {
            tabbar.active_tab + 1 < tab_count
        };
        let hovered = active && tabbar.tabbar_hover == hit;
        if hovered {
            let hpad = style.cell_w * 0.2;
            let vpad = style.cell_h * 0.1;
            fill_rounded_rect(
                rgba,
                style.canvas,
                (
                    r.x + hpad,
                    r.y + style.tab_vpad_top,
                    r.w - hpad * 2.0,
                    r.h - style.tab_vpad_top - vpad,
                ),
                SQUARE_CORNER,
                mix_rgb(style.band, style.theme.foreground, 0.12),
                1.0,
            );
        }
        let color = if !active {
            // Dim toward the band so the arrow reads as disabled.
            mix_rgb(style.band, style.theme.ansi[8], 0.4)
        } else if hovered {
            style.theme.foreground
        } else {
            style.theme.ansi[8]
        };
        let cx = r.x + r.w / 2.0;
        let cy = r.y + r.h / 2.0;
        let half = (style.cell_h * 0.2).round();
        let reach = half * 0.6;
        let thickness = 1.5;
        // Tip points in the travel direction; the two strokes form a chevron.
        let tip_x = if points_left { cx - reach } else { cx + reach };
        let back_x = if points_left { cx + reach } else { cx - reach };
        fill_line_segment(
            rgba,
            style.canvas,
            (back_x, cy - half),
            (tip_x, cy),
            thickness,
            color,
            1.0,
        );
        fill_line_segment(
            rgba,
            style.canvas,
            (tip_x, cy),
            (back_x, cy + half),
            thickness,
            color,
            1.0,
        );
    }
}
/// Paint the hamburger button: its hover pill and the three-line icon.
fn paint_hamburger(
    rgba: &mut [u8],
    style: &StripStyle<'_>,
    layout: &TabbarLayout,
    tabbar: &TopTabbar,
) {
    if let Some(hb) = layout.hamburger {
        let is_hovered = tabbar.tabbar_hover == TabbarHit::Hamburger;
        let is_open = tabbar.open_menu.is_some();
        let pill = button_pill(hb.x + hb.w / 2.0, hb.y + hb.h / 2.0, style.cell_h);
        if is_hovered || is_open {
            let btn_color = mix_rgb(style.band, style.theme.foreground, 0.12);
            fill_rounded_rect(rgba, style.canvas, pill, SQUARE_CORNER, btn_color, 1.0);
        }

        // Draw modern vector hamburger icon (3 clean horizontal lines),
        // centered on its pill.
        let color = style.theme.foreground;
        let alpha = if is_hovered || is_open { 1.0 } else { 0.62 };
        let (cx, cy) = pill_center(pill);
        let size = (style.cell_h * 0.40 * TITLEBAR_ICON_SCALE).round();
        let half = size / 2.0;
        let thickness = 1.25;
        let line_spacing = (style.cell_h * 0.14 * TITLEBAR_ICON_SCALE).round().max(3.0);

        fill_line_segment(
            rgba,
            style.canvas,
            (cx - half, cy - line_spacing),
            (cx + half, cy - line_spacing),
            thickness,
            color,
            alpha,
        );
        fill_line_segment(
            rgba,
            style.canvas,
            (cx - half, cy),
            (cx + half, cy),
            thickness,
            color,
            alpha,
        );
        fill_line_segment(
            rgba,
            style.canvas,
            (cx - half, cy + line_spacing),
            (cx + half, cy + line_spacing),
            thickness,
            color,
            alpha,
        );
    }
}
/// Paint the minimize, maximize, and close window controls: each button's
/// hover highlight and its vector icon.
fn paint_window_controls(
    rgba: &mut [u8],
    style: &StripStyle<'_>,
    layout: &TabbarLayout,
    tabbar: &TopTabbar,
) {
    // Window controls hover highlight and vector icons
    if let Some([minimize, maximize, close]) = layout.controls {
        let hover_control = match tabbar.tabbar_hover {
            TabbarHit::Minimize => {
                Some((minimize, mix_rgb(style.band, style.theme.foreground, 0.12)))
            }
            TabbarHit::Maximize => {
                Some((maximize, mix_rgb(style.band, style.theme.foreground, 0.12)))
            }
            TabbarHit::Close => Some((close, Rgb::new(220, 60, 60))),
            _ => None,
        };
        if let Some((region, color)) = hover_control {
            let pill = button_pill(
                region.x + region.w / 2.0,
                region.y + region.h / 2.0,
                style.cell_h,
            );
            fill_rounded_rect(rgba, style.canvas, pill, SQUARE_CORNER, color, 1.0);
        }

        let size = (style.cell_h * 0.45 * TITLEBAR_ICON_SCALE).round();
        let thickness = 1.35;
        let fg_color = style.theme.foreground;
        let muted_alpha = 0.62;

        // Draw Minimize icon (horizontal line), centered on its pill.
        {
            let is_hovered = tabbar.tabbar_hover == TabbarHit::Minimize;
            let alpha = if is_hovered { 1.0 } else { muted_alpha };
            let (cx, cy) = pill_center(button_pill(
                minimize.x + minimize.w / 2.0,
                minimize.y + minimize.h / 2.0,
                style.cell_h,
            ));
            fill_line_segment(
                rgba,
                style.canvas,
                (cx - size / 2.0, cy),
                (cx + size / 2.0, cy),
                thickness,
                fg_color,
                alpha,
            );
        }

        // Draw Maximize icon (hollow square), centered on its pill.
        {
            let is_hovered = tabbar.tabbar_hover == TabbarHit::Maximize;
            let alpha = if is_hovered { 1.0 } else { muted_alpha };
            let (cx, cy) = pill_center(button_pill(
                maximize.x + maximize.w / 2.0,
                maximize.y + maximize.h / 2.0,
                style.cell_h,
            ));
            let sq_x = cx - size / 2.0;
            let sq_y = cy - size / 2.0;

            // Top border
            fill_rounded_rect(
                rgba,
                style.canvas,
                (sq_x, sq_y, size, thickness),
                0.0,
                fg_color,
                alpha,
            );
            // Bottom border
            fill_rounded_rect(
                rgba,
                style.canvas,
                (sq_x, sq_y + size - thickness, size, thickness),
                0.0,
                fg_color,
                alpha,
            );
            // Left border
            fill_rounded_rect(
                rgba,
                style.canvas,
                (sq_x, sq_y + thickness, thickness, size - thickness * 2.0),
                0.0,
                fg_color,
                alpha,
            );
            // Right border
            fill_rounded_rect(
                rgba,
                style.canvas,
                (
                    sq_x + size - thickness,
                    sq_y + thickness,
                    thickness,
                    size - thickness * 2.0,
                ),
                0.0,
                fg_color,
                alpha,
            );
        }

        // Draw Close icon (X), centered on its pill.
        {
            let is_hovered = tabbar.tabbar_hover == TabbarHit::Close;
            let alpha = if is_hovered { 1.0 } else { muted_alpha };
            let (cx, cy) = pill_center(button_pill(
                close.x + close.w / 2.0,
                close.y + close.h / 2.0,
                style.cell_h,
            ));
            // Half-diagonal, not half-side: an X's corner-to-corner span is
            // its side length times sqrt(2), so `size / 2.0` here would read
            // visually larger than minimize's `size`-long line and
            // maximize's `size`-wide square. Scaling by `1 / sqrt(2)` makes
            // the X's diagonal reach exactly `size`, matching both.
            let half = size / (2.0 * std::f32::consts::SQRT_2);

            fill_line_segment(
                rgba,
                style.canvas,
                (cx - half, cy - half),
                (cx + half, cy + half),
                thickness,
                fg_color,
                alpha,
            );
            fill_line_segment(
                rgba,
                style.canvas,
                (cx - half, cy + half),
                (cx + half, cy - half),
                thickness,
                fg_color,
                alpha,
            );
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use glyphon::{FontSystem, SwashCache};

    use super::super::pane_strip::rasterize_pane_strip_rgba;
    use super::*;
    use crate::renderer::raster::test_support::*;
    use crate::renderer::test_support::sample_menu_chrome;
    use crate::tabbar::{MenuStyle, PaneStripLayout};

    const SURFACE_W: f32 = 1000.0;

    fn strip(tabbar: &TopTabbar) -> StripImage {
        tabbar_strip_rgba(tabbar, SURFACE_W, CELL_W, CELL_H, &Theme::dark())
    }

    #[test]
    fn test_strip_covers_its_whole_canvas() {
        // The strip is the window's title bar: any transparent pixel is a hole
        // straight through to whatever the clear color left behind.
        let image = strip(&sample_menu_chrome(None));
        assert_eq!(image.rgba.len(), (image.width * image.height * 4) as usize);
        for x in [0, image.width / 2, image.width - 1] {
            for y in [0, image.height / 2, image.height - 1] {
                assert_eq!(pixel(&image, x, y).3, 255, "pixel ({x}, {y}) is not opaque");
            }
        }
    }

    #[test]
    fn test_strip_band_uses_the_theme_tabbar_background() {
        // The band is what every pill floats on, so it has to be the theme's
        // tabbar color rather than the content background.
        let theme = Theme::dark();
        let image = strip(&sample_menu_chrome(None));
        // The strip's very last row sits below every tab pill's bottom inset.
        let (r, g, b, _) = pixel(&image, 2, image.height - 1);
        assert_eq!(
            (r, g, b),
            (theme.tabbar_bg.r, theme.tabbar_bg.g, theme.tabbar_bg.b)
        );
    }

    #[test]
    fn test_active_tab_pill_is_distinct_from_the_band() {
        // The active tab has to read as active. If the pill resolved to the
        // band color it would vanish into the chrome.
        let tabbar = sample_menu_chrome(None);
        let image = strip(&tabbar);
        let layout = tabbar_layout(&tabbar, SURFACE_W, CELL_W, CELL_H);
        let tab = layout.tabs[tabbar.active_tab];
        let mid_x = (tab.x + tab.w / 2.0) as u32;
        let mid_y = (tab.y + tab.h / 2.0) as u32;
        let band = pixel(&image, 2, image.height - 1);
        assert_ne!(
            pixel(&image, mid_x, mid_y),
            band,
            "the active pill is indistinguishable from the band"
        );
    }

    #[test]
    fn test_strip_height_follows_the_menu_style() {
        // Classic stacks a menubar row above the tabs; Modern is a single
        // band. The rasterized height has to match whichever the caller
        // reserved, or the strip is drawn over the panes.
        let mut modern = sample_menu_chrome(None);
        modern.menu_style = MenuStyle::Modern;
        let mut classic = sample_menu_chrome(None);
        classic.menu_style = MenuStyle::Classic;
        assert_eq!(
            strip(&modern).height,
            crate::modern_titlebar_height_px(CELL_H).ceil() as u32
        );
        assert_eq!(
            strip(&classic).height,
            (crate::tabbar::tabbar_rows(MenuStyle::Classic) as f32 * CELL_H).ceil() as u32
        );
    }

    #[test]
    fn test_titlebar_button_hover_pills_are_equal_squares_centered_in_the_bar() {
        let theme = Theme::dark();
        let mut bar = sample_menu_chrome(None);
        bar.tabs.clear();
        bar.open_menu = None;
        bar.window_controls = true;
        let paint = |bar: &TopTabbar| tabbar_strip_rgba(bar, 600.0, CELL_W, CELL_H, &theme);
        let idle = paint(&bar);
        let mut sides = Vec::new();
        for hover in [
            TabbarHit::Hamburger,
            TabbarHit::Minimize,
            TabbarHit::Maximize,
            TabbarHit::Close,
        ] {
            bar.tabbar_hover = hover;
            let (_, y, w, h) = changed_box(&idle.rgba, &paint(&bar).rgba, idle.width);
            assert_eq!(w, h, "{hover:?} pill is {w}x{h}, not square");
            let mid = y as f32 + h as f32 / 2.0;
            assert!(
                (mid - idle.height as f32 / 2.0).abs() <= 0.5,
                "{hover:?} pill is centered at {mid} in a {}px bar",
                idle.height
            );
            sides.push(w);
        }
        assert!(
            sides.windows(2).all(|p| p[0] == p[1]),
            "pill sizes differ: {sides:?}"
        );
        // The same size as the pane strips' pills, not just alike in the bar.
        assert_eq!(sides[0] as f32, CELL_H, "pills are not one cell tall");
    }

    /// The center, in edge coordinates, of the icon drawn inside `pill` of a
    /// `width`-wide RGBA image: the box around every pixel that differs from
    /// the pill's own background (read at its corner).
    fn icon_center(rgba: &[u8], width: u32, pill: (f32, f32, f32, f32)) -> (f32, f32) {
        let (px, py, side, _) = pill;
        let at = |x: u32, y: u32| &rgba[((y * width + x) * 4) as usize..][..3];
        let (x0, y0) = (px as u32, py as u32);
        let bg = at(x0, y0).to_vec();
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (u32::MAX, u32::MAX, 0, 0);
        for y in y0..y0 + side as u32 {
            for x in x0..x0 + side as u32 {
                let diff: i32 = at(x, y)
                    .iter()
                    .zip(&bg)
                    .map(|(a, b)| (*a as i32 - *b as i32).abs())
                    .sum();
                if diff > 40 {
                    (min_x, min_y) = (min_x.min(x), min_y.min(y));
                    (max_x, max_y) = (max_x.max(x), max_y.max(y));
                }
            }
        }
        assert!(min_x != u32::MAX, "no icon inside the pill at {pill:?}");
        (
            (min_x + max_x + 1) as f32 / 2.0,
            (min_y + max_y + 1) as f32 / 2.0,
        )
    }

    #[test]
    fn test_every_button_icon_is_centered_on_its_pill() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let check = |what: &str, rgba: &[u8], width, pill: (f32, f32, f32, f32)| {
            let (ix, iy) = icon_center(rgba, width, pill);
            let (cx, cy) = pill_center(pill);
            assert!(
                (ix - cx).abs() < 0.01 && (iy - cy).abs() < 0.01,
                "{what}: icon centered at ({ix}, {iy}), pill at ({cx}, {cy})"
            );
        };

        // The title bar's hamburger and window controls.
        let mut bar = sample_menu_chrome(None);
        bar.tabs.clear();
        bar.open_menu = None;
        bar.window_controls = true;
        let image = tabbar_strip_rgba(&bar, 600.0, CELL_W, CELL_H, &theme);
        let layout = tabbar_layout(&bar, 600.0, CELL_W, CELL_H);
        let [minimize, maximize, close] = layout.controls.expect("controls present");
        let hamburger = layout.hamburger.expect("modern style has a hamburger");
        for (what, r) in [
            ("hamburger", hamburger),
            ("minimize", minimize),
            ("maximize", maximize),
            ("close", close),
        ] {
            let pill = button_pill(r.x + r.w / 2.0, r.y + r.h / 2.0, CELL_H);
            // The strip is the text-free pass: icons are drawn into it.
            check(what, &image.rgba, image.width, pill);
        }

        // A pane strip's close, new-tab and scroll buttons.
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
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
        let mid = (h - 2) as f32 / 2.0;
        let wide = layout_pane_strip(&tabs, 700.0, h as f32, CELL_W, CELL_H, 0);
        let narrow = layout_pane_strip(&tabs, 300.0, h as f32, CELL_W, CELL_H, 0);
        let mut paint = |layout: &PaneStripLayout, width: u32| {
            rasterize_pane_strip_rgba(
                &tabs,
                layout,
                width,
                h,
                CELL_W,
                CELL_H,
                &theme,
                PaneStripHit::None,
                Rgb::new(60, 60, 60),
                crate::tabbar::PaneStripDropTarget::default(),
                &mut fonts,
                &mut swash,
                &ctx,
            )
        };
        let wide_rgba = paint(&wide, 700);
        let close = wide.closes[0];
        let close_pill = button_pill(
            close.x + close.w / 2.0 + TAB_CLOSE_SHIFT_RIGHT_PX,
            mid,
            CELL_H,
        );
        check("pane close", &wide_rgba, 700, close_pill);
        let nt = wide.new_tab.expect("the button fits");
        check(
            "new tab",
            &wide_rgba,
            700,
            button_pill(nt.x + nt.w / 2.0, mid, CELL_H),
        );
        let narrow_rgba = paint(&narrow, 300);
        for (what, r) in [
            ("scroll left", narrow.scroll_left),
            ("scroll right", narrow.scroll_right),
        ] {
            let r = r.expect("the strip paginates");
            check(
                what,
                &narrow_rgba,
                300,
                button_pill(r.x + r.w / 2.0, mid, CELL_H),
            );
        }
    }
}

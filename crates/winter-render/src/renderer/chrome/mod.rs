//! Chrome rasterization: the tabbar strip, dropdowns and menus, the
//! command palette, which-key hints, toasts, and URL tooltips, painted
//! into RGBA buffers for the image pass.

mod overlay;
mod paint;
mod view;

pub(crate) use view::TabbarText;
pub use view::{PaletteItem, PaletteView, WhichKeyView};

use super::colors::*;
use super::glyphs::*;
use super::GpuRenderer;
use super::{NoticeKind, StatusNotice};
use crate::image::ImagePlacement;
use crate::tabbar::{
    self, layout as tabbar_layout, Region, TabbarHit, TopTabbar, HOVER_PILL_H_PAD_CELLS,
    NEW_TAB_BOTTOM_INSET_RATIO, TAB_H_PAD_CELLS, ZOOM_CELLS,
};
use crate::theme::Rgb;
use glyphon::Color;
use overlay::{
    context_menu_rgba, dropdown_rgba, palette_rgba, submenu_rgba, toast_rgba, url_tooltip_rgba,
    which_key_rgba,
};
use paint::{fill_line_segment, fill_rounded_rect, shape_chrome_line, text_bounds, truncate_label};

// ========================================================================
// Constants
// ========================================================================

/// Reserved image-pass texture ids for the rasterized menu overlays. Set to the
/// top of the id space so they never collide with block image ids.
pub(super) const DROPDOWN_TEXTURE_ID: u64 = u64::MAX;

pub(super) const SUBMENU_TEXTURE_ID: u64 = u64::MAX - 1;

pub(super) const CONTEXT_MENU_TEXTURE_ID: u64 = u64::MAX - 5;

/// Reserved id for the rasterized top-tabbar strip (band + rounded tab pills).
pub(super) const TABBAR_STRIP_TEXTURE_ID: u64 = u64::MAX - 2;

/// Reserved id for the rasterized command-palette overlay.
pub(super) const PALETTE_TEXTURE_ID: u64 = u64::MAX - 3;

/// Reserved id for the rasterized URL hover tooltip.
pub(super) const URL_TOOLTIP_TEXTURE_ID: u64 = u64::MAX - 6;

/// Reserved id for the rasterized transient toast pill.
pub(super) const TOAST_TEXTURE_ID: u64 = u64::MAX - 7;

/// Reserved id for the rasterized which-key hint popup.
pub(super) const WHICH_KEY_TEXTURE_ID: u64 = u64::MAX - 8;

/// Maximum number of command results visible in the palette at once.
pub(super) const PALETTE_MAX_ITEMS: usize = 8;

/// Palette panel width as a fraction of the surface width.
pub(super) const PALETTE_WIDTH_RATIO: f32 = 0.62;

/// Palette panel top edge as a fraction of the surface height (VS Code style).
pub(super) const PALETTE_TOP_RATIO: f32 = 0.15;

/// Extra horizontal padding inside each palette result row, in pixels, applied
/// to both the left label and the right shortcut hint.
pub(super) const PALETTE_ITEM_PAD_X: f32 = 5.0;

/// Vertical padding above and below the text in each palette result row, in
/// pixels. The row height is one line of text plus this on each side, so the
/// inter-row gap is a small fixed amount rather than a multiple of the line.
pub(super) const PALETTE_ITEM_PAD_Y: f32 = 4.0;

/// Corner radius of the dropdown menu panel and its hover highlight, in pixels.
pub(super) const DROPDOWN_RADIUS: f32 = 12.0;

/// Width of the soft drop shadow cast around the dropdown panel, in pixels.
pub(super) const DROPDOWN_SHADOW: f32 = 22.0;

/// How far non-focused pane colors are lerped toward the background (0 = full, 1 = fully dimmed).
pub(super) const DIM_FACTOR: f32 = 0.4;

/// Peak opacity of the dropdown drop shadow, fading to zero at its outer edge.
pub(super) const DROPDOWN_SHADOW_ALPHA: f32 = 0.3;

/// Inset of the hover-highlight pill from the dropdown item-row edges, in pixels.
pub(super) const MENU_HOVER_INSET: f32 = 6.0;

/// Strength of the dropdown panel's hairline border, mixed from the surface
/// toward white for a crisp, elevated edge against the content behind it.
pub(super) const MENU_BORDER_MIX: f32 = 0.14;

/// Corner radius of the rounded tab tops, as a fraction of the cell height.
pub(super) const TAB_CORNER_RADIUS_RATIO: f32 = 0.34;

/// Flat pixels the close button (× glyph and its hover pill) is shifted
/// right of its horizontally-centered position within its close slot.
pub(super) const TAB_CLOSE_SHIFT_RIGHT_PX: f32 = 3.0;

/// Flat pixels the per-tab close button (× glyph and its hover pill) is
/// shifted up from its vertically-centered position within its close slot.
pub(super) const TAB_CLOSE_SHIFT_UP_PX: f32 = 1.0;

/// Flat pixels the tab title label is shifted up from its vertically-
/// centered position on the tab shape.
pub(super) const TAB_LABEL_SHIFT_UP_PX: f32 = 1.0;

/// Flat pixels trimmed off the top of the hamburger/minimize/maximize/close
/// control buttons' shared hover pill, shrinking its height from the top
/// edge only (the bottom edge is unchanged).
pub(super) const CONTROL_HOVER_PILL_TOP_TRIM_PX: f32 = 2.0;

/// Flat pixels the hamburger/minimize/maximize/close control buttons (icons
/// and their shared hover pill) are shifted up, as a group, from their
/// vertically-centered position on the tab shape.
pub(super) const CONTROL_SHIFT_UP_PX: f32 = 0.5;

/// Flat pixels the new-tab `+` glyph is shifted down from its vertically-
/// centered position on the tab shape.
pub(super) const NEW_TAB_GLYPH_SHIFT_DOWN_PX: f32 = 1.0;

/// Flat pixels trimmed off the top of the new-tab button's own hover pill
/// (the small pill behind the `+` glyph), shrinking its height from the top
/// edge only (the bottom edge is unchanged).
pub(super) const NEW_TAB_HOVER_PILL_TOP_TRIM_PX: f32 = 2.0;

/// How far the active tab's background is mixed toward `theme.foreground`
/// from `theme.tab_active_bg`, so it reads as clearly highlighted rather
/// than blending into the content view below it.
pub(super) const TAB_ACTIVE_HIGHLIGHT: f32 = 0.06;

/// Opacity of the hairline separating the tab band from the content below.
pub(super) const TABBAR_BORDER_ALPHA: f32 = 0.5;

/// Font-size multiplier for the tab close (`×`) and zoom glyphs, so they read
/// larger than the title text against the taller tab band.
pub(super) const TAB_BUTTON_GLYPH_RATIO: f32 = 1.25;

/// Glyph drawn left of the close button when a tab's pane is zoomed to fill the
/// viewport. Matches the window-control maximize glyph (U+25A1).
pub(super) const TAB_ZOOM_GLYPH: &str = "\u{25a1}";

/// Color of the dropdown drop shadow (alpha is applied per pixel).
pub(super) const SHADOW_COLOR: Rgb = Rgb::new(0, 0, 0);

// ========================================================================
// Implementation
// ========================================================================

impl GpuRenderer {
    /// Append the top tabbar's background quads to `verts` and return its text
    /// runs (tab titles, close glyphs, the new-tab button, and either the modern
    /// hamburger or the classic menu titles). The dropdown is drawn separately.
    pub(super) fn draw_tabbar(&mut self, tabbar: &TopTabbar, surface_w: f32) -> Vec<TabbarText> {
        let cw = self.cell_width;
        let ch = self.cell_height;
        let layout = tabbar_layout(tabbar, surface_w, cw, ch);
        let pad = cw * 0.4;
        let muted = self.theme.ansi[8].to_glyphon();
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
        let line_h = self.line_height;
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
            let avail = (title_end - (title_x + pad)).max(cw);
            let max_chars = (avail / cw).floor() as usize;
            // Prefix every tab title with its 1-based tab number so tabs are
            // always identifiable (and scrolled tabs stay so).
            let labeled = format!("{}: {}", i + 1, tabbar.tabs[i].title);
            let title = truncate_label(&labeled, max_chars);
            let active = i == tabbar.active_tab;
            let color = if active { active_tab } else { inactive_tab };
            // Tab titles use the regular (non-bold) sans-serif UI weight, like
            // VS Code's tab labels; the active tab is distinguished by its color
            // and pill.
            let buffer = self.tabbar_line_buffer(&title, color, false, true);
            texts.push(TabbarText {
                bounds: text_bounds(title_x, tab.y, title_end, tab.y + tab.h),
                buffer,
                color,
                left: title_x + pad,
                top: vcenter(
                    tab.y + tab_top_inset,
                    tab.h - tab_top_inset - tab_bottom_inset,
                ) - TAB_LABEL_SHIFT_UP_PX,
            });

            // Zoom/restore icon, drawn only while the pane is zoomed.
            if let Some(zoom_x) = zoom {
                let glyph_cw = cw * TAB_BUTTON_GLYPH_RATIO;
                let glyph_ch = self.line_height * TAB_BUTTON_GLYPH_RATIO;
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

        // New-tab button: a bigger `+`, like the zoom glyph, centered on the
        // "tab shape" (inset top and bottom, same as a real tab pill) rather
        // than the hover pill's own shorter, doubly-inset bounds.
        let new_tab = layout.new_tab;
        let tab_shape_top = new_tab.y + tab_top_inset;
        let tab_shape_h = new_tab.h - tab_top_inset - tab_bottom_inset;
        let plus_cw = cw * TAB_BUTTON_GLYPH_RATIO;
        let plus_ch = self.line_height * TAB_BUTTON_GLYPH_RATIO;
        let plus = self.tabbar_button_buffer("+", muted, TAB_BUTTON_GLYPH_RATIO);
        texts.push(TabbarText {
            bounds: text_bounds(
                new_tab.x,
                new_tab.y,
                new_tab.x + new_tab.w,
                new_tab.y + new_tab.h,
            ),
            buffer: plus,
            color: muted,
            left: new_tab.x + (new_tab.w - plus_cw) / 2.0,
            top: tab_shape_top + (tab_shape_h - plus_ch) / 2.0 + NEW_TAB_GLYPH_SHIFT_DOWN_PX,
        });

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

        texts
    }

    /// Rasterize the command palette overlay and upload it to the GPU. Returns
    /// an empty vec when the palette has no items to display.
    pub(super) fn rasterize_palette(
        &mut self,
        palette: &PaletteView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        let image = palette_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            palette,
            surface_w,
            surface_h,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            PALETTE_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        vec![ImagePlacement {
            alpha: 1.0,
            height: image.height as f32,
            id: PALETTE_TEXTURE_ID,
            v_max: 1.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        }]
    }

    /// Rasterize the which-key hint overlay and upload it to the GPU.
    pub(super) fn rasterize_which_key(
        &mut self,
        which_key: &WhichKeyView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        let image = which_key_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            which_key,
            surface_w,
            surface_h,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            WHICH_KEY_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        vec![ImagePlacement {
            alpha: 1.0,
            height: image.height as f32,
            id: WHICH_KEY_TEXTURE_ID,
            v_max: 1.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        }]
    }

    /// Rasterize a URL tooltip bubble near the cursor when a link is hovered.
    /// Returns `None` when no URL tooltip is set.
    pub(super) fn rasterize_url_tooltip(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
        surface_h: f32,
    ) -> Option<ImagePlacement> {
        let (url, cx, cy) = tabbar.url_tooltip.as_ref()?;
        let cw = self.cell_width;
        let ch = self.cell_height;
        let ctx = FontCtx {
            cell_h: ch,
            cell_w: cw,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        let anchor = Region {
            x: *cx,
            y: *cy,
            w: 1.0,
            h: ch,
        };
        let is_tab = matches!(tabbar.tabbar_hover, crate::tabbar::TabbarHit::Tab(_));
        let tab_layout = if is_tab {
            Some(tabbar_layout(tabbar, surface_w, cw, ch))
        } else {
            None
        };
        let hovered_tab = match tabbar.tabbar_hover {
            crate::tabbar::TabbarHit::Tab(idx) => Some(idx),
            _ => None,
        };
        let image = url_tooltip_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            url,
            &anchor,
            surface_w,
            surface_h,
            tab_layout.as_ref(),
            hovered_tab,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            URL_TOOLTIP_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        Some(ImagePlacement {
            alpha: 1.0,
            height: image.height as f32,
            id: URL_TOOLTIP_TEXTURE_ID,
            v_max: 1.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        })
    }

    /// Rasterize a transient toast pill (e.g. "Copied to clipboard") in the
    /// top-right corner, tucked just below the tab bar. Used to surface a notice
    /// when the status bar is hidden. Info notices read in green, errors in red.
    pub(super) fn rasterize_toast(
        &mut self,
        notice: &StatusNotice,
        surface_w: f32,
    ) -> Option<ImagePlacement> {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        let text_color = match notice.kind {
            NoticeKind::Error => self.theme.ansi[1],
            NoticeKind::Info => self.theme.ansi[4],
        };
        // Tuck the toast below the tab bar so it never overlaps the tabs. Mirrors
        // the top-inset math used elsewhere (rows of tabbar chrome × cell height).
        let top_inset = if self.tabbar_enabled {
            if self.modern {
                crate::modern_tabbar_height_px(self.cell_height)
            } else {
                2.0 * self.cell_height
            }
        } else {
            0.0
        };
        let image = toast_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            &notice.text,
            text_color,
            surface_w,
            top_inset,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            TOAST_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        Some(ImagePlacement {
            alpha: 1.0,
            height: image.height as f32,
            id: TOAST_TEXTURE_ID,
            v_max: 1.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        })
    }

    /// Rasterize the top-tabbar strip, the recessed band, the rounded tab
    /// cards, and the hairline border, into a texture composited under the tabbar
    /// text. Tabs float as fully-rounded pills, inset top and bottom; the active
    /// tab is filled with the terminal background so it reads as selected, while
    /// inactive tabs sit recessed and darker.
    pub(super) fn rasterize_tabbar_strip(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
    ) -> Option<ImagePlacement> {
        let cw = self.cell_width;
        let ch = self.cell_height;
        let layout = tabbar_layout(tabbar, surface_w, cw, ch);
        let tabbar_h = if tabbar.menu_style == crate::tabbar::MenuStyle::Modern {
            crate::modern_tabbar_height_px(ch)
        } else {
            tabbar::tabbar_rows(tabbar.menu_style) as f32 * ch
        };
        let width = surface_w.ceil().max(1.0) as u32;
        let height = tabbar_h.ceil().max(1.0) as u32;
        let canvas = (width, height);
        let band = self.theme.tabbar_bg;
        // Mixed toward the foreground (a neutral shift, not a color tint)
        // rather than left equal to the content background: a floating tab
        // pill in exactly the background color would read as chrome, not as
        // "this is the active tab".
        let active_bg = mix_rgb(
            self.theme.tab_active_bg,
            self.theme.foreground,
            TAB_ACTIVE_HIGHLIGHT,
        );
        let inactive_bg = mix_rgb(self.theme.tabbar_bg, self.theme.tab_active_bg, 0.5);

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
        if let Some(menubar_top) = layout.menubar_top {
            let _ = menubar_top;
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (0.0, layout.tab_row_top - 1.0, width as f32, 1.0),
                0.0,
                self.theme.divider,
                TABBAR_BORDER_ALPHA,
            );
        }

        // The hamburger, close, and new-tab hover pills are inset on all sides
        // so they read as floating buttons. Tabs themselves now match: inset
        // by the same amount on top and bottom (Brave-style floating pills,
        // all four corners rounded) instead of sitting flush against the
        // strip's bottom edge. `TAB_GAP_PX` still opens a hairline gap between
        // neighbors — the hit-test `Region`s stay flush, only the rendered
        // pill shrinks.
        let tab_vpad_top = tabbar::tab_top_inset_px(tabbar.menu_style);
        let tab_vpad_bottom = tabbar::TAB_BOTTOM_VPAD_PX;
        let pill_radius = ch * TAB_CORNER_RADIUS_RATIO;

        let hover_tab = match tabbar.tabbar_hover {
            TabbarHit::Tab(i) | TabbarHit::CloseTab(i) => Some(i),
            _ => None,
        };
        for (i, tab) in layout.tabs.iter().enumerate() {
            if tab.w <= 0.0 {
                continue;
            }
            let is_active = i == tabbar.active_tab;
            let base = if is_active { active_bg } else { inactive_bg };
            let color = if hover_tab == Some(i) && !is_active {
                mix_rgb(inactive_bg, active_bg, 0.55)
            } else {
                base
            };
            let bounds = (
                tab.x + tabbar::TAB_GAP_PX / 2.0,
                tab.y + tab_vpad_top,
                tab.w - tabbar::TAB_GAP_PX,
                tab.h - tab_vpad_top - tab_vpad_bottom,
            );
            fill_rounded_rect(&mut rgba, canvas, bounds, pill_radius, color, 1.0);
            // Close button hover: rounded rectangle highlight over the × region.
            // Centered on the tab shape (inset top and bottom), like the ×
            // glyph itself below, not the taller full element region.
            if let TabbarHit::CloseTab(hi) = tabbar.tabbar_hover {
                if hi == i {
                    let close = layout.closes[i];
                    let hpad = cw * 0.15;
                    let vpad = ch * 0.2;
                    let close_hover = mix_rgb(color, self.theme.foreground, 0.18);
                    fill_rounded_rect(
                        &mut rgba,
                        canvas,
                        (
                            close.x + hpad + TAB_CLOSE_SHIFT_RIGHT_PX,
                            close.y + tab_vpad_top + vpad - TAB_CLOSE_SHIFT_UP_PX,
                            close.w - hpad * 2.0,
                            close.h - tab_vpad_top - tab_vpad_bottom - vpad * 2.0,
                        ),
                        pill_radius,
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
                    self.theme.foreground
                } else {
                    self.theme.ansi[8]
                };
                let cx = close.x + close.w / 2.0 + TAB_CLOSE_SHIFT_RIGHT_PX;
                let cy = close.y + tab_vpad_top + (close.h - tab_vpad_top - tab_vpad_bottom) / 2.0
                    - TAB_CLOSE_SHIFT_UP_PX;
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
                    1.0,
                );
                fill_line_segment(
                    &mut rgba,
                    canvas,
                    (cx - half, cy + half),
                    (cx + half, cy - half),
                    thickness,
                    close_color,
                    1.0,
                );
            }
        }

        // New-tab's own horizontal hover padding — the reference width every
        // other titlebar button's hover pill now matches (see below). Shared
        // with `tabbar::layout`, which reserves extra spacing around the
        // narrower buttons so this wider pill never bleeds into a neighbor.
        let new_tab_hpad = HOVER_PILL_H_PAD_CELLS * cw;

        // New-tab button hover: small pill behind the + glyph. Same style as
        // every other titlebar button's hover pill — top aligned with a
        // tab's own top inset, fully rounded — but with extra bottom inset of
        // its own so it reads shorter than the others.
        if tabbar.tabbar_hover == TabbarHit::NewTab {
            let nt = layout.new_tab;
            let new_tab_vpad_bottom = NEW_TAB_BOTTOM_INSET_RATIO * ch;
            let new_tab_hover = mix_rgb(band, self.theme.foreground, 0.12);
            fill_rounded_rect(
                &mut rgba,
                canvas,
                (
                    nt.x + new_tab_hpad,
                    nt.y + tab_vpad_top + NEW_TAB_HOVER_PILL_TOP_TRIM_PX,
                    nt.w - new_tab_hpad * 2.0,
                    nt.h - tab_vpad_top - new_tab_vpad_bottom - NEW_TAB_HOVER_PILL_TOP_TRIM_PX,
                ),
                pill_radius,
                new_tab_hover,
                1.0,
            );
        }

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
                let hpad = cw * 0.2;
                let vpad = ch * 0.1;
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (
                        r.x + hpad,
                        r.y + tab_vpad_top,
                        r.w - hpad * 2.0,
                        r.h - tab_vpad_top - vpad,
                    ),
                    pill_radius,
                    mix_rgb(band, self.theme.foreground, 0.12),
                    1.0,
                );
            }
            let color = if !active {
                // Dim toward the band so the arrow reads as disabled.
                mix_rgb(band, self.theme.ansi[8], 0.4)
            } else if hovered {
                self.theme.foreground
            } else {
                self.theme.ansi[8]
            };
            let cx = r.x + r.w / 2.0;
            let cy = r.y + r.h / 2.0;
            let half = (ch * 0.2).round();
            let reach = half * 0.6;
            let thickness = 1.5;
            // Tip points in the travel direction; the two strokes form a chevron.
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

        // Every titlebar-edge button's hover pill is the same fixed width —
        // new-tab's own (`new_tab_hpad` inset from its box) — centered
        // horizontally in whichever button is hovered, regardless of that
        // button's own box width. Only the background color varies between
        // buttons, not the pill's footprint. Vertically, its top edge sits at
        // `tab_vpad_top` plus `CONTROL_HOVER_PILL_TOP_TRIM_PX` (a bit further
        // down than a tab's own top), so every hover pill's top aligns with
        // the others instead of floating centered in the (taller) titlebar
        // band; height still comes from close's own box.
        let vpad = ch * 0.1;
        let hover_pill_w = layout.new_tab.w - new_tab_hpad * 2.0;
        let hover_pill_top_inset = tab_vpad_top + CONTROL_HOVER_PILL_TOP_TRIM_PX;
        let hover_pill_h = layout
            .controls
            .map(|[_, _, close]| close.h - hover_pill_top_inset - vpad);

        // Hamburger pill uses the same vertical inset and radius as tabs.
        if let Some(hb) = layout.hamburger {
            let is_hovered = tabbar.tabbar_hover == TabbarHit::Hamburger;
            let is_open = tabbar.open_menu.is_some();
            if is_hovered || is_open {
                let pw = hover_pill_w;
                let ph = hover_pill_h.unwrap_or(hb.h - hover_pill_top_inset - vpad);
                let px = hb.x + (hb.w - pw) / 2.0;
                let py = hb.y + hover_pill_top_inset - CONTROL_SHIFT_UP_PX;
                let btn_color = mix_rgb(band, self.theme.foreground, 0.12);
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (px, py, pw, ph),
                    pill_radius,
                    btn_color,
                    1.0,
                );
            }

            // Draw modern vector hamburger icon (3 clean horizontal lines),
            // centered on the tab shape (inset top and bottom) to align
            // horizontally with the tab label.
            let color = self.theme.foreground;
            let alpha = if is_hovered || is_open { 1.0 } else { 0.62 };
            let cx = hb.x + hb.w / 2.0;
            let cy = hb.y + tab_vpad_top + (hb.h - tab_vpad_top - tab_vpad_bottom) / 2.0
                - CONTROL_SHIFT_UP_PX;
            let size = (ch * 0.40).round();
            let half = size / 2.0;
            let thickness = 1.25;
            let line_spacing = (ch * 0.14).round().max(3.0);

            fill_line_segment(
                &mut rgba,
                canvas,
                (cx - half, cy - line_spacing),
                (cx + half, cy - line_spacing),
                thickness,
                color,
                alpha,
            );
            fill_line_segment(
                &mut rgba,
                canvas,
                (cx - half, cy),
                (cx + half, cy),
                thickness,
                color,
                alpha,
            );
            fill_line_segment(
                &mut rgba,
                canvas,
                (cx - half, cy + line_spacing),
                (cx + half, cy + line_spacing),
                thickness,
                color,
                alpha,
            );
        }

        // Window controls hover highlight and vector icons
        if let Some([minimize, maximize, close]) = layout.controls {
            let hover_control = match tabbar.tabbar_hover {
                TabbarHit::Minimize => Some((minimize, mix_rgb(band, self.theme.foreground, 0.12))),
                TabbarHit::Maximize => Some((maximize, mix_rgb(band, self.theme.foreground, 0.12))),
                TabbarHit::Close => Some((close, Rgb::new(220, 60, 60))),
                _ => None,
            };
            if let Some((region, color)) = hover_control {
                let pw = hover_pill_w;
                let ph =
                    hover_pill_h.expect("controls present, so close's height was computed above");
                let px = region.x + (region.w - pw) / 2.0;
                let py = region.y + hover_pill_top_inset - CONTROL_SHIFT_UP_PX;
                fill_rounded_rect(&mut rgba, canvas, (px, py, pw, ph), pill_radius, color, 1.0);
            }

            let size = (ch * 0.45).round();
            let thickness = 1.35;
            let fg_color = self.theme.foreground;
            let muted_alpha = 0.62;

            // Draw Minimize icon (horizontal line), centered on the tab shape
            // (inset top and bottom) to align with the tab label.
            {
                let is_hovered = tabbar.tabbar_hover == TabbarHit::Minimize;
                let alpha = if is_hovered { 1.0 } else { muted_alpha };
                let cx = minimize.x + minimize.w / 2.0;
                let cy =
                    minimize.y + tab_vpad_top + (minimize.h - tab_vpad_top - tab_vpad_bottom) / 2.0
                        - CONTROL_SHIFT_UP_PX;
                fill_line_segment(
                    &mut rgba,
                    canvas,
                    (cx - size / 2.0, cy),
                    (cx + size / 2.0, cy),
                    thickness,
                    fg_color,
                    alpha,
                );
            }

            // Draw Maximize icon (hollow square), centered on the tab shape
            // (inset top and bottom) to align with the tab label.
            {
                let is_hovered = tabbar.tabbar_hover == TabbarHit::Maximize;
                let alpha = if is_hovered { 1.0 } else { muted_alpha };
                let cx = maximize.x + maximize.w / 2.0;
                let cy =
                    maximize.y + tab_vpad_top + (maximize.h - tab_vpad_top - tab_vpad_bottom) / 2.0
                        - CONTROL_SHIFT_UP_PX;
                let sq_x = cx - size / 2.0;
                let sq_y = cy - size / 2.0;

                // Top border
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (sq_x, sq_y, size, thickness),
                    0.0,
                    fg_color,
                    alpha,
                );
                // Bottom border
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (sq_x, sq_y + size - thickness, size, thickness),
                    0.0,
                    fg_color,
                    alpha,
                );
                // Left border
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
                    (sq_x, sq_y + thickness, thickness, size - thickness * 2.0),
                    0.0,
                    fg_color,
                    alpha,
                );
                // Right border
                fill_rounded_rect(
                    &mut rgba,
                    canvas,
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

            // Draw Close icon (X), centered on the tab shape (inset top and
            // bottom) to align with the tab label.
            {
                let is_hovered = tabbar.tabbar_hover == TabbarHit::Close;
                let alpha = if is_hovered { 1.0 } else { muted_alpha };
                let cx = close.x + close.w / 2.0;
                let cy = close.y + tab_vpad_top + (close.h - tab_vpad_top - tab_vpad_bottom) / 2.0
                    - CONTROL_SHIFT_UP_PX;
                // Half-diagonal, not half-side: an X's corner-to-corner span is
                // its side length times sqrt(2), so `size / 2.0` here would read
                // visually larger than minimize's `size`-long line and
                // maximize's `size`-wide square. Scaling by `1 / sqrt(2)` makes
                // the X's diagonal reach exactly `size`, matching both.
                let half = size / (2.0 * std::f32::consts::SQRT_2);

                fill_line_segment(
                    &mut rgba,
                    canvas,
                    (cx - half, cy - half),
                    (cx + half, cy + half),
                    thickness,
                    fg_color,
                    alpha,
                );
                fill_line_segment(
                    &mut rgba,
                    canvas,
                    (cx - half, cy + half),
                    (cx + half, cy - half),
                    thickness,
                    fg_color,
                    alpha,
                );
            }
        }

        self.tabbar_strip_pass.upload(
            &self.device,
            &self.queue,
            TABBAR_STRIP_TEXTURE_ID,
            &rgba,
            width,
            height,
        );
        Some(ImagePlacement {
            alpha: 1.0,
            height: height as f32,
            id: TABBAR_STRIP_TEXTURE_ID,
            v_max: 1.0,
            width: width as f32,
            x: 0.0,
            y: 0.0,
        })
    }

    /// Rasterize the open dropdown to a texture and return its placement, or
    /// `None` when no menu is open. The pixel work lives in [`dropdown_rgba`] so
    /// it can run (and be tested) without the GPU; this only uploads the result.
    /// Rasterize and place the open menu's overlays: the parent dropdown, and the
    /// open submenu (drawn after, so it overlays the parent). Returns one
    /// placement per visible panel, or empty when no menu is open.
    pub(super) fn rasterize_dropdown(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
    ) -> Vec<ImagePlacement> {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        // Computed sequentially: each borrows the shared font system in turn.
        let parent = dropdown_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            tabbar,
            surface_w,
        );
        let submenu = submenu_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            tabbar,
            surface_w,
        );
        let context_image = context_menu_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            tabbar,
            surface_w,
        );
        let mut placements = Vec::new();
        for (id, image) in [
            (DROPDOWN_TEXTURE_ID, parent),
            (SUBMENU_TEXTURE_ID, submenu),
            (CONTEXT_MENU_TEXTURE_ID, context_image),
        ] {
            let Some(image) = image else { continue };
            self.image_pass.upload(
                &self.device,
                &self.queue,
                id,
                &image.rgba,
                image.width,
                image.height,
            );
            placements.push(ImagePlacement {
                alpha: 1.0,
                height: image.height as f32,
                id,
                v_max: 1.0,
                width: image.width as f32,
                x: image.x,
                y: image.y,
            });
        }
        placements
    }

    /// Shape a single line of tabbar text into a reusable buffer. With
    /// `proportional` set, it uses a sans-serif UI font (for the dropdown menu)
    /// rather than the terminal's monospace family.
    pub(super) fn tabbar_line_buffer(
        &mut self,
        text: &str,
        color: Color,
        bold: bool,
        proportional: bool,
    ) -> glyphon::Buffer {
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size,
            line_height: self.line_height,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        shape_chrome_line(&mut self.font_system, &ctx, text, color, bold, proportional)
    }

    /// Shape a single tab-button glyph (close `×`, zoom icon) at a scaled-up
    /// font size so it reads as a control rather than body text. The terminal's
    /// monospace family is used (matching the window-control glyphs) so each
    /// glyph's advance is exactly one scaled cell, keeping the cw-based
    /// centering exact and preventing overflow into a neighbour's clip rect.
    pub(super) fn tabbar_button_buffer(
        &mut self,
        glyph: &str,
        color: Color,
        ratio: f32,
    ) -> glyphon::Buffer {
        let scaled_line_h = (self.line_height * ratio).round();
        let ctx = FontCtx {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            family: self.font_family.as_deref(),
            font_has_bold: self.font_has_bold,
            font_size: self.font_size * ratio,
            line_height: scaled_line_h,
            normal_weight: self.normal_weight.as_deref(),
            bold_weight: self.bold_weight.as_deref(),
        };
        shape_chrome_line(&mut self.font_system, &ctx, glyph, color, false, false)
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {}

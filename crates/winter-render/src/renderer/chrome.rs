//! Chrome rasterization: the tabbar strip, dropdowns and menus, the
//! command palette, which-key hints, toasts, and URL tooltips, painted
//! into RGBA buffers for the image pass.

use super::*;

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

/// One shaped text run of the top tabbar (a tab title, a menu title, a glyph),
/// plus where to place and clip it. Built fresh each frame and kept alive until
/// after `glyphon` prepares the text pass.
pub(super) struct TabbarText {
    pub(super) bounds: TextBounds,
    pub(super) buffer: glyphon::Buffer,
    pub(super) color: Color,
    pub(super) left: f32,
    pub(super) top: f32,
}

/// A rasterized dropdown overlay: its pixels and the surface position to place
/// them at (top-left, already offset to include the shadow margin).
pub(super) struct DropdownImage {
    pub(super) height: u32,
    pub(super) rgba: Vec<u8>,
    pub(super) width: u32,
    pub(super) x: f32,
    pub(super) y: f32,
}

/// One entry shown in the command palette results list.
pub struct PaletteItem {
    /// Command id dispatched when this entry is chosen.
    pub action: String,
    /// Human-readable entry text shown in the list.
    pub label: String,
    /// Char indices in `label` that matched the query, used to highlight them.
    pub match_positions: Vec<usize>,
    /// Keyboard shortcut hint shown on the right (e.g. `"Ctrl-Shift-T"`).
    pub shortcut: String,
}

/// The command palette state the renderer needs to draw its overlay.
pub struct PaletteView {
    /// Text shown when the filtered list is empty.
    pub empty_message: String,
    /// The entries to draw, already filtered and ranked by the caller.
    pub items: Vec<PaletteItem>,
    /// Draw an underline under highlighted match characters.
    pub match_underline: bool,
    /// The filter text as typed.
    pub query: String,
    /// Index into `items` of the highlighted entry.
    pub selected: usize,
}

/// The which-key hint overlay state shown when the user pauses mid-prefix.
#[derive(Clone, Debug)]
pub struct WhichKeyView {
    /// The continuations available from the pending prefix, as
    /// `(key, description)`.
    pub items: Vec<(String, String)>,
    /// The prefix already typed, shown as the popup's heading.
    pub title: String,
}

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

/// A glyphon clip rect from pixel edges.
pub(super) fn text_bounds(left: f32, top: f32, right: f32, bottom: f32) -> TextBounds {
    TextBounds {
        left: left as i32,
        top: top as i32,
        right: right as i32,
        bottom: bottom as i32,
    }
}

/// Shorten `s` to at most `max_chars`, appending an ellipsis when truncated.
pub(super) fn truncate_label(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return s.to_string();
    }
    match max_chars {
        0 => String::new(),
        1 => "\u{2026}".to_string(),
        _ => {
            let mut out: String = s.chars().take(max_chars - 1).collect();
            out.push('\u{2026}');
            out
        }
    }
}

/// Fill an opaque `(x, y, w, h)` rectangle of `color` into a `(canvas_w,
/// canvas_h)` `rgba` buffer, clipped to the canvas.
/// Alpha-composite `color` over the pixel at byte `idx` of an RGBA buffer.
pub(super) fn blend_px(rgba: &mut [u8], idx: usize, color: Rgb, alpha: f32) {
    let a = alpha.clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    rgba[idx] = (color.r as f32 * a + rgba[idx] as f32 * inv) as u8;
    rgba[idx + 1] = (color.g as f32 * a + rgba[idx + 1] as f32 * inv) as u8;
    rgba[idx + 2] = (color.b as f32 * a + rgba[idx + 2] as f32 * inv) as u8;
    let out_a = a + (rgba[idx + 3] as f32 / 255.0) * inv;
    rgba[idx + 3] = (out_a * 255.0) as u8;
}

/// Signed distance from `(px, py)` to the rounded rectangle `(x, y, w, h)` with
/// corner `radius`: negative inside, zero on the edge, positive outside.
pub(super) fn rounded_rect_sdf(px: f32, py: f32, rect: (f32, f32, f32, f32), radius: f32) -> f32 {
    let (rx, ry, rw, rh) = rect;
    let half_w = rw / 2.0;
    let half_h = rh / 2.0;
    let r = radius.min(half_w).min(half_h);
    let qx = (px - (rx + half_w)).abs() - (half_w - r);
    let qy = (py - (ry + half_h)).abs() - (half_h - r);
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

/// Fill a rounded rectangle into `rgba`, anti-aliasing the edge over a 1px band
/// and compositing at up to `max_alpha` opacity. All four corners share `radius`.
/// Fill a rounded rectangle into `rgba`, anti-aliasing the edge over a 1px band
/// and compositing at up to `max_alpha` opacity.
pub(super) fn fill_rounded_rect(
    rgba: &mut [u8],
    canvas: (u32, u32),
    rect: (f32, f32, f32, f32),
    radius: f32,
    color: Rgb,
    max_alpha: f32,
) {
    let (canvas_w, canvas_h) = canvas;
    let (rx, ry, rw, rh) = rect;
    let x0 = rx.floor().max(0.0) as u32;
    let y0 = ry.floor().max(0.0) as u32;
    let x1 = ((rx + rw).ceil() as i64).clamp(0, canvas_w as i64) as u32;
    let y1 = ((ry + rh).ceil() as i64).clamp(0, canvas_h as i64) as u32;
    for py in y0..y1 {
        for px in x0..x1 {
            let sdf = rounded_rect_sdf(px as f32 + 0.5, py as f32 + 0.5, rect, radius);
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage <= 0.0 {
                continue;
            }
            let idx = ((py * canvas_w + px) * 4) as usize;
            blend_px(rgba, idx, color, coverage * max_alpha);
        }
    }
}

/// Fill an anti-aliased line segment from `p1` to `p2` with given `thickness` and color.
pub(super) fn fill_line_segment(
    rgba: &mut [u8],
    canvas: (u32, u32),
    p1: (f32, f32),
    p2: (f32, f32),
    thickness: f32,
    color: Rgb,
    max_alpha: f32,
) {
    let (canvas_w, canvas_h) = canvas;
    let (x0, y0) = p1;
    let (x1, y1) = p2;
    let bx0 = x0.min(x1) - thickness;
    let by0 = y0.min(y1) - thickness;
    let bx1 = x0.max(x1) + thickness;
    let by1 = y0.max(y1) + thickness;

    let rx0 = bx0.floor().max(0.0) as u32;
    let ry0 = by0.floor().max(0.0) as u32;
    let rx1 = (bx1.ceil() as u32).min(canvas_w);
    let ry1 = (by1.ceil() as u32).min(canvas_h);

    let dx = x1 - x0;
    let dy = y1 - y0;
    let len_sq = dx * dx + dy * dy;

    for py in ry0..ry1 {
        for px in rx0..rx1 {
            let px_f = px as f32 + 0.5;
            let py_f = py as f32 + 0.5;

            let wx = px_f - x0;
            let wy = py_f - y0;

            let t = if len_sq > 0.0 {
                ((wx * dx + wy * dy) / len_sq).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let cx = x0 + t * dx;
            let cy = y0 + t * dy;

            let dist_sq = (px_f - cx) * (px_f - cx) + (py_f - cy) * (py_f - cy);
            let dist = dist_sq.sqrt();

            let sdf = dist - thickness / 2.0;
            let coverage = (0.5 - sdf).clamp(0.0, 1.0);
            if coverage > 0.0 {
                let idx = ((py * canvas_w + px) * 4) as usize;
                blend_px(rgba, idx, color, coverage * max_alpha);
            }
        }
    }
}

/// The shaped pixel width of a single-line text buffer, used to right-align the
/// dropdown shortcuts under a proportional font.
pub(super) fn buffer_width(buffer: &glyphon::Buffer) -> f32 {
    buffer
        .layout_runs()
        .map(|run| run.line_w)
        .fold(0.0, f32::max)
}

/// Shape one line of chrome text into a buffer without touching the GPU. With
/// `proportional`, it uses a sans-serif UI font instead of the terminal family.
pub(super) fn shape_chrome_line(
    font_system: &mut FontSystem,
    ctx: &FontCtx,
    text: &str,
    color: Color,
    bold: bool,
    proportional: bool,
) -> glyphon::Buffer {
    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_size(font_system, Some(f32::MAX), Some(ctx.line_height));
    let family = if proportional {
        Family::SansSerif
    } else {
        base_family(ctx.family)
    };
    let mut attrs = Attrs::new().family(family).color(color);
    if bold {
        let font_has_bold = proportional || ctx.font_has_bold;
        attrs = attrs.weight(effective_bold_weight(ctx.bold_weight, font_has_bold));
    } else {
        attrs = attrs.weight(parse_weight(
            ctx.normal_weight,
            glyphon::cosmic_text::Weight::NORMAL,
        ));
    }
    buffer.set_text(font_system, text, &attrs, Shaping::Advanced, None);
    buffer.shape_until_scroll(font_system, false);
    buffer
}

/// Composite `buffer`'s glyph coverage onto a `(canvas_w, canvas_h)` RGBA buffer
/// at pixel `offset`, clipping to the canvas. Bakes dropdown text into its texture.
pub(super) fn composite_buffer(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    rgba: &mut [u8],
    canvas: (u32, u32),
    buffer: &glyphon::Buffer,
    offset: (i32, i32),
    default_color: Color,
) {
    let (canvas_w, canvas_h) = canvas;
    let (ox, oy) = offset;
    buffer.draw(
        font_system,
        swash_cache,
        default_color,
        |x, y, w, h, color| {
            let alpha = color.a() as f32 / 255.0;
            if alpha <= 0.0 {
                return;
            }
            let (cr, cg, cb) = (color.r() as f32, color.g() as f32, color.b() as f32);
            for py in y..y + h as i32 {
                for px in x..x + w as i32 {
                    let gx = px + ox;
                    let gy = py + oy;
                    if gx < 0 || gy < 0 || gx >= canvas_w as i32 || gy >= canvas_h as i32 {
                        continue;
                    }
                    let idx = ((gy as u32 * canvas_w + gx as u32) * 4) as usize;
                    rgba[idx] = (cr * alpha + rgba[idx] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 1] = (cg * alpha + rgba[idx + 1] as f32 * (1.0 - alpha)) as u8;
                    rgba[idx + 2] = (cb * alpha + rgba[idx + 2] as f32 * (1.0 - alpha)) as u8;
                }
            }
        },
    );
}

/// Rasterize a transient toast pill with `text` in `text_color`, anchored to the
/// top-right of the surface and dropped `top_inset` pixels (the tab-bar height)
/// from the top so it sits just below the tabs. A rounded panel with a soft
/// shadow, positioned as a free-floating overlay.
#[allow(clippy::too_many_arguments)]
pub(super) fn toast_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    text: &str,
    text_color: Rgb,
    surface_w: f32,
    top_inset: f32,
) -> DropdownImage {
    const PAD_X: f32 = 14.0;
    const PAD_Y: f32 = 7.0;
    const RADIUS: f32 = 8.0;
    const SHADOW_MARGIN: f32 = 8.0;
    /// Gap from the right edge and from the bottom of the tab bar, in pixels.
    const EDGE_GAP: f32 = 16.0;

    let label_buf = shape_chrome_line(font_system, ctx, text, text_color.to_glyphon(), true, true);
    let text_w = buffer_width(&label_buf).ceil();
    let text_h = ctx.line_height;

    let panel_w = text_w + PAD_X * 2.0;
    let panel_h = text_h + PAD_Y * 2.0;
    let total_w = (panel_w + SHADOW_MARGIN * 2.0) as u32;
    let total_h = (panel_h + SHADOW_MARGIN * 2.0) as u32;
    let canvas = (total_w, total_h);

    // Top-right: right edge of the pill sits EDGE_GAP from the surface edge, its
    // top sits EDGE_GAP below the tab bar. SHADOW_MARGIN offsets keep the visible
    // panel (not its transparent shadow padding) at those gaps.
    let tip_x = (surface_w - EDGE_GAP - total_w as f32 + SHADOW_MARGIN).max(0.0);
    let tip_y = (top_inset + EDGE_GAP - SHADOW_MARGIN).max(0.0);

    let mut rgba = vec![0u8; (total_w * total_h * 4) as usize];

    // Soft drop-shadow.
    for py in 0..total_h {
        for px in 0..total_w {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
                RADIUS,
            );
            let falloff = (1.0 - sdf / SHADOW_MARGIN).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * total_w + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Panel background with a hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
        RADIUS,
        border,
        1.0,
    );
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (
            SHADOW_MARGIN + 1.0,
            SHADOW_MARGIN + 1.0,
            panel_w - 2.0,
            panel_h - 2.0,
        ),
        RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &label_buf,
        (
            (SHADOW_MARGIN + PAD_X) as i32,
            (SHADOW_MARGIN + PAD_Y) as i32,
        ),
        text_color.to_glyphon(),
    );

    DropdownImage {
        height: total_h,
        rgba,
        width: total_w,
        x: tip_x,
        y: tip_y,
    }
}

/// Rasterize a URL tooltip bubble near the cursor. `anchor` is the cursor cell
/// rect; the tooltip floats below it, clamped within the surface bounds.
#[allow(clippy::too_many_arguments)]
pub(super) fn url_tooltip_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    url: &str,
    anchor: &Region,
    surface_w: f32,
    surface_h: f32,
    tab_layout: Option<&TabbarLayout>,
    hovered_tab: Option<usize>,
) -> DropdownImage {
    const PAD_X: f32 = 8.0;
    const PAD_Y: f32 = 4.0;
    const RADIUS: f32 = 4.0;
    const SHADOW_MARGIN: f32 = 6.0;
    const MAX_URL_CHARS: usize = 80;

    let display_url = if url.len() > MAX_URL_CHARS {
        format!("{}...", &url[..MAX_URL_CHARS])
    } else {
        url.to_string()
    };

    let text_color = theme.foreground.to_glyphon();
    let label_buf = shape_chrome_line(font_system, ctx, &display_url, text_color, false, true);
    let text_w = buffer_width(&label_buf).ceil();
    let text_h = ctx.line_height;

    let panel_w = text_w + PAD_X * 2.0;
    let panel_h = text_h + PAD_Y * 2.0;
    let total_w = (panel_w + SHADOW_MARGIN * 2.0) as u32;
    let total_h = (panel_h + SHADOW_MARGIN * 2.0) as u32;
    let canvas = (total_w, total_h);

    // Place tooltip below the cursor, then flip above if it would go off-screen.
    let (tip_x, tip_y) = if let (Some(layout), Some(idx)) = (tab_layout, hovered_tab) {
        let tab_region = layout.tabs[idx];
        let tab_center_x = tab_region.x + tab_region.w * 0.5;
        let x = (tab_center_x - total_w as f32 * 0.5)
            .max(0.0)
            .min((surface_w - total_w as f32).max(0.0))
            .round();

        let tab_bottom_y = layout.new_tab.y + layout.new_tab.h;
        let y = (tab_bottom_y + 2.0 - SHADOW_MARGIN)
            .max(0.0)
            .min((surface_h - total_h as f32).max(0.0))
            .round();
        (x, y)
    } else {
        let x = (anchor.x - panel_w * 0.5 - SHADOW_MARGIN)
            .max(0.0)
            .min((surface_w - total_w as f32).max(0.0))
            .round();
        let below_y = anchor.y + anchor.h + 2.0 - SHADOW_MARGIN;
        let y = if below_y + total_h as f32 > surface_h {
            (anchor.y - total_h as f32 - 2.0 + SHADOW_MARGIN)
                .max(0.0)
                .round()
        } else {
            below_y.round()
        };
        (x, y)
    };

    let mut rgba = vec![0u8; (total_w * total_h * 4) as usize];

    for py in 0..total_h {
        for px in 0..total_w {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
                RADIUS,
            );
            let falloff = (1.0 - sdf / SHADOW_MARGIN).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * total_w + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (SHADOW_MARGIN, SHADOW_MARGIN, panel_w, panel_h),
        RADIUS,
        border,
        1.0,
    );
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (
            SHADOW_MARGIN + 1.0,
            SHADOW_MARGIN + 1.0,
            panel_w - 2.0,
            panel_h - 2.0,
        ),
        RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &label_buf,
        (
            (SHADOW_MARGIN + PAD_X) as i32,
            (SHADOW_MARGIN + PAD_Y) as i32,
        ),
        text_color,
    );

    DropdownImage {
        height: total_h,
        rgba,
        width: total_w,
        x: tip_x,
        y: tip_y,
    }
}

/// Rasterize the open dropdown overlay (soft shadow, elevated rounded panel,
/// rounded hover pill, and sans-serif item text) to an RGBA image without the
/// GPU. Returns `None` when no menu is open.
/// The parent dropdown panel for the open menu, or `None` when none is open.
pub(super) fn dropdown_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    tabbar: &TopTabbar,
    surface_w: f32,
) -> Option<DropdownImage> {
    let layout = tabbar_layout(tabbar, surface_w, ctx.cell_w, ctx.cell_h);
    let menu = tabbar.menus.get(tabbar.open_menu?)?;
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &menu.items,
        &layout.dropdown?,
        tabbar.selected_item,
    );
    Some(panel)
}

/// The open submenu's child panel, or `None` when no submenu is open.
pub(super) fn submenu_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    tabbar: &TopTabbar,
    surface_w: f32,
) -> Option<DropdownImage> {
    let layout = tabbar_layout(tabbar, surface_w, ctx.cell_w, ctx.cell_h);
    let parent = tabbar
        .menus
        .get(tabbar.open_menu?)?
        .items
        .get(tabbar.open_submenu?)?;
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &parent.children,
        &layout.submenu?,
        tabbar.selected_subitem,
    );
    Some(panel)
}

/// Rasterize the right-click context menu panel, or `None` when none is open.
pub(super) fn context_menu_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    tabbar: &TopTabbar,
    surface_w: f32,
) -> Option<DropdownImage> {
    let cm = tabbar.context_menu.as_ref()?;
    let layout = tabbar_layout(tabbar, surface_w, ctx.cell_w, ctx.cell_h);
    let panel = panel_rgba(
        font_system,
        swash_cache,
        ctx,
        theme,
        &cm.items,
        &layout.context_menu?,
        cm.selected,
    );
    Some(panel)
}

/// Rasterize one menu panel (`items` at `layout`) into an elevated, rounded,
/// shadowed card. `selected` is the hover-highlighted row. A submenu parent
/// (an item with children) gets a `›` chevron instead of a shortcut.
pub(super) fn panel_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    items: &[MenuItem],
    layout: &DropdownLayout,
    selected: Option<usize>,
) -> DropdownImage {
    // The panel sits inside a margin that holds its soft drop shadow, so the
    // texture is larger than the panel and is placed offset by the margin. The
    // panel interior still lands exactly at origin_x/top, keeping the app's
    // hit-testing geometry valid.
    let margin = DROPDOWN_SHADOW;
    let panel_w = layout.width.floor().max(1.0);
    let panel_h = (layout.items as f32 * layout.item_h + 2.0 * layout.pad)
        .floor()
        .max(1.0);
    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow: opacity fades with distance outside the panel.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel: a subtle hairline border, then the surface inset
    // one pixel within it, then the rounded hover pill for the selected row. The
    // first row begins below the panel's top padding.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );
    let row_top = margin + layout.pad;
    if let Some(sel) = selected {
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            row_top + sel as f32 * layout.item_h + MENU_HOVER_INSET,
            panel_w - 2.0 * MENU_HOVER_INSET,
            layout.item_h - 2.0 * MENU_HOVER_INSET,
        );
        let radius = (DROPDOWN_RADIUS - MENU_HOVER_INSET).max(0.0);
        fill_rounded_rect(
            &mut rgba,
            canvas,
            hover_rect,
            radius,
            theme.menu_hover_bg,
            1.0,
        );
    }

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let pad = (ctx.cell_w * 0.9) as i32;
    let origin = margin as i32;
    // Center each label vertically within its taller row.
    let text_dy = ((layout.item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    for (i, item) in items.iter().enumerate() {
        let row_y = (row_top + i as f32 * layout.item_h) as i32;
        if item.label == "-" {
            let line_y = row_y + (layout.item_h / 2.0) as i32;
            for x in (origin + pad)..(origin + panel_w as i32 - pad) {
                if (0..width as i32).contains(&x) && (0..height as i32).contains(&line_y) {
                    let idx = ((line_y * width as i32 + x) * 4) as usize;
                    blend_px(&mut rgba, idx, theme.divider, 1.0);
                }
            }
            continue;
        }
        let text_y = row_y + text_dy;
        let label = shape_chrome_line(font_system, ctx, &item.label, foreground, false, true);
        let pos = (origin + pad, text_y);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &label,
            pos,
            foreground,
        );

        // A submenu parent shows a chevron; a leaf shows its shortcut, if any.
        let trailing = if item.has_children() {
            Some("\u{203a}".to_string())
        } else if !item.shortcut.is_empty() {
            Some(item.shortcut.clone())
        } else {
            None
        };
        if let Some(text) = trailing {
            let buf = shape_chrome_line(font_system, ctx, &text, muted, false, true);
            let buf_w = buffer_width(&buf).ceil() as i32;
            let pos = (origin + panel_w as i32 - buf_w - pad, text_y);
            composite_buffer(
                font_system,
                swash_cache,
                &mut rgba,
                canvas,
                &buf,
                pos,
                muted,
            );
        }
    }

    // Snap the overlay to whole pixels. The texture is 1:1 texel-to-pixel, so a
    // fractional placement (origin_x/top derive from fractional cell sizes) would
    // make the shared linear sampler smear every baked glyph, blurring the menu.
    DropdownImage {
        height,
        rgba,
        width,
        x: (layout.origin_x - margin).round(),
        y: (layout.top - margin).round(),
    }
}

/// Render `text` into `rgba` at `offset`, coloring characters listed in
/// `match_positions` with `accent_color` and the rest with `base_color`.
/// When `underline` is true, a 1-pixel line is drawn below each matched span.
#[allow(clippy::too_many_arguments)]
pub(super) fn draw_highlighted_label(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    rgba: &mut [u8],
    canvas: (u32, u32),
    text: &str,
    match_positions: &[usize],
    base_color: Color,
    accent_color: Color,
    underline: bool,
    bold: bool,
    offset: (i32, i32),
) {
    let (canvas_w, canvas_h) = canvas;
    let weight = if bold {
        parse_weight(ctx.bold_weight, DEFAULT_BOLD_WEIGHT)
    } else {
        parse_weight(ctx.normal_weight, glyphon::cosmic_text::Weight::NORMAL)
    };

    // Shape the whole label in one buffer so every glyph shares the same font
    // and size; color the matched characters with attribute spans. Shaping the
    // label in per-match fragments lets cosmic-text resolve different fallback
    // fonts per segment, which makes the matched glyphs render unevenly.
    let base_attrs = Attrs::new()
        .family(Family::SansSerif)
        .color(base_color)
        .weight(weight);
    let mut attrs_list = glyphon::AttrsList::new(&base_attrs);
    let mut matched_bytes: std::collections::HashSet<usize> = std::collections::HashSet::new();
    let mut byte = 0usize;
    for (i, ch) in text.chars().enumerate() {
        let len = ch.len_utf8();
        if match_positions.binary_search(&i).is_ok() {
            let span = Attrs::new()
                .family(Family::SansSerif)
                .color(accent_color)
                .weight(weight);
            attrs_list.add_span(byte..byte + len, &span);
            matched_bytes.insert(byte);
        }
        byte += len;
    }

    let mut buffer = glyphon::Buffer::new(
        font_system,
        glyphon::Metrics::new(ctx.font_size, ctx.line_height),
    );
    buffer.set_size(font_system, Some(f32::MAX), Some(ctx.line_height));
    buffer.lines.clear();
    buffer.lines.push(BufferLine::new(
        text,
        glyphon::cosmic_text::LineEnding::default(),
        attrs_list,
        Shaping::Advanced,
    ));
    buffer.shape_until_scroll(font_system, false);
    composite_buffer(
        font_system,
        swash_cache,
        rgba,
        canvas,
        &buffer,
        offset,
        base_color,
    );

    // Underline the matched glyphs, located from the shaped layout.
    let underline_y = offset.1 + ctx.line_height as i32 - 2;
    if underline && underline_y >= 0 && underline_y < canvas_h as i32 {
        let uy = underline_y as u32;
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                if !matched_bytes.contains(&glyph.start) {
                    continue;
                }
                let gx0 = offset.0 + glyph.x as i32;
                let gx1 = gx0 + glyph.w.ceil() as i32;
                for px in gx0.max(0)..gx1.min(canvas_w as i32) {
                    let idx = (uy * canvas_w + px as u32) as usize * 4;
                    rgba[idx] = accent_color.r();
                    rgba[idx + 1] = accent_color.g();
                    rgba[idx + 2] = accent_color.b();
                    rgba[idx + 3] = 255;
                }
            }
        }
    }
}

/// Rasterize the command palette: a centered floating panel with a search input
/// at the top and a fuzzy-filtered list of commands below. Styled like the
/// dropdown but wider and vertically centered in the upper portion of the window.
pub(super) fn palette_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &PaletteView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let inner_pad = ctx.cell_h * 0.5;
    let input_h = ctx.cell_h * 2.2;
    let item_h = (ctx.line_height + PALETTE_ITEM_PAD_Y * 2.0).round();
    let display_count = view.items.len().min(PALETTE_MAX_ITEMS);
    let row_count = display_count.max(1);
    let panel_w = (surface_w * PALETTE_WIDTH_RATIO)
        .clamp(300.0, 680.0)
        .floor();
    let panel_h = (inner_pad + input_h + 1.0 + item_h * row_count as f32 + inner_pad)
        .floor()
        .max(1.0);

    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel with hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Hover highlight on the selected result row.
    let divider_y = margin + inner_pad + input_h;
    let results_top = divider_y + 1.0;
    if display_count > 0 {
        let sel = view.selected.min(display_count - 1);
        // The highlight fills the row vertically (only a 1px breath) so it covers
        // the text and selected rows butt against their neighbours with no gap.
        let hover_v_inset = 1.0;
        let hover_rect = (
            margin + MENU_HOVER_INSET,
            results_top + sel as f32 * item_h + hover_v_inset,
            panel_w - 2.0 * MENU_HOVER_INSET,
            item_h - 2.0 * hover_v_inset,
        );
        let radius = (DROPDOWN_RADIUS - MENU_HOVER_INSET).max(0.0);
        fill_rounded_rect(
            &mut rgba,
            canvas,
            hover_rect,
            radius,
            theme.menu_hover_bg,
            1.0,
        );
    }

    // Hairline divider between the input and the results.
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin, divider_y, panel_w, 1.0),
        0.0,
        theme.divider,
        0.5,
    );

    let foreground = theme.foreground.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    // Fuzzy-matched characters are tinted blue to stand out from the label.
    let match_color = theme.ansi[4].to_glyphon();
    let pad_x = (ctx.cell_w * 1.0) as i32;
    let origin = margin as i32;
    let input_text_dy = ((input_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let item_text_dy = ((item_h - ctx.line_height) / 2.0).max(0.0) as i32;
    let input_top = (margin + inner_pad) as i32;

    // "❯" prompt.
    let prompt_buf = shape_chrome_line(font_system, ctx, "\u{276f} ", accent, false, true);
    let prompt_w = buffer_width(&prompt_buf).ceil() as i32;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &prompt_buf,
        (origin + pad_x, input_top + input_text_dy),
        accent,
    );

    // Query text.
    let query_text = view.query.clone();
    let query_color = foreground;
    let query_buf = shape_chrome_line(font_system, ctx, &query_text, query_color, true, true);
    let query_w = buffer_width(&query_buf).ceil() as i32;
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &query_buf,
        (origin + pad_x + prompt_w, input_top + input_text_dy),
        query_color,
    );

    // Caret: a thin vertical bar right after the query text. A glyph caret would
    // sit centered in a full cell box, leaving an unnatural gap from the last
    // character, so draw the bar directly instead.
    let caret_x = (origin + pad_x + prompt_w + query_w) as f32 + 1.0;
    let caret_y = (input_top + input_text_dy) as f32 + 1.0;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (caret_x, caret_y, 2.0, (ctx.line_height - 2.0).max(1.0)),
        1.0,
        theme.cursor_bg,
        1.0,
    );

    // Result rows.
    if display_count == 0 {
        let no_match = shape_chrome_line(font_system, ctx, &view.empty_message, muted, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &no_match,
            (origin + pad_x, results_top as i32 + item_text_dy),
            muted,
        );
    } else {
        let item_pad = PALETTE_ITEM_PAD_X as i32;
        for (i, item) in view.items.iter().take(display_count).enumerate() {
            let row_y = (results_top + i as f32 * item_h) as i32 + item_text_dy;

            if item.match_positions.is_empty() || view.query.is_empty() {
                let label_buf =
                    shape_chrome_line(font_system, ctx, &item.label, foreground, true, true);
                composite_buffer(
                    font_system,
                    swash_cache,
                    &mut rgba,
                    canvas,
                    &label_buf,
                    (origin + pad_x + item_pad, row_y),
                    foreground,
                );
            } else {
                draw_highlighted_label(
                    font_system,
                    swash_cache,
                    ctx,
                    &mut rgba,
                    canvas,
                    &item.label,
                    &item.match_positions,
                    foreground,
                    match_color,
                    view.match_underline,
                    true,
                    (origin + pad_x + item_pad, row_y),
                );
            }

            let hint = if !item.shortcut.is_empty() {
                item.shortcut.as_str()
            } else if !item.action.is_empty() {
                item.action.as_str()
            } else {
                ""
            };
            if !hint.is_empty() {
                let hint_buf = shape_chrome_line(font_system, ctx, hint, muted, false, true);
                let hint_w = buffer_width(&hint_buf).ceil() as i32;
                composite_buffer(
                    font_system,
                    swash_cache,
                    &mut rgba,
                    canvas,
                    &hint_buf,
                    (origin + panel_w as i32 - hint_w - pad_x - item_pad, row_y),
                    muted,
                );
            }
        }
    }

    // Center horizontally; place in the upper third of the window.
    let palette_x = ((surface_w - panel_w) / 2.0).max(0.0);
    let palette_y = surface_h * PALETTE_TOP_RATIO;

    DropdownImage {
        height,
        rgba,
        width,
        x: (palette_x - margin).round(),
        y: (palette_y - margin).round(),
    }
}

/// Rasterize the which-key hint popup: an elevated card showing continuation keys
/// and labels when the user pauses mid-prefix.
pub(super) fn which_key_rgba(
    font_system: &mut FontSystem,
    swash_cache: &mut SwashCache,
    ctx: &FontCtx,
    theme: &Theme,
    view: &WhichKeyView,
    surface_w: f32,
    surface_h: f32,
) -> DropdownImage {
    let margin = DROPDOWN_SHADOW;
    let pad_x = (ctx.cell_w * 1.5).max(14.0);
    let pad_y = (ctx.cell_h * 0.8).max(10.0);
    let title_h = ctx.line_height + 4.0;
    let item_h = (ctx.line_height + 4.0).round();

    let cols = if view.items.len() > 6 { 2 } else { 1 };
    let rows = view.items.len().div_ceil(cols);

    let col_w = (ctx.cell_w * 26.0).max(220.0);
    let panel_w = (pad_x * 2.0 + col_w * cols as f32)
        .min(surface_w - 40.0)
        .floor()
        .max(220.0);
    let panel_h = (pad_y * 2.0 + title_h + rows as f32 * item_h + 8.0)
        .min(surface_h - 40.0)
        .floor()
        .max(60.0);

    let width = (panel_w + 2.0 * margin) as u32;
    let height = (panel_h + 2.0 * margin) as u32;
    let canvas = (width, height);
    let panel_rect = (margin, margin, panel_w, panel_h);

    let mut rgba = vec![0u8; (width * height * 4) as usize];

    // Soft drop shadow.
    for py in 0..height {
        for px in 0..width {
            let sdf = rounded_rect_sdf(
                px as f32 + 0.5,
                py as f32 + 0.5,
                panel_rect,
                DROPDOWN_RADIUS,
            );
            let falloff = (1.0 - sdf / margin).clamp(0.0, 1.0);
            if sdf <= 0.0 || falloff <= 0.0 {
                continue;
            }
            let idx = ((py * width + px) * 4) as usize;
            blend_px(
                &mut rgba,
                idx,
                SHADOW_COLOR,
                falloff * falloff * DROPDOWN_SHADOW_ALPHA,
            );
        }
    }

    // Elevated rounded panel with hairline border.
    let border = mix_rgb(theme.menu_bg, Rgb::new(255, 255, 255), MENU_BORDER_MIX);
    fill_rounded_rect(&mut rgba, canvas, panel_rect, DROPDOWN_RADIUS, border, 1.0);
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin + 1.0, margin + 1.0, panel_w - 2.0, panel_h - 2.0),
        DROPDOWN_RADIUS - 1.0,
        theme.menu_bg,
        1.0,
    );

    // Hairline divider under title.
    let divider_y = margin + pad_y + title_h;
    fill_rounded_rect(
        &mut rgba,
        canvas,
        (margin, divider_y, panel_w, 1.0),
        0.0,
        theme.divider,
        0.5,
    );

    let foreground = theme.foreground.to_glyphon();
    let accent = theme.cursor_bg.to_glyphon();
    let muted = theme.ansi[8].to_glyphon();

    // Draw Title.
    let title_buf = shape_chrome_line(font_system, ctx, &view.title, accent, true, true);
    composite_buffer(
        font_system,
        swash_cache,
        &mut rgba,
        canvas,
        &title_buf,
        ((margin + pad_x) as i32, (margin + pad_y) as i32),
        accent,
    );

    // Draw Items.
    let items_top = divider_y + 6.0;
    for (i, (key, label)) in view.items.iter().enumerate() {
        let col = i / rows;
        let row = i % rows;
        let item_x = margin + pad_x + col as f32 * col_w;
        let item_y = items_top + row as f32 * item_h;

        let key_buf = shape_chrome_line(font_system, ctx, key, accent, true, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &key_buf,
            (item_x as i32, item_y as i32),
            accent,
        );

        let arrow_buf = shape_chrome_line(font_system, ctx, "→", muted, false, true);
        let key_w = ctx.cell_w * 7.0;
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &arrow_buf,
            ((item_x + key_w) as i32, item_y as i32),
            muted,
        );

        let label_buf = shape_chrome_line(font_system, ctx, label, foreground, false, true);
        composite_buffer(
            font_system,
            swash_cache,
            &mut rgba,
            canvas,
            &label_buf,
            ((item_x + key_w + ctx.cell_w * 2.0) as i32, item_y as i32),
            foreground,
        );
    }

    let tip_x = ((surface_w - panel_w) * 0.5).max(0.0);
    let tip_y = ((surface_h - panel_h) * 0.5).max(0.0);

    DropdownImage {
        height,
        rgba,
        width,
        x: (tip_x - margin).round(),
        y: (tip_y - margin).round(),
    }
}

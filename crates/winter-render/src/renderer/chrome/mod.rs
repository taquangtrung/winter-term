//! Chrome rasterization: the tabbar strip, dropdowns and menus, the
//! command palette, which-key hints, toasts, and URL tooltips, painted
//! into RGBA buffers for the image pass.

mod overlay;
mod paint;
mod view;

use view::StripImage;
pub(crate) use view::TabbarText;
pub use view::{InputView, PaletteItem, PaletteView, WhichKeyView};

use super::colors::mix_rgb;
use super::glyphs::FontCtx;
use super::GpuRenderer;
use super::{NoticeKind, StatusNotice};
use crate::image::ImagePlacement;
use crate::tabbar::{
    self, layout as tabbar_layout, PaneStripEdges, PaneStripHit, PaneStripLayout, PaneTab, Region, TabbarHit,
    TabbarLayout, TopTabbar, TAB_H_PAD_CELLS,
    ZOOM_CELLS,
};
use crate::theme::{Rgb, Theme};
use glyphon::{Color, FontSystem, SwashCache};
use overlay::{
    context_menu_rgba, dropdown_rgba, input_rgba, palette_rgba, submenu_rgba, toast_rgba,
    url_tooltip_rgba, which_key_rgba,
};
use paint::{
    blit_svg, buffer_width, composite_buffer, fill_line_segment, fill_rounded_rect, shape_chrome_line, text_bounds,
    truncate_label,
};

// ========================================================================
// Constants
// ========================================================================

/// Reserved image-pass texture ids for the rasterized menu overlays. Set to the
/// top of the id space so they never collide with block image ids.
const DROPDOWN_TEXTURE_ID: u64 = u64::MAX;

const SUBMENU_TEXTURE_ID: u64 = u64::MAX - 1;

const CONTEXT_MENU_TEXTURE_ID: u64 = u64::MAX - 5;

/// Reserved id for the rasterized top-tabbar strip (band + rounded tab pills).
const TABBAR_STRIP_TEXTURE_ID: u64 = u64::MAX - 2;

/// Reserved id for the rasterized command-palette overlay.
const PALETTE_TEXTURE_ID: u64 = u64::MAX - 3;

/// Reserved id for the rasterized URL hover tooltip.
const URL_TOOLTIP_TEXTURE_ID: u64 = u64::MAX - 6;

/// Reserved id for the rasterized transient toast pill.
const TOAST_TEXTURE_ID: u64 = u64::MAX - 7;

/// Reserved id for the rasterized which-key hint popup.
const WHICH_KEY_TEXTURE_ID: u64 = u64::MAX - 8;

/// Reserved id for the rasterized input dialog.
const INPUT_TEXTURE_ID: u64 = u64::MAX - 9;

/// The input dialog's width as a fraction of the surface.
///
/// Narrower than the palette: it holds one line of an answer rather than a
/// list of choices, and a box the width of the window for a filename reads as
/// a panel rather than as a question.
pub(super) const INPUT_WIDTH_RATIO: f32 = 0.5;

/// The key-hint card's width and height as fractions of the surface.
///
/// A fixed size whatever the card holds: a hint is read at a glance, in the
/// same place every time, and a panel that grew and shrank with the number of
/// keys would move the first row somewhere new on every prefix. The keys flow
/// into as many columns as the fixed height needs.
///
/// Sized for the longest menu rather than the common one: a prefix offering a
/// dozen keys should still read down a single column, which is what the
/// height buys, and the width is what keeps a second column legible when one
/// offers more than that.
pub(super) const WHICH_KEY_WIDTH_RATIO: f32 = 0.66;
pub(super) const WHICH_KEY_HEIGHT_RATIO: f32 = 0.55;

/// The palette's width and height as fractions of the surface.
///
/// A fixed size, as the key-hint card is: the list narrows as the query is
/// typed, and a panel that shrank with it would walk its own rows out from
/// under the eye reading them. How many results are visible follows from the
/// height rather than from a count of its own, so a taller window shows more
/// of them.
pub(super) const PALETTE_WIDTH_RATIO: f32 = 0.7;
pub(super) const PALETTE_HEIGHT_RATIO: f32 = 0.6;
/// The palette's height as a fraction of the surface when docked at the top for Jump mode.
pub(super) const PALETTE_JUMP_HEIGHT_RATIO: f32 = 0.5;

/// Extra horizontal padding inside each palette result row, in pixels, applied
/// to both the left label and the right shortcut hint.
pub(super) const PALETTE_ITEM_PAD_X: f32 = 5.0;

/// Vertical padding above and below the text in each palette result row, in
/// pixels. The row height is one line of text plus this on each side, so the
/// inter-row gap is a small fixed amount rather than a multiple of the line.
pub(super) const PALETTE_ITEM_PAD_Y: f32 = 4.0;

/// Corner radius of the dropdown menu panel and its hover highlight, in pixels.
pub(super) const DROPDOWN_RADIUS: f32 = 12.0;

/// Corner radius of every pill on the tabbar, title bar and pane strips: none,
/// so their tabs and button highlights are square.
const SQUARE_CORNER: f32 = 0.0;

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

/// Flat pixels the close button (× glyph and its hover pill) is shifted
/// right of its horizontally-centered position within its close slot.
const TAB_CLOSE_SHIFT_RIGHT_PX: f32 = 3.0;

/// Flat pixels the per-tab close button (× glyph and its hover pill) is
/// shifted up from its vertically-centered position within its close slot.
const TAB_CLOSE_SHIFT_UP_PX: f32 = 1.0;

/// Flat pixels the tab title label is shifted up from its vertically-
/// centered position on the tab shape.
const TAB_LABEL_SHIFT_UP_PX: f32 = 1.0;

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

/// Color of the dropdown drop shadow (alpha is applied per pixel).
pub(super) const SHADOW_COLOR: Rgb = Rgb::new(0, 0, 0);

// ========================================================================
// Data Structures
// ========================================================================
/// The last rasterized tabbar strip, beside everything it was drawn from. The
/// strip is a full-width RGBA image repainted pixel by pixel and uploaded as a
/// texture, which is worth doing only when one of those inputs has moved.
pub(super) struct TabbarStripCache {
    cell_height: f32,
    cell_width: f32,
    /// Where the cached texture is drawn, handed back untouched on a hit.
    placement: ImagePlacement,
    surface_width: f32,
    tabbar: TopTabbar,
    theme: Theme,
}

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

/// The square hover pill behind a button's icon: one cell-height (to whole
/// pixels) on a side wherever it is drawn, so every button's pill is the same
/// size, centered on `(cx, cy)` to within half a pixel and snapped to whole
/// pixels so its edges stay crisp. Returned as `(x, y, width, height)`.
fn button_pill(cx: f32, cy: f32, cell_h: f32) -> (f32, f32, f32, f32) {
    let side = cell_h.round().max(1.0);
    ((cx - side / 2.0).round(), (cy - side / 2.0).round(), side, side)
}

/// The center of a button's pill, which is where the icon on it is drawn: the
/// pill is snapped to whole pixels, so its center, not the button box's, is
/// the point an icon has to be centered on to look centered on the pill.
fn pill_center((x, y, w, h): (f32, f32, f32, f32)) -> (f32, f32) {
    (x + w / 2.0, y + h / 2.0)
}

// ========================================================================
// Implementation
// ========================================================================

impl TabbarStripCache {
    /// Whether the cached strip was painted from exactly these inputs. The
    /// arguments mirror what the painting itself takes, so a new input has to
    /// be threaded through both and cannot be left out of the comparison.
    fn matches(
        &self,
        tabbar: &TopTabbar,
        surface_width: f32,
        cell_width: f32,
        cell_height: f32,
        theme: &Theme,
    ) -> bool {
        // Exact equality is what is wanted: these are the same values the last
        // paint was handed, not a measurement to be compared within a
        // tolerance, and any difference at all repaints.
        self.cell_height == cell_height
            && self.cell_width == cell_width
            && self.surface_width == surface_width
            && self.theme == *theme
            && self.tabbar == *tabbar
    }
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
            let tail: String = path.chars().skip(path.chars().count() - target.saturating_sub(1)).collect();
            text = format!("\u{2026}{tail}");
        }
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
            v_min: 0.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        }]
    }

    /// Rasterize the input dialog and upload it to the GPU.
    pub(super) fn rasterize_input(
        &mut self,
        input: &InputView,
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
        let image = input_rgba(
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
            &self.theme,
            input,
            surface_w,
            surface_h,
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            INPUT_TEXTURE_ID,
            &image.rgba,
            image.width,
            image.height,
        );
        vec![ImagePlacement {
            alpha: 1.0,
            height: image.height as f32,
            id: INPUT_TEXTURE_ID,
            v_max: 1.0,
            v_min: 0.0,
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
            v_min: 0.0,
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
            v_min: 0.0,
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
            NoticeKind::Progress => self.theme.ansi[3],
        };
        // Tuck the toast below the tab bar so it never overlaps the tabs. Mirrors
        // the top-inset math used elsewhere (rows of tabbar chrome × cell height).
        let top_inset = if self.tabbar_enabled {
            if self.modern {
                crate::modern_titlebar_height_px(self.cell_height)
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
            v_min: 0.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        })
    }

    /// Where to draw the tabbar strip, repainting it only when what it is drawn
    /// from has moved since the last frame. Everything the painting reads is
    /// compared, so a hit cannot serve a strip that should look different.
    pub(super) fn tabbar_strip_placement(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
    ) -> Option<ImagePlacement> {
        if let Some(cached) = &self.tabbar_strip_cache {
            if cached.matches(
                tabbar,
                surface_w,
                self.cell_width,
                self.cell_height,
                &self.theme,
            ) {
                return Some(cached.placement);
            }
        }
        let placement = self.rasterize_tabbar_strip(tabbar, surface_w)?;
        self.tabbar_strip_cache = Some(TabbarStripCache {
            cell_height: self.cell_height,
            cell_width: self.cell_width,
            placement,
            surface_width: surface_w,
            tabbar: tabbar.clone(),
            theme: self.theme.clone(),
        });
        Some(placement)
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
        let strip = tabbar_strip_rgba(
            tabbar,
            surface_w,
            self.cell_width,
            self.cell_height,
            &self.theme,
        );
        self.tabbar_strip_pass.upload(
            &self.device,
            &self.queue,
            TABBAR_STRIP_TEXTURE_ID,
            &strip.rgba,
            strip.width,
            strip.height,
        );
        Some(ImagePlacement {
            alpha: 1.0,
            height: strip.height as f32,
            id: TABBAR_STRIP_TEXTURE_ID,
            v_max: 1.0,
            v_min: 0.0,
            width: strip.width as f32,
            x: 0.0,
            y: 0.0,
        })
    }

    /// Rasterize and place the open menu's overlays: the parent dropdown, and
    /// the open submenu (drawn after, so it overlays the parent). Returns one
    /// placement per visible panel, or empty when no menu is open.
    ///
    /// The pixel work lives in [`dropdown_rgba`] and [`submenu_rgba`] so it can
    /// run (and be tested) without the GPU; this only uploads the result.
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
                v_min: 0.0,
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
    fn tabbar_line_buffer(
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
    fn tabbar_button_buffer(&mut self, glyph: &str, color: Color, ratio: f32) -> glyphon::Buffer {
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
// Tabbar strip painting
// ========================================================================

/// Paint the whole top-tabbar strip into an RGBA buffer: the recessed band,
/// every tab pill and its close icon, the new-tab and hamburger buttons, the
/// scroll chevrons, and the window controls.
///
/// Free of the GPU on purpose, like the other `*_rgba` painters in this module,
/// so the pixel work can be exercised without a device; the caller only uploads
/// the result.
pub(super) fn tabbar_strip_rgba(
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
            fill_rounded_rect(
                rgba,
                style.canvas,
                pill,
                SQUARE_CORNER,
                btn_color,
                1.0,
            );
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
            fill_rounded_rect(
                rgba,
                style.canvas,
                pill,
                SQUARE_CORNER,
                color,
                1.0,
            );
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

/// Active tab pill background color (#2a2f31): the pane's own background, so
/// the active tab reads as open into its pane.
const PANE_STRIP_ACTIVE_BG: Rgb = Rgb::new(0x2a, 0x2f, 0x31);

/// Non-occupied band background color (#1f2527).
const PANE_STRIP_BAND_BG: Rgb = Rgb::new(0x1f, 0x25, 0x27);

/// Side of a tab's icon square, as a fraction of cell height.
const TAB_ICON_HEIGHT_RATIO: f32 = 0.85;

/// Gap between a tab's icon and its title, in cells.
const TAB_ICON_GAP_CELLS: f32 = 0.5;

/// Rasterize a pane-level tab strip into an RGBA buffer.
#[allow(clippy::too_many_arguments)]
pub fn rasterize_pane_strip_rgba(
    tabs: &[PaneTab],
    layout: &PaneStripLayout,
    width: u32,
    height: u32,
    cw: f32,
    ch: f32,
    theme: &Theme,
    hover: PaneStripHit,
    separator: Rgb,
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
    let active_text_color = mix_rgb(theme.tab_active_fg, bright_target, 0.35).to_glyphon();
    let inactive_text_color = mix_rgb(theme.ansi[8], bright_target, 0.62).to_glyphon();

    // 2. Tab pills and their contents (text + close icon).
    for (i, tab) in tabs.iter().enumerate() {
        let tab_reg = layout.tabs[i];
        if tab_reg.w <= 0.0 {
            continue;
        }

        let is_hovered = matches!(hover, PaneStripHit::Tab(id) | PaneStripHit::CloseTab(id) if id == tab.pane_id);
        let pill_color = if tab.active {
            PANE_STRIP_ACTIVE_BG
        } else if is_hovered {
            mix_rgb(inactive_bg, PANE_STRIP_ACTIVE_BG, 0.55)
        } else {
            inactive_bg
        };

        let pill_bounds = (
            tab_reg.x + tabbar::TAB_GAP_PX / 2.0,
            tab_reg.y + tab_vpad_top,
            tab_reg.w - tabbar::TAB_GAP_PX,
            tab_reg.h - tab_vpad_top - tab_vpad_bottom,
        );
        fill_rounded_rect(&mut rgba, canvas, pill_bounds, SQUARE_CORNER, pill_color, 1.0);

        // Close button hover highlight
        let close_reg = layout.closes[i];
        let close_pill = button_pill(
            close_reg.x + close_reg.w / 2.0 + TAB_CLOSE_SHIFT_RIGHT_PX,
            pill_mid_y(close_reg),
            ch,
        );
        let is_close_hovered = hover == PaneStripHit::CloseTab(tab.pane_id);
        if is_close_hovered {
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
            let close_color = if is_close_hovered || is_hovered || tab.active {
                theme.foreground
            } else {
                theme.ansi[8]
            };
            let (cx, cy) = pill_center(close_pill);
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

        // Tab label
        let pad = cw * 0.4;
        let title_x = tab_reg.x + TAB_H_PAD_CELLS * cw;
        let title_end = close_reg.x;
        let mut text_x = title_x + pad;
        let text_color = if tab.active {
            active_text_color
        } else {
            inactive_text_color
        };
        if let Some(svg) = &tab.icon {
            let icon_px = (ch * TAB_ICON_HEIGHT_RATIO).round().max(1.0) as u32;
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
            text_x += icon_px as f32 + cw * TAB_ICON_GAP_CELLS;
        }
        let avail = (title_end - text_x).max(cw);
        let max_chars = (avail / cw).floor() as usize;
        let labeled = format!("{}: {}", i + 1, tab.title);
        let title = truncate_label(&labeled, max_chars);
        let mut buffer = shape_chrome_line(font_system, ctx, &title, text_color, false, true);
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
        separator,
        1.0,
    );

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
        x: f32,
        y: f32,
    ) -> ImagePlacement {
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
        let rgba = rasterize_pane_strip_rgba(
            tabs,
            layout,
            width,
            height,
            self.cell_width,
            self.cell_height,
            &self.theme,
            hover,
            self.split_divider_color(),
            &mut self.font_system,
            &mut self.swash_cache,
            &ctx,
        );
        let strip = add_strip_dividers(
            &rgba,
            width,
            height,
            edges,
            self.divider_width,
            self.split_divider_color(),
        );
        self.image_pass.upload(
            &self.device,
            &self.queue,
            texture_id,
            &strip.rgba,
            strip.width,
            strip.height,
        );
        ImagePlacement {
            alpha: 1.0,
            height: strip.height as f32,
            id: texture_id,
            v_max: 1.0,
            v_min: 0.0,
            width: strip.width as f32,
            x: x - strip.left as f32,
            y: y - strip.top as f32,
        }
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::sample_menu_chrome;
    use crate::tabbar::MenuStyle;

    const CELL_W: f32 = 9.0;
    const CELL_H: f32 = 18.0;

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

    const SURFACE_W: f32 = 1000.0;

    fn strip(tabbar: &TopTabbar) -> StripImage {
        tabbar_strip_rgba(tabbar, SURFACE_W, CELL_W, CELL_H, &Theme::dark())
    }

    /// A cache standing for a strip painted from `tabbar` at `CELL_W`/`CELL_H`.
    fn cache_of(tabbar: &TopTabbar, surface_width: f32, theme: &Theme) -> TabbarStripCache {
        TabbarStripCache {
            cell_height: CELL_H,
            cell_width: CELL_W,
            placement: ImagePlacement {
                alpha: 1.0,
                height: CELL_H,
                id: TABBAR_STRIP_TEXTURE_ID,
                v_max: 1.0,
                v_min: 0.0,
                width: surface_width,
                x: 0.0,
                y: 0.0,
            },
            surface_width,
            tabbar: tabbar.clone(),
            theme: theme.clone(),
        }
    }

    #[test]
    fn test_an_unchanged_tabbar_reuses_the_strip_it_already_painted() {
        let tabbar = sample_menu_chrome(None);
        let theme = Theme::dark();
        let cache = cache_of(&tabbar, 800.0, &theme);
        assert!(cache.matches(&tabbar, 800.0, CELL_W, CELL_H, &theme));
    }

    #[test]
    fn test_the_strip_is_repainted_when_anything_it_is_drawn_from_moves() {
        let tabbar = sample_menu_chrome(None);
        let theme = Theme::dark();
        let cache = cache_of(&tabbar, 800.0, &theme);

        // A renamed tab is the case that would show a stale title forever.
        let mut renamed = tabbar.clone();
        renamed.tabs[0].title = "renamed".into();
        assert!(
            !cache.matches(&renamed, 800.0, CELL_W, CELL_H, &theme),
            "title"
        );

        // The active tab moving repaints, or the highlight would not follow.
        let mut switched = tabbar.clone();
        switched.active_tab += 1;
        assert!(
            !cache.matches(&switched, 800.0, CELL_W, CELL_H, &theme),
            "active"
        );

        assert!(
            !cache.matches(&tabbar, 900.0, CELL_W, CELL_H, &theme),
            "resize"
        );
        assert!(
            !cache.matches(&tabbar, 800.0, CELL_W + 1.0, CELL_H, &theme),
            "font"
        );
        let light = Theme::light();
        assert!(
            !cache.matches(&tabbar, 800.0, CELL_W, CELL_H, &light),
            "theme"
        );
    }

    /// The RGBA pixel at `(x, y)`.
    fn pixel(image: &StripImage, x: u32, y: u32) -> (u8, u8, u8, u8) {
        let i = ((y * image.width + x) * 4) as usize;
        (
            image.rgba[i],
            image.rgba[i + 1],
            image.rgba[i + 2],
            image.rgba[i + 3],
        )
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

    /// The `(x, y, width, height)` box around every pixel where `after`
    /// differs from `before`, over a `width`-wide RGBA image.
    fn changed_box(before: &[u8], after: &[u8], width: u32) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for (i, (b, a)) in before.chunks(4).zip(after.chunks(4)).enumerate() {
            if b != a {
                let (x, y) = (i as u32 % width, i as u32 / width);
                (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
            }
        }
        assert!(x0 != u32::MAX, "nothing changed");
        (x0, y0, x1 - x0 + 1, y1 - y0 + 1)
    }

    #[test]
    fn test_pane_strip_button_hover_pills_are_equal_squares_centered_in_the_strip() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 700;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![
            PaneTab { active: true, icon: None, pane_id: 1, title: "zsh".into() },
            PaneTab { active: false, icon: None, pane_id: 2, title: "a".into() },
        ];
        // Narrow enough to paginate, so the scroll arrows are there to hover.
        let narrow = 300;
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let paged = layout_pane_strip(&tabs, narrow as f32, h as f32, CELL_W, CELL_H, 0);
        assert!(paged.scroll_right.is_some(), "the strip should paginate");
        let mut paint = |layout: &PaneStripLayout, width, hover| {
            rasterize_pane_strip_rgba(
                &tabs, layout, width, h, CELL_W, CELL_H, &theme, hover, Rgb::new(60, 60, 60),
                &mut fonts, &mut swash, &ctx,
            )
        };
        let idle = paint(&layout, w, PaneStripHit::None);
        let idle_paged = paint(&paged, narrow, PaneStripHit::None);
        let boxes = [
            changed_box(&idle, &paint(&layout, w, PaneStripHit::CloseTab(1)), w),
            changed_box(&idle, &paint(&layout, w, PaneStripHit::NewTab), w),
            changed_box(&idle_paged, &paint(&paged, narrow, PaneStripHit::ScrollRight), narrow),
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
        assert_eq!(boxes[0].2, boxes[1].2, "close and new-tab pills differ in size");
        assert_eq!(boxes[0].2, boxes[2].2, "close and scroll pills differ in size");
        assert_eq!(boxes[0].2 as f32, CELL_H, "pills are not one cell tall");
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
        assert!(sides.windows(2).all(|p| p[0] == p[1]), "pill sizes differ: {sides:?}");
        // The same size as the pane strips' pills, not just alike in the bar.
        assert_eq!(sides[0] as f32, CELL_H, "pills are not one cell tall");
    }

    #[test]
    fn test_pane_strip_tab_pills_have_square_corners() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![PaneTab {
            active: true,
            icon: None,
            pane_id: 1,
            title: "zsh".into(),
        }];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let rgba = rasterize_pane_strip_rgba(
            &tabs, &layout, w, h, CELL_W, CELL_H, &theme, PaneStripHit::None,
            Rgb::new(60, 60, 60), &mut fonts, &mut swash, &ctx,
        );
        let at = |x: u32, y: u32| {
            let i = ((y * w + x) * 4) as usize;
            rgba[i..i + 3].to_vec()
        };
        // The pill's top-left pixel is as filled as its interior; a rounded
        // corner would leave it partly band-colored.
        assert_eq!(at(1, 1), at(150, 12));
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
                let diff: i32 = at(x, y).iter().zip(&bg).map(|(a, b)| (*a as i32 - *b as i32).abs()).sum();
                if diff > 40 {
                    (min_x, min_y) = (min_x.min(x), min_y.min(y));
                    (max_x, max_y) = (max_x.max(x), max_y.max(y));
                }
            }
        }
        assert!(min_x != u32::MAX, "no icon inside the pill at {pill:?}");
        ((min_x + max_x + 1) as f32 / 2.0, (min_y + max_y + 1) as f32 / 2.0)
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
        for (what, r) in [("hamburger", hamburger), ("minimize", minimize), ("maximize", maximize), ("close", close)] {
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
            PaneTab { active: true, icon: None, pane_id: 1, title: "zsh".into() },
            PaneTab { active: false, icon: None, pane_id: 2, title: "a".into() },
        ];
        let mid = (h - 2) as f32 / 2.0;
        let wide = layout_pane_strip(&tabs, 700.0, h as f32, CELL_W, CELL_H, 0);
        let narrow = layout_pane_strip(&tabs, 300.0, h as f32, CELL_W, CELL_H, 0);
        let mut paint = |layout: &PaneStripLayout, width: u32| {
            rasterize_pane_strip_rgba(
                &tabs, layout, width, h, CELL_W, CELL_H, &theme, PaneStripHit::None,
                Rgb::new(60, 60, 60), &mut fonts, &mut swash, &ctx,
            )
        };
        let wide_rgba = paint(&wide, 700);
        let close = wide.closes[0];
        let close_pill = button_pill(close.x + close.w / 2.0 + TAB_CLOSE_SHIFT_RIGHT_PX, mid, CELL_H);
        check("pane close", &wide_rgba, 700, close_pill);
        let nt = wide.new_tab.expect("the button fits");
        check("new tab", &wide_rgba, 700, button_pill(nt.x + nt.w / 2.0, mid, CELL_H));
        let narrow_rgba = paint(&narrow, 300);
        for (what, r) in [("scroll left", narrow.scroll_left), ("scroll right", narrow.scroll_right)] {
            let r = r.expect("the strip paginates");
            check(what, &narrow_rgba, 300, button_pill(r.x + r.w / 2.0, mid, CELL_H));
        }
    }

    #[test]
    fn test_new_tab_plus_is_a_crisp_symmetric_cross_centered_on_its_pill() {
        use crate::renderer::test_support::sample_font_ctx;
        use crate::tabbar::{layout_pane_strip, PaneStripHit, PaneTab};
        let theme = Theme::dark();
        let ctx = sample_font_ctx();
        let mut fonts = FontSystem::new();
        let mut swash = SwashCache::new();
        let w = 400;
        let h = crate::pane_strip_height_px(CELL_H).ceil() as u32;
        let tabs = vec![PaneTab { active: true, icon: None, pane_id: 1, title: "zsh".into() }];
        let layout = layout_pane_strip(&tabs, w as f32, h as f32, CELL_W, CELL_H, 0);
        let nt = layout.new_tab.expect("the button fits");
        let rgba = rasterize_pane_strip_rgba(
            &tabs, &layout, w, h, CELL_W, CELL_H, &theme, PaneStripHit::None,
            Rgb::new(60, 60, 60), &mut fonts, &mut swash, &ctx,
        );
        let px = |x: u32, y: u32| rgba[((y * w + x) * 4) as usize..][..3].to_vec();
        let band = px(nt.x as u32 + 1, 1);
        let ink: Vec<(u32, u32)> = (0..h - 2)
            .flat_map(|y| (nt.x as u32..(nt.x + nt.w) as u32).map(move |x| (x, y)))
            .filter(|&(x, y)| px(x, y) != band)
            .collect();
        let (x0, x1) = (ink.iter().map(|p| p.0).min().unwrap(), ink.iter().map(|p| p.0).max().unwrap());
        let (y0, y1) = (ink.iter().map(|p| p.1).min().unwrap(), ink.iter().map(|p| p.1).max().unwrap());
        assert_eq!(x1 - x0, y1 - y0, "the '+' is not as wide as it is tall");
        let full = px(x0 + (x1 - x0) / 2, y0 + (y1 - y0) / 2);
        // Crisp: every pixel it touches is the same full-strength color, with
        // no half-lit edge.
        assert!(ink.iter().all(|&(x, y)| px(x, y) == full), "the '+' has soft edges");
        let pill = button_pill(nt.x + nt.w / 2.0, (h - 2) as f32 / 2.0, CELL_H);
        let (cx, cy) = ((x0 + x1 + 1) as f32 / 2.0, (y0 + y1 + 1) as f32 / 2.0);
        assert_eq!((cx, cy), (pill.0 + pill.2 / 2.0, pill.1 + pill.3 / 2.0), "off its pill");
    }
}

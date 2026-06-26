//! The chrome's way onto the GPU: lending a painter the renderer's font
//! machinery, uploading what it painted, and keeping each overlay's raster
//! for as long as nothing it was painted from has moved.
//!
//! Every overlay is a full RGBA image painted pixel by pixel on the CPU, so
//! repainting one on every frame it is open (each cursor blink, each tick of
//! a busy pane) was most of a frame's cost. A cache hit hands back the
//! placement of the texture already on the GPU.

use std::collections::HashMap;

use glyphon::{FontSystem, SwashCache};

use super::view::DropdownImage;
use super::DROPDOWN_SHADOW;
use crate::image::ImagePlacement;
use crate::renderer::glyphs::FontCtx;
use crate::renderer::parts::titlebar::tabbar_strip_rgba;
use crate::renderer::view::ToastRect;
use crate::renderer::widgets::find::find_rgba;
use crate::renderer::widgets::input::input_rgba;
use crate::renderer::widgets::menu::{context_menu_rgba, dropdown_rgba, submenu_rgba};
use crate::renderer::widgets::palette::palette_rgba;
use crate::renderer::widgets::toast;
use crate::renderer::widgets::tooltip::url_tooltip_rgba;
use crate::renderer::widgets::which_key::which_key_rgba;
use crate::renderer::widgets::{FindView, InputView, PaletteView, WhichKeyView};
use crate::renderer::{GpuRenderer, ToastCard, ToastHit};
use crate::tabbar::{
    layout as tabbar_layout, PaneStripDropTarget, PaneStripEdges, PaneStripHit, PaneStripLayout,
    PaneTab, Region, TabbarHit, TopTabbar,
};
use crate::theme::Theme;

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

/// Reserved ids for the toast cards, one per card shown at once: no more
/// cards are drawn than there are ids.
const TOAST_TEXTURE_IDS: [u64; 3] = [u64::MAX - 7, u64::MAX - 10, u64::MAX - 11];

/// Reserved id for the rasterized which-key hint popup.
const WHICH_KEY_TEXTURE_ID: u64 = u64::MAX - 8;

/// Reserved id for the rasterized find popup.
const FIND_TEXTURE_ID: u64 = u64::MAX - 4;

/// Reserved id for the rasterized input dialog.
const INPUT_TEXTURE_ID: u64 = u64::MAX - 9;

/// Rows of cell height the Classic-style tabbar takes, which the toast is
/// tucked below.
const CLASSIC_TABBAR_ROWS: f32 = 2.0;

// ========================================================================
// Data Structures
// ========================================================================

/// What a part or widget painter borrows from the renderer: the font machinery
/// mutably, the metrics and the theme read-only.
pub(in crate::renderer) struct ChromePainter<'a> {
    pub(in crate::renderer) ctx: FontCtx<'a>,
    pub(in crate::renderer) font_system: &'a mut FontSystem,
    pub(in crate::renderer) swash_cache: &'a mut SwashCache,
    pub(in crate::renderer) theme: &'a Theme,
}

/// The last raster of every overlay, beside what it was painted from.
#[derive(Default)]
pub(in crate::renderer) struct OverlayCaches {
    dropdown: RasterCache<TopTabbar>,
    find: RasterCache<FindView>,
    input: RasterCache<InputView>,
    palette: RasterCache<PaletteView>,
    /// One per pane strip texture, keyed by its texture id.
    pub(in crate::renderer) pane_strips: HashMap<u64, RasterCache<PaneStripKey>>,
    tabbar_strip: RasterCache<TopTabbar>,
    toast: RasterCache<ToastKey>,
    url_tooltip: RasterCache<TopTabbar>,
    which_key: RasterCache<WhichKeyView>,
}

/// Everything about the renderer an overlay's pixels depend on besides its
/// own view: a change to any of it repaints every overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(in crate::renderer) struct RasterContext {
    cell_h: f32,
    cell_w: f32,
    divider_width: f32,
    font_size: f32,
    line_height: f32,
    surface_h: f32,
    surface_w: f32,
    /// Bumped by every theme change, so a cache never compares whole themes.
    theme_generation: u64,
}

/// One overlay's last raster: the placements of its uploaded textures and
/// the context and view they were painted from.
pub(in crate::renderer) struct RasterCache<V> {
    entry: Option<CachedRaster<V>>,
}

struct CachedRaster<V> {
    context: RasterContext,
    placements: Vec<ImagePlacement>,
    view: V,
}

/// What a pane strip is painted from, besides the renderer's context.
#[derive(Clone, Debug, PartialEq)]
pub(in crate::renderer) struct PaneStripKey {
    pub(in crate::renderer) drop_target: PaneStripDropTarget,
    pub(in crate::renderer) edges: PaneStripEdges,
    pub(in crate::renderer) height: u32,
    pub(in crate::renderer) hover: PaneStripHit,
    pub(in crate::renderer) layout: PaneStripLayout,
    pub(in crate::renderer) origin: (f32, f32),
    pub(in crate::renderer) tabs: Vec<PaneTab>,
    pub(in crate::renderer) width: u32,
}

/// What the toast cards are painted from: the cards and how far below the
/// top edge the tabbar pushes them.
#[derive(Clone, Debug, PartialEq)]
struct ToastKey {
    cards: Vec<ToastCard>,
    top_inset: f32,
}

// ========================================================================
// RasterCache
// ========================================================================

impl<V> Default for RasterCache<V> {
    fn default() -> Self {
        Self { entry: None }
    }
}

impl<V> RasterCache<V> {
    /// The cached placements when they were painted under `context` from a
    /// view `matches` accepts, else `None`.
    pub(in crate::renderer) fn hit(
        &self,
        context: &RasterContext,
        matches: impl FnOnce(&V) -> bool,
    ) -> Option<&[ImagePlacement]> {
        let entry = self.entry.as_ref()?;
        (entry.context == *context && matches(&entry.view)).then_some(entry.placements.as_slice())
    }

    /// Remember `placements` as painted under `context` from `view`.
    pub(in crate::renderer) fn store(
        &mut self,
        context: RasterContext,
        view: V,
        placements: Vec<ImagePlacement>,
    ) {
        self.entry = Some(CachedRaster {
            context,
            placements,
            view,
        });
    }
}

// ========================================================================
// GpuRenderer: painting and uploading chrome
// ========================================================================

impl GpuRenderer {
    // --------------------------------------------------------------------
    // Shared plumbing
    // --------------------------------------------------------------------

    /// Lend a painter the font machinery and the metrics to shape with.
    pub(in crate::renderer) fn chrome_painter(&mut self) -> ChromePainter<'_> {
        ChromePainter {
            ctx: FontCtx {
                bold_weight: self.bold_weight.as_deref(),
                cell_h: self.cell_height,
                cell_w: self.cell_width,
                family: self.font_family.as_deref(),
                font_has_bold: self.font_has_bold,
                font_size: self.font_size,
                line_height: self.line_height,
                normal_weight: self.normal_weight.as_deref(),
            },
            font_system: &mut self.font_system,
            swash_cache: &mut self.swash_cache,
            theme: &self.theme,
        }
    }

    /// The renderer state every overlay raster depends on at this surface
    /// size.
    pub(in crate::renderer) fn raster_context(
        &self,
        surface_w: f32,
        surface_h: f32,
    ) -> RasterContext {
        RasterContext {
            cell_h: self.cell_height,
            cell_w: self.cell_width,
            divider_width: self.divider_width,
            font_size: self.font_size,
            line_height: self.line_height,
            surface_h,
            surface_w,
            theme_generation: self.theme_generation,
        }
    }

    /// Upload `image` as the texture `id` and place it where it was painted.
    pub(in crate::renderer) fn upload_overlay(
        &mut self,
        id: u64,
        image: &DropdownImage,
    ) -> ImagePlacement {
        self.image_pass.upload(
            &self.device,
            &self.queue,
            id,
            &image.rgba,
            image.width,
            image.height,
        );
        ImagePlacement {
            alpha: 1.0,
            height: image.height as f32,
            id,
            u_max: 1.0,
            u_min: 0.0,
            v_max: 1.0,
            v_min: 0.0,
            width: image.width as f32,
            x: image.x,
            y: image.y,
        }
    }

    /// The placements of the overlay `cache` selects for `view`: the cached
    /// ones when nothing they were painted from has moved, else the ones
    /// `paint` paints and uploads, which are then cached.
    fn cached_overlay<V: Clone + PartialEq>(
        &mut self,
        cache: fn(&mut OverlayCaches) -> &mut RasterCache<V>,
        context: RasterContext,
        view: &V,
        paint: impl FnOnce(&mut Self) -> Vec<ImagePlacement>,
    ) -> Vec<ImagePlacement> {
        if let Some(placements) = cache(&mut self.overlay_caches).hit(&context, |v| v == view) {
            return placements.to_vec();
        }
        let placements = paint(self);
        cache(&mut self.overlay_caches).store(context, view.clone(), placements.clone());
        placements
    }

    /// Paint `image` with `paint` and upload it as the texture `id`, or
    /// nothing when there is nothing to show.
    fn paint_overlay(
        &mut self,
        id: u64,
        paint: impl FnOnce(ChromePainter<'_>) -> Option<DropdownImage>,
    ) -> Vec<ImagePlacement> {
        let image = paint(self.chrome_painter());
        image
            .map(|image| self.upload_overlay(id, &image))
            .into_iter()
            .collect()
    }

    // --------------------------------------------------------------------
    // Overlays
    // --------------------------------------------------------------------

    /// Rasterize the command palette overlay and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_palette(
        &mut self,
        palette: &PaletteView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.palette,
            context,
            palette,
            |r| {
                r.paint_overlay(PALETTE_TEXTURE_ID, |p| {
                    Some(palette_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        palette,
                        surface_w,
                        surface_h,
                    ))
                })
            },
        )
    }

    /// Rasterize the input dialog and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_input(
        &mut self,
        input: &InputView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.input,
            context,
            input,
            |r| {
                r.paint_overlay(INPUT_TEXTURE_ID, |p| {
                    Some(input_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        input,
                        surface_w,
                        surface_h,
                    ))
                })
            },
        )
    }

    /// Rasterize the find popup and upload it to the GPU, noting where it
    /// landed for [`GpuRenderer::find_at`].
    pub(in crate::renderer) fn rasterize_find(
        &mut self,
        find: &FindView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        let placed = self.cached_overlay(
            |c| &mut c.find,
            context,
            find,
            |r| {
                r.paint_overlay(FIND_TEXTURE_ID, |p| {
                    Some(find_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        find,
                    ))
                })
            },
        );
        // The card's own box, inside the shadow its texture carries.
        self.find_rect = placed.first().map(|image| {
            (
                image.x + DROPDOWN_SHADOW,
                image.y + DROPDOWN_SHADOW,
                image.width - 2.0 * DROPDOWN_SHADOW,
                image.height - 2.0 * DROPDOWN_SHADOW,
            )
        });
        placed
    }

    /// Whether the surface point `(x, y)` is on the find popup, as it was
    /// last drawn: a click there is the popup's, not the pane's under it.
    pub fn find_at(&self, x: f32, y: f32) -> bool {
        self.find_rect
            .is_some_and(|(left, top, w, h)| x >= left && x < left + w && y >= top && y < top + h)
    }

    /// Rasterize the which-key hint overlay and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_which_key(
        &mut self,
        which_key: &WhichKeyView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.which_key,
            context,
            which_key,
            |r| {
                r.paint_overlay(WHICH_KEY_TEXTURE_ID, |p| {
                    Some(which_key_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        which_key,
                        surface_w,
                        surface_h,
                    ))
                })
            },
        )
    }

    /// Rasterize a URL tooltip bubble near the cursor when a link is hovered.
    /// Empty when no URL tooltip is set.
    pub(in crate::renderer) fn rasterize_url_tooltip(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let Some((url, cx, cy)) = tabbar.url_tooltip.as_ref() else {
            return Vec::new();
        };
        let context = self.raster_context(surface_w, surface_h);
        let (cw, ch) = (self.cell_width, self.cell_height);
        self.cached_overlay(
            |c| &mut c.url_tooltip,
            context,
            tabbar,
            |r| {
                let anchor = Region {
                    x: *cx,
                    y: *cy,
                    w: 1.0,
                    h: ch,
                };
                let hovered_tab = match tabbar.tabbar_hover {
                    TabbarHit::Tab(idx) => Some(idx),
                    _ => None,
                };
                let tab_layout = hovered_tab.map(|_| tabbar_layout(tabbar, surface_w, cw, ch));
                r.paint_overlay(URL_TOOLTIP_TEXTURE_ID, |p| {
                    Some(url_tooltip_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        url,
                        &anchor,
                        surface_w,
                        surface_h,
                        tab_layout.as_ref(),
                        hovered_tab,
                    ))
                })
            },
        )
    }

    /// Rasterize the toast cards, newest first, stacked down the top-right
    /// corner just below the tab bar, and remember where each landed for the
    /// pointer to find.
    pub(in crate::renderer) fn rasterize_toasts(
        &mut self,
        cards: &[ToastCard],
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let cards = &cards[..cards.len().min(TOAST_TEXTURE_IDS.len())];
        if cards.is_empty() {
            self.toast_rects.clear();
            return Vec::new();
        }
        // Below the tab bar, so a card never covers the tabs: rows of tabbar
        // chrome times the cell height, as the other overlays reckon it.
        let top_inset = match (self.tabbar_enabled, self.modern) {
            (false, _) => 0.0,
            (true, true) => crate::modern_titlebar_height_px(self.cell_height),
            (true, false) => CLASSIC_TABBAR_ROWS * self.cell_height,
        };
        let key = ToastKey {
            cards: cards.to_vec(),
            top_inset,
        };
        let context = self.raster_context(surface_w, surface_h);
        let panel_w = toast::card_width(surface_w);
        let panel_x = surface_w - toast::EDGE_GAP - panel_w;
        let placements = self.cached_overlay(
            |c| &mut c.toast,
            context,
            &key,
            |r| {
                let mut top = top_inset + toast::EDGE_GAP;
                let mut placements = Vec::new();
                for (card, id) in cards.iter().zip(TOAST_TEXTURE_IDS) {
                    let mut panel_h = 0.0;
                    placements.extend(r.paint_overlay(id, |p| {
                        let (frame, height) = toast::toast_card(
                            p.font_system,
                            p.swash_cache,
                            &p.ctx,
                            p.theme,
                            card,
                            panel_w,
                        );
                        panel_h = height;
                        Some(frame.into_image(panel_x, top))
                    }));
                    top += panel_h + toast::CARD_GAP;
                }
                placements
            },
        );
        // The panel is the placed image less its shadow margin on each side.
        self.toast_rects = cards
            .iter()
            .zip(&placements)
            .map(|(card, placed)| {
                let panel = (
                    placed.x + DROPDOWN_SHADOW,
                    placed.y + DROPDOWN_SHADOW,
                    placed.width - 2.0 * DROPDOWN_SHADOW,
                    placed.height - 2.0 * DROPDOWN_SHADOW,
                );
                ToastRect {
                    close: toast::close_rect(panel, self.cell_width, self.line_height),
                    id: card.id,
                    panel,
                }
            })
            .collect();
        placements
    }

    /// The toast card under the surface point `(x, y)`, as last drawn, and
    /// whether the point is on its close button.
    pub fn toast_at(&self, x: f32, y: f32) -> Option<ToastHit> {
        let inside = |(left, top, width, height): (f32, f32, f32, f32)| {
            x >= left && x < left + width && y >= top && y < top + height
        };
        self.toast_rects
            .iter()
            .find(|rect| inside(rect.panel))
            .map(|rect| ToastHit {
                close: inside(rect.close),
                id: rect.id,
            })
    }

    /// Rasterize and place the open menu's overlays: the parent dropdown, the
    /// open submenu (drawn after, so it overlays the parent), and the context
    /// menu. One placement per visible panel, or empty when no menu is open.
    ///
    /// The pixel work lives in [`dropdown_rgba`] and [`submenu_rgba`] so it can
    /// run (and be tested) without the GPU; this only uploads the result.
    pub(in crate::renderer) fn rasterize_dropdown(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.dropdown,
            context,
            tabbar,
            |r| {
                let mut placements = r.paint_overlay(DROPDOWN_TEXTURE_ID, |p| {
                    dropdown_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        tabbar,
                        surface_w,
                    )
                });
                placements.extend(r.paint_overlay(SUBMENU_TEXTURE_ID, |p| {
                    submenu_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        tabbar,
                        surface_w,
                    )
                }));
                placements.extend(r.paint_overlay(CONTEXT_MENU_TEXTURE_ID, |p| {
                    context_menu_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        tabbar,
                        surface_w,
                    )
                }));
                placements
            },
        )
    }

    /// Where to draw the tabbar strip, repainting it only when what it is drawn
    /// from has moved since the last frame.
    pub(in crate::renderer) fn tabbar_strip_placement(
        &mut self,
        tabbar: &TopTabbar,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.tabbar_strip,
            context,
            tabbar,
            |r| {
                let strip =
                    tabbar_strip_rgba(tabbar, surface_w, r.cell_width, r.cell_height, &r.theme);
                r.tabbar_strip_pass.upload(
                    &r.device,
                    &r.queue,
                    TABBAR_STRIP_TEXTURE_ID,
                    &strip.rgba,
                    strip.width,
                    strip.height,
                );
                vec![ImagePlacement {
                    alpha: 1.0,
                    height: strip.height as f32,
                    id: TABBAR_STRIP_TEXTURE_ID,
                    u_max: 1.0,
                    u_min: 0.0,
                    v_max: 1.0,
                    v_min: 0.0,
                    width: strip.width as f32,
                    x: 0.0,
                    y: 0.0,
                }]
            },
        )
    }
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::sample_menu_chrome;

    fn context() -> RasterContext {
        RasterContext {
            cell_h: 18.0,
            cell_w: 9.0,
            divider_width: 1.0,
            font_size: 15.0,
            line_height: 18.0,
            surface_h: 600.0,
            surface_w: 800.0,
            theme_generation: 0,
        }
    }

    fn placement() -> ImagePlacement {
        ImagePlacement {
            alpha: 1.0,
            height: 10.0,
            id: 7,
            u_max: 1.0,
            u_min: 0.0,
            v_max: 1.0,
            v_min: 0.0,
            width: 10.0,
            x: 0.0,
            y: 0.0,
        }
    }

    #[test]
    fn test_an_unchanged_overlay_reuses_the_raster_it_already_painted() {
        let tabbar = sample_menu_chrome(None);
        let mut cache = RasterCache::default();
        assert!(cache.hit(&context(), |v| *v == tabbar).is_none(), "cold");
        cache.store(context(), tabbar.clone(), vec![placement()]);
        let hit = cache.hit(&context(), |v| *v == tabbar).expect("a hit");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].id, 7);
    }

    #[test]
    fn test_an_overlay_is_repainted_when_anything_it_is_drawn_from_moves() {
        let tabbar = sample_menu_chrome(None);
        let mut cache = RasterCache::default();
        cache.store(context(), tabbar.clone(), vec![placement()]);

        // A renamed tab is the case that would show a stale title forever.
        let mut renamed = tabbar.clone();
        renamed.tabs[0].title = "renamed".into();
        assert!(cache.hit(&context(), |v| *v == renamed).is_none(), "title");
        let mut switched = tabbar.clone();
        switched.active_tab += 1;
        assert!(
            cache.hit(&context(), |v| *v == switched).is_none(),
            "active"
        );

        let moved = [
            RasterContext {
                surface_w: 900.0,
                ..context()
            },
            RasterContext {
                cell_w: 10.0,
                ..context()
            },
            RasterContext {
                font_size: 16.0,
                ..context()
            },
            RasterContext {
                theme_generation: 1,
                ..context()
            },
        ];
        for moved in moved {
            assert!(cache.hit(&moved, |v| *v == tabbar).is_none(), "{moved:?}");
        }
    }
}

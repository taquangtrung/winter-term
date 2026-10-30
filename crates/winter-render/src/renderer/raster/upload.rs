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
use crate::renderer::FrameHits;
use crate::renderer::widgets::find::find_rgba;
use crate::renderer::widgets::input::{confirm_button_rects, input_rgba, resolve_confirm_buttons};
use crate::renderer::widgets::menu::{context_menu_rgba, dropdown_rgba, submenu_rgba};
use crate::renderer::widgets::popup::popup_rgba;
use crate::renderer::widgets::picker::picker_rgba;
use crate::renderer::widgets::toast;
use crate::renderer::widgets::token_inspect::token_inspect_rgba;
use crate::renderer::widgets::completion::completion_rgba;
use crate::renderer::widgets::drop_list::drop_list_rgba;
use crate::renderer::widgets::info::info_rgba;
use crate::renderer::widgets::toolbar::{toolbar_rgba, ToolbarHit};
use crate::renderer::widgets::tooltip::url_tooltip_rgba;
use crate::renderer::widgets::which_key::which_key_rgba;
use crate::renderer::widgets::{
    CompletionView, DropListView, FindView, InfoView, InputView, PickerView, TokenInspectView, ToolbarView, WhichKeyView,
};
use crate::renderer::{GpuRenderer, ToastCard, ToastHit};
use crate::tabbar::{
    layout as tabbar_layout, PaneStripDropTarget, PaneStripHit, PaneStripLayout,
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

/// Reserved id for the rasterized token inspect popup.
const TOKEN_INSPECT_TEXTURE_ID: u64 = u64::MAX - 20;

/// Reserved id for the rasterized find popup.
const FIND_TEXTURE_ID: u64 = u64::MAX - 4;

/// Reserved id for the rasterized input dialog.
const INPUT_TEXTURE_ID: u64 = u64::MAX - 9;

/// The texture of the first pane toolbar on screen; each one after takes the
/// slot below it.
const TOOLBAR_TEXTURE_BASE: u64 = u64::MAX - 400;

/// The texture of the completion popup.
const COMPLETION_TEXTURE_ID: u64 = u64::MAX - 30;

/// The texture of the drop-down list open over a pane.
const DROP_LIST_TEXTURE_ID: u64 = u64::MAX - 32;

/// The texture of the info card.
const INFO_TEXTURE_ID: u64 = u64::MAX - 31;

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
    palette: RasterCache<PickerView>,
    /// One per pane strip texture, keyed by its texture id.
    pub(in crate::renderer) pane_strips: HashMap<u64, RasterCache<PaneStripKey>>,
    tabbar_strip: RasterCache<TopTabbar>,
    toast: RasterCache<ToastKey>,
    token_inspect: RasterCache<TokenInspectView>,
    completion: RasterCache<CompletionView>,
    drop_list: RasterCache<DropListView>,
    info: RasterCache<InfoView>,
    /// One per pane toolbar texture, keyed by its texture id, with where its
    /// buttons were drawn.
    toolbars: HashMap<u64, ToolbarCache>,
    url_tooltip: RasterCache<TopTabbar>,
    which_key: RasterCache<WhichKeyView>,
}

/// A pane toolbar as last rasterized: what it was painted from, the image
/// uploaded for it, and where its buttons landed.
pub(in crate::renderer) struct ToolbarCache {
    context: RasterContext,
    hits: Vec<ToolbarHit>,
    placements: Vec<ImagePlacement>,
    view: ToolbarView,
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
    /// How many rows a popup's input field is: a setting changed live, which
    /// cached popups would otherwise keep drawing at the old height.
    popup_input_height: f32,
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
                bold_weight: self.fonts.bold_weight.as_deref(),
                cell_h: self.fonts.cell_height,
                cell_w: self.fonts.cell_width,
                family: self.fonts.font_family.as_deref(),
                field_h: self.popup_field_height(),
                font_has_bold: self.fonts.font_has_bold,
                font_size: self.fonts.font_size,
                line_height: self.fonts.line_height,
                normal_weight: self.fonts.normal_weight.as_deref(),
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
            cell_h: self.fonts.cell_height,
            cell_w: self.fonts.cell_width,
            divider_width: self.chrome.divider_width,
            font_size: self.fonts.font_size,
            line_height: self.fonts.line_height,
            popup_input_height: self.chrome.popup_input_height,
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
            // A texture freed or replaced since leaves the cached placements
            // pointing at nothing, which drew a blank overlay.
            if placements.iter().all(|placement| self.image_pass.has(placement.id)) {
                return placements.to_vec();
            }
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

    /// Rasterize the completion popup and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_completion(
        &mut self,
        view: &CompletionView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.completion,
            context,
            view,
            |r| {
                r.paint_overlay(COMPLETION_TEXTURE_ID, |p| {
                    completion_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        view,
                        surface_w,
                        surface_h,
                    )
                })
            },
        )
    }

    /// Rasterize the drop-down list open over a pane and upload it to the
    /// GPU.
    pub(in crate::renderer) fn rasterize_drop_list(
        &mut self,
        view: &DropListView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.drop_list,
            context,
            view,
            |r| r.paint_overlay(DROP_LIST_TEXTURE_ID, |p| drop_list_rgba(p.font_system, p.swash_cache, &p.ctx, p.theme, view)),
        )
    }

    /// Rasterize the info card and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_info(
        &mut self,
        view: &InfoView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.info,
            context,
            view,
            |r| {
                r.paint_overlay(INFO_TEXTURE_ID, |p| {
                    info_rgba(p.font_system, p.swash_cache, &p.ctx, p.theme, view, surface_w, surface_h)
                })
            },
        )
    }

    /// Rasterize each pane's toolbar, reusing what was painted for one that
    /// has not changed, and keep where its buttons were drawn for a press.
    pub(in crate::renderer) fn rasterize_toolbars(
        &mut self,
        toolbars: &[&ToolbarView],
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        let mut placed = Vec::new();
        self.hits.toolbar_hits.clear();
        for (at, view) in toolbars.iter().enumerate() {
            let id = TOOLBAR_TEXTURE_BASE - at as u64;
            let fresh = self
                .overlay_caches
                .toolbars
                .get(&id)
                .is_some_and(|cache| cache.context == context && cache.view == **view);
            if !fresh {
                let (image, hits) = {
                    let painter = self.chrome_painter();
                    toolbar_rgba(
                        painter.font_system,
                        painter.swash_cache,
                        &painter.ctx,
                        painter.theme,
                        view,
                    )
                };
                let placements = vec![self.upload_overlay(id, &image)];
                self.overlay_caches.toolbars.insert(
                    id,
                    ToolbarCache {
                        context,
                        hits,
                        placements,
                        view: (*view).clone(),
                    },
                );
            }
            if let Some(cache) = self.overlay_caches.toolbars.get(&id) {
                placed.extend(cache.placements.iter().cloned());
                let (_, top, _, height) = view.rect;
                self.hits.toolbar_hits
                    .push((view.owner, (top, top + height), cache.hits.clone()));
            }
        }
        placed
    }

    /// Rasterize the command palette overlay and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_palette(
        &mut self,
        palette: &PickerView,
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
                    Some(match palette.anchor {
                        Some(anchor) => popup_rgba(
                            p.font_system,
                            p.swash_cache,
                            &p.ctx,
                            p.theme,
                            palette,
                            anchor,
                            surface_w,
                            surface_h,
                        ),
                        None => picker_rgba(
                            p.font_system,
                            p.swash_cache,
                            &p.ctx,
                            p.theme,
                            palette,
                            surface_w,
                            surface_h,
                        ),
                    })
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
        let placed = self.cached_overlay(
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
        );
        self.hits.input_rect = placed.first().map(|image| {
            (
                image.x + DROPDOWN_SHADOW,
                image.y + DROPDOWN_SHADOW,
                image.width - 2.0 * DROPDOWN_SHADOW,
                image.height - 2.0 * DROPDOWN_SHADOW,
            )
        });
        self.hits.input_button_rects = match self.hits.input_rect {
            Some(card_rect)
                if (input.input.is_none() && !input.keys.is_empty())
                    || !input.buttons.is_empty() =>
            {
                let buttons = resolve_confirm_buttons(input);
                let ctx = FontCtx {
                    bold_weight: self.fonts.bold_weight.as_deref(),
                    cell_h: self.fonts.cell_height,
                    cell_w: self.fonts.cell_width,
                    family: self.fonts.font_family.as_deref(),
                    field_h: self.popup_field_height(),
                    font_has_bold: self.fonts.font_has_bold,
                    font_size: self.fonts.font_size,
                    line_height: self.fonts.line_height,
                    normal_weight: self.fonts.normal_weight.as_deref(),
                };
                confirm_button_rects(&mut self.font_system, &ctx, &buttons, card_rect)
            }
            _ => Vec::new(),
        };
        placed
    }

    /// Rasterize the find popup and upload it to the GPU, noting where it
    /// landed for [`FrameHits::find_at`].
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
        self.hits.find_rect = placed.first().map(|image| {
            (
                image.x + DROPDOWN_SHADOW,
                image.y + DROPDOWN_SHADOW,
                image.width - 2.0 * DROPDOWN_SHADOW,
                image.height - 2.0 * DROPDOWN_SHADOW,
            )
        });
        placed
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

    /// Rasterize the token inspect overlay and upload it to the GPU.
    pub(in crate::renderer) fn rasterize_token_inspect(
        &mut self,
        token_inspect: &TokenInspectView,
        surface_w: f32,
        surface_h: f32,
    ) -> Vec<ImagePlacement> {
        let context = self.raster_context(surface_w, surface_h);
        self.cached_overlay(
            |c| &mut c.token_inspect,
            context,
            token_inspect,
            |r| {
                r.paint_overlay(TOKEN_INSPECT_TEXTURE_ID, |p| {
                    Some(token_inspect_rgba(
                        p.font_system,
                        p.swash_cache,
                        &p.ctx,
                        p.theme,
                        token_inspect,
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
        let (cw, ch) = (self.fonts.cell_width, self.fonts.cell_height);
        self.cached_overlay(
            |c| &mut c.url_tooltip,
            context,
            tabbar,
            |r| {
                // Under what it is about: the element named, a title-bar
                // tab, or else the pointer's cell.
                let pointer = Region {
                    x: *cx,
                    y: *cy,
                    w: 1.0,
                    h: ch,
                };
                let anchor = tabbar.tooltip_anchor.unwrap_or_else(|| match tabbar.tabbar_hover {
                    TabbarHit::Tab(idx) => tabbar_layout(tabbar, surface_w, cw, ch)
                        .tabs
                        .get(idx)
                        .copied()
                        .unwrap_or(pointer),
                    _ => pointer,
                });
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
            self.hits.toast_rects.clear();
            return Vec::new();
        }
        // Below the tab bar, so a card never covers the tabs: rows of tabbar
        // chrome times the cell height, as the other overlays reckon it.
        let top_inset = match (self.chrome.tabbar_enabled, self.chrome.modern) {
            (false, _) => 0.0,
            (true, true) => crate::modern_titlebar_height_px(self.fonts.cell_height),
            (true, false) => CLASSIC_TABBAR_ROWS * self.fonts.cell_height,
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
        self.hits.toast_rects = cards
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
                    close: toast::close_rect(panel, self.fonts.cell_width, self.fonts.line_height),
                    id: card.id,
                    panel,
                }
            })
            .collect();
        placements
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
                    tabbar_strip_rgba(tabbar, surface_w, r.fonts.cell_width, r.fonts.cell_height, &r.theme);
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
// FrameHits
// ========================================================================

impl FrameHits {
    /// Whether the surface point `(x, y)` is on the input or confirmation popup.
    pub fn input_at(&self, x: f32, y: f32) -> bool {
        self.input_rect
            .is_some_and(|(left, top, w, h)| x >= left && x < left + w && y >= top && y < top + h)
    }

    /// If `(x, y)` is over a confirmation dialog button, returns the index of that button.
    pub fn confirm_button_at(&self, x: f32, y: f32) -> Option<usize> {
        self.input_button_rects
            .iter()
            .position(|&(bx, by, bw, bh)| x >= bx && x < bx + bw && y >= by && y < by + bh)
    }

    /// Whether the surface point `(x, y)` is on the find popup, as it was
    /// last drawn: a click there is the popup's, not the pane's under it.
    pub fn find_at(&self, x: f32, y: f32) -> bool {
        self.find_rect
            .is_some_and(|(left, top, w, h)| x >= left && x < left + w && y >= top && y < top + h)
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
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use crate::renderer::test_support::sample_menu_chrome;

    #[test]
    fn test_every_reserved_texture_id_is_its_own() {
        // The token inspector and the first status icon shared an ID: one
        // drew over the other's texture, and the icon stayed wrong after the
        // inspector closed.
        let mut ids = vec![
            SUBMENU_TEXTURE_ID,
            CONTEXT_MENU_TEXTURE_ID,
            TABBAR_STRIP_TEXTURE_ID,
            PALETTE_TEXTURE_ID,
            URL_TOOLTIP_TEXTURE_ID,
            WHICH_KEY_TEXTURE_ID,
            TOKEN_INSPECT_TEXTURE_ID,
            FIND_TEXTURE_ID,
            INPUT_TEXTURE_ID,
            COMPLETION_TEXTURE_ID,
            DROP_LIST_TEXTURE_ID,
            INFO_TEXTURE_ID,
        ];
        ids.extend(TOAST_TEXTURE_IDS);
        ids.extend(crate::renderer::raster::STATUS_ICON_TEXTURE_IDS);
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two uses share an ID");
        assert!(ids.iter().all(|id| *id > TOOLBAR_TEXTURE_BASE), "the toolbars count down from below them");
    }

    fn context() -> RasterContext {
        RasterContext {
            cell_h: 18.0,
            cell_w: 9.0,
            divider_width: 1.0,
            font_size: 15.0,
            line_height: 18.0,
            popup_input_height: 1.0,
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

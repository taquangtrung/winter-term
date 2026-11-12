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
use crate::renderer::widgets::input::{confirm_button_rects, input_rgba, resolve_confirm_buttons};
use crate::renderer::widgets::toolbar::{toolbar_rgba, ToolbarHit};
use crate::renderer::widgets::{InputView, ToolbarView};
use crate::renderer::FrameHits;
use crate::renderer::GpuRenderer;
use crate::theme::Theme;

// ========================================================================
// Constants
// ========================================================================

/// Reserved image-pass texture id for the rasterized input dialog. The
/// reserved ids sit at the top of the id space so they never collide with
/// block image ids.
const INPUT_TEXTURE_ID: u64 = u64::MAX - 9;

/// The texture of the first pane toolbar on screen; each one after takes the
/// slot below it.
const TOOLBAR_TEXTURE_BASE: u64 = u64::MAX - 400;

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
    input: RasterCache<InputView>,
    /// One per pane toolbar texture, keyed by its texture id, with where its
    /// buttons were drawn.
    toolbars: HashMap<u64, ToolbarCache>,
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
        self.image_pass
            .upload(&self.gpu, id, &image.rgba, image.width, image.height);
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
            if placements
                .iter()
                .all(|placement| self.image_pass.has(placement.id))
            {
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
                self.hits
                    .toolbar_hits
                    .push((view.owner, (top, top + height), cache.hits.clone()));
            }
        }
        placed
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
}

// ========================================================================
// FrameHits
// ========================================================================

impl FrameHits {
    /// The pictures and pane toolbars the last frame drew in the content,
    /// for a host drawing them itself (see [`GpuRenderer::image_texture`]).
    pub fn pictures(&self) -> &[ImagePlacement] {
        &self.pictures
    }

    /// The popups the last frame drew floating over the content.
    pub fn popups(&self) -> &[ImagePlacement] {
        &self.popups
    }

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

    /// The pane toolbar button at the surface point `(x, y)`, as the bars were
    /// last drawn: whose bar it is, and what the button is told back as.
    pub fn toolbar_button_at(&self, x: f32, y: f32) -> Option<(u64, u16)> {
        self.toolbar_hits
            .iter()
            .filter(|(_, (top, bottom), _)| y >= *top && y < *bottom)
            .find_map(|(owner, _, hits)| {
                hits.iter()
                    .find(|hit| x >= hit.left && x < hit.right)
                    .map(|hit| (*owner, hit.button))
            })
    }
}

// ========================================================================
// Free functions
// ========================================================================

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_every_reserved_texture_id_is_its_own() {
        // The token inspector and the first status icon shared an ID: one
        // drew over the other's texture, and the icon stayed wrong after the
        // inspector closed.
        let mut ids = vec![INPUT_TEXTURE_ID];
        let count = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), count, "two uses share an ID");
        assert!(
            ids.iter().all(|id| *id > TOOLBAR_TEXTURE_BASE),
            "the toolbars count down from below them"
        );
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
        let view = String::from("Terminal 1");
        let mut cache = RasterCache::default();
        assert!(cache.hit(&context(), |v| *v == view).is_none(), "cold");
        cache.store(context(), view.clone(), vec![placement()]);
        let hit = cache.hit(&context(), |v| *v == view).expect("a hit");
        assert_eq!(hit.len(), 1);
        assert_eq!(hit[0].id, 7);
    }

    #[test]
    fn test_an_overlay_is_repainted_when_anything_it_is_drawn_from_moves() {
        let view = String::from("Terminal 1");
        let mut cache = RasterCache::default();
        cache.store(context(), view.clone(), vec![placement()]);

        // A renamed tab is the case that would show a stale title forever.
        let renamed = String::from("renamed");
        assert!(cache.hit(&context(), |v| *v == renamed).is_none(), "title");

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
            assert!(cache.hit(&moved, |v| *v == view).is_none(), "{moved:?}");
        }
    }
}

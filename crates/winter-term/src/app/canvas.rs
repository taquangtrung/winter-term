//! The canvas: what is drawn in a pane rather than painted as cells, by
//! inline blocks and tools alike. One way to put any content into a texture
//! at the size it is shown, and the textures holding what tools show over
//! their panes, each cropped to its pane.

use std::collections::HashSet;

use winter_render::renderer::GpuRenderer;
use winter_render::ImagePlacement;

use crate::model::layout::{Rect, TabId};
use crate::model::picture::{Content, PictureClip, PictureFrame};

use super::App;

// ========================================================================
// Constants
// ========================================================================

/// The longest side vectors are drawn at. Shown larger, the drawing is
/// stretched rather than drawn again, which keeps one zoom from asking for a
/// texture no GPU holds.
const MAX_VECTOR_SIDE: f32 = 4096.0;

// ========================================================================
// Data Structures
// ========================================================================

/// How content is drawn into its texture.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Fit {
    /// At its own size: pixels, decoded or already drawn.
    Natural,
    /// At this many pixels across and down, vectors drawn again for it.
    Size(u32, u32),
    /// Laid out to this many pixels across.
    Width(u32),
}

/// What the GPU holds for one piece of what a tool shows.
pub(crate) struct ToolTexture {
    /// Set once the content failed to draw, so it is said once, not every
    /// frame.
    failed: bool,
    /// The texture it is drawn into.
    id: u64,
    /// Its own size, once known.
    natural: Option<(f32, f32)>,
    /// How it was last drawn, `None` before it is.
    uploaded: Option<Fit>,
}

// ========================================================================
// Fit
// ========================================================================

impl Fit {
    /// How `content` shown `shown` pixels big is drawn: laid out to that
    /// width, drawn again at that size, or at its own.
    pub(crate) fn shown(content: &Content, shown: (f32, f32)) -> Self {
        match content {
            Content::Markdown(_) | Content::Text(_) => Self::Width(shown.0.floor().max(1.0) as u32),
            Content::Vector(_) => {
                let fit = (MAX_VECTOR_SIDE / shown.0.max(shown.1)).min(1.0);
                Self::Size(
                    (shown.0 * fit).round().max(1.0) as u32,
                    (shown.1 * fit).round().max(1.0) as u32,
                )
            }
            Content::Pixels(_) | Content::Raster(_) => Self::Natural,
        }
    }
}

// ========================================================================
// App: tool content
// ========================================================================

impl App {
    /// The placements of what the tools in `shown` draw over their panes
    /// this frame, each cropped to its pane, every tool told its pane's
    /// size first and a picture's own size after.
    pub(super) fn tool_canvas_placements(
        &mut self,
        shown: &[TabId],
        rects: &[(TabId, Rect)],
        ch: f32,
    ) -> Vec<ImagePlacement> {
        let focused = self.layout().focused();
        let mut placements = Vec::new();
        let mut drawn = HashSet::new();
        for tab in shown {
            let Some((_, rect)) = rects.iter().find(|(id, _)| id == tab) else {
                continue;
            };
            let pane_rect = App::layout_rect_to_pane(*rect).grid_area(ch);
            let pane = (pane_rect.width, pane_rect.height);
            let Some(slot) = self.open_tabs.tool_mut(tab) else {
                continue;
            };
            slot.tool.set_pane_size(pane);
            let tiles = slot.tool.tiles();
            // Rows the tool painted over its drawing are kept clear of it.
            let header = (slot.tool.header_rows() as f32 * ch).min(pane.1);
            let bounds = (0.0, header, pane.0, pane.1 - header);
            let picture = slot.tool.picture();
            // Faded with the rest of an inactive pane, as a tool's icons are.
            let alpha = match self.config.dim_inactive && *tab != focused {
                true => 1.0 - winter_render::DIM_FACTOR,
                false => 1.0,
            };
            let place = |id: u64, clip: PictureClip| ImagePlacement {
                alpha,
                height: clip.height,
                id,
                u_max: clip.u_max,
                u_min: clip.u_min,
                v_max: clip.v_max,
                v_min: clip.v_min,
                width: clip.width,
                x: pane_rect.x + clip.x,
                y: pane_rect.y + clip.y,
            };
            for tile in &tiles {
                let Some(clip) = PictureFrame::from(tile).clip(bounds) else {
                    continue;
                };
                let fit = Fit::shown(&tile.content, (tile.width, tile.height));
                if let Some((id, _)) = self.tool_texture(*tab, tile.key, &tile.content, |_| fit) {
                    drawn.insert((*tab, tile.key));
                    placements.push(place(id, clip));
                }
            }
            let Some(picture) = picture else {
                continue;
            };
            let vector = picture.content.is_vector();
            let view = picture.view;
            let shown_at = |natural| {
                let frame = view.frame(natural, pane, vector);
                Fit::shown(&picture.content, (frame.width, frame.height))
            };
            let Some((id, natural)) =
                self.tool_texture(*tab, picture.key, &picture.content, shown_at)
            else {
                continue;
            };
            drawn.insert((*tab, picture.key));
            if let Some(slot) = self.open_tabs.tool_mut(tab) {
                slot.tool.set_picture_geometry(natural, pane);
            }
            if let Some(clip) = view.frame(natural, pane, vector).clip(bounds) {
                placements.push(place(id, clip));
            }
        }
        self.forget_tool_textures(shown, &drawn);
        placements
    }

    /// The texture holding `content`, keyed `key` by tab `tab`'s tool, and
    /// the content's own size: drawn the first time it is asked for, and
    /// again whenever `fit` asks for it drawn differently. `None` for content
    /// that cannot be drawn, which is said once.
    fn tool_texture(
        &mut self,
        tab: TabId,
        key: u64,
        content: &Content,
        fit: impl Fn((f32, f32)) -> Fit,
    ) -> Option<(u64, (f32, f32))> {
        if !self.tool_textures.contains_key(&(tab, key)) {
            let id = self.next_image_id;
            self.next_image_id += 1;
            self.tool_textures.insert(
                (tab, key),
                ToolTexture {
                    failed: false,
                    id,
                    natural: None,
                    uploaded: None,
                },
            );
        }
        let held = self.tool_textures.get(&(tab, key))?;
        if held.failed {
            return None;
        }
        let (id, natural, uploaded) = (held.id, held.natural, held.uploaded);
        let renderer = self.renderer.as_mut()?;
        let natural = natural.or_else(|| own_size(renderer, content));
        // Content whose size comes from drawing it is drawn first to learn it.
        let want = fit(natural.unwrap_or((0.0, 0.0)));
        let drawn = match uploaded == Some(want) {
            true => natural,
            false => upload(renderer, id, content, want),
        };
        let held = self.tool_textures.get_mut(&(tab, key))?;
        let Some(size) = drawn else {
            held.failed = true;
            let name = self
                .open_tabs
                .tool(&tab)
                .map(|slot| slot.tool.title())
                .unwrap_or_default();
            self.set_error(format!("{name}: cannot draw this"));
            return None;
        };
        // A vector's own size is the one it declares, not the one it was
        // drawn at; everything else's is what drawing it came to.
        let natural = match content.is_vector() {
            true => natural.unwrap_or(size),
            false => size,
        };
        held.natural = Some(natural);
        held.uploaded = Some(want);
        Some((id, natural))
    }

    /// Free the textures of tools that have closed, and those a tool shown
    /// this frame has moved on from. A tool not on screen keeps its own, so
    /// switching back to it draws at once rather than decoding again.
    fn forget_tool_textures(&mut self, shown: &[TabId], drawn: &HashSet<(TabId, u64)>) {
        let gone: Vec<(TabId, u64)> = self
            .tool_textures
            .keys()
            .filter(|(tab, key)| {
                self.open_tabs.tool(tab).is_none()
                    || (shown.contains(tab) && !drawn.contains(&(*tab, *key)))
            })
            .copied()
            .collect();
        for held in gone {
            if let (Some(texture), Some(renderer)) =
                (self.tool_textures.remove(&held), self.renderer.as_mut())
            {
                renderer.free_image(texture.id);
            }
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Draw `content` into texture `id` as `fit` says, and say what size it
/// came to: its own for pixels, laid out for text, drawn for vectors. `None`
/// for content that cannot be drawn.
pub(crate) fn upload(
    renderer: &mut GpuRenderer,
    id: u64,
    content: &Content,
    fit: Fit,
) -> Option<(f32, f32)> {
    let size = |(width, height): (u32, u32)| (width as f32, height as f32);
    match (content, fit) {
        (Content::Raster(bytes), _) => renderer.upload_image(id, bytes).map(size),
        (Content::Pixels(pixels), _) => {
            renderer.upload_rgba(id, &pixels.rgba, pixels.width, pixels.height);
            Some(size((pixels.width, pixels.height)))
        }
        (Content::Vector(svg), Fit::Size(width, height)) => renderer
            .upload_svg_in_box(id, svg, width, height)
            .then(|| size((width, height))),
        (Content::Markdown(markdown), Fit::Width(width)) => renderer
            .upload_markdown(id, markdown, width as f32)
            .map(size),
        (Content::Text(text), Fit::Width(width)) => {
            renderer.upload_text(id, text, width as f32).map(size)
        }
        // A fit that does not suit the content draws nothing: vectors are
        // drawn at a size, text laid out to a width.
        (Content::Vector(_), Fit::Natural | Fit::Width(_))
        | (Content::Markdown(_) | Content::Text(_), Fit::Natural | Fit::Size(..)) => None,
    }
}

/// The size `content` gives itself, where that is known without drawing
/// it: an SVG's declared size, or pixels already drawn.
pub(crate) fn own_size(renderer: &mut GpuRenderer, content: &Content) -> Option<(f32, f32)> {
    match content {
        Content::Vector(svg) => renderer.svg_size(svg),
        Content::Pixels(pixels) => Some((pixels.width as f32, pixels.height as f32)),
        Content::Markdown(_) | Content::Raster(_) | Content::Text(_) => None,
    }
}

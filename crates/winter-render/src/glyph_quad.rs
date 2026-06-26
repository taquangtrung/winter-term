//! Textured-quad pass for fallback glyphs whose cell-width correction must
//! not touch cosmic-text's line layout (see
//! `GpuRenderer::ensure_fallback_glyph_quad` for why). Each distinct grapheme
//! (a character plus any combining tail it composes with, e.g. a skin-tone
//! modifier or a paired flag half: see the renderer's `write_glyph_key`) is
//! rasterized once, at the renderer's unmodified font size, into either a
//! single-channel coverage mask (tinted per-instance by the cell's foreground
//! color) or, for a genuine color glyph such as a color emoji, its own RGBA
//! drawn untouched. Either way it is drawn as its own quad, uniformly scaled
//! to fit the cell, entirely independent of any other glyph on the same row.
//!
//! The rasters live in two shared atlas textures, one per kind, so a frame
//! draws every quad in at most two calls however many there are. A glyph
//! not drawn in the current frame is evicted, least recently used first,
//! when a new one needs its room: a screen of CJK text can hold thousands of
//! distinct glyphs over a session, and keeping each forever (or in a texture
//! of its own) grew without bound.

use std::collections::HashMap;

use etagere::{size2, AllocId, BucketedAtlasAllocator};
use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState,
    FrontFace, MultisampleState, Origin3d, PipelineLayoutDescriptor, PrimitiveState,
    PrimitiveTopology, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor, Sampler,
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureViewDescriptor,
    TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

// ========================================================================
// Constants
// ========================================================================

const GLYPH_QUAD_SHADER: &str = include_str!("glyph_quad.wgsl");

/// Side of each atlas texture, in pixels, unless the device allows less.
/// A 2048 square holds several thousand cell-sized glyphs.
const ATLAS_SIZE: u32 = 2048;

/// Blank pixels kept around each raster in the atlas, so linear filtering
/// at a quad's edge never samples a neighbouring glyph.
const ATLAS_PADDING: u32 = 1;

/// Quads the vertex buffer holds before it first has to grow.
const INITIAL_QUAD_CAPACITY: usize = 256;

const VERTEX_BYTES: usize = 28;
const VERTS_PER_QUAD: usize = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// Where to draw a cached glyph this frame, in pixels from the surface
/// top-left, tinted by `color` (linear-encoded sRGB, matching the other
/// passes). `slot` is where its raster sits, as [`GlyphQuadPass::slot`]
/// answered for it this frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GlyphQuadPlacement {
    pub color: (f32, f32, f32),
    pub height: f32,
    pub slot: GlyphSlot,
    pub width: f32,
    pub x: f32,
    pub y: f32,
}

/// A cached raster's place in its atlas: its native pixel size (so callers
/// can scale the quad from it), which atlas holds it, and its texture
/// coordinates there.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct GlyphSlot {
    pub height: u32,
    pub is_color: bool,
    pub u0: f32,
    pub u1: f32,
    pub v0: f32,
    pub v1: f32,
    pub width: u32,
}

/// Pixel data for [`GlyphQuadPass::upload`]: tightly packed `width * height`
/// single-channel coverage bytes when `is_color` is false, or
/// `width * height * 4` straight-alpha RGBA bytes when true (a genuine color
/// glyph, drawn without per-instance tinting).
pub(crate) struct GlyphTexture<'a> {
    pub height: u32,
    pub is_color: bool,
    pub pixels: &'a [u8],
    pub width: u32,
}

/// GPU pipelines plus the two glyph atlases. Both pipelines share the vertex
/// shader and bind group layout, differing only in the fragment shader:
/// `pipeline` tints a coverage mask with the per-instance color,
/// `pipeline_color` passes a color glyph's own RGBA through untouched.
pub(crate) struct GlyphQuadPass {
    color: AtlasPage,
    /// Vertices of this frame's color quads, after the mask quads'.
    color_vertex_count: u32,
    mask: AtlasPage,
    mask_vertex_count: u32,
    pipeline: RenderPipeline,
    pipeline_color: RenderPipeline,
    quad_capacity: usize,
    vertex_buffer: Buffer,
}

/// One atlas texture and the index of what it holds.
struct AtlasPage {
    bind_group: BindGroup,
    bytes_per_pixel: u32,
    index: AtlasIndex,
    texture: Texture,
}

/// The CPU side of an atlas: which rectangle each key's raster occupies,
/// when each was last drawn, and the allocator handing out the room.
struct AtlasIndex {
    allocator: BucketedAtlasAllocator,
    entries: HashMap<String, AtlasEntry>,
    /// The frame being built; entries touched in it are never evicted.
    frame: u64,
    size: u32,
}

#[derive(Clone, Copy, Debug)]
struct AtlasEntry {
    id: AllocId,
    last_used: u64,
    slot: GlyphSlot,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct GlyphQuadVertex {
    color: (f32, f32, f32),
    u: f32,
    v: f32,
    x: f32,
    y: f32,
}

// ========================================================================
// GlyphQuadVertex
// ========================================================================

impl GlyphQuadVertex {
    fn new(x: f32, y: f32, u: f32, v: f32, color: (f32, f32, f32)) -> Self {
        Self { color, u, v, x, y }
    }

    fn to_bytes(self) -> [u8; VERTEX_BYTES] {
        let mut out = [0u8; VERTEX_BYTES];
        out[0..4].copy_from_slice(&self.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.u.to_le_bytes());
        out[12..16].copy_from_slice(&self.v.to_le_bytes());
        out[16..20].copy_from_slice(&self.color.0.to_le_bytes());
        out[20..24].copy_from_slice(&self.color.1.to_le_bytes());
        out[24..28].copy_from_slice(&self.color.2.to_le_bytes());
        out
    }
}

// ========================================================================
// AtlasIndex
// ========================================================================

impl AtlasIndex {
    fn new(size: u32) -> Self {
        Self {
            allocator: BucketedAtlasAllocator::new(size2(size as i32, size as i32)),
            entries: HashMap::new(),
            frame: 0,
            size,
        }
    }

    /// Where `key` sits, marking it drawn this frame, or `None` if absent.
    fn touch(&mut self, key: &str) -> Option<GlyphSlot> {
        let frame = self.frame;
        let entry = self.entries.get_mut(key)?;
        entry.last_used = frame;
        Some(entry.slot)
    }

    /// Make room for a `width` x `height` raster under `key`, replacing any
    /// it held and evicting glyphs not drawn this frame (oldest first) when
    /// the atlas is full. Returns the top-left to write the pixels at, or
    /// `None` when even that leaves no room (every glyph is on screen now, or
    /// the raster is larger than the atlas).
    fn insert(&mut self, key: &str, width: u32, height: u32, is_color: bool) -> Option<(u32, u32)> {
        self.remove(key);
        let padded = size2(
            (width + 2 * ATLAS_PADDING) as i32,
            (height + 2 * ATLAS_PADDING) as i32,
        );
        let allocation = loop {
            if let Some(allocation) = self.allocator.allocate(padded) {
                break allocation;
            }
            if !self.evict_least_recently_used() {
                return None;
            }
        };
        let x = allocation.rectangle.min.x as u32 + ATLAS_PADDING;
        let y = allocation.rectangle.min.y as u32 + ATLAS_PADDING;
        let size = self.size as f32;
        let slot = GlyphSlot {
            height,
            is_color,
            u0: x as f32 / size,
            u1: (x + width) as f32 / size,
            v0: y as f32 / size,
            v1: (y + height) as f32 / size,
            width,
        };
        self.entries.insert(
            key.to_string(),
            AtlasEntry {
                id: allocation.id,
                last_used: self.frame,
                slot,
            },
        );
        Some((x, y))
    }

    /// Drop `key`'s raster, if it has one here, and free its room.
    fn remove(&mut self, key: &str) {
        if let Some(entry) = self.entries.remove(key) {
            self.allocator.deallocate(entry.id);
        }
    }

    /// Evict the least recently drawn glyph not drawn this frame. `false`
    /// when there is none left to evict.
    fn evict_least_recently_used(&mut self) -> bool {
        let victim = self
            .entries
            .iter()
            .filter(|(_, entry)| entry.last_used < self.frame)
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(key, _)| key.clone());
        match victim {
            Some(key) => {
                self.remove(&key);
                true
            }
            None => false,
        }
    }
}

// ========================================================================
// AtlasPage
// ========================================================================

impl AtlasPage {
    fn new(
        device: &Device,
        layout: &BindGroupLayout,
        sampler: &Sampler,
        format: TextureFormat,
        size: u32,
    ) -> Self {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("winter glyph quad atlas"),
            size: Extent3d {
                width: size,
                height: size,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("winter glyph quad atlas bind group"),
            layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(sampler),
                },
            ],
        });
        let bytes_per_pixel = match format {
            TextureFormat::R8Unorm => 1,
            _ => 4,
        };
        Self {
            bind_group,
            bytes_per_pixel,
            index: AtlasIndex::new(size),
            texture,
        }
    }

    /// Write `pixels` (`width` x `height`, tightly packed) at `origin`.
    fn write(&self, queue: &Queue, origin: (u32, u32), pixels: &[u8], width: u32, height: u32) {
        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: Origin3d {
                    x: origin.0,
                    y: origin.1,
                    z: 0,
                },
                aspect: TextureAspect::All,
            },
            pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * self.bytes_per_pixel),
                rows_per_image: Some(height),
            },
            Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
    }
}

// ========================================================================
// GlyphQuadPass
// ========================================================================

impl GlyphQuadPass {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("winter glyph quad shader"),
            source: ShaderSource::Wgsl(GLYPH_QUAD_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("winter glyph quad bind group layout"),
            entries: &[
                BindGroupLayoutEntry {
                    binding: 0,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Texture {
                        sample_type: TextureSampleType::Float { filterable: true },
                        view_dimension: TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                BindGroupLayoutEntry {
                    binding: 1,
                    visibility: ShaderStages::FRAGMENT,
                    ty: BindingType::Sampler(SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
            label: Some("winter glyph quad layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let build_pipeline = |entry_point: &'static str, label: &'static str| {
            device.create_render_pipeline(&RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(VertexBufferLayout {
                        array_stride: VERTEX_BYTES as u64,
                        step_mode: VertexStepMode::Vertex,
                        attributes: &[
                            VertexAttribute {
                                offset: 0,
                                format: VertexFormat::Float32x2,
                                shader_location: 0,
                            },
                            VertexAttribute {
                                offset: 8,
                                format: VertexFormat::Float32x2,
                                shader_location: 1,
                            },
                            VertexAttribute {
                                offset: 16,
                                format: VertexFormat::Float32x3,
                                shader_location: 2,
                            },
                        ],
                    })],
                },
                fragment: Some(FragmentState {
                    module: &shader,
                    entry_point: Some(entry_point),
                    compilation_options: Default::default(),
                    targets: &[Some(ColorTargetState {
                        format,
                        blend: Some(BlendState::ALPHA_BLENDING),
                        write_mask: ColorWrites::ALL,
                    })],
                }),
                primitive: PrimitiveState {
                    topology: PrimitiveTopology::TriangleList,
                    strip_index_format: None,
                    front_face: FrontFace::Ccw,
                    cull_mode: None,
                    polygon_mode: wgpu::PolygonMode::Fill,
                    unclipped_depth: false,
                    conservative: false,
                },
                depth_stencil: None,
                multisample: MultisampleState::default(),
                cache: None,
                multiview_mask: None,
            })
        };
        let pipeline = build_pipeline("fs_main", "winter glyph quad pipeline");
        let pipeline_color = build_pipeline("fs_main_color", "winter glyph quad color pipeline");

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("winter glyph quad sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let atlas_size = ATLAS_SIZE.min(device.limits().max_texture_dimension_2d);
        let mask = AtlasPage::new(
            device,
            &bind_group_layout,
            &sampler,
            TextureFormat::R8Unorm,
            atlas_size,
        );
        let color = AtlasPage::new(
            device,
            &bind_group_layout,
            &sampler,
            TextureFormat::Rgba8Unorm,
            atlas_size,
        );
        Self {
            color,
            color_vertex_count: 0,
            mask,
            mask_vertex_count: 0,
            pipeline,
            pipeline_color,
            quad_capacity: INITIAL_QUAD_CAPACITY,
            vertex_buffer: create_vertex_buffer(device, INITIAL_QUAD_CAPACITY),
        }
    }

    /// Store `texture` as the raster cached under `key` (a grapheme string,
    /// not necessarily a single character: a ZWJ sequence, skin-tone
    /// modifier, or flag pair rasterizes to one composed glyph but spans
    /// more than one `char`). A raster with no room left this frame is not
    /// stored, and [`Self::slot`] keeps answering `None` for it.
    pub fn upload(&mut self, queue: &Queue, key: &str, texture: GlyphTexture) {
        let GlyphTexture {
            height,
            is_color,
            pixels,
            width,
        } = texture;
        let (page, other) = match is_color {
            true => (&mut self.color, &mut self.mask),
            false => (&mut self.mask, &mut self.color),
        };
        other.index.remove(key);
        if let Some(origin) = page.index.insert(key, width, height, is_color) {
            page.write(queue, origin, pixels, width, height);
        }
    }

    /// Where `key`'s raster sits, or `None` if nothing is cached for it.
    /// Asking marks it drawn this frame, which keeps it from being evicted
    /// to make room for another glyph before the frame is drawn.
    pub fn slot(&mut self, key: &str) -> Option<GlyphSlot> {
        self.mask
            .index
            .touch(key)
            .or_else(|| self.color.index.touch(key))
    }

    /// Build the vertex buffer for this frame's `placements`, growing it when
    /// they outnumber what it holds, and start the next frame.
    pub fn prepare(
        &mut self,
        device: &Device,
        queue: &Queue,
        placements: &[GlyphQuadPlacement],
        surface_w: f32,
        surface_h: f32,
    ) {
        // Mask quads first, then color ones, so each kind is one draw.
        let mut verts: Vec<GlyphQuadVertex> = Vec::with_capacity(placements.len() * VERTS_PER_QUAD);
        for is_color in [false, true] {
            for placement in placements.iter().filter(|p| p.slot.is_color == is_color) {
                verts.extend_from_slice(&quad_vertices(placement, surface_w, surface_h));
            }
            if !is_color {
                self.mask_vertex_count = verts.len() as u32;
            }
        }
        self.color_vertex_count = verts.len() as u32 - self.mask_vertex_count;
        if placements.len() > self.quad_capacity {
            self.quad_capacity = placements.len().next_power_of_two();
            self.vertex_buffer = create_vertex_buffer(device, self.quad_capacity);
        }
        if !verts.is_empty() {
            let bytes: Vec<u8> = verts.iter().flat_map(|v| v.to_bytes()).collect();
            queue.write_buffer(&self.vertex_buffer, 0, &bytes);
        }
        self.mask.index.frame += 1;
        self.color.index.frame += 1;
    }

    /// Draw the placements prepared this frame. Call inside an active pass
    /// after the background has been drawn (same slot as text/braille dots).
    pub fn render<'pass>(&'pass self, pass: &mut RenderPass<'pass>) {
        let mask_end = self.mask_vertex_count;
        let color_end = mask_end + self.color_vertex_count;
        if color_end == 0 {
            return;
        }
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        if mask_end > 0 {
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &self.mask.bind_group, &[]);
            pass.draw(0..mask_end, 0..1);
        }
        if color_end > mask_end {
            pass.set_pipeline(&self.pipeline_color);
            pass.set_bind_group(0, &self.color.bind_group, &[]);
            pass.draw(mask_end..color_end, 0..1);
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// A vertex buffer holding `quads` quads.
fn create_vertex_buffer(device: &Device, quads: usize) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some("winter glyph quad vertices"),
        size: (quads * VERTS_PER_QUAD * VERTEX_BYTES) as u64,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The two triangles of `placement`, in clip space, sampling its atlas slot.
fn quad_vertices(
    placement: &GlyphQuadPlacement,
    surface_w: f32,
    surface_h: f32,
) -> [GlyphQuadVertex; VERTS_PER_QUAD] {
    let x0 = placement.x / surface_w * 2.0 - 1.0;
    let x1 = (placement.x + placement.width) / surface_w * 2.0 - 1.0;
    let y0 = 1.0 - placement.y / surface_h * 2.0;
    let y1 = 1.0 - (placement.y + placement.height) / surface_h * 2.0;
    let GlyphSlot { u0, u1, v0, v1, .. } = placement.slot;
    let color = placement.color;
    [
        GlyphQuadVertex::new(x0, y0, u0, v0, color),
        GlyphQuadVertex::new(x1, y0, u1, v0, color),
        GlyphQuadVertex::new(x0, y1, u0, v1, color),
        GlyphQuadVertex::new(x0, y1, u0, v1, color),
        GlyphQuadVertex::new(x1, y0, u1, v0, color),
        GlyphQuadVertex::new(x1, y1, u1, v1, color),
    ]
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Fill a small atlas with `count` 8x8 glyphs named `g0`, `g1`, ...,
    /// one frame apart, and report which were stored.
    fn fill(index: &mut AtlasIndex, count: usize) -> Vec<bool> {
        (0..count)
            .map(|i| {
                let stored = index.insert(&format!("g{i}"), 8, 8, false).is_some();
                index.frame += 1;
                stored
            })
            .collect()
    }

    #[test]
    fn test_a_full_atlas_evicts_the_least_recently_drawn_glyph() {
        let mut index = AtlasIndex::new(32);
        let stored = fill(&mut index, 64);
        assert!(
            stored.iter().all(|&s| s),
            "older glyphs make room for newer"
        );
        assert!(index.touch("g63").is_some(), "the newest is kept");
        assert!(index.touch("g0").is_none(), "the oldest went first");
    }

    #[test]
    fn test_a_glyph_drawn_this_frame_is_never_evicted() {
        // Everything in the atlas is on screen this frame: a new glyph is
        // refused rather than pulling one out from under a placed quad.
        let mut index = AtlasIndex::new(32);
        let mut count = 0;
        while index.insert(&format!("g{count}"), 8, 8, false).is_some() {
            count += 1;
        }
        assert!(count > 0);
        for i in 0..count {
            assert!(index.touch(&format!("g{i}")).is_some(), "g{i} kept");
        }
    }

    #[test]
    fn test_slots_lie_inside_the_atlas_and_apart() {
        let mut index = AtlasIndex::new(64);
        index.insert("a", 10, 12, false);
        index.insert("b", 10, 12, false);
        let a = index.touch("a").unwrap();
        let b = index.touch("b").unwrap();
        for slot in [a, b] {
            assert!(slot.u0 >= 0.0 && slot.u1 <= 1.0 && slot.v0 >= 0.0 && slot.v1 <= 1.0);
            assert!((slot.u1 - slot.u0 - 10.0 / 64.0).abs() < 1e-6);
        }
        let overlaps = a.u0 < b.u1 && b.u0 < a.u1 && a.v0 < b.v1 && b.v0 < a.v1;
        assert!(!overlaps, "two rasters must not share pixels");
    }
}

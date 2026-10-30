//! Textured-quad pass: draws decoded raster images on the GPU, so image blocks
//! render natively without a webview. Textures are uploaded once and cached by
//! block id; each frame the caller supplies pixel placements and the pass draws
//! the visible ones.

use std::collections::HashMap;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState,
    FrontFace, MultisampleState, Origin3d, PipelineLayoutDescriptor, PrimitiveState,
    PrimitiveTopology, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TexelCopyBufferLayout, TexelCopyTextureInfo, Texture, TextureAspect, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureView,
    TextureViewDescriptor,
    TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

// ========================================================================
// Constants
// ========================================================================

const IMAGE_SHADER: &str = include_str!("image.wgsl");
const MAX_QUADS: usize = 1024;
const VERTEX_BYTES: usize = 20;
const VERTS_PER_QUAD: u32 = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// Where to draw a cached image this frame, in pixels from the surface
/// top-left. `id` keys the uploaded texture. `u_min`/`u_max` and
/// `v_min`/`v_max` bound the sampled texture columns and rows (`0.0`/`1.0` =
/// whole image); narrowing them crops the image
/// rather than squashing it, which is how a block is kept inside the content
/// area as it scrolls past either edge. `alpha` multiplies the sampled color
/// (1.0 = full opacity), used to dim a closed live block.
#[derive(Clone, Copy, Debug)]
pub struct ImagePlacement {
    /// Opacity multiplier applied to the sampled color, `1.0` being fully
    /// opaque. Used to dim a closed live block.
    pub alpha: f32,
    /// Destination height in physical pixels.
    pub height: f32,
    /// Which uploaded texture to sample.
    pub id: u64,
    /// Right bound of the sampled `u` texture coordinate, `1.0` for a whole
    /// image. Below `1.0` the image is cropped from the right, which is how
    /// a picture zoomed wider than its pane is kept inside it.
    pub u_max: f32,
    /// Left bound of the sampled `u` texture coordinate, `0.0` for a whole
    /// image. Above `0.0` the image is cropped from the left.
    pub u_min: f32,
    /// Upper bound of the sampled `v` texture coordinate, `1.0` for a whole
    /// image. Below `1.0` the image is cropped from the bottom, which is how a
    /// block taller than its reserved band, or one running past the bottom of
    /// the content area, is clipped.
    pub v_max: f32,
    /// Lower bound of the sampled `v` texture coordinate, `0.0` for a whole
    /// image. Above `0.0` the image is cropped from the top, which is how a
    /// block whose band has scrolled partly above the content area keeps its
    /// remaining rows aligned with the grid instead of disappearing.
    pub v_min: f32,
    /// Destination width in physical pixels.
    pub width: f32,
    /// Destination left edge in physical pixels, from the window's left.
    pub x: f32,
    /// Destination top edge in physical pixels, from the window's top.
    pub y: f32,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct ImageVertex {
    alpha: f32,
    u: f32,
    v: f32,
    x: f32,
    y: f32,
}

/// GPU pipeline plus a per-image texture/bind-group cache.
pub(crate) struct ImagePass {
    /// How many of `draws` are the base layer, under what overlays the
    /// content (see [`ImagePass::render_base`]).
    base: usize,
    bind_group_layout: BindGroupLayout,
    draws: Vec<(u64, u32)>,
    pipeline: RenderPipeline,
    sampler: wgpu::Sampler,
    textures: HashMap<u64, ImageTexture>,
    vertex_buffer: Buffer,
}

/// One cached image: the texture holding its pixels, the bind group that
/// draws it, and its size, so a same-sized upload can rewrite the pixels in
/// place rather than allocate a fresh texture.
struct ImageTexture {
    bind_group: BindGroup,
    height: u32,
    texture: Texture,
    width: u32,
}

// ========================================================================
// ImageVertex
// ========================================================================

impl ImageVertex {
    fn new(x: f32, y: f32, u: f32, v: f32, alpha: f32) -> Self {
        Self { alpha, u, v, x, y }
    }

    fn to_bytes(self) -> [u8; VERTEX_BYTES] {
        let mut out = [0u8; VERTEX_BYTES];
        out[0..4].copy_from_slice(&self.x.to_le_bytes());
        out[4..8].copy_from_slice(&self.y.to_le_bytes());
        out[8..12].copy_from_slice(&self.u.to_le_bytes());
        out[12..16].copy_from_slice(&self.v.to_le_bytes());
        out[16..20].copy_from_slice(&self.alpha.to_le_bytes());
        out
    }
}

// ========================================================================
// ImagePass
// ========================================================================

impl ImagePass {
    pub fn new(device: &Device, format: TextureFormat) -> Self {
        let shader = device.create_shader_module(ShaderModuleDescriptor {
            label: Some("winter image shader"),
            source: ShaderSource::Wgsl(IMAGE_SHADER.into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&BindGroupLayoutDescriptor {
            label: Some("winter image bind group layout"),
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
            label: Some("winter image layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&RenderPipelineDescriptor {
            label: Some("winter image pipeline"),
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
                            format: VertexFormat::Float32,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: Some(FragmentState {
                module: &shader,
                entry_point: Some("fs_main"),
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
        });

        let sampler = device.create_sampler(&SamplerDescriptor {
            label: Some("winter image sampler"),
            mag_filter: FilterMode::Linear,
            min_filter: FilterMode::Linear,
            ..Default::default()
        });

        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("winter image vertices"),
            size: (MAX_QUADS * VERTS_PER_QUAD as usize * VERTEX_BYTES) as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            base: 0,
            bind_group_layout,
            draws: Vec::new(),
            pipeline,
            sampler,
            textures: HashMap::new(),
            vertex_buffer,
        }
    }

    /// Whether a texture is already cached for `id`.
    pub fn has(&self, id: u64) -> bool {
        self.textures.contains_key(&id)
    }

    /// Drop the texture cached under `id`, releasing its GPU memory. The
    /// bind group owns the texture view, which owns the texture, so removing
    /// it is what actually frees the memory; without this the cache only
    /// ever grows, keeping every decoded image block alive for the life of
    /// the process.
    pub fn remove(&mut self, id: u64) {
        self.textures.remove(&id);
    }

    /// Upload `rgba` (tightly packed, `width * height * 4` bytes) as a texture
    /// cached under `id`, replacing any previous one. A replacement of the
    /// same size is written into the texture already there: an overlay
    /// repainted while it stays open costs a copy, not an allocation.
    ///
    /// An image the GPU cannot hold (no pixels, more than its largest texture
    /// side, or bytes that do not match the size) is not uploaded, and `id`
    /// keeps what it had: handed to wgpu, it failed every frame with an error
    /// on the log and nothing drawn. Says whether it was uploaded.
    pub fn upload(
        &mut self,
        device: &Device,
        queue: &Queue,
        id: u64,
        rgba: &[u8],
        width: u32,
        height: u32,
    ) -> bool {
        if !fits_texture(width, height, rgba.len(), device.limits().max_texture_dimension_2d) {
            return false;
        }
        let cached = self.ensure_texture(device, id, width, height);
        write_rgba(queue, &cached.texture, rgba, width, height);
        true
    }

    /// A view of the `width` x `height` texture cached under `id`, for a
    /// compute pass to draw into (a scene rendered by Vello), made or
    /// reused as [`Self::upload`] does.
    pub fn target(&mut self, device: &Device, id: u64, width: u32, height: u32) -> TextureView {
        let cached = self.ensure_texture(device, id, width, height);
        cached.texture.create_view(&TextureViewDescriptor::default())
    }

    /// The texture cached under `id`, made anew unless the one there is
    /// already `width` x `height`.
    fn ensure_texture(
        &mut self,
        device: &Device,
        id: u64,
        width: u32,
        height: u32,
    ) -> &ImageTexture {
        let reusable = self
            .textures
            .get(&id)
            .is_some_and(|cached| (cached.width, cached.height) == (width, height));
        if !reusable {
            let image = self.create_texture(device, width, height);
            self.textures.insert(id, image);
        }
        &self.textures[&id]
    }

    /// A `width` x `height` texture and the bind group that samples it.
    fn create_texture(&self, device: &Device, width: u32, height: u32) -> ImageTexture {
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("winter image texture"),
            size: Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            // Holds sRGB-encoded bytes; the image shader decodes them to
            // linear before the (sRGB) surface re-encodes them. Vello writes
            // the same encoding, as a storage texture.
            format: TextureFormat::Rgba8Unorm,
            usage: TextureUsages::TEXTURE_BINDING
                | TextureUsages::COPY_DST
                | TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("winter image bind group"),
            layout: &self.bind_group_layout,
            entries: &[
                BindGroupEntry {
                    binding: 0,
                    resource: BindingResource::TextureView(&view),
                },
                BindGroupEntry {
                    binding: 1,
                    resource: BindingResource::Sampler(&self.sampler),
                },
            ],
        });
        ImageTexture {
            bind_group,
            height,
            texture,
            width,
        }
    }

    /// Build the vertex buffer for this frame's `placements`, the first
    /// `base` of them the layer under what overlays the content (see
    /// [`Self::render_base`]). Placements with no cached texture, or beyond
    /// [`MAX_QUADS`], are skipped.
    pub fn prepare(
        &mut self,
        queue: &Queue,
        placements: &[ImagePlacement],
        base: usize,
        surface_w: f32,
        surface_h: f32,
    ) {
        self.draws.clear();
        self.base = 0;
        let mut verts: Vec<ImageVertex> = Vec::new();
        for (index, placement) in placements.iter().enumerate() {
            if index == base {
                self.base = self.draws.len();
            }
            if !self.textures.contains_key(&placement.id) || self.draws.len() >= MAX_QUADS {
                continue;
            }
            let x0 = placement.x / surface_w * 2.0 - 1.0;
            let x1 = (placement.x + placement.width) / surface_w * 2.0 - 1.0;
            let y0 = 1.0 - placement.y / surface_h * 2.0;
            let y1 = 1.0 - (placement.y + placement.height) / surface_h * 2.0;
            let (u0, u1) = (placement.u_min, placement.u_max);
            let v0 = placement.v_min;
            let v1 = placement.v_max;
            let a = placement.alpha;
            let first = verts.len() as u32;
            verts.extend_from_slice(&[
                ImageVertex::new(x0, y0, u0, v0, a),
                ImageVertex::new(x1, y0, u1, v0, a),
                ImageVertex::new(x0, y1, u0, v1, a),
                ImageVertex::new(x0, y1, u0, v1, a),
                ImageVertex::new(x1, y0, u1, v0, a),
                ImageVertex::new(x1, y1, u1, v1, a),
            ]);
            self.draws.push((placement.id, first));
        }
        if placements.len() <= base {
            self.base = self.draws.len();
        }
        if !verts.is_empty() {
            let bytes: Vec<u8> = verts.iter().flat_map(|v| v.to_bytes()).collect();
            queue.write_buffer(&self.vertex_buffer, 0, &bytes);
        }
    }

    /// Draw the base layer prepared this frame: what sits in the content (a
    /// pane's strip of tabs, an image block, a toolbar). Call inside an active
    /// pass after the background and text have been drawn.
    pub fn render_base<'pass>(&'pass self, pass: &mut RenderPass<'pass>) {
        self.render_draws(pass, &self.draws[..self.base]);
    }

    /// Draw the overlay layer prepared this frame: what floats over the content
    /// (a tooltip, a menu, a popup, a toast), over anything drawn on the base
    /// layer since.
    pub fn render_overlays<'pass>(&'pass self, pass: &mut RenderPass<'pass>) {
        self.render_draws(pass, &self.draws[self.base..]);
    }

    /// Draw `draws`, in order.
    fn render_draws<'pass>(&'pass self, pass: &mut RenderPass<'pass>, draws: &[(u64, u32)]) {
        if draws.is_empty() {
            return;
        }
        pass.set_pipeline(&self.pipeline);
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for (id, first) in draws {
            if let Some(image) = self.textures.get(id) {
                pass.set_bind_group(0, &image.bind_group, &[]);
                pass.draw(*first..*first + VERTS_PER_QUAD, 0..1);
            }
        }
    }
}

// ========================================================================
// Free functions
// ========================================================================

/// Write `rgba` (`width` x `height`, tightly packed) over all of `texture`.
fn write_rgba(queue: &Queue, texture: &Texture, rgba: &[u8], width: u32, height: u32) {
    queue.write_texture(
        TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: Origin3d::ZERO,
            aspect: TextureAspect::All,
        },
        rgba,
        TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width * 4),
            rows_per_image: Some(height),
        },
        Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

/// Whether a `width` x `height` image of `bytes` RGBA bytes can be a
/// texture on a device whose largest side is `max_side`.
fn fits_texture(width: u32, height: u32, bytes: usize, max_side: u32) -> bool {
    let expected = width as usize * height as usize * 4;
    width > 0 && height > 0 && width <= max_side && height <= max_side && bytes == expected
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_an_image_the_gpu_cannot_hold_is_not_uploaded() {
        // A zero-sized strip or an outline taller than the largest texture
        // went to wgpu as it was, and failed there every frame.
        assert!(fits_texture(4, 2, 32, 8192));
        assert!(!fits_texture(0, 2, 0, 8192), "no pixels");
        assert!(!fits_texture(4, 9000, 4 * 9000 * 4, 8192), "taller than a texture");
        assert!(!fits_texture(4, 2, 31, 8192), "bytes not matching the size");
    }

    #[test]
    fn test_vertex_to_bytes_matches_the_declared_stride_and_field_order() {
        // Regression risk: VERTEX_BYTES and the pipeline's VertexBufferLayout
        // must agree with this layout byte-for-byte, or wgpu either
        // validation-panics at pipeline creation or silently reads the wrong
        // field per attribute.
        let v = ImageVertex::new(1.0, 2.0, 3.0, 4.0, 0.5);
        let bytes = v.to_bytes();
        assert_eq!(bytes.len(), VERTEX_BYTES);
        assert_eq!(f32::from_le_bytes(bytes[0..4].try_into().unwrap()), 1.0);
        assert_eq!(f32::from_le_bytes(bytes[4..8].try_into().unwrap()), 2.0);
        assert_eq!(f32::from_le_bytes(bytes[8..12].try_into().unwrap()), 3.0);
        assert_eq!(f32::from_le_bytes(bytes[12..16].try_into().unwrap()), 4.0);
        assert_eq!(f32::from_le_bytes(bytes[16..20].try_into().unwrap()), 0.5);
    }
}

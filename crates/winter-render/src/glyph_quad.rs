//! Textured-quad pass for fallback glyphs whose cell-width correction must
//! not touch cosmic-text's line layout (see `GpuRenderer::fallback_glyph_scale`
//! in `renderer.rs` for why). Each distinct grapheme (a character plus any
//! combining tail it composes with, e.g. a skin-tone modifier or a paired
//! flag half — see `renderer.rs`'s `glyph_key`) is rasterized once, at the
//! renderer's unmodified font size, into either a single-channel coverage
//! mask (tinted per-instance by the cell's foreground color) or, for a genuine
//! color glyph such as a color emoji, its own RGBA drawn untouched. Either way
//! it's drawn as its own quad, uniformly scaled to fit the cell, entirely
//! independent of any other glyph on the same row.

use std::collections::HashMap;

use wgpu::{
    BindGroup, BindGroupDescriptor, BindGroupEntry, BindGroupLayout, BindGroupLayoutDescriptor,
    BindGroupLayoutEntry, BindingResource, BindingType, BlendState, Buffer, BufferDescriptor,
    BufferUsages, ColorTargetState, ColorWrites, Device, Extent3d, FilterMode, FragmentState,
    FrontFace, MultisampleState, Origin3d, PipelineLayoutDescriptor, PrimitiveState,
    PrimitiveTopology, Queue, RenderPass, RenderPipeline, RenderPipelineDescriptor,
    SamplerBindingType, SamplerDescriptor, ShaderModuleDescriptor, ShaderSource, ShaderStages,
    TexelCopyBufferLayout, TexelCopyTextureInfo, TextureAspect, TextureDescriptor,
    TextureDimension, TextureFormat, TextureSampleType, TextureUsages, TextureViewDescriptor,
    TextureViewDimension, VertexAttribute, VertexBufferLayout, VertexFormat, VertexState,
    VertexStepMode,
};

// ========================================================================
// Constants
// ========================================================================

const GLYPH_QUAD_SHADER: &str = include_str!("glyph_quad.wgsl");
const MAX_QUADS: usize = 256;
const VERTEX_BYTES: usize = 28;
const VERTS_PER_QUAD: u32 = 6;

// ========================================================================
// Data Structures
// ========================================================================

/// Where to draw a cached glyph mask this frame, in pixels from the surface
/// top-left, tinted by `color` (linear-encoded sRGB, matching the other
/// passes). `key` looks up the cached texture (see [`GlyphQuadPass::upload`]
/// for why it's a grapheme string, not a single `char`).
#[derive(Clone, Debug)]
pub struct GlyphQuadPlacement {
    pub color: (f32, f32, f32),
    pub height: f32,
    pub key: String,
    pub width: f32,
    pub x: f32,
    pub y: f32,
}

/// Pixel data for [`GlyphQuadPass::upload`]: tightly packed `width * height`
/// single-channel coverage bytes when `is_color` is false, or
/// `width * height * 4` straight-alpha RGBA bytes when true (a genuine color
/// glyph, drawn without per-instance tinting).
pub struct GlyphTexture<'a> {
    pub height: u32,
    pub is_color: bool,
    pub pixels: &'a [u8],
    pub width: u32,
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

/// A cached glyph texture: the bind group to draw it, its pixel size (so
/// callers can scale the quad from its native dimensions), and whether it
/// holds genuine RGBA color (a color emoji) rather than a coverage mask
/// tinted per-instance.
struct GlyphQuadTexture {
    bind_group: BindGroup,
    height: u32,
    is_color: bool,
    width: u32,
}

/// GPU pipelines plus a per-character glyph texture cache. Two pipelines share
/// the same vertex shader and bind group layout, differing only in the
/// fragment shader: `pipeline` tints a single-channel coverage mask with the
/// per-instance color, `pipeline_color` passes a color glyph's own RGBA
/// through untouched.
pub struct GlyphQuadPass {
    bind_group_layout: BindGroupLayout,
    draws: Vec<(String, u32)>,
    pipeline: RenderPipeline,
    pipeline_color: RenderPipeline,
    sampler: wgpu::Sampler,
    textures: HashMap<String, GlyphQuadTexture>,
    vertex_buffer: Buffer,
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
                    buffers: &[VertexBufferLayout {
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
                    }],
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

        let vertex_buffer = device.create_buffer(&BufferDescriptor {
            label: Some("winter glyph quad vertices"),
            size: (MAX_QUADS * VERTS_PER_QUAD as usize * VERTEX_BYTES) as u64,
            usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Self {
            bind_group_layout,
            draws: Vec::new(),
            pipeline,
            pipeline_color,
            sampler,
            textures: HashMap::new(),
            vertex_buffer,
        }
    }

    /// Upload `texture` as the glyph texture cached under `key` (a grapheme
    /// string built by [`crate::renderer`]'s `glyph_key`, not necessarily a
    /// single character: a ZWJ sequence, skin-tone modifier, or flag pair
    /// rasterizes to one composed glyph but spans more than one `char`).
    pub fn upload(&mut self, device: &Device, queue: &Queue, key: &str, texture: GlyphTexture) {
        let GlyphTexture {
            height,
            is_color,
            pixels,
            width,
        } = texture;
        let format = if is_color {
            TextureFormat::Rgba8Unorm
        } else {
            TextureFormat::R8Unorm
        };
        let bytes_per_pixel = if is_color { 4 } else { 1 };
        let size = Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        };
        let texture = device.create_texture(&TextureDescriptor {
            label: Some("winter glyph quad texture"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: TextureDimension::D2,
            format,
            usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
            view_formats: &[],
        });

        queue.write_texture(
            TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: Origin3d::ZERO,
                aspect: TextureAspect::All,
            },
            pixels,
            TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * bytes_per_pixel),
                rows_per_image: Some(height),
            },
            size,
        );

        let view = texture.create_view(&TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&BindGroupDescriptor {
            label: Some("winter glyph quad bind group"),
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
        self.textures.insert(
            key.to_string(),
            GlyphQuadTexture {
                bind_group,
                height,
                is_color,
                width,
            },
        );
    }

    /// The native pixel size of the cached mask for `key`, or `None` if
    /// nothing is cached (so the caller knows how to scale the quad).
    pub fn dims(&self, key: &str) -> Option<(u32, u32)> {
        self.textures.get(key).map(|tex| (tex.width, tex.height))
    }

    /// Build the vertex buffer for this frame's `placements`. Placements with
    /// no cached mask, or beyond [`MAX_QUADS`], are skipped.
    pub fn prepare(
        &mut self,
        queue: &Queue,
        placements: &[GlyphQuadPlacement],
        surface_w: f32,
        surface_h: f32,
    ) {
        self.draws.clear();
        let mut verts: Vec<GlyphQuadVertex> = Vec::new();
        for placement in placements {
            if !self.textures.contains_key(&placement.key) || self.draws.len() >= MAX_QUADS {
                continue;
            }
            let x0 = placement.x / surface_w * 2.0 - 1.0;
            let x1 = (placement.x + placement.width) / surface_w * 2.0 - 1.0;
            let y0 = 1.0 - placement.y / surface_h * 2.0;
            let y1 = 1.0 - (placement.y + placement.height) / surface_h * 2.0;
            let color = placement.color;
            let first = verts.len() as u32;
            verts.extend_from_slice(&[
                GlyphQuadVertex::new(x0, y0, 0.0, 0.0, color),
                GlyphQuadVertex::new(x1, y0, 1.0, 0.0, color),
                GlyphQuadVertex::new(x0, y1, 0.0, 1.0, color),
                GlyphQuadVertex::new(x0, y1, 0.0, 1.0, color),
                GlyphQuadVertex::new(x1, y0, 1.0, 0.0, color),
                GlyphQuadVertex::new(x1, y1, 1.0, 1.0, color),
            ]);
            self.draws.push((placement.key.clone(), first));
        }
        if !verts.is_empty() {
            let bytes: Vec<u8> = verts.iter().flat_map(|v| v.to_bytes()).collect();
            queue.write_buffer(&self.vertex_buffer, 0, &bytes);
        }
    }

    /// Draw the placements prepared this frame. Call inside an active pass
    /// after the background has been drawn (same slot as text/braille dots).
    pub fn render<'pass>(&'pass self, pass: &mut RenderPass<'pass>) {
        if self.draws.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, self.vertex_buffer.slice(..));
        for (key, first) in &self.draws {
            if let Some(tex) = self.textures.get(key) {
                pass.set_pipeline(if tex.is_color {
                    &self.pipeline_color
                } else {
                    &self.pipeline
                });
                pass.set_bind_group(0, &tex.bind_group, &[]);
                pass.draw(*first..*first + VERTS_PER_QUAD, 0..1);
            }
        }
    }
}

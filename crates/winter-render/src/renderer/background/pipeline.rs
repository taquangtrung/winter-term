//! The wgpu pipelines the background pass draws with: opaque quads, and
//! anti-aliased procedural braille dots blended over them.

use wgpu::{
    BlendState, Buffer, BufferDescriptor, BufferUsages, ColorTargetState, ColorWrites, Device,
    FragmentState, FrontFace, MultisampleState, PipelineLayoutDescriptor, PolygonMode,
    PrimitiveState, PrimitiveTopology, RenderPipeline, RenderPipelineDescriptor,
    ShaderModuleDescriptor, ShaderSource, TextureFormat, VertexAttribute, VertexBufferLayout,
    VertexFormat, VertexState, VertexStepMode,
};

use super::{BgVertex, DotVertex};

// ========================================================================
// Constants
// ========================================================================

const BG_SHADER: &str = include_str!("../../bg.wgsl");

const DOT_SHADER: &str = include_str!("../../dot.wgsl");

/// Label of the background vertex buffer.
pub(crate) const BG_BUFFER_LABEL: &str = "winter bg vertices";

/// Label of the braille dot vertex buffer.
pub(crate) const DOT_BUFFER_LABEL: &str = "winter braille dot vertices";

// ========================================================================
// Pipelines
// ========================================================================

pub(crate) fn create_bg_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("winter bg shader"),
        source: ShaderSource::Wgsl(BG_SHADER.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("winter bg layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });

    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("winter bg pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(VertexBufferLayout {
                array_stride: std::mem::size_of::<BgVertex>() as u64,
                step_mode: VertexStepMode::Vertex,
                attributes: &[
                    VertexAttribute {
                        offset: 0,
                        format: VertexFormat::Float32x2,
                        shader_location: 0,
                    },
                    VertexAttribute {
                        offset: 8,
                        format: VertexFormat::Float32x3,
                        shader_location: 1,
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
                blend: Some(BlendState::REPLACE),
                write_mask: ColorWrites::ALL,
            })],
        }),
        primitive: PrimitiveState {
            topology: PrimitiveTopology::TriangleList,
            strip_index_format: None,
            front_face: FrontFace::Ccw,
            cull_mode: None,
            polygon_mode: PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

/// Pipeline for procedural braille dots: like the bg pipeline but with a `uv`
/// attribute and alpha blending, so the fragment shader can draw anti-aliased
/// circles that blend over the cell background.
pub(crate) fn create_dot_pipeline(device: &Device, format: TextureFormat) -> RenderPipeline {
    let shader = device.create_shader_module(ShaderModuleDescriptor {
        label: Some("winter dot shader"),
        source: ShaderSource::Wgsl(DOT_SHADER.into()),
    });

    let layout = device.create_pipeline_layout(&PipelineLayoutDescriptor {
        label: Some("winter dot layout"),
        bind_group_layouts: &[],
        immediate_size: 0,
    });

    device.create_render_pipeline(&RenderPipelineDescriptor {
        label: Some("winter dot pipeline"),
        layout: Some(&layout),
        vertex: VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            compilation_options: Default::default(),
            buffers: &[Some(VertexBufferLayout {
                array_stride: std::mem::size_of::<DotVertex>() as u64,
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
            polygon_mode: PolygonMode::Fill,
            unclipped_depth: false,
            conservative: false,
        },
        depth_stencil: None,
        multisample: MultisampleState::default(),
        cache: None,
        multiview_mask: None,
    })
}

// ========================================================================
// Vertex buffers
// ========================================================================

/// A vertex buffer of `size` bytes that frames write into.
pub(crate) fn create_vertex_buffer(device: &Device, label: &'static str, size: u64) -> Buffer {
    device.create_buffer(&BufferDescriptor {
        label: Some(label),
        size,
        usage: BufferUsages::VERTEX | BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// Replace `buffer` with a larger one when `needed` bytes do not fit, so a
/// frame's vertices are never written past its end. A write past the end is
/// a wgpu validation error that loses the whole frame, which a large window
/// at a small font reached with a fixed size.
pub(crate) fn ensure_vertex_capacity(
    device: &Device,
    buffer: &mut Buffer,
    label: &'static str,
    needed: u64,
) {
    if let Some(size) = grown_buffer_size(buffer.size(), needed) {
        *buffer = create_vertex_buffer(device, label, size);
    }
}

/// The size to grow a `current`-byte buffer to so `needed` bytes fit, or
/// `None` when they already do. Grows to a power of two, so a frame that
/// keeps getting a little bigger does not reallocate every time.
fn grown_buffer_size(current: u64, needed: u64) -> Option<u64> {
    (needed > current).then(|| needed.next_power_of_two())
}

// ========================================================================
// Tests
// ========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_a_buffer_grows_only_when_the_frame_outgrows_it() {
        assert_eq!(grown_buffer_size(1024, 1024), None, "an exact fit stays");
        assert_eq!(grown_buffer_size(1024, 1025), Some(2048));
        let size = grown_buffer_size(4 << 20, 7_000_000).unwrap();
        assert!(size >= 7_000_000, "the grown buffer holds the frame");
    }
}

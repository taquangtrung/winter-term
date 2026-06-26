struct VertexInput {
    @location(0) position: vec2f,
    @location(1) uv: vec2f,
    @location(2) color: vec3f,
}

struct VertexOutput {
    @builtin(position) position: vec4f,
    @location(0) uv: vec2f,
    @location(1) color: vec3f,
}

@vertex
fn vs_main(in: VertexInput) -> VertexOutput {
    var out: VertexOutput;
    out.position = vec4f(in.position, 0.0, 1.0);
    out.uv = in.uv;
    out.color = in.color;
    return out;
}

@group(0) @binding(0) var tex: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;

// Decode an sRGB color to linear, matching the bg/dot shaders so glyph quads
// blend gamma-correctly over the (sRGB) surface.
fn srgb_to_linear(c: vec3f) -> vec3f {
    let cutoff = c <= vec3f(0.04045);
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(high, low, cutoff);
}

// The texture is a single-channel coverage mask (a rasterized glyph); tint it
// with the per-instance color, same role `fs_main` plays for cosmic-text glyphs.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let alpha = textureSample(tex, samp, in.uv).r;
    return vec4f(srgb_to_linear(in.color), alpha);
}

// The texture is a genuine color glyph (COLR/CBDT color emoji): pass its own
// RGBA through untouched instead of tinting with the per-instance color, which
// would otherwise flatten it to a monochrome silhouette.
@fragment
fn fs_main_color(in: VertexOutput) -> @location(0) vec4f {
    let sampled = textureSample(tex, samp, in.uv);
    return vec4f(srgb_to_linear(sampled.rgb), sampled.a);
}

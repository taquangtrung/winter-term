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

// Decode an sRGB color to linear, matching the bg shader so braille dots blend
// gamma-correctly over the (sRGB) surface.
fn srgb_to_linear(c: vec3f) -> vec3f {
    let cutoff = c <= vec3f(0.04045);
    let low = c / 12.92;
    let high = pow((c + 0.055) / 1.055, vec3f(2.4));
    return select(high, low, cutoff);
}

// Anti-aliased filled circle. `uv` runs [-1, 1] across the dot quad, so the
// circle edge is at length(uv) == 1; fwidth gives a ~1px feather for a smooth
// edge at any dot size. Alpha feeds standard alpha blending over the cell.
@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4f {
    let d = length(in.uv);
    let aa = fwidth(d);
    let alpha = 1.0 - smoothstep(1.0 - aa, 1.0, d);
    return vec4f(srgb_to_linear(in.color), alpha);
}

// A blur along a path: the average of the source at the taps' moves of each pixel. The short box
// is `taps`, the comb `comb`, and every tap at once is the short box at each of the comb's moves.

struct SweepTap {
    // `M` as `(m00, m01, m10, m11)`: a tap reads `M p + t.xy` for canvas position `p`.
    m: vec4<f32>,
    t: vec4<f32>,
}

struct SweepParams {
    // `x`: taps of the short box, `y`: 1 when reads of the source clamp to the canvas, `z`: taps
    // of the comb.
    header: vec4<u32>,
    // Canvas pixel of the source's first texel in `xy`, and the canvas size in `zw`.
    source: vec4<i32>,
    // Canvas pixel of the first texel of the stored short boxes in `xy`, and of the output in
    // `zw`.
    origins: vec4<i32>,
    // `x`: how much of the output the comb of stored short boxes makes, against every tap at
    // once.
    blend: vec4<f32>,
    taps: array<SweepTap, 64>,
    comb: array<SweepTap, 64>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: SweepParams;
@group(0) @binding(2) var linear_sampler: sampler;
@group(0) @binding(3) var boxes_out: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var boxes_tex: texture_2d<f32>;

fn sweep_inside(index: f32, extent: f32) -> f32 {
    return select(0.0, 1.0, index >= 0.0 && index < extent);
}

// Fraction of a bilinear footprint at `pos` that lies inside a texture of `dims`.
fn sweep_coverage(pos: vec2<f32>, dims: vec2<f32>) -> f32 {
    let p = pos - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    let x = mix(sweep_inside(base.x, dims.x), sweep_inside(base.x + 1.0, dims.x), f.x);
    let y = mix(sweep_inside(base.y, dims.y), sweep_inside(base.y + 1.0, dims.y), f.y);
    return x * y;
}

// `tex`, whose first texel is at canvas pixel `origin`, at canvas position `pos`, transparent
// where it stores nothing. Scaling the sampler's clamped read by the footprint's coverage reads
// the texels outside as transparent.
fn sweep_read(tex: texture_2d<f32>, origin: vec2<i32>, pos: vec2<f32>) -> vec4<f32> {
    let local = pos - vec2<f32>(origin);
    let dims = vec2<f32>(textureDimensions(tex));
    return textureSampleLevel(tex, linear_sampler, local / dims, 0.0) * sweep_coverage(local, dims);
}

fn sweep_move(tap: SweepTap, pos: vec2<f32>) -> vec2<f32> {
    return vec2<f32>(dot(tap.m.xy, pos), dot(tap.m.zw, pos)) + tap.t.xy;
}

// The short box of the source about `pos`. Only reads of the source clamp, so a box past the
// canvas still averages what it covers.
fn sweep_box(pos: vec2<f32>) -> vec4<f32> {
    var sum = vec4<f32>(0.0);
    for (var index = 0u; index < params.header.x; index = index + 1u) {
        var at = sweep_move(params.taps[index], pos);
        if (params.header.y == 1u) {
            at = clamp(at, vec2<f32>(0.5), vec2<f32>(params.source.zw) - vec2<f32>(0.5));
        }
        sum += sweep_read(source_tex, params.source.xy, at);
    }
    return sum / f32(params.header.x);
}

@compute @workgroup_size(8, 8, 1)
fn boxes_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(boxes_out);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pos = vec2<f32>(id.xy) + vec2<f32>(0.5) + vec2<f32>(params.origins.xy);
    textureStore(boxes_out, vec2<i32>(id.xy), sweep_box(pos));
}

// The comb of the stored short boxes, which their bilinear reads soften, crossfaded with the
// short box taken directly at each of the comb's moves.
@compute @workgroup_size(8, 8, 1)
fn comb_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pos = vec2<f32>(id.xy) + vec2<f32>(0.5) + vec2<f32>(params.origins.zw);
    let blend = params.blend.x;
    var stored = vec4<f32>(0.0);
    var direct = vec4<f32>(0.0);
    for (var index = 0u; index < params.header.z; index = index + 1u) {
        let at = sweep_move(params.comb[index], pos);
        if (blend > 0.0) {
            stored += sweep_read(boxes_tex, params.origins.xy, at);
        }
        if (blend < 1.0) {
            direct += sweep_box(at);
        }
    }
    textureStore(output_tex, vec2<i32>(id.xy), mix(direct, stored, blend) / f32(params.header.z));
}

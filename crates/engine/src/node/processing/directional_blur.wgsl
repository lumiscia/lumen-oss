// One pass of a blur along a path: each texel averages its input at the taps' moves of it.

struct SweepTap {
    // `M` as `(m00, m01, m10, m11)`: a tap reads `M p + t.xy` for canvas position `p`.
    m: vec4<f32>,
    t: vec4<f32>,
}

struct SweepParams {
    // `x`: tap count, `y`: 1 when reads clamp to the canvas.
    header: vec4<u32>,
    // Canvas pixel of the input's first texel in `xy`, and the canvas size in `zw`.
    input: vec4<i32>,
    // Canvas pixel of the output's first texel in `xy`.
    output: vec4<i32>,
    taps: array<SweepTap, 64>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: SweepParams;
@group(0) @binding(2) var linear_sampler: sampler;
@group(0) @binding(3) var inner_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba8unorm, write>;

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

// The input at canvas position `pos`, transparent where it stores nothing. Scaling the sampler's
// clamped read by the footprint's coverage reads the texels outside as transparent.
fn sweep_read(pos: vec2<f32>) -> vec4<f32> {
    var at = pos;
    if (params.header.y == 1u) {
        at = clamp(at, vec2<f32>(0.5), vec2<f32>(params.input.zw) - vec2<f32>(0.5));
    }
    let local = at - vec2<f32>(params.input.xy);
    let dims = vec2<f32>(textureDimensions(input_tex));
    return textureSampleLevel(input_tex, linear_sampler, local / dims, 0.0) * sweep_coverage(local, dims);
}

fn sweep(id: vec2<u32>) -> vec4<f32> {
    let pos = vec2<f32>(id) + vec2<f32>(0.5) + vec2<f32>(params.output.xy);
    var sum = vec4<f32>(0.0);
    for (var index = 0u; index < params.header.x; index = index + 1u) {
        let tap = params.taps[index];
        sum += sweep_read(vec2<f32>(dot(tap.m.xy, pos), dot(tap.m.zw, pos)) + tap.t.xy);
    }
    return sum / f32(params.header.x);
}

@compute @workgroup_size(8, 8, 1)
fn inner_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(inner_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    textureStore(inner_tex, vec2<i32>(id.xy), sweep(id.xy));
}

@compute @workgroup_size(8, 8, 1)
fn outer_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    textureStore(output_tex, vec2<i32>(id.xy), sweep(id.xy));
}

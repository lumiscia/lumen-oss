// Shared separable Gaussian blur. Each pass blurs along one axis with
// linear-sampled tap pairs: two adjacent Gaussian weights collapse into one
// bilinear fetch, halving the texture reads.
//
// The kernel is truncated at three sigma, so a pass costs about `3 * sigma`
// taps. To keep that bounded, sigma above 20 is blurred on a copy reduced 4x
// per axis and sigma above 80 on one reduced 16x, then sampled back bilinearly.
// Every entry point is always dispatched; the ones the current sigma does not
// need return immediately.
//
// Positions are in texel units where texel `i` has its center at `i + 0.5`.

// `values`: x sigma, y straight-alpha source, z transparent edges.
// `offset.xy`: whole-pixel shift of the source.
struct GaussianParams {
    values: vec4<f32>,
    offset: vec4<f32>,
}

@group(0) @binding(0) var tex_a: texture_2d<f32>;
@group(0) @binding(1) var tex_b: texture_2d<f32>;
@group(0) @binding(2) var tex_c: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: GaussianParams;
@group(0) @binding(4) var linear_sampler: sampler;
@group(0) @binding(5) var out_float: texture_storage_2d<rgba16float, write>;
@group(0) @binding(6) var out_unorm: texture_storage_2d<rgba8unorm, write>;

const GAUSSIAN_MIN_SIGMA: f32 = 0.001;
const GAUSSIAN_EXTENT_SIGMAS: f32 = 3.0;
const GAUSSIAN_MIN_PAIR_WEIGHT: f32 = 1e-30;
const GAUSSIAN_QUARTER_FROM_SIGMA: f32 = 20.0;
const GAUSSIAN_SIXTEENTH_FROM_SIGMA: f32 = 80.0;
// Variance per axis that box reduction and bilinear sampling add, as a fraction
// of the square of the reduction factor.
const GAUSSIAN_RESAMPLE_VARIANCE: f32 = 0.25;

fn gaussian_edge_inside(index: f32, extent: f32) -> f32 {
    return select(0.0, 1.0, index >= 0.0 && index < extent);
}

// Fraction of a bilinear footprint at `pos` that lies inside the texture.
fn gaussian_coverage(pos: vec2<f32>, dims: vec2<f32>) -> f32 {
    let p = pos - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    let x = mix(gaussian_edge_inside(base.x, dims.x), gaussian_edge_inside(base.x + 1.0, dims.x), f.x);
    let y = mix(gaussian_edge_inside(base.y, dims.y), gaussian_edge_inside(base.y + 1.0, dims.y), f.y);
    return x * y;
}

fn gaussian_load_premultiplied(tex: texture_2d<f32>, coord: vec2<f32>, dims: vec2<f32>) -> vec4<f32> {
    let clamped = clamp(coord, vec2<f32>(0.0), dims - vec2<f32>(1.0));
    return premultiply(textureLoad(tex, vec2<i32>(clamped), 0));
}

// Bilinear fetch. Straight-alpha sources are premultiplied before
// interpolating, so transparent texels do not drag their color into the edge.
// Texels outside the texture clamp to the edge, or read as transparent when
// `transparent_edge` is set.
fn gaussian_tap(
    tex: texture_2d<f32>,
    samp: sampler,
    pos: vec2<f32>,
    straight: bool,
    transparent_edge: bool,
) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex));
    var color: vec4<f32>;
    if (straight) {
        let p = pos - vec2<f32>(0.5);
        let base = floor(p);
        let f = p - base;
        let top = mix(
            gaussian_load_premultiplied(tex, base, dims),
            gaussian_load_premultiplied(tex, base + vec2<f32>(1.0, 0.0), dims),
            f.x,
        );
        let bottom = mix(
            gaussian_load_premultiplied(tex, base + vec2<f32>(0.0, 1.0), dims),
            gaussian_load_premultiplied(tex, base + vec2<f32>(1.0, 1.0), dims),
            f.x,
        );
        color = mix(top, bottom, f.y);
    } else {
        color = textureSampleLevel(tex, samp, pos / dims, 0.0);
    }
    if (transparent_edge) {
        color = color * gaussian_coverage(pos, dims);
    }
    return color;
}

// One normalized Gaussian pass of standard deviation `sigma` along `axis`,
// truncated at three sigma. The result is premultiplied when the input is.
fn gaussian_blur_1d(
    tex: texture_2d<f32>,
    samp: sampler,
    center: vec2<f32>,
    axis: vec2<f32>,
    sigma: f32,
    straight: bool,
    transparent_edge: bool,
) -> vec4<f32> {
    var acc = gaussian_tap(tex, samp, center, straight, transparent_edge);
    if (sigma < GAUSSIAN_MIN_SIGMA) {
        return acc;
    }

    let falloff = -0.5 / (sigma * sigma);
    let extent = ceil(sigma * GAUSSIAN_EXTENT_SIGMAS);
    var total = 1.0;
    for (var i = 1.0; i <= extent; i = i + 2.0) {
        let w0 = exp(falloff * i * i);
        let w1 = exp(falloff * (i + 1.0) * (i + 1.0));
        let weight = w0 + w1;
        if (weight < GAUSSIAN_MIN_PAIR_WEIGHT) {
            break;
        }
        let offset = axis * ((i * w0 + (i + 1.0) * w1) / weight);
        acc = acc + weight * (
            gaussian_tap(tex, samp, center + offset, straight, transparent_edge)
            + gaussian_tap(tex, samp, center - offset, straight, transparent_edge)
        );
        total = total + 2.0 * weight;
    }
    return acc / total;
}

// 0 blurs at full resolution, 1 at quarter and 2 at sixteenth.
fn gaussian_level(sigma: f32) -> u32 {
    if (sigma <= GAUSSIAN_QUARTER_FROM_SIGMA) {
        return 0u;
    }
    if (sigma <= GAUSSIAN_SIXTEENTH_FROM_SIGMA) {
        return 1u;
    }
    return 2u;
}

// Sigma to blur a reduced copy with so the full-resolution result has `sigma`,
// after subtracting the blur the resampling itself adds.
fn gaussian_level_sigma(sigma: f32, level: u32) -> f32 {
    let factor = select(16.0, 4.0, level == 1u);
    let remaining = sigma * sigma - GAUSSIAN_RESAMPLE_VARIANCE * factor * factor;
    return sqrt(max(remaining, 0.0)) / factor;
}

fn gaussian_edge() -> bool {
    return params.values.z >= 0.5;
}

@compute @workgroup_size(8, 8, 1)
fn horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(out_float);
    let sigma = params.values.x;
    if (id.x >= size.x || id.y >= size.y || gaussian_level(sigma) != 0u) {
        return;
    }

    let center = vec2<f32>(id.xy) + vec2<f32>(0.5) - params.offset.xy;
    let color = gaussian_blur_1d(tex_a, linear_sampler, center, vec2<f32>(1.0, 0.0), sigma, params.values.y >= 0.5, gaussian_edge());
    textureStore(out_float, vec2<i32>(id.xy), color);
}

// Averages the 4x4 block under each output texel with four bilinear taps.
fn gaussian_reduce(id: vec3<u32>, tex: texture_2d<f32>, shift: vec2<f32>, straight: bool) {
    let size = textureDimensions(out_float);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let center = (vec2<f32>(id.xy) + vec2<f32>(0.5)) * 4.0 - shift;
    let edge = gaussian_edge();
    var color = gaussian_tap(tex, linear_sampler, center + vec2<f32>(-1.0, -1.0), straight, edge);
    color = color + gaussian_tap(tex, linear_sampler, center + vec2<f32>(1.0, -1.0), straight, edge);
    color = color + gaussian_tap(tex, linear_sampler, center + vec2<f32>(-1.0, 1.0), straight, edge);
    color = color + gaussian_tap(tex, linear_sampler, center + vec2<f32>(1.0, 1.0), straight, edge);
    textureStore(out_float, vec2<i32>(id.xy), color * 0.25);
}

fn gaussian_reduced_blur(id: vec3<u32>, tex: texture_2d<f32>, axis: vec2<f32>, level: u32) {
    let size = textureDimensions(out_float);
    let sigma = params.values.x;
    if (id.x >= size.x || id.y >= size.y || gaussian_level(sigma) != level) {
        return;
    }

    let center = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let color = gaussian_blur_1d(tex, linear_sampler, center, axis, gaussian_level_sigma(sigma, level), false, gaussian_edge());
    textureStore(out_float, vec2<i32>(id.xy), color);
}

@compute @workgroup_size(8, 8, 1)
fn quarter_down_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (gaussian_level(params.values.x) >= 1u) {
        gaussian_reduce(id, tex_a, params.offset.xy, params.values.y >= 0.5);
    }
}

@compute @workgroup_size(8, 8, 1)
fn quarter_horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, tex_a, vec2<f32>(1.0, 0.0), 1u);
}

@compute @workgroup_size(8, 8, 1)
fn quarter_vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, tex_a, vec2<f32>(0.0, 1.0), 1u);
}

@compute @workgroup_size(8, 8, 1)
fn sixteenth_down_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (gaussian_level(params.values.x) == 2u) {
        gaussian_reduce(id, tex_a, vec2<f32>(0.0), false);
    }
}

@compute @workgroup_size(8, 8, 1)
fn sixteenth_horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, tex_a, vec2<f32>(1.0, 0.0), 2u);
}

@compute @workgroup_size(8, 8, 1)
fn sixteenth_vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, tex_a, vec2<f32>(0.0, 1.0), 2u);
}

// Final full-resolution pass: the vertical blur of `tex_a` at level 0, or a
// bilinear upsample of the reduced result in `tex_b` / `tex_c` otherwise.
@compute @workgroup_size(8, 8, 1)
fn resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(out_unorm);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let sigma = params.values.x;
    let level = gaussian_level(sigma);
    let center = vec2<f32>(id.xy) + vec2<f32>(0.5);
    var color: vec4<f32>;
    if (level == 0u) {
        color = gaussian_blur_1d(tex_a, linear_sampler, center, vec2<f32>(0.0, 1.0), sigma, false, gaussian_edge());
    } else if (level == 1u) {
        color = textureSampleLevel(tex_b, linear_sampler, center / (4.0 * vec2<f32>(textureDimensions(tex_b))), 0.0);
    } else {
        color = textureSampleLevel(tex_c, linear_sampler, center / (16.0 * vec2<f32>(textureDimensions(tex_c))), 0.0);
    }
    textureStore(out_unorm, vec2<i32>(id.xy), color);
}

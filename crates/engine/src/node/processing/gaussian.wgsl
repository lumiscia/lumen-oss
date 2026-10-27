// Shared separable Gaussian blur. Each pass blurs along one axis with
// linear-sampled tap pairs: two adjacent Gaussian weights collapse into one
// bilinear fetch, halving the texture reads.
//
// The kernel is truncated at three sigma, so a pass costs about `3 * sigma`
// taps. To keep that bounded, larger sigmas are blurred on a copy reduced 4x
// per axis (quarter) or 16x (sixteenth), then sampled back bilinearly. Around
// each switch both levels are evaluated and crossfaded, so the output is
// continuous in sigma. Every entry point is always dispatched; the ones the
// current sigma does not need return immediately.
//
// Reduced copies and the full-resolution horizontal result are padded past the
// canvas on every side. Padding is reduced from the same shifted, edge-extended
// source as the canvas, so content outside the canvas keeps contributing.
//
// The constants below come from a header `gaussian.rs` prepends: the sigma bands
// (`GAUSSIAN_QUARTER_FROM`, `GAUSSIAN_QUARTER_FULL`, `GAUSSIAN_SIXTEENTH_FROM`,
// `GAUSSIAN_SIXTEENTH_FULL`) and the padding in texels of each level
// (`GAUSSIAN_FULL_PAD`, `GAUSSIAN_QUARTER_PAD`, `GAUSSIAN_SIXTEENTH_PAD`).
//
// The source may store only part of the canvas, with every canvas pixel outside it
// transparent, and the result likewise covers only a region. Reads of the source are made in
// canvas pixels and resolve through its origin, so they treat pixels outside the stored part
// as transparent and clamp to (or, with `transparent_edge`, leave) the canvas as a full-frame
// source would.
//
// Positions are in texel units where texel `i` has its center at `i + 0.5`. A reduced texel
// `i` covers canvas pixels `(first + i - pad) * factor ..`, where `first` is the output's
// origin in blocks of `factor`, so reduced blocks stay aligned to the canvas.

// `values`: x sigma, y straight-alpha source, z transparent edges.
// `offset.xy`: whole-pixel shift of the source.
// `region.xy`: canvas pixel of the output's first texel.
// `source.xy`: canvas pixel of the source's first texel, `source.zw`: its display size.
struct GaussianParams {
    values: vec4<f32>,
    offset: vec4<f32>,
    region: vec4<i32>,
    source: vec4<i32>,
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

// The source texel at canvas pixel `coord`: clamped to the canvas, then transparent where the
// source stores nothing.
fn gaussian_source_texel(tex: texture_2d<f32>, coord: vec2<f32>, straight: bool) -> vec4<f32> {
    let display = vec2<f32>(params.source.zw);
    let local = clamp(coord, vec2<f32>(0.0), display - vec2<f32>(1.0)) - vec2<f32>(params.source.xy);
    let size = vec2<f32>(textureDimensions(tex));
    if (any(local < vec2<f32>(0.0)) || any(local >= size)) {
        return vec4<f32>(0.0);
    }
    let color = textureLoad(tex, vec2<i32>(local), 0);
    return select(color, premultiply(color), straight);
}

// `gaussian_tap` for the source, at canvas position `pos`. Where the bilinear footprint stays
// within the stored texels, or leaves them only through an edge of the canvas that the
// sampler's clamp reproduces, the sampler is used. Elsewhere the texels are blended by hand.
fn gaussian_source_tap(
    tex: texture_2d<f32>,
    samp: sampler,
    pos: vec2<f32>,
    straight: bool,
    transparent_edge: bool,
) -> vec4<f32> {
    let p = pos - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    let origin = vec2<f32>(params.source.xy);
    let size = vec2<f32>(textureDimensions(tex));
    let local = base - origin;
    let at_low_edge = origin <= vec2<f32>(0.0);
    let at_high_edge = origin + size >= vec2<f32>(params.source.zw);
    let inside_low = (local >= vec2<f32>(0.0)) | at_low_edge;
    let inside_high = (local + vec2<f32>(1.0) < size) | at_high_edge;
    var color: vec4<f32>;
    if (!straight && all(inside_low & inside_high)) {
        color = textureSampleLevel(tex, samp, (pos - origin) / size, 0.0);
    } else {
        let top = mix(
            gaussian_source_texel(tex, base, straight),
            gaussian_source_texel(tex, base + vec2<f32>(1.0, 0.0), straight),
            f.x,
        );
        let bottom = mix(
            gaussian_source_texel(tex, base + vec2<f32>(0.0, 1.0), straight),
            gaussian_source_texel(tex, base + vec2<f32>(1.0, 1.0), straight),
            f.x,
        );
        color = mix(top, bottom, f.y);
    }
    if (transparent_edge) {
        color = color * gaussian_coverage(pos, vec2<f32>(params.source.zw));
    }
    return color;
}

fn gaussian_fetch(
    tex: texture_2d<f32>,
    samp: sampler,
    pos: vec2<f32>,
    straight: bool,
    transparent_edge: bool,
    from_source: bool,
) -> vec4<f32> {
    if (from_source) {
        return gaussian_source_tap(tex, samp, pos, straight, transparent_edge);
    }
    return gaussian_tap(tex, samp, pos, straight, transparent_edge);
}

// One normalized Gaussian pass of standard deviation `sigma` along `axis`,
// truncated at three sigma. The result is premultiplied when the input is. `from_source` reads
// `tex` as the source, at canvas positions.
fn gaussian_blur_1d(
    tex: texture_2d<f32>,
    samp: sampler,
    center: vec2<f32>,
    axis: vec2<f32>,
    sigma: f32,
    straight: bool,
    transparent_edge: bool,
    from_source: bool,
) -> vec4<f32> {
    var acc = gaussian_fetch(tex, samp, center, straight, transparent_edge, from_source);
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
            gaussian_fetch(tex, samp, center + offset, straight, transparent_edge, from_source)
            + gaussian_fetch(tex, samp, center - offset, straight, transparent_edge, from_source)
        );
        total = total + 2.0 * weight;
    }
    return acc / total;
}

// Sigma to blur a reduced copy with so the full-resolution result has `sigma`,
// after subtracting the blur the resampling itself adds.
fn gaussian_level_sigma(sigma: f32, factor: f32) -> f32 {
    let remaining = sigma * sigma - GAUSSIAN_RESAMPLE_VARIANCE * factor * factor;
    return sqrt(max(remaining, 0.0)) / factor;
}

fn gaussian_uses_full(sigma: f32) -> bool {
    return sigma < GAUSSIAN_QUARTER_FULL;
}

fn gaussian_uses_quarter(sigma: f32) -> bool {
    return sigma > GAUSSIAN_QUARTER_FROM && sigma < GAUSSIAN_SIXTEENTH_FULL;
}

fn gaussian_uses_sixteenth(sigma: f32) -> bool {
    return sigma > GAUSSIAN_SIXTEENTH_FROM;
}

fn gaussian_edge() -> bool {
    return params.values.z >= 0.5;
}

// Blurs the source along x for every padded row of the horizontal texture.
@compute @workgroup_size(8, 8, 1)
fn horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(out_float);
    let sigma = params.values.x;
    if (id.x >= size.x || id.y >= size.y || !gaussian_uses_full(sigma)) {
        return;
    }

    let canvas = vec2<f32>(id.xy) + vec2<f32>(params.region.xy) + vec2<f32>(0.5, 0.5 - GAUSSIAN_FULL_PAD);
    let color = gaussian_blur_1d(tex_a, linear_sampler, canvas - params.offset.xy, vec2<f32>(1.0, 0.0), sigma, params.values.y >= 0.5, gaussian_edge(), true);
    textureStore(out_float, vec2<i32>(id.xy), color);
}

// The canvas block of `factor` pixels that the output's first pixel is in.
fn gaussian_first_block(factor: f32) -> vec2<f32> {
    return vec2<f32>(params.region.xy / vec2<i32>(i32(factor)));
}

// Averages the `factor` x `factor` canvas block under each padded output texel
// of the shifted source with bilinear taps that each cover 2x2 texels.
fn gaussian_reduce(id: vec3<u32>, factor: f32, pad: f32) {
    let size = textureDimensions(out_float);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let center = (vec2<f32>(id.xy) + gaussian_first_block(factor) + vec2<f32>(0.5 - pad)) * factor - params.offset.xy;
    let straight = params.values.y >= 0.5;
    let edge = gaussian_edge();
    let taps = i32(factor) / 2;
    var color = vec4<f32>(0.0);
    for (var y = 0; y < taps; y = y + 1) {
        for (var x = 0; x < taps; x = x + 1) {
            let delta = vec2<f32>(f32(2 * x - (taps - 1)), f32(2 * y - (taps - 1)));
            color = color + gaussian_source_tap(tex_a, linear_sampler, center + delta, straight, edge);
        }
    }
    textureStore(out_float, vec2<i32>(id.xy), color / f32(taps * taps));
}

fn gaussian_reduced_blur(id: vec3<u32>, axis: vec2<f32>, factor: f32, used: bool) {
    let size = textureDimensions(out_float);
    if (id.x >= size.x || id.y >= size.y || !used) {
        return;
    }

    let center = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let color = gaussian_blur_1d(tex_a, linear_sampler, center, axis, gaussian_level_sigma(params.values.x, factor), false, gaussian_edge(), false);
    textureStore(out_float, vec2<i32>(id.xy), color);
}

@compute @workgroup_size(8, 8, 1)
fn quarter_down_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (gaussian_uses_quarter(params.values.x)) {
        gaussian_reduce(id, 4.0, GAUSSIAN_QUARTER_PAD);
    }
}

@compute @workgroup_size(8, 8, 1)
fn quarter_horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, vec2<f32>(1.0, 0.0), 4.0, gaussian_uses_quarter(params.values.x));
}

@compute @workgroup_size(8, 8, 1)
fn quarter_vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, vec2<f32>(0.0, 1.0), 4.0, gaussian_uses_quarter(params.values.x));
}

@compute @workgroup_size(8, 8, 1)
fn sixteenth_down_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (gaussian_uses_sixteenth(params.values.x)) {
        gaussian_reduce(id, 16.0, GAUSSIAN_SIXTEENTH_PAD);
    }
}

@compute @workgroup_size(8, 8, 1)
fn sixteenth_horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, vec2<f32>(1.0, 0.0), 16.0, gaussian_uses_sixteenth(params.values.x));
}

@compute @workgroup_size(8, 8, 1)
fn sixteenth_vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    gaussian_reduced_blur(id, vec2<f32>(0.0, 1.0), 16.0, gaussian_uses_sixteenth(params.values.x));
}

// Samples a padded reduced result at canvas position `canvas`.
fn gaussian_sample_reduced(tex: texture_2d<f32>, canvas: vec2<f32>, factor: f32, pad: f32) -> vec4<f32> {
    let texel = canvas / factor + vec2<f32>(pad) - gaussian_first_block(factor);
    return textureSampleLevel(tex, linear_sampler, texel / vec2<f32>(textureDimensions(tex)), 0.0);
}

// Final full-resolution pass: crossfades the vertical blur of the horizontal
// result in `tex_a` with the upsampled quarter (`tex_b`) and sixteenth
// (`tex_c`) results, weighted by how far sigma is into each band.
@compute @workgroup_size(8, 8, 1)
fn resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(out_unorm);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let sigma = params.values.x;
    let local = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let canvas = local + vec2<f32>(params.region.xy);
    let reduced = smoothstep(GAUSSIAN_QUARTER_FROM, GAUSSIAN_QUARTER_FULL, sigma);
    let sixteenth = smoothstep(GAUSSIAN_SIXTEENTH_FROM, GAUSSIAN_SIXTEENTH_FULL, sigma);
    var color = vec4<f32>(0.0);
    if (gaussian_uses_full(sigma)) {
        let center = local + vec2<f32>(0.0, GAUSSIAN_FULL_PAD);
        color = (1.0 - reduced) * gaussian_blur_1d(tex_a, linear_sampler, center, vec2<f32>(0.0, 1.0), sigma, false, gaussian_edge(), false);
    }
    if (gaussian_uses_quarter(sigma)) {
        color = color + reduced * (1.0 - sixteenth) * gaussian_sample_reduced(tex_b, canvas, 4.0, GAUSSIAN_QUARTER_PAD);
    }
    if (gaussian_uses_sixteenth(sigma)) {
        color = color + reduced * sixteenth * gaussian_sample_reduced(tex_c, canvas, 16.0, GAUSSIAN_SIXTEENTH_PAD);
    }
    textureStore(out_unorm, vec2<i32>(id.xy), color);
}

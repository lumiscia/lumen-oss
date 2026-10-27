struct TransformTap {
    scale: vec2<f32>,
    translate: vec2<f32>,
    pivot: vec2<f32>,
    rotate_radians: f32,
    _pad: u32,
}

// With taps, the transform moves during the shutter and the output averages the source
// sampled under each. The array length is `MAX_MOTION_TAPS`.
struct TransformParams {
    scale: vec2<f32>,
    translate: vec2<f32>,
    pivot: vec2<f32>,
    rotate_radians: f32,
    sampling: u32,
    tap_count: u32,
    straight_source: u32,
    // Canvas pixel of the output's first texel, and source pixel of the source's.
    output_origin: vec2<i32>,
    source_origin: vec2<i32>,
    _pad0: u32,
    _pad1: u32,
    taps: array<TransformTap, 32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: TransformParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

// A pixel of the source, premultiplied first when `straight`, so that filtering weights
// colour by coverage.
fn read_pixel(pixel: vec2<i32>, straight: bool) -> vec4<f32> {
    let size = textureDimensions(input_tex);
    let coord = pixel - params.source_origin;
    if (coord.x < 0 || coord.y < 0 || coord.x >= i32(size.x) || coord.y >= i32(size.y)) {
        return vec4<f32>(0.0);
    }
    return to_premultiplied_alpha(textureLoad(input_tex, coord, 0), straight);
}

// Sample coordinates are continuous, with pixel centres at integer + 0.5.
fn sample_nearest(coord: vec2<f32>, straight: bool) -> vec4<f32> {
    return read_pixel(vec2<i32>(floor(coord)), straight);
}

fn sample_linear(coord: vec2<f32>, straight: bool) -> vec4<f32> {
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = read_pixel(p0, straight);
    let c10 = read_pixel(p0 + vec2<i32>(1, 0), straight);
    let c01 = read_pixel(p0 + vec2<i32>(0, 1), straight);
    let c11 = read_pixel(p0 + vec2<i32>(1, 1), straight);
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

// The source pixel shown at output `pixel`.
fn source_coord(
    pixel: vec2<f32>,
    scale_param: vec2<f32>,
    translate: vec2<f32>,
    pivot: vec2<f32>,
    rotate_radians: f32,
) -> vec2<f32> {
    let scale = select(scale_param, vec2<f32>(0.000001), abs(scale_param) <= vec2<f32>(0.000001));
    let centered = pixel - pivot - translate;
    let c = cos(-rotate_radians);
    let s = sin(-rotate_radians);
    let unrotated = vec2<f32>(
        centered.x * c - centered.y * s,
        centered.x * s + centered.y * c,
    );
    return (unrotated / scale) + pivot;
}

// Samples the source as stored, or premultiplied when `straight`.
fn sample_source(coord: vec2<f32>, straight: bool) -> vec4<f32> {
    if (params.sampling == 0u) {
        return sample_nearest(coord, straight);
    }
    return sample_linear(coord, straight);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = vec2<f32>(vec2<i32>(id.xy) + params.output_origin) + vec2<f32>(0.5);
    var color = vec4<f32>(0.0);
    if (params.tap_count == 0u) {
        color = sample_source(
            source_coord(pixel, params.scale, params.translate, params.pivot, params.rotate_radians),
            false,
        );
    } else {
        // Straight alpha is premultiplied per texel before filtering and averaging, so
        // transparent texels and taps add no colour.
        let straight = params.straight_source != 0u;
        for (var index = 0u; index < params.tap_count; index = index + 1u) {
            let tap = params.taps[index];
            color += sample_source(
                source_coord(pixel, tap.scale, tap.translate, tap.pivot, tap.rotate_radians),
                straight,
            );
        }
        color /= f32(params.tap_count);
        if (straight) {
            color = unpremultiply(color);
        }
    }
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

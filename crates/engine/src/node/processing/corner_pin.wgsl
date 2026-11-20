// One position of the quad over the shutter.
struct CornerPinTap {
    // The rows of the matrix that takes a canvas pixel `(x, y, 1)` to the source point
    // `(a, b) / c` for the product `(a, b, c)`, in `xyz`.
    rows: array<vec4<f32>, 3>,
    // The lines of the quad's sides, each `(nx, ny, c)` with `nx * x + ny * y + c` the distance
    // inside the side in pixels.
    edges: array<vec4<f32>, 4>,
}

// With more than one tap, the quad moves during the shutter and the output averages the source
// pinned to each. The array length is `MAX_MOTION_TAPS`.
struct CornerPinParams {
    sampling: u32,
    tap_count: u32,
    // Canvas pixel of the output's first texel, and source pixel of the source's.
    output_origin: vec2<i32>,
    source_origin: vec2<i32>,
    _pad: vec2<u32>,
    taps: array<CornerPinTap, 32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: CornerPinParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

// A pixel of the source, so that filtering weights colour by coverage.
fn read_pixel(pixel: vec2<i32>) -> vec4<f32> {
    let size = textureDimensions(input_tex);
    let coord = pixel - params.source_origin;
    if (coord.x < 0 || coord.y < 0 || coord.x >= i32(size.x) || coord.y >= i32(size.y)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(input_tex, coord, 0);
}

// Sample coordinates are continuous, with pixel centres at integer + 0.5.
fn sample_nearest(coord: vec2<f32>) -> vec4<f32> {
    return read_pixel(vec2<i32>(floor(coord)));
}

fn sample_linear(coord: vec2<f32>) -> vec4<f32> {
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = read_pixel(p0);
    let c10 = read_pixel(p0 + vec2<i32>(1, 0));
    let c01 = read_pixel(p0 + vec2<i32>(0, 1));
    let c11 = read_pixel(p0 + vec2<i32>(1, 1));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

// A texel of a mip level of the source, which holds the source's pixels only; outside it is clear.
fn read_level_pixel(coord: vec2<i32>, level: u32, size: vec2<i32>) -> vec4<f32> {
    if (any(coord < vec2<i32>(0)) || any(coord >= size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(input_tex, coord, level);
}

// How much of the pixel at `pixel` the quad covers, from its distance to the nearest side.
fn coverage(tap: CornerPinTap, pixel: vec2<f32>) -> f32 {
    var nearest = 1.0e30;
    for (var side = 0u; side < 4u; side = side + 1u) {
        let edge = tap.edges[side];
        nearest = min(nearest, dot(edge.xy, pixel) + edge.z);
    }
    return clamp(0.5 + nearest, 0.0, 1.0);
}

// The source pinned to one quad, at the pixel centred on `pixel`.
fn sample_tap(tap: CornerPinTap, pixel: vec2<f32>) -> vec4<f32> {
    let covered = coverage(tap, pixel);
    if (covered <= 0.0) {
        return vec4<f32>(0.0);
    }
    let at = vec3<f32>(pixel, 1.0);
    let divisor = dot(tap.rows[2].xyz, at);
    if (!(divisor > 0.0)) {
        return vec4<f32>(0.0);
    }
    let coord = vec2<f32>(dot(tap.rows[0].xyz, at), dot(tap.rows[1].xyz, at)) / divisor;
    if (params.sampling == 0u) {
        return sample_nearest(coord) * covered;
    }
    // The texels of the source that the pixel covers along its axes, from the Jacobian of the map
    // from the canvas to the source, which perspective varies across the quad.
    let along_x = (vec2<f32>(tap.rows[0].x, tap.rows[1].x) - coord * tap.rows[2].x) / divisor;
    let along_y = (vec2<f32>(tap.rows[0].y, tap.rows[1].y) - coord * tap.rows[2].y) / divisor;
    let footprint = vec2<f32>(length(vec2<f32>(along_x.x, along_y.x)), length(vec2<f32>(along_x.y, along_y.y)));
    if (max(footprint.x, footprint.y) > 1.0) {
        return minify(coord - vec2<f32>(params.source_origin), footprint, minify_lod(footprint)) * covered;
    }
    return sample_linear(coord) * covered;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = vec2<f32>(vec2<i32>(id.xy) + params.output_origin) + vec2<f32>(0.5);
    var color = vec4<f32>(0.0);
    for (var index = 0u; index < params.tap_count; index = index + 1u) {
        color += sample_tap(params.taps[index], pixel);
    }
    color /= f32(params.tap_count);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

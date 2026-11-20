struct ErodeDilateParams {
    // `x`: radius magnitude, `y`: softness, `z`: 1 for the square shape, in device pixels.
    values: vec4<f32>,
    // `xy`: source texel minus output texel, `z`: jump step, `w`: 1 when eroding.
    offset: vec4<i32>,
    // 1 on each side (left, top, right, bottom) past which the source reads as transparent.
    edges: vec4<i32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var seeds_tex: texture_2d<u32>;
@group(0) @binding(2) var<uniform> params: ErodeDilateParams;
@group(0) @binding(3) var seeds_out: texture_storage_2d<rg32uint, write>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba16float, write>;

const NO_SEED: u32 = 0xffffffffu;
const FAR: f32 = 1e30;

fn source_at(pixel: vec2<i32>) -> vec4<f32> {
    let at = pixel + params.offset.xy;
    if (any(at < vec2<i32>(0)) || any(at >= vec2<i32>(textureDimensions(source_tex)))) {
        return vec4<f32>(0.0);
    }
    return textureLoad(source_tex, at, 0);
}

// Metric between two texels: squared length for the round shape, largest axis for the square.
fn metric(delta: vec2<i32>) -> i32 {
    if (params.values.z > 0.5) {
        return max(abs(delta.x), abs(delta.y));
    }
    return delta.x * delta.x + delta.y * delta.y;
}

fn metric_distance(value: i32) -> f32 {
    if (params.values.z > 0.5) {
        return f32(value);
    }
    return sqrt(f32(value));
}

// Marks each texel that can be copied from, with its own position, and every other one with none.
// A dilation copies from texels that are at least half covered, an erosion from the ones that are
// less so.
@compute @workgroup_size(8, 8, 1)
fn seed_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(seeds_out);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy);
    let covered = source_at(pixel).a >= 0.5;
    let seed = covered != (params.offset.w == 1);
    textureStore(seeds_out, pixel, vec4<u32>(select(NO_SEED, id.x, seed), id.y, 0u, 0u));
}

// One jump flooding pass: takes the nearest seed among the texels `step` away in each direction.
@compute @workgroup_size(8, 8, 1)
fn jump_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<i32>(textureDimensions(seeds_out));
    if (id.x >= u32(size.x) || id.y >= u32(size.y)) {
        return;
    }
    let pixel = vec2<i32>(id.xy);
    var best = textureLoad(seeds_tex, pixel, 0).xy;
    var best_metric = 0x7fffffff;
    if (best.x != NO_SEED) {
        best_metric = metric(pixel - vec2<i32>(best));
    }
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let at = pixel + vec2<i32>(dx, dy) * params.offset.z;
            if (any(at < vec2<i32>(0)) || any(at >= size)) {
                continue;
            }
            let seed = textureLoad(seeds_tex, at, 0).xy;
            if (seed.x == NO_SEED) {
                continue;
            }
            let value = metric(pixel - vec2<i32>(seed));
            if (value < best_metric) {
                best_metric = value;
                best = seed;
            }
        }
    }
    textureStore(seeds_out, pixel, vec4<u32>(best, 0u, 0u));
}

// Coverage ramp over `softness` plus the pixel's own width, centred on `radius`, for a pixel whose
// centre is `edge` from the edge of the source's shape.
fn ramp(edge: f32) -> f32 {
    let spread = params.values.y * 0.5 + 0.5;
    return smoothstep(params.values.x - spread, params.values.x + spread, edge);
}

@compute @workgroup_size(8, 8, 1)
fn resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy);
    let own = source_at(pixel);
    let seed = textureLoad(seeds_tex, pixel, 0).xy;
    var edge = FAR;
    if (seed.x != NO_SEED) {
        // The seed is a texel centre, so the shape's edge is half a texel nearer.
        edge = metric_distance(metric(pixel - vec2<i32>(seed))) - 0.5;
    }

    if (params.offset.w == 1) {
        // Past an edge of the data that is inside the canvas, the source is transparent.
        let grid = vec2<i32>(size);
        var nearest = vec4<i32>(pixel.x + 1, pixel.y + 1, grid.x - pixel.x, grid.y - pixel.y);
        nearest = select(vec4<i32>(0x7fffffff), nearest, params.edges == vec4<i32>(1));
        edge = min(edge, f32(min(min(nearest.x, nearest.y), min(nearest.z, nearest.w))) - 0.5);
        let kept = min(own.a, ramp(edge));
        var eroded = own;
        if (own.a > 0.0) {
            eroded = own * (kept / own.a);
        }
        textureStore(output_tex, pixel, eroded);
        return;
    }

    // The shape itself stays fully covered; softness only feathers outward from it.
    var coverage = select(0.0, 1.0 - ramp(edge), seed.x != NO_SEED);
    if (own.a >= 0.5) {
        coverage = 1.0;
    }
    if (own.a >= coverage) {
        textureStore(output_tex, pixel, own);
        return;
    }
    let colour = textureLoad(source_tex, vec2<i32>(seed) + params.offset.xy, 0);
    textureStore(output_tex, pixel, vec4<f32>(colour.rgb / colour.a * coverage, coverage));
}

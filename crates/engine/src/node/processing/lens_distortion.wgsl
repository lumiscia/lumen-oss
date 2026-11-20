struct LensParams {
    // The radial coefficients `k1` and `k2`, the chromatic aberration, and the fit scale.
    lens: vec4<f32>,
    // The optical centre in canvas pixels in `xy`, and the frame's half-diagonal in pixels in `z`.
    frame: vec4<f32>,
    // The direction (0 distorts, 1 undistorts) and the edge mode (0 clamps, 1 is transparent).
    modes: vec4<u32>,
    // Canvas pixels of the first texels of the output in `xy` and the source in `zw`.
    origins: vec4<i32>,
    // The source's display size in `xy`.
    source: vec4<i32>,
    // Where the lens folds back on itself, if `z` is 1: the radius in `x` and the radius it moves
    // to in `y`. Only radii below `y` undistort.
    branch: vec4<f32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: LensParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

const NEWTON_ITERATIONS: i32 = 32;
const NEWTON_TOLERANCE: f32 = 1e-6;
// Where the inverse lens has no solution, pixels read this many times their own distance, far
// enough out for the edge mode to decide what they show.
const FOLDED_FACTOR: f32 = 1e4;
// Minification past which a pixel is averaged over four subsamples.
const MINIFY_LIMIT: f32 = 1.5;

fn lens_radius(rho: f32) -> f32 {
    let rho2 = rho * rho;
    return rho * (1.0 + params.lens.x * rho2 + params.lens.y * rho2 * rho2);
}

// How a pixel at `p`, in half-diagonals from the optical centre, maps to where it reads. `x` is
// the factor its offset is multiplied by, and `y` the largest factor by which the source is
// stretched or shrunk there, in source pixels per output pixel.
fn lens_map(p: vec2<f32>) -> vec2<f32> {
    let k1 = params.lens.x;
    let k2 = params.lens.y;
    let r2 = dot(p, p);
    if (params.modes.x == 0u) {
        let factor = 1.0 + k1 * r2 + k2 * r2 * r2;
        let slope = 1.0 + 3.0 * k1 * r2 + 5.0 * k2 * r2 * r2;
        return vec2<f32>(factor, max(abs(factor), abs(slope)));
    }
    let r = sqrt(r2);
    if (r <= 0.0) {
        return vec2<f32>(1.0, 1.0);
    }
    var high: f32;
    if (params.branch.z == 1.0) {
        if (r >= params.branch.y) {
            return vec2<f32>(FOLDED_FACTOR, FOLDED_FACTOR);
        }
        high = params.branch.x;
    } else {
        high = max(r, 1.0);
        for (var i = 0; i < 64 && lens_radius(high) < r; i++) {
            high *= 2.0;
        }
    }
    // Newton's method kept inside a bracket on the one-to-one branch, bisecting when a step would
    // leave it.
    var low = 0.0;
    var rho = min(r, high);
    for (var i = 0; i < NEWTON_ITERATIONS; i++) {
        let value = lens_radius(rho) - r;
        if (value > 0.0) {
            high = rho;
        } else {
            low = rho;
        }
        let rho2 = rho * rho;
        let slope = 1.0 + 3.0 * k1 * rho2 + 5.0 * k2 * rho2 * rho2;
        var next = rho - value / slope;
        if (!(next > low && next < high)) {
            next = 0.5 * (low + high);
        }
        let done = abs(next - rho) <= NEWTON_TOLERANCE * abs(next);
        rho = next;
        if (done) {
            break;
        }
    }
    let rho2 = rho * rho;
    let slope = max(1.0 + 3.0 * k1 * rho2 + 5.0 * k2 * rho2 * rho2, 1e-6);
    let factor = rho / r;
    return vec2<f32>(factor, max(abs(factor), 1.0 / slope));
}

fn read_source(pixel: vec2<i32>) -> vec4<f32> {
    return sample_or_transparent(source_tex, pixel, params.origins.zw);
}

// Sample coordinates are continuous canvas pixels, with pixel centres at integer + 0.5.
fn sample_source(position: vec2<f32>) -> vec4<f32> {
    var coord = position;
    if (params.modes.y == 0u) {
        coord = clamp(coord, vec2<f32>(0.5), vec2<f32>(params.source.xy) - vec2<f32>(0.5));
    }
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = read_source(p0);
    let c10 = read_source(p0 + vec2<i32>(1, 0));
    let c01 = read_source(p0 + vec2<i32>(0, 1));
    let c11 = read_source(p0 + vec2<i32>(1, 1));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

// The colour at the canvas position `position`. Each of red and blue reads its own premultiplied
// sample, and alpha is the greatest of the three so that fringes keep their coverage.
fn shade(position: vec2<f32>) -> vec4<f32> {
    let center = params.frame.xy;
    let half_diagonal = params.frame.z;
    let offset = (position - center) / half_diagonal;
    let read = offset * (lens_map(offset).x * params.lens.w);
    let aberration = params.lens.z;
    let green = sample_source(center + read * half_diagonal);
    if (aberration == 0.0) {
        return green;
    }
    let red = sample_source(center + read * (1.0 - aberration) * half_diagonal);
    let blue = sample_source(center + read * (1.0 + aberration) * half_diagonal);
    return vec4<f32>(red.r, green.g, blue.b, max(red.a, max(green.a, blue.a)));
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let position = vec2<f32>(vec2<i32>(id.xy) + params.origins.xy) + vec2<f32>(0.5);
    let offset = (position - params.frame.xy) / params.frame.z;
    let minification = lens_map(offset).y * params.lens.w * (1.0 + abs(params.lens.z));
    var color: vec4<f32>;
    if (minification > MINIFY_LIMIT) {
        // A rotated grid, so that no two subsamples share a row or column.
        color = 0.25 * (
            shade(position + vec2<f32>(0.125, 0.375))
            + shade(position + vec2<f32>(0.375, -0.125))
            + shade(position + vec2<f32>(-0.125, -0.375))
            + shade(position + vec2<f32>(-0.375, 0.125))
        );
    } else {
        color = shade(position);
    }
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

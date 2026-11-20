struct HalftoneParams {
    // Premultiplied linear colours.
    ink: vec4<f32>,
    paper: vec4<f32>,
    // The cell size in device pixels, then the cosine and sine of the grid's angle.
    grid: vec4<f32>,
    // The canvas pixel of the source's first texel in `xy`, then the shape.
    origin_shape: vec4<i32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: HalftoneParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

const HALFTONE_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const HALFTONE_TOUCHING: f32 = 0.7853981633974483;
const HALFTONE_FULL_RADIUS: f32 = 0.7071067811865476;

// The premultiplied colour of the source at a continuous texel coordinate, with texel centres at
// integer + 0.5, blended linearly and held at the edges.
fn sample_linear(coord: vec2<f32>) -> vec4<f32> {
    let last = vec2<i32>(textureDimensions(input_tex)) - vec2<i32>(1);
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = textureLoad(input_tex, clamp(p0, vec2<i32>(0), last), 0);
    let c10 = textureLoad(input_tex, clamp(p0 + vec2<i32>(1, 0), vec2<i32>(0), last), 0);
    let c01 = textureLoad(input_tex, clamp(p0 + vec2<i32>(0, 1), vec2<i32>(0), last), 0);
    let c11 = textureLoad(input_tex, clamp(p0 + vec2<i32>(1, 1), vec2<i32>(0), last), 0);
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

// 1 minus the Rec. 709 luma of the sRGB-encoded straight colour `rgb`.
fn darkness_of(rgb: vec3<f32>) -> f32 {
    let encoded = clamp(lumen_linear_to_srgb(rgb), vec3<f32>(0.0), vec3<f32>(1.0));
    return clamp(1.0 - dot(encoded, HALFTONE_LUMA), 0.0, 1.0);
}

// The fraction of a unit cell that discs of radius `radius` centred on every cell cover, for a
// radius from 0.5, where neighbours touch, to the one that covers the cell.
fn merged_area(radius: f32) -> f32 {
    let lens = 2.0 * radius * radius * acos(0.5 / radius) - 0.5 * sqrt(4.0 * radius * radius - 1.0);
    return 3.141592653589793 * radius * radius - 2.0 * lens;
}

// The radius, in cells, of discs that cover `darkness` of the cell.
fn dot_radius(darkness: f32) -> f32 {
    if (darkness <= HALFTONE_TOUCHING) {
        return sqrt(darkness / 3.141592653589793);
    }
    var low = 0.5;
    var high = HALFTONE_FULL_RADIUS;
    for (var step = 0; step < 16; step++) {
        let middle = 0.5 * (low + high);
        if (merged_area(middle) < darkness) {
            low = middle;
        } else {
            high = middle;
        }
    }
    return 0.5 * (low + high);
}

// How far, in device pixels, a full-darkness mark is grown so that its antialiased edge
// doesn't thin out where marks meet. Zero until darkness is nearly full.
fn solid_margin(darkness: f32) -> f32 {
    return smoothstep(0.9, 1.0, darkness);
}

// The coverage of a mark of `darkness` at `offset` from its cell's centre, in the grid's axes and
// device pixels.
fn mark_coverage(offset: vec2<f32>, darkness: f32, pitch: f32, shape: i32) -> f32 {
    let margin = solid_margin(darkness);
    if (shape == 0) {
        let radius = dot_radius(darkness) * pitch;
        let faint = min(1.0, 4.0 * radius * radius);
        return clamp(0.5 - (length(offset) - radius - margin), 0.0, 1.0) * faint;
    }
    let thickness = darkness * pitch;
    let faint = min(1.0, thickness);
    return clamp(0.5 - (abs(offset.y) - 0.5 * thickness - margin), 0.0, 1.0) * faint;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let own = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    if (own.a <= 0.0) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    let own_straight = unpremultiply(own).rgb;

    let pitch = params.grid.x;
    let axis_x = vec2<f32>(params.grid.y, params.grid.z);
    let axis_y = vec2<f32>(-params.grid.z, params.grid.y);
    let origin = vec2<f32>(params.origin_shape.xy);
    let shape = params.origin_shape.z;
    let position = vec2<f32>(id.xy) + vec2<f32>(0.5) + origin;
    let grid_position = vec2<f32>(dot(position, axis_x), dot(position, axis_y));
    let cell = floor(grid_position / pitch);

    // A line reaches only the cells above and below its own, a dot all eight around it.
    let reach_x = select(1, 0, shape == 1);
    var coverage = 0.0;
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -reach_x; dx <= reach_x; dx++) {
            let centre = (cell + vec2<f32>(f32(dx), f32(dy)) + vec2<f32>(0.5)) * pitch;
            let canvas = centre.x * axis_x + centre.y * axis_y;
            let sampled = sample_linear(canvas - origin);
            var straight = own_straight;
            if (sampled.a > ALPHA_EPSILON) {
                straight = unpremultiply(sampled).rgb;
            }
            let darkness = darkness_of(straight);
            coverage = max(coverage, mark_coverage(grid_position - centre, darkness, pitch, shape));
        }
    }

    let color = mix(params.paper, params.ink, coverage);
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(color * own.a));
}

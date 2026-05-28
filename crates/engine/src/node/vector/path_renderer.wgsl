struct Paint {
    colors: array<vec4<f32>, 8>,
    offsets: array<vec4<f32>, 8>,
    start: vec2<f32>,
    end: vec2<f32>,
    center: vec2<f32>,
    radius: vec2<f32>,
    angle: f32,
    kind: u32,
    units: u32,
    spread: u32,
    interpolation: u32,
    stop_count: u32,
    anti_alias: u32,
    _pad: u32,
}

// TODO: move common shader structs/functions such as Paint into a shared WGSL
// include once the shader assembly path can concatenate common modules.
struct PathParams {
    fill_paint: Paint,
    stroke_paint: Paint,
    position: vec2<f32>,
    bounds_min: vec2<f32>,
    bounds_size: vec2<f32>,
    stroke_width: f32,
    flags: u32,
    point_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

struct PathPoints {
    points: array<vec2<f32>>,
}

@group(0) @binding(0) var<uniform> params: PathParams;
@group(0) @binding(1) var<storage, read> path_points: PathPoints;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn spread_t(paint: Paint, value: f32) -> f32 {
    if (paint.spread == 1u) {
        return fract(value);
    }
    if (paint.spread == 2u) {
        let repeated = fract(value * 0.5) * 2.0;
        return select(repeated, 2.0 - repeated, repeated > 1.0);
    }
    return clamp(value, 0.0, 1.0);
}

fn paint_space_point(paint: Paint, pixel: vec2<f32>, local01: vec2<f32>) -> vec2<f32> {
    return select(local01, pixel, paint.units == 1u);
}

fn interpolate_color(paint: Paint, t_raw: f32) -> vec4<f32> {
    let t = spread_t(paint, t_raw);
    let count = max(paint.stop_count, 1u);
    var color = paint.colors[0];
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (i < count) {
            let left = paint.offsets[i - 1u].x;
            let right = paint.offsets[i].x;
            let span = max(right - left, 0.00001);
            let amount = clamp((t - left) / span, 0.0, 1.0);
            color = select(color, mix(paint.colors[i - 1u], paint.colors[i], amount), t >= left);
        }
    }
    return color;
}

fn sample_paint(paint: Paint, pixel: vec2<f32>, local01: vec2<f32>) -> vec4<f32> {
    let p = paint_space_point(paint, pixel, local01);
    if (paint.kind == 1u) {
        let axis = paint.end - paint.start;
        return interpolate_color(paint, dot(p - paint.start, axis) / max(dot(axis, axis), 0.00001));
    }
    if (paint.kind == 2u) {
        let delta = (p - paint.center) / max(paint.radius, vec2<f32>(0.00001));
        return interpolate_color(paint, length(delta));
    }
    if (paint.kind == 3u) {
        let angle = atan2(p.y - paint.center.y, p.x - paint.center.x) / 6.28318530718 + 0.5;
        return interpolate_color(paint, fract(angle - paint.angle / 360.0));
    }
    return paint.colors[0];
}

fn distance_to_segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let ab = b - a;
    let h = clamp(dot(p - a, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0);
    return length(p - (a + ab * h));
}

fn polygon_contains(p: vec2<f32>) -> bool {
    var inside = false;
    let count = params.point_count;
    if (count < 3u) {
        return false;
    }

    var j = count - 1u;
    for (var i = 0u; i < count; i = i + 1u) {
        let pi = path_points.points[i] + params.position;
        let pj = path_points.points[j] + params.position;
        let denominator = pj.y - pi.y;
        let safe_denominator = select(denominator, 0.000001, abs(denominator) < 0.000001);
        let crosses = ((pi.y > p.y) != (pj.y > p.y)) &&
            (p.x < (pj.x - pi.x) * (p.y - pi.y) / safe_denominator + pi.x);
        if (crosses) {
            inside = !inside;
        }
        j = i;
    }
    return inside;
}

fn path_edge_distance(p: vec2<f32>) -> f32 {
    let count = params.point_count;
    if (count < 2u) {
        return 1000000.0;
    }

    var min_distance = 1000000.0;
    var previous = path_points.points[count - 1u] + params.position;
    for (var i = 0u; i < count; i = i + 1u) {
        let current = path_points.points[i] + params.position;
        min_distance = min(min_distance, distance_to_segment(p, previous, current));
        previous = current;
    }
    return min_distance;
}

fn sample_path(pixel: vec2<f32>) -> vec4<f32> {
    let fill_enabled = (params.flags & 1u) != 0u;
    let stroke_enabled = (params.flags & 2u) != 0u;
    let distance = path_edge_distance(pixel);
    let fill_alpha = select(0.0, 1.0, fill_enabled && polygon_contains(pixel));
    let stroke_alpha = select(
        0.0,
        clamp(0.5 - (distance - params.stroke_width * 0.5), 0.0, 1.0),
        stroke_enabled,
    );
    let bounds_origin = params.position + params.bounds_min;
    let local01 = clamp((pixel - bounds_origin) / max(params.bounds_size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let fill = sample_paint(params.fill_paint, pixel, local01) * fill_alpha;
    let stroke = sample_paint(params.stroke_paint, pixel, local01) * stroke_alpha;
    return mix(fill, stroke, stroke_alpha);
}

fn sample_path_aa(pixel_origin: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25;
            color += sample_path(pixel_origin + offset);
        }
    }
    return color * 0.0625;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel_origin = vec2<f32>(f32(id.x), f32(id.y));
    if ((params.flags & 4u) == 0u) {
        textureStore(output_tex, vec2<i32>(id.xy), sample_path(pixel_origin + vec2<f32>(0.5)));
        return;
    }
    let color = sample_path_aa(pixel_origin);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

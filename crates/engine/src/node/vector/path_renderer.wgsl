struct PathParams {
    fill_color: vec4<f32>,
    fill_gradient_start_color: vec4<f32>,
    fill_gradient_end_color: vec4<f32>,
    stroke_color: vec4<f32>,
    position: vec2<f32>,
    fill_gradient_start: vec2<f32>,
    fill_gradient_end: vec2<f32>,
    stroke_width: f32,
    fill_paint: u32,
    flags: u32,
    point_count: u32,
    _pad0: u32,
    _pad1: u32,
}

struct PathPoints {
    points: array<vec2<f32>>,
}

@group(0) @binding(0) var<uniform> params: PathParams;
@group(0) @binding(1) var<storage, read> path_points: PathPoints;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

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

fn linear_gradient_color(local_pixel: vec2<f32>) -> vec4<f32> {
    let axis = params.fill_gradient_end - params.fill_gradient_start;
    let axis_len_sq = max(dot(axis, axis), 0.0001);
    let t = clamp(dot(local_pixel - params.fill_gradient_start, axis) / axis_len_sq, 0.0, 1.0);
    return mix(params.fill_gradient_start_color, params.fill_gradient_end_color, t);
}

fn fill_color_for(local_pixel: vec2<f32>) -> vec4<f32> {
    return select(params.fill_color, linear_gradient_color(local_pixel), params.fill_paint == 1u);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel = vec2<f32>(f32(id.x) + 0.5, f32(id.y) + 0.5);
    let fill_enabled = (params.flags & 1u) != 0u;
    let stroke_enabled = (params.flags & 2u) != 0u;
    let distance = path_edge_distance(pixel);
    let fill_alpha = select(0.0, 1.0, fill_enabled && polygon_contains(pixel));
    let stroke_alpha = select(
        0.0,
        clamp(0.5 - (distance - params.stroke_width * 0.5), 0.0, 1.0),
        stroke_enabled,
    );
    let fill = fill_color_for(pixel - params.position) * fill_alpha;
    let stroke = params.stroke_color * stroke_alpha;
    let color = mix(fill, stroke, stroke_alpha);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

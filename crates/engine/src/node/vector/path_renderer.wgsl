struct PathParams {
    fill_paint: Paint,
    stroke_paint: Paint,
    position: vec2<f32>,
    bounds_min: vec2<f32>,
    bounds_size: vec2<f32>,
    stroke_width: f32,
    flags: u32,
    point_count: u32,
    trim_start: f32,
    trim_end: f32,
    _pad: u32,
}

struct PathPoint {
    position: vec2<f32>,
    contour_start: u32,
    flags: u32,
    offset: f32,
    _pad: f32,
}

struct PathPoints {
    points: array<PathPoint>,
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
    for (var i = 0u; i < count; i = i + 1u) {
        let current = path_points.points[i];
        let is_end = (current.flags & 1u) != 0u;
        let next_index = select(i + 1u, current.contour_start, is_end);
        if (next_index >= count || next_index == i) {
            continue;
        }
        let a = current.position + params.position;
        let b = path_points.points[next_index].position + params.position;
        let denominator = b.y - a.y;
        let safe_denominator = select(denominator, 0.000001, abs(denominator) < 0.000001);
        let crosses = ((a.y > p.y) != (b.y > p.y)) &&
            (p.x < (b.x - a.x) * (p.y - a.y) / safe_denominator + a.x);
        if (crosses) {
            inside = !inside;
        }
    }
    return inside;
}

fn edge_coverage(distance: f32, edge_antialias: bool) -> f32 {
    return select(
        select(0.0, 1.0, distance <= 0.0),
        clamp(0.5 - distance, 0.0, 1.0),
        edge_antialias,
    );
}

fn fill_coverage(signed_distance: f32, inside: bool, edge_antialias: bool) -> f32 {
    return select(
        select(0.0, 1.0, inside),
        clamp(0.5 - signed_distance, 0.0, 1.0),
        edge_antialias,
    );
}

struct PathEdgeDistances {
    full: f32,
    trimmed: f32,
}

fn path_edge_distances(p: vec2<f32>) -> PathEdgeDistances {
    let count = params.point_count;
    if (count < 2u) {
        return PathEdgeDistances(1000000.0, 1000000.0);
    }

    var distances = PathEdgeDistances(1000000.0, 1000000.0);
    let fill_enabled = (params.flags & 1u) != 0u;
    let trimmed_stroke_enabled = (params.flags & 2u) != 0u && params.trim_start < params.trim_end;
    for (var i = 0u; i < count; i = i + 1u) {
        let current = path_points.points[i];
        let is_end = (current.flags & 1u) != 0u;
        let is_closed = (current.flags & 2u) != 0u;
        if (is_end && !is_closed) {
            continue;
        }
        let next_index = select(i + 1u, current.contour_start, is_end);
        if (next_index >= count || next_index == i) {
            continue;
        }
        let next = path_points.points[next_index];
        let segment_start_position = current.position + params.position;
        let segment_end_position = next.position + params.position;
        if (fill_enabled) {
            distances.full = min(
                distances.full,
                distance_to_segment(p, segment_start_position, segment_end_position),
            );
        }

        let segment_end = select(next.offset, 1.0, is_end);
        if (trimmed_stroke_enabled && segment_end >= params.trim_start && current.offset <= params.trim_end) {
            let span = max(segment_end - current.offset, 0.000001);
            let start_t = clamp((params.trim_start - current.offset) / span, 0.0, 1.0);
            let end_t = clamp((params.trim_end - current.offset) / span, 0.0, 1.0);
            distances.trimmed = min(
                distances.trimmed,
                distance_to_segment(
                    p,
                    mix(segment_start_position, segment_end_position, start_t),
                    mix(segment_start_position, segment_end_position, end_t),
                ),
            );
        }
    }
    return distances;
}

fn sample_path(pixel: vec2<f32>) -> vec4<f32> {
    let fill_enabled = (params.flags & 1u) != 0u;
    let stroke_enabled = (params.flags & 2u) != 0u;
    let edge_antialias = (params.flags & 4u) != 0u;
    let distances = path_edge_distances(pixel);
    let inside = polygon_contains(pixel);
    let signed_distance = select(distances.full, -distances.full, inside);
    let fill_alpha = select(
        0.0,
        fill_coverage(signed_distance, inside, edge_antialias),
        fill_enabled,
    );
    let stroke_half_width = params.stroke_width * 0.5;
    let stroke_alpha = select(
        0.0,
        edge_coverage(distances.trimmed - stroke_half_width, edge_antialias),
        stroke_enabled,
    );
    let bounds_origin = params.position + params.bounds_min;
    let local01 = clamp((pixel - bounds_origin) / max(params.bounds_size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let fill = sample_paint(params.fill_paint, pixel, local01) * fill_alpha;
    let stroke = sample_paint(params.stroke_paint, pixel, local01) * stroke_alpha;
    return stroke + fill * (1.0 - stroke.a);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel_origin = vec2<f32>(f32(id.x), f32(id.y));
    textureStore(output_tex, vec2<i32>(id.xy), sample_path(pixel_origin + vec2<f32>(0.5)));
}

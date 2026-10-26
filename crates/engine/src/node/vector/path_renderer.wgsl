struct PathUniforms {
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
    transform: DrawableTransform,
    screen_bounds: vec4<f32>,
    band_origin: f32,
    band_scale: f32,
    band_count: u32,
    reach: f32,
    motion: DrawableMotion,
}

// Bits of `flags` in `PathPoint`; unrelated to the style bits in `PathUniforms.flags`.
const POINT_END: u32 = 1u;
const POINT_CLOSED: u32 = 2u;

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

// A `(start, count)` pair per band, then the first point index of each listed segment.
struct PathBands {
    values: array<u32>,
}

@group(0) @binding(0) var<uniform> params: PathUniforms;
@group(0) @binding(1) var<storage, read> path_points: PathPoints;
@group(0) @binding(2) var<storage, read> path_bands: PathBands;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

// Vector from the nearest point of segment `a`-`b` to `p`.
fn segment_offset(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let ab = b - a;
    let h = clamp(dot(p - a, ab) / max(dot(ab, ab), 0.0001), 0.0, 1.0);
    return p - (a + ab * h);
}

// Signed canvas distance from `p` to the stroke around segment `a`-`b`, all untransformed.
// Under uniform scale its caps are round, so the distance from the segment is exact.
// Otherwise this is the band between its caps' tangents: untransformed points scaled by
// `scale` keep their canvas distances, and there the band is a parallelogram measured
// exactly. Its caps are then measured separately, by `path_edges`. Inside, the deepest
// piece gives the depth, which matches the stroke's own near its edge.
fn stroke_band_distance(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half_width: f32, scale: vec2<f32>) -> f32 {
    if (scale.x == scale.y) {
        return (length(segment_offset(p, a, b)) - half_width) * scale.x;
    }
    // Points of the band are `a + along * axis + across * side`, with `along` from 0 to 1
    // and `across` from -1 to 1.
    let pa = (p - a) * scale;
    let direction = b - a;
    let direction_length = length(direction);
    let normal = vec2<f32>(-direction.y, direction.x) / max(direction_length, 1e-20);
    let side = select(vec2<f32>(0.0), normal * half_width, direction_length > 0.0) * scale;
    let axis = direction * scale;
    let area = axis.x * side.y - axis.y * side.x;
    let safe_area = select(area, 1.0, area == 0.0);
    let along = (pa.x * side.y - pa.y * side.x) / safe_area;
    let across = (axis.x * pa.y - axis.y * pa.x) / safe_area;
    let inside = area != 0.0 && along >= 0.0 && along <= 1.0 && abs(across) <= 1.0;
    // Inside, the nearest of the band's four sides gives its depth. Outside, its ends lie
    // within the caps, so only the long side facing `p` can be nearer than they are.
    let depth = abs(area) * min(
        (1.0 - abs(across)) / max(length(axis), 1e-20),
        min(along, 1.0 - along) / max(length(side), 1e-20),
    );
    let facing = select(-side, side, across >= 0.0);
    let outside = length(segment_offset(pa, facing, axis + facing));
    return select(outside, -depth, inside);
}

// The fractions of a segment between which the trimmed stroke covers it, if it does, and
// whether the stroke ends at its end, where no following segment starts with the cap.
struct StrokeSpan {
    start: f32,
    end: f32,
    covered: bool,
    end_cap: bool,
}

fn stroke_span(current: PathPoint, next: PathPoint, is_end: bool) -> StrokeSpan {
    let segment_end = select(next.offset, 1.0, is_end);
    let span = max(segment_end - current.offset, 0.000001);
    let end = clamp((params.trim_end - current.offset) / span, 0.0, 1.0);
    let next_is_open_end = (next.flags & (POINT_END | POINT_CLOSED)) == POINT_END;
    return StrokeSpan(
        clamp((params.trim_start - current.offset) / span, 0.0, 1.0),
        end,
        segment_end >= params.trim_start && current.offset <= params.trim_end,
        end < 1.0 || is_end || next_is_open_end,
    );
}

// Whether segment `a`-`b` can reach `p`. The open end of a contour joins its start only to
// close the fill.
fn segment_reaches(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, flags: u32) -> bool {
    return (flags & (POINT_END | POINT_CLOSED)) != POINT_END
        && all(p >= min(a, b) - params.reach)
        && all(p <= max(a, b) + params.reach);
}

fn fill_coverage(signed_distance: f32, inside: bool, edge_antialias: bool) -> f32 {
    return select(
        select(0.0, 1.0, inside),
        clamp(0.5 - signed_distance, 0.0, 1.0),
        edge_antialias,
    );
}

// Canvas-space distance to the nearest edge of the fill, the signed canvas-space distance
// to the nearest edge of the trimmed stroke (negative inside it), both within reach, and
// whether the point is inside the fill.
struct PathEdges {
    full: f32,
    stroke: f32,
    inside: bool,
}

// Visits only the segments listed in the band containing `p`, which is relative to
// `params.position`. Segments beyond `params.reach` cannot cover the pixel, so they
// only take part in the fill's crossing count. Fill edges are measured after they have
// been transformed to the canvas. The stroke is the union of a capsule around each
// segment, measured exactly in canvas pixels, so its caps and joins stay elliptical
// under non-uniform scale.
fn path_edges(transform: DrawableTransform, p: vec2<f32>, fill_enabled: bool, stroke_enabled: bool) -> PathEdges {
    var edges = PathEdges(1000000.0, 1000000.0, false);
    let canvas_p = drawable_vector_to_canvas(transform, p);
    let stroke_half_width = params.stroke_width * 0.5;
    let axis_scale = drawable_axis_scale(transform);
    let band_position = (p.y - params.band_origin) * params.band_scale;
    if (band_position < 0.0 || band_position >= f32(params.band_count)) {
        return edges;
    }
    let band = min(u32(band_position), params.band_count - 1u);
    let start = path_bands.values[band * 2u];
    let end = start + path_bands.values[band * 2u + 1u];
    let trimmed_stroke_enabled = stroke_enabled && params.trim_start < params.trim_end;
    var stroke_reaches = false;
    for (var k = start; k < end; k = k + 1u) {
        let i = path_bands.values[k];
        let current = path_points.points[i];
        let is_end = (current.flags & POINT_END) != 0u;
        let next = path_points.points[select(i + 1u, current.contour_start, is_end)];
        let a = current.position;
        let b = next.position;
        if (fill_enabled && (a.y > p.y) != (b.y > p.y)) {
            let denominator = b.y - a.y;
            let safe_denominator = select(denominator, 0.000001, abs(denominator) < 0.000001);
            if (p.x < (b.x - a.x) * (p.y - a.y) / safe_denominator + a.x) {
                edges.inside = !edges.inside;
            }
        }
        if (!segment_reaches(p, a, b, current.flags)) {
            continue;
        }
        if (fill_enabled) {
            let canvas_a = drawable_vector_to_canvas(transform, a);
            let canvas_b = drawable_vector_to_canvas(transform, b);
            edges.full = min(edges.full, length(segment_offset(canvas_p, canvas_a, canvas_b)));
        }
        let span = stroke_span(current, next, is_end);
        if (trimmed_stroke_enabled && span.covered) {
            let distance = stroke_band_distance(
                p,
                mix(a, b, span.start),
                mix(a, b, span.end),
                stroke_half_width,
                axis_scale,
            );
            edges.stroke = min(edges.stroke, distance);
            stroke_reaches = true;
        }
    }

    // Elliptical caps take a second pass, kept apart so their cost does not slow the first
    // for every path, and made only where they can still change the coverage. Each segment
    // adds the cap at its start, and is visited again for the one at its end if no segment
    // follows there.
    if (stroke_reaches && axis_scale.x != axis_scale.y && edges.stroke > -0.5) {
        let cap = vec2<f32>(stroke_half_width) * axis_scale;
        var k = start;
        var at_end = false;
        while (k < end) {
            let i = path_bands.values[k];
            let current = path_points.points[i];
            let is_end = (current.flags & POINT_END) != 0u;
            let next = path_points.points[select(i + 1u, current.contour_start, is_end)];
            let a = current.position;
            let b = next.position;
            let span = stroke_span(current, next, is_end);
            let reaches = span.covered && segment_reaches(p, a, b, current.flags);
            if (reaches) {
                let centre = mix(a, b, select(span.start, span.end, at_end));
                edges.stroke = min(edges.stroke, ellipse_distance((p - centre) * axis_scale, cap).distance);
            }
            at_end = !at_end && reaches && span.end_cap;
            k = k + select(1u, 0u, at_end);
        }
    }
    return edges;
}

fn sample_path(pixel: vec2<f32>, transform: DrawableTransform) -> vec4<f32> {
    let fill_enabled = (params.flags & STYLE_FILL_ENABLED) != 0u;
    let stroke_enabled = (params.flags & STYLE_STROKE_ENABLED) != 0u;
    let edge_antialias = (params.flags & STYLE_EDGE_ANTIALIAS) != 0u;
    // Edges are located in untransformed space and measured in canvas pixels, so
    // antialiasing stays one canvas pixel wide at any scale.
    let local = drawable_to_local(transform, pixel);
    let edges = path_edges(transform, local - params.position, fill_enabled, stroke_enabled);
    let signed_distance = select(edges.full, -edges.full, edges.inside);
    let fill_alpha = select(
        0.0,
        fill_coverage(signed_distance, edges.inside, edge_antialias),
        fill_enabled,
    );
    let stroke_alpha = select(0.0, edge_coverage(edges.stroke, edge_antialias), stroke_enabled);
    if (fill_alpha <= 0.0 && stroke_alpha <= 0.0) {
        return vec4<f32>(0.0);
    }
    let bounds_origin = params.position + params.bounds_min;
    let local01 = clamp((local - bounds_origin) / max(params.bounds_size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let fill = sample_paint(params.fill_paint, local, local01) * fill_alpha;
    let stroke = sample_paint(params.stroke_paint, local, local01) * stroke_alpha;
    return (stroke + fill * (1.0 - stroke.a)) * transform.opacity;
}

// Averages the path over the shutter's taps when it moves.
fn path_color(pixel: vec2<f32>) -> vec4<f32> {
    if (any(pixel < params.screen_bounds.xy) || any(pixel > params.screen_bounds.zw)) {
        return vec4<f32>(0.0);
    }
    if (params.motion.tap_count == 0u) {
        return sample_path(pixel, params.transform);
    }
    var color = vec4<f32>(0.0);
    for (var tap = 0u; tap < params.motion.tap_count; tap = tap + 1u) {
        color += sample_path(pixel, params.motion.taps[tap]);
    }
    return color / f32(params.motion.tap_count);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel_origin = vec2<f32>(f32(id.x), f32(id.y));
    textureStore(output_tex, vec2<i32>(id.xy), path_color(pixel_origin + vec2<f32>(0.5)));
}

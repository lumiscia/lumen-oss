struct PathUniforms {
    fill_paint: Paint,
    stroke_paint: Paint,
    position: vec2<f32>,
    bounds_min: vec2<f32>,
    bounds_size: vec2<f32>,
    stroke_width: f32,
    flags: u32,
    // Untransformed distance the fill's edge moves outward by.
    fill_offset: f32,
    trim_start: f32,
    trim_end: f32,
    // Untransformed width the fill's edge fades over.
    fill_feather: f32,
    transform: DrawableTransform,
    screen_bounds: vec4<f32>,
    band_origin: f32,
    band_scale: f32,
    band_count: u32,
    reach: f32,
    // Canvas pixel of the output's first texel.
    origin: vec2<i32>,
    _origin_pad: vec2<i32>,
    // Lengths in `dash` that the stroke alternately draws and skips, or 0 for a solid stroke.
    dash_count: u32,
    dash_period: f32,
    // Distance into `dash` at which every contour starts.
    dash_start_phase: f32,
    _dash_pad: u32,
    dash: array<vec4<f32>, 8>,
    motion: DrawableMotion,
}

// Bits of `flags` in `PathPoint`; unrelated to the style bits in `PathUniforms.flags`.
const POINT_END: u32 = 1u;
const POINT_CLOSED: u32 = 2u;
// The segment from the point is no edge of the fill, as coincident pieces of the outline
// cancel under the even-odd rule. It still counts toward the crossings, and is stroked.
const POINT_FILL_HIDDEN: u32 = 4u;

struct PathPoint {
    position: vec2<f32>,
    contour_start: u32,
    flags: u32,
    // Fraction of its contour's length at which the point lies.
    offset: f32,
    // Distance into the dash pattern at which the point lies, within one period. A contour's
    // first point holds it where the contour ends instead, after its closing segment.
    dash_phase: f32,
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
@group(0) @binding(4) var<storage, read> repeat_copies: array<RepeatCopy>;
@group(0) @binding(5) var<storage, read> repeat_taps: array<DrawableTransform>;
@group(0) @binding(6) var<storage, read> repeat_tiles: array<u32>;

// Vector from the nearest point of segment `a`-`b` to `p`.
fn segment_offset(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> vec2<f32> {
    let ab = b - a;
    let length_squared = dot(ab, ab);
    let h = select(0.0, clamp(dot(p - a, ab) / length_squared, 0.0, 1.0), length_squared > 0.0);
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

// The fractions of a segment between which the trimmed stroke covers it, if it does, whether
// the stroke ends at its end, where no following segment starts with the cap, the distances
// into the dash pattern at which the segment starts and ends, and its length.
struct StrokeSpan {
    start: f32,
    end: f32,
    covered: bool,
    end_cap: bool,
    start_phase: f32,
    end_phase: f32,
    length: f32,
}

// The span of the segment from point `index`, `current`, to `next`.
fn stroke_span(index: u32, current: PathPoint, next: PathPoint, is_end: bool) -> StrokeSpan {
    let segment_end = select(next.offset, 1.0, is_end);
    let span = max(segment_end - current.offset, 0.000001);
    // Trim bounds beyond the segment clip nothing, even where `span` is too short to divide by.
    let start = select(
        clamp((params.trim_start - current.offset) / span, 0.0, 1.0),
        0.0,
        params.trim_start <= current.offset,
    );
    let end = select(
        clamp((params.trim_end - current.offset) / span, 0.0, 1.0),
        1.0,
        params.trim_end >= segment_end,
    );
    let next_is_open_end = is_open_end(next.flags);
    return StrokeSpan(
        start,
        end,
        segment_end >= params.trim_start && current.offset <= params.trim_end,
        end < 1.0 || is_end || next_is_open_end,
        select(current.dash_phase, params.dash_start_phase, index == current.contour_start),
        next.dash_phase,
        max(length(next.position - current.position), 1e-20),
    );
}

fn dash_length(index: u32) -> f32 {
    return params.dash[index / 4u][index % 4u];
}

// The start and end along a segment of the dash at or before `distance` along it, in `xy`, and
// of the dash after it, in `zw`, where the segment starts `phase` into the pattern.
fn dashes_around(phase: f32, distance: f32) -> vec4<f32> {
    let shifted = phase + distance;
    var start = distance - (shifted - floor(shifted / params.dash_period) * params.dash_period);
    var index = 0u;
    loop {
        let next = start + dash_length(index) + dash_length(index + 1u);
        if (distance < next || index + 2u >= params.dash_count) {
            break;
        }
        start = next;
        index = index + 2u;
    }
    let next = start + dash_length(index) + dash_length(index + 1u);
    let following = select(index + 2u, 0u, index + 2u >= params.dash_count);
    return vec4<f32>(start, start + dash_length(index), next, next + dash_length(following));
}

// `dash` clipped to `span`, both `(start, end)` fractions of a segment, or an empty piece,
// whose start is past its end, where it misses the span.
fn dash_piece(dash: vec2<f32>, span: vec2<f32>) -> vec2<f32> {
    let piece = clamp(dash, vec2<f32>(span.x), vec2<f32>(span.y));
    return select(vec2<f32>(1.0, 0.0), piece, dash.y >= span.x && dash.x <= span.y);
}

// The fraction of segment `a`-`b` at which the stroke's disc, an ellipse in canvas pixels, comes
// nearest `p`, or the middle of the run of discs that cover `p`. As the disc slides along the
// segment, `p` moves along a line relative to its centre. That line comes nearest the ellipse
// beside `support`, its point farthest along the line's normal, and every chord parallel to the
// line is centred on the diameter through `support`.
fn nearest_fraction(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, half_width: f32, scale: vec2<f32>) -> f32 {
    let pa = (p - a) * scale;
    let axis = (b - a) * scale;
    let axis_length2 = max(dot(axis, axis), 1e-20);
    let normal = vec2<f32>(-axis.y, axis.x) * inverseSqrt(axis_length2);
    let radii = half_width * scale;
    let reach = max(length(radii * normal), 1e-20);
    let support = radii * radii * normal / reach;
    let side = clamp(dot(pa, normal) / reach, -1.0, 1.0);
    return (dot(pa, axis) - side * dot(support, axis)) / axis_length2;
}

// The pieces of segment `a`-`b` that the stroke draws and that can be nearest `p`, as
// `(start, end)` fractions of the segment in `xy` and `zw`, measured from `a`, or from `b`
// where `from_b`; empty ones start past their end.
struct StrokePieces {
    pieces: vec4<f32>,
    from_b: bool,
}

// Without dashes the pieces are the span. With them they are the dashes either side of the
// point of the span whose stroke comes nearest `p`: canvas distance from `p` is convex along
// the segment, so every other dash is farther than one of them. Dashes are measured from the
// end nearer `p`, so that they stay precise near either end of a long segment.
fn stroke_pieces(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, span: StrokeSpan, half_width: f32, scale: vec2<f32>) -> StrokePieces {
    let empty = vec2<f32>(1.0, 0.0);
    if (!span.covered) {
        return StrokePieces(vec4<f32>(empty, empty), false);
    }
    if (params.dash_count == 0u) {
        return StrokePieces(vec4<f32>(span.start, span.end, empty), false);
    }
    let from_b = dot(p - b, p - b) < dot(p - a, p - a);
    let near_span = select(vec2<f32>(span.start, span.end), 1.0 - vec2<f32>(span.end, span.start), from_b);
    let along = clamp(
        nearest_fraction(p, select(a, b, from_b), select(b, a, from_b), half_width, scale),
        near_span.x,
        near_span.y,
    );
    // From `b` the pattern runs backwards, so it is measured as the distance before `b`.
    let direction = select(1.0, -1.0, from_b);
    let dashes = dashes_around(
        select(span.start_phase, span.end_phase, from_b),
        direction * along * span.length,
    ) * direction / span.length;
    let first = select(dashes.xy, dashes.yx, from_b);
    let second = select(dashes.zw, dashes.wz, from_b);
    return StrokePieces(vec4<f32>(dash_piece(first, near_span), dash_piece(second, near_span)), from_b);
}

// The point at fraction `t` of segment `a`-`b`, measured from the end that `pieces` are.
fn piece_point(a: vec2<f32>, b: vec2<f32>, pieces: StrokePieces, t: f32) -> vec2<f32> {
    return select(mix(a, b, t), mix(b, a, t), pieces.from_b);
}

// Whether `flags` end an open contour, whose segment back to its start only closes the
// fill and is never stroked.
fn is_open_end(flags: u32) -> bool {
    return (flags & (POINT_END | POINT_CLOSED)) == POINT_END;
}

// Whether segment `a`-`b` can reach `p`.
fn segment_reaches(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> bool {
    return all(p >= min(a, b) - params.reach) && all(p <= max(a, b) + params.reach);
}

fn fill_coverage(signed_distance: f32, inside: bool, edge_antialias: bool) -> f32 {
    return select(
        select(0.0, 1.0, inside),
        clamp(0.5 - signed_distance, 0.0, 1.0),
        edge_antialias,
    );
}

// Whether the fill is offset or feathered, which measures its edges untransformed.
fn fill_is_soft() -> bool {
    return params.fill_offset != 0.0 || params.fill_feather > 0.0;
}

// No edge within reach: deep inside or far outside the fill.
const SOFT_FILL_FAR: f32 = 1e30;

// Coverage of the fill moved out by `fill_offset` and faded over `fill_feather`, centred on
// its moved edge. Moving the edges by `r` adds or removes, along each of them, a capsule of
// untransformed radius `r` around it, so the moved edge is where the untransformed distance to
// the nearest edge is `r`.
//
// The fade is measured untransformed, from that distance. Antialiasing adds one canvas pixel
// to it, converted to untransformed pixels by the ratio of the untransformed to the canvas
// signed distance from the moved edge. Both distances depend only on the outline's shape, not
// on how it is divided, and change continuously, so the ratio does too away from the edge,
// and on the edge the distance it scales is 0. As the offset and feather go to 0, the canvas
// distance is the sharp fill's, so the coverage meets it.
fn soft_fill_coverage(edges: PathEdges, axis_scale: vec2<f32>, edge_antialias: bool) -> f32 {
    if (edges.past >= SOFT_FILL_FAR) {
        return select(0.0, 1.0, edges.inside);
    }
    let radius = abs(params.fill_offset);
    let toward = (params.fill_offset >= 0.0) != edges.inside;
    let local = select(edges.nearest + radius, edges.nearest - radius, toward);
    // Inside the capsules, the depth is never past the capsules' far side either, which bounds
    // it near the ends of edges and meets `past` across the original outline.
    let within = -min(edges.past, max(edges.depth, -edges.capsule));
    let canvas = select(edges.past, select(within, edges.capsule, edges.capsule > 0.0), toward);
    let local_signed = select(local, -local, edges.inside);
    let canvas_signed = select(canvas, -canvas, edges.inside);
    // Untransformed pixels per canvas pixel across the moved edge, within the transform's range.
    let ratio = local_signed / select(canvas_signed, 1.0, canvas_signed == 0.0);
    let longest = max(axis_scale.x, axis_scale.y);
    let shortest = max(min(axis_scale.x, axis_scale.y), 1e-20);
    let per_canvas = select(1.0 / longest, clamp(ratio, 1.0 / longest, 1.0 / shortest), canvas_signed != 0.0);
    let width = params.fill_feather + select(0.0, per_canvas, edge_antialias);
    if (width <= 0.0) {
        return select(0.0, 1.0, local_signed <= 0.0);
    }
    let t = clamp(0.5 - local_signed / width, 0.0, 1.0);
    // The fade eases in and out like a blurred edge as it widens past the antialiasing.
    return mix(t, t * t * (3.0 - 2.0 * t), params.fill_feather / width);
}

// Canvas-space distance to the nearest edge of the fill, the signed canvas-space distance
// to the nearest edge of the trimmed stroke (negative inside it), both within reach, and
// whether the point is inside the fill. A soft fill measures, instead of `full`, the
// untransformed distance to its nearest edge and, in canvas pixels: the signed distance to the
// nearest capsule around its edges, which is exact outside them; the distance past the nearest
// edge's capsule, on the side its edge moves away from; and `depth`, how far inside the
// capsules the point is, which `capsule` alone underestimates where they meet.
struct PathEdges {
    full: f32,
    stroke: f32,
    inside: bool,
    nearest: f32,
    capsule: f32,
    past: f32,
    depth: f32,
}

// Takes in a piece of the fill's edge whose capsule is `capsule` canvas pixels from the point,
// signed, whose capsule's far side, across the piece, is `past` from it, and inside whose
// capsule the point is at least `depth` deep.
fn soft_fill_piece(edges: PathEdges, capsule: f32, past: f32, depth: f32) -> PathEdges {
    var next = edges;
    next.capsule = min(next.capsule, capsule);
    next.past = min(next.past, past);
    next.depth = max(next.depth, depth);
    return next;
}

// Takes in the elliptical end, under non-uniform scale, of the capsule around a piece of the
// fill's edge that ends at `end`, unless it is too far to be nearer than the pieces so far. It
// only bounds the capsules: past an edge, the moved edge beside it is always nearer.
fn soft_fill_end(edges: PathEdges, p: vec2<f32>, end: vec2<f32>, radius: f32, scale: vec2<f32>) -> PathEdges {
    let q = (p - end) * scale;
    if (length(q) - radius * max(scale.x, scale.y) >= edges.capsule) {
        return edges;
    }
    let capsule = ellipse_distance(q, vec2<f32>(radius) * scale).distance;
    return soft_fill_piece(edges, capsule, SOFT_FILL_FAR, -SOFT_FILL_FAR);
}

// Visits only the segments listed in the band containing `p`, which is relative to
// `params.position`. Segments beyond `params.reach` cannot cover the pixel, so they
// only take part in the fill's crossing count. Fill edges are measured after they have
// been transformed to the canvas. The stroke is the union of a capsule around each
// segment, measured exactly in canvas pixels, so its caps and joins stay elliptical
// under non-uniform scale.
fn path_edges(transform: DrawableTransform, p: vec2<f32>, fill_enabled: bool, stroke_enabled: bool) -> PathEdges {
    var edges = PathEdges(
        1000000.0,
        1000000.0,
        false,
        SOFT_FILL_FAR,
        SOFT_FILL_FAR,
        SOFT_FILL_FAR,
        -SOFT_FILL_FAR,
    );
    let fill_radius = abs(params.fill_offset);
    let soft_fill = fill_is_soft();
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
        if (!segment_reaches(p, a, b)) {
            continue;
        }
        let fill_edge = fill_enabled && (current.flags & POINT_FILL_HIDDEN) == 0u;
        if (fill_edge && soft_fill) {
            let nearest = segment_offset(p, a, b);
            let local_distance = length(nearest);
            edges.nearest = min(edges.nearest, local_distance);
            if (axis_scale.x == axis_scale.y) {
                let distance = local_distance * axis_scale.x;
                let width = fill_radius * axis_scale.x;
                edges = soft_fill_piece(edges, distance - width, distance + width, width - distance);
            } else {
                let distance = length(segment_offset((p - a) * axis_scale, vec2<f32>(0.0), (b - a) * axis_scale));
                let direction = b - a;
                let normal = vec2<f32>(-direction.y, direction.x) / max(length(direction), 1e-20);
                // Canvas pixels per untransformed pixel across the piece, so the capsule's sides
                // are `width` canvas pixels either side of it.
                let across = drawable_distance_scale(transform, normal);
                let width = fill_radius * across;
                // Beside the piece, the depth is measured across it, to the side of the capsule's
                // strip. Past its ends it is first-order from the end, but never past that side,
                // so pieces of one straight edge measure the depth the whole edge does. At the end
                // itself the direction from it is undefined, so the normal stands in.
                let strip = (fill_radius - abs(dot(p - a, normal))) * across;
                let along = dot(p - a, direction) / max(dot(direction, direction), 1e-20);
                let toward_end = select(nearest / local_distance, normal, local_distance == 0.0);
                let end_depth = (fill_radius - local_distance) * drawable_distance_scale(transform, toward_end);
                let depth = select(min(end_depth, strip), strip, along > 0.0 && along < 1.0);
                let band = stroke_band_distance(p, a, b, fill_radius, axis_scale);
                edges = soft_fill_piece(edges, band, distance + width, depth);
                if (fill_radius > 0.0) {
                    edges = soft_fill_end(edges, p, a, fill_radius, axis_scale);
                    edges = soft_fill_end(edges, p, b, fill_radius, axis_scale);
                }
            }
        } else if (fill_edge) {
            let canvas_a = drawable_vector_to_canvas(transform, a);
            let canvas_b = drawable_vector_to_canvas(transform, b);
            edges.full = min(edges.full, length(segment_offset(canvas_p, canvas_a, canvas_b)));
        }
        if (is_open_end(current.flags)) {
            continue;
        }
        if (!trimmed_stroke_enabled) {
            continue;
        }
        let pieces = stroke_pieces(
            p,
            a,
            b,
            stroke_span(i, current, next, is_end),
            stroke_half_width,
            axis_scale,
        );
        for (var piece_index = 0u; piece_index < 2u; piece_index = piece_index + 1u) {
            let piece = select(pieces.pieces.xy, pieces.pieces.zw, piece_index == 1u);
            if (piece.x <= piece.y) {
                let distance = stroke_band_distance(
                    p,
                    piece_point(a, b, pieces, piece.x),
                    piece_point(a, b, pieces, piece.y),
                    stroke_half_width,
                    axis_scale,
                );
                edges.stroke = min(edges.stroke, distance);
                stroke_reaches = true;
            }
        }
    }

    // Elliptical caps take a second pass, kept apart so their cost does not slow the first
    // for every path, and made only where they can still change the coverage. Each piece
    // adds the cap at its start, and the one at its end unless a solid stroke goes on into
    // the next segment there. A dash always adds both, as the next segment can round away a
    // piece of it that starts exactly there.
    if (stroke_reaches && axis_scale.x != axis_scale.y && edges.stroke > -0.5) {
        let cap = vec2<f32>(stroke_half_width) * axis_scale;
        for (var k = start; k < end; k = k + 1u) {
            let i = path_bands.values[k];
            let current = path_points.points[i];
            let is_end = (current.flags & POINT_END) != 0u;
            let next = path_points.points[select(i + 1u, current.contour_start, is_end)];
            let a = current.position;
            let b = next.position;
            if (is_open_end(current.flags) || !segment_reaches(p, a, b)) {
                continue;
            }
            let span = stroke_span(i, current, next, is_end);
            let pieces = stroke_pieces(p, a, b, span, stroke_half_width, axis_scale);
            for (var cap_index = 0u; cap_index < 4u; cap_index = cap_index + 1u) {
                let piece = select(pieces.pieces.xy, pieces.pieces.zw, cap_index >= 2u);
                let at_end = (cap_index & 1u) == 1u;
                let continues = params.dash_count == 0u && !span.end_cap;
                if (piece.x > piece.y || (at_end && piece.y >= 1.0 && continues)) {
                    continue;
                }
                let centre = piece_point(a, b, pieces, select(piece.x, piece.y, at_end));
                edges.stroke = min(edges.stroke, ellipse_distance((p - centre) * axis_scale, cap).distance);
            }
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
    var fill_alpha = 0.0;
    if (fill_enabled && fill_is_soft()) {
        fill_alpha = soft_fill_coverage(edges, drawable_axis_scale(transform), edge_antialias);
    } else if (fill_enabled) {
        let signed_distance = select(edges.full, -edges.full, edges.inside);
        fill_alpha = fill_coverage(signed_distance, edges.inside, edge_antialias);
    }
    let stroke_alpha = select(
        0.0,
        stroke_coverage(edges.stroke, canvas_stroke_width(transform, params.stroke_width), edge_antialias),
        stroke_enabled,
    );
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

    let pixel_origin = vec2<f32>(vec2<i32>(id.xy) + params.origin);
    textureStore(output_tex, vec2<i32>(id.xy), path_color(pixel_origin + vec2<f32>(0.5)));
}

// Draws each copy that the tile of `texel` lists over the ones before it. `repeat_tiles`
// starts with the tile side and column count, then a `(start, count)` pair per tile.
fn repeat_color(texel: vec2<u32>, pixel: vec2<f32>) -> vec4<f32> {
    let tile = texel / repeat_tiles[0];
    let entry = 2u + 2u * (tile.y * repeat_tiles[1] + tile.x);
    let start = repeat_tiles[entry];
    let end = start + repeat_tiles[entry + 1u];
    var color = vec4<f32>(0.0);
    for (var at = start; at < end; at = at + 1u) {
        let copy = repeat_copies[repeat_tiles[at]];
        if (any(pixel < copy.bounds.xy) || any(pixel > copy.bounds.zw)) {
            continue;
        }
        var copy_color = vec4<f32>(0.0);
        if (copy.tap_count == 0u) {
            copy_color = sample_path(pixel, copy.transform);
        } else {
            for (var tap = 0u; tap < copy.tap_count; tap = tap + 1u) {
                copy_color += sample_path(pixel, repeat_taps[copy.first_tap + tap]);
            }
            copy_color /= f32(copy.tap_count);
        }
        color = copy_color + color * (1.0 - copy_color.a);
    }
    return color;
}

@compute @workgroup_size(8, 8, 1)
fn cs_repeat(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel_origin = vec2<f32>(vec2<i32>(id.xy) + params.origin);
    textureStore(output_tex, vec2<i32>(id.xy), repeat_color(id.xy, pixel_origin + vec2<f32>(0.5)));
}

// Rasterizes a drawing: each workgroup covers a quarter of one tile of the output, and composites
// the copies its tile lists at every pixel, from the top down, until the pixel is opaque.

// One drawable: the fill or stroke of a path or a shape, or a shadow of one, which each of its
// copies places. Paths read their points and their bands and grid from the shared buffers, at
// offsets folded into the indices they hold.
struct VectorDrawable {
    // Maps the untransformed points measured here to those the paint is laid out in.
    paint_frame: mat2x2<f32>,
    // The box a paint in bounding-box units spans.
    paint_min: vec2<f32>,
    paint_size: vec2<f32>,
    position: vec2<f32>,
    // Width and height of a shape.
    size: vec2<f32>,
    grid_origin: vec2<f32>,
    grid_scale: vec2<f32>,
    grid_size: vec2<u32>,
    kind: u32,
    flags: u32,
    stroke_width: f32,
    // Untransformed distance the outline grows by.
    fill_offset: f32,
    fill_feather: f32,
    trim_start: f32,
    trim_end: f32,
    band_origin: f32,
    band_scale: f32,
    band_count: u32,
    reach: f32,
    grid_start: u32,
    // Index in `path_bands` of the first band's `(start, count)` pair.
    band_start: u32,
    geometry_kind: u32,
    border_radius: f32,
    paint: u32,
    // Canvas width a shadow's edge fades over, and for a shape how far it grows first.
    shadow_softness: f32,
    shadow_spread: f32,
    dash_count: u32,
    dash_period: f32,
    dash_start_phase: f32,
    // Index in `paints` of a shadow's colour, which `paint`'s alpha scales.
    shadow_paint: u32,
    dash: array<vec4<f32>, 8>,
}

// One place a drawable is drawn at.
struct VectorCopy {
    transform: DrawableTransform,
    screen_bounds: vec4<f32>,
    // Linear colour multiplying a fill's or stroke's paint.
    tint: vec3<f32>,
    drawable: u32,
}

struct DrawingParams {
    // Canvas pixel of the output's first texel.
    origin: vec2<i32>,
    tile_columns: u32,
    _pad: u32,
}

// Values of `VectorDrawable.kind`.
const DRAWABLE_PATH: u32 = 0u;
const DRAWABLE_SHAPE: u32 = 1u;
// Bits of `VectorDrawable.flags` beside the style bits. A shadow is drawn in its paint wherever
// its fill or stroke fades in. An offset stroke is a path's stroke measured from the signed
// distance to its outline, which `fill_offset` moves. A sheared drawable's linear map has
// columns that are not perpendicular.
const STYLE_SHADOW: u32 = 8u;
const STYLE_OFFSET_STROKE: u32 = 16u;
const DRAWABLE_SHEARED: u32 = 32u;
const TILE_SIZE: u32 = 16u;
// Side in texels of the square a workgroup draws.
const GROUP_SIZE: u32 = 8u;

@group(0) @binding(0) var<uniform> drawing: DrawingParams;
@group(0) @binding(1) var<storage, read> drawables: array<VectorDrawable>;
@group(0) @binding(2) var<storage, read> copies: array<VectorCopy>;
@group(0) @binding(3) var<storage, read> path_points: PathPoints;
@group(0) @binding(4) var<storage, read> path_bands: PathBands;
@group(0) @binding(5) var<storage, read> paints: array<Paint>;
// A `(start, count)` pair per tile, then the copies each lists, topmost first.
@group(0) @binding(6) var<storage, read> tiles: array<u32>;
@group(0) @binding(7) var output_tex: texture_storage_2d<rgba16float, write>;

// The drawable being drawn, which the path and shape geometry read as `params`, and the copy of
// it being drawn.
var<private> params: VectorDrawable;
var<private> placed: VectorCopy;

fn dash_length(index: u32) -> f32 {
    return drawables[placed.drawable].dash[index / 4u][index % 4u];
}

// Coverage of a shadow `distance` canvas pixels outside its grown outline, fading like a blurred
// edge over `shadow_softness`, centred on the outline.
fn shadow_coverage(distance: f32) -> f32 {
    if (params.shadow_softness <= 0.0) {
        return clamp(0.5 - distance, 0.0, 1.0);
    }
    let t = clamp(0.5 - distance / params.shadow_softness, 0.0, 1.0);
    return t * t * (3.0 - 2.0 * t);
}

fn sample_drawable_paint(local: vec2<f32>) -> vec4<f32> {
    let laid_out = params.paint_frame * local;
    let unit = clamp(
        (laid_out - params.paint_min) / max(params.paint_size, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    return sample_paint(paints[params.paint], laid_out, unit);
}

// The drawable's colour at `local`, or a shadow's colour where its drawable's paint is opaque.
fn drawable_color(local: vec2<f32>) -> vec4<f32> {
    let source = sample_drawable_paint(local);
    if ((params.flags & STYLE_SHADOW) == 0u) {
        return source * vec4<f32>(placed.tint, 1.0);
    }
    return sample_paint(paints[params.shadow_paint], local, vec2<f32>(0.0)) * source.a;
}

// Coverage of a shape's fill, `distance` canvas pixels outside its edge, faded over `fill_feather`
// untransformed pixels at the transform's mean scale.
fn shape_fill_coverage(distance: f32, transform: DrawableTransform, edge_antialias: bool) -> f32 {
    if (params.fill_feather <= 0.0) {
        return edge_coverage(distance, edge_antialias);
    }
    let scale = drawable_axis_scale(transform);
    let feather = params.fill_feather * sqrt(scale.x * scale.y);
    let width = feather + select(0.0, 1.0, edge_antialias);
    let t = clamp(0.5 - distance / width, 0.0, 1.0);
    return mix(t, t * t * (3.0 - 2.0 * t), feather / width);
}

// The canvas distance to the stroke centred on an offset path's outline, negative inside it, and
// how much of the stroke trim and dash leave at the point, both measured along the original
// outline from the point on it nearest `p`. The fill grid only lists edges that can be nearest
// where the stroke's coverage varies, and decides the cells it cannot reach.
struct OffsetStroke {
    distance: f32,
    along: f32,
}

fn offset_stroke(transform: DrawableTransform, p: vec2<f32>) -> OffsetStroke {
    var stroke = OffsetStroke(1000000.0, 0.0);
    let cell_position = (p - params.grid_origin) * params.grid_scale;
    if (any(cell_position < vec2<f32>(0.0)) || any(cell_position >= vec2<f32>(params.grid_size))) {
        return stroke;
    }
    let cell = min(vec2<u32>(cell_position), params.grid_size - 1u);
    let entry = params.grid_start + 2u * (cell.y * params.grid_size.x + cell.x);
    let start = path_bands.values[entry];
    let cell_count = path_bands.values[entry + 1u];
    if ((cell_count & CELL_FILL_DECIDED) != 0u) {
        return stroke;
    }
    let end = start + (cell_count & CELL_COUNT);
    var nearest = 1e30;
    var nearest_index = 0u;
    var nearest_t = 0.0;
    var toward = vec2<f32>(0.0);
    for (var k = start; k < end; k = k + 1u) {
        let i = path_bands.values[k];
        let current_point = path_points.points[i];
        if ((current_point.flags & POINT_FILL_HIDDEN) != 0u) {
            continue;
        }
        let is_end = (current_point.flags & POINT_END) != 0u;
        let a = current_point.position;
        let b = path_points.points[select(i + 1u, current_point.contour_start, is_end)].position;
        if (!segment_reaches(p, a, b)) {
            continue;
        }
        let ab = b - a;
        let length_squared = dot(ab, ab);
        let t = select(0.0, clamp(dot(p - a, ab) / length_squared, 0.0, 1.0), length_squared > 0.0);
        let offset = p - (a + ab * t);
        let distance = length(offset);
        if (distance < nearest) {
            nearest = distance;
            nearest_index = i;
            nearest_t = t;
            toward = offset;
        }
    }
    if (nearest >= 1e30) {
        return stroke;
    }
    let signed_distance = select(nearest, -nearest, path_inside(p));
    let local = abs(signed_distance - params.fill_offset) - params.stroke_width * 0.5;
    let point = path_points.points[nearest_index];
    let is_end = (point.flags & POINT_END) != 0u;
    let next = path_points.points[select(nearest_index + 1u, point.contour_start, is_end)];
    let direction = next.position - point.position;
    let across = select(toward, vec2<f32>(-direction.y, direction.x), nearest <= 1e-6);
    stroke.distance = local * drawable_distance_scale(transform, across);

    // Trim and dash fade over a canvas pixel along the outline, measured on the nearest segment.
    let along_scale = drawable_distance_scale(transform, direction);
    let segment_length = length(direction);
    let segment_end = select(next.offset, 1.0, is_end);
    let span = max(segment_end - point.offset, 0.000001);
    let fraction = point.offset + nearest_t * (segment_end - point.offset);
    let per_fraction = segment_length / span * along_scale;
    var along = 1.0;
    if (params.trim_start > 0.0) {
        along *= clamp(0.5 + (fraction - params.trim_start) * per_fraction, 0.0, 1.0);
    }
    if (params.trim_end < 1.0) {
        along *= clamp(0.5 + (params.trim_end - fraction) * per_fraction, 0.0, 1.0);
    }
    if (params.dash_count > 0u) {
        let phase = select(point.dash_phase, params.dash_start_phase, nearest_index == point.contour_start);
        let distance = nearest_t * segment_length;
        let dashes = dashes_around(phase, distance);
        let outside = min(
            max(dashes.x - distance, distance - dashes.y),
            max(dashes.z - distance, distance - dashes.w),
        );
        along *= clamp(0.5 - outside * along_scale, 0.0, 1.0);
    }
    stroke.along = along;
    return stroke;
}

fn draw_path(pixel: vec2<f32>) -> vec4<f32> {
    let transform = placed.transform;
    let fill_enabled = (params.flags & STYLE_FILL_ENABLED) != 0u;
    let stroke_enabled = (params.flags & STYLE_STROKE_ENABLED) != 0u;
    let edge_antialias = (params.flags & STYLE_EDGE_ANTIALIAS) != 0u;
    let shadow = (params.flags & STYLE_SHADOW) != 0u;
    let local = drawable_to_local(transform, pixel);
    let p = local - params.position;
    var alpha = 0.0;
    if ((params.flags & STYLE_OFFSET_STROKE) != 0u) {
        let stroke = offset_stroke(transform, p);
        // A shadow's stroke reaches to the far side of its fade, so it fades in from there.
        let across = select(
            stroke_coverage(stroke.distance, canvas_stroke_width(transform, params.stroke_width), true),
            shadow_coverage(stroke.distance + params.shadow_softness * 0.5),
            shadow,
        );
        alpha = across * stroke.along;
    } else {
        let edges = path_edges(transform, p, fill_enabled, stroke_enabled);
        if (fill_enabled && fill_is_soft()) {
            alpha = soft_fill_coverage(edges, drawable_axis_scale(transform), edge_antialias);
        } else if (fill_enabled) {
            let signed_distance = select(edges.full, -edges.full, edges.inside);
            alpha = fill_coverage(signed_distance, edges.inside, edge_antialias);
        } else if (shadow) {
            alpha = shadow_coverage(edges.stroke + params.shadow_softness * 0.5);
        } else {
            alpha = stroke_coverage(edges.stroke, canvas_stroke_width(transform, params.stroke_width), edge_antialias);
        }
    }
    if (alpha <= 0.0) {
        return vec4<f32>(0.0);
    }
    return drawable_color(local) * alpha * transform.opacity;
}

// The shape's outline grown by `fill_offset`: the signed canvas distance to its fill, and to its
// stroke centred on that outline. A rounded box grows exactly, by growing its corners' radius
// with it. An ellipse grows by its untransformed distance, which converts to canvas pixels to
// first order, as its exact canvas distance does not hold once it is offset or sheared.
fn offset_shape_distance(transform: DrawableTransform, p: vec2<f32>, half_size: vec2<f32>) -> ShapeDistance {
    let offset = params.fill_offset;
    let half_width = params.stroke_width * 0.5;
    if (params.geometry_kind == 1u) {
        let outline = ellipse_distance(p, half_size);
        let across = drawable_distance_scale(transform, outline.normal);
        var fill = (outline.distance - offset) * across;
        if (offset == 0.0 && (params.flags & DRAWABLE_SHEARED) == 0u) {
            let scale = drawable_axis_scale(transform);
            fill = ellipse_distance(p * scale, half_size * scale).distance;
        }
        return ShapeDistance(fill, (abs(outline.distance - offset) - half_width) * across);
    }
    let r = min(params.border_radius, min(half_size.x, half_size.y));
    let grown = half_size + vec2<f32>(offset);
    let fill = rounded_box_distance(transform, p, grown, max(r + offset, 0.0));
    let outer = rounded_box_distance(transform, p, grown + vec2<f32>(half_width), max(r + offset + half_width, 0.0));
    let inner = rounded_box_distance(transform, p, grown - vec2<f32>(half_width), max(r + offset - half_width, 0.0));
    return ShapeDistance(fill, max(outer, -inner));
}

fn draw_shape(pixel: vec2<f32>) -> vec4<f32> {
    let transform = placed.transform;
    let half_size = max(params.size * 0.5, vec2<f32>(0.5));
    let center = params.position + half_size;
    let p = drawable_local_offset(transform, pixel, center);
    let shape = offset_shape_distance(transform, p, half_size);
    let fill_enabled = (params.flags & STYLE_FILL_ENABLED) != 0u;
    let edge_antialias = (params.flags & STYLE_EDGE_ANTIALIAS) != 0u;
    var alpha = 0.0;
    if ((params.flags & STYLE_SHADOW) != 0u) {
        alpha = shadow_coverage(select(shape.stroke, shape.fill, fill_enabled) - params.shadow_spread);
    } else if (fill_enabled) {
        alpha = shape_fill_coverage(shape.fill, transform, edge_antialias);
    } else {
        alpha = stroke_coverage(shape.stroke, canvas_stroke_width(transform, params.stroke_width), edge_antialias);
    }
    if (alpha <= 0.0) {
        return vec4<f32>(0.0);
    }
    return drawable_color(center + p) * alpha * transform.opacity;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(workgroup_id) group: vec3<u32>, @builtin(local_invocation_id) texel_in_group: vec3<u32>) {
    let texel = group.xy * GROUP_SIZE + texel_in_group.xy;
    if (any(texel >= textureDimensions(output_tex))) {
        return;
    }
    let pixel = vec2<f32>(vec2<i32>(texel) + drawing.origin) + vec2<f32>(0.5);
    let tile = group.xy * GROUP_SIZE / TILE_SIZE;
    let entry = 2u * (tile.y * drawing.tile_columns + tile.x);
    let start = tiles[entry];
    let end = start + tiles[entry + 1u];
    var color = vec4<f32>(0.0);
    for (var at = start; at < end; at = at + 1u) {
        placed = copies[tiles[at]];
        // Read before the per-pixel test, where the whole workgroup reads one record at once.
        params = drawables[placed.drawable];
        // Bounds that are not finite hold every pixel.
        if (!(any(pixel < placed.screen_bounds.xy) || any(pixel > placed.screen_bounds.zw))) {
            var drawn: vec4<f32>;
            if (params.kind == DRAWABLE_SHAPE) {
                drawn = draw_shape(pixel);
            } else {
                drawn = draw_path(pixel);
            }
            // Under what is above it, which is the same as drawing it first and the rest over it.
            color += drawn * (1.0 - color.a);
        }
        if (color.a >= 1.0) {
            break;
        }
    }
    textureStore(output_tex, vec2<i32>(texel), color);
}

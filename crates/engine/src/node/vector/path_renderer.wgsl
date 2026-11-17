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
    grid_origin: vec2<f32>,
    // Cells per untransformed pixel along each axis.
    grid_scale: vec2<f32>,
    grid_size: vec2<u32>,
    // Index in `path_bands` of the grid's first `(start, count)` pair.
    grid_start: u32,
    // Index in `path_bands` of the first band's `(start, count)` pair.
    band_start: u32,
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

@group(0) @binding(0) var<uniform> params: PathUniforms;
@group(0) @binding(1) var<storage, read> path_points: PathPoints;
@group(0) @binding(2) var<storage, read> path_bands: PathBands;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var<storage, read> repeat_copies: array<RepeatCopy>;
@group(0) @binding(5) var<storage, read> repeat_taps: array<DrawableTransform>;
@group(0) @binding(6) var<storage, read> repeat_tiles: array<u32>;

fn dash_length(index: u32) -> f32 {
    return params.dash[index / 4u][index % 4u];
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

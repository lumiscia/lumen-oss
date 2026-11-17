struct ShapeUniforms {
    fill_paint: Paint,
    stroke_paint: Paint,
    position: vec2<f32>,
    size: vec2<f32>,
    border_radius: f32,
    stroke_width: f32,
    geometry_kind: u32,
    flags: u32,
    transform: DrawableTransform,
    screen_bounds: vec4<f32>,
    // Canvas pixel of the output's first texel.
    origin: vec2<i32>,
    _origin_pad: vec2<i32>,
    motion: DrawableMotion,
}

@group(0) @binding(0) var<uniform> params: ShapeUniforms;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(2) var<storage, read> repeat_copies: array<RepeatCopy>;
@group(0) @binding(3) var<storage, read> repeat_taps: array<DrawableTransform>;
@group(0) @binding(4) var<storage, read> repeat_tiles: array<u32>;

fn sample_shape(pixel: vec2<f32>, transform: DrawableTransform) -> vec4<f32> {
    // Geometry is evaluated untransformed, about the shape's centre; distances are in
    // canvas pixels so antialiasing stays one canvas pixel wide at any scale.
    let half_size = max(params.size * 0.5, vec2<f32>(0.5));
    let center = params.position + half_size;
    let p = drawable_local_offset(transform, pixel, center);
    let local = center + p;
    let shape = shape_distance(transform, p, half_size);
    let fill_enabled = (params.flags & STYLE_FILL_ENABLED) != 0u;
    let stroke_enabled = (params.flags & STYLE_STROKE_ENABLED) != 0u;
    let edge_antialias = (params.flags & STYLE_EDGE_ANTIALIAS) != 0u;
    let fill_alpha = select(0.0, edge_coverage(shape.fill, edge_antialias), fill_enabled);
    let stroke_alpha = select(
        0.0,
        stroke_coverage(shape.stroke, canvas_stroke_width(transform, params.stroke_width), edge_antialias),
        stroke_enabled,
    );
    let local01 = clamp((local - params.position) / max(params.size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let fill = sample_paint(params.fill_paint, local, local01) * fill_alpha;
    let stroke = sample_paint(params.stroke_paint, local, local01) * stroke_alpha;
    return (stroke + fill * (1.0 - stroke.a)) * transform.opacity;
}

// Averages the shape over the shutter's taps when it moves.
fn shape_color(pixel: vec2<f32>) -> vec4<f32> {
    if (any(pixel < params.screen_bounds.xy) || any(pixel > params.screen_bounds.zw)) {
        return vec4<f32>(0.0);
    }
    if (params.motion.tap_count == 0u) {
        return sample_shape(pixel, params.transform);
    }
    var color = vec4<f32>(0.0);
    for (var tap = 0u; tap < params.motion.tap_count; tap = tap + 1u) {
        color += sample_shape(pixel, params.motion.taps[tap]);
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
    textureStore(output_tex, vec2<i32>(id.xy), shape_color(pixel_origin + vec2<f32>(0.5)));
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
            copy_color = sample_shape(pixel, copy.transform);
        } else {
            for (var tap = 0u; tap < copy.tap_count; tap = tap + 1u) {
                copy_color += sample_shape(pixel, repeat_taps[copy.first_tap + tap]);
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

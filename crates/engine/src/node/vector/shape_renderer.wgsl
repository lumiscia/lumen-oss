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
}

@group(0) @binding(0) var<uniform> params: ShapeUniforms;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

// Signed canvas distances to the fill and to the stroke, negative inside.
struct ShapeDistance {
    fill: f32,
    stroke: f32,
}

// A box centred on the origin with corners rounded by `radius`. Sides are measured per
// axis in canvas space, so the nearest side in canvas space wins under non-uniform scale.
// Outside a sharp corner the canvas distance to the corner point is exact.
fn rounded_box_distance(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    if (q.x > 0.0 && q.y > 0.0) {
        let corner = q * sign(p);
        if (radius <= 0.0) {
            return length(drawable_vector_to_canvas(params.transform, corner));
        }
        return (length(q) - radius) * drawable_distance_scale(params.transform, corner);
    }
    let scale = vec2<f32>(
        drawable_distance_scale(params.transform, vec2<f32>(1.0, 0.0)),
        drawable_distance_scale(params.transform, vec2<f32>(0.0, 1.0)),
    );
    let distance = (q - vec2<f32>(radius)) * scale;
    return max(distance.x, distance.y);
}

// The stroke is the fill grown by half its width minus the fill shrunk by half its width,
// so each of its edges is measured against its own offset side.
fn rect_distance(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> ShapeDistance {
    let r = min(radius, min(half_size.x, half_size.y));
    let half_width = params.stroke_width * 0.5;
    let outer = rounded_box_distance(p, half_size + vec2<f32>(half_width), r + half_width);
    let inner = rounded_box_distance(p, half_size - vec2<f32>(half_width), max(r - half_width, 0.0));
    return ShapeDistance(rounded_box_distance(p, half_size, r), max(outer, -inner));
}

// The fill's canvas image is an ellipse too, so it is measured exactly. The stroke is the
// exact untransformed distance from the outline, less half the width, divided by the
// canvas length of its gradient.
fn ellipse_shape_distance(p: vec2<f32>, half_size: vec2<f32>) -> ShapeDistance {
    let scale = drawable_axis_scale(params.transform);
    let outline = ellipse_distance(p, half_size);
    return ShapeDistance(
        ellipse_distance(p * scale, half_size * scale).distance,
        (abs(outline.distance) - params.stroke_width * 0.5)
            * drawable_distance_scale(params.transform, outline.normal),
    );
}

fn shape_distance(p: vec2<f32>, half_size: vec2<f32>) -> ShapeDistance {
    if (params.geometry_kind == 1u) {
        return ellipse_shape_distance(p, half_size);
    }
    return rect_distance(p, half_size, params.border_radius);
}

fn sample_shape(pixel: vec2<f32>) -> vec4<f32> {
    if (any(pixel < params.screen_bounds.xy) || any(pixel > params.screen_bounds.zw)) {
        return vec4<f32>(0.0);
    }
    // Geometry is evaluated untransformed, about the shape's centre; distances are in
    // canvas pixels so antialiasing stays one canvas pixel wide at any scale.
    let half_size = max(params.size * 0.5, vec2<f32>(0.5));
    let center = params.position + half_size;
    let p = drawable_local_offset(params.transform, pixel, center);
    let local = center + p;
    let shape = shape_distance(p, half_size);
    let fill_enabled = (params.flags & STYLE_FILL_ENABLED) != 0u;
    let stroke_enabled = (params.flags & STYLE_STROKE_ENABLED) != 0u;
    let edge_antialias = (params.flags & STYLE_EDGE_ANTIALIAS) != 0u;
    let fill_alpha = select(0.0, edge_coverage(shape.fill, edge_antialias), fill_enabled);
    let stroke_alpha = select(
        0.0,
        edge_coverage(shape.stroke, edge_antialias),
        stroke_enabled,
    );
    let local01 = clamp((local - params.position) / max(params.size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let fill = sample_paint(params.fill_paint, local, local01) * fill_alpha;
    let stroke = sample_paint(params.stroke_paint, local, local01) * stroke_alpha;
    return (stroke + fill * (1.0 - stroke.a)) * params.transform.opacity;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel_origin = vec2<f32>(f32(id.x), f32(id.y));
    textureStore(output_tex, vec2<i32>(id.xy), sample_shape(pixel_origin + vec2<f32>(0.5)));
}

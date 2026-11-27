// Rectangle and ellipse outlines measured from a pixel. `drawing.wgsl` declares the `params` it
// reads.

// Signed canvas distances to the fill and to the stroke, negative inside.
struct ShapeDistance {
    fill: f32,
    stroke: f32,
}

// A box centred on the origin with corners rounded by `radius`. Sides are measured per
// axis in canvas space, so the nearest side in canvas space wins under non-uniform scale.
// Outside a sharp corner the canvas distance to the corner point is exact.
fn rounded_box_distance(transform: DrawableTransform, p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - half_size + vec2<f32>(radius);
    if (q.x > 0.0 && q.y > 0.0) {
        let corner = q * sign(p);
        if (radius <= 0.0) {
            return length(drawable_vector_to_canvas(transform, corner));
        }
        return (length(q) - radius) * drawable_distance_scale(transform, corner);
    }
    let scale = vec2<f32>(
        drawable_distance_scale(transform, vec2<f32>(1.0, 0.0)),
        drawable_distance_scale(transform, vec2<f32>(0.0, 1.0)),
    );
    let distance = (q - vec2<f32>(radius)) * scale;
    return max(distance.x, distance.y);
}

// The stroke is the fill grown by half its width minus the fill shrunk by half its width,
// so each of its edges is measured against its own offset side.
fn rect_distance(transform: DrawableTransform, p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> ShapeDistance {
    let r = min(radius, min(half_size.x, half_size.y));
    let half_width = params.stroke_width * 0.5;
    let outer = rounded_box_distance(transform, p, half_size + vec2<f32>(half_width), r + half_width);
    let inner = rounded_box_distance(transform, p, half_size - vec2<f32>(half_width), max(r - half_width, 0.0));
    return ShapeDistance(rounded_box_distance(transform, p, half_size, r), max(outer, -inner));
}

// The fill's canvas image is an ellipse too, so it is measured exactly. The stroke is the
// exact untransformed distance from the outline, less half the width, divided by the
// canvas length of its gradient.
fn ellipse_shape_distance(transform: DrawableTransform, p: vec2<f32>, half_size: vec2<f32>) -> ShapeDistance {
    let scale = drawable_axis_scale(transform);
    let outline = ellipse_distance(p, half_size);
    return ShapeDistance(
        ellipse_distance(p * scale, half_size * scale).distance,
        (abs(outline.distance) - params.stroke_width * 0.5)
            * drawable_distance_scale(transform, outline.normal),
    );
}

fn shape_distance(transform: DrawableTransform, p: vec2<f32>, half_size: vec2<f32>) -> ShapeDistance {
    if (params.geometry_kind == 1u) {
        return ellipse_shape_distance(transform, p, half_size);
    }
    return rect_distance(transform, p, half_size, params.border_radius);
}

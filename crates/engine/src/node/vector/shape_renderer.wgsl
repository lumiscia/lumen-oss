struct ShapeParams {
    fill_paint: Paint,
    stroke_paint: Paint,
    position: vec2<f32>,
    size: vec2<f32>,
    border_radius: f32,
    stroke_width: f32,
    geometry_kind: u32,
    flags: u32,
}

@group(0) @binding(0) var<uniform> params: ShapeParams;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn sd_rect(p: vec2<f32>, half_size: vec2<f32>, radius: f32) -> f32 {
    let r = min(radius, min(half_size.x, half_size.y));
    let q = abs(p) - half_size + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn sd_ellipse(p: vec2<f32>, half_size: vec2<f32>) -> f32 {
    let safe_half = max(half_size, vec2<f32>(0.5));
    let normalized = p / safe_half;
    return (length(normalized) - 1.0) * min(safe_half.x, safe_half.y);
}

fn edge_coverage(distance: f32, edge_antialias: bool) -> f32 {
    return select(
        select(0.0, 1.0, distance <= 0.0),
        clamp(0.5 - distance, 0.0, 1.0),
        edge_antialias,
    );
}

fn shape_distance(pixel: vec2<f32>) -> f32 {
    let half_size = max(params.size * 0.5, vec2<f32>(0.5));
    let center = params.position + half_size;
    let local = pixel - center;
    return select(
        sd_rect(local, half_size, params.border_radius),
        sd_ellipse(local, half_size),
        params.geometry_kind == 1u,
    );
}

fn sample_shape(pixel: vec2<f32>) -> vec4<f32> {
    let distance = shape_distance(pixel);
    let fill_enabled = (params.flags & 1u) != 0u;
    let stroke_enabled = (params.flags & 2u) != 0u;
    let edge_antialias = (params.flags & 4u) != 0u;
    let fill_alpha = select(0.0, edge_coverage(distance, edge_antialias), fill_enabled);
    let stroke_distance = abs(distance) - params.stroke_width * 0.5;
    let stroke_alpha = select(
        0.0,
        edge_coverage(stroke_distance, edge_antialias),
        stroke_enabled,
    );
    let local01 = clamp((pixel - params.position) / max(params.size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
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
    textureStore(output_tex, vec2<i32>(id.xy), sample_shape(pixel_origin + vec2<f32>(0.5)));
}

struct ShapeParams {
    fill_color: vec4<f32>,
    fill_gradient_start_color: vec4<f32>,
    fill_gradient_end_color: vec4<f32>,
    stroke_color: vec4<f32>,
    position: vec2<f32>,
    size: vec2<f32>,
    fill_gradient_start: vec2<f32>,
    fill_gradient_end: vec2<f32>,
    border_radius: f32,
    stroke_width: f32,
    geometry_kind: u32,
    fill_paint: u32,
    flags: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
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
    let half_size = max(params.size * 0.5, vec2<f32>(0.5));
    let center = params.position + half_size;
    let local = pixel - center;
    let distance = select(
        sd_rect(local, half_size, params.border_radius),
        sd_ellipse(local, half_size),
        params.geometry_kind == 1u,
    );
    let fill_enabled = (params.flags & 1u) != 0u;
    let stroke_enabled = (params.flags & 2u) != 0u;
    let fill_alpha = select(0.0, clamp(0.5 - distance, 0.0, 1.0), fill_enabled);
    let stroke_distance = abs(distance) - params.stroke_width * 0.5;
    let stroke_alpha = select(0.0, clamp(0.5 - stroke_distance, 0.0, 1.0), stroke_enabled);
    let fill = fill_color_for(pixel - params.position) * fill_alpha;
    let stroke = params.stroke_color * stroke_alpha;
    let color = mix(fill, stroke, stroke_alpha);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

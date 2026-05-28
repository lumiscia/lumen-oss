struct Paint {
    colors: array<vec4<f32>, 8>,
    offsets: array<vec4<f32>, 8>,
    start: vec2<f32>,
    end: vec2<f32>,
    center: vec2<f32>,
    radius: vec2<f32>,
    angle: f32,
    kind: u32,
    units: u32,
    spread: u32,
    interpolation: u32,
    stop_count: u32,
    anti_alias: u32,
    _pad: u32,
}

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

fn spread_t(paint: Paint, value: f32) -> f32 {
    if (paint.spread == 1u) {
        return fract(value);
    }
    if (paint.spread == 2u) {
        let repeated = fract(value * 0.5) * 2.0;
        return select(repeated, 2.0 - repeated, repeated > 1.0);
    }
    return clamp(value, 0.0, 1.0);
}

fn paint_space_point(paint: Paint, pixel: vec2<f32>, local01: vec2<f32>) -> vec2<f32> {
    return select(local01, pixel, paint.units == 1u);
}

fn interpolate_color(paint: Paint, t_raw: f32) -> vec4<f32> {
    let t = spread_t(paint, t_raw);
    let count = max(paint.stop_count, 1u);
    var color = paint.colors[0];
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (i < count) {
            let left = paint.offsets[i - 1u].x;
            let right = paint.offsets[i].x;
            let span = max(right - left, 0.00001);
            let amount = clamp((t - left) / span, 0.0, 1.0);
            color = select(color, mix(paint.colors[i - 1u], paint.colors[i], amount), t >= left);
        }
    }
    return color;
}

fn sample_paint(paint: Paint, pixel: vec2<f32>, local01: vec2<f32>) -> vec4<f32> {
    let p = paint_space_point(paint, pixel, local01);
    if (paint.kind == 1u) {
        let axis = paint.end - paint.start;
        return interpolate_color(paint, dot(p - paint.start, axis) / max(dot(axis, axis), 0.00001));
    }
    if (paint.kind == 2u) {
        let delta = (p - paint.center) / max(paint.radius, vec2<f32>(0.00001));
        return interpolate_color(paint, length(delta));
    }
    if (paint.kind == 3u) {
        let angle = atan2(p.y - paint.center.y, p.x - paint.center.x) / 6.28318530718 + 0.5;
        return interpolate_color(paint, fract(angle - paint.angle / 360.0));
    }
    return paint.colors[0];
}

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
    let fill_alpha = select(0.0, clamp(0.5 - distance, 0.0, 1.0), fill_enabled);
    let stroke_distance = abs(distance) - params.stroke_width * 0.5;
    let stroke_alpha = select(0.0, clamp(0.5 - stroke_distance, 0.0, 1.0), stroke_enabled);
    let local01 = clamp((pixel - params.position) / max(params.size, vec2<f32>(1.0)), vec2<f32>(0.0), vec2<f32>(1.0));
    let fill = sample_paint(params.fill_paint, pixel, local01) * fill_alpha;
    let stroke = sample_paint(params.stroke_paint, pixel, local01) * stroke_alpha;
    return mix(fill, stroke, stroke_alpha);
}

fn sample_shape_aa(pixel_origin: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25;
            color += sample_shape(pixel_origin + offset);
        }
    }
    return color * 0.0625;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let dims = textureDimensions(output_tex);
    if (id.x >= dims.x || id.y >= dims.y) {
        return;
    }

    let pixel_origin = vec2<f32>(f32(id.x), f32(id.y));
    if ((params.flags & 4u) == 0u) {
        textureStore(output_tex, vec2<i32>(id.xy), sample_shape(pixel_origin + vec2<f32>(0.5)));
        return;
    }
    let color = sample_shape_aa(pixel_origin);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

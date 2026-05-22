struct Paint {
    colors: array<vec4<f32>, 8>,
    offsets: array<f32, 8>,
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
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var<uniform> paint: Paint;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn spread_t(value: f32) -> f32 {
    if (paint.spread == 1u) {
        return fract(value);
    }
    if (paint.spread == 2u) {
        let repeated = fract(value * 0.5) * 2.0;
        return select(repeated, 2.0 - repeated, repeated > 1.0);
    }
    return clamp(value, 0.0, 1.0);
}

fn interpolate_color(t_raw: f32) -> vec4<f32> {
    let t = spread_t(t_raw);
    let count = max(paint.stop_count, 1u);
    var color = paint.colors[0];
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (i < count) {
            let left = paint.offsets[i - 1u];
            let right = paint.offsets[i];
            let span = max(right - left, 0.00001);
            let amount = clamp((t - left) / span, 0.0, 1.0);
            color = select(color, mix(paint.colors[i - 1u], paint.colors[i], amount), t >= left);
        }
    }
    return color;
}

fn sample_paint(pixel: vec2<f32>, local01: vec2<f32>) -> vec4<f32> {
    let p = select(local01, pixel, paint.units == 1u);
    if (paint.kind == 1u) {
        let axis = paint.end - paint.start;
        return interpolate_color(dot(p - paint.start, axis) / max(dot(axis, axis), 0.00001));
    }
    if (paint.kind == 2u) {
        let delta = (p - paint.center) / max(paint.radius, vec2<f32>(0.00001));
        return interpolate_color(length(delta));
    }
    if (paint.kind == 3u) {
        let angle = atan2(p.y - paint.center.y, p.x - paint.center.x) / 6.28318530718 + 0.5;
        return interpolate_color(angle - paint.angle / 360.0);
    }
    return paint.colors[0];
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<f32>(f32(id.x) + 0.5, f32(id.y) + 0.5);
    let local01 = pixel / vec2<f32>(f32(size.x), f32(size.y));
    textureStore(output_tex, vec2<i32>(id.xy), sample_paint(pixel, local01));
}

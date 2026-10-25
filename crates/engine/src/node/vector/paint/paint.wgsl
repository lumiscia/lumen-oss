// Shared paint sampling for background, shape and path shaders. Requires the
// alpha chunk. Paint colours arrive as straight sRGB and are sampled as
// premultiplied colour, with gradient stops interpolated in premultiplied space.

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
    paint_supersample: u32,
    _pad: u32,
}

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

// TODO: support interpolation modes beyond the current sRGB mix path.
fn interpolate_color(paint: Paint, t_raw: f32) -> vec4<f32> {
    let t = spread_t(paint, t_raw);
    let count = max(paint.stop_count, 1u);
    var color = premultiply(paint.colors[0]);
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (i < count) {
            let left = paint.offsets[i - 1u].x;
            let right = paint.offsets[i].x;
            let span = max(right - left, 0.00001);
            let amount = clamp((t - left) / span, 0.0, 1.0);
            let blended = mix(premultiply(paint.colors[i - 1u]), premultiply(paint.colors[i]), amount);
            color = select(color, blended, t >= left);
        }
    }
    return color;
}

fn sample_paint(paint: Paint, pixel: vec2<f32>, local01: vec2<f32>) -> vec4<f32> {
    let p = select(local01, pixel, paint.units == 1u);
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
    return premultiply(paint.colors[0]);
}


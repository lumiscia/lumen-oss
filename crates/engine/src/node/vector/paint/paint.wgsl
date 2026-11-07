// Shared paint sampling for background, shape and path shaders. Requires the
// alpha chunk. Paint colours arrive as straight sRGB and are sampled as
// premultiplied colour, with gradient stops interpolated in premultiplied space, in sRGB or in
// linear light as the paint asks.

struct Paint {
    colors: array<vec4<f32>, 8>,
    offsets: array<vec4<f32>, 8>,
    start: vec2<f32>,
    end: vec2<f32>,
    center: vec2<f32>,
    radius: vec2<f32>,
    focal_offset: vec2<f32>,
    angle: f32,
    kind: u32,
    units: u32,
    spread: u32,
    interpolation: u32,
    stop_count: u32,
    paint_supersample: u32,
    // The bits of the half width of the stroke a text paint draws along distance field glyphs,
    // or 0 for every other paint.
    stroke: u32,
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

fn srgb_to_linear(color: vec3<f32>) -> vec3<f32> {
    return select(pow((color + 0.055) / 1.055, vec3<f32>(2.4)), color / 12.92, color <= vec3<f32>(0.04045));
}

fn linear_to_srgb(color: vec3<f32>) -> vec3<f32> {
    return select(1.055 * pow(color, vec3<f32>(1.0 / 2.4)) - 0.055, color * 12.92, color <= vec3<f32>(0.0031308));
}

// Stop `index` premultiplied, with its colour in linear light when the paint interpolates there.
fn gradient_stop(paint: Paint, index: u32) -> vec4<f32> {
    let stop = paint.colors[index];
    if (paint.interpolation == 1u) {
        return premultiply(vec4<f32>(srgb_to_linear(stop.rgb), stop.a));
    }
    return premultiply(stop);
}

fn interpolate_color(paint: Paint, t_raw: f32) -> vec4<f32> {
    let t = spread_t(paint, t_raw);
    let count = max(paint.stop_count, 1u);
    var color = gradient_stop(paint, 0u);
    for (var i = 1u; i < 8u; i = i + 1u) {
        if (i < count) {
            let left = paint.offsets[i - 1u].x;
            let right = paint.offsets[i].x;
            let span = max(right - left, 0.00001);
            let amount = clamp((t - left) / span, 0.0, 1.0);
            let blended = mix(gradient_stop(paint, i - 1u), gradient_stop(paint, i), amount);
            color = select(color, blended, t >= left);
        }
    }
    if (paint.interpolation == 1u) {
        // Encoding needs straight colour, and unpremultiply maps alpha 0 to transparent black.
        let straight = unpremultiply(color);
        return premultiply(vec4<f32>(linear_to_srgb(straight.rgb), straight.a));
    }
    return color;
}

// Position of `p` between the focal point (0) and the radial circle (1). In the space where that
// circle is the unit circle, `t` is the largest root of |q - f (1 - t)| = t. The focal point lies
// strictly inside the circle, so `a` is positive and the discriminant is never negative.
fn radial_t(paint: Paint, p: vec2<f32>) -> f32 {
    let delta = (p - paint.center) / max(paint.radius, vec2<f32>(0.00001));
    let focal = paint.focal_offset;
    if (all(focal == vec2<f32>(0.0))) {
        return length(delta);
    }
    let d = delta - focal;
    let a = 1.0 - dot(focal, focal);
    let b = dot(d, focal);
    return (b + sqrt(b * b + a * dot(d, d))) / a;
}

fn sample_paint(paint: Paint, pixel: vec2<f32>, local01: vec2<f32>) -> vec4<f32> {
    let p = select(local01, pixel, paint.units == 1u);
    if (paint.kind == 1u) {
        let axis = paint.end - paint.start;
        return interpolate_color(paint, dot(p - paint.start, axis) / max(dot(axis, axis), 0.00001));
    }
    if (paint.kind == 2u) {
        return interpolate_color(paint, radial_t(paint, p));
    }
    if (paint.kind == 3u) {
        let angle = atan2(p.y - paint.center.y, p.x - paint.center.x) / 6.28318530718 + 0.5;
        return interpolate_color(paint, fract(angle - paint.angle / 360.0));
    }
    return premultiply(paint.colors[0]);
}


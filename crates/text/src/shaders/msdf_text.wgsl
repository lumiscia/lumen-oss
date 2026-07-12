struct TextGlobals {
    target_size: vec2<f32>,
    px_range: f32,
    glyph_count: u32,
}

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

struct GlyphInstance {
    rect: vec4<f32>,
    uv_rect: vec4<f32>,
    color: vec4<f32>,
    mode: u32,
    _padding0: u32,
    _padding1: u32,
    _padding2: u32,
}

struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
}

@group(0) @binding(0) var<uniform> globals: TextGlobals;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(0) @binding(3) var<storage, read> glyphs: array<GlyphInstance>;
@group(0) @binding(4) var<uniform> paint: Paint;

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
            let left = paint.offsets[i - 1u].x;
            let right = paint.offsets[i].x;
            let span = max(right - left, 0.00001);
            let amount = clamp((t - left) / span, 0.0, 1.0);
            color = select(color, mix(paint.colors[i - 1u], paint.colors[i], amount), t >= left);
        }
    }
    return color;
}

fn sample_paint(pixel: vec2<f32>) -> vec4<f32> {
    let local01 = pixel / max(globals.target_size, vec2<f32>(1.0));
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
        return interpolate_color(fract(angle - paint.angle / 360.0));
    }
    return paint.colors[0];
}

fn sample_paint_aa(pixel: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25 - vec2<f32>(0.5);
            color += sample_paint(pixel + offset);
        }
    }
    return color * 0.0625;
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    if (instance_index >= globals.glyph_count) {
        var out: VertexOut;
        out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.uv = vec2<f32>(0.0);
        out.color = vec4<f32>(0.0);
        out.mode = 0u;
        return out;
    }
    let glyph = glyphs[instance_index];
    let corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0),
        vec2<f32>(1.0, 1.0),
    );
    let corner = corners[vertex_index];
    let pixel = glyph.rect.xy + corner * glyph.rect.zw;
    let clip = vec2<f32>(
        (pixel.x / globals.target_size.x) * 2.0 - 1.0,
        1.0 - (pixel.y / globals.target_size.y) * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.uv = mix(glyph.uv_rect.xy, glyph.uv_rect.zw, corner);
    out.color = glyph.color;
    out.mode = glyph.mode;
    return out;
}

fn median3(value: vec3<f32>) -> f32 {
    return max(min(value.r, value.g), min(max(value.r, value.g), value.b));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let sample = textureSample(atlas_texture, atlas_sampler, in.uv);
    let uv_width = fwidth(in.uv);
    var text_paint = sample_paint(in.position.xy);
    if (paint.paint_supersample != 0u && paint.kind != 0u) {
        text_paint = sample_paint_aa(in.position.xy);
    }

    if (in.mode == 1u) {
        return vec4<f32>(sample.rgb, sample.a * in.color.a * text_paint.a);
    }

    if (in.mode == 0u) {
        return vec4<f32>(text_paint.rgb, sample.a * in.color.a * text_paint.a);
    }

    let msdf_distance = median3(sample.rgb) - 0.5;
    let sdf_distance = sample.a - 0.5;
    let sign_disagrees = (msdf_distance >= 0.0) != (sdf_distance >= 0.0);
    let signed_distance = select(msdf_distance, sdf_distance, sign_disagrees);
    let atlas_size = vec2<f32>(textureDimensions(atlas_texture, 0));
    let unit_range = vec2<f32>(globals.px_range) / atlas_size;
    let screen_tex_size = vec2<f32>(1.0) / uv_width;
    let screen_px_range = max(0.5 * dot(unit_range, screen_tex_size), 1.0);
    let screen_px_distance = screen_px_range * signed_distance;
    let alpha = clamp(screen_px_distance + 0.5, 0.0, 1.0)
        * in.color.a
        * text_paint.a;
    return vec4<f32>(text_paint.rgb, alpha);
}

struct TextGlobals {
    target_size: vec2<f32>,
    px_range: f32,
    glyph_count: u32,
    paint_bounds: vec4<f32>,
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
    @location(0) texel: vec2<f32>,
    @location(1) color: vec4<f32>,
    @location(2) @interpolate(flat) mode: u32,
    @location(3) @interpolate(flat) uv_rect: vec4<f32>,
}

@group(0) @binding(0) var<uniform> globals: TextGlobals;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(0) @binding(3) var<storage, read> glyphs: array<GlyphInstance>;
struct TextStyle {
    fill: Paint,
    transform: DrawableTransform,
}

@group(0) @binding(4) var<uniform> style: TextStyle;

// Requires the engine's alpha, paint and drawable transform chunks prepended.
fn text_paint_at(pixel: vec2<f32>) -> vec4<f32> {
    let local01 = clamp(
        (pixel - globals.paint_bounds.xy) / max(globals.paint_bounds.zw, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    return sample_paint(style.fill, pixel, local01);
}

// Averages the paint over the canvas pixel; paint follows the text, so each sample
// is evaluated at its untransformed position.
fn text_paint_aa(pixel: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25 - vec2<f32>(0.5);
            color += text_paint_at(drawable_to_local(style.transform, pixel + offset));
        }
    }
    return color * 0.0625;
}

@vertex
fn vs_main(
    @builtin(vertex_index) vertex_index: u32,
    @builtin(instance_index) instance_index: u32,
) -> VertexOut {
    if (instance_index >= globals.glyph_count || style.transform.opacity <= 0.0) {
        var out: VertexOut;
        out.position = vec4<f32>(2.0, 2.0, 0.0, 1.0);
        out.texel = vec2<f32>(0.0);
        out.color = vec4<f32>(0.0);
        out.mode = 0u;
        out.uv_rect = vec4<f32>(0.0);
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
    // Scaled or rotated raster glyphs are filtered, so their quads grow by a texel
    // to fit the filter's falloff; raster texels are one layout pixel.
    let pad = select(0.0, 1.0, drawable_is_linear(style.transform) && glyph.mode != 2u);
    let local = glyph.rect.xy - vec2<f32>(pad) + corner * (glyph.rect.zw + vec2<f32>(2.0 * pad));
    let pixel = drawable_to_canvas(style.transform, local);
    let clip = vec2<f32>(
        (pixel.x / globals.target_size.x) * 2.0 - 1.0,
        1.0 - (pixel.y / globals.target_size.y) * 2.0,
    );

    var out: VertexOut;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.texel = mix(glyph.uv_rect.xy - vec2<f32>(pad), glyph.uv_rect.zw + vec2<f32>(pad), corner);
    out.color = glyph.color;
    out.mode = glyph.mode;
    out.uv_rect = glyph.uv_rect;
    return out;
}

// Raster texel, transparent outside the glyph's atlas slot.
fn raster_texel(coord: vec2<i32>, uv_rect: vec4<f32>) -> vec4<f32> {
    let slot_min = vec2<i32>(uv_rect.xy);
    let slot_max = vec2<i32>(uv_rect.zw) - vec2<i32>(1);
    let inside = all(coord >= slot_min) && all(coord <= slot_max);
    let texel = textureLoad(atlas_texture, clamp(coord, slot_min, slot_max), 0);
    return select(vec4<f32>(0.0), texel, inside);
}

fn raster_bilinear(texel: vec2<f32>, uv_rect: vec4<f32>) -> vec4<f32> {
    let base = floor(texel - vec2<f32>(0.5));
    let frac = texel - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = raster_texel(p0, uv_rect);
    let c10 = raster_texel(p0 + vec2<i32>(1, 0), uv_rect);
    let c01 = raster_texel(p0 + vec2<i32>(0, 1), uv_rect);
    let c11 = raster_texel(p0 + vec2<i32>(1, 1), uv_rect);
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

fn median3(value: vec3<f32>) -> f32 {
    return max(min(value.r, value.g), min(max(value.r, value.g), value.b));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Raster glyphs map 1:1 onto atlas texels unless scaled or rotated. Distance fields
    // are filtered, and sampled outside the branch because derivatives need uniform flow.
    let raster = select(
        textureLoad(atlas_texture, vec2<i32>(floor(in.texel)), 0),
        raster_bilinear(in.texel, in.uv_rect),
        drawable_is_linear(style.transform),
    );
    let field = textureSample(
        atlas_texture,
        atlas_sampler,
        in.texel / vec2<f32>(textureDimensions(atlas_texture)),
    );
    let screen_px_range = max(0.5 * dot(vec2<f32>(globals.px_range), 1.0 / fwidth(in.texel)), 1.0);
    // Paint follows the text, so it is evaluated at the untransformed pixel.
    var text_paint = text_paint_at(drawable_to_local(style.transform, in.position.xy));
    if (style.fill.paint_supersample != 0u) {
        text_paint = text_paint_aa(in.position.xy);
    }
    // The atlas, the paint and the output are premultiplied. Opacity is applied once
    // the glyphs are composited, by the opacity pass below.
    if (in.mode == 1u) {
        return raster * (in.color.a * text_paint.a);
    }
    var coverage = raster.a;
    if (in.mode == 2u) {
        coverage = clamp(screen_px_range * (median3(field.rgb) - 0.5) + 0.5, 0.0, 1.0);
    }
    return text_paint * (coverage * in.color.a);
}

// Opacity pass: a full-target triangle drawn over the composited text with a blend that
// multiplies the target by the source alpha, so overlapping glyphs fade as one layer.
@vertex
fn vs_opacity(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((vertex_index << 1u) & 2u), f32(vertex_index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs_opacity() -> @location(0) vec4<f32> {
    return vec4<f32>(0.0, 0.0, 0.0, style.transform.opacity);
}

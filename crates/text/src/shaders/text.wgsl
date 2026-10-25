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
}

@group(0) @binding(0) var<uniform> globals: TextGlobals;
@group(0) @binding(1) var atlas_texture: texture_2d<f32>;
@group(0) @binding(2) var atlas_sampler: sampler;
@group(0) @binding(3) var<storage, read> glyphs: array<GlyphInstance>;
@group(0) @binding(4) var<uniform> fill: Paint;

// Requires the engine's alpha and paint chunks prepended.
fn text_paint_at(pixel: vec2<f32>) -> vec4<f32> {
    let local01 = clamp(
        (pixel - globals.paint_bounds.xy) / max(globals.paint_bounds.zw, vec2<f32>(1.0)),
        vec2<f32>(0.0),
        vec2<f32>(1.0),
    );
    return sample_paint(fill, pixel, local01);
}

fn text_paint_aa(pixel: vec2<f32>) -> vec4<f32> {
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let offset = (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25 - vec2<f32>(0.5);
            color += text_paint_at(pixel + offset);
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
        out.texel = vec2<f32>(0.0);
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
    out.texel = mix(glyph.uv_rect.xy, glyph.uv_rect.zw, corner);
    out.color = glyph.color;
    out.mode = glyph.mode;
    return out;
}

fn median3(value: vec3<f32>) -> f32 {
    return max(min(value.r, value.g), min(max(value.r, value.g), value.b));
}

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    // Raster glyphs map 1:1 onto atlas texels. Distance fields are filtered,
    // and sampled outside the branch because derivatives need uniform flow.
    let raster = textureLoad(atlas_texture, vec2<i32>(floor(in.texel)), 0);
    let field = textureSample(
        atlas_texture,
        atlas_sampler,
        in.texel / vec2<f32>(textureDimensions(atlas_texture)),
    );
    let screen_px_range = max(0.5 * dot(vec2<f32>(globals.px_range), 1.0 / fwidth(in.texel)), 1.0);
    var text_paint = text_paint_at(in.position.xy);
    if (fill.paint_supersample != 0u) {
        text_paint = text_paint_aa(in.position.xy);
    }
    // The atlas, the paint and the output are premultiplied.
    if (in.mode == 1u) {
        return raster * (in.color.a * text_paint.a);
    }
    var coverage = raster.a;
    if (in.mode == 2u) {
        coverage = clamp(screen_px_range * (median3(field.rgb) - 0.5) + 0.5, 0.0, 1.0);
    }
    return text_paint * (coverage * in.color.a);
}

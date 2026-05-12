struct TextGlobals {
    target_size: vec2<f32>,
    px_range: f32,
    glyph_count: u32,
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

@fragment
fn fs_main(in: VertexOut) -> @location(0) vec4<f32> {
    let sample = textureSample(atlas_texture, atlas_sampler, in.uv);
    if (in.mode == 1u) {
        return vec4<f32>(sample.rgb, sample.a * in.color.a);
    }
    return vec4<f32>(in.color.rgb, sample.a * in.color.a);
}

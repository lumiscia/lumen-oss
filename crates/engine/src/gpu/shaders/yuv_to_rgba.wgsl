// YUV planes to premultiplied RGBA, rotated upright and stretched to the output by nearest
// neighbour. Each pixel uses its own luma and the chroma sample covering it, as swscale does.
struct Params {
    coded_width: u32,
    coded_height: u32,
    // Clockwise quarter turns.
    rotation: u32,
    sample_shift: u32,
    chroma_shift_x: u32,
    chroma_shift_y: u32,
    interleaved: u32,
    has_alpha: u32,
    y_offset: f32,
    y_scale: f32,
    c_offset: f32,
    c_scale: f32,
    a_scale: f32,
    r_cr: f32,
    g_cb: f32,
    g_cr: f32,
    b_cb: f32,
    // Non-zero when the planes' colour is already multiplied by alpha.
    premultiplied: u32,
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var luma: texture_2d<u32>;
// Cb and Cr pairs when interleaved, otherwise Cb.
@group(0) @binding(1) var chroma_u: texture_2d<u32>;
@group(0) @binding(2) var chroma_v: texture_2d<u32>;
@group(0) @binding(3) var alpha: texture_2d<u32>;
@group(0) @binding(4) var<uniform> params: Params;
@group(0) @binding(5) var output: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let w = params.coded_width;
    let h = params.coded_height;
    let quarter = (params.rotation & 1u) == 1u;
    let display = select(vec2<u32>(w, h), vec2<u32>(h, w), quarter);
    let d = id.xy * display / size;
    var coded: vec2<u32>;
    switch params.rotation {
        case 1u: { coded = vec2<u32>(d.y, h - 1u - d.x); }
        case 2u: { coded = vec2<u32>(w - 1u - d.x, h - 1u - d.y); }
        case 3u: { coded = vec2<u32>(w - 1u - d.y, d.x); }
        default: { coded = d; }
    }

    let shift = params.sample_shift;
    let chroma_at = vec2<u32>(coded.x >> params.chroma_shift_x, coded.y >> params.chroma_shift_y);
    var cb_cr: vec2<u32>;
    if (params.interleaved != 0u) {
        cb_cr = textureLoad(chroma_u, chroma_at, 0).xy;
    } else {
        cb_cr = vec2<u32>(textureLoad(chroma_u, chroma_at, 0).x, textureLoad(chroma_v, chroma_at, 0).x);
    }
    let y = (f32(textureLoad(luma, coded, 0).x >> shift) - params.y_offset) * params.y_scale;
    let c = (vec2<f32>(cb_cr >> vec2<u32>(shift)) - params.c_offset) * params.c_scale;
    let rgb = clamp(
        vec3<f32>(
            y + params.r_cr * c.y,
            y + params.g_cb * c.x + params.g_cr * c.y,
            y + params.b_cb * c.x,
        ),
        vec3<f32>(0.0),
        vec3<f32>(1.0),
    );
    var a = 1.0;
    if (params.has_alpha != 0u) {
        a = min(f32(textureLoad(alpha, coded, 0).x >> shift) * params.a_scale, 1.0);
    }
    let color = select(rgb * a, rgb, params.premultiplied != 0u);
    textureStore(output, id.xy, vec4<f32>(color, a));
}

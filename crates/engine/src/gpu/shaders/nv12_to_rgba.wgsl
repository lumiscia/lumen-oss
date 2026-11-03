// NV12 in a buffer to RGBA, using each pixel's own luma and the 2x2 block's chroma, as swscale does
// for 4:2:0 at the same size.
struct Params {
    width: u32,
    height: u32,
    pitch: u32,
    chroma_offset: u32,
    y_scale: f32,
    y_offset: f32,
    r_cr: f32,
    g_cb: f32,
    g_cr: f32,
    b_cb: f32,
}

@group(0) @binding(0) var<storage, read> nv12: array<u32>;
@group(0) @binding(1) var output: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(2) var<uniform> params: Params;

fn byte_at(offset: u32) -> u32 {
    return (nv12[offset / 4u] >> ((offset % 4u) * 8u)) & 0xffu;
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) {
        return;
    }

    let chroma = params.chroma_offset + (id.y / 2u) * params.pitch + (id.x & ~1u);
    let luma = params.y_scale * (f32(byte_at(id.y * params.pitch + id.x)) - params.y_offset);
    let cb = f32(byte_at(chroma)) - 128.0;
    let cr = f32(byte_at(chroma + 1u)) - 128.0;
    let rgb = vec3<f32>(
        luma + params.r_cr * cr,
        luma + params.g_cb * cb + params.g_cr * cr,
        luma + params.b_cb * cb,
    );
    textureStore(output, id.xy, vec4<f32>(clamp(rgb / 255.0, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}

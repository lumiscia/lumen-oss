// The working space, linear and premultiplied, to BT.709 limited-range YUV 4:2:0 of its
// sRGB-encoded colour over black, matching the CPU encoder's colour conversion, at 8 or 10 bits.
// Each code is rounded once, from float colour, with the ordered dither when `dither` is set.
// Samples above 8 bits take two little-endian bytes: in the high bits for the two-plane layout
// (P010), in the low bits for the planar one. Requires the colour and dither chunks prepended.
struct Params {
    width: u32,
    height: u32,
    // Bytes between luma rows, and between chroma rows of the two-plane layout. Planar chroma
    // rows are half that apart.
    pitch: u32,
    chroma_offset: u32,
    dither: u32,
    depth: u32,
    planar: u32,
    _pad: u32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> yuv: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

const KR: f32 = 0.2126;
const KB: f32 = 0.0722;

// The encoded colour at a pixel. Premultiplied colour over black is the colour itself.
fn rgb(x: u32, y: u32) -> vec3<f32> {
    let clamped = vec2<u32>(min(x, params.width - 1u), min(y, params.height - 1u));
    let light = max(textureLoad(source, clamped, 0).rgb, vec3<f32>(0.0));
    return clamp(lumen_linear_to_srgb(light), vec3<f32>(0.0), vec3<f32>(1.0));
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(KR, 1.0 - KR - KB, KB));
}

// `value`, in 8-bit code steps, as a code of `params.depth` bits, dithered at `pixel` of the plane
// it belongs to.
fn to_code(value: f32, pixel: vec2<u32>) -> u32 {
    let scale = f32(1u << (params.depth - 8u));
    let top = f32((1u << params.depth) - 1u);
    return u32(clamp(lumen_quantise(value * scale, pixel, params.dither != 0u), 0.0, top));
}

fn luma_code(color: vec3<f32>, pixel: vec2<u32>) -> u32 {
    return to_code(16.0 + 219.0 * luma(color), pixel);
}

// Packs the first `count` of `samples` into the words from `byte_offset`, a multiple of 4.
fn store(byte_offset: u32, samples: array<u32, 8>, count: u32) {
    var values = samples;
    let base = byte_offset / 4u;
    if (params.depth > 8u) {
        let shift = select(0u, 16u - params.depth, params.planar == 0u);
        for (var i = 0u; i < count; i += 2u) {
            yuv[base + i / 2u] = (values[i] << shift) | ((values[i + 1u] << shift) << 16u);
        }
    } else {
        for (var i = 0u; i < count; i += 4u) {
            yuv[base + i / 4u] = values[i] | (values[i + 1u] << 8u) | (values[i + 2u] << 16u)
                | (values[i + 3u] << 24u);
        }
    }
}

// Each invocation writes an 8x2 pixel block: eight luma samples on each row and four Cb, Cr
// samples. Rows are padded to whole blocks, so a block never crosses into the next row.
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x * 8u;
    let y = id.y * 2u;
    if (x >= params.width || y >= params.height) {
        return;
    }

    let sample_bytes = select(1u, 2u, params.depth > 8u);
    var top: array<u32, 8>;
    var bottom: array<u32, 8>;
    var interleaved: array<u32, 8>;
    var cbs: array<u32, 8>;
    var crs: array<u32, 8>;
    for (var pair = 0u; pair < 4u; pair++) {
        let left = x + pair * 2u;
        let a = rgb(left, y);
        let b = rgb(left + 1u, y);
        let c = rgb(left, y + 1u);
        let d = rgb(left + 1u, y + 1u);
        top[pair * 2u] = luma_code(a, vec2<u32>(left, y));
        top[pair * 2u + 1u] = luma_code(b, vec2<u32>(left + 1u, y));
        bottom[pair * 2u] = luma_code(c, vec2<u32>(left, y + 1u));
        bottom[pair * 2u + 1u] = luma_code(d, vec2<u32>(left + 1u, y + 1u));

        let average = (a + b + c + d) * 0.25;
        let average_luma = luma(average);
        let sample = vec2<u32>(left / 2u, y / 2u);
        let cb = to_code(128.0 + 224.0 * (average.b - average_luma) / (2.0 * (1.0 - KB)), sample);
        let cr = to_code(128.0 + 224.0 * (average.r - average_luma) / (2.0 * (1.0 - KR)), sample);
        interleaved[pair * 2u] = cb;
        interleaved[pair * 2u + 1u] = cr;
        cbs[pair] = cb;
        crs[pair] = cr;
    }

    store(y * params.pitch + x * sample_bytes, top, 8u);
    if (y + 1u < params.height) {
        store((y + 1u) * params.pitch + x * sample_bytes, bottom, 8u);
    }
    let chroma_row = y / 2u;
    if (params.planar == 0u) {
        store(params.chroma_offset + chroma_row * params.pitch + x * sample_bytes, interleaved, 8u);
    } else {
        let half = params.pitch / 2u;
        let cb_offset = params.chroma_offset + chroma_row * half + (x / 2u) * sample_bytes;
        store(cb_offset, cbs, 4u);
        store(cb_offset + half * ((params.height + 1u) / 2u), crs, 4u);
    }
}

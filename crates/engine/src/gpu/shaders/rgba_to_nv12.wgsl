// Full-range RGBA to BT.709 limited-range NV12, matching the CPU encoder's colour conversion.
struct Params {
    width: u32,
    height: u32,
    pitch: u32,
    chroma_offset: u32,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> nv12: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

const KR: f32 = 0.2126;
const KB: f32 = 0.0722;

fn rgb(x: u32, y: u32) -> vec3<f32> {
    let clamped = vec2<u32>(min(x, params.width - 1u), min(y, params.height - 1u));
    return textureLoad(source, clamped, 0).rgb;
}

fn luma(color: vec3<f32>) -> f32 {
    return dot(color, vec3<f32>(KR, 1.0 - KR - KB, KB));
}

fn to_byte(value: f32) -> u32 {
    return u32(clamp(floor(value + 0.5), 0.0, 255.0));
}

fn luma_byte(color: vec3<f32>) -> u32 {
    return to_byte(16.0 + 219.0 * luma(color));
}

// Each invocation writes a 4x2 pixel block: four luma bytes on each row and two CbCr pairs.
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x * 4u;
    let y = id.y * 2u;
    if (x >= params.width || y >= params.height) {
        return;
    }

    var top = 0u;
    var bottom = 0u;
    var chroma = 0u;
    for (var pair = 0u; pair < 2u; pair++) {
        let left = x + pair * 2u;
        let a = rgb(left, y);
        let b = rgb(left + 1u, y);
        let c = rgb(left, y + 1u);
        let d = rgb(left + 1u, y + 1u);
        let shift = pair * 16u;
        top |= (luma_byte(a) | (luma_byte(b) << 8u)) << shift;
        bottom |= (luma_byte(c) | (luma_byte(d) << 8u)) << shift;

        let average = (a + b + c + d) * 0.25;
        let average_luma = luma(average);
        let cb = 128.0 + 224.0 * (average.b - average_luma) / (2.0 * (1.0 - KB));
        let cr = 128.0 + 224.0 * (average.r - average_luma) / (2.0 * (1.0 - KR));
        chroma |= (to_byte(cb) | (to_byte(cr) << 8u)) << shift;
    }

    let row_words = params.pitch / 4u;
    let column = x / 4u;
    nv12[y * row_words + column] = top;
    if (y + 1u < params.height) {
        nv12[(y + 1u) * row_words + column] = bottom;
    }
    nv12[params.chroma_offset / 4u + (y / 2u) * row_words + column] = chroma;
}

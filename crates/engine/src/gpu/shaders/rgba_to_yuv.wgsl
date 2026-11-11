// The working space, linear and premultiplied, to BT.709 limited-range YUV, matching the CPU
// encoder's colour conversion, at 8 or 10 bits: 4:2:0, or 4:4:4 with `full_chroma`. Without
// `alpha` the colour is the frame over black. With it the colour is straight, unpremultiplied in
// linear light as a PNG's is, and black where the alpha code is 0, and an alpha plane follows at
// `alpha_offset`, full range and undithered. 4:2:0 chroma is the 2x2 block's average of encoded
// colour weighted by alpha, which is the plain average when opaque. Each colour code is rounded
// once, from float colour, with the ordered dither when `dither` is set. Samples above 8 bits take
// two little-endian bytes: in the high bits for the two-plane layout (P010), in the low bits for
// the planar ones. Requires the colour and dither chunks prepended.
struct Params {
    width: u32,
    height: u32,
    // Bytes between luma rows, between alpha rows and between the chroma rows of the two-plane
    // and 4:4:4 layouts. Planar 4:2:0 chroma rows are half that apart.
    pitch: u32,
    chroma_offset: u32,
    dither: u32,
    depth: u32,
    planar: u32,
    full_chroma: u32,
    alpha: u32,
    alpha_offset: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> yuv: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

const KR: f32 = 0.2126;
const KB: f32 = 0.0722;

// A pixel's encoded colour and its weight in a chroma average, which is its alpha.
struct Pixel {
    color: vec3<f32>,
    alpha: f32,
}

// The alpha code of coverage `alpha`, full range at `params.depth` bits.
fn alpha_code(alpha: f32) -> u32 {
    return u32(floor(alpha * f32((1u << params.depth) - 1u) + 0.5));
}

fn pixel(x: u32, y: u32) -> Pixel {
    let clamped = vec2<u32>(min(x, params.width - 1u), min(y, params.height - 1u));
    let texel = textureLoad(source, clamped, 0);
    let light = max(texel.rgb, vec3<f32>(0.0));
    if (params.alpha == 0u) {
        // Premultiplied colour over black is the colour itself.
        return Pixel(clamp(lumen_linear_to_srgb(light), vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    }
    let a = clamp(texel.a, 0.0, 1.0);
    if (alpha_code(a) == 0u) {
        return Pixel(vec3<f32>(0.0), 0.0);
    }
    return Pixel(lumen_straight_srgb(light, a), a);
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

fn cb_code(color: vec3<f32>, sample: vec2<u32>) -> u32 {
    return to_code(128.0 + 224.0 * (color.b - luma(color)) / (2.0 * (1.0 - KB)), sample);
}

fn cr_code(color: vec3<f32>, sample: vec2<u32>) -> u32 {
    return to_code(128.0 + 224.0 * (color.r - luma(color)) / (2.0 * (1.0 - KR)), sample);
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

// Each invocation writes an 8x2 pixel block: eight luma samples on each row, the block's chroma
// and, with alpha, eight alpha samples on each row. Rows are padded to whole blocks, so a block
// never crosses into the next row.
@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x = id.x * 8u;
    let y = id.y * 2u;
    if (x >= params.width || y >= params.height) {
        return;
    }

    let sample_bytes = select(1u, 2u, params.depth > 8u);
    let has_bottom = y + 1u < params.height;
    var upper: array<Pixel, 8>;
    var lower: array<Pixel, 8>;
    var top: array<u32, 8>;
    var bottom: array<u32, 8>;
    var alpha_top: array<u32, 8>;
    var alpha_bottom: array<u32, 8>;
    for (var i = 0u; i < 8u; i++) {
        upper[i] = pixel(x + i, y);
        lower[i] = pixel(x + i, y + 1u);
        top[i] = luma_code(upper[i].color, vec2<u32>(x + i, y));
        bottom[i] = luma_code(lower[i].color, vec2<u32>(x + i, y + 1u));
        alpha_top[i] = alpha_code(upper[i].alpha);
        alpha_bottom[i] = alpha_code(lower[i].alpha);
    }

    store(y * params.pitch + x * sample_bytes, top, 8u);
    if (has_bottom) {
        store((y + 1u) * params.pitch + x * sample_bytes, bottom, 8u);
    }
    if (params.alpha != 0u) {
        store(params.alpha_offset + y * params.pitch + x * sample_bytes, alpha_top, 8u);
        if (has_bottom) {
            store(params.alpha_offset + (y + 1u) * params.pitch + x * sample_bytes, alpha_bottom, 8u);
        }
    }

    if (params.full_chroma != 0u) {
        var cb_top: array<u32, 8>;
        var cr_top: array<u32, 8>;
        var cb_bottom: array<u32, 8>;
        var cr_bottom: array<u32, 8>;
        for (var i = 0u; i < 8u; i++) {
            cb_top[i] = cb_code(upper[i].color, vec2<u32>(x + i, y));
            cr_top[i] = cr_code(upper[i].color, vec2<u32>(x + i, y));
            cb_bottom[i] = cb_code(lower[i].color, vec2<u32>(x + i, y + 1u));
            cr_bottom[i] = cr_code(lower[i].color, vec2<u32>(x + i, y + 1u));
        }
        let cb_plane = params.chroma_offset;
        let cr_plane = params.chroma_offset + params.pitch * params.height;
        let row = y * params.pitch + x * sample_bytes;
        store(cb_plane + row, cb_top, 8u);
        store(cr_plane + row, cr_top, 8u);
        if (has_bottom) {
            store(cb_plane + row + params.pitch, cb_bottom, 8u);
            store(cr_plane + row + params.pitch, cr_bottom, 8u);
        }
        return;
    }

    var interleaved: array<u32, 8>;
    var cbs: array<u32, 8>;
    var crs: array<u32, 8>;
    for (var pair = 0u; pair < 4u; pair++) {
        let a = upper[pair * 2u];
        let b = upper[pair * 2u + 1u];
        let c = lower[pair * 2u];
        let d = lower[pair * 2u + 1u];
        // sum(alpha * colour) / sum(alpha): premultiplied by alpha, averaged, unpremultiplied.
        let weight = a.alpha + b.alpha + c.alpha + d.alpha;
        var average = vec3<f32>(0.0);
        if (weight > 0.0) {
            average = (a.color * a.alpha + b.color * b.alpha + c.color * c.alpha + d.color * d.alpha)
                / weight;
        }
        let sample = vec2<u32>((x + pair * 2u) / 2u, y / 2u);
        let cb = cb_code(average, sample);
        let cr = cr_code(average, sample);
        interleaved[pair * 2u] = cb;
        interleaved[pair * 2u + 1u] = cr;
        cbs[pair] = cb;
        crs[pair] = cr;
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

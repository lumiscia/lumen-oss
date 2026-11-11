// YUV planes to linear, premultiplied RGBA, rotated upright and stretched to the output by nearest
// neighbour. Each pixel uses its own luma and the chroma sample covering it, as swscale does. The
// matrix gives encoded R'G'B'. SDR is decoded with the sRGB curve; PQ and HLG are decoded to
// display light in nits and tone-mapped to SDR, with 203 nits as SDR white. Linear light then
// goes from the source primaries to BT.709, and colour outside BT.709 is clipped at zero.
// Requires the colour chunk.
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
    // 0 for SDR, 1 for PQ, 2 for HLG.
    transfer: u32,
    _pad: u32,
    // The source primaries to BT.709, in linear light.
    gamut: mat3x3<f32>,
    // The BT.2390 EETF: x is the PQ code of the HDR peak, y the PQ code of SDR white as a fraction
    // of x, z the knee in the same units.
    tone: vec4<f32>,
}

const SDR_WHITE_NITS: f32 = 203.0;
const HLG_PEAK_NITS: f32 = 1000.0;

// SMPTE ST 2084: PQ code values to nits.
fn pq_to_nits(e: vec3<f32>) -> vec3<f32> {
    let p = pow(clamp(e, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(32.0 / 2523.0));
    let num = max(p - 0.8359375, vec3<f32>(0.0));
    return 10000.0 * pow(num / (18.8515625 - 18.6875 * p), vec3<f32>(16384.0 / 2610.0));
}

fn nits_to_pq(nits: f32) -> f32 {
    let y = pow(clamp(nits / 10000.0, 0.0, 1.0), 2610.0 / 16384.0);
    return pow((0.8359375 + 18.8515625 * y) / (1.0 + 18.6875 * y), 2523.0 / 32.0);
}

// ARIB STD-B67: HLG code values to scene light in 0..1.
fn hlg_to_scene(e: vec3<f32>) -> vec3<f32> {
    let low = e * e / 3.0;
    let high = (exp((e - 0.55991073) / 0.17883277) + 0.28466892) / 12.0;
    return select(high, low, e <= vec3<f32>(0.5));
}

// The HLG OOTF for a 1000-nit display, system gamma 1.2, on BT.709 linear scene light: luminance
// is the same in any primaries, so this follows the primaries conversion.
fn hlg_ootf(scene: vec3<f32>) -> vec3<f32> {
    let luminance = max(dot(scene, vec3<f32>(0.2126, 0.7152, 0.0722)), 0.0);
    return HLG_PEAK_NITS * pow(luminance, 0.2) * scene;
}

// Display light in nits to SDR, where SDR white is 1. The BT.2390 EETF maps max(R, G, B) from the
// HDR peak's range onto SDR white's, and the gain it gives scales all three channels.
fn hdr_to_sdr(nits: vec3<f32>) -> vec3<f32> {
    let rgb = max(nits, vec3<f32>(0.0));
    let peak = max(rgb.r, max(rgb.g, rgb.b));
    let ks = params.tone.z;
    let e = min(nits_to_pq(peak) / params.tone.x, 1.0);
    if (peak <= 0.0 || e <= ks) {
        return rgb / SDR_WHITE_NITS;
    }
    let t = (e - ks) / (1.0 - ks);
    let t2 = t * t;
    let t3 = t2 * t;
    let mapped = (2.0 * t3 - 3.0 * t2 + 1.0) * ks + (t3 - 2.0 * t2 + t) * (1.0 - ks)
        + (-2.0 * t3 + 3.0 * t2) * params.tone.y;
    let mapped_nits = pq_to_nits(vec3<f32>(mapped * params.tone.x)).x;
    return rgb * (mapped_nits / (peak * SDR_WHITE_NITS));
}

@group(0) @binding(0) var luma: texture_2d<u32>;
// Cb and Cr pairs when interleaved, otherwise Cb.
@group(0) @binding(1) var chroma_u: texture_2d<u32>;
@group(0) @binding(2) var chroma_v: texture_2d<u32>;
@group(0) @binding(3) var alpha: texture_2d<u32>;
@group(0) @binding(4) var<uniform> params: Params;
@group(0) @binding(5) var output: texture_storage_2d<rgba16float, write>;

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
    // Premultiplied colour may be brighter than its alpha: its straight colour is then past
    // white, which the curve extends to, as RGBA frames do.
    var straight = rgb;
    if (params.premultiplied != 0u) {
        straight = select(vec3<f32>(0.0), rgb / a, a > 0.0);
    }
    var linear: vec3<f32>;
    switch params.transfer {
        case 1u: { linear = hdr_to_sdr(params.gamut * pq_to_nits(straight)); }
        case 2u: { linear = hdr_to_sdr(hlg_ootf(params.gamut * hlg_to_scene(straight))); }
        default: { linear = max(params.gamut * lumen_srgb_to_linear(straight), vec3<f32>(0.0)); }
    }
    textureStore(output, id.xy, vec4<f32>(linear * a, a));
}

struct MergeParams {
    opacity: f32,
    blend_mode: u32,
    has_mask: u32,
    // Bit 0: base, bit 1: overlay, bit 2: mask is straight alpha.
    straight_inputs: u32,
    mask_mode: u32,
    invert: u32,
    feather: f32,
    _pad: u32,
}

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_tex: texture_2d<f32>;
@group(0) @binding(2) var mask_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: MergeParams;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn sample_or_transparent(tex: texture_2d<f32>, pixel: vec2<u32>) -> vec4<f32> {
    let size = textureDimensions(tex);
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, vec2<i32>(pixel), 0);
}

const MASK_MODE_LUMA: u32 = 1u;
const REC709_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);
const GOLDEN_ANGLE: f32 = 2.39996323;
const MAX_FEATHER_TAPS: u32 = 256u;

// Matte value of one mask texel. Luma is taken over black: the Rec. 709 weights applied to
// the unpremultiplied colour, scaled by alpha, which equals the weights applied to the
// premultiplied colour. Antialiased edges therefore keep their coverage.
fn matte_at(pixel: vec2<i32>) -> f32 {
    let texel = textureLoad(mask_tex, pixel, 0);
    if (params.mask_mode == MASK_MODE_LUMA) {
        let premultiplied = to_premultiplied_alpha(texel, (params.straight_inputs & 4u) != 0u);
        return dot(premultiplied.rgb, REC709_LUMA);
    }
    return texel.a;
}

// Mean matte over a disc around the pixel, sampled on a golden-angle spiral so every tap
// covers an equal area. Taps outside the mask clamp to its nearest edge texel.
fn feathered_matte(pixel: vec2<u32>) -> f32 {
    let size = textureDimensions(mask_tex);
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return 0.0;
    }
    let center = vec2<i32>(pixel);
    if (params.feather <= 0.0) {
        return matte_at(center);
    }
    let taps = min(MAX_FEATHER_TAPS, max(8u, u32(ceil(params.feather * params.feather * 0.8))));
    let max_texel = vec2<i32>(size) - vec2<i32>(1);
    var sum = 0.0;
    for (var i = 0u; i < taps; i++) {
        let radius = params.feather * sqrt((f32(i) + 0.5) / f32(taps));
        let angle = f32(i) * GOLDEN_ANGLE;
        let offset = vec2<i32>(round(radius * vec2<f32>(cos(angle), sin(angle))));
        sum += matte_at(clamp(center + offset, vec2<i32>(0), max_texel));
    }
    return sum / f32(taps);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = id.xy;
    let base = to_premultiplied_alpha(sample_or_transparent(base_tex, pixel), (params.straight_inputs & 1u) != 0u);
    let overlay = to_premultiplied_alpha(sample_or_transparent(overlay_tex, pixel), (params.straight_inputs & 2u) != 0u);
    var mask = 1.0;
    if (params.has_mask == 1u) {
        mask = clamp(feathered_matte(pixel), 0.0, 1.0);
        if (params.invert != 0u) {
            mask = 1.0 - mask;
        }
    }
    let opacity = clamp(params.opacity, 0.0, 1.0) * mask;
    textureStore(output_tex, vec2<i32>(pixel), composite(base, overlay * opacity, params.blend_mode));
}

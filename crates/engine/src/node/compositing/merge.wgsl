struct MergeParams {
    opacity: f32,
    blend_mode: u32,
    // 0: no mask, 1: the feathered mask, 2: the mask as is.
    mask_source: u32,
    // Bit 0: base, bit 1: overlay, bit 2: mask (applied when it is read as is).
    straight_inputs: u32,
    mask_mode: u32,
    invert: u32,
}

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_tex: texture_2d<f32>;
@group(0) @binding(2) var mask_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: MergeParams;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(5) var raw_mask_tex: texture_2d<f32>;

fn sample_or_transparent(tex: texture_2d<f32>, pixel: vec2<u32>) -> vec4<f32> {
    let size = textureDimensions(tex);
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, vec2<i32>(pixel), 0);
}

const MASK_MODE_LUMA: u32 = 1u;
const REC709_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// Matte value of one texel of the (possibly blurred) mask. Luma is taken over black: the
// Rec. 709 weights applied to the premultiplied colour, so antialiased edges keep their
// coverage. Both alpha and luma are linear in the premultiplied channels, so the blur can
// run on the colour and be reduced to a matte afterwards.
fn matte_of(texel: vec4<f32>) -> f32 {
    if (params.mask_mode == MASK_MODE_LUMA) {
        return dot(texel.rgb, REC709_LUMA);
    }
    return texel.a;
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
    if (params.mask_source != 0u) {
        var texel = sample_or_transparent(mask_tex, pixel);
        if (params.mask_source == 2u) {
            texel = to_premultiplied_alpha(sample_or_transparent(raw_mask_tex, pixel), (params.straight_inputs & 4u) != 0u);
        }
        mask = clamp(matte_of(texel), 0.0, 1.0);
        if (params.invert != 0u) {
            mask = 1.0 - mask;
        }
    }
    let opacity = clamp(params.opacity, 0.0, 1.0) * mask;
    textureStore(output_tex, vec2<i32>(pixel), composite(base, overlay * opacity, params.blend_mode));
}

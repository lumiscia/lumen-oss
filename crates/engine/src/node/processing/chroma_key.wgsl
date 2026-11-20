struct ChromaKeyParams {
    // The key's chromaticity in `xy`, then the tolerance and softness.
    key: vec4<f32>,
    // The spill amount in `x`.
    spill: vec4<f32>,
    // The key's dominant channel, then 1 for matte only.
    modes: vec4<u32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ChromaKeyParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

// Below this brightness a pixel's chromaticity is noise, so the pixel is never keyed.
const KEY_FLOOR: f32 = 0.002;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = unpremultiply(textureLoad(input_tex, vec2<i32>(id.xy), 0));
    let rgb = max(color.rgb, vec3<f32>(0.0));
    let sum = rgb.x + rgb.y + rgb.z;
    let tolerance = params.key.z;
    let softness = params.key.w;

    var keep = 1.0;
    if (sum >= KEY_FLOOR) {
        let distance = length(rgb.xy / sum - params.key.xy);
        if (softness > 0.0) {
            keep = smoothstep(tolerance, tolerance + softness, distance);
        } else {
            keep = select(0.0, 1.0, distance > tolerance);
        }
    }
    let alpha = color.a * keep;

    if (params.modes.y != 0u) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(alpha));
        return;
    }

    // Limit the key's dominant channel to the larger of the other two.
    var suppressed = color.rgb;
    let dominant = params.modes.x;
    let others = max(suppressed[(dominant + 1u) % 3u], suppressed[(dominant + 2u) % 3u]);
    suppressed[dominant] = mix(suppressed[dominant], min(suppressed[dominant], others), params.spill.x);
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(premultiply(vec4<f32>(suppressed, alpha))));
}

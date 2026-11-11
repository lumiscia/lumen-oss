struct ExposureParams {
    exposure: f32,
    contrast: f32,
    offset: f32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ExposureParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    // Exposure scales linear light, which may go past white. Offset and contrast, which pivots on
    // mid-grey, act on the sRGB-encoded result.
    let color = unpremultiply(textureLoad(input_tex, vec2<i32>(id.xy), 0));
    let exposed = lumen_linear_to_srgb(color.rgb * exp2(params.exposure));
    let encoded = (exposed + vec3<f32>(params.offset) - vec3<f32>(0.5)) * params.contrast + vec3<f32>(0.5);
    let rgb = max(lumen_srgb_to_linear(encoded), vec3<f32>(0.0));
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(premultiply(vec4<f32>(rgb, color.a))));
}

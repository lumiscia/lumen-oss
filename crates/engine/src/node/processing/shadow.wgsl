// `color.a` is 1.0 when the source raster is straight alpha. The shadow color
// alpha is already folded into `values.w`, the only value used here.
struct ShadowParams {
    color: vec4<f32>,
    values: vec4<f32>,
}

@group(0) @binding(0) var blurred_tex: texture_2d<f32>;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: ShadowParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn composite_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = vec2<i32>(id.xy);
    let blurred = textureLoad(blurred_tex, pixel, 0);
    let source = to_premultiplied_alpha(textureLoad(source_tex, pixel, 0), params.color.a >= 0.5);
    let shadow_alpha = blurred.a * clamp(params.values.w, 0.0, 1.0);
    let shadow = vec4<f32>(params.color.rgb * shadow_alpha, shadow_alpha);
    let out_alpha = source.a + shadow.a * (1.0 - source.a);
    let out_rgb = source.rgb + shadow.rgb * (1.0 - source.a);
    textureStore(output_tex, pixel, vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(out_alpha, 0.0, 1.0)));
}

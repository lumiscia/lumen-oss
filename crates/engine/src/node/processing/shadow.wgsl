// `color.a` is 1.0 when the source raster is straight alpha. The shadow color
// alpha is already folded into `values.w`. `values.z` is the Gaussian sigma.
struct ShadowParams {
    color: vec4<f32>,
    values: vec4<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var source_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: ShadowParams;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var intermediate_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(intermediate_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let offset = round(params.values.xy);
    let center = vec2<f32>(id.xy) + vec2<f32>(0.5) - offset;
    let blurred = gaussian_blur_1d(input_tex, linear_sampler, center, vec2<f32>(1.0, 0.0), params.values.z, false, true);
    textureStore(intermediate_tex, vec2<i32>(id.xy), vec4<f32>(0.0, 0.0, 0.0, blurred.a));
}

@compute @workgroup_size(8, 8, 1)
fn vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = vec2<i32>(id.xy);
    let center = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let blurred = gaussian_blur_1d(input_tex, linear_sampler, center, vec2<f32>(0.0, 1.0), params.values.z, false, true);

    let source = to_premultiplied_alpha(textureLoad(source_tex, pixel, 0), params.color.a >= 0.5);
    let shadow_alpha = blurred.a * clamp(params.values.w, 0.0, 1.0);
    let shadow = vec4<f32>(params.color.rgb * shadow_alpha, shadow_alpha);
    let out_alpha = source.a + shadow.a * (1.0 - source.a);
    let out_rgb = source.rgb + shadow.rgb * (1.0 - source.a);
    textureStore(output_tex, pixel, vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(out_alpha, 0.0, 1.0)));
}

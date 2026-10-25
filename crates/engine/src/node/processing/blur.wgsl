// `values.x` is the Gaussian sigma in pixels. `values.y` is 1.0 when the source
// is straight alpha; the result is always premultiplied.
struct BlurParams {
    values: vec4<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: BlurParams;
@group(0) @binding(3) var linear_sampler: sampler;
@group(0) @binding(4) var intermediate_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(5) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(intermediate_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let center = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let color = gaussian_blur_1d(input_tex, linear_sampler, center, vec2<f32>(1.0, 0.0), params.values.x, params.values.y >= 0.5, false);
    textureStore(intermediate_tex, vec2<i32>(id.xy), color);
}

@compute @workgroup_size(8, 8, 1)
fn vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let center = vec2<f32>(id.xy) + vec2<f32>(0.5);
    let color = gaussian_blur_1d(input_tex, linear_sampler, center, vec2<f32>(0.0, 1.0), params.values.x, false, false);
    textureStore(output_tex, vec2<i32>(id.xy), color);
}

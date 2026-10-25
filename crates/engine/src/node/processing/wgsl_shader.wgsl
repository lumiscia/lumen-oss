// Custom shaders receive the input raster as stored, which is premultiplied
// alpha unless an upstream Alpha Premultiply node converted it, and must write
// output in the same representation. This default template expects
// premultiplied input and tints its straight colour by `values.xyz`.

struct ShaderParams {
    values: vec4<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ShaderParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = unpremultiply(textureLoad(input_tex, vec2<i32>(id.xy), 0));
    let rgb = clamp(color.rgb + params.values.xyz, vec3<f32>(0.0), vec3<f32>(1.0));
    textureStore(output_tex, vec2<i32>(id.xy), premultiply(vec4<f32>(rgb, color.a)));
}

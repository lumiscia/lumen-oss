// Custom shaders receive every input as linear light with premultiplied alpha,
// and their output is read the same way. This default template adds
// `values.xyz` to the straight linear colour of input0. `input0`, `output`,
// `input_sampler`, `lumen` and the `lumen_srgb_to_linear` and
// `lumen_linear_to_srgb` helpers come from the prelude.

struct ShaderParams {
    values: vec4<f32>,
}

@group(0) @binding(1) var<uniform> params: ShaderParams;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = unpremultiply(textureLoad(input0, vec2<i32>(id.xy), 0));
    let rgb = max(color.rgb + params.values.xyz, vec3<f32>(0.0));
    textureStore(output, vec2<i32>(id.xy), premultiply(vec4<f32>(rgb, color.a)));
}

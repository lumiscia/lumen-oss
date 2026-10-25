struct OpacityParams {
    values: vec4<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: OpacityParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    let opacity = clamp(params.values.x, 0.0, 1.0);
    let premultiplied = params.values.y >= 0.5;
    let rgb_multiplier = select(1.0, opacity, premultiplied);
    textureStore(
        output_tex,
        vec2<i32>(id.xy),
        vec4<f32>(color.rgb * rgb_multiplier, color.a * opacity),
    );
}

struct AlphaPremultiplyParams {
    operation: f32,
    _pad: vec3<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: AlphaPremultiplyParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    var rgb = color.rgb * color.a;
    if (params.operation >= 0.5) {
        rgb = select(vec3<f32>(0.0), color.rgb / color.a, color.a > 0.000001);
    }
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), color.a));
}

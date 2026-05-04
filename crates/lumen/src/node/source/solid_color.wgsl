struct ColorParams {
    color: vec4<f32>,
}

@group(0) @binding(0) var<uniform> params: ColorParams;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    textureStore(output_tex, vec2<i32>(id.xy), params.color);
}

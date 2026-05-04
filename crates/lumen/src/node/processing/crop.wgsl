struct CropParams {
    origin: vec2<i32>,
    size: vec2<u32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: CropParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let output_size = textureDimensions(output_tex);
    if (id.x >= output_size.x || id.y >= output_size.y) {
        return;
    }

    if (id.x >= params.size.x || id.y >= params.size.y) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    let input_size = textureDimensions(input_tex);
    let source = params.origin + vec2<i32>(id.xy);
    if (source.x < 0 || source.y < 0 || source.x >= i32(input_size.x) || source.y >= i32(input_size.y)) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    textureStore(output_tex, vec2<i32>(id.xy), textureLoad(input_tex, source, 0));
}

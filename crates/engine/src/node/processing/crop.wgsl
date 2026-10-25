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

    let pixel = vec2<i32>(id.xy);
    let local = pixel - params.origin;
    if (local.x < 0 || local.y < 0 || local.x >= i32(params.size.x) || local.y >= i32(params.size.y)) {
        textureStore(output_tex, pixel, vec4<f32>(0.0));
        return;
    }

    textureStore(output_tex, pixel, textureLoad(input_tex, pixel, 0));
}

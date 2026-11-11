struct CropParams {
    origin: vec2<i32>,
    size: vec2<u32>,
    source_origin: vec2<i32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: CropParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let output_size = textureDimensions(output_tex);
    if (id.x >= output_size.x || id.y >= output_size.y) {
        return;
    }

    let pixel = vec2<i32>(id.xy);
    if (any(pixel < params.origin)) {
        textureStore(output_tex, pixel, vec4<f32>(0.0));
        return;
    }
    // Both operands are known to satisfy pixel >= origin, so the wrapped u32 difference is exact.
    let local = bitcast<vec2<u32>>(pixel) - bitcast<vec2<u32>>(params.origin);
    if (any(local >= params.size)) {
        textureStore(output_tex, pixel, vec4<f32>(0.0));
        return;
    }

    // The source stores only its data, which is transparent around it.
    let texel = pixel - params.source_origin;
    let source_size = vec2<i32>(textureDimensions(input_tex));
    if (any(texel < vec2<i32>(0)) || any(texel >= source_size)) {
        textureStore(output_tex, pixel, vec4<f32>(0.0));
        return;
    }

    textureStore(output_tex, pixel, textureLoad(input_tex, texel, 0));
}

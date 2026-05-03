@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let output_size = textureDimensions(output_tex);
    if (id.x >= output_size.x || id.y >= output_size.y) {
        return;
    }

    let source_size = textureDimensions(source_tex);
    let pixel = id.xy;
    var color = vec4<f32>(0.0);
    if (pixel.x < source_size.x && pixel.y < source_size.y) {
        color = textureLoad(source_tex, vec2<i32>(pixel), 0);
    }
    textureStore(output_tex, vec2<i32>(pixel), color);
}

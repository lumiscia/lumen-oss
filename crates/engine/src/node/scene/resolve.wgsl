// Averages a supersampled scene, already tone mapped, down to the output and stores it as a Lumen
// raster: premultiplied linear light.

struct Params {
    supersample: u32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let ss = max(params.supersample, 1u);
    var sum = vec4<f32>(0.0);
    for (var y = 0u; y < ss; y += 1u) {
        for (var x = 0u; x < ss; x += 1u) {
            sum += textureLoad(input_tex, vec2<i32>(id.xy * ss + vec2<u32>(x, y)), 0);
        }
    }
    let color = sum / f32(ss * ss);
    if (color.a <= 1e-6) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(max(color.rgb, vec3<f32>(0.0)), min(color.a, 1.0)));
}

// Averages a supersampled scene's mattes down to the output as the resolve averages its colour,
// and stores them as they are: the first target's red, green and blue, then the second target's
// red as alpha. Each is a channel's coverage, so none is premultiplied by another.

struct Params {
    supersample: u32,
    // 1 when the second target was drawn, for a fourth name.
    second: u32,
    _pad0: u32,
    _pad1: u32,
}

@group(0) @binding(0) var first_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var second_tex: texture_2d<f32>;

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
            let at = vec2<i32>(id.xy * ss + vec2<u32>(x, y));
            let first = textureLoad(first_tex, at, 0);
            var fourth = 0.0;
            if (params.second != 0u) {
                fourth = textureLoad(second_tex, at, 0).r;
            }
            sum += vec4<f32>(first.rgb, fourth);
        }
    }
    let matte = clamp(sum / f32(ss * ss), vec4<f32>(0.0), vec4<f32>(1.0));
    textureStore(output_tex, vec2<i32>(id.xy), matte);
}

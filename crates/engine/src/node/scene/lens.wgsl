// Sums the views through a camera's lens, each a supersampled scene averaged down to the output as
// `resolve.wgsl` and `matte.wgsl` average one, in full float. Each sample but the last adds itself
// to the sum of those before it; the last stores the mean of them all as the resolves store a
// single view.

struct Params {
    supersample: u32,
    // 1 when the second matte target was drawn, for a fourth name.
    second: u32,
    // 1 for the first sample, which has no sum before it.
    first: u32,
    // 1 over the number of samples.
    weight: f32,
}

@group(0) @binding(0) var sample_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;
// The second matte target; colour doesn't read it.
@group(0) @binding(3) var second_tex: texture_2d<f32>;
@group(0) @binding(4) var sum_tex: texture_2d<f32>;
@group(0) @binding(5) var next_tex: texture_storage_2d<rgba32float, write>;

fn color_at(id: vec2<u32>) -> vec4<f32> {
    let ss = max(params.supersample, 1u);
    var sum = vec4<f32>(0.0);
    for (var y = 0u; y < ss; y += 1u) {
        for (var x = 0u; x < ss; x += 1u) {
            sum += textureLoad(sample_tex, vec2<i32>(id * ss + vec2<u32>(x, y)), 0);
        }
    }
    return sum / f32(ss * ss);
}

fn matte_at(id: vec2<u32>) -> vec4<f32> {
    let ss = max(params.supersample, 1u);
    var sum = vec4<f32>(0.0);
    for (var y = 0u; y < ss; y += 1u) {
        for (var x = 0u; x < ss; x += 1u) {
            let at = vec2<i32>(id * ss + vec2<u32>(x, y));
            let first = textureLoad(sample_tex, at, 0);
            var fourth = 0.0;
            if (params.second != 0u) {
                fourth = textureLoad(second_tex, at, 0).r;
            }
            sum += vec4<f32>(first.rgb, fourth);
        }
    }
    return sum / f32(ss * ss);
}

fn summed(id: vec2<u32>, value: vec4<f32>) -> vec4<f32> {
    if (params.first != 0u) {
        return value;
    }
    return value + textureLoad(sum_tex, vec2<i32>(id), 0);
}

fn outside(id: vec2<u32>, size: vec2<u32>) -> bool {
    return id.x >= size.x || id.y >= size.y;
}

@compute @workgroup_size(8, 8, 1)
fn cs_color_add(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id.xy, textureDimensions(next_tex))) {
        return;
    }
    textureStore(next_tex, vec2<i32>(id.xy), summed(id.xy, color_at(id.xy)));
}

@compute @workgroup_size(8, 8, 1)
fn cs_color_last(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id.xy, textureDimensions(output_tex))) {
        return;
    }
    let color = summed(id.xy, color_at(id.xy)) * params.weight;
    if (color.a <= 1e-6) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(max(color.rgb, vec3<f32>(0.0)), min(color.a, 1.0)));
}

@compute @workgroup_size(8, 8, 1)
fn cs_matte_add(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id.xy, textureDimensions(next_tex))) {
        return;
    }
    textureStore(next_tex, vec2<i32>(id.xy), summed(id.xy, matte_at(id.xy)));
}

@compute @workgroup_size(8, 8, 1)
fn cs_matte_last(@builtin(global_invocation_id) id: vec3<u32>) {
    if (outside(id.xy, textureDimensions(output_tex))) {
        return;
    }
    let matte = summed(id.xy, matte_at(id.xy)) * params.weight;
    textureStore(output_tex, vec2<i32>(id.xy), clamp(matte, vec4<f32>(0.0), vec4<f32>(1.0)));
}

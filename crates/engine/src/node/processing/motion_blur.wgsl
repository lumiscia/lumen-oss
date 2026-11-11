struct Params {
    /// Added to an output texel to find the same pixel in the running sum and in the sample.
    sum_offset: vec2<i32>,
    sample_offset: vec2<i32>,
    has_sum: u32,
    samples: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var sum_tex: texture_2d<f32>;
@group(0) @binding(1) var sample_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: Params;
@group(0) @binding(3) var sum_out: texture_storage_2d<rgba32float, write>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba16float, write>;

fn inside(coord: vec2<i32>, size: vec2<u32>) -> bool {
    return all(coord >= vec2<i32>(0)) && all(coord < vec2<i32>(size));
}

/// Every sample so far, summed.
fn total(id: vec2<u32>) -> vec4<f32> {
    var sum = vec4<f32>(0.0);
    let at_sum = vec2<i32>(id) + params.sum_offset;
    if (params.has_sum != 0u && inside(at_sum, textureDimensions(sum_tex))) {
        sum = textureLoad(sum_tex, at_sum, 0);
    }
    let at_sample = vec2<i32>(id) + params.sample_offset;
    if (inside(at_sample, textureDimensions(sample_tex))) {
        sum += textureLoad(sample_tex, at_sample, 0);
    }
    return sum;
}

@compute @workgroup_size(8, 8, 1)
fn accumulate_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= textureDimensions(sum_out))) {
        return;
    }
    textureStore(sum_out, id.xy, total(id.xy));
}

// `value` at the nearest half float, so that storing it doesn't depend on how the device rounds.
// The mean of equal samples is then that sample exactly, though the division may be a few ulps off.
fn nearest_half(value: vec4<f32>) -> vec4<f32> {
    let bits = bitcast<vec4<u32>>(value);
    let normal = bitcast<vec4<f32>>((bits + vec4<u32>(0x1000u)) & vec4<u32>(0xffffe000u));
    // Below the smallest normal half, halves are whole multiples of 2^-24.
    let subnormal = round(value * 16777216.0) / 16777216.0;
    return select(normal, subnormal, abs(value) < vec4<f32>(6.1035156e-5));
}

// Adds the last sample and writes the mean to the working space.
@compute @workgroup_size(8, 8, 1)
fn resolve_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id.xy >= textureDimensions(output_tex))) {
        return;
    }
    let mean = nearest_half(total(id.xy) / f32(params.samples));
    textureStore(output_tex, id.xy, vec4<f32>(mean.rgb, clamp(mean.a, 0.0, 1.0)));
}

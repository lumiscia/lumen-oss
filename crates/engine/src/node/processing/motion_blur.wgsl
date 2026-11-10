struct Params {
    /// Added to an output texel to find the same pixel in the running sum and in the sample.
    sum_offset: vec2<i32>,
    sample_offset: vec2<i32>,
    has_sum: u32,
    samples: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var sum_tex: texture_2d<u32>;
@group(0) @binding(1) var sample_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: Params;

fn inside(coord: vec2<i32>, size: vec2<u32>) -> bool {
    return all(coord >= vec2<i32>(0)) && all(coord < vec2<i32>(size));
}

/// The 8-bit premultiplied channels of every sample so far, summed.
fn total(id: vec2<u32>) -> vec4<u32> {
    var sum = vec4<u32>(0u);
    let at_sum = vec2<i32>(id) + params.sum_offset;
    if (params.has_sum != 0u && inside(at_sum, textureDimensions(sum_tex))) {
        sum = textureLoad(sum_tex, at_sum, 0);
    }
    let at_sample = vec2<i32>(id) + params.sample_offset;
    if (inside(at_sample, textureDimensions(sample_tex))) {
        sum += vec4<u32>(round(textureLoad(sample_tex, at_sample, 0) * 255.0));
    }
    return sum;
}

// The sRGB transfer function, shared by engine shaders and the `wgsl_shader` prelude. Rasters hold
// linear light; JSON colours, images, video and the output are sRGB-encoded. Values outside 0..1
// follow the curve mirrored about zero, so headroom above 1 survives a round trip.

fn lumen_srgb_to_linear(encoded: vec3<f32>) -> vec3<f32> {
    let c = abs(encoded);
    let linear = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return sign(encoded) * linear;
}

// `color` with every channel finite, for nodes whose arithmetic can overflow a half float or make
// NaN: NaN, tested by its bits since an optimised comparison may not see it, becomes 0, and the
// rest is clamped to the largest half float.
fn lumen_finite(color: vec4<f32>) -> vec4<f32> {
    let magnitude = bitcast<vec4<u32>>(color) & vec4<u32>(0x7fffffffu);
    let clamped = clamp(color, vec4<f32>(-65504.0), vec4<f32>(65504.0));
    return select(clamped, vec4<f32>(0.0), magnitude > vec4<u32>(0x7f800000u));
}

// `1.055 * p - 0.055`, written so that white encodes to exactly 1.
fn lumen_linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let c = abs(linear);
    let p = pow(c, vec3<f32>(1.0 / 2.4));
    let encoded = select(p + 0.055 * (p - 1.0), c * 12.92, c <= vec3<f32>(0.0031308));
    return sign(linear) * encoded;
}

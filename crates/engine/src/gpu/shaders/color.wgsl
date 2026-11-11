// The sRGB transfer function, shared by engine shaders and the `wgsl_shader` prelude. Rasters hold
// linear light; JSON colours, images, video and the output are sRGB-encoded. Values outside 0..1
// follow the curve mirrored about zero, so headroom above 1 survives a round trip.

fn lumen_srgb_to_linear(encoded: vec3<f32>) -> vec3<f32> {
    let c = abs(encoded);
    let linear = select(pow((c + 0.055) / 1.055, vec3<f32>(2.4)), c / 12.92, c <= vec3<f32>(0.04045));
    return sign(encoded) * linear;
}

fn lumen_linear_to_srgb(linear: vec3<f32>) -> vec3<f32> {
    let c = abs(linear);
    let encoded = select(1.055 * pow(c, vec3<f32>(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3<f32>(0.0031308));
    return sign(linear) * encoded;
}

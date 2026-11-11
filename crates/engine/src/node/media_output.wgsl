// Converts a frame's working-space raster to its output. An 8-bit target takes it sRGB-encoded
// and quantised once, with its alpha premultiplied, straight or shown over black; quantised colour
// never exceeds quantised alpha, and alpha 0 is transparent black. A float target takes the
// working space as it is. Requires the alpha, colour and dither chunks.

struct Output {
    origin: vec2<i32>,
    // 0 premultiplied, 1 straight, 2 opaque.
    alpha: u32,
    dither: u32,
    // The target's highest code, or 0 for a float target.
    levels: f32,
    // Non-zero when the target encodes to sRGB itself as it stores.
    srgb_target: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> output: Output;

@vertex
fn vs_main(@builtin(vertex_index) vertex_index: u32) -> @builtin(position) vec4<f32> {
    let positions = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -3.0),
        vec2<f32>(3.0, 1.0),
        vec2<f32>(-1.0, 1.0),
    );
    return vec4<f32>(positions[vertex_index], 0.0, 1.0);
}

// `value`, 0 to 1, quantised to the target's codes.
fn quantised(value: vec3<f32>, pixel: vec2<u32>) -> vec3<f32> {
    let dither = output.dither != 0u;
    let codes = value * output.levels;
    return vec3<f32>(
        lumen_quantise(codes.r, pixel, dither),
        lumen_quantise(codes.g, pixel, dither),
        lumen_quantise(codes.b, pixel, dither),
    ) / output.levels;
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let canvas = vec2<i32>(floor(position.xy));
    let pixel = canvas - output.origin;
    let source_size = vec2<i32>(textureDimensions(source_tex));
    var texel = vec4<f32>(0.0);
    if (all(pixel >= vec2<i32>(0)) && all(pixel < source_size)) {
        texel = textureLoad(source_tex, pixel, 0);
    }
    if (output.levels == 0.0) {
        return texel;
    }
    let light = max(texel.rgb, vec3<f32>(0.0));
    let at = vec2<u32>(canvas);
    var color: vec4<f32>;
    if (output.alpha == 2u) {
        color = vec4<f32>(quantised(clamp(lumen_linear_to_srgb(light), vec3<f32>(0.0), vec3<f32>(1.0)), at), 1.0);
    } else {
        let a = clamp(texel.a, 0.0, 1.0);
        let coverage = floor(a * output.levels + 0.5) / output.levels;
        let straight = lumen_straight_srgb(light, a);
        if (coverage == 0.0) {
            color = vec4<f32>(0.0);
        } else if (output.alpha == 1u) {
            color = vec4<f32>(quantised(straight, at), coverage);
        } else {
            color = vec4<f32>(min(quantised(straight * a, at), vec3<f32>(coverage)), coverage);
        }
    }
    if (output.srgb_target != 0u) {
        color = vec4<f32>(lumen_srgb_to_linear(color.rgb), color.a);
    }
    return color;
}

// Converts a frame's working-space raster to its output: sRGB-encoded, shown over black or with
// its alpha kept and its encoded colour premultiplied, and quantised once to the target's codes.
// Requires the alpha, colour and dither chunks.

struct Output {
    origin: vec2<i32>,
    opaque: u32,
    dither: u32,
    // The target's highest code, or 0 for a float target, which is written unquantised.
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

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let canvas = vec2<i32>(floor(position.xy));
    let pixel = canvas - output.origin;
    let source_size = vec2<i32>(textureDimensions(source_tex));
    var texel = vec4<f32>(0.0);
    if (all(pixel >= vec2<i32>(0)) && all(pixel < source_size)) {
        texel = textureLoad(source_tex, pixel, 0);
    }
    var color: vec4<f32>;
    if (output.opaque != 0u) {
        color = vec4<f32>(lumen_linear_to_srgb(max(texel.rgb, vec3<f32>(0.0))), 1.0);
    } else {
        let straight = unpremultiply(max(texel, vec4<f32>(0.0)));
        let a = min(straight.a, 1.0);
        color = vec4<f32>(min(lumen_linear_to_srgb(straight.rgb), vec3<f32>(1.0)) * a, a);
    }
    color = clamp(color, vec4<f32>(0.0), vec4<f32>(1.0));
    if (output.levels > 0.0) {
        let offset = select(0.0, lumen_dither(vec2<u32>(canvas)), output.dither != 0u);
        let rgb = round(color.rgb * output.levels + offset) / output.levels;
        color = vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), round(color.a * output.levels) / output.levels);
    }
    if (output.srgb_target != 0u) {
        color = vec4<f32>(lumen_srgb_to_linear(color.rgb), color.a);
    }
    return color;
}

struct DisplacementParams {
    // `xy`: pixels moved per map byte above 128, on each axis.
    amount: vec4<f32>,
    // The channel that drives each axis, the edge mode, and the sampling.
    modes: vec4<u32>,
    // Canvas pixels of the first texels of the output in `xy` and the map in `zw`.
    origins: vec4<i32>,
    // Canvas pixel of the source's first texel in `xy`, and the source's display size in `zw`.
    source: vec4<i32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var map_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: DisplacementParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba16float, write>;

const DISPLACE_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// A map texel's sRGB code's distance from 128, scaled by the map's alpha so that transparent
// areas, and everything outside the map, displace nothing. The straight colour is encoded and
// rounded to whole codes, so a neutral grey of 128 stays exactly neutral at any alpha.
fn map_offset(texel: vec4<f32>, channel: u32) -> f32 {
    if (texel.a <= 0.0) {
        return 0.0;
    }
    let encoded = clamp(lumen_linear_to_srgb(unpremultiply(texel).rgb), vec3<f32>(0.0), vec3<f32>(1.0));
    let rgb = round(encoded * 255.0);
    let alpha = min(texel.a, 1.0);
    var value: f32;
    switch (channel) {
        case 0u: { value = rgb.r; }
        case 1u: { value = rgb.g; }
        case 2u: { value = rgb.b; }
        default: { value = dot(rgb, DISPLACE_LUMA); }
    }
    return (value - 128.0) * alpha;
}

fn read_source(pixel: vec2<i32>) -> vec4<f32> {
    return sample_or_transparent(source_tex, pixel, params.source.xy);
}

// Sample coordinates are continuous, with pixel centres at integer + 0.5.
fn sample_source(coord: vec2<f32>) -> vec4<f32> {
    if (params.modes.w == 0u) {
        return read_source(vec2<i32>(floor(coord)));
    }
    let base = floor(coord - vec2<f32>(0.5));
    let frac = coord - vec2<f32>(0.5) - base;
    let p0 = vec2<i32>(base);
    let c00 = read_source(p0);
    let c10 = read_source(p0 + vec2<i32>(1, 0));
    let c01 = read_source(p0 + vec2<i32>(0, 1));
    let c11 = read_source(p0 + vec2<i32>(1, 1));
    return mix(mix(c00, c10, frac.x), mix(c01, c11, frac.x), frac.y);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy) + params.origins.xy;
    let texel = sample_or_transparent(map_tex, pixel, params.origins.zw);
    let offset = vec2<f32>(map_offset(texel, params.modes.x), map_offset(texel, params.modes.y));
    var coord = vec2<f32>(pixel) + vec2<f32>(0.5) - offset * params.amount.xy;
    if (params.modes.z == 0u) {
        coord = clamp(coord, vec2<f32>(0.5), vec2<f32>(params.source.zw) - vec2<f32>(0.5));
    }
    textureStore(output_tex, vec2<i32>(id.xy), sample_source(coord));
}

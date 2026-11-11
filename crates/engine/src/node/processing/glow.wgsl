struct GlowParams {
    // Premultiplied tint.
    tint: vec4<f32>,
    // `x`: threshold, `y`: intensity, `z`: 1 when tinted.
    values: vec4<f32>,
    // Canvas pixels of the first texels of the output in `xy` and the source in `zw`.
    origins: vec4<i32>,
    // Canvas pixel of the glow's first texel in `xy`.
    glow_origin: vec4<i32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var glow_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: GlowParams;
@group(0) @binding(3) var bright_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba16float, write>;

const GLOW_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn glow_read(tex: texture_2d<f32>, pixel: vec2<i32>, origin: vec2<i32>) -> vec4<f32> {
    let coord = pixel - origin;
    let size = vec2<i32>(textureDimensions(tex));
    if (any(coord < vec2<i32>(0)) || any(coord >= size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, coord, 0);
}

// The light a premultiplied pixel casts. Its linear luminance `l`, sRGB-encoded, is remapped from
// `threshold..1` to `0..1` and the colour scaled to match, so white glows at full strength and
// anything at or below the threshold not at all. Alpha is the brightest channel, which the blur
// bounds to 1: the glow is light, so black adds nothing and bright light keeps its headroom.
@compute @workgroup_size(8, 8, 1)
fn bright_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(bright_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let color = textureLoad(source_tex, vec2<i32>(id.xy), 0);
    let threshold = params.values.x;
    let premultiplied_luma = dot(color.rgb, GLOW_LUMA);
    var light = vec3<f32>(0.0);
    if (premultiplied_luma > 0.0 && color.a > 0.0) {
        let l = premultiplied_luma / color.a;
        let encoded = lumen_linear_to_srgb(vec3<f32>(l)).x;
        // Past a threshold of 254/255 the ramp narrows no further, so white fades out
        // continuously as the threshold reaches 1.
        let ramp = clamp((encoded - threshold) / max(1.0 - threshold, 1.0 / 255.0), 0.0, 1.0);
        light = color.rgb * (ramp / l);
    }
    if (params.values.z > 0.5) {
        light = dot(light, GLOW_LUMA) * params.tint.rgb;
    }
    light = light * params.values.y;
    textureStore(bright_tex, vec2<i32>(id.xy), vec4<f32>(light, max(light.r, max(light.g, light.b))));
}

// Adds the glow to the source, premultiplied. The light may take colour past white.
@compute @workgroup_size(8, 8, 1)
fn composite_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy) + params.origins.xy;
    let source = glow_read(source_tex, pixel, params.origins.zw);
    let glow = glow_read(glow_tex, pixel, params.glow_origin.xy);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(source.rgb + glow.rgb, min(source.a + glow.a, 1.0)));
}

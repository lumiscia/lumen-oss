struct PosterizeParams {
    // The palette sorted by brightness, as straight linear colours.
    colors: array<vec4<f32>, 16>,
    // The brightness of each palette colour, sorted, four to a row.
    lumas: array<vec4<f32>, 4>,
    // The canvas pixel of the source's first texel in `xy` and the dither cell in device pixels
    // in `z`.
    origin: vec4<i32>,
    // The dither amount in `x`.
    dither: vec4<f32>,
    // The number of levels, the mode, and the number of palette colours.
    levels_mode_count: vec4<u32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: PosterizeParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

const POSTERIZE_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

fn palette_luma(index: u32) -> f32 {
    return params.lumas[index / 4u][index % 4u];
}

// The palette colour, as linear light, for a pixel of encoded brightness `luma`. `shift`, from
// -0.5 to 0.5, moves the pixel across the midpoint between the two colours that bracket it.
fn palette_color(luma: f32, shift: f32) -> vec3<f32> {
    let count = params.levels_mode_count.z;
    var lower = 0u;
    for (var index = 1u; index < count; index++) {
        if (palette_luma(index) <= luma) {
            lower = index;
        }
    }
    var chosen = lower;
    if (lower + 1u < count && luma >= palette_luma(lower)) {
        let gap = palette_luma(lower + 1u) - palette_luma(lower);
        let along = (luma - palette_luma(lower)) / max(gap, 1e-6);
        if (along + shift > 0.5) {
            chosen = lower + 1u;
        }
    }
    return params.colors[chosen].rgb;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    if (color.a <= 0.0) {
        textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(0.0));
        return;
    }

    let pixel = vec2<i32>(id.xy) + params.origin.xy;
    let cell = vec2<i32>(floor(vec2<f32>(pixel) / f32(params.origin.z)));
    let shift = lumen_dither(bitcast<vec2<u32>>(cell)) * params.dither.x;
    let encoded = clamp(lumen_linear_to_srgb(unpremultiply(color).rgb), vec3<f32>(0.0), vec3<f32>(1.0));

    var rgb: vec3<f32>;
    if (params.levels_mode_count.y == 0u) {
        let steps = f32(params.levels_mode_count.x - 1u);
        let quantized = clamp(floor(encoded * steps + vec3<f32>(0.5 + shift)) / steps, vec3<f32>(0.0), vec3<f32>(1.0));
        rgb = lumen_srgb_to_linear(quantized);
    } else {
        rgb = palette_color(dot(encoded, POSTERIZE_LUMA), shift);
    }
    textureStore(output_tex, vec2<i32>(id.xy), lumen_finite(vec4<f32>(rgb * color.a, color.a)));
}

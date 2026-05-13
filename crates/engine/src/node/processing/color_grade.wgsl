struct ColorGradeParams {
    strength: f32,
    interpolation: u32,
    _pad: vec2<u32>,
}

struct ColorGradeLut {
    stops: array<vec4<f32>, 256>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ColorGradeParams;
@group(0) @binding(2) var<storage, read> lut: ColorGradeLut;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn sample_lut(value: f32, channel: u32) -> f32 {
    let scaled = clamp(value, 0.0, 1.0) * 255.0;
    if (params.interpolation == 0u) {
        let index = u32(round(scaled));
        return lut.stops[index][channel];
    }
    let low = u32(floor(scaled));
    let high = min(low + 1u, 255u);
    let t = scaled - f32(low);
    return mix(lut.stops[low][channel], lut.stops[high][channel], t);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    let graded = vec3<f32>(
        sample_lut(color.r, 0u),
        sample_lut(color.g, 1u),
        sample_lut(color.b, 2u)
    );
    let rgb = mix(color.rgb, graded, clamp(params.strength, 0.0, 1.0));
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), color.a));
}

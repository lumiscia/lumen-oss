struct HueSaturationParams {
    hue_offset: f32,
    saturation: f32,
    lightness: f32,
    _pad: f32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: HueSaturationParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn rgb_to_hsl(rgb: vec3<f32>) -> vec3<f32> {
    let max_value = max(max(rgb.r, rgb.g), rgb.b);
    let min_value = min(min(rgb.r, rgb.g), rgb.b);
    let lightness = (max_value + min_value) * 0.5;
    let delta = max_value - min_value;
    if (delta <= 0.000001) {
        return vec3<f32>(0.0, 0.0, lightness);
    }

    let saturation = select(
        delta / (max_value + min_value),
        delta / (2.0 - max_value - min_value),
        lightness > 0.5
    );
    var hue = 0.0;
    if (abs(max_value - rgb.r) <= 0.000001) {
        hue = ((rgb.g - rgb.b) / delta + select(0.0, 6.0, rgb.g < rgb.b)) / 6.0;
    } else if (abs(max_value - rgb.g) <= 0.000001) {
        hue = ((rgb.b - rgb.r) / delta + 2.0) / 6.0;
    } else {
        hue = ((rgb.r - rgb.g) / delta + 4.0) / 6.0;
    }
    return vec3<f32>(hue, saturation, lightness);
}

fn hue_to_rgb(p: f32, q: f32, initial_t: f32) -> f32 {
    let t = fract(initial_t);
    if (t < 1.0 / 6.0) {
        return p + (q - p) * 6.0 * t;
    }
    if (t < 1.0 / 2.0) {
        return q;
    }
    if (t < 2.0 / 3.0) {
        return p + (q - p) * (2.0 / 3.0 - t) * 6.0;
    }
    return p;
}

fn hsl_to_rgb(hsl: vec3<f32>) -> vec3<f32> {
    if (hsl.y <= 0.000001) {
        return vec3<f32>(hsl.z);
    }
    let q = select(
        hsl.z + hsl.y - hsl.z * hsl.y,
        hsl.z * (1.0 + hsl.y),
        hsl.z < 0.5
    );
    let p = 2.0 * hsl.z - q;
    return vec3<f32>(
        hue_to_rgb(p, q, hsl.x + 1.0 / 3.0),
        hue_to_rgb(p, q, hsl.x),
        hue_to_rgb(p, q, hsl.x - 1.0 / 3.0)
    );
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    var hsl = rgb_to_hsl(color.rgb);
    hsl.x = fract(hsl.x + params.hue_offset);
    hsl.y = clamp(hsl.y * params.saturation, 0.0, 1.0);
    hsl.z = clamp(hsl.z + params.lightness, 0.0, 1.0);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(hsl_to_rgb(hsl), color.a));
}

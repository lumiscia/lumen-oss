struct BooleanParams {
    values: vec4<f32>,
}

@group(0) @binding(0) var a_tex: texture_2d<f32>;
@group(0) @binding(1) var b_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: BooleanParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn sample_or_transparent(tex: texture_2d<f32>, pixel: vec2<u32>) -> vec4<f32> {
    let size = textureDimensions(tex);
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, vec2<i32>(pixel), 0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let a = sample_or_transparent(a_tex, id.xy);
    let b = sample_or_transparent(b_tex, id.xy);
    let operation = u32(params.values.x);
    let threshold = clamp(params.values.y, 0.0, 1.0);
    let aa = select(a.a, 1.0, a.a > threshold);
    let ba = select(b.a, 1.0, b.a > threshold);
    var alpha = max(aa, ba);
    if (operation == 1u) {
        alpha = min(aa, ba);
    } else if (operation == 2u) {
        alpha = aa * (1.0 - ba);
    } else if (operation == 3u) {
        alpha = abs(aa - ba);
    }
    let rgb = select(b.rgb, a.rgb, aa >= ba);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(rgb * alpha, alpha));
}

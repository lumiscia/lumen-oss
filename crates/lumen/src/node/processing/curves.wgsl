struct CurvesParams {
    values: vec4<f32>,
}

struct CurvesTable {
    entries: array<vec4<f32>, 256>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: CurvesParams;
@group(0) @binding(2) var<storage, read> curves: CurvesTable;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn apply_curve(value: f32, channel: u32) -> f32 {
    let scaled = clamp(value, 0.0, 1.0) * 255.0;
    let low = u32(floor(scaled));
    let high = min(low + 1u, 255u);
    let t = scaled - f32(low);
    return mix(curves.entries[low][channel], curves.entries[high][channel], t);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    let curved = vec3<f32>(
        apply_curve(color.r, 0u),
        apply_curve(color.g, 1u),
        apply_curve(color.b, 2u),
    );
    let strength = clamp(params.values.x, 0.0, 1.0);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(mix(color.rgb, curved, strength), color.a));
}

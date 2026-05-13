struct ChannelShuffleParams {
    selector_indices: vec4<f32>,
    selector_values: vec4<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: ChannelShuffleParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn select_channel(color: vec4<f32>, selector_index: f32, selector_value: f32) -> f32 {
    if (selector_index < 0.5) {
        return color.r;
    }
    if (selector_index < 1.5) {
        return color.g;
    }
    if (selector_index < 2.5) {
        return color.b;
    }
    if (selector_index < 3.5) {
        return color.a;
    }
    return selector_value;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(
        select_channel(color, params.selector_indices.x, params.selector_values.x),
        select_channel(color, params.selector_indices.y, params.selector_values.y),
        select_channel(color, params.selector_indices.z, params.selector_values.z),
        select_channel(color, params.selector_indices.w, params.selector_values.w)
    ));
}

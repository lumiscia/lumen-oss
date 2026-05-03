struct LevelsParams {
    black_point: f32,
    white_point: f32,
    gamma: f32,
    output_black: f32,
    output_white: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: LevelsParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    let black = clamp(params.black_point, 0.0, 1.0);
    let white = max(clamp(params.white_point, 0.0, 1.0), black + 0.000001);
    let gamma = max(params.gamma, 0.0001);
    let output_black = clamp(params.output_black, 0.0, 1.0);
    let output_white = clamp(params.output_white, 0.0, 1.0);
    var normalized = clamp((color.rgb - vec3<f32>(black)) / (white - black), vec3<f32>(0.0), vec3<f32>(1.0));
    normalized = pow(normalized, vec3<f32>(1.0 / gamma));
    let rgb = output_black + normalized * (output_white - output_black);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(rgb, color.a));
}

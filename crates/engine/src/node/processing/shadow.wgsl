struct ShadowParams {
    color: vec4<f32>,
    values: vec4<f32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var blur_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: ShadowParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn read_source_alpha(coord: vec2<i32>) -> f32 {
    let size = textureDimensions(source_tex);
    if (coord.x < 0 || coord.y < 0 || coord.x >= i32(size.x) || coord.y >= i32(size.y)) {
        return 0.0;
    }
    return textureLoad(source_tex, coord, 0).a;
}

fn read_blur_alpha(coord: vec2<i32>) -> f32 {
    let size = textureDimensions(blur_tex);
    if (coord.x < 0 || coord.y < 0 || coord.x >= i32(size.x) || coord.y >= i32(size.y)) {
        return 0.0;
    }
    return textureLoad(blur_tex, coord, 0).a;
}

@compute @workgroup_size(8, 8, 1)
fn horizontal_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = vec2<i32>(id.xy);
    let offset = vec2<i32>(round(params.values.xy));
    let radius = min(u32(params.values.z), 32u);
    var alpha = 0.0;
    var count = 0.0;
    for (var x = -i32(radius); x <= i32(radius); x = x + 1) {
        alpha = alpha + read_source_alpha(pixel - offset + vec2<i32>(x, 0));
        count = count + 1.0;
    }

    let blurred = alpha / max(count, 1.0);
    textureStore(output_tex, pixel, vec4<f32>(0.0, 0.0, 0.0, blurred));
}

@compute @workgroup_size(8, 8, 1)
fn vertical_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let pixel = vec2<i32>(id.xy);
    let radius = min(u32(params.values.z), 32u);
    var alpha = 0.0;
    var count = 0.0;
    for (var y = -i32(radius); y <= i32(radius); y = y + 1) {
        alpha = alpha + read_blur_alpha(pixel + vec2<i32>(0, y));
        count = count + 1.0;
    }

    let source = textureLoad(source_tex, pixel, 0);
    let shadow_alpha = (alpha / max(count, 1.0)) * params.color.a * clamp(params.values.w, 0.0, 1.0);
    let shadow = vec4<f32>(params.color.rgb * shadow_alpha, shadow_alpha);
    let out_alpha = source.a + shadow.a * (1.0 - source.a);
    let out_rgb = source.rgb + shadow.rgb * (1.0 - source.a);
    textureStore(output_tex, pixel, vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(out_alpha, 0.0, 1.0)));
}

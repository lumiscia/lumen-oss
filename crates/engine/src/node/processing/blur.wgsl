struct BlurParams {
    values: vec4<u32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: BlurParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn read_pixel(coord: vec2<i32>) -> vec4<f32> {
    let size = textureDimensions(input_tex);
    let clamped = clamp(coord, vec2<i32>(0), vec2<i32>(size) - vec2<i32>(1));
    return textureLoad(input_tex, clamped, 0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let radius = min(params.values.x, 32u);
    var color = vec4<f32>(0.0);
    var count = 0.0;
    for (var y = -i32(radius); y <= i32(radius); y = y + 1) {
        for (var x = -i32(radius); x <= i32(radius); x = x + 1) {
            color = color + read_pixel(vec2<i32>(id.xy) + vec2<i32>(x, y));
            count = count + 1.0;
        }
    }
    textureStore(output_tex, vec2<i32>(id.xy), color / count);
}

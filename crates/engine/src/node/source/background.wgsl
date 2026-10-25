@group(0) @binding(0) var<uniform> paint: Paint;
@group(0) @binding(1) var output_tex: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel_origin = vec2<f32>(f32(id.x), f32(id.y));
    let target_size = vec2<f32>(f32(size.x), f32(size.y));
    if (paint.paint_supersample == 0u) {
        let pixel = pixel_origin + vec2<f32>(0.5);
        textureStore(output_tex, vec2<i32>(id.xy), sample_paint(paint, pixel, pixel / target_size));
        return;
    }
    var color = vec4<f32>(0.0);
    for (var y = 0u; y < 4u; y = y + 1u) {
        for (var x = 0u; x < 4u; x = x + 1u) {
            let sample_pixel = pixel_origin + (vec2<f32>(f32(x), f32(y)) + vec2<f32>(0.5)) * 0.25;
            color += sample_paint(paint, sample_pixel, sample_pixel / target_size);
        }
    }
    textureStore(output_tex, vec2<i32>(id.xy), color * 0.0625);
}

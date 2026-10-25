struct MergeParams {
    opacity: f32,
    blend_mode: u32,
    has_mask: u32,
    _pad: u32,
}

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_tex: texture_2d<f32>;
@group(0) @binding(2) var mask_tex: texture_2d<f32>;
@group(0) @binding(3) var<uniform> params: MergeParams;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba8unorm, write>;

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

    let pixel = id.xy;
    let base = sample_or_transparent(base_tex, pixel);
    let overlay = sample_or_transparent(overlay_tex, pixel);
    let mask = select(1.0, sample_or_transparent(mask_tex, pixel).a, params.has_mask == 1u);
    let opacity = clamp(params.opacity, 0.0, 1.0) * mask;
    textureStore(output_tex, vec2<i32>(pixel), composite(base, overlay * opacity, params.blend_mode));
}

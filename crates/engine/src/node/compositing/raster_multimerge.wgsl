struct RasterMultiMergeParams {
    values: vec4<f32>,
}

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: RasterMultiMergeParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

fn sample_or_transparent(tex: texture_2d<f32>, pixel: vec2<u32>) -> vec4<f32> {
    let size = textureDimensions(tex);
    if (pixel.x >= size.x || pixel.y >= size.y) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, vec2<i32>(pixel), 0);
}

fn blend_rgb(base: vec3<f32>, overlay: vec3<f32>, mode: u32) -> vec3<f32> {
    if (mode == 1u) {
        return base * overlay;
    }
    if (mode == 2u) {
        return vec3<f32>(1.0) - ((vec3<f32>(1.0) - base) * (vec3<f32>(1.0) - overlay));
    }
    if (mode == 3u) {
        let low = 2.0 * base * overlay;
        let high = vec3<f32>(1.0) - (2.0 * (vec3<f32>(1.0) - base) * (vec3<f32>(1.0) - overlay));
        return select(high, low, base <= vec3<f32>(0.5));
    }
    if (mode == 4u) {
        return min(base, overlay);
    }
    if (mode == 5u) {
        return max(base, overlay);
    }
    return overlay;
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }

    let base = sample_or_transparent(base_tex, id.xy);
    let overlay = sample_or_transparent(overlay_tex, id.xy);
    let opacity = clamp(params.values.x, 0.0, 1.0);
    let blended = blend_rgb(base.rgb, overlay.rgb, u32(params.values.y));
    let alpha = overlay.a * opacity;
    let out_alpha = alpha + base.a * (1.0 - alpha);
    let out_rgb = mix(base.rgb, blended, alpha);
    textureStore(output_tex, vec2<i32>(id.xy), vec4<f32>(clamp(out_rgb, vec3<f32>(0.0), vec3<f32>(1.0)), clamp(out_alpha, 0.0, 1.0)));
}

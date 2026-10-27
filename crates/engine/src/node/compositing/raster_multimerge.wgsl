struct RasterMultiMergeParams {
    values: vec4<f32>,
    // Output texel of the first invocation, then canvas pixels of the first texels of the
    // output, base and overlay.
    start: vec2<i32>,
    origin: vec2<i32>,
    base_origin: vec2<i32>,
    overlay_origin: vec2<i32>,
}

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var overlay_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: RasterMultiMergeParams;
@group(0) @binding(3) var output_tex: texture_storage_2d<rgba8unorm, write>;

// The canvas pixel `pixel` of a texture whose first texel is at `origin`.
fn sample_or_transparent(tex: texture_2d<f32>, pixel: vec2<i32>, origin: vec2<i32>) -> vec4<f32> {
    let coord = pixel - origin;
    let size = vec2<i32>(textureDimensions(tex));
    if (any(coord < vec2<i32>(0)) || any(coord >= size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, coord, 0);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let texel = vec2<i32>(id.xy) + params.start;
    if (any(texel >= vec2<i32>(textureDimensions(output_tex)))) {
        return;
    }

    let pixel = texel + params.origin;
    let base = to_premultiplied_alpha(sample_or_transparent(base_tex, pixel, params.base_origin), params.values.z >= 0.5);
    let overlay = to_premultiplied_alpha(sample_or_transparent(overlay_tex, pixel, params.overlay_origin), params.values.w >= 0.5);
    let opacity = clamp(params.values.x, 0.0, 1.0);
    textureStore(output_tex, texel, composite(base, overlay * opacity, u32(params.values.y)));
}

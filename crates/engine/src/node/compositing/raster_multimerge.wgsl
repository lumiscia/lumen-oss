struct RasterMultiMergeParams {
    opacity: f32,
    blend_mode: u32,
    // Output texel of the first invocation, then canvas pixels of the first texels of the
    // output, base and each overlay.
    start: vec2<i32>,
    origin: vec2<i32>,
    base_origin: vec2<i32>,
    overlay_origins: array<vec4<i32>, 8>,
    // Overlays the pass composites, in binding order.
    count: u32,
}

@group(0) @binding(0) var base_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: RasterMultiMergeParams;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;
@group(0) @binding(3) var overlay_0: texture_2d<f32>;
@group(0) @binding(4) var overlay_1: texture_2d<f32>;
@group(0) @binding(5) var overlay_2: texture_2d<f32>;
@group(0) @binding(6) var overlay_3: texture_2d<f32>;
@group(0) @binding(7) var overlay_4: texture_2d<f32>;
@group(0) @binding(8) var overlay_5: texture_2d<f32>;
@group(0) @binding(9) var overlay_6: texture_2d<f32>;
@group(0) @binding(10) var overlay_7: texture_2d<f32>;

// `stack` with overlay `index` over it, or as it is when the pass has no such overlay.
fn with_overlay(stack: vec4<f32>, tex: texture_2d<f32>, index: u32, pixel: vec2<i32>) -> vec4<f32> {
    if (index >= params.count) {
        return stack;
    }
    let overlay = sample_or_transparent(tex, pixel, params.overlay_origins[index].xy);
    return composite(stack, overlay * clamp(params.opacity, 0.0, 1.0), params.blend_mode);
}

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let texel = vec2<i32>(id.xy) + params.start;
    if (any(texel >= vec2<i32>(textureDimensions(output_tex)))) {
        return;
    }

    let pixel = texel + params.origin;
    var stack = sample_or_transparent(base_tex, pixel, params.base_origin);
    stack = with_overlay(stack, overlay_0, 0u, pixel);
    stack = with_overlay(stack, overlay_1, 1u, pixel);
    stack = with_overlay(stack, overlay_2, 2u, pixel);
    stack = with_overlay(stack, overlay_3, 3u, pixel);
    stack = with_overlay(stack, overlay_4, 4u, pixel);
    stack = with_overlay(stack, overlay_5, 5u, pixel);
    stack = with_overlay(stack, overlay_6, 6u, pixel);
    stack = with_overlay(stack, overlay_7, 7u, pixel);
    textureStore(output_tex, texel, stack);
}

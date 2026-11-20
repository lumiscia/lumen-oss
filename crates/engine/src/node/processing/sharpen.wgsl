struct SharpenParams {
    // `x`: amount, `y`: threshold.
    values: vec4<f32>,
}

@group(0) @binding(0) var source_tex: texture_2d<f32>;
@group(0) @binding(1) var blur_tex: texture_2d<f32>;
@group(0) @binding(2) var<uniform> params: SharpenParams;
@group(0) @binding(4) var output_tex: texture_storage_2d<rgba16float, write>;

const SHARPEN_LUMA: vec3<f32> = vec3<f32>(0.2126, 0.7152, 0.0722);

// The sRGB-encoded straight colour of a linear premultiplied pixel with coverage.
fn sharpen_straight(color: vec4<f32>) -> vec3<f32> {
    return lumen_linear_to_srgb(color.rgb / max(color.a, 1e-6));
}

fn sharpen_luma(color: vec4<f32>) -> f32 {
    return dot(sharpen_straight(color), SHARPEN_LUMA);
}

// Shifts each pixel's encoded luma away from the luma of its blur, by `amount` times the detail
// past `threshold`, held to the range of the 3x3 pixels around it so edges gain no halo.
@compute @workgroup_size(8, 8, 1)
fn composite_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = vec2<i32>(id.xy);
    let source = textureLoad(source_tex, pixel, 0);
    if (source.a <= 0.0) {
        textureStore(output_tex, pixel, source);
        return;
    }
    let rgb = sharpen_straight(source);
    let luma = dot(rgb, SHARPEN_LUMA);
    let blurred = textureLoad(blur_tex, pixel, 0);
    var detail = 0.0;
    if (blurred.a > 1e-6) {
        detail = luma - sharpen_luma(blurred);
    }
    detail = sign(detail) * max(abs(detail) - params.values.y, 0.0);

    var low = luma;
    var high = luma;
    let bounds = vec2<i32>(textureDimensions(source_tex));
    for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
            let at = pixel + vec2<i32>(dx, dy);
            if (any(at < vec2<i32>(0)) || any(at >= bounds)) {
                continue;
            }
            let neighbour = textureLoad(source_tex, at, 0);
            if (neighbour.a > 0.0) {
                let l = sharpen_luma(neighbour);
                low = min(low, l);
                high = max(high, l);
            }
        }
    }
    let shift = clamp(luma + params.values.x * detail, low, high) - luma;
    let ceiling = max(1.0, max(rgb.r, max(rgb.g, rgb.b)));
    let sharpened = clamp(rgb + vec3<f32>(shift), vec3<f32>(0.0), vec3<f32>(ceiling));
    textureStore(output_tex, pixel, lumen_finite(vec4<f32>(lumen_srgb_to_linear(sharpened) * source.a, source.a)));
}

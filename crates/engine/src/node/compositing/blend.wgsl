// Shared blend helpers for Merge and RasterMultiMerge. Requires the alpha chunk.

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

// Composites premultiplied `source` over premultiplied `base`. Opacity and
// masks must already be folded into `source`.
fn composite(base: vec4<f32>, source: vec4<f32>, mode: u32) -> vec4<f32> {
    let alpha = source.a + base.a * (1.0 - source.a);
    var rgb = source.rgb + base.rgb * (1.0 - source.a);
    if (mode != 0u) {
        let blended = blend_rgb(unpremultiply(base).rgb, unpremultiply(source).rgb, mode);
        rgb = source.rgb * (1.0 - base.a) + base.rgb * (1.0 - source.a) + source.a * base.a * blended;
    }
    return clamp(vec4<f32>(rgb, alpha), vec4<f32>(0.0), vec4<f32>(1.0));
}


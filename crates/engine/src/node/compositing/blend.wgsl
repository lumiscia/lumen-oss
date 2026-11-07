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
    if (mode == 6u) {
        return min(base + overlay, vec3<f32>(1.0));
    }
    if (mode == 7u) {
        return abs(base - overlay);
    }
    if (mode == 8u) {
        return base + overlay - 2.0 * base * overlay;
    }
    if (mode == 9u) {
        let dodged = min(base / max(vec3<f32>(1.0) - overlay, vec3<f32>(1e-6)), vec3<f32>(1.0));
        let non_zero = select(dodged, vec3<f32>(1.0), overlay >= vec3<f32>(1.0));
        return select(non_zero, vec3<f32>(0.0), base <= vec3<f32>(0.0));
    }
    if (mode == 10u) {
        let burned = vec3<f32>(1.0) - min((vec3<f32>(1.0) - base) / max(overlay, vec3<f32>(1e-6)), vec3<f32>(1.0));
        let non_zero = select(burned, vec3<f32>(0.0), overlay <= vec3<f32>(0.0));
        return select(non_zero, vec3<f32>(1.0), base >= vec3<f32>(1.0));
    }
    if (mode == 11u) {
        let low = base * (2.0 * overlay);
        let doubled = 2.0 * overlay - vec3<f32>(1.0);
        let high = base + doubled - base * doubled;
        return select(high, low, overlay <= vec3<f32>(0.5));
    }
    if (mode == 12u) {
        let curve = select(
            sqrt(base),
            ((16.0 * base - vec3<f32>(12.0)) * base + vec3<f32>(4.0)) * base,
            base <= vec3<f32>(0.25),
        );
        let low = base - (vec3<f32>(1.0) - 2.0 * overlay) * base * (vec3<f32>(1.0) - base);
        let high = base + (2.0 * overlay - vec3<f32>(1.0)) * (curve - base);
        return select(high, low, overlay <= vec3<f32>(0.5));
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


// Quantising to whole code values with an ordered dither. The 8x8 Bayer thresholds,
// `(rank + 0.5) / 64 - 0.5`, span the whole code step, so a flat area of any value averages to
// that value, to 1/64 of a code. A value within `LUMEN_SNAP` of a code takes that code without
// dither: half-float rasters hold an 8-bit sRGB code to within 0.08 of it, so 8-bit content
// comes out exactly, at the cost of a bias of up to `LUMEN_SNAP` for values that close to a code.

const LUMEN_SNAP: f32 = 0.1;

// The dither threshold of `pixel`, strictly inside half a code step either way.
fn lumen_dither(pixel: vec2<u32>) -> f32 {
    let x = pixel.x & 7u;
    let xy = x ^ (pixel.y & 7u);
    let rank = ((xy & 1u) << 5u) | ((x & 1u) << 4u) | ((xy & 2u) << 2u) | ((x & 2u) << 1u)
        | ((xy & 4u) >> 1u) | ((x & 4u) >> 2u);
    return (f32(rank) + 0.5) / 64.0 - 0.5;
}

// `code`, in code steps, as a whole code: the nearest, or with `dither` the dithered one.
fn lumen_quantise(code: f32, pixel: vec2<u32>, dither: bool) -> f32 {
    let nearest = floor(code + 0.5);
    if (!dither || abs(code - nearest) < LUMEN_SNAP) {
        return nearest;
    }
    return floor(code + 0.5 + lumen_dither(pixel));
}

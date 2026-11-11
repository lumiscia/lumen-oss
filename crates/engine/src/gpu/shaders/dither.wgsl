// Ordered dither for quantising the output to whole code values: an 8x8 Bayer offset in code
// steps, under 0.4 either way, so a value within 0.1 of a code, as half-float rasters keep sRGB
// codes, keeps that code.

fn lumen_dither(pixel: vec2<u32>) -> f32 {
    let x = pixel.x & 7u;
    let xy = x ^ (pixel.y & 7u);
    let rank = ((xy & 1u) << 5u) | ((x & 1u) << 4u) | ((xy & 2u) << 2u) | ((x & 2u) << 1u)
        | ((xy & 4u) >> 1u) | ((x & 4u) >> 2u);
    return ((f32(rank) + 0.5) / 64.0 - 0.5) * 0.8;
}

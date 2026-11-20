// Filtering for a source shown smaller than its size, appended to a shader that declares the
// source texture `input_tex` and `read_level_pixel`, which reads a texel of one of its mip levels
// whose sides are `size`.
// Rasters hold premultiplied linear light, which is what is averaged, so coverage weights colour
// and a transparent border neither darkens nor fringes what it meets.

// The most texels, of the level it reads, that the filter spans along an axis.
const MINIFY_MAX_SPAN: f32 = 16.0;

// The footprint along the narrower axis up to which the box is taken on the source itself; keep in
// sync with `UNCHAINED_FOOTPRINT` in `minify.rs`.
const MINIFY_UNCHAINED_FOOTPRINT: f32 = 4.0;

// The mean of the light under the box that `footprint` texels of the level-0 texture wide and
// high, centred on `center` (pixel centres at integer + 0.5), covers, read from mip level
// `level`; each texel counts by the area it shares with the box. A footprint under one texel of
// the level is one, which is bilinear filtering. The level's texels are as many level-0 texels as
// the ratio of the level-0 sides to its own.
fn minify_level(center: vec2<f32>, footprint: vec2<f32>, level: u32) -> vec4<f32> {
    let level_size = vec2<i32>(textureDimensions(input_tex, level));
    let ratio = vec2<f32>(textureDimensions(input_tex, 0)) / vec2<f32>(level_size);
    let span = clamp(footprint / ratio, vec2<f32>(1.0), vec2<f32>(MINIFY_MAX_SPAN));
    let low = center / ratio - span * 0.5;
    let high = low + span;
    let last = vec2<i32>(ceil(high));
    var sum = vec4<f32>(0.0);
    for (var y = i32(floor(low.y)); y < last.y; y += 1) {
        let weight_y = min(high.y, f32(y + 1)) - max(low.y, f32(y));
        for (var x = i32(floor(low.x)); x < last.x; x += 1) {
            let weight = weight_y * (min(high.x, f32(x + 1)) - max(low.x, f32(x)));
            sum += read_level_pixel(vec2<i32>(x, y), level, level_size) * weight;
        }
    }
    return sum / (span.x * span.y);
}

// `minify_level` blended between the two whole levels around `lod`, so that nothing steps as the
// footprint grows.
fn minify(center: vec2<f32>, footprint: vec2<f32>, lod: f32) -> vec4<f32> {
    let top = f32(textureNumLevels(input_tex) - 1u);
    let at = clamp(lod, 0.0, top);
    let finer = floor(at);
    var color = minify_level(center, footprint, u32(finer));
    if (at > finer) {
        color = mix(color, minify_level(center, footprint, u32(finer) + 1u), at - finer);
    }
    return color;
}

// The mip level whose texel is `extent` texels of the level-0 texture along an axis of `side`
// texels, going by the sides the levels really have; keep in sync with `lod_at` in `minify.rs`.
fn minify_lod_at(extent: f32, side: u32) -> f32 {
    if (!(extent > 1.0)) {
        return 0.0;
    }
    let top = firstLeadingBit(max(side, 1u));
    var level = 0u;
    for (var candidate = top; candidate > 0u; candidate = candidate - 1u) {
        if (f32(side) / f32(side >> candidate) <= extent) {
            level = candidate;
            break;
        }
    }
    if (level == top) {
        return f32(level);
    }
    let finer = log(f32(side) / f32(side >> level));
    let coarser = log(f32(side) / f32(side >> (level + 1u)));
    return f32(level) + clamp((log(extent) - finer) / (coarser - finer), 0.0, 1.0);
}

// The level `minify` reads for a pixel covering `footprint` texels along the source's axes, as
// `lod` in `minify.rs` chooses it for the whole of an image.
fn minify_lod(footprint: vec2<f32>) -> f32 {
    let sides = textureDimensions(input_tex, 0);
    let x_narrower = footprint.x <= footprint.y;
    let narrow = select(footprint.y, footprint.x, x_narrower);
    let wide = select(footprint.x, footprint.y, x_narrower);
    let narrow_side = select(sides.y, sides.x, x_narrower);
    let wide_side = select(sides.x, sides.y, x_narrower);
    let rise = clamp((narrow - MINIFY_UNCHAINED_FOOTPRINT) / MINIFY_UNCHAINED_FOOTPRINT, 0.0, 1.0);
    return max(
        rise * minify_lod_at(narrow, narrow_side),
        minify_lod_at(wide / MINIFY_MAX_SPAN, wide_side),
    );
}

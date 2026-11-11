// Filtering for a source shown smaller than its size, appended to a shader that declares the
// source texture `input_tex` and `read_level_pixel`, which reads a texel of one of its mip levels
// whose sides are `size`.
// Rasters hold premultiplied linear light, which is what is averaged, so coverage weights colour
// and a transparent border neither darkens nor fringes what it meets.

// The most texels, of the level it reads, that the filter spans along an axis.
const MINIFY_MAX_SPAN: f32 = 16.0;

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

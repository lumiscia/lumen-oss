// Fills a level of a mip chain from the level above it, or from the raster itself for the first.
// Each texel of a level is the average, weighted by area, of every texel of the one above it that
// it covers, so that an odd side's last row or column counts as well.

@group(0) @binding(0) var source: texture_2d<f32>;

// Where the first level's texels start in `source`, for a chain of part of a raster.
struct Region {
    origin: vec2<i32>,
    _pad: vec2<i32>,
}

@group(0) @binding(1) var<uniform> region: Region;

// Rasters hold premultiplied linear light, which is what is averaged.
fn to_linear(stored: vec4<f32>) -> vec4<f32> {
    return stored;
}

fn to_stored(light: vec4<f32>) -> vec4<f32> {
    return light;
}

// A triangle covering the target.
@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> @builtin(position) vec4<f32> {
    let corner = vec2<f32>(f32((index << 1u) & 2u), f32(index & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// The raster itself, or the part of it that `region` places, as the first level.
@fragment
fn fs_copy(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(source, vec2<i32>(at.xy) + region.origin, 0);
}

// The level below `source`, whose sides are half of its sides, rounded down.
@fragment
fn fs_halve(@builtin(position) at: vec4<f32>) -> @location(0) vec4<f32> {
    let above = vec2<i32>(textureDimensions(source));
    let ratio = vec2<f32>(above) / vec2<f32>(max(above / 2, vec2<i32>(1)));
    let start = floor(at.xy) * ratio;
    let end = start + ratio;
    let last = min(vec2<i32>(ceil(end)), above);
    var sum = vec4<f32>(0.0);
    var covered = 0.0;
    for (var y = i32(floor(start.y)); y < last.y; y += 1) {
        let share_y = min(end.y, f32(y + 1)) - max(start.y, f32(y));
        for (var x = i32(floor(start.x)); x < last.x; x += 1) {
            let share = share_y * (min(end.x, f32(x + 1)) - max(start.x, f32(x)));
            sum += to_linear(textureLoad(source, vec2<i32>(x, y), 0)) * share;
            covered += share;
        }
    }
    return to_stored(sum / max(covered, 1e-6));
}

// The canvas pixel `pixel` of a texture whose first texel is at `origin`, or transparent where
// the texture has no texel.
fn sample_or_transparent(tex: texture_2d<f32>, pixel: vec2<i32>, origin: vec2<i32>) -> vec4<f32> {
    let coord = pixel - origin;
    let size = vec2<i32>(textureDimensions(tex));
    if (any(coord < vec2<i32>(0)) || any(coord >= size)) {
        return vec4<f32>(0.0);
    }
    return textureLoad(tex, coord, 0);
}

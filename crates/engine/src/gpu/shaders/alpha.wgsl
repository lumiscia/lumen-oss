// Shared alpha helpers prepended to engine shaders. Engine rasters store
// premultiplied alpha unless their metadata says otherwise.

const ALPHA_EPSILON: f32 = 0.000001;

fn premultiply(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * color.a, color.a);
}

// Fully transparent pixels unpremultiply to transparent black.
fn unpremultiply(color: vec4<f32>) -> vec4<f32> {
    if (color.a <= ALPHA_EPSILON) {
        return vec4<f32>(0.0);
    }
    return vec4<f32>(color.rgb / color.a, color.a);
}

fn to_straight_alpha(color: vec4<f32>, premultiplied: bool) -> vec4<f32> {
    if (premultiplied) {
        return unpremultiply(color);
    }
    return color;
}

fn from_straight_alpha(color: vec4<f32>, premultiplied: bool) -> vec4<f32> {
    if (premultiplied) {
        return premultiply(color);
    }
    return color;
}


// Shared alpha helpers prepended to engine shaders. Engine rasters store premultiplied alpha.

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

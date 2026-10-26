// Transform and opacity shared by the shape, path and text renderers.

struct DrawableTransform {
    linear: mat2x2<f32>,
    inverse_linear: mat2x2<f32>,
    offset: vec2<f32>,
    inverse_offset: vec2<f32>,
    opacity: f32,
    flags: u32,
    _pad: vec2<u32>,
}

// Transforms to average over the shutter, each mapping geometry laid out at the frame to
// the canvas at the tap's time. With no taps the frame's transform is drawn alone. The
// array length is `MAX_MOTION_TAPS`.
struct DrawableMotion {
    tap_count: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
    taps: array<DrawableTransform, 32>,
}

// Bit of `DrawableTransform.flags`, set when scale or rotation differ from the identity.
// Without it, points pass through unchanged so untransformed output stays exact.
const TRANSFORM_LINEAR: u32 = 1u;

fn drawable_is_linear(transform: DrawableTransform) -> bool {
    return (transform.flags & TRANSFORM_LINEAR) != 0u;
}

fn drawable_to_canvas(transform: DrawableTransform, local: vec2<f32>) -> vec2<f32> {
    return select(
        local,
        transform.linear * local + transform.offset,
        drawable_is_linear(transform),
    );
}

// Maps a vector (a difference of points) to canvas space.
fn drawable_vector_to_canvas(transform: DrawableTransform, vector: vec2<f32>) -> vec2<f32> {
    return select(vector, transform.linear * vector, drawable_is_linear(transform));
}

fn drawable_to_local(transform: DrawableTransform, pixel: vec2<f32>) -> vec2<f32> {
    return select(
        pixel,
        transform.inverse_linear * pixel + transform.inverse_offset,
        drawable_is_linear(transform),
    );
}

// Untransformed vector from `local` to the point under `pixel`. It is measured from
// `local` in canvas pixels first, so extreme scales do not cancel it away.
fn drawable_local_offset(transform: DrawableTransform, pixel: vec2<f32>, local: vec2<f32>) -> vec2<f32> {
    return select(
        pixel - local,
        transform.inverse_linear * (pixel - drawable_to_canvas(transform, local)),
        drawable_is_linear(transform),
    );
}

// Canvas pixels per untransformed pixel along each untransformed axis. The linear map is
// a rotation after a scale, so scaling an untransformed vector by these gives its canvas
// image rotated back onto the untransformed axes, with every distance kept.
fn drawable_axis_scale(transform: DrawableTransform) -> vec2<f32> {
    if (!drawable_is_linear(transform)) {
        return vec2<f32>(1.0);
    }
    return vec2<f32>(length(transform.linear[0]), length(transform.linear[1]));
}

// Canvas pixels per untransformed pixel, measured across an edge whose distance grows
// along `normal`. Multiplying a local distance by it gives the canvas distance, which
// is exact for straight edges under any scale and first-order accurate near curves.
fn drawable_distance_scale(transform: DrawableTransform, normal: vec2<f32>) -> f32 {
    if (!drawable_is_linear(transform)) {
        return 1.0;
    }
    let local_length = length(normal);
    let canvas_gradient = length(transpose(transform.inverse_linear) * normal);
    if (local_length <= 0.000001 || canvas_gradient <= 0.000001) {
        return sqrt(abs(determinant(transform.linear)));
    }
    return local_length / canvas_gradient;
}

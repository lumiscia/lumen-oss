use bytemuck::{Pod, Zeroable};

pub(crate) const DRAWABLE_SHADER: &str = include_str!("drawable.wgsl");

// Bit of `GpuDrawableTransform::flags`; keep in sync with `drawable.wgsl`.
const TRANSFORM_LINEAR: u32 = 1;
// Scales closer to zero than this hide the drawable, like the `transform` node's clamp.
const MIN_SCALE: f64 = 0.000001;

/// The largest absolute scale component, so the most canvas pixels per local pixel.
pub(crate) fn largest_scale(scale: (f64, f64)) -> f64 {
    scale.0.abs().max(scale.1.abs()).max(MIN_SCALE)
}

/// The smallest absolute scale component, so the fewest canvas pixels per local pixel.
pub(crate) fn smallest_scale(scale: (f64, f64)) -> f64 {
    scale.0.abs().min(scale.1.abs()).max(MIN_SCALE)
}

/// Transform and opacity shared by the shape, path and text nodes, applied analytically
/// by their shaders. Follows the `transform` node: rotation is in degrees, clockwise on
/// screen, and scale and rotation pivot about `position + anchor`. A drawable therefore
/// matches its untransformed self fed through a `transform` node with that custom pivot.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct DrawableTransform {
    pub(crate) position: (f64, f64),
    pub(crate) scale: (f64, f64),
    pub(crate) rotation: f64,
    pub(crate) anchor: (f64, f64),
    pub(crate) opacity: f64,
}

impl DrawableTransform {
    pub(crate) fn to_gpu(self) -> GpuDrawableTransform {
        let pivot = [
            self.position.0 + self.anchor.0,
            self.position.1 + self.anchor.1,
        ];
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let (scale_x, scale_y) = self.scale;
        let degenerate = scale_x.abs() < MIN_SCALE || scale_y.abs() < MIN_SCALE;
        let linear = [
            [cos * scale_x, sin * scale_x],
            [-sin * scale_y, cos * scale_y],
        ];
        let [inverse_x, inverse_y] =
            [scale_x, scale_y].map(|scale| 1.0 / scale.abs().max(MIN_SCALE).copysign(scale));
        let inverse_linear = [
            [cos * inverse_x, -sin * inverse_y],
            [sin * inverse_x, cos * inverse_y],
        ];
        let is_linear = linear != [[1.0, 0.0], [0.0, 1.0]];
        GpuDrawableTransform {
            linear: to_f32(linear),
            inverse_linear: to_f32(inverse_linear),
            offset: pivot_offset(linear, pivot),
            inverse_offset: pivot_offset(inverse_linear, pivot),
            opacity: if degenerate {
                0.0
            } else {
                self.opacity.clamp(0.0, 1.0) as f32
            },
            flags: if is_linear { TRANSFORM_LINEAR } else { 0 },
            _pad: [0; 2],
        }
    }
}

/// The translation that keeps `pivot` fixed under the column-major `linear` map.
fn pivot_offset(linear: [[f64; 2]; 2], pivot: [f64; 2]) -> [f32; 2] {
    let mapped = [
        linear[0][0] * pivot[0] + linear[1][0] * pivot[1],
        linear[0][1] * pivot[0] + linear[1][1] * pivot[1],
    ];
    [(pivot[0] - mapped[0]) as f32, (pivot[1] - mapped[1]) as f32]
}

fn to_f32(matrix: [[f64; 2]; 2]) -> [[f32; 2]; 2] {
    matrix.map(|column| column.map(|value| value as f32))
}

#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Pod, Zeroable)]
pub(crate) struct GpuDrawableTransform {
    /// Columns of the map from untransformed to canvas pixels.
    pub(crate) linear: [[f32; 2]; 2],
    pub(crate) inverse_linear: [[f32; 2]; 2],
    pub(crate) offset: [f32; 2],
    pub(crate) inverse_offset: [f32; 2],
    pub(crate) opacity: f32,
    pub(crate) flags: u32,
    pub(crate) _pad: [u32; 2],
}

impl GpuDrawableTransform {
    /// Maps an untransformed point to canvas pixels, like `drawable_to_canvas`.
    pub(crate) fn to_canvas(self, point: [f32; 2]) -> [f32; 2] {
        if self.flags & TRANSFORM_LINEAR == 0 {
            return point;
        }
        let [column_x, column_y] = self.linear;
        [
            column_x[0] * point[0] + column_y[0] * point[1] + self.offset[0],
            column_x[1] * point[0] + column_y[1] * point[1] + self.offset[1],
        ]
    }

    /// Canvas-space `[min_x, min_y, max_x, max_y]` of the untransformed box from `min` to
    /// `max`.
    pub(crate) fn canvas_bounds(self, min: [f32; 2], max: [f32; 2]) -> [f32; 4] {
        let corners = [min, [max[0], min[1]], [min[0], max[1]], max];
        corners.map(|corner| self.to_canvas(corner)).iter().fold(
            [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
            |[min_x, min_y, max_x, max_y], [x, y]| {
                [min_x.min(*x), min_y.min(*y), max_x.max(*x), max_y.max(*y)]
            },
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transform(scale: (f64, f64), rotation: f64) -> DrawableTransform {
        DrawableTransform {
            position: (10.0, 20.0),
            scale,
            rotation,
            anchor: (5.0, 5.0),
            opacity: 1.0,
        }
    }

    fn apply(matrix: [[f32; 2]; 2], offset: [f32; 2], point: [f32; 2]) -> [f32; 2] {
        [
            matrix[0][0] * point[0] + matrix[1][0] * point[1] + offset[0],
            matrix[0][1] * point[0] + matrix[1][1] * point[1] + offset[1],
        ]
    }

    #[test]
    fn identity_has_no_linear_part_or_offset() {
        let gpu = transform((1.0, 1.0), 0.0).to_gpu();
        assert_eq!(gpu.flags, 0);
        assert_eq!(gpu.offset, [0.0, 0.0]);
        assert_eq!(gpu.inverse_offset, [0.0, 0.0]);
        assert_eq!(gpu.opacity, 1.0);
    }

    #[test]
    fn rotates_clockwise_about_the_anchor_and_inverts() {
        let gpu = transform((2.0, 1.0), 90.0).to_gpu();
        assert_eq!(gpu.flags, TRANSFORM_LINEAR);
        let pivot = [15.0, 25.0];
        assert_eq!(apply(gpu.linear, gpu.offset, pivot), pivot);
        // One pixel right of the pivot, doubled, then turned to point down the screen.
        let moved = apply(gpu.linear, gpu.offset, [16.0, 25.0]);
        assert!((moved[0] - 15.0).abs() < 1e-5 && (moved[1] - 27.0).abs() < 1e-5);
        let back = apply(gpu.inverse_linear, gpu.inverse_offset, moved);
        assert!((back[0] - 16.0).abs() < 1e-5 && (back[1] - 25.0).abs() < 1e-5);
    }

    #[test]
    fn zero_scale_hides_without_infinite_inverse() {
        let gpu = transform((0.0, 1.0), 0.0).to_gpu();
        assert_eq!(gpu.opacity, 0.0);
        assert!(
            gpu.inverse_linear
                .iter()
                .flatten()
                .all(|value| value.is_finite())
        );
    }
}

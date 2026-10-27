use bytemuck::{Pod, Zeroable};

use crate::expr::ExpressionContext;
use crate::node::motion::{
    self, Lerp, MAX_BOUNDED_MAGNITUDE, MAX_MOTION_TAPS, ROUNDING_ERROR_FACTOR, Shutter,
};
use crate::node::{Deferred, NodeId};

pub(crate) const DRAWABLE_SHADER: &str = include_str!("drawable.wgsl");

// Bit of `GpuDrawableTransform::flags`; keep in sync with `drawable.wgsl`.
const TRANSFORM_LINEAR: u32 = 1;
// Scales closer to zero than this hide the drawable, like the `transform` node's clamp.
const MIN_SCALE: f64 = 0.000001;
// Times a text layer's scale is lowered to fit its layer, and the lowest factor it reaches.
const MAX_LAYER_FITS: usize = 4;
const MIN_LAYER_FACTOR: f64 = 1.0 / 1024.0;

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
    /// Whether the scale is too close to zero to draw anything.
    fn is_degenerate(&self) -> bool {
        self.scale.0.abs() < MIN_SCALE || self.scale.1.abs() < MIN_SCALE
    }

    pub(crate) fn to_gpu(self) -> GpuDrawableTransform {
        let pivot = [
            self.position.0 + self.anchor.0,
            self.position.1 + self.anchor.1,
        ];
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let (scale_x, scale_y) = self.scale;
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
            opacity: if self.is_degenerate() {
                0.0
            } else {
                self.opacity.clamp(0.0, 1.0) as f32
            },
            flags: if is_linear { TRANSFORM_LINEAR } else { 0 },
            _pad: [0; 2],
        }
    }

    /// `to_gpu` for geometry laid out at `layout_position` rather than at `position`, which
    /// moves it by their difference. The map is always marked linear, since it has an offset.
    pub(crate) fn to_gpu_from(self, layout_position: (f64, f64)) -> GpuDrawableTransform {
        let gpu = DrawableTransform {
            position: layout_position,
            ..self
        }
        .to_gpu();
        gpu.translated([
            (self.position.0 - layout_position.0) as f32,
            (self.position.1 - layout_position.1) as f32,
        ])
    }
}

/// Opacity stays at the frame's value over the shutter.
impl Lerp for DrawableTransform {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            position: motion::lerp2(self.position, other.position, t),
            scale: motion::lerp2(self.scale, other.scale, t),
            rotation: motion::lerp(self.rotation, other.rotation, t),
            anchor: motion::lerp2(self.anchor, other.anchor, t),
            opacity: self.opacity,
        }
    }
}

/// The unevaluated transform params of a shape, path or text node.
pub(crate) struct DeferredTransform<'a> {
    pub(crate) position: &'a Deferred<(f64, f64)>,
    pub(crate) scale: &'a Deferred<(f64, f64)>,
    pub(crate) rotation: &'a Deferred<f64>,
    pub(crate) anchor: &'a Deferred<(f64, f64)>,
}

impl DeferredTransform<'_> {
    /// Evaluates the transform at either end of a `shutter_angle` shutter around `frame`,
    /// the transform at the frame. `None` when motion blur is off or nothing moves.
    pub(crate) fn shutter(
        &self,
        shutter_angle: f64,
        frame: DrawableTransform,
        node_id: NodeId,
        expr: &ExpressionContext<'_>,
    ) -> crate::Result<Option<Shutter<DrawableTransform>>> {
        let Some(offset) = motion::shutter_offset(shutter_angle) else {
            return Ok(None);
        };
        let at = |subframe: f64| -> crate::Result<DrawableTransform> {
            let expr = expr.at_subframe(subframe);
            Ok(DrawableTransform {
                position: self.position.eval(node_id, "position", &expr)?,
                scale: self.scale.eval(node_id, "scale", &expr)?,
                rotation: self.rotation.eval(node_id, "rotation", &expr)?,
                anchor: self.anchor.eval(node_id, "anchor", &expr)?,
                opacity: frame.opacity,
            })
        };
        let shutter = Shutter {
            open: at(-offset)?,
            frame,
            close: at(offset)?,
        };
        Ok((!shutter.is_static()).then_some(shutter))
    }
}

/// The largest absolute scale components that the transform has at the frame or at either
/// end of its shutter, so also anywhere between them.
pub(crate) fn peak_scale(
    frame: (f64, f64),
    shutter: Option<&Shutter<DrawableTransform>>,
) -> (f64, f64) {
    let frame = (frame.0.abs(), frame.1.abs());
    let Some(shutter) = shutter else {
        return frame;
    };
    [shutter.open.scale, shutter.close.scale]
        .iter()
        .fold(frame, |peak, scale| {
            (peak.0.max(scale.0.abs()), peak.1.max(scale.1.abs()))
        })
}

/// The taps a drawable averages over its shutter, each moving the geometry laid out at the
/// frame to where it lies at the tap's time. Without taps the drawable is not blurred, and
/// is drawn at the frame alone; with them it is drawn at the taps alone.
#[derive(Debug, Clone, Default)]
pub(crate) struct DrawableMotion {
    layout_position: (f64, f64),
    taps: Vec<DrawableTransform>,
}

impl DrawableMotion {
    /// Taps for geometry within the untransformed box from `min` to `max`.
    pub(crate) fn new(
        shutter: Option<&Shutter<DrawableTransform>>,
        min: [f32; 2],
        max: [f32; 2],
    ) -> Self {
        let Some(shutter) = shutter else {
            return Self::default();
        };
        let layout_position = shutter.frame.position;
        let corners =
            [min, [max[0], min[1]], [min[0], max[1]], max].map(|corner| corner.map(f64::from));
        let taps = shutter.taps(&corners, |transform, point| {
            let point = point.map(|value| value as f32);
            transform
                .to_gpu_from(layout_position)
                .to_canvas(point)
                .map(f64::from)
        });
        Self {
            layout_position,
            taps,
        }
    }

    /// The transforms the drawable is drawn under.
    fn drawn<'a>(&'a self, frame: &'a DrawableTransform) -> &'a [DrawableTransform] {
        if self.taps.is_empty() {
            std::slice::from_ref(frame)
        } else {
            &self.taps
        }
    }

    /// Whether anything is drawn under some transform: the layer is not fully transparent
    /// and not every transform it is drawn under collapses it. A zero scale at the frame
    /// alone hides nothing while the taps around it have a scale.
    pub(crate) fn is_visible(&self, frame: &DrawableTransform) -> bool {
        frame.opacity > 0.0
            && self
                .drawn(frame)
                .iter()
                .any(|transform| !transform.is_degenerate())
    }

    /// The smallest absolute scale component of the drawable, over the transforms that
    /// draw it.
    pub(crate) fn smallest_scale(&self, frame: &DrawableTransform) -> f64 {
        self.drawn(frame)
            .iter()
            .filter(|transform| !transform.is_degenerate())
            .map(|transform| smallest_scale(transform.scale))
            .fold(f64::MAX, f64::min)
    }

    /// `frame.canvas_bounds`, or when blurred the box that holds the box from `min` to
    /// `max` under every tap.
    pub(crate) fn canvas_bounds(
        &self,
        frame: GpuDrawableTransform,
        min: [f32; 2],
        max: [f32; 2],
    ) -> [f32; 4] {
        if self.taps.is_empty() {
            return frame.canvas_bounds(min, max);
        }
        self.gpu_taps()
            .map(|tap| tap.canvas_bounds(min, max))
            .reduce(union_bounds)
            .unwrap_or_default()
    }

    fn gpu_taps(&self) -> impl Iterator<Item = GpuDrawableTransform> + '_ {
        self.taps
            .iter()
            .map(|tap| tap.to_gpu_from(self.layout_position))
    }

    /// The transform that text is drawn under, to a layer of `layer` pixels, before the
    /// layer is smeared over the taps. Without taps, that is the frame's.
    ///
    /// With them it scales by the componentwise peak of the absolute scales of the frame
    /// and the taps, and otherwise follows the frame. Every tap then maps the layer to the
    /// canvas by a map that nowhere magnifies, however the scales and rotations differ, and
    /// the peak is the same whichever way the shutter runs. The layer is moved to hold what
    /// the taps show of the text's box from `min` to `max`, which the canvas may not at the
    /// frame: text sliding in from beyond an edge is still smeared across it.
    ///
    /// The box and the canvas are held in full when they fit the layer. When the taps show
    /// more than that, the layer is drawn at a lower scale until they fit, rather than
    /// clipped, and clipped about its middle only when even that does not suffice.
    pub(crate) fn layer_transform(
        &self,
        frame: &DrawableTransform,
        min: [f32; 2],
        max: [f32; 2],
        canvas: [f32; 2],
        layer: [f32; 2],
    ) -> GpuDrawableTransform {
        if self.taps.is_empty() {
            return frame.to_gpu();
        }
        let peak = self
            .taps
            .iter()
            .fold((frame.scale.0.abs(), frame.scale.1.abs()), |peak, tap| {
                (peak.0.max(tap.scale.0.abs()), peak.1.max(tap.scale.1.abs()))
            });
        let reference = |factor: f64| {
            DrawableTransform {
                scale: (
                    (peak.0 * factor).copysign(frame.scale.0),
                    (peak.1 * factor).copysign(frame.scale.1),
                ),
                ..*frame
            }
            .to_gpu()
        };

        let mut factor = 1.0;
        let mut transform = reference(factor);
        let mut bounds = self.layer_bounds(transform, min, max, canvas);
        for _ in 0..MAX_LAYER_FITS {
            let Some([x0, y0, x1, y1]) = bounds else {
                break;
            };
            let fit = f64::from((layer[0] / (x1 - x0)).min(layer[1] / (y1 - y0)));
            if fit >= 1.0 || !fit.is_finite() {
                break;
            }
            // Slightly under, since the margins around the bounds do not shrink.
            factor = (factor * fit * 0.99).max(MIN_LAYER_FACTOR);
            transform = reference(factor);
            bounds = self.layer_bounds(transform, min, max, canvas);
        }
        let Some([x0, y0, x1, y1]) = bounds else {
            return transform;
        };
        // Keep the layer where it is, pixel for pixel, unless it must move to hold them.
        let shift = |low: f32, high: f32, layer: f32| {
            if low >= 0.0 && high <= layer {
                0.0
            } else if high - low > layer {
                -((low + high - layer) / 2.0).floor()
            } else {
                -low.floor()
            }
        };
        transform.translated([shift(x0, x1, layer[0]), shift(y0, y1, layer[1])])
    }

    /// The part of the box from `min` to `max`, as `layer` holds it, that any tap shows on
    /// the canvas, with a margin for filtering and antialiasing. `None` when no tap shows
    /// any of it.
    fn layer_bounds(
        &self,
        layer: GpuDrawableTransform,
        min: [f32; 2],
        max: [f32; 2],
        canvas: [f32; 2],
    ) -> Option<[f32; 4]> {
        let [box_min_x, box_min_y, box_max_x, box_max_y] = layer.canvas_bounds(min, max);
        // A canvas pixel for the filter, and another for antialiasing.
        let margin = 2.0;
        self.gpu_taps()
            .filter(|tap| tap.opacity > 0.0)
            .filter_map(|tap| {
                let shown = [
                    [-1.0, -1.0],
                    [canvas[0] + 1.0, -1.0],
                    [-1.0, canvas[1] + 1.0],
                    [canvas[0] + 1.0, canvas[1] + 1.0],
                ]
                .map(|corner| layer.to_canvas(tap.to_local(corner)));
                let [x0, y0, x1, y1] = shown.iter().fold(
                    [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
                    |[x0, y0, x1, y1], [x, y]| [x0.min(*x), y0.min(*y), x1.max(*x), y1.max(*y)],
                );
                let bounds = [
                    x0.max(box_min_x - margin),
                    y0.max(box_min_y - margin),
                    x1.min(box_max_x + margin),
                    y1.min(box_max_y + margin),
                ];
                (bounds[0] < bounds[2] && bounds[1] < bounds[3]).then_some(bounds)
            })
            .reduce(union_bounds)
    }

    pub(crate) fn to_gpu(&self) -> GpuDrawableMotion {
        let mut gpu = GpuDrawableMotion::zeroed();
        gpu.tap_count = self.taps.len() as u32;
        for (slot, tap) in gpu.taps.iter_mut().zip(self.gpu_taps()) {
            *slot = tap;
        }
        gpu
    }
}

fn union_bounds(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    if a.iter().chain(&b).any(|value| value.is_nan()) {
        return [f32::NAN; 4];
    }
    [
        a[0].min(b[0]),
        a[1].min(b[1]),
        a[2].max(b[2]),
        a[3].max(b[3]),
    ]
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

    /// Maps a canvas point to the untransformed point under it, like `drawable_to_local`.
    pub(crate) fn to_local(self, pixel: [f32; 2]) -> [f32; 2] {
        if self.flags & TRANSFORM_LINEAR == 0 {
            return pixel;
        }
        let [column_x, column_y] = self.inverse_linear;
        [
            column_x[0] * pixel[0] + column_y[0] * pixel[1] + self.inverse_offset[0],
            column_x[1] * pixel[0] + column_y[1] * pixel[1] + self.inverse_offset[1],
        ]
    }

    /// This map followed by a move of `shift` canvas pixels. The map is then linear, since
    /// it has an offset.
    pub(crate) fn translated(mut self, shift: [f32; 2]) -> Self {
        let [inverse_x, inverse_y] = self.inverse_linear;
        self.offset = [self.offset[0] + shift[0], self.offset[1] + shift[1]];
        self.inverse_offset = [
            self.inverse_offset[0] - (inverse_x[0] * shift[0] + inverse_y[0] * shift[1]),
            self.inverse_offset[1] - (inverse_x[1] * shift[0] + inverse_y[1] * shift[1]),
        ];
        self.flags |= TRANSFORM_LINEAR;
        self
    }

    /// Canvas-space `[min_x, min_y, max_x, max_y]` of the untransformed box from `min` to
    /// `max`, widened by the single-precision rounding of the shader's map. All NaN when a
    /// corner does not land on a finite point, or when the map or box reaches past
    /// `MAX_BOUNDED_MAGNITUDE`, where that rounding is not worth bounding.
    pub(crate) fn canvas_bounds(self, min: [f32; 2], max: [f32; 2]) -> [f32; 4] {
        let corners = [min, [max[0], min[1]], [min[0], max[1]], max];
        let corners = corners.map(|corner| self.to_canvas(corner));
        let magnitude = [min, max]
            .iter()
            .flatten()
            .chain(self.linear.iter().flatten())
            .chain(&self.offset)
            .chain(corners.iter().flatten())
            .fold(0.0_f32, |largest, value| largest.max(value.abs()));
        if corners.iter().flatten().any(|value| !value.is_finite())
            || f64::from(magnitude) > MAX_BOUNDED_MAGNITUDE
        {
            return [f32::NAN; 4];
        }
        let pad = (ROUNDING_ERROR_FACTOR * f64::from(f32::EPSILON)) as f32 * magnitude;
        let [min_x, min_y, max_x, max_y] = corners.iter().fold(
            [f32::MAX, f32::MAX, f32::MIN, f32::MIN],
            |[min_x, min_y, max_x, max_y], [x, y]| {
                [min_x.min(*x), min_y.min(*y), max_x.max(*x), max_y.max(*y)]
            },
        );
        [min_x - pad, min_y - pad, max_x + pad, max_y + pad]
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuDrawableMotion {
    /// Taps to average, or 0 to draw with the frame's transform alone.
    pub(crate) tap_count: u32,
    pub(crate) _pad: [u32; 3],
    pub(crate) taps: [GpuDrawableTransform; MAX_MOTION_TAPS],
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
    fn taps_move_geometry_laid_out_at_the_frame() {
        let frame = transform((1.0, 1.0), 0.0);
        let close = DrawableTransform {
            position: (14.0, 20.0),
            rotation: 90.0,
            ..frame
        };
        let gpu = close.to_gpu_from(frame.position);
        // The pivot moves with the position, and the geometry turns about it.
        let pivot = gpu.to_canvas([15.0, 25.0]);
        assert!((pivot[0] - 19.0).abs() < 1e-5 && (pivot[1] - 25.0).abs() < 1e-5);
        let turned = gpu.to_canvas([16.0, 25.0]);
        assert!((turned[0] - 19.0).abs() < 1e-5 && (turned[1] - 26.0).abs() < 1e-5);
        let back = apply(gpu.inverse_linear, gpu.inverse_offset, turned);
        assert!((back[0] - 16.0).abs() < 1e-5 && (back[1] - 25.0).abs() < 1e-5);

        let motion = DrawableMotion::new(
            Some(&Shutter {
                open: frame,
                frame,
                close,
            }),
            [10.0, 20.0],
            [20.0, 30.0],
        );
        let gpu = motion.to_gpu();
        assert!(gpu.tap_count >= 2);
        let bounds = motion.canvas_bounds(frame.to_gpu(), [10.0, 20.0], [20.0, 30.0]);
        assert!(bounds[2] > 20.0);
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

    #[test]
    fn blurred_drawable_is_visible_while_some_tap_has_a_scale() {
        let frame = transform((0.0, 1.0), 0.0);
        let shutter = Shutter {
            open: transform((-0.5, 1.0), 0.0),
            frame,
            close: transform((0.5, 1.0), 0.0),
        };
        let motion = DrawableMotion::new(Some(&shutter), [10.0, 20.0], [20.0, 30.0]);
        assert!(motion.is_visible(&frame));
        assert!(motion.smallest_scale(&frame) > 0.001);
        assert!(!DrawableMotion::default().is_visible(&frame));
        let hidden = DrawableTransform {
            opacity: 0.0,
            ..transform((1.0, 1.0), 0.0)
        };
        assert!(!motion.is_visible(&hidden));
    }
}

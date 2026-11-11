use crate::error::RenderError;
use crate::node::motion::{self, Lerp, MAX_BOUNDED_MAGNITUDE, ROUNDING_ERROR_FACTOR, Shutter};
use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, JsonNode, NodeId, NodeParams,
    processing::{
        RasterSampling,
        minify::{self, Minification},
    },
};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("transform.wgsl"),
    include_str!("../../gpu/shaders/minify.wgsl")
);

/// Transforms a raster into the composition canvas bounds.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct TransformParams {
    /// Horizontal scale multiplier.
    #[meta(name = "Scale X", step = 0.1)]
    pub scale_x: f64,
    /// Vertical scale multiplier.
    #[meta(name = "Scale Y", step = 0.1)]
    pub scale_y: f64,
    /// Horizontal translation in pixels.
    #[meta(name = "Translate X", step = 1)]
    pub translate_x: f64,
    /// Vertical translation in pixels.
    #[meta(name = "Translate Y", step = 1)]
    pub translate_y: f64,
    /// Rotation in degrees.
    #[meta(name = "Rotate", step = 1)]
    pub rotate: f64,
    /// Horizontal pivot point in pixels.
    #[meta(name = "Pivot X", step = 1)]
    pub pivot_x: f64,
    /// Vertical pivot point in pixels.
    #[meta(name = "Pivot Y", step = 1)]
    pub pivot_y: f64,
    /// Uses the pivot as given; when off, the pivot is the source centre.
    #[meta(name = "Custom pivot")]
    pub custom_pivot: bool,
    /// Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame.
    #[meta(name = "Motion blur", min = 0, max = 360, step = 1)]
    pub motion_blur: f64,
    /// Sampling filter used when transforming.
    #[meta(kind = "enum", enum_type = RasterSampling)]
    pub sampling: RasterSampling,
}

impl Default for TransformParams {
    fn default() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
            custom_pivot: false,
            motion_blur: 0.0,
            sampling: RasterSampling::Linear,
        }
    }
}

/// Transforms a raster into the composition canvas bounds.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "transform", name = "Transform", category = "processing")]
pub struct Transform {
    pub id: NodeId,
    #[params]
    pub params: TransformParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

impl Transform {
    /// A pivot is only read while `custom_pivot` is on, so one set without it would be ignored.
    fn reject_ignored_pivot(&self) -> crate::Result<()> {
        let moved = |pivot: &Deferred<f64>| match pivot {
            Deferred::Value(value) => *value != 0.0,
            Deferred::Expr(_) => true,
        };
        if matches!(self.params.custom_pivot, Deferred::Value(false))
            && (moved(&self.params.pivot_x) || moved(&self.params.pivot_y))
        {
            return Err(RenderError::NodeEvaluation {
                node_id: self.id,
                node_kind: <Transform as JsonNode>::KIND,
                details: "a pivot only applies when `custom_pivot` is on; set custom_pivot: true"
                    .to_string(),
            }
            .into());
        }
        Ok(())
    }
}

/// The largest rotation, in radians, that bounds are taken for. Past it a GPU's sine and cosine
/// of the single-precision angle are not reliable enough to pad for.
const MAX_BOUNDED_RADIANS: f64 = 1000.0;

/// The error assumed in the shader's sine and cosine, as a fraction of the distance from the
/// pivot, which rotating moves a point by.
const SHADER_ANGLE_ERROR: f64 = 1e-3;

/// What a `transform` node needs to place its source at a frame.
#[derive(Debug, Clone)]
struct TransformMotion {
    node_id: NodeId,
    params: TransformParamsDelegate,
    source_size: lumen_gpu::Size,
}

impl TransformMotion {
    /// The taps to average over the shutter, or `None` when motion blur is off or nothing
    /// moves.
    fn taps(
        &self,
        evaluated: &TransformParams,
        expr: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Option<Vec<TransformKey>>> {
        let Some(shutter) = self.shutter(evaluated, expr)? else {
            return Ok(None);
        };
        let (width, height) = (
            f64::from(self.source_size.width),
            f64::from(self.source_size.height),
        );
        let corners = [[0.0, 0.0], [width, 0.0], [0.0, height], [width, height]];
        Ok(Some(
            shutter.taps(&corners, |key, point| key.to_canvas(point)),
        ))
    }

    fn bounds_of_keys(&self, data: lumen_gpu::RectI, keys: &[TransformKey]) -> Option<[f32; 4]> {
        let (x0, y0) = (f64::from(data.x) - 0.5, f64::from(data.y) - 0.5);
        let (x1, y1) = (
            x0 + f64::from(data.width) + 1.0,
            y0 + f64::from(data.height) + 1.0,
        );
        let corners = [[x0, y0], [x1, y0], [x0, y1], [x1, y1]];
        let mut bounds = [f64::MAX, f64::MAX, f64::MIN, f64::MIN];
        let mut pad = 2.0;
        let mut magnitude = 0.0_f64;
        for key in keys {
            let tap = key.sampled().to_gpu();
            let radians = f64::from(tap.rotate_radians);
            if radians.is_nan() || radians.abs() > MAX_BOUNDED_RADIANS {
                return None;
            }
            let (sin, cos) = radians.sin_cos();
            let [scale, translate, pivot] =
                [tap.scale, tap.translate, tap.pivot].map(|pair| pair.map(f64::from));
            magnitude = [scale, translate, pivot]
                .iter()
                .flatten()
                .fold(magnitude, |largest, value| largest.max(value.abs()));
            for corner in corners {
                let x = (corner[0] - pivot[0]) * scale[0];
                let y = (corner[1] - pivot[1]) * scale[1];
                let point = [
                    x * cos - y * sin + pivot[0] + translate[0],
                    x * sin + y * cos + pivot[1] + translate[1],
                ];
                if !point.iter().all(|value| value.is_finite()) {
                    return None;
                }
                magnitude = [corner[0], corner[1], x, y, point[0], point[1]]
                    .iter()
                    .fold(magnitude, |largest, value| largest.max(value.abs()));
                bounds = [
                    bounds[0].min(point[0]),
                    bounds[1].min(point[1]),
                    bounds[2].max(point[0]),
                    bounds[3].max(point[1]),
                ];
                if radians != 0.0 {
                    pad = f64::max(pad, 2.0 + SHADER_ANGLE_ERROR * x.hypot(y));
                }
            }
        }
        if magnitude > MAX_BOUNDED_MAGNITUDE {
            return None;
        }
        // Single-precision rounding in the shader grows with the largest value it handles.
        pad += ROUNDING_ERROR_FACTOR * f64::from(f32::EPSILON) * magnitude;
        let [min_x, min_y, max_x, max_y] = bounds;
        Some([min_x - pad, min_y - pad, max_x + pad, max_y + pad].map(|value| value as f32))
    }

    /// The transform at either end of the shutter and at the frame, or `None` when motion
    /// blur is off or nothing moves.
    fn shutter(
        &self,
        evaluated: &TransformParams,
        expr: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Option<Shutter<TransformKey>>> {
        let Some(offset) = motion::shutter_offset(evaluated.motion_blur) else {
            return Ok(None);
        };
        let params = &self.params;
        let at = |subframe: f64| -> crate::Result<TransformKey> {
            let expr = expr.at_subframe(subframe);
            let eval = |value: &crate::node::Deferred<f64>, name: &str| {
                value.eval(self.node_id, name, &expr)
            };
            Ok(self.key(&TransformParams {
                scale_x: eval(&params.scale_x, "scale_x")?,
                scale_y: eval(&params.scale_y, "scale_y")?,
                translate_x: eval(&params.translate_x, "translate_x")?,
                translate_y: eval(&params.translate_y, "translate_y")?,
                rotate: eval(&params.rotate, "rotate")?,
                pivot_x: eval(&params.pivot_x, "pivot_x")?,
                pivot_y: eval(&params.pivot_y, "pivot_y")?,
                ..evaluated.clone()
            }))
        };
        let shutter = Shutter {
            open: at(-offset)?,
            frame: self.key(evaluated),
            close: at(offset)?,
        };
        Ok((!shutter.is_static()).then_some(shutter))
    }

    fn key(&self, params: &TransformParams) -> TransformKey {
        let pivot = resolve_pivot(
            params.custom_pivot,
            [params.pivot_x as f32, params.pivot_y as f32],
            self.source_size,
        );
        TransformKey {
            scale: (params.scale_x, params.scale_y),
            translate: (params.translate_x, params.translate_y),
            pivot: (f64::from(pivot[0]), f64::from(pivot[1])),
            rotate: params.rotate,
        }
    }
}

/// The `transform` node's motion params at one time, with the pivot resolved.
#[derive(Debug, Clone, Copy, PartialEq)]
struct TransformKey {
    scale: (f64, f64),
    translate: (f64, f64),
    pivot: (f64, f64),
    rotate: f64,
}

impl Lerp for TransformKey {
    fn lerp(&self, other: &Self, t: f64) -> Self {
        Self {
            scale: motion::lerp2(self.scale, other.scale, t),
            translate: motion::lerp2(self.translate, other.translate, t),
            pivot: motion::lerp2(self.pivot, other.pivot, t),
            rotate: motion::lerp(self.rotate, other.rotate, t),
        }
    }
}

impl TransformKey {
    /// Where the source pixel at `point` lands on the canvas.
    fn to_canvas(self, point: [f64; 2]) -> [f64; 2] {
        let (sin, cos) = self.rotate.to_radians().sin_cos();
        let x = (point[0] - self.pivot.0) * self.scale.0;
        let y = (point[1] - self.pivot.1) * self.scale.1;
        [
            x * cos - y * sin + self.pivot.0 + self.translate.0,
            x * sin + y * cos + self.pivot.1 + self.translate.1,
        ]
    }

    /// Texels of the source that one output pixel covers along the source's axes: the length of
    /// each row of the matrix that maps output pixels to source ones, which rotating does not
    /// change.
    fn footprint(self) -> [f32; 2] {
        let key = self.sampled();
        [
            (1.0 / key.scale.0.abs()) as f32,
            (1.0 / key.scale.1.abs()) as f32,
        ]
    }

    /// This key with each scale that the shader would replace, by its clamp, replaced.
    fn sampled(self) -> Self {
        let clamp = |scale: f64| {
            if scale.abs() <= 0.000001 {
                0.000001
            } else {
                scale
            }
        };
        Self {
            scale: (clamp(self.scale.0), clamp(self.scale.1)),
            ..self
        }
    }

    fn to_gpu(self) -> params::TransformTap {
        params::TransformTap {
            scale: [self.scale.0 as f32, self.scale.1 as f32],
            translate: [self.translate.0 as f32, self.translate.1 as f32],
            pivot: [self.pivot.0 as f32, self.pivot.1 as f32],
            rotate_radians: (self.rotate as f32).to_radians(),
            lod: 0.0,
            footprint: [0.0; 2],
            _pad: [0; 2],
        }
    }
}

/// Without a custom pivot the pivot is the source centre.
fn resolve_pivot(custom_pivot: bool, pivot: [f32; 2], source_size: lumen_gpu::Size) -> [f32; 2] {
    if custom_pivot {
        pivot
    } else {
        [
            source_size.width as f32 * 0.5,
            source_size.height as f32 * 0.5,
        ]
    }
}

impl crate::gpu::frame_graph::RecordNode for Transform {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        crate::gpu::mips::programs(programs)?;
        programs.spatial("transform", SHADER)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        self.reject_ignored_pivot()?;
        let source = cx.required(self.id, "source", self.source)?;
        let motion = TransformMotion {
            node_id: self.id,
            params: self.params.clone(),
            source_size: source.domain.display_size(),
        };
        let (evaluated, taps) = {
            let expr = cx.expr();
            let mut evaluated = self.params.eval(&DelegateEvalContext {
                node_id: self.id,
                expr: &expr,
            })?;
            if !cx.analytic_motion_blur() {
                evaluated.motion_blur = 0.0;
            }
            let taps = motion.taps(&evaluated, &expr)?;
            (evaluated, taps)
        };
        let frame_key = [motion.key(&evaluated)];
        let keys = taps.as_deref().unwrap_or(&frame_key);
        let bounds = motion.bounds_of_keys(source.domain.data_rect, keys);
        let bounds = cx.bounds_of(bounds);
        let domain = cx.source_domain(bounds);
        let pivot = resolve_pivot(
            evaluated.custom_pivot,
            [evaluated.pivot_x as f32, evaluated.pivot_y as f32],
            motion.source_size,
        );
        let mut params = params::TransformParams {
            scale: [evaluated.scale_x as f32, evaluated.scale_y as f32],
            translate: [evaluated.translate_x as f32, evaluated.translate_y as f32],
            pivot,
            rotate_radians: (evaluated.rotate as f32).to_radians(),
            sampling: evaluated.sampling as u32,
            output_origin: [domain.data_rect.x, domain.data_rect.y],
            source_origin: source.origin(),
            ..bytemuck::Zeroable::zeroed()
        };
        let size = domain.storage_size;
        let tex = cx.storage_target(size);
        // Each tap, or the transform alone, is filtered for how far it shrinks the source.
        let mips = Minification::mips_fit(cx, source.domain.storage_size);
        let filters: Vec<Option<Minification>> = keys
            .iter()
            .map(|key| {
                (evaluated.sampling == RasterSampling::Linear)
                    .then(|| Minification::new(key.footprint(), source.domain.storage_size, mips))
                    .flatten()
            })
            .collect();
        let filtered = |filter: Option<&Minification>| {
            filter.map_or((0.0, [0.0; 2]), |filter| (filter.lod, filter.footprint))
        };
        match &taps {
            Some(taps) => {
                params.tap_count = taps.len() as u32;
                for ((gpu, tap), filter) in params.taps.iter_mut().zip(taps).zip(&filters) {
                    *gpu = tap.to_gpu();
                    (gpu.lod, gpu.footprint) = filtered(filter.as_ref());
                }
            }
            None => (params.lod, params.footprint) = filtered(filters[0].as_ref()),
        }
        let levels = filters.iter().flatten().map(|filter| filter.levels()).max();
        let input = minify::source(cx, self.source, source, None, levels.unwrap_or(1))?;
        cx.spatial_pass("transform", SHADER, input, &params, tex, size)?;
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            bounds: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Transform;
    use crate::node::Deferred;

    fn pivot_error(set: impl FnOnce(&mut Transform)) -> Option<String> {
        let mut transform = Transform::default();
        set(&mut transform);
        transform
            .reject_ignored_pivot()
            .err()
            .map(|error| error.to_string())
    }

    #[test]
    fn pivot_without_custom_pivot_is_rejected() {
        assert_eq!(pivot_error(|_| {}), None);
        assert_eq!(
            pivot_error(|transform| transform.params.pivot_x = Deferred::value(0.0)),
            None
        );
        let error = pivot_error(|transform| transform.params.pivot_y = Deferred::value(12.0))
            .expect("a nonzero pivot is ignored");
        assert!(error.contains("custom_pivot: true"), "{error}");
        assert!(
            pivot_error(|transform| {
                transform.params.pivot_x =
                    Deferred::Expr(crate::expr::Expression::parse("frame").unwrap());
            })
            .is_some()
        );
        assert_eq!(
            pivot_error(|transform| {
                transform.params.custom_pivot = Deferred::value(true);
                transform.params.pivot_x = Deferred::value(12.0);
            }),
            None
        );
    }
}

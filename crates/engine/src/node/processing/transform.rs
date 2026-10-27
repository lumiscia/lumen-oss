use crate::node::motion::{self, Lerp, MAX_BOUNDED_MAGNITUDE, ROUNDING_ERROR_FACTOR, Shutter};
use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("transform.wgsl")
);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TransformSampling {
    Nearest = 0,
    #[default]
    Linear = 1,
}

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
    /// Uses the pivot as given; when off, a (0, 0) pivot means the source centre.
    #[meta(name = "Custom pivot")]
    pub custom_pivot: bool,
    /// Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame.
    #[meta(name = "Motion blur", min = 0, max = 360, step = 1)]
    pub motion_blur: f64,
    /// Sampling filter used when transforming.
    #[meta(kind = "enum", enum_type = TransformSampling)]
    pub sampling: TransformSampling,
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
            sampling: TransformSampling::Linear,
        }
    }
}

/// Transforms a raster into the composition canvas bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "transform", name = "Transform", category = "processing")]
pub struct Transform {
    pub id: NodeId,
    #[params]
    pub params: TransformParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: TransformParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Transform {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let source = ctx
            .compile_bounded_port(&self.source)?
            .into_raster(self.source.id, &self.source.port)?;
        let motion = TransformMotion {
            node_id: self.id,
            params: self.params.clone(),
            source_size: source.domain.display_size(),
        };
        let bounds = ctx.source_bounds(
            self.id,
            compiler::SourceMotion::of(motion.is_animated(), &motion.params),
            |expr| motion.canvas_bounds(source.domain.data_rect, expr),
        );
        let domain = ctx.source_domain(bounds);
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "transform",
            SHADER,
            std::mem::size_of::<compiler::TransformParams>() as u64,
            compiler::FilterDomain::Bounded(domain),
        )?;
        ctx.register_compiled_node(CompiledTransform {
            motion,
            straight_source: source.metadata.alpha_mode == AlphaMode::Unpremultiplied,
            output_origin: [domain.data_rect.x, domain.data_rect.y],
            source_origin: [source.domain.data_rect.x, source.domain.data_rect.y],
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain,
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledTransform {
    motion: TransformMotion,
    straight_source: bool,
    /// Canvas pixel of the output's first texel.
    output_origin: [i32; 2],
    /// Source pixel of the source's first texel.
    source_origin: [i32; 2],
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledTransform {
    fn node_id(&self) -> NodeId {
        self.motion.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let expr = ctx.expr_context(self.motion.node_id, "params");
        let evaluated = self.motion.params.eval(&NodeParamEvalContext {
            node_id: self.motion.node_id,
            expr: &expr,
        })?;
        let pivot = resolve_pivot(
            evaluated.custom_pivot,
            [evaluated.pivot_x as f32, evaluated.pivot_y as f32],
            self.motion.source_size,
        );
        let mut params = compiler::TransformParams {
            scale: [evaluated.scale_x as f32, evaluated.scale_y as f32],
            translate: [evaluated.translate_x as f32, evaluated.translate_y as f32],
            pivot,
            rotate_radians: (evaluated.rotate as f32).to_radians(),
            sampling: evaluated.sampling as u32,
            straight_source: u32::from(self.straight_source),
            output_origin: self.output_origin,
            source_origin: self.source_origin,
            ..bytemuck::Zeroable::zeroed()
        };
        if let Some(taps) = self.motion.taps(&evaluated, &expr)? {
            params.tap_count = taps.len() as u32;
            for (gpu, tap) in params.taps.iter_mut().zip(&taps) {
                *gpu = tap.to_gpu();
            }
        }
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
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
    /// Whether the placement may change between frames.
    fn is_animated(&self) -> bool {
        let params = &self.params;
        params.scale_x.is_expr()
            || params.scale_y.is_expr()
            || params.translate_x.is_expr()
            || params.translate_y.is_expr()
            || params.rotate.is_expr()
            || params.pivot_x.is_expr()
            || params.pivot_y.is_expr()
            || params.custom_pivot.is_expr()
            || params.motion_blur.is_expr()
    }

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

    /// Canvas-space `[min_x, min_y, max_x, max_y]` that the source's pixels within `data`
    /// can show on at the frame of `expr`, under each tap when blurred. A source pixel
    /// shows wherever the filter footprint around an output pixel reaches it.
    ///
    /// The placement is taken from the single-precision values that the shader receives.
    /// `None` when it is not finite or turns by more than `MAX_BOUNDED_RADIANS`, where the
    /// shader's sine and cosine cannot be bounded.
    fn canvas_bounds(
        &self,
        data: lumen_gpu::RectI,
        expr: &crate::expr::ExpressionContext<'_>,
    ) -> crate::Result<Option<[f32; 4]>> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr,
        })?;
        let keys = self
            .taps(&evaluated, expr)?
            .unwrap_or_else(|| vec![self.key(&evaluated)]);
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
                return Ok(None);
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
                    return Ok(None);
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
            return Ok(None);
        }
        // Single-precision rounding in the shader grows with the largest value it handles.
        pad += ROUNDING_ERROR_FACTOR * f64::from(f32::EPSILON) * magnitude;
        let [min_x, min_y, max_x, max_y] = bounds;
        Ok(Some(
            [min_x - pad, min_y - pad, max_x + pad, max_y + pad].map(|value| value as f32),
        ))
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

    fn to_gpu(self) -> compiler::TransformTap {
        compiler::TransformTap {
            scale: [self.scale.0 as f32, self.scale.1 as f32],
            translate: [self.translate.0 as f32, self.translate.1 as f32],
            pivot: [self.pivot.0 as f32, self.pivot.1 as f32],
            rotate_radians: (self.rotate as f32).to_radians(),
            _pad: 0,
        }
    }
}

/// Without a custom pivot, a pivot of (0, 0) within 1e-6 selects the source centre.
fn resolve_pivot(custom_pivot: bool, pivot: [f32; 2], source_size: lumen_gpu::Size) -> [f32; 2] {
    if custom_pivot || pivot.iter().any(|component| component.abs() > 0.000001) {
        pivot
    } else {
        [
            source_size.width as f32 * 0.5,
            source_size.height as f32 * 0.5,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_pivot;

    #[test]
    fn legacy_pivot_fallback_tolerates_near_zero() {
        let size = lumen_gpu::Size {
            width: 8,
            height: 4,
        };
        assert_eq!(resolve_pivot(false, [0.0000001, 0.0], size), [4.0, 2.0]);
        assert_eq!(resolve_pivot(false, [0.0, 0.0001], size), [0.0, 0.0001]);
        assert_eq!(
            resolve_pivot(true, [0.0000001, 0.0], size),
            [0.0000001, 0.0]
        );
    }
}

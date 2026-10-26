use crate::node::motion::{self, Lerp, Shutter};
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
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "transform",
            SHADER,
            std::mem::size_of::<compiler::TransformParams>() as u64,
            Some(lumen_gpu::Size::new(
                ctx.composition().render_settings.width.max(1),
                ctx.composition().render_settings.height.max(1),
            )),
        )?;
        ctx.register_compiled_node(CompiledTransform {
            node_id: self.id,
            params: self.params.clone(),
            source_size: source.domain.storage_size,
            straight_source: source.metadata.alpha_mode == AlphaMode::Unpremultiplied,
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(lumen_gpu::Size::new(
                ctx.composition().render_settings.width.max(1),
                ctx.composition().render_settings.height.max(1),
            )),
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledTransform {
    node_id: NodeId,
    params: TransformParamsDelegate,
    source_size: lumen_gpu::Size,
    straight_source: bool,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledTransform {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let expr = ctx.expr_context(self.node_id, "params");
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &expr,
        })?;
        let pivot = resolve_pivot(
            evaluated.custom_pivot,
            [evaluated.pivot_x as f32, evaluated.pivot_y as f32],
            self.source_size,
        );
        let mut params = compiler::TransformParams {
            scale: [evaluated.scale_x as f32, evaluated.scale_y as f32],
            translate: [evaluated.translate_x as f32, evaluated.translate_y as f32],
            pivot,
            rotate_radians: (evaluated.rotate as f32).to_radians(),
            sampling: evaluated.sampling as u32,
            straight_source: u32::from(self.straight_source),
            ..bytemuck::Zeroable::zeroed()
        };
        if let Some(shutter) = self.shutter(&evaluated, &expr)? {
            let (width, height) = (
                f64::from(self.source_size.width),
                f64::from(self.source_size.height),
            );
            let corners = [[0.0, 0.0], [width, 0.0], [0.0, height], [width, height]];
            let taps = shutter.taps(&corners, |key, point| key.to_canvas(point));
            params.tap_count = taps.len() as u32;
            for (gpu, tap) in params.taps.iter_mut().zip(&taps) {
                *gpu = tap.to_gpu();
            }
        }
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl CompiledTransform {
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

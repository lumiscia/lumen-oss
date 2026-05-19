use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("transform.wgsl");

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::NodeEnum)]
#[repr(i64)]
pub enum TransformSampling {
    Nearest = 0,
    Linear = 1,
}

impl TransformSampling {
    pub fn from_int(value: i64) -> Self {
        if value == Self::Nearest as i64 {
            Self::Nearest
        } else {
            Self::Linear
        }
    }
}

/// Transforms a raster inside its existing static bounds.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedTransformParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct TransformParams {
    /// Horizontal scale multiplier.
    #[param(kind = "float", name = "Scale X", step = 0.1)]
    pub scale_x: Deferred<f64>,
    /// Vertical scale multiplier.
    #[param(kind = "float", name = "Scale Y", step = 0.1)]
    pub scale_y: Deferred<f64>,
    /// Horizontal translation in pixels.
    #[param(kind = "float", name = "Translate X", step = 1)]
    pub translate_x: Deferred<f64>,
    /// Vertical translation in pixels.
    #[param(kind = "float", name = "Translate Y", step = 1)]
    pub translate_y: Deferred<f64>,
    /// Rotation in degrees.
    #[param(kind = "float", name = "Rotate", step = 1)]
    pub rotate: Deferred<f64>,
    /// Horizontal pivot point in pixels.
    #[param(kind = "float", name = "Pivot X", step = 1)]
    pub pivot_x: Deferred<f64>,
    /// Vertical pivot point in pixels.
    #[param(kind = "float", name = "Pivot Y", step = 1)]
    pub pivot_y: Deferred<f64>,
    /// Sampling filter used when transforming.
    #[param(kind = "enum", enum_type = TransformSampling)]
    pub sampling: Deferred<i64>,
}

impl Default for TransformParams {
    fn default() -> Self {
        Self {
            scale_x: Deferred::value(1.0),
            scale_y: Deferred::value(1.0),
            translate_x: Deferred::value(0.0),
            translate_y: Deferred::value(0.0),
            rotate: Deferred::value(0.0),
            pivot_x: Deferred::value(0.0),
            pivot_y: Deferred::value(0.0),
            sampling: Deferred::value(TransformSampling::Linear as i64),
        }
    }
}

/// Transforms a raster inside its existing static bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "transform", name = "Transform", category = "processing")]
pub struct Transform {
    pub id: NodeId,
    #[params]
    pub params: TransformParams,

    #[input()]
    pub source: PortRef,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: TransformParams::default(),
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
        )?;
        ctx.push_frame_binding(TransformFrameBinding {
            node_id: self.id,
            scale_x: self.params.scale_x.clone(),
            scale_y: self.params.scale_y.clone(),
            translate_x: self.params.translate_x.clone(),
            translate_y: self.params.translate_y.clone(),
            rotate: self.params.rotate.clone(),
            pivot_x: self.params.pivot_x.clone(),
            pivot_y: self.params.pivot_y.clone(),
            sampling: self.params.sampling.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct TransformFrameBinding {
    node_id: NodeId,
    scale_x: Deferred<f64>,
    scale_y: Deferred<f64>,
    translate_x: Deferred<f64>,
    translate_y: Deferred<f64>,
    rotate: Deferred<f64>,
    pivot_x: Deferred<f64>,
    pivot_y: Deferred<f64>,
    sampling: Deferred<i64>,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for TransformFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = compiler::TransformParams {
            scale: [
                self.scale_x.resolve_float(
                    self.node_id,
                    "scale_x",
                    &ctx.expr_context(self.node_id, "scale_x"),
                )? as f32,
                self.scale_y.resolve_float(
                    self.node_id,
                    "scale_y",
                    &ctx.expr_context(self.node_id, "scale_y"),
                )? as f32,
            ],
            translate: [
                self.translate_x.resolve_float(
                    self.node_id,
                    "translate_x",
                    &ctx.expr_context(self.node_id, "translate_x"),
                )? as f32,
                self.translate_y.resolve_float(
                    self.node_id,
                    "translate_y",
                    &ctx.expr_context(self.node_id, "translate_y"),
                )? as f32,
            ],
            pivot: [
                self.pivot_x.resolve_float(
                    self.node_id,
                    "pivot_x",
                    &ctx.expr_context(self.node_id, "pivot_x"),
                )? as f32,
                self.pivot_y.resolve_float(
                    self.node_id,
                    "pivot_y",
                    &ctx.expr_context(self.node_id, "pivot_y"),
                )? as f32,
            ],
            rotate_radians: (self.rotate.resolve_float(
                self.node_id,
                "rotate",
                &ctx.expr_context(self.node_id, "rotate"),
            )? as f32)
                .to_radians(),
            sampling: TransformSampling::from_int(self.sampling.resolve_int(
                self.node_id,
                "sampling",
                &ctx.expr_context(self.node_id, "sampling"),
            )?) as u32,
            _pad: [0; 4],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

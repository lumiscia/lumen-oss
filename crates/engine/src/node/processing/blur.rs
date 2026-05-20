use crate::node::{Deferred, NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("blur.wgsl");

#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedBlurParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct BlurParams {
    /// Blur radius in pixels.
    #[param(kind = "float", min = 0, step = 0.5)]
    pub radius: Deferred<f64>,
}

impl Default for BlurParams {
    fn default() -> Self {
        Self {
            radius: Deferred::value(4.0),
        }
    }
}

/// Applies a simple box blur to a raster.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "blur", name = "Blur", category = "processing")]
pub struct Blur {
    pub id: NodeId,
    #[params]
    pub params: BlurParams,
    #[input()]
    pub source: PortRef,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: BlurParams::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct BlurFrameBinding {
    node_id: NodeId,
    params: BlurParams,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for BlurFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let radius = params.radius.round().clamp(0.0, 32.0) as u32;
        let params = compiler::BlurParams {
            values: [radius, 0, 0, 0],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for Blur {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "blur",
            SHADER,
            std::mem::size_of::<compiler::BlurParams>() as u64,
        )?;
        ctx.push_frame_binding(BlurFrameBinding {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

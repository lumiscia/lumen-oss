use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("opacity.wgsl");

/// Parameters for uniformly adjusting raster opacity.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct OpacityParams {
    /// Opacity multiplier applied to the raster.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
}

impl Default for OpacityParams {
    fn default() -> Self {
        Self { opacity: 1.0 }
    }
}

/// Uniformly adjusts raster opacity.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "opacity", name = "Opacity", category = "processing")]
pub struct Opacity {
    pub id: NodeId,
    #[params]
    pub params: OpacityParamsDelegate,
    #[input()]
    pub source: PortRef,
}

impl Default for Opacity {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: OpacityParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledOpacity {
    node_id: NodeId,
    params: OpacityParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledOpacity {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let params = compiler::OpacityParams {
            values: [evaluated.opacity.clamp(0.0, 1.0) as f32, 0.0, 0.0, 0.0],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for Opacity {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "opacity",
            SHADER,
            std::mem::size_of::<compiler::OpacityParams>() as u64,
        )?;
        ctx.register_compiled_node(CompiledOpacity {
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

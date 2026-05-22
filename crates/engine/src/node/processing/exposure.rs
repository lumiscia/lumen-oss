use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("exposure.wgsl");

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ExposureParams {
    /// Exposure offset in stops.
    #[meta(step = 0.01)]
    pub exposure: f64,
    /// Contrast multiplier.
    #[meta(min = 0, step = 0.01)]
    pub contrast: f64,
    /// Linear color offset.
    #[meta(step = 0.01)]
    pub offset: f64,
}

impl Default for ExposureParams {
    fn default() -> Self {
        Self {
            exposure: 0.0,
            contrast: 1.0,
            offset: 0.0,
        }
    }
}

/// Adjusts raster exposure, contrast, and offset.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "exposure", name = "Exposure", category = "processing")]
pub struct Exposure {
    pub id: NodeId,
    #[params]
    pub params: ExposureParamsDelegate,
    #[input()]
    pub source: PortRef,
}

impl Default for Exposure {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ExposureParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct ExposureFrameBinding {
    node_id: NodeId,
    params: ExposureParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for ExposureFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let params = compiler::ExposureParams {
            exposure: params.exposure as f32,
            contrast: params.contrast as f32,
            offset: params.offset as f32,
            _pad: 0.0,
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for Exposure {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "exposure",
            SHADER,
            std::mem::size_of::<compiler::ExposureParams>() as u64,
        )?;
        ctx.push_frame_binding(ExposureFrameBinding {
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

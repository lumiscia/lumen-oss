use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::{
    AlphaMode, BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("exposure.wgsl")
);

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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "exposure", name = "Exposure", category = "processing")]
pub struct Exposure {
    pub id: NodeId,
    #[params]
    pub params: ExposureParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[derive(Debug, Clone)]
struct CompiledExposure {
    node_id: NodeId,
    params: ExposureParamsDelegate,
    alpha_mode: AlphaMode,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledExposure {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        let params = compiler::ExposureParams {
            exposure: params.exposure as f32,
            contrast: params.contrast as f32,
            offset: params.offset as f32,
            premultiplied: u32::from(self.alpha_mode == AlphaMode::Premultiplied),
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for Exposure {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            self.source,
            "exposure",
            SHADER,
            std::mem::size_of::<compiler::ExposureParams>() as u64,
            compiler::FilterDomain::PremultipliedSource,
        )?;
        ctx.register_compiled_node(CompiledExposure {
            node_id: self.id,
            params: self.params.clone(),
            alpha_mode: source.alpha_mode,
            buffer: params,
        });
        Ok(RasterHandle {
            texture,
            domain: source.domain,
            alpha_mode: source.alpha_mode,
            bounds: source.bounds,
        })
    }
}

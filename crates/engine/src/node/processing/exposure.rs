use crate::node::{NodeId, NodeParams};

use crate::gpu::{GpuCompileNode, RasterHandle, compiler};

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

impl GpuCompileNode for Exposure {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        ctx.unary_filter(
            self.id,
            self.source,
            "exposure",
            SHADER,
            &self.params,
            |params| compiler::ExposureParams {
                exposure: params.exposure as f32,
                contrast: params.contrast as f32,
                offset: params.offset as f32,
            },
        )
    }
}

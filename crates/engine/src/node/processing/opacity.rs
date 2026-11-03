use crate::node::{NodeId, NodeParams};

use crate::gpu::{GpuCompileNode, RasterHandle, compiler};

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
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "opacity", name = "Opacity", category = "processing")]
pub struct Opacity {
    pub id: NodeId,
    #[params]
    pub params: OpacityParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

impl GpuCompileNode for Opacity {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        ctx.unary_filter(
            self.id,
            self.source,
            "opacity",
            SHADER,
            &self.params,
            |params| compiler::OpacityParams {
                opacity: params.opacity.clamp(0.0, 1.0) as f32,
            },
        )
    }
}

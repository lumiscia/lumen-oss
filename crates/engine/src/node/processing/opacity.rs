use crate::node::{DelegateEvalContext, DelegateEvaluable};
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

impl crate::gpu::frame_graph::RecordNode for Opacity {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.spatial("opacity", SHADER)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let params = compiler::OpacityParams {
            opacity: evaluated.opacity.clamp(0.0, 1.0) as f32,
        };
        cx.filter("opacity", SHADER, source, &params)
    }
}

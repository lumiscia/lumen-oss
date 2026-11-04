use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::params;

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
        let params = params::OpacityParams {
            opacity: evaluated.opacity.clamp(0.0, 1.0) as f32,
        };
        cx.filter("opacity", SHADER, source, &params)
    }
}

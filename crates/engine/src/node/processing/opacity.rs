use bytemuck::{Pod, Zeroable};

use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::frame_graph::ComputeProgram;

pub(crate) const SHADER: &str = include_str!("opacity.wgsl");
const OPACITY: ComputeProgram = ComputeProgram::spatial("opacity", SHADER);

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

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "OpacityParams", source = SHADER)]
struct OpacityUniforms {
    opacity: f32,
}

impl crate::gpu::frame_graph::RecordNode for Opacity {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&OPACITY)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
            pixel_scale: cx.pixel_scale(),
        })?;
        let params = OpacityUniforms {
            opacity: evaluated.opacity.clamp(0.0, 1.0) as f32,
        };
        cx.filter(&OPACITY, source, &params)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}

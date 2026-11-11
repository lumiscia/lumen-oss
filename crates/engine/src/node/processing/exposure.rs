use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
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

impl crate::gpu::frame_graph::RecordNode for Exposure {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.spatial("exposure", SHADER)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let gpu_params = params::ExposureParams {
            exposure: params.exposure as f32,
            contrast: params.contrast as f32,
            offset: params.offset as f32,
        };
        cx.filter("exposure", SHADER, source, &gpu_params)
    }
}

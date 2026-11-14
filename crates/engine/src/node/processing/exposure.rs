use bytemuck::{Pod, Zeroable};

use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::frame_graph::ComputeProgram;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("exposure.wgsl")
);
const EXPOSURE: ComputeProgram = ComputeProgram::spatial("exposure", SHADER);

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ExposureParams {
    /// Exposure in stops, multiplying linear light by 2 to the power of this.
    #[meta(step = 0.01)]
    pub exposure: f64,
    /// Contrast multiplier around sRGB-encoded mid grey, after the exposure.
    #[meta(min = 0, step = 0.01)]
    pub contrast: f64,
    /// Offset added to the sRGB-encoded colour, after the exposure and before the contrast.
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

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "ExposureParams", source = SHADER)]
struct ExposureUniforms {
    exposure: f32,
    contrast: f32,
    offset: f32,
}

impl crate::gpu::frame_graph::RecordNode for Exposure {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&EXPOSURE)
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
        let gpu_params = ExposureUniforms {
            exposure: params.exposure as f32,
            contrast: params.contrast as f32,
            offset: params.offset as f32,
        };
        cx.filter(&EXPOSURE, source, &gpu_params)
    }

    fn magnification(&self) -> f64 {
        1.0
    }
}

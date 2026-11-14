use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::frame_graph::ComputeProgram;
use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("hue_saturation.wgsl")
);
const HUE_SATURATION: ComputeProgram = ComputeProgram::spatial("hue-saturation", SHADER);

/// Adjusts raster hue, saturation, and lightness of the sRGB-encoded colour.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct HueSaturationParams {
    /// Hue rotation in degrees.
    #[meta(name = "Hue", step = 1)]
    pub hue_degrees: f64,
    /// Saturation multiplier.
    #[meta(step = 0.01)]
    pub saturation: f64,
    /// Lightness offset.
    #[meta(step = 0.01)]
    pub lightness: f64,
}

impl Default for HueSaturationParams {
    fn default() -> Self {
        Self {
            hue_degrees: 0.0,
            saturation: 1.0,
            lightness: 0.0,
        }
    }
}

/// Adjusts raster hue, saturation, and lightness of the sRGB-encoded colour.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(
    kind = "hue_saturation",
    name = "Hue Saturation",
    category = "processing"
)]
pub struct HueSaturation {
    pub id: NodeId,
    #[params]
    pub params: HueSaturationParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

impl crate::gpu::frame_graph::RecordNode for HueSaturation {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute(&HUE_SATURATION)
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
        let gpu_params = params::HueSaturationParams {
            hue_offset: (params.hue_degrees / 360.0) as f32,
            saturation: params.saturation as f32,
            lightness: params.lightness as f32,
        };
        cx.filter(&HUE_SATURATION, source, &gpu_params)
    }
}

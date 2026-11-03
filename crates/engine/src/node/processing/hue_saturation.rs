use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::{GpuCompileNode, RasterHandle, compiler};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("hue_saturation.wgsl")
);

/// Adjusts raster hue, saturation, and lightness.
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

/// Adjusts raster hue, saturation, and lightness.
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

impl GpuCompileNode for HueSaturation {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        ctx.unary_filter(
            self.id,
            self.source,
            "hue-saturation",
            SHADER,
            &self.params,
            |params| compiler::HueSaturationParams {
                hue_offset: (params.hue_degrees / 360.0) as f32,
                saturation: params.saturation as f32,
                lightness: params.lightness as f32,
            },
        )
    }
}

impl crate::gpu::frame_graph::RecordNode for HueSaturation {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.spatial("hue-saturation", SHADER)
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
        let gpu_params = compiler::HueSaturationParams {
            hue_offset: (params.hue_degrees / 360.0) as f32,
            saturation: params.saturation as f32,
            lightness: params.lightness as f32,
        };
        cx.filter("hue-saturation", SHADER, source, &gpu_params)
    }
}

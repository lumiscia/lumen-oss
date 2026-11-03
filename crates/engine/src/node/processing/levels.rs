use crate::node::{DelegateEvalContext, DelegateEvaluable};
use crate::node::{NodeId, NodeParams};

use crate::gpu::{GpuCompileNode, RasterHandle, compiler};

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("levels.wgsl")
);

/// Remaps raster black, white, gamma, and output range.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct LevelsParams {
    /// Input black point.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub black_point: f64,
    /// Input white point.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub white_point: f64,
    /// Midtone gamma adjustment.
    #[meta(min = 0.01, step = 0.01)]
    pub gamma: f64,
    /// Output black level.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub output_black: f64,
    /// Output white level.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub output_white: f64,
}

impl Default for LevelsParams {
    fn default() -> Self {
        Self {
            black_point: 0.0,
            white_point: 1.0,
            gamma: 1.0,
            output_black: 0.0,
            output_white: 1.0,
        }
    }
}

/// Remaps raster black, white, gamma, and output range.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "levels", name = "Levels", category = "processing")]
pub struct Levels {
    pub id: NodeId,
    #[params]
    pub params: LevelsParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

impl GpuCompileNode for Levels {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        ctx.unary_filter(
            self.id,
            self.source,
            "levels",
            SHADER,
            &self.params,
            |params| compiler::LevelsParams {
                black_point: params.black_point as f32,
                white_point: params.white_point as f32,
                gamma: params.gamma as f32,
                output_black: params.output_black as f32,
                output_white: params.output_white as f32,
            },
        )
    }
}

impl crate::gpu::frame_graph::RecordNode for Levels {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.spatial("levels", SHADER)
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
        let gpu_params = compiler::LevelsParams {
            black_point: params.black_point as f32,
            white_point: params.white_point as f32,
            gamma: params.gamma as f32,
            output_black: params.output_black as f32,
            output_white: params.output_white as f32,
        };
        cx.filter("levels", SHADER, source, &gpu_params)
    }
}

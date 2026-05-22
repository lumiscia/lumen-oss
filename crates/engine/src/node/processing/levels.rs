use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("levels.wgsl");

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
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "levels", name = "Levels", category = "processing")]
pub struct Levels {
    pub id: NodeId,
    #[params]
    pub params: LevelsParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: LevelsParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct CompiledLevels {
    node_id: NodeId,
    params: LevelsParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledLevels {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let gpu_params = compiler::LevelsParams {
            black_point: params.black_point as f32,
            white_point: params.white_point as f32,
            gamma: params.gamma as f32,
            output_black: params.output_black as f32,
            output_white: params.output_white as f32,
            _pad: [0.0; 3],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}

impl GpuCompileNode for Levels {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "levels",
            SHADER,
            std::mem::size_of::<compiler::LevelsParams>() as u64,
        )?;
        ctx.register_compiled_node(CompiledLevels {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

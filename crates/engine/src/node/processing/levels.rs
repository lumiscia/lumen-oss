use crate::node::{Deferred, NodeId, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("levels.wgsl");

/// Remaps raster black, white, gamma, and output range.
#[derive(Debug, Clone, lumen_macros::NodeParams)]
#[params(evaluated = EvaluatedLevelsParams)]
#[cfg_attr(feature = "json", derive(serde::Deserialize), serde(default))]
pub struct LevelsParams {
    /// Input black point.
    #[param(kind = "float", min = 0, max = 1, step = 0.01)]
    pub black_point: Deferred<f64>,
    /// Input white point.
    #[param(kind = "float", min = 0, max = 1, step = 0.01)]
    pub white_point: Deferred<f64>,
    /// Midtone gamma adjustment.
    #[param(kind = "float", min = 0.01, step = 0.01)]
    pub gamma: Deferred<f64>,
    /// Output black level.
    #[param(kind = "float", min = 0, max = 1, step = 0.01)]
    pub output_black: Deferred<f64>,
    /// Output white level.
    #[param(kind = "float", min = 0, max = 1, step = 0.01)]
    pub output_white: Deferred<f64>,
}

impl Default for LevelsParams {
    fn default() -> Self {
        Self {
            black_point: Deferred::value(0.0),
            white_point: Deferred::value(1.0),
            gamma: Deferred::value(1.0),
            output_black: Deferred::value(0.0),
            output_white: Deferred::value(1.0),
        }
    }
}

/// Remaps raster black, white, gamma, and output range.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "levels", name = "Levels", category = "processing")]
pub struct Levels {
    pub id: NodeId,
    #[params]
    pub params: LevelsParams,

    #[input()]
    pub source: PortRef,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: LevelsParams::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct LevelsFrameBinding {
    node_id: NodeId,
    black_point: Deferred<f64>,
    white_point: Deferred<f64>,
    gamma: Deferred<f64>,
    output_black: Deferred<f64>,
    output_white: Deferred<f64>,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for LevelsFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = compiler::LevelsParams {
            black_point: self.black_point.resolve_float(
                self.node_id,
                "black_point",
                &ctx.expr_context(self.node_id, "black_point"),
            )? as f32,
            white_point: self.white_point.resolve_float(
                self.node_id,
                "white_point",
                &ctx.expr_context(self.node_id, "white_point"),
            )? as f32,
            gamma: self.gamma.resolve_float(
                self.node_id,
                "gamma",
                &ctx.expr_context(self.node_id, "gamma"),
            )? as f32,
            output_black: self.output_black.resolve_float(
                self.node_id,
                "output_black",
                &ctx.expr_context(self.node_id, "output_black"),
            )? as f32,
            output_white: self.output_white.resolve_float(
                self.node_id,
                "output_white",
                &ctx.expr_context(self.node_id, "output_white"),
            )? as f32,
            _pad: [0.0; 3],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
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
        ctx.push_frame_binding(LevelsFrameBinding {
            node_id: self.id,
            black_point: self.params.black_point.clone(),
            white_point: self.params.white_point.clone(),
            gamma: self.params.gamma.clone(),
            output_black: self.params.output_black.clone(),
            output_white: self.params.output_white.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

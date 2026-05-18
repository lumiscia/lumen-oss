use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("levels.wgsl");

/// Remaps raster black, white, gamma, and output range.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "levels", name = "Levels", category = "processing")]
pub struct Levels {
    pub id: NodeId,
    /// Input black point.
    #[property(kind = "float", min = 0, max = 1, step = 0.01)]
    pub black_point: NodeProperty,
    /// Input white point.
    #[property(kind = "float", min = 0, max = 1, step = 0.01)]
    pub white_point: NodeProperty,
    /// Midtone gamma adjustment.
    #[property(kind = "float", min = 0.01, step = 0.01)]
    pub gamma: NodeProperty,
    /// Output black level.
    #[property(kind = "float", min = 0, max = 1, step = 0.01)]
    pub output_black: NodeProperty,
    /// Output white level.
    #[property(kind = "float", min = 0, max = 1, step = 0.01)]
    pub output_white: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Levels {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            black_point: NodeProperty::Float(0.0),
            white_point: NodeProperty::Float(1.0),
            gamma: NodeProperty::Float(1.0),
            output_black: NodeProperty::Float(0.0),
            output_white: NodeProperty::Float(1.0),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct LevelsFrameBinding {
    node_id: NodeId,
    black_point: NodeProperty,
    white_point: NodeProperty,
    gamma: NodeProperty,
    output_black: NodeProperty,
    output_white: NodeProperty,
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
            black_point: self.black_point.clone(),
            white_point: self.white_point.clone(),
            gamma: self.gamma.clone(),
            output_black: self.output_black.clone(),
            output_white: self.output_white.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

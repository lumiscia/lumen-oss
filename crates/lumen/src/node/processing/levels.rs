use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("levels.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "levels",
    label = "Levels",
    description = "Remaps raster black, white, gamma, and output range.",
    category = "processing"
)]
pub struct Levels {
    pub id: NodeId,
    #[property(kind = "float")]
    pub black_point: NodeProperty,
    #[property(kind = "float")]
    pub white_point: NodeProperty,
    #[property(kind = "float")]
    pub gamma: NodeProperty,
    #[property(kind = "float")]
    pub output_black: NodeProperty,
    #[property(kind = "float")]
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
        ctx.push_frame_binding(FrameBinding::Levels {
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

impl GpuFrameBindNode for Levels {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Levels {
            node_id,
            black_point,
            white_point,
            gamma,
            output_black,
            output_white,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::LevelsParams {
            black_point: black_point.resolve_float(
                *node_id,
                "black_point",
                &ctx.expr_context(*node_id, "black_point"),
            )? as f32,
            white_point: white_point.resolve_float(
                *node_id,
                "white_point",
                &ctx.expr_context(*node_id, "white_point"),
            )? as f32,
            gamma: gamma.resolve_float(*node_id, "gamma", &ctx.expr_context(*node_id, "gamma"))?
                as f32,
            output_black: output_black.resolve_float(
                *node_id,
                "output_black",
                &ctx.expr_context(*node_id, "output_black"),
            )? as f32,
            output_white: output_white.resolve_float(
                *node_id,
                "output_white",
                &ctx.expr_context(*node_id, "output_white"),
            )? as f32,
            _pad: [0.0; 3],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

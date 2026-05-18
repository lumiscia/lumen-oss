use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("alpha_premultiply.wgsl");

/// Converts raster alpha between premultiplied and unpremultiplied representations.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "alpha_premultiply",
    name = "Alpha Premultiply",
    category = "processing"
)]
pub struct AlphaPremultiply {
    pub id: NodeId,
    /// Alpha conversion mode.
    #[property(kind = "string", format = "alpha_premultiply_mode")]
    pub mode: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for AlphaPremultiply {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            mode: NodeProperty::String("premultiply".to_string()),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct AlphaPremultiplyFrameBinding {
    node_id: NodeId,
    mode: NodeProperty,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for AlphaPremultiplyFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let mode = self.mode.resolve_string(
            self.node_id,
            "mode",
            &ctx.expr_context(self.node_id, "mode"),
        )?;
        let params = compiler::AlphaPremultiplyParams {
            values: [
                compiler::alpha_operation(self.node_id, &mode)?,
                0.0,
                0.0,
                0.0,
            ],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

impl GpuCompileNode for AlphaPremultiply {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let metadata = match &self.mode {
            NodeProperty::String(mode) if compiler::alpha_operation(self.id, mode)? < 0.5 => {
                Some(AlphaMode::Premultiplied)
            }
            NodeProperty::String(mode) if compiler::alpha_operation(self.id, mode)? >= 0.5 => {
                Some(AlphaMode::Unpremultiplied)
            }
            _ => None,
        };
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "alpha-premultiply",
            SHADER,
            std::mem::size_of::<compiler::AlphaPremultiplyParams>() as u64,
        )?;
        ctx.push_frame_binding(AlphaPremultiplyFrameBinding {
            node_id: self.id,
            mode: self.mode.clone(),
            buffer: params,
        });

        let mut output_metadata = source.metadata;
        if let Some(alpha_mode) = metadata {
            output_metadata.alpha_mode = alpha_mode;
        }
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: output_metadata,
        }))
    }
}

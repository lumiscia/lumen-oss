use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    AlphaMode, BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode,
    GpuFrameBindNode, RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("alpha_premultiply.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "alpha_premultiply",
    label = "Alpha Premultiply",
    description = "Converts raster alpha between premultiplied and unpremultiplied representations.",
    category = "processing"
)]
pub struct AlphaPremultiply {
    pub id: NodeId,
    #[property(kind = "string")]
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
        ctx.push_frame_binding(FrameBinding::AlphaPremultiply {
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

impl GpuFrameBindNode for AlphaPremultiply {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::AlphaPremultiply {
            node_id,
            mode,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let mode = mode.resolve_string(*node_id, "mode", &ctx.expr_context(*node_id, "mode"))?;
        let params = compiler::AlphaPremultiplyParams {
            operation: compiler::alpha_operation(*node_id, &mode)?,
            _pad: [0.0; 3],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

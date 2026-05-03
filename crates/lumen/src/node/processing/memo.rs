use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "memo",
    label = "Memo",
    description = "Aliases a raster input through a stable cache boundary.",
    category = "processing"
)]
pub struct Memo {
    pub id: NodeId,
    #[property(kind = "string")]
    pub cache_id: NodeProperty,
    #[property(kind = "bool")]
    pub allow_expressions: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Memo {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            cache_id: NodeProperty::String(String::new()),
            allow_expressions: NodeProperty::Bool(false),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Memo {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }
        let source = ctx.compile_port(&self.source)?;
        ctx.push_frame_binding(FrameBinding::Memo {
            node_id: self.id,
            cache_id: self.cache_id.clone(),
            allow_expressions: self.allow_expressions.clone(),
        });
        Ok(source)
    }
}

impl GpuFrameBindNode for Memo {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        _bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Memo {
            node_id,
            cache_id,
            allow_expressions,
        } = binding
        else {
            return Ok(());
        };
        let _ = cache_id.resolve_string(
            *node_id,
            "cache_id",
            &ctx.expr_context(*node_id, "cache_id"),
        )?;
        let _ = allow_expressions.resolve_bool(
            *node_id,
            "allow_expressions",
            &ctx.expr_context(*node_id, "allow_expressions"),
        )?;
        Ok(())
    }
}

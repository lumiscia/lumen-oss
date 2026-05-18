use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};

/// Aliases a raster input through a stable cache boundary.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "memo", name = "Memo", category = "processing")]
pub struct Memo {
    pub id: NodeId,
    /// Stable cache identifier for this memo boundary.
    #[property(kind = "string", name = "Cache key", role = "cache_id")]
    pub cache_id: NodeProperty,
    /// Allows property expressions to be evaluated across this memo boundary.
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
        ctx.push_frame_binding(MemoFrameBinding {
            node_id: self.id,
            cache_id: self.cache_id.clone(),
            allow_expressions: self.allow_expressions.clone(),
        });
        Ok(source)
    }
}

#[derive(Debug, Clone)]
struct MemoFrameBinding {
    node_id: NodeId,
    cache_id: NodeProperty,
    allow_expressions: NodeProperty,
}

impl GpuFrameBinding for MemoFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, _bound: &mut BoundFrame) -> crate::Result<()> {
        let _ = self.cache_id.resolve_string(
            self.node_id,
            "cache_id",
            &ctx.expr_context(self.node_id, "cache_id"),
        )?;
        let _ = self.allow_expressions.resolve_bool(
            self.node_id,
            "allow_expressions",
            &ctx.expr_context(self.node_id, "allow_expressions"),
        )?;
        Ok(())
    }
}

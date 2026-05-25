use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};

/// Aliases a raster input through a stable cache boundary.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct MemoParams {
    /// Stable cache identifier for this memo boundary.
    #[meta(name = "Cache key", role = "cache_id")]
    pub cache_id: String,
    /// Allows property expressions to be evaluated across this memo boundary.
    #[meta()]
    pub allow_expressions: bool,
}

/// Aliases a raster input through a stable cache boundary.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "memo", name = "Memo", category = "processing")]
pub struct Memo {
    pub id: NodeId,
    #[params]
    pub params: MemoParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Memo {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: MemoParamsDelegate::default(),
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
        ctx.register_compiled_node(CompiledMemo {
            node_id: self.id,
            params: self.params.clone(),
        });
        Ok(source)
    }
}

#[derive(Debug, Clone)]
struct CompiledMemo {
    node_id: NodeId,
    params: MemoParamsDelegate,
}

impl GpuCompiledNode for CompiledMemo {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, _bound: &mut BoundFrame) -> crate::Result<()> {
        let _ = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        Ok(())
    }
}

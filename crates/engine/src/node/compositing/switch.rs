use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::{GpuCompileNode, RasterHandle};

/// Selects one raster input according to a controlled layer index.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct SwitchParams {
    /// Zero-based input index to route to the output.
    #[meta(name = "Selected layer", min = 0, step = 1)]
    pub selected_layer: i64,
}

/// Selects one raster input according to a controlled layer index.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "switch", name = "Switch", category = "compositing")]
pub struct Switch {
    pub id: NodeId,
    #[params]
    pub params: SwitchParamsDelegate,

    #[input(optional, variadic)]
    pub layers: Vec<NodeId>,
}

pub fn selected_layer_for_frame(
    node: &Switch,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<Option<usize>> {
    let params = node.params.eval(&DelegateEvalContext {
        node_id: node.id,
        expr: ctx,
    })?;
    Ok((params.selected_layer >= 0).then_some(params.selected_layer as usize))
}

impl GpuCompileNode for Switch {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let selected_layer = selected_layer_for_frame(self, &ctx.expr_context())?;
        let Some(index) = selected_layer else {
            return Ok(ctx.compile_transparent(self.id));
        };
        let Some(layer) = self.layers.get(index) else {
            return Ok(ctx.compile_transparent(self.id));
        };
        ctx.compile_bounded_port(*layer)
    }
}

impl crate::gpu::frame_graph::RecordNode for Switch {
    fn programs(&self, _: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let selected = selected_layer_for_frame(self, &cx.expr())?;
        match selected.and_then(|index| self.layers.get(index)) {
            Some(&layer) => cx.input(layer),
            None => Ok(cx.transparent(cx.canvas_size())),
        }
    }
}

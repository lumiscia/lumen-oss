use crate::node::{NodeId, NodeParams, PortRef};

use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding};

/// Selects one raster input according to a controlled layer index.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct SwitchParams {
    /// Zero-based input index to route to the output.
    #[meta(name = "Selected layer", min = 0, step = 1)]
    pub selected_layer: i64,
}

/// Selects one raster input according to a controlled layer index.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "switch", name = "Switch", category = "compositing")]
pub struct Switch {
    pub id: NodeId,
    #[params]
    pub params: SwitchParamsDelegate,

    #[input(optional, variadic)]
    pub layers: Vec<PortRef>,
}

impl Default for Switch {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: SwitchParamsDelegate::default(),
            layers: Vec::new(),
        }
    }
}

pub fn selected_layer_for_frame(
    node: &Switch,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<Option<usize>> {
    let selected = node
        .params
        .selected_layer
        .resolve_int(node.id, "selected_layer", ctx)?;
    Ok((selected >= 0).then_some(selected as usize))
}

impl GpuCompileNode for Switch {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let selected_layer =
            selected_layer_for_frame(self, &ctx.expr_context(self.id, "selected_layer"))?;
        ctx.push_frame_binding(SwitchFrameBinding {
            node_id: self.id,
            selected_layer,
        });

        let Some(index) = selected_layer else {
            return Ok(ctx.compile_transparent(self.id));
        };
        let Some(layer) = self.layers.get(index) else {
            return Ok(ctx.compile_transparent(self.id));
        };
        if layer.is_empty() {
            return Ok(ctx.compile_transparent(self.id));
        }
        ctx.compile_port(layer)
    }
}

#[derive(Debug, Clone)]
struct SwitchFrameBinding {
    node_id: NodeId,
    selected_layer: Option<usize>,
}

impl GpuFrameBinding for SwitchFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, _ctx: &FrameBindContext<'_>, _bound: &mut BoundFrame) -> crate::Result<()> {
        let _ = self.selected_layer;
        Ok(())
    }
}

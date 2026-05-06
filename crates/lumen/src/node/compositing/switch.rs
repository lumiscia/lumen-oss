use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
};

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "switch",
    label = "Switch",
    description = "Selects one raster input according to a controlled layer index.",
    category = "compositing"
)]
pub struct Switch {
    pub id: NodeId,
    #[property(kind = "int")]
    pub selected_layer: NodeProperty,
    #[input(optional, variadic)]
    pub layers: Vec<PortRef>,
}

impl Default for Switch {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            selected_layer: NodeProperty::Int(0),
            layers: Vec::new(),
        }
    }
}

pub fn selected_layer_for_frame(
    node: &Switch,
    ctx: &crate::expr::ExpressionContext<'_>,
) -> crate::Result<Option<usize>> {
    let selected = node
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
        ctx.push_frame_binding(FrameBinding::Switch {
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

impl GpuFrameBindNode for Switch {
    fn bind_gpu_frame(
        &self,
        _ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        _bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Switch { .. } = binding else {
            return Ok(());
        };
        Ok(())
    }
}

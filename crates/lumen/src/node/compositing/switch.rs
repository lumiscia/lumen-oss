use std::{collections::HashMap, ops::Range};

use crate::node::{NodeId, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    compiler,
};

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "switch",
    label = "Switch",
    description = "Selects one raster input according to configured frame ranges.",
    category = "compositing"
)]
pub struct Switch {
    pub id: NodeId,
    #[input(optional, variadic)]
    pub layers: Vec<PortRef>,
    pub map: HashMap<u16, Range<u32>>,
}

impl Default for Switch {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            layers: Vec::new(),
            map: HashMap::new(),
        }
    }
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

        let selected_layer = compiler::selected_switch_layer(self, 0);
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
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        if let FrameBinding::SolidColor {
            node_id,
            color,
            buffer,
        } = binding
        {
            let color =
                color.resolve_color(*node_id, "color", &ctx.expr_context(*node_id, "color"))?;
            bound.write_buffer(
                *buffer,
                0,
                bytemuck::bytes_of(&compiler::ColorParams::from_rgba8(color)),
            );
        }
        Ok(())
    }
}

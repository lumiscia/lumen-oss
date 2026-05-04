use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("blur.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "blur",
    label = "Blur",
    description = "Applies a simple box blur to a raster.",
    category = "processing"
)]
pub struct Blur {
    pub id: NodeId,
    #[property(kind = "float")]
    pub radius: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Blur {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            radius: NodeProperty::Float(4.0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Blur {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "blur",
            SHADER,
            std::mem::size_of::<compiler::BlurParams>() as u64,
        )?;
        ctx.push_frame_binding(FrameBinding::Blur {
            node_id: self.id,
            radius: self.radius.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Blur {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Blur {
            node_id,
            radius,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let radius = radius
            .resolve_float(*node_id, "radius", &ctx.expr_context(*node_id, "radius"))?
            .round()
            .clamp(0.0, 32.0) as u32;
        let params = compiler::BlurParams {
            values: [radius, 0, 0, 0],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

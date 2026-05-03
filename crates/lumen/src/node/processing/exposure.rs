use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("exposure.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "exposure",
    label = "Exposure",
    description = "Adjusts raster exposure, contrast, and offset.",
    category = "processing"
)]
pub struct Exposure {
    pub id: NodeId,
    #[property(kind = "float")]
    pub exposure: NodeProperty,
    #[property(kind = "float")]
    pub contrast: NodeProperty,
    #[property(kind = "float")]
    pub offset: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Exposure {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            exposure: NodeProperty::Float(0.0),
            contrast: NodeProperty::Float(1.0),
            offset: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Exposure {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "exposure",
            SHADER,
            std::mem::size_of::<compiler::ExposureParams>() as u64,
        )?;
        ctx.push_frame_binding(FrameBinding::Exposure {
            node_id: self.id,
            exposure: self.exposure.clone(),
            contrast: self.contrast.clone(),
            offset: self.offset.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Exposure {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Exposure {
            node_id,
            exposure,
            contrast,
            offset,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::ExposureParams {
            exposure: exposure.resolve_float(
                *node_id,
                "exposure",
                &ctx.expr_context(*node_id, "exposure"),
            )? as f32,
            contrast: contrast.resolve_float(
                *node_id,
                "contrast",
                &ctx.expr_context(*node_id, "contrast"),
            )? as f32,
            offset: offset.resolve_float(
                *node_id,
                "offset",
                &ctx.expr_context(*node_id, "offset"),
            )? as f32,
            _pad: 0.0,
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

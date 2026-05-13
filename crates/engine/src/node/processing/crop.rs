use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("crop.wgsl");

/// Extracts a fixed raster region into static output bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "crop", name = "Crop", category = "processing")]
pub struct Crop {
    pub id: NodeId,
    /// Left edge of the crop region in pixels.
    #[property(kind = "int", step = 1)]
    pub x: NodeProperty,
    /// Top edge of the crop region in pixels.
    #[property(kind = "int", step = 1)]
    pub y: NodeProperty,
    /// Width of the crop region in pixels.
    #[property(kind = "int", min = 0, step = 1)]
    pub width: NodeProperty,
    /// Height of the crop region in pixels.
    #[property(kind = "int", min = 0, step = 1)]
    pub height: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for Crop {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            x: NodeProperty::Int(0),
            y: NodeProperty::Int(0),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Crop {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let source = ctx
            .compile_port(&self.source)?
            .into_raster(self.source.id, &self.source.port)?;
        let size = lumen_gpu::Size::new(
            ctx.static_dimension(&self.width, self.id, "width")?,
            ctx.static_dimension(&self.height, self.id, "height")?,
        );
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("crop:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("crop:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::CropParams>() as u64),
        );
        let program = ctx.spatial_program(self.id, "crop", SHADER);
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("crop:{}:apply", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: compiler::spatial_bindings(source.texture, params, texture),
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(FrameBinding::Crop {
            node_id: self.id,
            x: self.x.clone(),
            y: self.y.clone(),
            width: self.width.clone(),
            height: self.height.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

impl GpuFrameBindNode for Crop {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::Crop {
            node_id,
            x,
            y,
            width,
            height,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let params = compiler::CropParams {
            origin: [
                x.resolve_int(*node_id, "x", &ctx.expr_context(*node_id, "x"))? as i32,
                y.resolve_int(*node_id, "y", &ctx.expr_context(*node_id, "y"))? as i32,
            ],
            size: [
                width
                    .resolve_int(*node_id, "width", &ctx.expr_context(*node_id, "width"))?
                    .max(0) as u32,
                height
                    .resolve_int(*node_id, "height", &ctx.expr_context(*node_id, "height"))?
                    .max(0) as u32,
            ],
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

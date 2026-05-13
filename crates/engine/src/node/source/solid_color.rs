use crate::node::{NodeId, NodeProperty};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, RasterMetadata, compiler,
};

pub(crate) const SHADER: &str = include_str!("solid_color.wgsl");

/// Generates a solid raster texture.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "solid_color", name = "Solid Color", category = "source")]
pub struct SolidColor {
    pub id: NodeId,
    /// Fill color.
    #[property(kind = "color")]
    pub color: NodeProperty,
    /// Output width in pixels. Use 0 to match the composition width.
    #[property(kind = "int", min = 0, step = 1)]
    pub width: NodeProperty,
    /// Output height in pixels. Use 0 to match the composition height.
    #[property(kind = "int", min = 0, step = 1)]
    pub height: NodeProperty,
}

impl Default for SolidColor {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            color: NodeProperty::Color([0, 0, 0, 255]),
            width: NodeProperty::Int(0),
            height: NodeProperty::Int(0),
        }
    }
}

impl GpuCompileNode for SolidColor {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &crate::node::PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(ctx.missing_output(self.id, &port.port));
        }

        let width = ctx.static_dimension(&self.width, self.id, "width")?;
        let height = ctx.static_dimension(&self.height, self.id, "height")?;
        let size = lumen_gpu::Size::new(width, height);
        let texture = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("solid-color:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(self.id.0),
            Some(format!("solid-color:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::ColorParams>() as u64),
        );
        let program = ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(self.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("solid-color".to_string()),
                shader: SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("solid-color:{}:fill", self.id.0)),
            owner: Some(lumen_gpu::NodeKey(self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::uniform(0, 0, params),
                lumen_gpu::Binding::storage_texture(0, 1, texture),
            ],
            dispatch: compiler::dispatch_for(size).into(),
        });
        ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(self.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        ctx.push_frame_binding(FrameBinding::SolidColor {
            node_id: self.id,
            color: self.color.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }
}

impl GpuFrameBindNode for SolidColor {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::SolidColor {
            node_id,
            color,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let color = color.resolve_color(*node_id, "color", &ctx.expr_context(*node_id, "color"))?;
        bound.write_buffer(
            *buffer,
            0,
            bytemuck::bytes_of(&compiler::ColorParams::from_rgba8(color)),
        );
        Ok(())
    }
}

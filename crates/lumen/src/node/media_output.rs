use crate::node::{NodeId, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("media_output.wgsl");
pub(crate) const RENDER_SHADER: &str = include_str!("media_output_render.wgsl");

/// Copies the compiled raster into the final composition output.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "media_output", name = "Media Output", category = "output")]
pub struct MediaOutput {
    pub id: NodeId,
    #[input()]
    pub source: PortRef,
}

impl Default for MediaOutput {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for MediaOutput {
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
            ctx.composition().render_settings.width.max(1),
            ctx.composition().render_settings.height.max(1),
        );
        let output_format = ctx.output_format();
        let output = ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(self.id.0),
            Some("media-output:final".to_string()),
            media_output_texture_desc(size, output_format),
        );
        if output_format == lumen_gpu::wgpu::TextureFormat::Rgba8Unorm {
            let program = ctx.builder_mut().program_for(
                lumen_gpu::NodeKey(self.id.0),
                lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                    label: Some("media-output".to_string()),
                    shader: SHADER.to_string(),
                    entry: "cs_main".to_string(),
                    bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                        lumen_gpu::BindingLayoutEntry::texture(
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
                label: Some("media-output:copy".to_string()),
                owner: Some(lumen_gpu::NodeKey(self.id.0)),
                program,
                bindings: vec![
                    lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                    lumen_gpu::Binding::storage_texture(0, 1, output),
                ],
                dispatch: compiler::dispatch_for(size).into(),
            });
        } else {
            let sampler = ctx.builder_mut().sampler(
                Some("media-output:sampler".to_string()),
                lumen_gpu::wgpu::SamplerDescriptor {
                    address_mode_u: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                    address_mode_v: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                    address_mode_w: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                    mag_filter: lumen_gpu::wgpu::FilterMode::Nearest,
                    min_filter: lumen_gpu::wgpu::FilterMode::Nearest,
                    mipmap_filter: lumen_gpu::wgpu::MipmapFilterMode::Nearest,
                    ..Default::default()
                },
            );
            let program = ctx.builder_mut().program_for(
                lumen_gpu::NodeKey(self.id.0),
                lumen_gpu::ProgramDesc::Render(lumen_gpu::RenderProgramDesc {
                    label: Some("media-output".to_string()),
                    shader: RENDER_SHADER.to_string(),
                    vertex_entry: "vs_main".to_string(),
                    fragment_entry: "fs_main".to_string(),
                    bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                        lumen_gpu::BindingLayoutEntry::texture(
                            0,
                            lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                        ),
                        lumen_gpu::BindingLayoutEntry::sampler(
                            1,
                            lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                        ),
                    ]),
                    targets: vec![Some(lumen_gpu::wgpu::ColorTargetState {
                        format: output_format,
                        blend: Some(lumen_gpu::wgpu::BlendState::REPLACE),
                        write_mask: lumen_gpu::wgpu::ColorWrites::ALL,
                    })],
                    vertex_buffers: Vec::new(),
                    primitive: lumen_gpu::wgpu::PrimitiveState::default(),
                }),
            );
            ctx.builder_mut().render_pass(lumen_gpu::RenderPassDesc {
                label: Some("media-output:render".to_string()),
                owner: Some(lumen_gpu::NodeKey(self.id.0)),
                program,
                targets: vec![lumen_gpu::RenderTargetRef {
                    texture: output,
                    load: lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
                    store: lumen_gpu::wgpu::StoreOp::Store,
                }],
                bindings: vec![
                    lumen_gpu::Binding::sampled_texture(0, 0, source.texture),
                    lumen_gpu::Binding::sampler(0, 1, sampler),
                ],
                vertex_buffers: Vec::new(),
                index_buffer: None,
                draw: lumen_gpu::DrawCommand::Draw(lumen_gpu::Draw {
                    vertices: 0..3,
                    instances: 0..1,
                }),
                scissor: None,
            });
        }

        Ok(CompiledOutput::Raster(RasterHandle {
            texture: output,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: source.metadata,
        }))
    }
}

fn media_output_texture_desc(
    size: lumen_gpu::Size,
    format: lumen_gpu::wgpu::TextureFormat,
) -> lumen_gpu::TextureDesc {
    if format == lumen_gpu::wgpu::TextureFormat::Rgba8Unorm {
        compiler::copyable_texture_desc(size)
    } else {
        lumen_gpu::TextureDesc {
            domain: lumen_gpu::TextureDomain::full_frame(size),
            format,
            usage: lumen_gpu::wgpu::TextureUsages::COPY_SRC
                | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
                | lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT,
        }
    }
}

impl GpuFrameBindNode for MediaOutput {
    fn bind_gpu_frame(
        &self,
        _ctx: &FrameBindContext<'_>,
        _binding: &FrameBinding,
        _bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        Ok(())
    }
}
